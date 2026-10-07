#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

//! Integration tests for Freebuff's codec and its per-project SQLite store.
//! Fixtures build real `desktop-v2.db` databases with the app's schema (the
//! columns it actually writes), so discovery, load, save, and delete run
//! against the same shapes the desktop app produces.

use chrono::{DateTime, TimeZone, Utc};
use contextleleo::common::{Block, Message, Meta, Role, Tool, ToolOutput};
use contextleleo::harness::freebuff::{self, Freebuff, FreebuffStore};
use contextleleo::{Codec, Common, Store, TextCodec, Transcript};
use serde_json::{Value, json};

fn ts_millis(secs: u32) -> i64 {
    Utc.timestamp_opt(i64::from(secs), 0)
        .single()
        .unwrap()
        .timestamp_millis()
}

fn meta(id: &str, cwd: &str) -> Meta {
    Meta {
        id: id.to_string(),
        timestamp: DateTime::parse_from_rfc3339("2026-02-03T04:05:06Z")
            .unwrap()
            .with_timezone(&Utc),
        cwd: Some(cwd.to_string()),
        git_branch: Some("main".to_string()),
        title: Some("Ported thread".to_string()),
        cli_version: None,
        model: Some("m-7e20df6765".to_string()),
        lineage: None,
    }
}

fn make_store(root: &std::path::Path) -> FreebuffStore {
    let dir = root.join("projects");
    std::fs::create_dir_all(&dir).unwrap();
    FreebuffStore {
        projects_dir: dir,
        isolated: true,
    }
}

