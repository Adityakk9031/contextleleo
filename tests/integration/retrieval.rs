#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

//! Integration tests for the Jev retrieval layer: deterministic ranking,
//! signal overlap, noise filtering, budgeted selection, the pipeline into
//! the existing Jev optimizer, and read-only guarantees over the sources.

use chrono::{DateTime, TimeZone, Utc};
use contextleleo::common::{Block, Message, Meta, Role, Tool, ToolOutput};
use contextleleo::harness::simple::Simple;
use contextleleo::retrieval::{
    ContextRetriever, IndexRetriever, RetrievalOptions, Signal, assemble,
    retrieve_and_optimize_default,
};
use contextleleo::search::{DocKey, Index};
use contextleleo::{Codec, Common, Transcript};

fn ts(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_780_000_000 + secs, 0).single().unwrap()
}

fn meta(id: &str, age_days_ago: i64, cwd: &str) -> Meta {
    Meta {
        id: id.to_string(),
        timestamp: ts(-age_days_ago * 86_400),
        cwd: Some(cwd.to_string()),
        git_branch: Some("main".to_string()),
        title: Some(format!("session {id}")),
        cli_version: None,
        model: None,
        lineage: None,
    }
}

fn text_message(role: Role, text: &str, secs: i64) -> Message {
    Message {
        role,
        content: vec![Block::Text {
            text: text.to_string(),
        }],
        timestamp: ts(secs),
        model: None,
        stop_reason: None,
        usage: None,
    }
}

/// A session with one oversized Redis tool result, in the shape the
/// scenario tests need.
fn redis_session(id: &str, age_days: i64, oversized: bool) -> Transcript<Common> {
    let result_text = if oversized {
        format!(
            "connection error: redis maxConnections timeout\n{}",
            "log line\n".repeat(200)
        )
    } else {
        "redis ok".to_string()
    };
    Transcript::new(
        meta(id, age_days, "/work/repo"),
        vec![
            text_message(Role::User, "Fix the Redis timeout bug.", 0),
            Message {
                role: Role::Assistant,
                content: vec![
                    Block::ToolUse {
                        id: "call-1".to_string(),
                        tool: Tool::from_canonical(
                            "Bash",
                            serde_json::json!({"command": "redis-cli CONFIG GET maxConnections"}),
                        ),
                    },
                    Block::Text {
                        text: "maxConnections was increased from 20 to 100.".to_string(),
                    },
                ],
                timestamp: ts(1),
                model: None,
                stop_reason: None,
                usage: None,
            },
            Message {
                role: Role::User,
                content: vec![Block::ToolResult {
                    tool_use_id: "call-1".to_string(),
                    content: ToolOutput::Text(result_text),
                    is_error: oversized,
                }],
                timestamp: ts(2),
                model: None,
                stop_reason: None,
                usage: None,
            },
        ],
    )
}

fn index_of(transcripts: &[Transcript<Common>]) -> Index {
    let mut index = Index::new();
    for t in transcripts {
        index.insert(
            DocKey {
                harness: contextleleo::HarnessId::Simple,
                id: t.meta.id.clone(),
                source: None,
            },
            t,
        );
    }
    index
}

const NOW_SECS: i64 = 1000;

fn retrieve(
    index: &Index,
    query: &str,
    options: &RetrievalOptions,
) -> Vec<contextleleo::retrieval::RetrievedContext> {
    IndexRetriever::new(index)
        .at(ts(NOW_SECS))
        .retrieve(query, options)
        .unwrap()
}

// ── ranking ──────────────────────────────────────────────────────────

