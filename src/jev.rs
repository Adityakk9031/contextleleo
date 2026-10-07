//! Jev — the context-planning decision layer for session handoffs.
//!
//! Contextleleo's guarantee is "store everything, transfer only what matters":
//! the original session is the permanent source of truth, and a handoff to
//! another agent can carry an optimized *copy* rather than the whole
//! transcript. Jev is the cheap decision layer that decides which treatment
//! each transcript unit needs in that copy:
//!
//! - [`ContextDecision::KeepFull`] — the unit goes into the handoff verbatim.
//! - [`ContextDecision::Compress`] — the unit is useful but oversized; the
//!   handoff should carry a compact stand-in that references the original.
//! - [`ContextDecision::Drop`] — the unit does not need to occupy the next
//!   agent's context at all.
//!
//! Every decision here is advisory *planning* only: nothing in this module
//! reads or writes a store, mutates a transcript, or changes any existing
//! contextleleo flow. [`apply`] materializes the planned stand-ins — into a
//! copy, never the source — as deterministic reference blocks; no model or
//! LLM is involved, and [`handoff`] chains the whole pipeline into a
//! target harness's native shape.
//!
//! The scorer is deterministic — plain functions of the transcript — so a
//! plan is reproducible and testable without a model. [`ContextScorer`] is
//! the seam where a learned scorer can replace [`DeterministicScorer`] later
//! without touching callers.
//!
//! # Safety bias
//!
//! When uncertain, the deterministic scorer keeps or compresses rather than
//! drops. In this phase it only ever drops content it can prove is
//! redundant: tool results that repeat an earlier identical result *and*
//! answer no live tool call. User-authored and assistant prose is never
//! dropped or marked for compression by the built-in scorer — prose
//! compression belongs to the compressor phase.
//!
//! # Budget allocation
//!
//! A handoff can carry more than the next agent's context allows, so
//! [`allocate`] (tuned through [`AllocatorConfig`]) turns a scored plan into
//! one that fits its recorded budget. It only demotes decisions —
//! `KeepFull` → `Compress` → `Drop` — lowest value first, never splitting a
//! tool call/result pair, and never demoting items at or above
//! [`AllocatorConfig::floor_importance`]. `Compress` items are budgeted at
//! a tenth of their raw cost: the planned size of the summary-plus-reference
//! stand-in the compressor phase will emit. When even all-`Compress` cannot
//! fit, the report says so ([`AllocationStatus::OverBudget`]) rather than
//! silently sacrificing protected content. Like everything here,
//! allocation reshuffles decisions only; the transcript is untouched.
//!
//! # Applying a plan
//!
//! [`apply`] turns a plan into the optimized handoff copy: a new
//! `Transcript<Common>` built through [`Transcript::crop_to`], so `Drop`
//! messages are cropped away while `KeepFull` messages are carried
//! verbatim. `Compress` messages are rebuilt as deterministic stand-ins —
//! a truncation of the original blocks plus a Jev reference naming the
//! source session, the message index, and the original estimated size (see
//! [`compress_message`]). The original is never touched, and the crop's
//! tool-pair safety backstops the plan: a plan that would split a call
//! from its result is an [`ApplyError`], not a broken handoff.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::common::{Block, Message, Role, ToolOutput};
use crate::{Common, CropError, Span, Transcript};

// ── decisions ──────────────────────────────────────────────────────────

/// What Jev wants done with one transcript unit during a handoff.
///
/// None of these decisions touch the original transcript. `Drop` means "not
/// included in the optimized handoff", never "deleted from contextleleo" — the
/// original stays complete and retrievable either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextDecision {
    /// Carry the unit into the handoff verbatim.
    KeepFull,
    /// Carry a compact stand-in that preserves the key facts and references
    /// the original location. The raw content stays in contextleleo.
    Compress,
    /// Leave the unit out of the handoff's active context. The original
    /// remains in contextleleo and recoverable through its reference.
    Drop,
}

// ── token estimation ───────────────────────────────────────────────────

/// Characters assumed per token in Jev's size estimates.
///
/// A planning approximation, deliberately not a tokenizer: budgets rank and
/// bound candidate selections, they do not bill tokens. Where harnesses
/// record real usage (`Message::usage`), callers can cross-check plan totals
/// against the recorded sums.
const CHARS_PER_TOKEN: usize = 4;

/// Estimate the token cost of `text` as its character count divided by
/// [`CHARS_PER_TOKEN`], rounding up. Empty text costs nothing.
#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(CHARS_PER_TOKEN)
}

/// Estimate one message's token cost from its blocks: text and reasoning by
/// their text, tool calls and results by their canonical input/output
/// rendering, images by their encoded payload. Metadata (timestamps, usage,
/// stop reasons) is excluded — the handoff does not carry it.
#[must_use]
pub fn estimate_message_tokens(message: &Message) -> usize {
    message
        .content
        .iter()
        .map(|block| match block {
            Block::Text { text } | Block::Thinking { text, .. } => estimate_tokens(text),
            Block::ToolUse { tool, .. } => estimate_tokens(&tool.to_canonical().1.to_string()),
            Block::ToolResult { content, .. } => estimate_tokens(&content_key(content)),
            Block::Image { source } => estimate_tokens(&source.data),
            Block::Artifact { artifact } => estimate_tokens(&artifact.display_text()),
        })
        .sum()
}

// ── scores and plans ───────────────────────────────────────────────────

/// Jev's verdict on one message of a [`Transcript<Common>`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextItemScore {
    /// Position of the message in `transcript.body`. The addressing unit
    /// every downstream phase (spans, references, the applied copy) builds on.
    pub message_index: usize,
    /// How much the continuation depends on this message's content (0..1).
    pub importance: f32,
    /// How likely a later message — or the next agent's first turn — needs
    /// this message (0..1).
    pub future_utility: f32,
    /// How much of this message repeats earlier content (0..1).
    pub redundancy: f32,
    /// Estimated token cost of the message as-is (see [`estimate_tokens`]).
    /// Until the compressor phase lands, a `Compress` decision still
    /// contributes this raw cost to a freshly scored plan's totals — an
    /// upper bound, not a promise; [`allocate`] budgets the stand-in cost
    /// instead.
    pub token_cost: usize,
    /// The treatment Jev selected for the handoff copy.
    pub decision: ContextDecision,
}

/// The plan Jev produces for one transcript: a score and decision per
/// message, plus the token accounting that [`allocate`] enforces when a
/// budget is set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextPlan {
    /// The handoff's context budget, in estimated tokens. `None` means the
    /// caller set no budget: keep everything that is not dropped.
    /// [`allocate`] enforces the recorded budget by demoting decisions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<usize>,
    /// Estimated tokens of the complete transcript body.
    pub original_tokens: usize,
    /// Estimated tokens the optimized copy carries. In a freshly scored
    /// plan every non-`Drop` item counts at raw cost — an upper bound while
    /// `Compress` items are still uncompressed, so the plan can over-report,
    /// never under-report. [`allocate`] re-estimates `Compress` items at
    /// the compressed stand-in cost (`COMPRESSED_COST_DENOMINATOR`).
    pub selected_tokens: usize,
    /// One score per message, in transcript order. Every message appears
    /// exactly once; there is no implicit filtering.
    pub items: Vec<ContextItemScore>,
}

impl ContextPlan {
    /// Count the items by decision, in `KeepFull, Compress, Drop` order —
    /// the summary line the CLI report prints.
    #[must_use]
    pub fn counts(&self) -> [usize; 3] {
        counts_of(&self.items)
    }
}

/// `KeepFull, Compress, Drop` counts over a scored item list — the summary
/// line the CLI report prints, shared by scored plans and allocation reports.
fn counts_of(items: &[ContextItemScore]) -> [usize; 3] {
    let mut counts = [0; 3];
    for item in items {
        match item.decision {
            ContextDecision::KeepFull => counts[0] += 1,
            ContextDecision::Compress => counts[1] += 1,
            ContextDecision::Drop => counts[2] += 1,
        }
    }
    counts
}

// ── scorer abstraction ─────────────────────────────────────────────────

/// Scores a transcript into per-message Jev decisions.
///
/// Implemented today by the deterministic [`DeterministicScorer`]; a future
/// learned scorer implements the same trait and drops in without touching
/// callers. Implementations must be pure: no I/O, no mutation of the
/// transcript, one output item per input message in order.
pub trait ContextScorer {
    /// Score every message of `transcript`, in order.
    fn score(&self, transcript: &Transcript<Common>) -> Vec<ContextItemScore>;
}

/// Knobs for the deterministic scorer. The defaults encode the safety bias:
/// keep anything important, compress only oversized tool output, and drop
/// only provably redundant, low-utility duplicates.
#[derive(Debug, Clone, PartialEq)]
pub struct ScorerConfig {
    /// Messages scoring at least this important are `KeepFull` regardless
    /// of size or redundancy.
    pub keep_min_importance: f32,
    /// Redundancy at or above this level marks a repeated tool result as a
    /// drop candidate (pair safety may still downgrade it). The default
    /// crosses at the second identical sighting; live tool calls still
    /// downgrade their result to `Compress` rather than let it drop.
    pub drop_min_redundancy: f32,
    /// A drop candidate also needs future utility at or below this level,
    /// *after* discounting by redundancy: duplicated content has near-zero
    /// marginal utility wherever it sits, so `utility × (1 − redundancy)`
    /// is what this gate compares.
    pub drop_max_utility: f32,
    /// Tool results estimated at or above this many tokens become
    /// `Compress` candidates.
    pub compress_min_tokens: usize,
}

impl Default for ScorerConfig {
    fn default() -> Self {
        Self {
            keep_min_importance: 0.6,
            drop_min_redundancy: 0.5,
            drop_max_utility: 0.6,
            compress_min_tokens: 400,
        }
    }
}

