//! Jev retrieval: search stored history for the context a new task needs,
//! rank it deterministically, and hand the smallest useful set to the
//! existing Jev optimization pipeline.
//!
//! The layer is **read-only** and sits beside the transcript model — it
//! never mutates a stored session. Discovery, parsing, and indexing reuse
//! the existing machinery ([`crate::local`] for sessions, [`crate::search`]
//! for line-level text search), and optimization stays in [`crate::jev`]:
//! retrieval selects *what* history matters, Jev decides *how* it fits the
//! budget (keep / compress / drop).
//!
//! ```text
//! query → candidate hits → scored chunks → budgeted selection
//!       → assembled Transcript<Common> → jev::plan / allocate / apply
//!       → optimized context (every block traceable to session#message)
//! ```
//!
//! Ranking is deterministic and local: keyword overlap (the search index's
//! own scores), file-path, code-symbol, error-token, and tool-name overlap
//! with the query, plus recency. No embeddings, no network — a future
//! semantic retriever implements [`ContextRetriever`] and drops in without
//! touching callers or the optimizer.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::common::{Block, Message, Meta, Role};
use crate::jev;
use crate::search::{DocKey, Index, Origin, Query};
use crate::{Common, Transcript};

// ── results ────────────────────────────────────────────────────────────

/// Where one retrieved chunk came from, in the same terms the rest of the
/// crate uses: a [`DocKey`] for the session and the one-based message
/// number `view <id>#<n>` prints. `None` marks session metadata (the
/// header's title/cwd matched), which has no message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceReference {
    /// The session the chunk came from.
    pub session: DocKey,
    /// Zero-based index into the source transcript's body, when the chunk
    /// is a message rather than the session header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_index: Option<usize>,
}

impl SourceReference {
    /// The `session#message` locator the CLI prints and [`crate::fragment`]
    /// parses — `1`-based, like every user-facing message number.
    #[must_use]
    pub fn locator(&self) -> String {
        match self.message_index {
            Some(index) => format!("{}#{}", self.session, index + 1),
            None => self.session.to_string(),
        }
    }
}

/// One retrieved piece of history: a contiguous slice of one source
/// message, its relevance, and where it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetrievedContext {
    /// Session and message the chunk came from.
    pub source: SourceReference,
    /// The chunk's text: the matched block rendered to plain text.
    pub content: String,
    /// Relevance in `0..=1` after normalization — the ranker's output, not
    /// a probability.
    pub relevance: f32,
    /// Signals that fired for this chunk, for explanations and tests.
    pub signals: Vec<Signal>,
    /// The session's own timestamp (recency ranking uses it; the optimizer
    /// timestamps the assembled message with it).
    pub session_time: DateTime<Utc>,
}

impl RetrievedContext {
    /// Estimated token cost of the chunk ([`jev::estimate_tokens`]).
    #[must_use]
    pub fn tokens(&self) -> usize {
        jev::estimate_tokens(&self.content)
    }
}

/// Which ranking signal contributed to a chunk's score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    /// Text matched the query (the search index's score).
    Keyword,
    /// A file path in the chunk also appears in the query.
    FileOverlap,
    /// A code symbol in the chunk also appears in the query.
    SymbolOverlap,
    /// Error-looking text in the chunk overlaps the query's error terms.
    ErrorOverlap,
    /// A tool name in the chunk appears in the query.
    ToolOverlap,
    /// The session is recent (scaled by age).
    Recency,
    /// The chunk quotes a phrase from the query verbatim.
    Phrase,
}

// ── options ────────────────────────────────────────────────────────────

/// Tunables for a retrieval pass. Defaults retrieve a small, cheap set:
/// at most [`DEFAULT_MAX_CHUNKS`] chunks, and nothing scoring under
/// [`MIN_RELEVANCE`].
#[derive(Debug, Clone)]
pub struct RetrievalOptions {
    /// Maximum chunks returned, after ranking and filtering.
    pub max_chunks: usize,
    /// Hard cap on the estimated tokens the retrieved set may carry — the
    /// pre-optimization bound (the Jev budget is a second, final gate).
    pub max_tokens: Option<usize>,
    /// Restrict candidates to these harnesses; `None` searches all.
    pub harnesses: Option<Vec<crate::HarnessId>>,
    /// Restrict candidates to sessions recorded in or under this directory.
    pub cwd: Option<String>,
    /// Drop chunks scoring under this (0..1).
    pub min_relevance: f32,
    /// Return each session's best chunk only (`true`) or several (`false`).
    pub one_chunk_per_session: bool,
}