#[test]
fn relevant_session_ranks_above_irrelevant() {
    let redis = redis_session("redis-fix", 2, false);
    let auth = Transcript::new(
        meta("auth-fix", 1, "/work/repo"),
        vec![text_message(
            Role::User,
            "The login page token refresh fails silently.",
            0,
        )],
    );
    let index = index_of(&[redis, auth]);
    let hits = retrieve(
        &index,
        "Fix the Redis timeout bug — what configuration did we change?",
        &RetrievalOptions::default(),
    );
    assert!(!hits.is_empty(), "the redis session must surface");
    assert!(
        hits.iter().all(|hit| hit.source.session.id == "redis-fix"),
        "only the redis session is relevant, got {:?}",
        hits.iter()
            .map(|h| h.source.session.id.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn file_and_symbol_overlap_improves_ranking() {
    // Both sessions mention redis; one also touches the file and symbol the
    // query names. The overlap must win.
    let plain = Transcript::new(
        meta("plain", 0, "/work/repo"),
        vec![text_message(
            Role::User,
            "redis maxConnections timeout again, checked redis-cli",
            0,
        )],
    );
    let rich = Transcript::new(
        meta("rich", 30, "/work/repo"),
        vec![text_message(
            Role::User,
            "raised maxConnections in src/config.rs pool.rs after the redis timeout",
            0,
        )],
    );
    let index = index_of(&[plain, rich]);
    let hits = retrieve(
        &index,
        "redis maxConnections timeout in src/config.rs",
        &RetrievalOptions::default(),
    );
    assert!(hits.len() >= 2);
    let top = &hits[0];
    assert_eq!(
        top.source.session.id, "rich",
        "file-path overlap ranks first"
    );
    assert!(top.signals.contains(&Signal::FileOverlap));
}

#[test]
fn error_overlap_is_ranked() {
    let error = Transcript::new(
        meta("err", 3, "/work/repo"),
        vec![Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                tool_use_id: "c".to_string(),
                content: ToolOutput::Text("connection timeout: redis pool exhausted".to_string()),
                is_error: true,
            }],
            timestamp: ts(0),
            model: None,
            stop_reason: None,
            usage: None,
        }],
    );
    let quiet = Transcript::new(
        meta("quiet", 0, "/work/repo"),
        vec![text_message(Role::User, "redis timeout discussion", 0)],
    );
    let index = index_of(&[error, quiet]);
    let hits = retrieve(
        &index,
        "redis connection timeout error",
        &RetrievalOptions::default(),
    );
    assert!(!hits.is_empty());
    let err_hit = hits
        .iter()
        .find(|h| h.source.session.id == "err")
        .expect("error session surfaces");
    assert!(err_hit.signals.contains(&Signal::ErrorOverlap));
}

#[test]
fn irrelevant_sessions_are_filtered() {
    let noise = Transcript::new(
        meta("noise", 0, "/work/repo"),
        vec![text_message(
            Role::User,
            "unrelated note about the frontend login styling",
            0,
        )],
    );
    let index = index_of(&[noise]);
    let hits = retrieve(
        &index,
        "redis maxConnections timeout",
        &RetrievalOptions::default(),
    );
    assert!(
        hits.is_empty(),
        "a session sharing no terms must not surface"
    );
}

#[test]
fn multiple_sessions_can_be_returned() {
    let a = redis_session("redis-fix", 2, false);
    let b = Transcript::new(
        meta("redis-incident", 40, "/work/repo"),
        vec![text_message(
            Role::User,
            "production redis timeout incident recap",
            0,
        )],
    );
    let index = index_of(&[a, b]);
    let hits = retrieve(&index, "redis timeout", &RetrievalOptions::default());
    let sessions: std::collections::HashSet<&str> =
        hits.iter().map(|h| h.source.session.id.as_str()).collect();
    assert!(sessions.len() >= 2, "both redis sessions surface");
    // Recency and phrase overlap put the fix session first.
    assert_eq!(hits[0].source.session.id, "redis-fix");
}

#[test]
fn source_references_remain_valid() {
    let redis = redis_session("redis-fix", 2, false);
    let index = index_of(&[redis]);
    let hits = retrieve(&index, "redis maxConnections", &RetrievalOptions::default());
    assert!(!hits.is_empty());
    for hit in hits {
        let locator = hit.source.locator();
        assert!(
            locator.starts_with("simple:redis-fix#"),
            "locator: {locator}"
        );
        // The 1-based message number addresses a real message.
        if let Some(index) = hit.source.message_index {
            assert!(hit.span_valid(index));
        }
    }
}

trait SpanCheck {
    fn span_valid(&self, index: usize) -> bool;
}

impl SpanCheck for contextleleo::retrieval::RetrievedContext {
    fn span_valid(&self, index: usize) -> bool {
        index < 3 // the redis_session fixture has 3 messages
    }
}

// ── assembly → optimizer ─────────────────────────────────────────────