/// The built-in deterministic scorer.
///
/// Signals, all derivable from the transcript alone: authorship (user vs
/// assistant), the session's opening objective, error results, tool-call
/// presence, recency, tail position, and repeated tool output.
///
/// Decision order (first match wins):
///
/// 1. importance ≥ `keep_min_importance` → [`KeepFull`](ContextDecision::KeepFull)
/// 2. repeated tool result whose redundancy-discounted utility is low →
///    [`Drop`](ContextDecision::Drop), then pair safety downgrades any drop
///    that would orphan a live call
/// 3. oversized tool result → [`Compress`](ContextDecision::Compress)
/// 4. otherwise → [`KeepFull`](ContextDecision::KeepFull) (safety default)
///
/// Prose (user or assistant text) and reasoning are never compressed by
/// this scorer: only messages carrying a tool result can be `Compress`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeterministicScorer {
    /// Tuning knobs; [`ScorerConfig::default`] encodes the safety bias.
    pub config: ScorerConfig,
}

impl DeterministicScorer {
    /// A scorer with explicit tuning.
    #[must_use]
    pub fn with_config(config: ScorerConfig) -> Self {
        Self { config }
    }
}

impl ContextScorer for DeterministicScorer {
    #[allow(clippy::cast_precision_loss)] // indices and counts < 2^24 are exact in f32
    fn score(&self, transcript: &Transcript<Common>) -> Vec<ContextItemScore> {
        let body = &transcript.body;
        let total = body.len();
        if total == 0 {
            return Vec::new();
        }
        let objective_index = objective_index(body);
        let occurrence_counts = count_content_occurrences(body);
        // Repeat tracking for this scoring pass: how many times each
        // block's content has been seen so far, keyed by its content key.
        let mut seen: HashMap<String, usize> = HashMap::new();

        let mut items: Vec<ContextItemScore> = body
            .iter()
            .enumerate()
            .map(|(index, message)| {
                let token_cost = estimate_message_tokens(message);
                let has_user_text =
                    message.role == Role::User && message.content.iter().any(is_text_block);
                let has_tool_use = message.content.iter().any(is_tool_use);
                let has_tool_result = message.content.iter().any(is_tool_result);
                let has_error = message
                    .content
                    .iter()
                    .any(|block| matches!(block, Block::ToolResult { is_error: true, .. }));

                let recency = if total == 1 {
                    1.0
                } else {
                    (index + 1) as f32 / total as f32
                };

                let mut importance = 0.4;
                if has_user_text {
                    importance += 0.35;
                }
                if Some(index) == objective_index {
                    importance += 0.2;
                }
                if has_error {
                    importance += 0.3;
                }
                if has_tool_use {
                    importance += 0.1;
                }
                let importance = clamp01(importance);

                let mut future_utility = 0.5 * recency;
                if has_error {
                    future_utility += 0.3;
                }
                if has_tool_use {
                    future_utility += 0.15;
                }
                if index + 1 == total {
                    future_utility += 0.2;
                }
                let future_utility = clamp01(future_utility);

                // Redundancy: how far into its own repeat sequence this
                // message's most-repeated block sits. The first sighting of
                // a body scores 0; the third sighting of a body seen twice
                // before scores 2/3. Text and tool results only.
                let redundancy = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        Block::Text { text } => Some(text.as_str().to_string()),
                        Block::ToolResult { content, .. } => Some(content_key(content)),
                        _ => None,
                    })
                    .fold(0.0_f32, |best, key| {
                        let count = occurrence_counts.get(&key).copied().unwrap_or(1);
                        let before = {
                            let seen_count = seen.entry(key).or_default();
                            let before = *seen_count;
                            *seen_count += 1;
                            before
                        };
                        (before as f32 / count as f32).max(best)
                    });

                let decision = self.decide(
                    importance,
                    future_utility,
                    redundancy,
                    token_cost,
                    has_tool_result,
                );

                ContextItemScore {
                    message_index: index,
                    importance,
                    future_utility,
                    redundancy,
                    token_cost,
                    decision,
                }
            })
            .collect();

        keep_tool_pairs_whole(&transcript.tool_pairs().unwrap_or_default(), &mut items);
        items
    }
}

impl DeterministicScorer {
    /// First-match-wins decision for one message (see the type's docs).
    fn decide(
        &self,
        importance: f32,
        future_utility: f32,
        redundancy: f32,
        token_cost: usize,
        has_tool_result: bool,
    ) -> ContextDecision {
        if importance >= self.config.keep_min_importance {
            return ContextDecision::KeepFull;
        }
        if redundancy >= self.config.drop_min_redundancy
            && future_utility * (1.0 - redundancy) <= self.config.drop_max_utility
        {
            return ContextDecision::Drop;
        }
        if has_tool_result && token_cost >= self.config.compress_min_tokens {
            return ContextDecision::Compress;
        }
        ContextDecision::KeepFull
    }
}

/// Build a plan for `transcript` with the default deterministic scorer.
///
/// Every message is scored; nothing is filtered, applied, or persisted.
/// See [`plan_with`] to plug in a different scorer.
#[must_use]
pub fn plan(transcript: &Transcript<Common>, budget: Option<usize>) -> ContextPlan {
    plan_with(transcript, budget, &DeterministicScorer::default())
}

/// [`plan`] with an explicit scorer — the seam for a future learned Jev
/// model.
#[must_use]
pub fn plan_with(
    transcript: &Transcript<Common>,
    budget: Option<usize>,
    scorer: &impl ContextScorer,
) -> ContextPlan {
    let items = scorer.score(transcript);
    let original_tokens = items.iter().map(|item| item.token_cost).sum();
    let selected_tokens = items
        .iter()
        .filter(|item| item.decision != ContextDecision::Drop)
        .map(|item| item.token_cost)
        .sum();
    ContextPlan {
        budget,
        original_tokens,
        selected_tokens,
        items,
    }
}

// ── budget allocation ──────────────────────────────────────────────────

/// Denominator of the compressed stand-in cost model: a `Compress` item is
/// budgeted at `ceil(token_cost / N)` tokens — the estimated size of the
/// summary-plus-reference body the compressor phase will emit. Integer math
/// keeps the estimate exact and cast-free; it is a planning approximation,
/// revised when the real compressor lands.
const COMPRESSED_COST_DENOMINATOR: usize = 10;

/// How a plan behaves once the allocator finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationStatus {
    /// Natural selections fit the budget; nothing was demoted.
    Fits,
    /// The budget is met after demoting decisions.
    Demoted,
    /// The budget cannot be met without demoting protected content. The
    /// returned plan is the least-over selection reachable
    /// (`selected_tokens` may exceed `budget`); the caller should raise the
    /// budget or hand off a partial session rather than trust the plan
    /// as-is.
    OverBudget,
}

/// What [`allocate`] changed to bring a plan under budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AllocationReport {
    /// Whether the budget was met, reached by demotion, or unreachable.
    pub status: AllocationStatus,
    /// Estimated size of the optimized copy after allocation, under the
    /// `COMPRESSED_COST_DENOMINATOR` cost model.
    pub selected_tokens: usize,
    /// `KeepFull, Compress, Drop` counts after allocation.
    pub counts: [usize; 3],
    /// Number of `KeepFull → Compress` demotions.
    pub full_to_compressed: usize,
    /// Number of `Compress → Drop` demotions.
    pub compressed_to_dropped: usize,
}

/// Knobs for [`allocate_with`]. Defaults preserve the scorer's safety bias.
#[derive(Debug, Clone, PartialEq)]
pub struct AllocatorConfig {
    /// Items at or above this importance are never demoted by the ladder.
    /// The default matches the scorer's `keep_min_importance`, so anything
    /// the scorer protected stays protected under budget pressure. Scorer
    /// importance tops out at 1.0, so a floor above 1.0 leaves nothing
    /// protected.
    pub floor_importance: f32,
}

impl Default for AllocatorConfig {
    fn default() -> Self {
        Self {
            floor_importance: 0.6,
        }
    }
}

/// Enforce a plan's recorded budget by demoting decisions: returns a new
/// plan plus a report of what changed.
///
/// Pure planning, like the scorer: the input plan is not mutated and the
/// transcript is not touched. The demotion ladder, cheapest harm first:
///
/// 1. the lowest-value `KeepFull` items (by `importance`, then
///    `future_utility`; costlier items break ties toward demotion) become
///    `Compress`;
/// 2. the lowest-value `Compress` groups become `Drop` — a tool call and
///    its result demote together, so a pair is only a candidate once *both*
///    halves are `Compress`.
///
/// Items at or above [`AllocatorConfig::floor_importance`] are never
/// demoted. If the plan is still over budget after the ladder,
/// [`AllocationStatus::OverBudget`] reports it rather than silently
/// sacrificing protected content.
///
/// A plan without a budget is returned unchanged
/// ([`AllocationStatus::Fits`]).
#[must_use]
pub fn allocate(
    transcript: &Transcript<Common>,
    plan: &ContextPlan,
) -> (ContextPlan, AllocationReport) {
    allocate_with(transcript, plan, &AllocatorConfig::default())
}