impl Default for RetrievalOptions {
    fn default() -> Self {
        RetrievalOptions {
            max_chunks: DEFAULT_MAX_CHUNKS,
            max_tokens: None,
            harnesses: None,
            cwd: None,
            min_relevance: MIN_RELEVANCE,
            one_chunk_per_session: false,
        }
    }
}

/// Chunks returned when [`RetrievalOptions`] is default: small enough to
/// optimize fast, large enough to cover a multi-session investigation.
pub const DEFAULT_MAX_CHUNKS: usize = 12;

/// Relevance floor for a chunk to survive filtering — anything lower is
/// noise by construction (a single stray keyword hit).
pub const MIN_RELEVANCE: f32 = 0.15;

// ── the retriever abstraction ──────────────────────────────────────────

/// A source of retrieved history. Implemented today by
/// [`IndexRetriever`] over the local search index; a semantic/embedding
/// retriever implements the same trait and drops in without touching the
/// engine, the CLI, or the optimizer. Implementations must be read-only.
pub trait ContextRetriever {
    /// Retrieve chunks relevant to `query`, ranked most-relevant first.
    ///
    /// # Errors
    /// When the backend itself fails; an empty history is `Ok(vec![])`.
    fn retrieve(
        &self,
        query: &str,
        options: &RetrievalOptions,
    ) -> crate::Result<Vec<RetrievedContext>>;
}

/// Retrieve from an already-built search [`Index`] — the shared cache the
/// `query` command and the MCP server fill. Cheap: no re-parsing.
pub struct IndexRetriever<'a> {
    index: &'a Index,
    now: Option<DateTime<Utc>>,
}

impl<'a> IndexRetriever<'a> {
    /// Retrieve from `index`. `now` anchors recency (tests pass fixed
    /// times); `None` uses the wall clock.
    #[must_use]
    pub fn new(index: &'a Index) -> Self {
        IndexRetriever { index, now: None }
    }

    /// Anchor recency to `now` instead of the wall clock.
    #[must_use]
    pub fn at(mut self, now: DateTime<Utc>) -> Self {
        self.now = Some(now);
        self
    }
}

impl ContextRetriever for IndexRetriever<'_> {
    fn retrieve(
        &self,
        query: &str,
        options: &RetrievalOptions,
    ) -> crate::Result<Vec<RetrievedContext>> {
        let now = self.now.unwrap_or_else(Utc::now);
        // The index's fuzzy mode is AND semantics — every atom must match —
        // so a natural-language query would need one line carrying all of
        // its words. Retrieval wants OR: run one fuzzy query per term and
        // merge the per-document matches, keeping each line's best score.
        // The ranker below, not the index, decides how much overlap matters.
        let terms = query_terms(query);
        let limit = options.max_chunks.saturating_mul(16).max(64);
        let mut by_key: HashMap<DocKey, (Meta, HashMap<usize, crate::search::Hit>)> =
            HashMap::new();
        let mut run = |pattern: &str| {
            let mut search = Query::fuzzy(pattern.to_string());
            search.limit = Some(limit);
            search.harnesses = options.harnesses.clone();
            search.cwd = options.cwd.clone();
            search.hits_per_doc = Some(12);
            // Tool output is searched too: errors, stack traces, and config
            // values live in results, and the default query scope skips it.
            search.origins = Origin::ALL.to_vec();
            for doc in self.index.query(&search) {
                let entry = by_key
                    .entry(doc.key.clone())
                    .or_insert_with(|| (doc.meta.clone(), HashMap::new()));
                for hit in doc.hits {
                    entry
                        .1
                        .entry(hit.span.0.start)
                        .and_modify(|kept| {
                            if hit.score > kept.score {
                                *kept = hit.clone();
                            }
                        })
                        .or_insert_with(|| hit.clone());
                }
            }
        };
        if terms.is_empty() {
            // A query of only short words: try the whole string, best effort.
            run(query.trim());
        } else {
            for term in &terms {
                run(term);
            }
        }
        // Rank the merged hits: each source's best line score normalizes its
        // own chunks, exactly as the single-query path would.
        let mut chunks: Vec<RetrievedContext> = by_key
            .into_iter()
            .flat_map(|(key, (meta, hits))| {
                let mut hits: Vec<crate::search::Hit> = hits.into_values().collect();
                hits.sort_by_key(|hit| hit.span.0.start);
                rank_hits(&key, &meta, &hits, query, now)
            })
            .filter(|chunk| chunk.relevance >= options.min_relevance)
            .collect();
        chunks.sort_by(|a, b| {
            b.relevance
                .partial_cmp(&a.relevance)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.source.session.id.cmp(&b.source.session.id))
                .then_with(|| {
                    a.source
                        .message_index
                        .unwrap_or(0)
                        .cmp(&b.source.message_index.unwrap_or(0))
                })
        });
        if options.one_chunk_per_session {
            let mut seen = HashSet::new();
            chunks.retain(|chunk| seen.insert(chunk.source.session.id.clone()));
        }
        Ok(select_within_budget(
            chunks,
            options.max_chunks,
            options.max_tokens,
        ))
    }
}

