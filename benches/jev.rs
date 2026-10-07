#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used, missing_docs)]

//! Jev benchmarks on the same synthetic 200-message session as the codec
//! bench: scoring, budget allocation, plan application (crop plus the
//! deterministic compressor), and the full handoff pipeline through a
//! target codec. Reproducible on any machine; numbers are planning-cost
//! measurements, not token claims.

use std::hint::black_box;

use chrono::DateTime;
use contextleleo::common::{Block, Message, Meta, Role, Tool, ToolOutput};
use contextleleo::harness::simple::Simple;
use contextleleo::jev;
use contextleleo::{Common, Transcript};
use criterion::{Criterion, criterion_group, criterion_main};

/// 50 exchanges of user ask → assistant thinking+text → Read call → result:
/// 200 messages, in the shape and size of a real working session. A third
/// of the tool results are oversized (multi-KB blobs) so the scorer has
/// `Compress` candidates to weigh, and every session repeats an identical
/// build result so it has `Drop` candidates too.
fn synthetic_session() -> Transcript<Common> {
    let ts = |secs: i64| DateTime::from_timestamp(1_780_000_000 + secs, 0).unwrap();
    let meta = Meta {
        id: "bench-1".to_string(),
        timestamp: ts(0),
        cwd: Some("/work/repo".to_string()),
        git_branch: Some("main".to_string()),
        title: Some("bench session".to_string()),
        cli_version: Some("1.2.3".to_string()),
        model: Some("claude-opus-4-8".to_string()),
        lineage: None,
    };
    let oversized = "line of build output\n".repeat(160);
    let repeated = "cached: ok";
    let mut body = Vec::new();
    for i in 0..50i64 {
        body.push(Message {
            role: Role::User,
            content: vec![Block::Text {
                text: format!("please fix failure {i} in the parser, it drops the {i}th field"),
            }],
            timestamp: ts(i * 4),
            model: None,
            stop_reason: None,
            usage: None,
        });
        body.push(Message {
            role: Role::Assistant,
            content: vec![
                Block::Thinking {
                    text: format!("field {i} is skipped when the index wraps at the boundary"),
                    signature: None,
                    encrypted: None,
                },
                Block::Text {
                    text: format!("Patching the bound for field {i}."),
                },
                Block::ToolUse {
                    id: format!("call-{i}"),
                    tool: Tool::Edit {
                        file_path: format!("/work/repo/src/parser_{i}.rs"),
                        old_string: format!("i <= {i}"),
                        new_string: format!("i < {i}"),
                        replace_all: false,
                    },
                },
            ],
            timestamp: ts(i * 4 + 1),
            model: Some("claude-opus-4-8".to_string()),
            stop_reason: None,
            usage: None,
        });
        body.push(Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                tool_use_id: format!("call-{i}"),
                // One blob in three is oversized; the rest repeat so the
                // redundancy path sees real duplicates.
                content: ToolOutput::Text(if i % 3 == 0 {
                    oversized.clone()
                } else {
                    repeated.to_string()
                }),
                is_error: false,
            }],
            timestamp: ts(i * 4 + 2),
            model: None,
            stop_reason: None,
            usage: None,
        });
    }
    Transcript::new(meta, body)
}

fn bench_jev(c: &mut Criterion) {
    let transcript = synthetic_session();

    c.bench_function("jev/plan_200", |b| {
        b.iter(|| jev::plan(black_box(&transcript), black_box(None)));
    });
    c.bench_function("jev/plan_budget_48k_200", |b| {
        b.iter(|| jev::plan(black_box(&transcript), black_box(Some(48_000))));
    });

    let natural = jev::plan(&transcript, None);
    c.bench_function("jev/allocate_no_budget_200", |b| {
        b.iter(|| jev::allocate(black_box(&transcript), black_box(&natural)));
    });

    // A tight budget exercises the demotion ladder for real: the natural
    // selection already fits 12k under the compressed cost model, so 5k
    // forces prose folds and pair drops.
    let mut budgeted = natural.clone();
    budgeted.budget = Some(5_000);
    c.bench_function("jev/allocate_budget_5k_200", |b| {
        b.iter(|| jev::allocate(black_box(&transcript), black_box(&budgeted)));
    });

    let (allocated, _report) = jev::allocate(&transcript, &budgeted);
    c.bench_function("jev/apply_200", |b| {
        b.iter(|| jev::apply(black_box(&transcript), black_box(&allocated)).unwrap());
    });

    c.bench_function("jev/handoff_to_simple_200", |b| {
        b.iter(|| {
            jev::handoff_with_default_scorer::<Simple>(
                black_box(&transcript),
                black_box(Some(5_000)),
            )
            .unwrap();
        });
    });
}

criterion_group!(benches, bench_jev);
criterion_main!(benches);