#[test]
fn assembly_is_traceable_and_carries_the_query() {
    let chunks = vec![contextleleo::retrieval::RetrievedContext {
        source: contextleleo::retrieval::SourceReference {
            session: DocKey {
                harness: contextleleo::HarnessId::Simple,
                id: "redis-fix".to_string(),
                source: None,
            },
            message_index: Some(1),
        },
        content: "maxConnections was increased from 20 to 100.".to_string(),
        relevance: 0.9,
        signals: vec![Signal::Keyword],
        session_time: ts(0),
    }];
    let assembled = assemble(&chunks, "What configuration did we change?");
    assert_eq!(assembled.body.len(), 2);
    let Block::Text { text } = &assembled.body[1].content[0] else {
        panic!("expected text");
    };
    assert!(
        text.contains("simple:redis-fix#2"),
        "source locator present"
    );
    assert!(text.contains("maxConnections was increased from 20 to 100."));
    assert!(
        assembled.body[0].content[0]
            .block_text()
            .contains("What configuration did we change?")
    );
}

trait BlockText {
    fn block_text(&self) -> &str;
}

impl BlockText for Block {
    fn block_text(&self) -> &str {
        match self {
            Block::Text { text } => text,
            _ => "",
        }
    }
}

#[test]
fn pipeline_retrieves_optimizes_and_respects_budget() {
    let big = redis_session("redis-fix", 2, true); // oversized tool result
    let index = index_of(&[big]);
    let retriever = IndexRetriever::new(&index).at(ts(NOW_SECS));
    // The retrieval pre-gate is the first budget gate: chunk selection is
    // capped before Jev ever sees the assembly. With a Jev budget above the
    // capped assembly, the honest outcome is `Fits` — nothing needs
    // demoting, and the optimized copy is the assembly unchanged.
    let options = RetrievalOptions {
        max_chunks: 8,
        max_tokens: Some(60),
        ..RetrievalOptions::default()
    };
    let handoff = retrieve_and_optimize_default(
        &retriever,
        "redis maxConnections timeout",
        &options,
        Some(400),
    )
    .unwrap();

    assert!(!handoff.chunks.is_empty());
    let selected: usize = handoff.chunks.iter().map(|chunk| chunk.tokens()).sum();
    assert!(
        selected <= 60,
        "pre-gate capped chunk selection, got {selected}"
    );
    assert_eq!(
        handoff.report.status,
        contextleleo::jev::AllocationStatus::Fits,
        "a capped assembly that fits the budget is returned untouched"
    );
    assert_eq!(
        handoff.assembled_tokens, handoff.optimized_tokens,
        "nothing was demoted, so the optimized copy is the assembly"
    );
    // Every surviving content block still names its source or is the ask.
    for message in &handoff.optimized.body {
        for block in &message.content {
            if let Block::Text { text } = block {
                assert!(
                    text.contains("jev:")
                        || text.contains("[retrieved")
                        || text.contains("[context request]"),
                    "traceable: {text}"
                );
            }
        }
    }
}