/// Discover every local session, build an index, and retrieve — the
/// one-shot path for callers without a cached index.
///
/// # Errors
/// When discovery or parsing fails (see [`crate::local`]).
pub fn retrieve_local(
    query: &str,
    options: &RetrievalOptions,
) -> crate::Result<Vec<RetrievedContext>> {
    let sessions = crate::local::discover_with(|_, _| {});
    if sessions.is_empty() {
        return Ok(Vec::new());
    }
    let mut index = Index::new();
    let mut lookup: HashMap<String, &crate::local::Session> = HashMap::new();
    for session in &sessions {
        // A parse failure skips the session rather than failing retrieval:
        // history that cannot render cannot rank either.
        if let Ok(common) = session.read() {
            let key = DocKey {
                harness: session.harness,
                id: session.meta.id.clone(),
                source: None,
            };
            index.insert(key, &common);
            lookup.insert(session.meta.id.clone(), session);
        }
    }
    let retriever = IndexRetriever::new(&index);
    let mut hits = retriever.retrieve(query, options)?;
    // Resolve sourceless ids against the discovered sessions so callers can
    // read the full transcript behind any chunk.
    for hit in &mut hits {
        if let Some(session) = lookup.get(&hit.source.session.id) {
            hit.session_time = session.meta.timestamp;
        }
    }
    Ok(hits)
}

// ── deterministic ranking ──────────────────────────────────────────────

/// Query terms the ranker extracts: lowercase, length ≥ 3, minus stopwords.
fn query_terms(query: &str) -> Vec<String> {
    const STOPWORDS: [&str; 14] = [
        "the", "and", "for", "was", "were", "what", "how", "did", "does", "with", "that", "this",
        "from", "have",
    ];
    query
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '.')
        .map(str::to_ascii_lowercase)
        .filter(|term| term.len() >= 3 && !STOPWORDS.contains(&term.as_str()))
        .collect()
}

