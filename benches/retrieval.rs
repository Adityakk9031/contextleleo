#![allow(
    clippy::cast_possible_wrap,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    missing_docs
)]

//! Retrieval benchmarks over a synthetic 40-session store: candidate search
//! plus deterministic ranking, chunk assembly, the full retrieve → assemble
//! → Jev-optimize pipeline, and — for comparison — Jev alone over the full
//! concatenated history the pipeline replaces. Reproducible on any machine;
//! numbers are cost measurements, not token claims.

use std::hint::black_box;

use chrono::{DateTime, TimeZone, Utc};
use contextleleo::common::{Block, Message, Meta, Role, Tool, ToolOutput};
use contextleleo::harness::simple::Simple;
use contextleleo::jev;
use contextleleo::retrieval::{
    ContextRetriever, IndexRetriever, RetrievalOptions, assemble, retrieve_and_optimize_default,
};
use contextleleo::search::{DocKey, Index};
use contextleleo::{Common, Transcript};
use criterion::{Criterion, criterion_group, criterion_main};

/// Sessions in the synthetic store: 5 partial matches for the query and 35
/// unrelated filler sessions, the shape of a real session directory.
const SESSIONS: usize = 40;

/// The handoff query: every term deliberately matches only the redis
/// sessions, so ranking has real work to do.
const QUERY: &str = "redis maxConnections timeout pool exhausted";

/// The Jev budget the pipeline bench plans against.
const BUDGET: usize = 600;

fn ts(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_780_000_000 + secs, 0).unwrap()
}

fn meta(id: &str, age_days: i64) -> Meta {
    Meta {
        id: id.to_string(),
        timestamp: ts(-age_days * 86_400),
        cwd: Some("/work/repo".to_string()),
        git_branch: Some("main".to_string()),
        title: Some(format!("session {id}")),
        cli_version: None,
        model: None,
        lineage: None,
    }
}

fn text(role: Role, text: impl Into<String>, secs: i64) -> Message {
    Message {
        role,
        content: vec![Block::Text { text: text.into() }],
        timestamp: ts(secs),
        model: None,
        stop_reason: None,
        usage: None,
    }
}

fn tool(command: impl Into<String>, secs: i64) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![Block::ToolUse {
            id: "call".to_string(),
            tool: Tool::from_canonical("Bash", serde_json::json!({ "command": command.into() })),
        }],
        timestamp: ts(secs),
        model: None,
        stop_reason: None,
        usage: None,
    }
}

fn result(text: impl Into<String>, secs: i64, is_error: bool) -> Message {
    Message {
        role: Role::User,
        content: vec![Block::ToolResult {
            tool_use_id: "call".to_string(),
            content: ToolOutput::Text(text.into()),
            is_error,
        }],
        timestamp: ts(secs),
        model: None,
        stop_reason: None,
        usage: None,
    }
}

/// A redis/timeout session: the query's target shape, with an oversized
/// tool result so the optimizer has something to compress.
fn redis_session(index: usize) -> Transcript<Common> {
    let oversized = format!(
        "connection timeout: redis pool exhausted\n{}",
        "log line\n".repeat(120)
    );
    Transcript::new(
        meta(&format!("bench-redis-{index:03}"), (index as i64 % 30) + 1),
        vec![
            text(Role::User, "Fix the Redis timeout issue in production.", 0),
            tool("redis-cli CONFIG GET maxConnections", 1),
            text(
                Role::Assistant,
                "maxConnections was increased from 20 to 100 — that fixed the timeout.",
                2,
            ),
            result(oversized, 3, true),
        ],
    )
}

/// An unrelated filler session: same shape, disjoint vocabulary, one small
/// tool result so the index has comparable density across documents.
fn filler_session(index: usize, topic: &str) -> Transcript<Common> {
    let first_word = topic.split(' ').next().unwrap_or("issue");
    Transcript::new(
        meta(&format!("bench-filler-{index:03}"), (index as i64 % 60) + 1),
        vec![
            text(
                Role::User,
                format!("Investigate the {topic} problem reported this week."),
                0,
            ),
            tool(format!("grep -rn {first_word} src/"), 1),
            text(
                Role::Assistant,
                format!("The {topic} issue is fixed; nothing else changed."),
                2,
            ),
            result("ok, 3 matches\n".repeat(40), 3, false),
        ],
    )
}

const TOPICS: [&str; 7] = [
    "authentication token refresh",
    "css grid layout overflow",
    "database migration rollback",
    "webpack build cache",
    "kubernetes pod restart",
    "logging rotation policy",
    "websocket reconnect jitter",
];

/// Build the store and its search index once; the benches borrow it.
fn build() -> (Vec<Transcript<Common>>, Index) {
    let mut sessions = Vec::with_capacity(SESSIONS);
    for i in 0..SESSIONS {
        if i % 8 == 0 {
            sessions.push(redis_session(i));
        } else {
            sessions.push(filler_session(i, TOPICS[i % TOPICS.len()]));
        }
    }
    let mut index = Index::new();
    for session in &sessions {
        index.insert(
            DocKey {
                harness: contextleleo::HarnessId::Simple,
                id: session.meta.id.clone(),
                source: None,
            },
            session,
        );
    }
    (sessions, index)
}

/// What "hand off everything" would cost: every session's body in one
/// transcript, the input Jev gets on the no-retrieval path.
fn full_history(sessions: &[Transcript<Common>]) -> Transcript<Common> {
    let body = sessions
        .iter()
        .flat_map(|s| s.body.iter().cloned())
        .collect();
    Transcript::new(meta("bench-full-history", 0), body)
}

fn bench_retrieval(c: &mut Criterion) {
    let (sessions, index) = build();
    let retriever = IndexRetriever::new(&index).at(ts(200 * 86_400));
    let options = RetrievalOptions::default();
    let history = full_history(&sessions);

    let mut group = c.benchmark_group("retrieval");

    group.bench_function("rank_and_select_40_sessions", |b| {
        b.iter(|| {
            retriever
                .retrieve(black_box(QUERY), black_box(&options))
                .expect("retrieval succeeds")
        });
    });

    let chunks = retriever
        .retrieve(QUERY, &options)
        .expect("retrieval succeeds");
    group.bench_function("assemble_chunks", |b| {
        b.iter(|| assemble(black_box(&chunks), black_box(QUERY)));
    });

    group.bench_function("pipeline_with_jev_budget", |b| {
        b.iter(|| {
            retrieve_and_optimize_default(
                black_box(&retriever),
                black_box(QUERY),
                black_box(&options),
                black_box(Some(BUDGET)),
            )
            .expect("pipeline succeeds")
        });
    });

    group.bench_function("jev_full_history_no_retrieval", |b| {
        b.iter(|| {
            jev::handoff::<Simple>(
                black_box(&history),
                black_box(Some(BUDGET)),
                &jev::DeterministicScorer::default(),
            )
            .expect("handoff succeeds")
        });
    });

    group.finish();
}

criterion_group!(benches, bench_retrieval);
criterion_main!(benches);