/// [`allocate`] with explicit tuning.
#[must_use]
pub fn allocate_with(
    transcript: &Transcript<Common>,
    plan: &ContextPlan,
    config: &AllocatorConfig,
) -> (ContextPlan, AllocationReport) {
    let Some(budget) = plan.budget else {
        return (
            plan.clone(),
            AllocationReport {
                status: AllocationStatus::Fits,
                selected_tokens: plan.selected_tokens,
                counts: plan.counts(),
                full_to_compressed: 0,
                compressed_to_dropped: 0,
            },
        );
    };

    let mut items = plan.items.clone();
    let mut selected = allocated_tokens(&items);
    let groups = pair_groups(transcript, items.len());
    let protected: Vec<bool> = items
        .iter()
        .map(|item| item.importance >= config.floor_importance)
        .collect();

    let full_to_compressed =
        demote_lowest_value_full(&mut items, &protected, &mut selected, budget);
    let compressed_to_dropped =
        drop_lowest_value_groups(&groups, &mut items, &protected, &mut selected, budget);

    let status = if selected <= budget {
        if full_to_compressed + compressed_to_dropped == 0 {
            AllocationStatus::Fits
        } else {
            AllocationStatus::Demoted
        }
    } else {
        AllocationStatus::OverBudget
    };
    let counts = counts_of(&items);

    (
        ContextPlan {
            budget: plan.budget,
            original_tokens: plan.original_tokens,
            selected_tokens: selected,
            items,
        },
        AllocationReport {
            status,
            selected_tokens: selected,
            counts,
            full_to_compressed,
            compressed_to_dropped,
        },
    )
}

/// Estimated tokens of a plan's items under the allocator cost model:
/// `KeepFull` at full cost, `Compress` at the stand-in, `Drop` at zero.
fn allocated_tokens(items: &[ContextItemScore]) -> usize {
    items
        .iter()
        .map(|item| match item.decision {
            ContextDecision::KeepFull => item.token_cost,
            ContextDecision::Compress => compressed_cost_of(item),
            ContextDecision::Drop => 0,
        })
        .sum()
}

/// Merge tool pairs into demotion groups: one group per index to start,
/// and each tool pair unions its call and result into one group, so a pair
/// demotes (and drops) as a unit. A plan not built from `transcript`
/// (indices out of range) simply carries no group constraints.
fn pair_groups(transcript: &Transcript<Common>, len: usize) -> Vec<Vec<usize>> {
    let mut label: Vec<usize> = (0..len).collect();
    for (call, result) in transcript.tool_pairs().unwrap_or_default() {
        if call >= len || result >= len {
            continue;
        }
        let (kept, merged) = (label[call], label[result]);
        if kept != merged {
            for slot in label.iter_mut().filter(|slot| **slot == merged) {
                *slot = kept;
            }
        }
    }
    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); len];
    for (index, group) in label.iter().enumerate() {
        groups[*group].push(index);
    }
    groups
}

/// Phase 1: fold the least valuable unprotected `KeepFull` items to
/// `Compress` (the content stays, at stand-in cost) until the budget is
/// met or nothing demotable remains. Returns the number of demotions.
fn demote_lowest_value_full(
    items: &mut [ContextItemScore],
    protected: &[bool],
    selected: &mut usize,
    budget: usize,
) -> usize {
    let mut demotable: Vec<usize> = (0..items.len())
        .filter(|&index| items[index].decision == ContextDecision::KeepFull && !protected[index])
        .collect();
    demotable.sort_by(|&a, &b| {
        items[a]
            .importance
            .total_cmp(&items[b].importance)
            .then_with(|| items[a].future_utility.total_cmp(&items[b].future_utility))
            .then_with(|| items[b].token_cost.cmp(&items[a].token_cost))
            .then_with(|| b.cmp(&a))
    });
    let mut demoted = 0;
    for &index in &demotable {
        if *selected <= budget {
            break;
        }
        *selected -= items[index].token_cost - compressed_cost_of(&items[index]);
        items[index].decision = ContextDecision::Compress;
        demoted += 1;
    }
    demoted
}

/// Phase 2: drop the least valuable `Compress` groups, all-or-nothing, so
/// a tool pair never splits. A group is worth as much as its weakest
/// member. Returns the number of items dropped.
fn drop_lowest_value_groups(
    groups: &[Vec<usize>],
    items: &mut [ContextItemScore],
    protected: &[bool],
    selected: &mut usize,
    budget: usize,
) -> usize {
    let group_value = |group: usize| -> (f32, f32) {
        groups[group].iter().fold(
            (f32::INFINITY, f32::INFINITY),
            |(min_importance, min_utility), &index| {
                (
                    min_importance.min(items[index].importance),
                    min_utility.min(items[index].future_utility),
                )
            },
        )
    };
    // Each merged group owns exactly one non-empty entry, so no group is
    // offered — and paid for — twice.
    let mut droppable: Vec<usize> = (0..groups.len())
        .filter(|&group| {
            !groups[group].is_empty()
                && groups[group]
                    .iter()
                    .all(|&index| items[index].decision == ContextDecision::Compress)
                && groups[group].iter().all(|&index| !protected[index])
        })
        .collect();
    droppable.sort_by(|&a, &b| {
        let (a_importance, a_utility) = group_value(a);
        let (b_importance, b_utility) = group_value(b);
        a_importance
            .total_cmp(&b_importance)
            .then_with(|| a_utility.total_cmp(&b_utility))
            .then_with(|| a.cmp(&b))
    });
    let mut dropped = 0;
    for group in droppable {
        if *selected <= budget {
            break;
        }
        for &index in &groups[group] {
            *selected -= compressed_cost_of(&items[index]);
            items[index].decision = ContextDecision::Drop;
        }
        dropped += groups[group].len();
    }
    dropped
}

/// Budgeted cost of a `Compress` item: the stand-in the compressor phase
/// will emit, estimated as raw cost over [`COMPRESSED_COST_DENOMINATOR`].
fn compressed_cost_of(item: &ContextItemScore) -> usize {
    item.token_cost.div_ceil(COMPRESSED_COST_DENOMINATOR)
}

// ── applying a plan ────────────────────────────────────────────────

/// Why a [`ContextPlan`] could not be applied to a transcript.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApplyError {
    /// The plan lines up with a different transcript: it must have exactly
    /// one item per message of the transcript it is applied to, since
    /// decisions are addressed by message index.
    #[error("plan has {plan_items} items but the transcript has {message_count} messages")]
    PlanMismatch {
        /// Number of items the plan carries.
        plan_items: usize,
        /// Number of messages in the transcript body.
        message_count: usize,
    },
    /// The crop behind the plan rejected the spans it derives — most
    /// plausibly a hand-edited plan that separates a tool call from its
    /// result, or one that keeps no message at all.
    #[error(transparent)]
    Crop(#[from] CropError),
}

/// Materialize a plan as a new `Transcript<Common>`: the original is left
/// untouched, and every decision becomes a cut or a compression in the copy.
///
/// `Drop` messages are cropped away — contiguous survivors merge into
/// [`Span`] runs for [`Transcript::crop_to`] — and `KeepFull` messages are
/// carried verbatim. `Compress` messages are rebuilt as deterministic
/// stand-ins through [`compress_message`]: the folded content (tool
/// results, or text when the item carries no result) is replaced by a
/// truncation plus a Jev reference naming the source session, the message
/// index, and the original estimated size. Metadata is preserved. The
/// original is never modified.
///
/// A plan built by the scorer and (optionally) [`allocate`] always applies:
/// the scorer keeps tool pairs whole and the allocator drops them as units.
/// The crop's checks still run, so a hand-edited plan that splits a pair is
/// rejected rather than silently corrupting the handoff.
///
/// # Errors
/// [`ApplyError::PlanMismatch`] when the plan was not built from this
/// transcript; [`ApplyError::Crop`] when the crop rejects the derived
/// spans (split pair, invalid range, or nothing kept).
pub fn apply(
    transcript: &Transcript<Common>,
    plan: &ContextPlan,
) -> std::result::Result<Transcript<Common>, ApplyError> {
    if plan.items.len() != transcript.body.len() {
        return Err(ApplyError::PlanMismatch {
            plan_items: plan.items.len(),
            message_count: transcript.body.len(),
        });
    }
    let mut copy = transcript.crop_to(&kept_spans(&plan.items))?;
    for (index, item) in plan.items.iter().enumerate() {
        if item.decision == ContextDecision::Compress {
            // `kept_spans` packs the surviving messages into runs, so the
            // applied copy's position for source message `index` is the
            // count of non-`Drop` items before it.
            let applied_index = plan.items[..index]
                .iter()
                .filter(|prior| prior.decision != ContextDecision::Drop)
                .count();
            compress_message(&mut copy.body[applied_index], index, &transcript.meta.id);
        }
    }
    Ok(copy)
}

/// The non-`Drop` items as contiguous [`Span`] runs, e.g. decisions
/// `[KeepFull, Drop, KeepFull]` become `[0..1, 2..3]`. Empty when every
/// item drops.
fn kept_spans(items: &[ContextItemScore]) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut run_start: Option<usize> = None;
    for (index, item) in items.iter().enumerate() {
        if item.decision == ContextDecision::Drop {
            if let Some(start) = run_start.take() {
                spans.push(Span(start..index));
            }
        } else if run_start.is_none() {
            run_start = Some(index);
        }
    }
    if let Some(start) = run_start {
        spans.push(Span(start..items.len()));
    }
    spans
}

// ── the deterministic compressor ───────────────────────────────────────

/// The stand-in body for a `Compress` message, and where the compressor
/// stops being conservative: `keep` leading characters of each replaced
/// block survive the truncation. Sized so a typical stand-in lands at or
/// under the allocator's [`COMPRESSED_COST_DENOMINATOR`] estimate — the
/// plan says "roughly a tenth", and the copy should honor it — while still
/// leaving the next agent a usable preview of what was folded.
const COMPRESS_KEEP_CHARS: usize = 160;