/// Whether `text` looks like a file path or was one in the query.
fn path_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .filter(|word| word.contains('/') || word.contains(".rs") || word.contains(".ts"))
        .map(|word| {
            word.trim_matches(|c: char| !c.is_alphanumeric())
                .to_string()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

/// Score one chunk against the query's terms. `weights` sums the signals;
/// the total is clamped to 1.0 at the end.
#[allow(clippy::too_many_arguments)]
fn chunk_score(
    content: &str,
    origin: Origin,
    index_score: u32,
    max_index_score: u32,
    terms: &[String],
    paths: &[String],
    errors: &[String],
    age_days: f32,
) -> (f32, Vec<Signal>) {
    let lower = content.to_ascii_lowercase();
    let mut score = 0.0;
    let mut signals = Vec::new();

    // Literal term overlap is the core signal — NOT the index's fuzzy
    // score, which is subsequence-based ("redis" matches "unrelated")
    // and only serves as the candidate prefilter. Each query term found
    // verbatim in the chunk earns a share of 0.45, so one stray match
    // stays under the noise floor and a multi-term hit dominates.
    let hits = terms
        .iter()
        .filter(|term| lower.contains(term.as_str()))
        .count();
    if hits > 0 {
        score += 0.45 * (hits as f32 / terms.len().max(1) as f32);
        signals.push(Signal::Keyword);
    }
    // The index's normalized score grades among chunks with equal term
    // coverage (position bonuses, exact-phrase bonus).
    if index_score > 0 && max_index_score > 0 {
        score += 0.10 * (index_score as f32 / max_index_score as f32);
    }

    // File overlap: a query path term appears in the chunk.
    if paths
        .iter()
        .any(|p| !p.is_empty() && lower.contains(&p.to_ascii_lowercase()))
    {
        score += 0.20;
        signals.push(Signal::FileOverlap);
    }

    // Symbol overlap: at least two query terms appear as distinct words,
    // suggesting identifiers rather than prose.
    if terms.iter().filter(|t| lower.contains(t.as_str())).count() >= 2 {
        score += 0.15;
        signals.push(Signal::SymbolOverlap);
    }

    // Error overlap: the chunk is a tool result carrying error text whose
    // terms the query shares.
    if origin == Origin::ToolResult
        && (lower.contains("error")
            || lower.contains("failed")
            || lower.contains("panic")
            || lower.contains("timeout")
            || lower.contains("exhausted")
            || lower.contains("refused"))
        && errors.iter().any(|e| lower.contains(e.as_str()))
    {
        score += 0.20;
        signals.push(Signal::ErrorOverlap);
    }

    // Tool overlap: a tool name appears in the query.
    for tool in [
        "bash",
        "read",
        "write",
        "edit",
        "glob",
        "grep",
        "list_directory",
        "view_file",
    ] {
        if lower.contains(tool) && terms.iter().any(|t| t == tool) {
            score += 0.05;
            signals.push(Signal::ToolOverlap);
            break;
        }
    }

    // Recency is a tiebreaker, never a standalone reason to surface a
    // chunk: it only counts when at least one content signal fired.
    if !signals.is_empty() {
        score += 0.10 * (1.0 / (1.0 + (age_days - 14.0).max(0.0) / 365.0));
        signals.push(Signal::Recency);
    }

    (score.min(1.0), signals)
}

// ── ranking hits into chunks ───────────────────────────────────────────

/// Rank one document's hits into scored chunks.
fn rank_hits(
    key: &DocKey,
    meta: &Meta,
    hits: &[crate::search::Hit],
    query: &str,
    now: DateTime<Utc>,
) -> Vec<RetrievedContext> {
    let terms = query_terms(query);
    let paths = path_terms(query);
    // Error terms: query words that themselves look like error vocabulary,
    // so "redis timeout" matches a "connection timeout" error.
    let errors: Vec<String> = terms
        .iter()
        .filter(|t| {
            t.contains("error")
                || t.contains("fail")
                || t.contains("timeout")
                || t.contains("panic")
                || t.contains("refuse")
        })
        .cloned()
        .collect();
    let max_score = hits.iter().map(|hit| hit.score).max().unwrap_or(1).max(1);
    let age_days = (now - meta.timestamp).num_hours().max(0) as f32 / 24.0;

    hits.iter()
        .filter(|hit| hit.origin != Origin::Meta)
        // Session-header matches (title/cwd) find sessions, not context:
        // a chunk must carry message content to be worth handing to an
        // agent, and a Meta hit has no message behind it.
        .map(|hit| {
            let (mut relevance, mut signals) = chunk_score(
                &hit.line, hit.origin, hit.score, max_score, &terms, &paths, &errors, age_days,
            );
            // An exact phrase from the query is the strongest single signal
            // a line can carry.
            if query.chars().count() >= 8 {
                let folded_query = query.to_ascii_lowercase();
                let folded_line = hit.line.to_ascii_lowercase();
                if folded_line.contains(&folded_query) {
                    relevance += 0.25;
                    signals.push(Signal::Phrase);
                }
            }
            RetrievedContext {
                source: SourceReference {
                    session: key.clone(),
                    message_index: (hit.span.0.start < hit.span.0.end).then(|| hit.span.0.start),
                },
                content: hit.line.clone(),
                relevance: relevance.min(1.0),
                signals,
                session_time: meta.timestamp,
            }
        })
        .collect()
}

/// Take the ranked head, respecting the chunk count and the optional token
/// ceiling: chunks are never truncated here (the optimizer does that), a
/// chunk that would exceed the token cap is skipped for the next one.
fn select_within_budget(
    ranked: Vec<RetrievedContext>,
    max_chunks: usize,
    max_tokens: Option<usize>,
) -> Vec<RetrievedContext> {
    let mut chosen = Vec::new();
    let mut spent = 0usize;
    for chunk in ranked {
        if chosen.len() >= max_chunks {
            break;
        }
        let cost = chunk.tokens();
        if let Some(cap) = max_tokens {
            if cost > cap.saturating_sub(spent) {
                continue;
            }
        }
        spent += cost;
        chosen.push(chunk);
    }
    chosen
}

// ── relevance-weighted scoring ────────────────────────────────────────

/// Share of a chunk's base Jev importance that survives when its retrieval
/// relevance is zero. The other end is fixed: relevance 1.0 keeps the base
/// score untouched. At the 0.15 `min_relevance` floor a chunk keeps ~62%
/// of its base importance — for ordinary prose (base 0.75) that is ~0.46,
/// below the allocator's 0.6 protection floor, so weakly relevant chunks
/// are what a tight budget sheds first. A 0.75-base chunk stays protected
/// only above a relevance of ~0.56.
const RELEVANCE_FLOOR_WEIGHT: f32 = 0.55;

/// A Jev scorer that carries retrieval relevance into optimization.
///
/// Wraps any [`jev::ContextScorer`] and scales each message's importance by
/// its chunk's retrieval relevance — nothing else. The base scorer still
/// decides content treatment (tool-result compression, redundancy drops);
/// relevance only decides how strongly the budget ladder may demote a
/// chunk when the assembled context does not fit. Without budget pressure
/// the wrapper changes nothing: every decision stands.
pub struct RelevanceScorer<'a, S: jev::ContextScorer> {
    base: &'a S,
    relevance_by_index: HashMap<usize, f32>,
}

impl<'a, S: jev::ContextScorer> RelevanceScorer<'a, S> {
    /// Wrap `base` with per-message relevance weights keyed by position in
    /// the assembled transcript (see [`relevance_by_message_index`]).
    #[must_use]
    pub fn new(base: &'a S, relevance_by_index: HashMap<usize, f32>) -> Self {
        Self {
            base,
            relevance_by_index,
        }
    }
}

impl<S: jev::ContextScorer> jev::ContextScorer for RelevanceScorer<'_, S> {
    fn score(&self, transcript: &Transcript<Common>) -> Vec<jev::ContextItemScore> {
        let mut items = self.base.score(transcript);
        for (index, item) in items.iter_mut().enumerate() {
            let Some(relevance) = self.relevance_by_index.get(&index) else {
                continue; // no weight registered: keep the base score
            };
            let factor = RELEVANCE_FLOOR_WEIGHT + (1.0 - RELEVANCE_FLOOR_WEIGHT) * relevance;
            item.importance = (item.importance * factor).clamp(0.0, 1.0);
        }
        items
    }
}