#[test]
fn pipeline_sheds_the_least_relevant_chunks_first() {
    // Two sessions: one small chunk that matches everything (high
    // relevance, protected), one oversized chunk that barely matches (low
    // relevance, demotable). A tight budget must shed the big weak chunk —
    // through the existing Jev ladder, not retrieval-side truncation.
    let err_log = Transcript::new(
        meta("err-log", 1, "/work/repo"),
        vec![Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                tool_use_id: "c".to_string(),
                content: ToolOutput::Text(
                    "connection error: redis maxConnections timeout".to_string(),
                ),
                is_error: true,
            }],
            timestamp: ts(0),
            model: None,
            stop_reason: None,
            usage: None,
        }],
    );
    // One single 2 500-char line: matches only "redis", so its relevance
    // stays low while its token cost dominates the assembly.
    let weak_line = format!(
        "redis pool sizing tuning notes {}",
        "capacity planning ".repeat(150)
    );
    let pool_notes = Transcript::new(
        meta("pool-notes", 2, "/work/repo"),
        vec![text_message(Role::User, &weak_line, 0)],
    );
    let index = index_of(&[err_log, pool_notes]);
    let retriever = IndexRetriever::new(&index).at(ts(NOW_SECS));
    let handoff = retrieve_and_optimize_default(
        &retriever,
        "redis maxConnections timeout",
        &RetrievalOptions::default(),
        Some(200),
    )
    .unwrap();

    assert_eq!(handoff.chunks.len(), 2, "one chunk per session");
    assert!(
        handoff.assembled_tokens > 200,
        "the fixture must exceed the budget"
    );
    assert_eq!(
        handoff.report.status,
        contextleleo::jev::AllocationStatus::Demoted,
        "budget pressure reached the Jev allocator"
    );
    assert!(
        handoff.optimized_tokens < handoff.assembled_tokens,
        "compression actually shrank the context ({} → {} tokens)",
        handoff.assembled_tokens,
        handoff.optimized_tokens
    );
    assert!(
        handoff.optimized_tokens <= 200 + 40,
        "the oversized chunk's stand-in is small enough to meet the budget ({} tokens)",
        handoff.optimized_tokens
    );
    // The weak chunk assembled first (ascending relevance) and was the one
    // compressed: its text became a truncation and the stand-in reference
    // was appended beside it; the strong chunk survives verbatim.
    let weak_texts: Vec<&str> = handoff.optimized.body[1]
        .content
        .iter()
        .filter_map(|block| match block {
            Block::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        weak_texts
            .iter()
            .any(|text| text.contains("[jev: truncated]")),
        "the oversized weak chunk was truncated: {weak_texts:?}"
    );
    assert!(
        weak_texts
            .iter()
            .any(|text| text.contains("[jev: compressed from")),
        "the weak chunk carries the compression reference: {weak_texts:?}"
    );
    let strong_text = &handoff.optimized.body[2].content[0];
    let Block::Text { text: strong_out } = strong_text else {
        panic!("expected text");
    };
    assert!(
        strong_out.contains("connection error: redis maxConnections timeout"),
        "the strongest chunk survives the budget intact: {strong_out}"
    );
    assert!(
        !strong_out.contains("[jev:"),
        "the protected chunk was not compressed"
    );
    for message in &handoff.optimized.body {
        for block in &message.content {
            if let Block::Text { text } = block {
                assert!(
                    text.contains("jev:")
                        || text.contains("[retrieved")
                        || text.contains("[context request]"),
                    "traceable: {text}"
                );
            }
        }
    }
}

#[test]
fn pipeline_without_budget_returns_everything_selected() {
    let big = redis_session("redis-fix", 2, true);
    let index = index_of(&[big]);
    let retriever = IndexRetriever::new(&index).at(ts(NOW_SECS));
    let options = RetrievalOptions::default();
    let handoff =
        retrieve_and_optimize_default(&retriever, "redis timeout", &options, None).unwrap();
    assert_eq!(handoff.assembled_tokens, handoff.optimized_tokens);
    assert_eq!(
        handoff.report.counts[2], 0,
        "nothing dropped without budget pressure"
    );
}

#[test]
fn sources_are_never_mutated_by_the_pipeline() {
    let big = redis_session("redis-fix", 2, true);
    let before = big.clone();
    let index = index_of(std::slice::from_ref(&big));
    let retriever = IndexRetriever::new(&index).at(ts(NOW_SECS));
    let _ = retrieve_and_optimize_default(
        &retriever,
        "redis timeout",
        &RetrievalOptions::default(),
        Some(40),
    )
    .unwrap();
    // The indexed transcript is unchanged (the index borrows it); the
    // assembled/optimized copies are fresh objects.
    assert_eq!(big, before);
}

#[test]
fn retrieved_tool_results_stay_coherent_through_a_handoff() {
    let big = redis_session("redis-fix", 2, false);
    let index = index_of(&[big]);
    let retriever = IndexRetriever::new(&index).at(ts(NOW_SECS));
    let handoff = retrieve_and_optimize_default(
        &retriever,
        "redis maxConnections timeout",
        &RetrievalOptions::default(),
        Some(400),
    )
    .unwrap();
    // The optimized context converts into a real harness shape and back,
    // proving the assembled context is a well-formed conversation.
    let native = Simple::from_common(&handoff.optimized).unwrap();
    let round = Simple::to_common(&native).unwrap();
    assert!(!round.body.is_empty());
    let text = round.body[0].content[0].block_text().to_string();
    assert!(text.contains("[context request]"));
}
