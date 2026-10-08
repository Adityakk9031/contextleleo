//! The retrieval layer end to end, measured: a synthetic 44-session store,
//! one handoff query, three numbers —
//!
//! 1. **Full history** — every session concatenated: what a context-unaware
//!    handoff would carry to the target agent.
//! 2. **Retrieved** — the chunks the retrieval layer selects (deterministic
//!    ranking, `max_tokens` pre-gate).
//! 3. **Retrieved + Jev-optimized** — the existing Jev pipeline's
//!    keep / compress / drop pass over the assembled context.
//!
//! Read-only by construction: the store is indexed in memory and nothing
//! mutates it. Run with `cargo run --example retrieval_demo` (`search` is a
//! default feature). No network, no embeddings.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use chrono::{DateTime, TimeZone, Utc};
use contextleleo::common::{Block, Message, Meta, Role, Tool, ToolOutput};
use contextleleo::jev;
use contextleleo::retrieval::{IndexRetriever, RetrievalOptions};
use contextleleo::search::{DocKey, Index};
use contextleleo::{Common, Transcript};

fn ts(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_780_000_000 + secs, 0).unwrap()
}

fn meta(id: &str, age_days: i64, title: &str) -> Meta {
    Meta {
        id: id.to_string(),
        timestamp: ts(-age_days * 86_400),
        cwd: Some("/work/repo".to_string()),
        git_branch: Some("main".to_string()),
        title: Some(title.to_string()),
        cli_version: None,
        model: None,
        lineage: None,
    }
}

fn text(t: &str, secs: i64) -> Message {
    Message {
        role: Role::User,
        content: vec![Block::Text {
            text: t.to_string(),
        }],
        timestamp: ts(secs),
        model: None,
        stop_reason: None,
        usage: None,
    }
}

fn tool(input: &str, secs: i64) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![Block::ToolUse {
            id: format!("call-{secs}"),
            tool: Tool::from_canonical("Bash", serde_json::json!({ "command": input })),
        }],
        timestamp: ts(secs),
        model: None,
        stop_reason: None,
        usage: None,
    }
}

fn result(t: String, call_secs: i64, secs: i64, is_error: bool) -> Message {
    Message {
        role: Role::User,
        content: vec![Block::ToolResult {
            tool_use_id: format!("call-{call_secs}"),
            content: ToolOutput::Text(t),
            is_error,
        }],
        timestamp: ts(secs),
        model: None,
        stop_reason: None,
        usage: None,
    }
}

/// The handoff query a continuation would ask.
const QUERY: &str = "We fixed the Redis timeout issue before. What configuration did we change?";

// A demo script: the linear print-out is the point, and the ratio below is a
// display-only float.
#[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
fn main() {
    let now = ts(200 * 86_400);
    // Four scenario sessions plus filler, the shape of a real store: the
    // target incident, a near-miss topic, an unrelated one, and a partial
    // match.
    let scenario: Vec<(&str, i64, &str, Vec<Message>)> = vec![
        (
            "redis-timeout",
            12,
            "Redis timeout debugging",
            vec![
                text("Fix the Redis timeout issue in production.", 0),
                tool("redis-cli CONFIG GET maxConnections", 1),
                text(
                    "maxConnections was increased from 20 to 100 — that fixed the timeout.",
                    2,
                ),
                result(
                    format!(
                        "CONFIG GET maxConnections\n\"maxConnections\" \"100\"\nconnection timeout: redis pool exhausted\n{}",
                        "log line\n".repeat(600)
                    ),
                    1,
                    3,
                    true,
                ),
            ],
        ),
        (
            "pool-sizing",
            31,
            "Database connection pool changes",
            vec![
                text("Adjust the database connection pool sizing.", 0),
                tool("pg pool_size 10 -> 40", 1),
                text(
                    "Raised the pg pool_size from 10 to 40; redis config untouched.",
                    2,
                ),
            ],
        ),
        (
            "auth-mobile",
            78,
            "Frontend authentication issue",
            vec![
                text(
                    "The frontend login token refresh fails on mobile Safari.",
                    0,
                ),
                text("Fixed by clearing stale cookie sessions.", 1),
            ],
        ),
        (
            "incident",
            103,
            "Redis production incident",
            vec![
                text(
                    "Redis production incident: node ran out of memory during the timeout storm.",
                    0,
                ),
                tool("redis-cli INFO memory", 1),
                text(
                    "Root cause: the timeout storm; maxConnections tuning later prevented a repeat.",
                    2,
                ),
                result(
                    format!(
                        "used_memory_human: 6.1G\nevicted_keys: 88123\n{}",
                        "trace\n".repeat(600)
                    ),
                    1,
                    3,
                    false,
                ),
            ],
        ),
    ];

    // Index the store exactly as the `context` command does: read-only, one
    // DocKey per session. The transcripts outlive the index (insertion
    // borrows them); their token totals are the no-retrieval baseline.
    let mut corpus: Vec<Transcript<Common>> = Vec::new();
    for (id, days, title, msgs) in &scenario {
        corpus.push(Transcript::new(meta(id, *days, title), msgs.clone()));
    }
    for i in 0..40 {
        let id = format!("filler-{i}");
        corpus.push(Transcript::new(
            meta(&id, 5 + i64::from(i) % 60, &format!("filler session {i}")),
            vec![text(
                &format!("routine work item {i}: updated dependencies and docs."),
                0,
            )],
        ));
    }
    let mut index = Index::new();
    let mut all_tokens = 0usize;
    for transcript in &corpus {
        all_tokens += transcript
            .body
            .iter()
            .map(jev::estimate_message_tokens)
            .sum::<usize>();
        index.insert(
            DocKey {
                harness: contextleleo::HarnessId::Simple,
                id: transcript.meta.id.clone(),
                source: None,
            },
            transcript,
        );
    }
    let sessions_indexed = corpus.len();

    let retriever = IndexRetriever::new(&index).at(now);
    let options = RetrievalOptions {
        max_chunks: 20,
        ..RetrievalOptions::default()
    };

    let handoff = contextleleo::retrieval::retrieve_and_optimize_default(
        &retriever,
        QUERY,
        &options,
        Some(120),
    )
    .expect("pipeline succeeds");

    println!("Full history:      {all_tokens:>6} tokens across {sessions_indexed} sessions");
    println!(
        "Retrieved:         {:>6} tokens in {} chunks (retrieval {:?})",
        handoff.assembled_tokens,
        handoff.chunks.len(),
        handoff.retrieval_latency
    );
    println!(
        "Jev optimized:     {:>6} tokens (keep {} · compress {} · drop {}) (optimize {:?})",
        handoff.optimized_tokens,
        handoff.report.counts[0],
        handoff.report.counts[1],
        handoff.report.counts[2],
        handoff.optimization_latency
    );
    println!(
        "Compression ratio: {:.1}x full → optimized",
        all_tokens as f32 / handoff.optimized_tokens.max(1) as f32
    );
    println!("\nTop chunks (every one traceable to session#message):");
    for chunk in handoff.chunks.iter().take(5) {
        let head: String = chunk.content.chars().take(58).collect();
        println!(
            "  {:.2}  {}  {head}",
            chunk.relevance,
            chunk.source.locator()
        );
    }
    println!("\nAssembled + optimized transcript (what the target agent receives):");
    for (n, message) in handoff.optimized.body.iter().enumerate() {
        let head: String = message
            .content
            .iter()
            .filter_map(|block| match block {
                Block::Text { text } => Some(text.chars().take(72).collect::<String>()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ⏎ ");
        println!("  {n:>2} [{:?}] {head}", message.role);
    }
}