/// Each assembled message's retrieval relevance, keyed by message position
/// — the exact mirror of [`assemble`]'s layout: message 0 is the query
/// (always fully important), message `i + 1` carries the `i`-th least
/// relevant chunk. This mapping and `assemble` must stay in sync; both
/// live in this module so the coupling has one home.
fn relevance_by_message_index(chunks: &[RetrievedContext]) -> HashMap<usize, f32> {
    let mut ascending: Vec<f32> = chunks.iter().map(|chunk| chunk.relevance).collect();
    ascending.sort_by(f32::total_cmp);
    let mut by_index = HashMap::with_capacity(ascending.len() + 1);
    by_index.insert(0usize, 1.0);
    for (index, relevance) in ascending.into_iter().enumerate() {
        by_index.insert(index + 1, relevance);
    }
    by_index
}

// ── assembly: chunks → Transcript<Common> ──────────────────────────────

/// Assemble retrieved chunks into one `Transcript<Common>` the Jev pipeline
/// can optimize: one user message per chunk, each carrying a provenance
/// header naming its source locator, and the query leading. Freshly built —
/// nothing in any stored session is touched.
///
/// Chunks are appended **in ascending relevance order**: message `i + 1`
/// carries the `i`-th least relevant chunk. [`RelevanceScorer`] turns that
/// order into per-message importance, so the optimizer's demotion ladder
/// sheds the weakest chunks first when a budget forces cuts — exactly the
/// ranking retrieval established.
#[must_use]
pub fn assemble(chunks: &[RetrievedContext], query: &str) -> Transcript<Common> {
    let id = format!("jev-retrieval-{}", Utc::now().timestamp_millis());
    let mut ranked: Vec<&RetrievedContext> = chunks.iter().collect();
    ranked.sort_by(|a, b| {
        a.relevance
            .partial_cmp(&b.relevance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let total = ranked.len();
    let mut messages = Vec::with_capacity(chunks.len() + 1);
    for (n, chunk) in ranked.into_iter().enumerate() {
        let text = format!(
            "[retrieved {} of {} · source {} · relevance {:.2}]\n{}",
            n + 1,
            total,
            chunk.source.locator(),
            chunk.relevance,
            chunk.content,
        );
        messages.push(Message {
            role: Role::User,
            content: vec![Block::Text { text }],
            timestamp: chunk.session_time,
            model: None,
            stop_reason: None,
            usage: None,
        });
    }
    // The query itself leads the assembled context, so the optimizer sees
    // what the handoff is for.
    let ask = format!("[context request]\n{query}");
    messages.insert(
        0,
        Message {
            role: Role::User,
            content: vec![Block::Text { text: ask }],
            timestamp: chunks
                .first()
                .map(|c| c.session_time)
                .unwrap_or_else(Utc::now),
            model: None,
            stop_reason: None,
            usage: None,
        },
    );
    Transcript::new(
        Meta {
            id,
            timestamp: Utc::now(),
            cwd: None,
            git_branch: None,
            title: Some("Jev retrieved context".to_string()),
            cli_version: None,
            model: None,
            lineage: None,
        },
        messages,
    )
}

/// Estimated tokens of a transcript body ([`jev::estimate_message_tokens`])
/// — the number `context --budget` compares against.
fn body_tokens(transcript: &Transcript<Common>) -> usize {
    transcript
        .body
        .iter()
        .map(jev::estimate_message_tokens)
        .sum()
}

// ── the pipeline: retrieve → optimize ──────────────────────────────────

/// What one pass of the full pipeline produced.
#[derive(Debug, Clone)]
pub struct RetrievedHandoff {
    /// Chunks retrieval selected, ranked most-relevant first.
    pub chunks: Vec<RetrievedContext>,
    /// The assembled transcript *before* optimization (every chunk intact).
    pub assembled: Transcript<Common>,
    /// The transcript *after* the existing Jev pipeline applied its
    /// keep/compress/drop decisions to the assembled context.
    pub optimized: Transcript<Common>,
    /// Jev's own accounting of the optimization step.
    pub report: jev::AllocationReport,
    /// Estimated tokens of the assembled (pre-optimization) context.
    pub assembled_tokens: usize,
    /// Estimated tokens of the optimized context.
    pub optimized_tokens: usize,
    /// How long retrieval took.
    pub retrieval_latency: std::time::Duration,
    /// How long optimization took.
    pub optimization_latency: std::time::Duration,
}

/// The full pipeline: retrieve → assemble → optimize through the existing
/// Jev machinery. `budget` is the final context budget in estimated tokens
/// (the same `--budget` the `continue --jev` path takes); retrieval has its
/// own `max_tokens` pre-gate in `options`. Retrieval relevance is carried
/// into the optimizer via [`RelevanceScorer`]: when the assembled context
/// cannot meet the budget, the least relevant chunks are shed first.
///
/// # Errors
/// When retrieval fails, or the Jev pipeline rejects the assembled context
/// (a well-formed assembly always applies).
pub fn retrieve_and_optimize<R: ContextRetriever>(
    retriever: &R,
    query: &str,
    options: &RetrievalOptions,
    budget: Option<usize>,
    scorer: &impl jev::ContextScorer,
) -> crate::Result<RetrievedHandoff> {
    let started = Instant::now();
    let chunks = retriever.retrieve(query, options)?;
    let retrieval_latency = started.elapsed();

    let assembled = assemble(&chunks, query);
    let assembled_tokens = body_tokens(&assembled);

    let opt_started = Instant::now();
    // Retrieval's ranking rides into Jev as importance: a budget that the
    // assembled context cannot meet sheds the least relevant chunks first,
    // never an arbitrary one.
    let weighted = RelevanceScorer::new(scorer, relevance_by_message_index(&chunks));
    let planned = jev::plan_with(&assembled, budget, &weighted);
    let (allocated, report) = jev::allocate(&assembled, &planned);
    let optimized =
        jev::apply(&assembled, &allocated).map_err(|e| crate::Error::Unconvertible {
            harness: "retrieval",
            detail: format!("assembled context failed to optimize: {e}"),
        })?;
    let optimization_latency = opt_started.elapsed();

    Ok(RetrievedHandoff {
        optimized_tokens: body_tokens(&optimized),
        chunks,
        assembled,
        optimized,
        report,
        assembled_tokens,
        retrieval_latency,
        optimization_latency,
    })
}

/// [`retrieve_and_optimize`] with the default deterministic scorer.
///
/// # Errors
/// Same as [`retrieve_and_optimize`].
pub fn retrieve_and_optimize_default<R: ContextRetriever>(
    retriever: &R,
    query: &str,
    options: &RetrievalOptions,
    budget: Option<usize>,
) -> crate::Result<RetrievedHandoff> {
    retrieve_and_optimize(
        retriever,
        query,
        options,
        budget,
        &jev::DeterministicScorer::default(),
    )
}