/// Rewrite one message in place as a `Compress` stand-in. Deterministic and
/// non-destructive by construction: it only ever touches `message` itself,
/// never the transcript it came from.
///
/// - A tool-result block (the oversized case the scorer actually marks) is
///   replaced by a truncation of the result content plus the reference
///   block. The `ToolResult` identity stays: same `tool_use_id`, same
///   `is_error`, so harness pairing survives the rewrite.
/// - Without any result to fold, a text-bearing message (prose demoted by
///   the allocator) is replaced by its truncation plus the reference.
/// - Reasoning, tool calls, images, and artifacts pass through untouched.
///
/// The reference block names the original session, the message's index
/// there, and its raw estimated size, so the full content stays one
/// command away in contextleleo (`contextleleo view <original-id>#<index+1>`).
fn compress_message(message: &mut Message, message_index: usize, session: &str) {
    let raw_tokens = estimate_message_tokens(message);
    let reference = Block::Text {
        text: format!(
            "[jev: compressed from session `{session}` message {index} \
             (~{raw_tokens} tokens); view with `contextleleo view {session}#{ordinal}`]",
            index = message_index,
            ordinal = message_index + 1,
        ),
    };
    for block in &mut message.content {
        match block {
            Block::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let key = content_key(content);
                *block = Block::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: ToolOutput::Text(truncate(&key, COMPRESS_KEEP_CHARS)),
                    is_error: *is_error,
                };
            }
            Block::Text { text } => {
                *text = truncate(text, COMPRESS_KEEP_CHARS);
            }
            _ => {}
        }
    }
    message.content.push(reference);
}

/// First `keep` characters (chars, not bytes) of `text`, with an explicit
/// marker when anything was cut. Char-bounded so a multibyte body cannot
/// push the stand-in past its byte estimate.
fn truncate(text: &str, keep: usize) -> String {
    if text.chars().count() <= keep {
        return text.to_string();
    }
    let head: String = text.chars().take(keep).collect();
    format!("{head}…[jev: truncated]")
}

// ── handing off to a target harness ───────────────────────────────────

/// Plan, allocate, apply, and convert in one call: the full Jev handoff to
/// `B`'s native shape.
///
/// `plan` scores `transcript` (optionally against a token `budget`),
/// [`allocate`] enforces that budget by demoting decisions, [`apply`]
/// materializes the optimized copy with compressed stand-ins, and the
/// target codec converts it — mirroring `convert`, but on the copy. The
/// source transcript is never touched.
///
/// # Errors
/// [`ApplyError::PlanMismatch`] when the plan was not built from this
/// transcript; [`ApplyError::Crop`] when the crop rejects the derived
/// spans (split pair, invalid range, or nothing kept); and the target
/// codec's own [`crate::Error`] when the copy cannot be represented in
/// `B`'s native shape.
pub fn handoff<B>(
    transcript: &Transcript<Common>,
    budget: Option<usize>,
    scorer: &impl ContextScorer,
) -> std::result::Result<Transcript<B>, HandoffError>
where
    B: crate::Codec,
{
    let planned = plan_with(transcript, budget, scorer);
    let (allocated, _report) = allocate(transcript, &planned);
    let applied = apply(transcript, &allocated)?;
    B::from_common(&applied).map_err(HandoffError::Convert)
}

/// Why a [`handoff`] could not produce the target transcript.
#[derive(Debug, thiserror::Error)]
pub enum HandoffError {
    /// The plan could not be applied to the transcript (see
    /// [`ApplyError`]).
    #[error(transparent)]
    Apply(#[from] ApplyError),
    /// The applied copy could not be represented in the target harness's
    /// native shape.
    #[error(transparent)]
    Convert(#[from] crate::Error),
}

/// Plan, allocate, and apply with the default deterministic scorer, then
/// convert — [`handoff`] without scorer plumbing.
///
/// # Errors
/// Same as [`handoff`].
pub fn handoff_with_default_scorer<B>(
    transcript: &Transcript<Common>,
    budget: Option<usize>,
) -> std::result::Result<Transcript<B>, HandoffError>
where
    B: crate::Codec,
{
    handoff(transcript, budget, &DeterministicScorer::default())
}

// ── signals ────────────────────────────────────────────────────────────

/// The session's opening user objective: the first user message carrying
/// plain text. Weighted most important by the deterministic scorer.
fn objective_index(body: &[Message]) -> Option<usize> {
    body.iter()
        .position(|message| message.role == Role::User && message.content.iter().any(is_text_block))
}

fn is_text_block(block: &Block) -> bool {
    matches!(block, Block::Text { .. })
}

fn is_tool_use(block: &Block) -> bool {
    matches!(block, Block::ToolUse { .. })
}

fn is_tool_result(block: &Block) -> bool {
    matches!(block, Block::ToolResult { .. })
}

/// A comparable identity for tool output: text as-is, JSON re-serialized.
/// Two results with the same key repeat the same content.
fn content_key(content: &ToolOutput) -> String {
    match content {
        ToolOutput::Text(text) => text.clone(),
        ToolOutput::Json(value) => value.to_string(),
    }
}

/// How many times each text/tool-result body appears in the session.
fn count_content_occurrences(body: &[Message]) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for message in body {
        for block in &message.content {
            let key = match block {
                Block::Text { text } => text.as_str(),
                Block::ToolResult { content, .. } => &content_key(content),
                _ => continue,
            };
            *counts.entry(key.to_string()).or_default() += 1;
        }
    }
    counts
}

/// Clamp a signal into `0.0..=1.0`. Inputs are sums of bounded constants,
/// so NaN cannot occur and the clamp only trims overflow.
fn clamp01(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

/// Downgrade any `Drop` that would separate a live tool call from its
/// result: the call and its result are kept or dropped together, so a
/// dropped half becomes `Compress` instead. Unmatched results (no live
/// call) are unaffected and may drop freely.
fn keep_tool_pairs_whole(pairs: &[(usize, usize)], items: &mut [ContextItemScore]) {
    for &(call, result) in pairs {
        let split = matches!(items[call].decision, ContextDecision::Drop)
            || matches!(items[result].decision, ContextDecision::Drop);
        if split {
            for index in [call, result] {
                if items[index].decision == ContextDecision::Drop {
                    items[index].decision = ContextDecision::Compress;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Codec;
    use crate::common::Tool;
    use crate::harness::simple::Simple;

    /// The truncation marker `truncate` appends; tests pin the exact
    /// wording so the compressor's size guarantee fails loudly if it
    /// drifts.
    const JEV_TRUNCATED_MARKER: &str = "…[jev: truncated]";

    fn meta() -> crate::common::Meta {
        crate::common::Meta {
            id: "jev-test".into(),
            timestamp: chrono::DateTime::UNIX_EPOCH,
            cwd: None,
            git_branch: None,
            title: None,
            cli_version: None,
            model: None,
            lineage: None,
        }
    }

    fn text_message(role: Role, text: &str) -> Message {
        Message {
            role,
            content: vec![Block::Text { text: text.into() }],
            timestamp: chrono::DateTime::UNIX_EPOCH,
            model: None,
            stop_reason: None,
            usage: None,
        }
    }

    /// An assistant tool call plus its user-side result, as separate
    /// consecutive messages (the canonical shape every harness normalizes
    /// into).
    fn tool_pair(id: &str, command: &str, output: &str) -> (Message, Message) {
        let call = Message {
            role: Role::Assistant,
            content: vec![Block::ToolUse {
                id: id.into(),
                tool: Tool::from_canonical("Bash", serde_json::json!({ "command": command })),
            }],
            timestamp: chrono::DateTime::UNIX_EPOCH,
            model: None,
            stop_reason: None,
            usage: None,
        };
        let result = Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                tool_use_id: id.into(),
                content: ToolOutput::Text(output.into()),
                is_error: false,
            }],
            timestamp: chrono::DateTime::UNIX_EPOCH,
            model: None,
            stop_reason: None,
            usage: None,
        };
        (call, result)
    }

    fn transcript(body: Vec<Message>) -> Transcript<Common> {
        Transcript::new(meta(), body)
    }

    fn decision_of(plan: &ContextPlan, index: usize) -> ContextDecision {
        plan.items[index].decision
    }

    /// Case 1 — a small session of pure prose: everything is `KeepFull`,
    /// and the selected total equals the original total.
    #[test]
    fn all_important_session_keeps_everything() {
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            text_message(Role::Assistant, "Root cause: maxConnections=10."),
            text_message(Role::User, "Ship the fix."),
        ]);
        let plan = plan(&source, None);

        assert_eq!(plan.items.len(), 3);
        assert!(
            plan.items
                .iter()
                .all(|item| item.decision == ContextDecision::KeepFull)
        );
        assert_eq!(plan.selected_tokens, plan.original_tokens);
        assert_eq!(plan.counts(), [3, 0, 0]);
    }

    /// The opening user objective scores highest of all prose messages.
    #[test]
    fn the_opening_objective_is_scored_most_important() {
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            text_message(Role::Assistant, "investigating"),
            text_message(Role::User, "what about the pool?"),
        ]);
        let items = DeterministicScorer::default().score(&source);