/// A project directory with a schema-complete database the app wrote:
/// the core tables, one project row keyed by path, one thread, and two
/// message rows (user text, assistant tool call with an inline result).
fn seed_project(store: &FreebuffStore, cwd: &str, thread_id: &str) -> std::path::PathBuf {
    let dir = store.projects_dir.join(format!(
        "fixture-{}",
        // Stable per-cwd label; identity matching goes through
        // project.json, never the name.
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("project.json"),
        json!({
            "version": 1,
            "projectId": uuid::Uuid::new_v4().to_string(),
            "projectPath": cwd,
            "database": "desktop-v2.db",
        })
        .to_string(),
    )
    .unwrap();

    let now = ts_millis(0);
    let conn = rusqlite::Connection::open(dir.join("desktop-v2.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE projects (
            id TEXT PRIMARY KEY, root_path TEXT NOT NULL,
            default_branch TEXT NOT NULL DEFAULT 'main', created_at INTEGER NOT NULL);
         CREATE TABLE threads (
            id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            project_path TEXT NOT NULL,
            title TEXT NOT NULL DEFAULT 'New thread',
            status TEXT NOT NULL DEFAULT 'open',
            harness_id TEXT, model TEXT, byok_connection TEXT, reasoning_effort TEXT,
            agent_mode TEXT NOT NULL DEFAULT 'build',
            execution_mode TEXT NOT NULL DEFAULT 'local',
            branch TEXT, worktree_path TEXT, source_branch TEXT, source_ref TEXT,
            base_ref TEXT, last_seen_head TEXT,
            turn_state TEXT NOT NULL DEFAULT 'idle',
            queue_paused INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
         CREATE TABLE messages (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
            request_id TEXT, input_id TEXT, origin TEXT,
            steering INTEGER NOT NULL DEFAULT 0,
            role TEXT NOT NULL,
            parts_json TEXT NOT NULL DEFAULT '[]',
            attachments_json TEXT NOT NULL DEFAULT '[]',
            metrics_json TEXT NOT NULL DEFAULT '{}',
            ts INTEGER NOT NULL);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO projects (id, root_path, created_at) VALUES (?1, ?1, ?2)",
        rusqlite::params![cwd, now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO threads (id, project_id, project_path, title, model, branch,
                              agent_mode, execution_mode, created_at, updated_at)
         VALUES (?1, ?2, ?2, 'Seeded thread', 'm-7e20df6765', 'main',
                 'build', 'local', ?3, ?4)",
        rusqlite::params![thread_id, cwd, now, ts_millis(90)],
    )
    .unwrap();
    let user_parts = json!([{"kind": "text", "text": "hello freebuff"}]);
    let tool_parts = json!([
        {"kind": "reasoning", "id": "r1", "text": "thinking", "open": false, "collapse": "preview"},
        {"kind": "tool", "id": "call-1", "toolName": "bash",
         "input": {"command": "ls"},
         "output": "stdout:\nfile.rs\n", "status": "success", "exitCode": 0}
    ]);
    conn.execute(
        "INSERT INTO messages (thread_id, role, parts_json, ts) VALUES (?1, 'user', ?2, ?3)",
        rusqlite::params![thread_id, user_parts.to_string(), ts_millis(10)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO messages (thread_id, role, parts_json, ts) VALUES (?1, 'assistant', ?2, ?3)",
        rusqlite::params![thread_id, tool_parts.to_string(), ts_millis(20)],
    )
    .unwrap();
    dir
}

fn sample_common(id: &str, cwd: &str) -> Transcript<Common> {
    let messages = vec![
        Message {
            role: Role::User,
            content: vec![Block::Text {
                text: "hello freebuff".to_string(),
            }],
            timestamp: DateTime::parse_from_rfc3339("2026-02-03T04:05:06Z")
                .unwrap()
                .with_timezone(&Utc),
            model: None,
            stop_reason: None,
            usage: None,
        },
        Message {
            role: Role::Assistant,
            content: vec![
                Block::ToolUse {
                    id: "call-1".to_string(),
                    tool: Tool::from_canonical("Bash", json!({"command": "ls"})),
                },
                Block::ToolResult {
                    tool_use_id: "call-1".to_string(),
                    content: ToolOutput::Text("stdout:\nfile.rs\n".to_string()),
                    is_error: false,
                },
            ],
            timestamp: DateTime::parse_from_rfc3339("2026-02-03T04:05:16Z")
                .unwrap()
                .with_timezone(&Utc),
            model: None,
            stop_reason: None,
            usage: None,
        },
    ];
    Transcript::new(meta(id, cwd), messages)
}

#[test]
fn codec_maps_parts_blocks_and_tool_results() {
    let thread = freebuff::ThreadRow {
        id: "t-1".to_string(),
        project_id: String::new(),
        project_path: "/tmp/proj".to_string(),
        title: "T".to_string(),
        status: "open".to_string(),
        harness_id: Some("codebuff".to_string()),
        model: Some("m-7e20df6765".to_string()),
        branch: Some("main".to_string()),
        agent_mode: "build".to_string(),
        execution_mode: "local".to_string(),
        created_at: ts_millis(0),
        updated_at: ts_millis(30),
        extra: Default::default(),
    };
    let messages = vec![
        freebuff::MessageRow {
            seq: 1,
            request_id: None,
            input_id: None,
            origin: None,
            steering: false,
            role: "user".to_string(),
            parts_json: json!([
                {"kind": "text", "text": "hi"},
                {"kind": "ad", "id": "a1"},
            ])
            .to_string(),
            attachments_json: "[]".to_string(),
            metrics_json: "{}".to_string(),
            ts: ts_millis(10),
            extra: Default::default(),
        },
        freebuff::MessageRow {
            seq: 2,
            request_id: None,
            input_id: None,
            origin: None,
            steering: false,
            role: "assistant".to_string(),
            parts_json: json!([
                {"kind": "reasoning", "id": "r1", "text": "pondering"},
                {"kind": "tool", "id": "call-1", "toolName": "run_terminal_command",
                 "input": {"command": "cargo test"},
                 "output": "stdout:\nok\n", "status": "failure", "exitCode": 101},
            ])
            .to_string(),
            attachments_json: "[]".to_string(),
            metrics_json: "{}".to_string(),
            ts: ts_millis(20),
            extra: Default::default(),
        },
    ];
    let native = Transcript::new(
        freebuff::meta_from_thread(&thread),
        freebuff::Session { thread, messages },
    );
    let common = Freebuff::to_common(&native).unwrap();
    assert_eq!(common.body.len(), 2);
    let user = &common.body[0];
    assert!(matches!(
        user.content.as_slice(),
        [Block::Text { text }] if text == "hi"
    ));
    let assistant = &common.body[1];
    assert_eq!(assistant.content.len(), 3);
    let (name, input) = match &assistant.content[1] {
        Block::ToolUse { tool, .. } => tool.to_canonical(),
        other => panic!("expected tool use, got {other:?}"),
    };
    assert_eq!(name, "Bash");
    assert_eq!(input["command"], "cargo test");
    match &assistant.content[2] {
        Block::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            assert_eq!(tool_use_id, "call-1");
            assert!(is_error);
            match content {
                ToolOutput::Text(text) => assert_eq!(text, "stdout:\nok\n"),
                other => panic!("expected text output, got {other:?}"),
            }
        }
        other => panic!("expected tool result, got {other:?}"),
    }

    // Round trip: rows rebuilt from canonical survive a second pass.
    let back = Freebuff::from_common(&common).unwrap();
    let again = Freebuff::to_common(&back).unwrap();
    assert_eq!(common.body.len(), again.body.len());
    assert_eq!(
        Value::from(
            common
                .body
                .iter()
                .map(|m| serde_json::to_value(m).unwrap())
                .collect::<Vec<_>>()
        ),
        Value::from(
            again
                .body
                .iter()
                .map(|m| serde_json::to_value(m).unwrap())
                .collect::<Vec<_>>()
        )
    );
}

#[test]
fn text_codec_round_trips_and_rejects_idless_threads() {
    let common = sample_common("t-text", "/tmp/proj");
    let native = Freebuff::from_common(&common).unwrap();
    let text = Freebuff::to_text(&native).unwrap();
    let reparsed = Freebuff::from_text(&text).unwrap();
    assert_eq!(reparsed.body.thread.id, "t-text");
    assert_eq!(reparsed.body.messages.len(), 2);

    let bad = json!({"thread": {"id": ""}, "messages": []}).to_string();
    assert!(Freebuff::from_text(&bad).is_err());
}

#[test]
#[cfg(feature = "opencode")]
fn store_discovers_loads_and_skips_foreign_databases() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_store(dir.path());
    let thread_id = "11111111-2222-4333-8444-555555555555";
    seed_project(&store, "/tmp/proj-a", thread_id);

    // A foreign SQLite file in a project directory: skipped, not fatal.
    let foreign_dir = store
        .projects_dir
        .join("foreign-00000000-0000-4000-8000-000000000000");
    std::fs::create_dir_all(&foreign_dir).unwrap();
    let conn = rusqlite::Connection::open(foreign_dir.join("desktop-v2.db")).unwrap();
    conn.execute_batch("CREATE TABLE other (x TEXT); INSERT INTO other VALUES ('1');")
        .unwrap();

    let found = store.discover().unwrap();
    assert_eq!(found.len(), 1, "foreign database must not appear");
    assert_eq!(found[0].reference, thread_id);
    assert_eq!(found[0].meta.title.as_deref(), Some("Seeded thread"));
    assert_eq!(found[0].meta.cwd.as_deref(), Some("/tmp/proj-a"));

    let loaded = store.load(&thread_id.to_string()).unwrap();
    assert_eq!(loaded.body.thread.id, thread_id);
    assert_eq!(loaded.body.messages.len(), 2);
    let common = Freebuff::to_common(&loaded).unwrap();
    assert_eq!(common.body.len(), 2);

    assert!(
        store
            .load(&"99999999-9999-4999-8999-999999999999".to_string())
            .is_err()
    );
}

#[test]
#[cfg(feature = "opencode")]
fn store_saves_into_project_by_cwd_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_store(dir.path());
    // A fixed id: an empty meta id would mint a fresh thread per save,
    // the same convention the file-backed stores use.
    let common = sample_common("rt-thread", "/tmp/proj-save");

    let saved = store
        .save(&Freebuff::from_common(&common).unwrap())
        .unwrap();
    assert!(!saved.id.is_empty());

    // The project directory was created and named for the project, with a
    // project.json pointing at the cwd.
    let entries: Vec<_> = std::fs::read_dir(&store.projects_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].join("desktop-v2.db").is_file());
    let doc: Value =
        serde_json::from_str(&std::fs::read_to_string(entries[0].join("project.json")).unwrap())
            .unwrap();
    assert_eq!(doc["projectPath"], "/tmp/proj-save");
    assert_eq!(doc["database"], "desktop-v2.db");

    // The database has the app's project row keyed by the path, and the
    // thread references it.
    let conn = rusqlite::Connection::open(entries[0].join("desktop-v2.db")).unwrap();
    let project_id: String = conn
        .query_row("SELECT id FROM projects", [], |r| r.get(0))
        .unwrap();
    assert_eq!(project_id, "/tmp/proj-save");
    let thread_project: String = conn
        .query_row(
            "SELECT project_id FROM threads WHERE id = ?1",
            rusqlite::params![saved.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(thread_project, "/tmp/proj-save");

    // Loading it back yields the same conversation, then a second save
    // replaces (never duplicates) the rows.
    let reloaded = Freebuff::to_common(&store.load(&saved.id).unwrap()).unwrap();
    assert_eq!(reloaded.body.len(), common.body.len());
    let saved_again = store
        .save(&Freebuff::from_common(&common).unwrap())
        .unwrap();
    assert_eq!(saved_again.id, saved.id);
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM messages WHERE thread_id = ?1",
            rusqlite::params![saved.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2, "save must replace, not duplicate, messages");
}

#[test]
#[cfg(feature = "opencode")]
fn store_save_adopts_existing_identity_and_matches_by_path() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_store(dir.path());
    let common = sample_common("", "/tmp/proj-adopt");
    let native = Freebuff::from_common(&common).unwrap();
    let saved = store.save(&native).unwrap();

    // The identity file matches the directory suffix and the project.json.
    let entries: Vec<_> = std::fs::read_dir(&store.projects_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    let doc: Value =
        serde_json::from_str(&std::fs::read_to_string(entries[0].join("project.json")).unwrap())
            .unwrap();
    let identity = doc["projectId"].as_str().unwrap().to_string();
    assert!(
        entries[0]
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with(&identity)
    );
    // Isolated stores mint throwaway identities and never write outside
    // the projects root: no .freebuff marker was created.
    assert!(!std::path::Path::new("/tmp/proj-adopt/.freebuff").exists());

    // A fresh store (simulating a later run) finds the project by path and
    // saves into the same directory.
    let store2 = make_store(dir.path());
    let common2 = sample_common("new-thread", "/tmp/proj-adopt");
    let saved2 = store2
        .save(&Freebuff::from_common(&common2).unwrap())
        .unwrap();
    assert_ne!(saved2.id, saved.id);
    let conn = rusqlite::Connection::open(entries[0].join("desktop-v2.db")).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM threads", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
#[cfg(feature = "opencode")]
fn store_save_updates_existing_thread_and_fingerprints_track_updates() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_store(dir.path());
    let common = sample_common("fp-thread", "/tmp/proj-fp");
    let saved = store
        .save(&Freebuff::from_common(&common).unwrap())
        .unwrap();

    let before = store.fingerprints(&[saved.id.clone()]).unwrap();
    assert!(!before.get(&saved.id).map(String::is_empty).unwrap_or(true));

    // Extend the conversation and save again: the fingerprint moves.
    let mut extended = common.clone();
    extended.body.push(Message {
        role: Role::User,
        content: vec![Block::Text {
            text: "and one more".to_string(),
        }],
        timestamp: DateTime::parse_from_rfc3339("2026-02-03T04:06:06Z")
            .unwrap()
            .with_timezone(&Utc),
        model: None,
        stop_reason: None,
        usage: None,
    });
    store
        .save(&Freebuff::from_common(&extended).unwrap())
        .unwrap();
    let after = store.fingerprints(&[saved.id.clone()]).unwrap();
    assert_ne!(
        before.get(&saved.id),
        after.get(&saved.id),
        "updated_at must advance on re-save"
    );

    let reloaded = Freebuff::to_common(&store.load(&saved.id).unwrap()).unwrap();
    assert_eq!(reloaded.body.len(), 3);
}

#[test]
#[cfg(feature = "opencode")]
fn store_delete_removes_thread_and_messages() {
    let dir = tempfile::tempdir().unwrap();
    let store = make_store(dir.path());
    let thread_id = "22222222-3333-4333-8444-555555555555";
    seed_project(&store, "/tmp/proj-del", thread_id);

    store.delete(&thread_id.to_string()).unwrap();
    assert!(store.discover().unwrap().is_empty());
    assert!(store.delete(&thread_id.to_string()).is_err());
}