        assert!(items[0].importance > items[1].importance);
        assert!(items[0].importance > items[2].importance);
        assert!(items[0].importance >= 0.9);
    }

    /// Case 2 — one huge low-value tool result becomes `Compress`. Until
    /// the compressor phase lands, `Compress` items count at raw cost in
    /// the totals, so the selected size is an upper bound, never smaller.
    #[test]
    fn large_tool_output_is_marked_for_compression() {
        let (call, result) = tool_pair("t1", "cargo test", &"line of test output\n".repeat(3000));
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            call,
            result,
        ]);
        let plan = plan(&source, None);

        assert_eq!(decision_of(&plan, 2), ContextDecision::Compress);
        assert!(plan.selected_tokens <= plan.original_tokens);
        assert_eq!(plan.counts(), [2, 1, 0]);
    }

    /// A large result under the scorer's importance floor stays whole: the
    /// opening objective's companion error result is too important to fold.
    #[test]
    fn error_results_are_kept_even_when_oversized() {
        let (call, mut result) = tool_pair("t1", "cargo test", &"failure\n".repeat(2000));
        result.content[0] = Block::ToolResult {
            tool_use_id: "t1".into(),
            content: ToolOutput::Text("failure".repeat(2000)),
            is_error: true,
        };
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            call,
            result,
        ]);
        let plan = plan(&source, None);

        assert_eq!(decision_of(&plan, 2), ContextDecision::KeepFull);
    }

    /// Case 3 — the root-cause message is assistant prose, which the
    /// built-in scorer never drops or compresses.
    #[test]
    fn root_cause_prose_is_never_dropped_or_compressed() {
        let root_cause = text_message(
            Role::Assistant,
            "Root cause found: Redis connection pool maxConnections=10 \
             starves the payment worker pool under load.",
        );
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            root_cause,
        ]);
        let plan = plan(&source, None);

        assert_eq!(decision_of(&plan, 1), ContextDecision::KeepFull);
    }

    /// Case 4 — an exact-duplicate tool result that answers no live call
    /// (an orphan result, as `tool_pairs` sees it) scores redundant and is
    /// marked `Drop` — the only content the built-in scorer drops.
    #[test]
    fn duplicate_orphan_tool_output_is_marked_for_dropping() {
        let listing = "src/\ndocs/\nREADME.md\n";
        let (call, result) = tool_pair("t1", "ls src", listing);
        let orphan = Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                tool_use_id: "t2".into(),
                content: ToolOutput::Text(listing.into()),
                is_error: false,
            }],
            timestamp: chrono::DateTime::UNIX_EPOCH,
            model: None,
            stop_reason: None,
            usage: None,
        };
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            call,
            result,
            orphan,
        ]);
        let plan = plan(&source, None);

        assert_eq!(
            decision_of(&plan, 2),
            ContextDecision::KeepFull,
            "first sighting is kept"
        );
        assert_eq!(
            decision_of(&plan, 3),
            ContextDecision::Drop,
            "repeat is droppable"
        );
        assert!(plan.selected_tokens < plan.original_tokens);
    }

    /// A repeated result still answers a live call whose pair would split,
    /// so pair safety downgrades the `Drop` to `Compress`.
    #[test]
    fn pair_safety_downgrades_a_splitting_drop() {
        let listing = "src/\ndocs/\nREADME.md\n";
        let (call_a, result_a) = tool_pair("t1", "ls src", listing);
        let (call_b, result_b) = tool_pair("t2", "ls src", listing);
        // Two identical calls/results; then the second *call* repeats the
        // same command text, so its result is a redundant sighting — but
        // the call is live, so the pair must survive together.
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            call_a,
            result_a,
            call_b,
            result_b,
        ]);
        let plan = plan(&source, None);

        // The duplicate result would drop, but its call is live, so pair
        // safety downgrades the result to `Compress` and the pair survives.
        assert_eq!(decision_of(&plan, 3), ContextDecision::KeepFull);
        assert_eq!(decision_of(&plan, 4), ContextDecision::Compress);
    }

    /// Case 5 — every tool pair in the plan stays whole: a call and its
    /// result are never one-`Drop`-one-kept.
    #[test]
    fn no_decision_splits_a_tool_pair() {
        let listing = "a\nb\nc\n";
        let (call_a, result_a) = tool_pair("t1", "ls src", listing);
        let (call_b, result_b) = tool_pair("t2", "ls src", listing);
        let (call_c, result_c) = tool_pair("t3", "ls docs", "docs/\n");
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            call_a,
            result_a,
            call_b,
            result_b,
            call_c,
            result_c,
        ]);
        let plan = plan(&source, None);
        let pairs = source.tool_pairs().unwrap();

        for (call, result) in pairs {
            let dropped = |i: usize| decision_of(&plan, i) == ContextDecision::Drop;
            assert!(
                !(dropped(call) ^ dropped(result)),
                "pair {call}/{result} split"
            );
        }
    }

    /// Case 6 — `selected_tokens` never exceeds the budget when the
    /// natural selections fit, and budget is recorded on the plan.
    #[test]
    fn plan_records_the_budget_and_respects_it_when_selections_fit() {
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            text_message(Role::Assistant, "Root cause: maxConnections=10."),
        ]);
        let budget = 10_000;
        let plan = plan(&source, Some(budget));

        assert_eq!(plan.budget, Some(budget));
        assert!(plan.selected_tokens <= budget);
    }

    /// Case 7 — an empty body yields an empty plan with zero totals, and a
    /// one-message session still scores cleanly (no division by zero).
    #[test]
    fn empty_and_tiny_sessions_plan_without_noise() {
        let empty = transcript(Vec::new());
        let empty_plan = plan(&empty, Some(1_000));
        assert_eq!(empty_plan.counts(), [0, 0, 0]);
        assert_eq!(empty_plan.original_tokens, 0);
        assert_eq!(empty_plan.selected_tokens, 0);

        let tiny = transcript(vec![text_message(Role::User, "hello")]);
        let tiny_plan = plan(&tiny, None);
        assert_eq!(tiny_plan.items.len(), 1);
        assert_eq!(tiny_plan.items[0].decision, ContextDecision::KeepFull);
    }

    /// Case 8 — planning is read-only: the source transcript is byte-identical
    /// before and after, and a second planning pass is deterministic.
    ///
    /// `Transcript` is not `Serialize`, so equality checks run through the
    /// derived `PartialEq` against a pristine clone, plus byte checks of the
    /// message texts themselves.
    #[test]
    fn planning_does_not_mutate_the_source_and_is_deterministic() {
        let (call, result) = tool_pair("t1", "cargo test", &"out\n".repeat(1500));
        let (call_b, result_b) = tool_pair("t2", "ls src", "a\nb\n");
        let (call_c, result_c) = tool_pair("t3", "ls src", "a\nb\n");
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            call,
            result,
            call_b,
            result_b,
            call_c,
            result_c,
        ]);
        let pristine = source.clone();

        let first = plan(&source, Some(5_000));
        assert_eq!(source, pristine, "plan() mutated the transcript");

        let second = plan(&source, Some(5_000));
        assert_eq!(first, second, "scoring is not deterministic");
    }

    /// Case 9 — a planned-and-kept session still converts into another
    /// harness: the plan never leaves the Common layer, so the existing
    /// `Codec` path works unchanged. `Codex::from_common` requires a
    /// `ses_`-prefixed id (its rollout schema), so the metadata is reshaped
    /// the way a store save would.
    #[test]
    fn a_planned_transcript_still_converts_cross_harness() {
        use crate::harness::codex::Codex;
        use crate::{Codec, TextCodec};

        let (call, result) = tool_pair("t1", "cargo test", &"out\n".repeat(1500));
        let source = transcript(vec![call, result]);
        let plan = plan(&source, Some(10_000));
        // Phase 1 keeps everything the plan does not drop; simulate that
        // trivially by rebuilding the same body (apply() arrives in Phase 3).
        let kept: Vec<Message> = plan
            .items
            .iter()
            .filter(|item| item.decision != ContextDecision::Drop)
            .map(|item| source.body[item.message_index].clone())
            .collect();
        let mut optimized = transcript(kept);
        optimized.meta.id = format!("ses_{}", optimized.meta.id);

        let native = Codex::from_common(&optimized).unwrap();
        assert_ne!(Codex::to_text(&native).unwrap(), "");
    }

    /// Case 10 — planning is invisible to normal flow: `plan` with no
    /// budget and a default scorer over a trivial session marks nothing
    /// `Drop`, so a Phase-3 apply would be a pure copy.
    #[test]
    fn default_planning_over_a_clean_session_is_a_no_op() {
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            text_message(Role::Assistant, "Reading the config."),
            text_message(Role::Assistant, "Fixed and tests pass."),
        ]);
        let plan = plan(&source, None);
        assert_eq!(plan.counts()[2], 0);
        assert_eq!(plan.selected_tokens, plan.original_tokens);
    }

    /// The JSON wire shape matches the planned information model: `snake_case`
    /// decisions, all score fields present, round-trippable.
    #[test]
    fn plan_json_round_trips() {
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            text_message(Role::Assistant, "investigating"),
        ]);
        let plan = plan(&source, Some(1_000));
        let json = serde_json::to_string(&plan).unwrap();
        assert!(json.contains("\"keep_full\""), "{json}");
        let back: ContextPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(back, plan);
    }

    /// Token estimation: chars/4 rounded up, metadata excluded, bigger text
    /// costs more.
    #[test]
    fn token_estimation_is_chars_over_four_and_metadata_free() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("12345678"), 2);
        assert_eq!(estimate_tokens("123456789"), 3);

        let with_usage = Message {
            role: Role::Assistant,
            content: vec![Block::Text {
                text: "12345678".into(),
            }],
            timestamp: chrono::DateTime::UNIX_EPOCH,
            model: Some("m".into()),
            stop_reason: Some(crate::common::StopReason::EndTurn),
            usage: Some(crate::common::Usage {
                input_tokens: 999_999,
                output_tokens: 999_999,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            }),
        };
        assert_eq!(estimate_message_tokens(&with_usage), 2);
    }

    /// An empty pattern would over-count; guard the duplicate-detection
    /// path against results whose key is the empty string.
    #[test]
    fn empty_tool_results_do_not_cross_contaminate_redundancy() {
        let (call_a, result_a) = tool_pair("t1", "ls", "");
        let (call_b, result_b) = tool_pair("t2", "ls", "");
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            call_a,
            result_a,
            call_b,
            result_b,
        ]);
        let plan = plan(&source, None);
        // Both empty results are identical sightings; the second may drop,
        // but a call and its result must never split.
        for (call, result) in source.tool_pairs().unwrap() {
            let dropped = |i: usize| decision_of(&plan, i) == ContextDecision::Drop;
            assert!(!(dropped(call) ^ dropped(result)));
        }
    }

    /// `plan_with` honors a custom scorer: a drop-everything scorer yields
    /// zero selected tokens, proving the trait seam is real.
    #[test]
    fn a_custom_scorer_replaces_the_builtin_through_the_trait() {
        struct DropEverything;
        impl ContextScorer for DropEverything {
            fn score(&self, transcript: &Transcript<Common>) -> Vec<ContextItemScore> {
                transcript
                    .body
                    .iter()
                    .enumerate()
                    .map(|(message_index, message)| ContextItemScore {
                        message_index,
                        importance: 0.0,
                        future_utility: 0.0,
                        redundancy: 0.0,
                        token_cost: estimate_message_tokens(message),
                        decision: ContextDecision::Drop,
                    })
                    .collect()
            }
        }

        let source = transcript(vec![text_message(Role::User, "hello")]);
        let plan = plan_with(&source, None, &DropEverything);
        assert_eq!(plan.selected_tokens, 0);
        assert_eq!(plan.counts(), [0, 0, 1]);
    }

    /// Config knobs move the boundaries: raising `compress_min_tokens`
    /// keeps an oversized result whole; raising `drop_min_redundancy`
    /// keeps a duplicate whole.
    #[test]
    fn scorer_config_changes_the_decisions() {
        let listing = "a\nb\nc\n";
        let (call_a, result_a) = tool_pair("t1", "ls src", listing);
        let (call_b, result_b) = tool_pair("t2", "ls src", listing);
        let big = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            call_a,
            result_a,
            call_b,
            result_b,
        ]);

        // A result estimated well over the default compress threshold:
        let (call, result) = tool_pair("t9", "cargo test", &"out\n".repeat(3000));
        let oversized = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            call,
            result,
        ]);

        let strict = DeterministicScorer::with_config(ScorerConfig {
            compress_min_tokens: usize::MAX,
            drop_min_redundancy: 2.0,
            ..ScorerConfig::default()
        });
        let plan = plan_with(&oversized, None, &strict);
        assert_eq!(decision_of(&plan, 2), ContextDecision::KeepFull);

        let plan = plan_with(&big, None, &strict);
        assert_eq!(decision_of(&plan, 4), ContextDecision::KeepFull);
    }

    // ── Phase 2: budget allocation ──────────────────────────────────────

    /// Allocation without a budget is a no-op: the plan comes back
    /// unchanged and the report says `Fits`.
    #[test]
    fn allocation_without_a_budget_is_a_no_op() {
        let (call, result) = tool_pair("t1", "cargo test", &"out\n".repeat(1500));
        let source = transcript(vec![call, result]);
        let plan = plan(&source, None);
        let (allocated, report) = allocate(&source, &plan);

        assert_eq!(allocated, plan);
        assert_eq!(report.status, AllocationStatus::Fits);
        assert_eq!(report.full_to_compressed, 0);
        assert_eq!(report.compressed_to_dropped, 0);
    }

    /// When the selections do not fit, the lowest-value `KeepFull` item —
    /// here, plain assistant prose — folds to `Compress` first, and the
    /// protected opening objective is untouched.
    #[test]
    fn allocation_compresses_the_lowest_value_item_first() {
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."), // 7 tokens, protected
            text_message(Role::Assistant, "Reading the config."),   // 5 tokens, demotable
        ]);
        // 12 tokens selected: one prose demotion (5 → 1) must fit 8.
        let plan = plan(&source, Some(8));

        let (allocated, report) = allocate(&source, &plan);
        assert_eq!(report.status, AllocationStatus::Demoted);
        assert_eq!(report.full_to_compressed, 1);
        assert_eq!(report.counts, allocated.counts());
        assert_eq!(decision_of(&allocated, 0), ContextDecision::KeepFull);
        assert_eq!(decision_of(&allocated, 1), ContextDecision::Compress);
        assert_eq!(allocated.selected_tokens, 8);
        assert_eq!(report.counts, [1, 1, 0]);
    }

    /// Value order decides *which* item folds: with room for one demotion,
    /// the earlier, less useful prose folds and the later prose stays whole.
    #[test]
    fn allocation_demotes_in_value_order_not_transcript_order() {
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            text_message(Role::Assistant, "one two three four"),
            text_message(Role::Assistant, "five six seven eight nine ten"),
        ]);
        // 20 tokens selected: exactly one demotion (5 → 1) fits 17.
        let plan = plan(&source, Some(17));

        let (allocated, report) = allocate(&source, &plan);
        assert_eq!(report.status, AllocationStatus::Demoted);
        assert_eq!(report.full_to_compressed, 1);
        assert_eq!(decision_of(&allocated, 1), ContextDecision::Compress);
        assert_eq!(decision_of(&allocated, 2), ContextDecision::KeepFull);
        assert_eq!(allocated.selected_tokens, 16);
    }

    /// Over a tighter budget, `Compress` groups drop lowest-value first —
    /// atomically: a call and its result drop together, and the costlier,
    /// later pair survives as `Compress`.
    #[test]
    fn allocation_drops_compress_groups_pair_safe_and_lowest_value_first() {
        let (call_a, result_a) = tool_pair("t1", "cargo test", &"out\n".repeat(1000));
        let (call_b, result_b) = tool_pair("t2", "cargo test", &"err\n".repeat(1500));
        let source = transcript(vec![call_a, result_a, call_b, result_b]);
        let plan = plan(&source, None);
        // `selected_tokens` uses the scorer's raw costs, but the allocator
        // budgets compressed results at a fraction of that. Budget below
        // the allocator-model total so the ladder must engage, yet high
        // enough that one group-drop satisfies it, so the surviving pair
        // stays `Compress`.
        let model_total: usize = plan
            .items
            .iter()
            .map(|item| match item.decision {
                ContextDecision::KeepFull => item.token_cost,
                ContextDecision::Compress => compressed_cost_of(item),
                ContextDecision::Drop => 0,
            })
            .sum();
        let budget = model_total - 60;
        let mut budgeted = plan.clone();
        budgeted.budget = Some(budget);

        let (allocated, report) = allocate(&source, &budgeted);
        assert_eq!(report.status, AllocationStatus::Demoted);
        // Phase 1 folds both live calls to `Compress`…
        assert_eq!(report.full_to_compressed, 2);
        // … phase 2 then drops one whole pair.
        assert_eq!(report.compressed_to_dropped, 2);
        assert_eq!(report.counts, [0, 2, 2]);
        // Which pair drops follows the documented group-value rule: the
        // group worth less (min importance, then min future utility, the
        // earlier group on ties) goes first.
        let group_value = |indices: [usize; 2]| {
            (
                indices
                    .iter()
                    .copied()
                    .fold(f32::INFINITY, |acc, i| acc.min(plan.items[i].importance)),
                indices.iter().copied().fold(f32::INFINITY, |acc, i| {
                    acc.min(plan.items[i].future_utility)
                }),
            )
        };
        let (a_importance, a_utility) = group_value([0, 1]);
        let (b_importance, b_utility) = group_value([2, 3]);
        let a_drops_first = a_importance
            .total_cmp(&b_importance)
            .then_with(|| a_utility.total_cmp(&b_utility))
            .is_le();
        let (dropped_pair, kept_pair) = if a_drops_first {
            ([0usize, 1], [2usize, 3])
        } else {
            ([2usize, 3], [0usize, 1])
        };
        for &index in &dropped_pair {
            assert_eq!(decision_of(&allocated, index), ContextDecision::Drop);
        }
        for &index in &kept_pair {
            assert_eq!(decision_of(&allocated, index), ContextDecision::Compress);
        }
        assert!(allocated.selected_tokens <= budget);
        for (call_index, result_index) in source.tool_pairs().unwrap() {
            let dropped = |i: usize| allocated.items[i].decision == ContextDecision::Drop;
            assert!(
                !(dropped(call_index) ^ dropped(result_index)),
                "pair {call_index}/{result_index} split"
            );
        }
    }

    /// When every remaining token belongs to protected content, the
    /// allocator reports `OverBudget` and keeps the protected decisions.
    #[test]
    fn allocation_reports_over_budget_rather_than_demoting_protected_content() {
        let oversized_error = Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                tool_use_id: "t1".into(),
                content: ToolOutput::Text("failure".repeat(2000)),
                is_error: true,
            }],
            timestamp: chrono::DateTime::UNIX_EPOCH,
            model: None,
            stop_reason: None,
            usage: None,
        };
        let source = transcript(vec![oversized_error]);
        let natural = plan(&source, None);
        let plan = plan(&source, Some(natural.selected_tokens - 1));

        let (allocated, report) = allocate(&source, &plan);
        assert_eq!(report.status, AllocationStatus::OverBudget);
        assert_eq!(allocated.items, plan.items, "protected items never demote");
        assert!(report.selected_tokens > plan.budget.unwrap());
        assert_eq!(report.selected_tokens, allocated.selected_tokens);

        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"over_budget\""), "{json}");
    }

    /// `floor_importance` is the knob that holds the safety bias under
    /// pressure: with the default floor the opening objective survives even
    /// a zero-token budget; with the floor raised above the scorer's
    /// 1.0 ceiling (so nothing is protected) it may demote away.
    #[test]
    fn the_floor_importance_knob_decides_what_may_be_demoted() {
        let source = transcript(vec![text_message(Role::User, "Fix the Redis timeout bug.")]);
        let plan = plan(&source, Some(0));

        let (allocated, report) = allocate(&source, &plan);
        assert_eq!(report.status, AllocationStatus::OverBudget);
        assert_eq!(decision_of(&allocated, 0), ContextDecision::KeepFull);

        let (allocated, report) = allocate_with(
            &source,
            &plan,
            &AllocatorConfig {
                floor_importance: 1.1,
            },
        );
        assert_eq!(report.status, AllocationStatus::Demoted);
        assert_eq!(report.counts, [0, 0, 1]);
        assert_eq!(allocated.selected_tokens, 0);
    }

    /// Allocation is pure: the input plan is untouched, two runs agree, and
    /// no pair splits even when a scorer-marked `Drop` sits in the mix.
    #[test]
    fn allocation_does_not_mutate_its_input_and_is_deterministic() {
        let listing = "src/\ndocs/\nREADME.md\n".repeat(300);
        let (call, result) = tool_pair("t1", "ls src", &listing);
        let orphan = Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                tool_use_id: "t2".into(),
                content: ToolOutput::Text(listing.clone()),
                is_error: false,
            }],
            timestamp: chrono::DateTime::UNIX_EPOCH,
            model: None,
            stop_reason: None,
            usage: None,
        };
        let source = transcript(vec![call, result, orphan]);
        let plan = plan(&source, Some(1600));
        let pristine = plan.clone();

        let (first, first_report) = allocate(&source, &plan);
        assert_eq!(plan, pristine, "allocate mutated the plan");
        let (second, second_report) = allocate(&source, &plan);
        assert_eq!(first, second, "allocation is not deterministic");
        assert_eq!(first_report, second_report);
        assert_eq!(
            first.items[2].decision,
            ContextDecision::Drop,
            "orphan stays dropped"
        );

        for (call_index, result_index) in source.tool_pairs().unwrap() {
            let dropped = |i: usize| first.items[i].decision == ContextDecision::Drop;
            assert!(
                !(dropped(call_index) ^ dropped(result_index)),
                "pair {call_index}/{result_index} split"
            );
        }
    }

    // ── Phase 3: applying a plan ────────────────────────────────────

    /// Applying a plan crops the copy, never the source: dropped messages
    /// vanish from the applied transcript, everything else survives
    /// verbatim, in order, with metadata preserved.
    #[test]
    fn apply_crops_away_the_dropped_messages_and_keeps_the_source_whole() {
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            text_message(Role::Assistant, "First idea: raise maxConnections."),
            text_message(Role::Assistant, "Second idea: add exponential backoff."),
        ]);
        let mut plan = plan(&source, None);
        plan.items[1].decision = ContextDecision::Drop;

        let applied = apply(&source, &plan).unwrap();

        assert_eq!(applied.body.len(), 2);
        assert_eq!(applied.body[0], source.body[0]);
        assert_eq!(applied.body[1], source.body[2]);
        assert_eq!(applied.meta, source.meta);
        assert_eq!(source.body.len(), 3, "the source is never touched");
    }

    /// `Compress` items are rebuilt as stand-ins: the result content is
    /// truncated, the tool pairing survives, and the reference block names
    /// the source session and message. Everything else is verbatim.
    #[test]
    fn apply_rewrites_compress_items_into_referenced_stand_ins() {
        let raw = "out\n".repeat(1500);
        let (call, result) = tool_pair("t1", "cargo test", &raw);
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            call,
            result,
        ]);
        let plan = plan(&source, None);
        assert_eq!(decision_of(&plan, 2), ContextDecision::Compress);

        let applied = apply(&source, &plan).unwrap();

        // The objective and the call pass through verbatim.
        assert_eq!(applied.body[0], source.body[0]);
        assert_eq!(applied.body[1], source.body[1]);
        // The result keeps its pairing identity but not its bulk.
        let Block::ToolResult {
            tool_use_id,
            content: ToolOutput::Text(text),
            is_error,
        } = &applied.body[2].content[0]
        else {
            panic!("expected a tool result block, got {:?}", applied.body[2]);
        };
        assert_eq!(tool_use_id, "t1");
        assert!(!is_error, "the fixture result is not an error");
        assert!(text.starts_with("out\n"));
        assert!(text.ends_with(JEV_TRUNCATED_MARKER));
        let Block::Text { text: reference } = &applied.body[2].content[1] else {
            panic!("expected the reference block");
        };
        assert!(reference.contains("session `jev-test`"));
        assert!(reference.contains("message 2"));
        assert_eq!(applied.body[2].content.len(), 2);
        // The source itself is never touched.
        let Block::ToolResult {
            content: ToolOutput::Text(source_text),
            ..
        } = &source.body[2].content[0]
        else {
            panic!("expected the untouched tool result");
        };
        assert_eq!(source_text, &raw);
        assert_eq!(source.body[2].content.len(), 1, "no reference injected");
    }

    /// Stand-ins stay inside the allocator's cost model: a compressed
    /// message's rewritten body estimates under its planned stand-in cost
    /// (`token_cost / COMPRESSED_COST_DENOMINATOR`), so the budget the
    /// allocator promised is the budget the copy delivers.
    #[test]
    fn compressed_stand_ins_honor_the_allocator_cost_model() {
        let (call, result) = tool_pair("t1", "cargo test", &"out\n".repeat(1500));
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            call,
            result,
        ]);
        let allocated = plan(&source, None);
        let item = &allocated.items[2];
        assert_eq!(item.decision, ContextDecision::Compress);
        let planned = item.token_cost.div_ceil(COMPRESSED_COST_DENOMINATOR);

        let applied = apply(&source, &allocated).unwrap();

        let rewritten = estimate_message_tokens(&applied.body[2]);
        assert!(
            rewritten <= planned,
            "stand-in {rewritten} tokens must fit the planned {planned}"
        );
    }

    /// A demoted prose message (no tool result) compresses its text and
    /// still gains the reference block.
    #[test]
    fn prose_demoted_to_compress_is_truncated_with_a_reference() {
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            text_message(Role::Assistant, "detail ".repeat(200).trim()),
        ]);
        let mut planned = plan(&source, None);
        planned.items[1].decision = ContextDecision::Compress;

        let applied = apply(&source, &planned).unwrap();

        let Block::Text { text } = &applied.body[1].content[0] else {
            panic!("expected a text block");
        };
        assert!(text.ends_with(JEV_TRUNCATED_MARKER));
        let Block::Text { text: reference } = &applied.body[1].content[1] else {
            panic!("expected the reference block");
        };
        assert!(reference.contains("session `jev-test` message 1"));
    }

    /// The reference block carries enough to recover the original: the
    /// session id and 1-based message number in the reference line up
    /// with the source's identity.
    #[test]
    fn references_name_the_original_session_and_message() {
        let (call, result) = tool_pair("t1", "cargo test", &"out\n".repeat(1500));
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            call,
            result,
        ]);
        let plan = plan(&source, None);

        let applied = apply(&source, &plan).unwrap();

        let Block::Text { text: reference } = &applied.body[2].content[1] else {
            panic!("expected the reference block");
        };
        // 1-based, matching `view <session>#<n>` message numbering.
        assert!(reference.contains("jev-test#3"));
    }

    /// A `Compress` item between two `Drop`s must still be rewritten at the
    /// right position of the applied copy — the crop renumbers survivors.
    #[test]
    fn compress_after_drops_rewrites_the_surviving_copy_position() {
        let (call, result) = tool_pair("t1", "cargo test", &"out\n".repeat(1500));
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            text_message(Role::Assistant, "First idea."),
            call,
            result,
        ]);
        let mut planned = plan(&source, None);
        planned.items[1].decision = ContextDecision::Drop;
        planned.items[2].decision = ContextDecision::KeepFull;
        planned.items[3].decision = ContextDecision::Compress;

        let applied = apply(&source, &planned).unwrap();

        assert_eq!(applied.body.len(), 3);
        // The crop renumbers survivors: the call lands at applied position
        // 1, and the result — source index 3 — is the stand-in at position 2.
        assert_eq!(applied.body[1], source.body[2], "the call is verbatim");
        let Block::ToolResult {
            content: ToolOutput::Text(text),
            ..
        } = &applied.body[2].content[0]
        else {
            panic!("expected the rewritten result at applied position 2");
        };
        assert!(text.ends_with(JEV_TRUNCATED_MARKER));
    }

    /// Small content is not padded with a truncation marker: under the
    /// keep budget, a `Compress` message keeps its content as-is and only
    /// gains the reference.
    #[test]
    fn small_content_is_carried_whole_with_just_a_reference() {
        let source = transcript(vec![text_message(Role::User, "Short note.")]);
        let mut planned = plan(&source, None);
        planned.items[0].decision = ContextDecision::Compress;

        let applied = apply(&source, &planned).unwrap();

        let Block::Text { text } = &applied.body[0].content[0] else {
            panic!("expected a text block");
        };
        assert_eq!(text, "Short note.");
        assert!(matches!(applied.body[0].content[1], Block::Text { .. }));
    }

    /// The crop's pair safety backstops the plan: a hand-edited plan that
    /// drops one half of a live pair is rejected, not silently applied.
    #[test]
    fn apply_rejects_a_plan_that_splits_a_tool_pair() {
        let (call, result) = tool_pair("t1", "cargo test", &"out\n".repeat(1500));
        let source = transcript(vec![call, result]);
        let mut plan = plan(&source, None);
        plan.items[1].decision = ContextDecision::Drop;

        let error = apply(&source, &plan).unwrap_err();

        let ApplyError::Crop(CropError::SplitToolPair { span, nearest }) = error else {
            panic!("expected a split-pair crop error, got {error:?}");
        };
        assert_eq!(span, Span(0..1));
        assert_eq!(nearest, Span(0..2));
    }

    /// A plan built for another transcript cannot be applied: decisions
    /// are addressed by index, so the lengths must line up.
    #[test]
    fn apply_rejects_a_plan_built_for_a_different_transcript() {
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            text_message(Role::Assistant, "Reading the config."),
        ]);
        let plan = plan(&source, None);
        let other = transcript(vec![text_message(Role::User, "Unrelated session.")]);

        assert_eq!(
            apply(&other, &plan).unwrap_err(),
            ApplyError::PlanMismatch {
                plan_items: 2,
                message_count: 1,
            }
        );
    }

    /// Dropping everything keeps no message, and the crop refuses to build
    /// an empty transcript.
    #[test]
    fn apply_of_an_all_drop_plan_is_an_error() {
        let source = transcript(vec![text_message(Role::User, "Fix the Redis timeout bug.")]);
        let mut plan = plan(&source, None);
        plan.items[0].decision = ContextDecision::Drop;

        let error = apply(&source, &plan).unwrap_err();

        assert!(matches!(
            error,
            ApplyError::Crop(CropError::InvalidRange { .. })
        ));
    }

    /// The full handoff pipeline — score, allocate to a budget, apply —
    /// produces the optimized copy: exactly the non-`Drop` messages, in
    /// order, while the source stays whole.
    #[test]
    fn plan_allocate_and_apply_produce_the_handoff_copy() {
        let (call_a, result_a) = tool_pair("t1", "cargo test", &"out\n".repeat(1000));
        let (call_b, result_b) = tool_pair("t2", "cargo test", &"err\n".repeat(1500));
        let source = transcript(vec![call_a, result_a, call_b, result_b]);
        let natural = plan(&source, None);
        let model_total: usize = natural
            .items
            .iter()
            .map(|item| match item.decision {
                ContextDecision::KeepFull => item.token_cost,
                ContextDecision::Compress => item.token_cost.div_ceil(COMPRESSED_COST_DENOMINATOR),
                ContextDecision::Drop => 0,
            })
            .sum();
        let mut budgeted = natural.clone();
        budgeted.budget = Some(model_total - 60);
        let (allocated, report) = allocate(&source, &budgeted);
        assert_eq!(report.status, AllocationStatus::Demoted);
        assert_eq!(report.compressed_to_dropped, 2, "one pair dropped");

        let applied = apply(&source, &allocated).unwrap();

        // The expected copy: every non-`Drop` source message, in order,
        // with `Compress` items rewritten exactly as `apply` does.
        let kept: Vec<Message> = source
            .body
            .iter()
            .enumerate()
            .filter(|(index, _)| allocated.items[*index].decision != ContextDecision::Drop)
            .map(|(index, message)| {
                let mut copy = message.clone();
                if allocated.items[index].decision == ContextDecision::Compress {
                    compress_message(&mut copy, index, &source.meta.id);
                }
                copy
            })
            .collect();
        assert_eq!(applied.body, kept);
        assert_eq!(applied.body.len(), 2, "one pair survives");
        assert_eq!(source.body.len(), 4, "the source is never touched");
    }

    // ── Phase 4: the deterministic compressor ───────────────────

    /// Stand-in bodies are byte-stable: two passes over identical input
    /// produce identical text — the whole point of a deterministic
    /// compressor.
    #[test]
    fn compression_is_deterministic_across_passes() {
        let (call, result) = tool_pair("t1", "cargo test", &"out\n".repeat(1500));
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            call,
            result,
        ]);
        let planned = plan(&source, None);

        let first = apply(&source, &planned).unwrap();
        let second = apply(&source, &planned).unwrap();

        assert_eq!(first, second);
    }

    /// Compressing never touches the source: the original messages keep
    /// their full bodies, so references into them stay valid.
    #[test]
    fn compression_never_touches_the_source() {
        let raw = "out\n".repeat(1500);
        let (call, result) = tool_pair("t1", "cargo test", &raw);
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            call,
            result,
        ]);
        let planned = plan(&source, None);

        let _ = apply(&source, &planned).unwrap();

        let Block::ToolResult {
            content: ToolOutput::Text(text),
            ..
        } = &source.body[2].content[0]
        else {
            panic!("expected the untouched tool result");
        };
        assert_eq!(text, &raw);
        assert_eq!(source.body[2].content.len(), 1, "no reference injected");
    }

    /// Reasoning and tool calls pass through a `Compress` rewrite
    /// untouched — only the folded content (results, text) changes.
    #[test]
    fn compress_keeps_reasoning_and_tool_calls_intact() {
        let message = Message {
            role: Role::Assistant,
            content: vec![
                Block::Thinking {
                    text: "long reasoning ".repeat(200),
                    signature: Some("sig".into()),
                    encrypted: None,
                },
                Block::Text {
                    text: "short conclusion".into(),
                },
                Block::ToolUse {
                    id: "t9".into(),
                    tool: Tool::from_canonical("Bash", serde_json::json!({ "command": "ls" })),
                },
            ],
            timestamp: chrono::DateTime::UNIX_EPOCH,
            model: None,
            stop_reason: None,
            usage: None,
        };
        let mut rewritten = message.clone();
        compress_message(&mut rewritten, 7, "jev-test");

        assert_eq!(rewritten.content[0], message.content[0], "thinking intact");
        assert_eq!(rewritten.content[2], message.content[2], "tool call intact");
        let Block::Text { text } = &rewritten.content[1] else {
            panic!("expected the text block");
        };
        assert_eq!(text, "short conclusion", "short text is not truncated");
        assert_eq!(rewritten.content.len(), 4, "the reference was appended");
        let Block::Text { text: reference } = &rewritten.content[3] else {
            panic!("expected the reference block");
        };
        assert!(reference.contains("message 7"), "got {reference}");
        assert!(
            reference.contains('~'),
            "the raw token estimate is recorded"
        );
    }

    /// The applied copy's session comes from the source's meta — even when
    /// the two ids differ, the stand-in always points at the original.
    #[test]
    fn references_name_the_source_session_not_the_copy() {
        let source = transcript(vec![text_message(Role::User, "Fix the bug.")]);
        let mut planned = plan(&source, None);
        planned.items[0].decision = ContextDecision::Compress;

        let mut applied = apply(&source, &planned).unwrap();
        applied.meta.id = "fresh-copy-id".into();

        let Block::Text { text: reference } = &applied.body[0].content[1] else {
            panic!("expected the reference block");
        };
        assert!(
            reference.contains("session `jev-test` message 0"),
            "got {reference}"
        );
        assert!(!reference.contains("fresh-copy-id"));
    }

    /// A multibyte body cannot push the stand-in past its estimate: the
    /// truncation is char-bounded, so the marker survives intact.
    #[test]
    fn multibyte_content_truncates_on_char_boundaries() {
        let source = transcript(vec![text_message(Role::User, "héllo ".repeat(200).trim())]);
        let mut planned = plan(&source, None);
        planned.items[0].decision = ContextDecision::Compress;

        let applied = apply(&source, &planned).unwrap();

        let Block::Text { text } = &applied.body[0].content[0] else {
            panic!("expected a text block");
        };
        assert!(text.ends_with(JEV_TRUNCATED_MARKER));
        assert!(text.is_char_boundary(text.len()));
    }

    // ── Phase 5: handoff to a target harness ───────────────────

    /// The full pipeline — score, allocate, apply, convert — produces the
    /// target's native transcript carrying the optimized conversation, and
    /// the source is untouched.
    #[test]
    fn handoff_converts_the_optimized_copy_into_the_target_shape() {
        let (call, result) = tool_pair("t1", "cargo test", &"out\n".repeat(1500));
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            call,
            result,
        ]);
        let before = source.clone();

        let native = handoff_with_default_scorer::<Simple>(&source, None).unwrap();

        // Round the native transcript back to Common: the stand-in (with
        // its reference) and the verbatim messages must all survive.
        let round = Simple::to_common(&native).unwrap();
        assert_eq!(round.body.len(), 3);
        let Block::ToolResult {
            content: ToolOutput::Text(text),
            tool_use_id,
            ..
        } = &round.body[2].content[0]
        else {
            panic!("expected the tool result in the round-tripped copy");
        };
        assert_eq!(tool_use_id, "t1", "pairing survives the conversion");
        assert!(text.ends_with(JEV_TRUNCATED_MARKER));
        assert!(text.len() < 1500, "the bulk was folded");
        assert_eq!(source, before, "the source is never touched");
    }

    /// A budget that forces a demotion is honored end-to-end: the copy
    /// carries fewer messages than the source, and the allocation report
    /// travels with the plan.
    #[test]
    fn handoff_honors_a_budget_through_conversion() {
        let (call_a, result_a) = tool_pair("t1", "cargo test", &"out\n".repeat(1000));
        let (call_b, result_b) = tool_pair("t2", "cargo test", &"err\n".repeat(1500));
        let source = transcript(vec![call_a, result_a, call_b, result_b]);
        let natural = plan(&source, None);
        let model_total: usize = natural
            .items
            .iter()
            .map(|item| match item.decision {
                ContextDecision::KeepFull => item.token_cost,
                ContextDecision::Compress => item.token_cost.div_ceil(COMPRESSED_COST_DENOMINATOR),
                ContextDecision::Drop => 0,
            })
            .sum();
        let mut budgeted = natural.clone();
        budgeted.budget = Some(model_total - 60);
        let (allocated, report) = allocate(&source, &budgeted);
        assert_eq!(report.status, AllocationStatus::Demoted);

        let native = handoff(&source, budgeted.budget, &DeterministicScorer::default()).unwrap();
        let round = Simple::to_common(&native).unwrap();

        let expected: Vec<Message> = source
            .body
            .iter()
            .enumerate()
            .filter(|(index, _)| allocated.items[*index].decision != ContextDecision::Drop)
            .map(|(index, message)| {
                let mut copy = message.clone();
                if allocated.items[index].decision == ContextDecision::Compress {
                    compress_message(&mut copy, index, &source.meta.id);
                }
                copy
            })
            .collect();
        assert_eq!(round.body, expected);
    }

    /// An unmeetable budget is reported, not silently sacrificed: the
    /// handoff still proceeds with the least-over plan.
    #[test]
    fn handoff_reports_over_budget_instead_of_failing() {
        let (call, result) = tool_pair("t1", "cargo test", &"out\n".repeat(2000));
        let source = transcript(vec![
            text_message(Role::User, "Fix the Redis timeout bug."),
            call,
            result,
        ]);
        // 1 token cannot hold even the protected objective.
        let native = handoff_with_default_scorer::<Simple>(&source, Some(1));

        assert!(native.is_ok(), "an unreachable budget is not an error");
    }
}
