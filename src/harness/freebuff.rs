//! Freebuff desktop threads: rows in the app's per-project SQLite database
//! (`desktop-v2.db`).
//!
//! Freebuff is the desktop client whose threads live **per project** under the
//! app's state directory (`~/.config/freebuff-desktop/projects/<name>-<id>/`,
//! or `dirname($FREEBUFF_DESKTOP_STATE_PATH)/projects/`). Each project holds
//! one `desktop-v2.db` with a `projects`/`threads`/`messages` schema plus an
//! FTS5 search mirror; a thread is one conversation, and its `messages` rows
//! carry a role and a `parts_json` array of typed parts (`text`, `reasoning`,
//! `tool`, `ad`, `changes`, `compaction`). Everything here is
//! reverse-engineered from the app's bundled orchestrator (schema strings,
//! table names) and observed `desktop-v2.db` files, 2026-10.
//!
//! Timestamps are integer **milliseconds** since the epoch (`created_at`,
//! `updated_at`, `ts` — verified against real rows). Thread and project ids
//! are UUIDs; the identity that ties a project root to its state directory
//! lives in `<root>/.freebuff/project-id` (a bare UUID and newline).
//!
//! Tolerance: an unreadable or foreign project database is skipped by
//! discovery, not fatal; a `parts_json` that fails to parse degrades to no
//! parts while the raw text stays in the body; unknown part kinds and
//! unknown thread/message columns ride in the body untouched.
//!
//! Known losses through [`Common`]: the `ad` and `changes` part kinds (not
//! conversation), `reasoning` part ids, `tool` `exitCode` (canonical results
//! carry only `is_error`), inline images and artifacts, and the compaction
//! receipt's metadata (only its summary crosses, as a plain text block). A
//! same-format round trip through [`Common`] also drops `ad`/`changes` parts
//! — unmodeled records survive only in the body itself.

use std::collections::VecDeque;

#[cfg(feature = "opencode")]
use std::collections::HashMap;

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

#[cfg(feature = "opencode")]
use uuid::Uuid;

use crate::common::{Block, Message, Meta, Role, Tool, ToolOutput};
use crate::error::{Error, Result};
use crate::transcript::{Codec, Common, Discovered, Harness, Saved, Store, TextCodec, Transcript};

#[cfg(feature = "opencode")]
use rusqlite::types::Value as SqlValue;
#[cfg(feature = "opencode")]
use rusqlite::types::ValueRef;
#[cfg(feature = "opencode")]
use rusqlite::{Connection, OpenFlags, params};

/// The Freebuff harness marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Freebuff;

impl Harness for Freebuff {
    const NAME: &'static str = "freebuff";
    type Body = Session;
}

/// Faithful representation of one Freebuff thread: its `threads` row and
/// every `messages` row for it, in `seq` order. JSON-in-TEXT columns stay
/// the raw strings the app wrote, byte-lossless; unknown columns ride in
/// `extra` maps so additive schema changes round-trip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// The `threads` row for this conversation.
    pub thread: ThreadRow,
    /// The thread's `messages` rows, ordered by `seq`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<MessageRow>,
}

/// One `threads` row: the columns the schema defines typed, and anything
/// newer under `extra`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadRow {
    pub id: String,
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub project_path: String,
    #[serde(default)]
    pub title: String,
    #[serde(default = "default_thread_status")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default = "default_agent_mode")]
    pub agent_mode: String,
    #[serde(default = "default_execution_mode")]
    pub execution_mode: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub extra: Map<String, Value>,
}

fn default_thread_status() -> String {
    "open".to_string()
}

fn default_agent_mode() -> String {
    "build".to_string()
}

fn default_execution_mode() -> String {
    "local".to_string()
}

/// One `messages` row. `parts_json`/`attachments_json`/`metrics_json` are
/// kept as the verbatim TEXT cells; the codec parses them on demand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageRow {
    pub seq: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub steering: bool,
    pub role: String,
    pub parts_json: String,
    #[serde(default = "default_empty_array")]
    pub attachments_json: String,
    #[serde(default = "default_empty_object")]
    pub metrics_json: String,
    pub ts: i64,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub extra: Map<String, Value>,
}

fn default_empty_array() -> String {
    "[]".to_string()
}

fn default_empty_object() -> String {
    "{}".to_string()
}

impl MessageRow {
    /// The row's parts, or an empty list when `parts_json` doesn't parse —
    /// a malformed cell degrades to no parts, never an error.
    #[must_use]
    pub fn parts(&self) -> Vec<Value> {
        serde_json::from_str(&self.parts_json).unwrap_or_default()
    }
}

/// Milliseconds since the epoch, the app's column convention, as a UTC time.
/// Out-of-range values land on the epoch rather than failing the parse.
#[must_use]
pub fn millis_to_datetime(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms)
        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap_or_default())
}

/// A UTC time as the app's integer milliseconds.
#[must_use]
pub fn datetime_to_millis(ts: DateTime<Utc>) -> i64 {
    ts.timestamp_millis()
}

// ── codec ──────────────────────────────────────────────────────────────

impl Codec for Freebuff {
    fn to_common(transcript: &Transcript<Self>) -> Result<Transcript<Common>> {
        let meta = meta_from_thread(&transcript.body.thread);
        let mut messages = Vec::new();
        for row in &transcript.body.messages {
            let Some(role) = role_of(&row.role) else {
                continue;
            };
            let mut blocks = Vec::new();
            for part in row.parts() {
                let Some(kind) = part.get("kind").and_then(Value::as_str) else {
                    continue;
                };
                match kind {
                    "text" => {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            blocks.push(Block::Text {
                                text: text.to_string(),
                            });
                        }
                    }
                    "reasoning" => {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            blocks.push(Block::Thinking {
                                text: text.to_string(),
                                signature: str_field(&part, "signature"),
                                encrypted: str_field(&part, "encrypted"),
                            });
                        }
                    }
                    "tool" => {
                        let id = str_field(&part, "id")
                            .unwrap_or_else(|| synth_tool_id(&transcript.body.thread.id, row.seq));
                        let name = str_field(&part, "toolName").unwrap_or_default();
                        let output = part.get("output");
                        if name.is_empty() {
                            // A bare result part (from_common writes one when
                            // a canonical result rides a different message
                            // than its call): output only, paired by id. The
                            // app itself never writes this shape.
                            if let Some(output) = output {
                                let content = match output {
                                    Value::String(text) => ToolOutput::Text(text.clone()),
                                    other => ToolOutput::Json(other.clone()),
                                };
                                blocks.push(Block::ToolResult {
                                    tool_use_id: id,
                                    content,
                                    is_error: tool_is_error(&part),
                                });
                            }
                            continue;
                        }
                        let input = part.get("input").cloned().unwrap_or(Value::Null);
                        blocks.push(Block::ToolUse {
                            id: id.clone(),
                            tool: normalize_tool(&name, input),
                        });
                        // The result rides the same part; only its presence
                        // makes it a result (a pending call has none).
                        if let Some(output) = output {
                            let content = match output {
                                Value::String(text) => ToolOutput::Text(text.clone()),
                                other => ToolOutput::Json(other.clone()),
                            };
                            blocks.push(Block::ToolResult {
                                tool_use_id: id,
                                content,
                                is_error: tool_is_error(&part),
                            });
                        }
                    }
                    // A compaction receipt's summary is conversation context
                    // the model received; only the summary crosses (the
                    // trigger/threshold bookkeeping does not).
                    "compaction" => {
                        if let Some(summary) = part
                            .get("receipt")
                            .and_then(|r| r.get("summary"))
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                        {
                            blocks.push(Block::Text {
                                text: summary.to_string(),
                            });
                        }
                    }
                    // `ad` and `changes` are display-only; unknown kinds may
                    // be future conversation. Both stay in the body only.
                    _ => {}
                }
            }
            if blocks.is_empty() {
                continue;
            }
            messages.push(Message {
                role,
                content: blocks,
                timestamp: millis_to_datetime(row.ts),
                model: None,
                stop_reason: None,
                usage: None,
            });
        }
        Ok(Transcript::new(meta, messages))
    }

    fn from_common(transcript: &Transcript<Common>) -> Result<Transcript<Self>> {
        let thread = thread_from_meta(&transcript.meta, transcript.body.last());
        let messages = messages_from_common(&transcript.body);
        Ok(Transcript::new(
            transcript.meta.clone(),
            Session { thread, messages },
        ))
    }
}

fn role_of(role: &str) -> Option<Role> {
    match role {
        "user" => Some(Role::User),
        "assistant" => Some(Role::Assistant),
        _ => None,
    }
}

fn str_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// `failure` status or a non-zero exit code marks the result an error.
fn tool_is_error(part: &Value) -> bool {
    let failed = part
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|s| s.eq_ignore_ascii_case("failure") || s.eq_ignore_ascii_case("error"));
    let nonzero = part
        .get("exitCode")
        .and_then(Value::as_i64)
        .is_some_and(|code| code != 0);
    failed || nonzero
}

/// A stable id for a tool part the app wrote without one.
fn synth_tool_id(thread_id: &str, seq: i64) -> String {
    format!("freebuff-{thread_id}-{seq}")
}

/// Session metadata from the `threads` row alone — everything discovery
/// needs without touching `messages`.
#[must_use]
pub fn meta_from_thread(thread: &ThreadRow) -> Meta {
    let nonempty = |s: &String| (!s.is_empty()).then(|| s.clone());
    Meta {
        id: thread.id.clone(),
        timestamp: millis_to_datetime(thread.created_at),
        cwd: nonempty(&thread.project_path),
        git_branch: thread.branch.clone().filter(|b| !b.is_empty()),
        title: nonempty(&thread.title),
        cli_version: None,
        model: thread.model.clone().filter(|m| !m.is_empty()),
        lineage: None,
    }
}

/// The `threads` row for a canonical transcript. `project_id` is a
/// placeholder — the store replaces it with the owning project's identity
/// on save; the text form keeps it so a rendered document is self-contained.
fn thread_from_meta(meta: &Meta, last: Option<&Message>) -> ThreadRow {
    let created_at = datetime_to_millis(meta.timestamp);
    let updated_at = last
        .map_or(created_at, |m| datetime_to_millis(m.timestamp))
        .max(created_at);
    ThreadRow {
        id: meta.id.clone(),
        project_id: String::new(),
        project_path: meta.cwd.clone().unwrap_or_default(),
        title: meta
            .title
            .clone()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "New thread".to_string()),
        status: default_thread_status(),
        // The app's own harness tag ("codebuff") is not contextleleo's to
        // invent; NULL is valid and the app fills it in on resume.
        harness_id: None,
        model: meta.model.clone(),
        branch: meta.git_branch.clone(),
        agent_mode: default_agent_mode(),
        execution_mode: default_execution_mode(),
        created_at,
        updated_at,
        extra: Map::new(),
    }
}

/// Canonical messages back into `messages` rows: one row per message, with
/// each [`Block::ToolUse`] becoming a `tool` part that absorbs its paired
/// [`Block::ToolResult`] (the app keeps call and result in one part).
/// Results whose call was dropped (images, artifacts, foreign messages)
/// arrive as synthetic `unknown` tool parts so their output survives.
#[allow(clippy::too_many_lines)]
fn messages_from_common(body: &[Message]) -> Vec<MessageRow> {
    #[derive(Clone, Copy)]
    struct Call {
        row: usize,
        part: usize,
    }
    // One slot per canonical message; empty slots drop out at the end, so
    // row indexes always line up with `body` indexes during the walk.
    let mut rows: Vec<(Role, Vec<Value>, DateTime<Utc>)> = body
        .iter()
        .map(|m| (m.role, Vec::new(), m.timestamp))
        .collect();
    // Unpaired tool calls in emission order, for FIFO result pairing.
    let mut pending: VecDeque<(String, Call)> = VecDeque::new();
    for (i, message) in body.iter().enumerate() {
        for block in &message.content {
            match block {
                Block::Text { text } => {
                    rows[i].1.push(json!({"kind": "text", "text": text}));
                }
                Block::Thinking { text, .. } => {
                    // The app renders reasoning parts keyed by id; a stable
                    // synthetic one keeps the part shape familiar.
                    let id = format!("p{}-{}", i + 1, rows[i].1.len() + 1);
                    rows[i].1.push(json!({
                        "kind": "reasoning",
                        "id": id,
                        "text": text,
                        "open": false,
                        "collapse": "preview",
                    }));
                }
                Block::ToolUse { id, tool } => {
                    let (name, input) = denormalize_tool(tool);
                    rows[i].1.push(json!({
                        "kind": "tool",
                        "id": id,
                        "toolName": name,
                        "input": input,
                    }));
                    pending.push_back((
                        id.clone(),
                        Call {
                            row: i,
                            part: rows[i].1.len() - 1,
                        },
                    ));
                }
                Block::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    let (output, status) = tool_result_fields(content, *is_error);
                    // An explicitly paired call absorbs its own result; a
                    // result with no matching call takes the oldest pending
                    // one (the Anthropic ordering convention); a result with
                    // no call at all rides as a synthetic `unknown` part.
                    let absorbed = pending
                        .iter()
                        .position(|(id, _)| id == tool_use_id)
                        .and_then(|pos| pending.remove(pos))
                        .or_else(|| pending.pop_front());
                    match absorbed {
                        // Same message: the app's own shape — the result
                        // rides the call's part.
                        Some((id, call)) if call.row == i => {
                            if let Some(obj) = rows[call.row].1[call.part].as_object_mut() {
                                obj.insert("output".into(), output);
                                obj.insert("status".into(), status.into());
                                if id != *tool_use_id {
                                    // FIFO fallback paired a different call;
                                    // the call's id wins so the pair stays
                                    // addressable, as in the Simple codec.
                                    obj.insert("id".into(), Value::String(id));
                                }
                            }
                        }
                        // A different message: the app's rows are role-typed
                        // (a user-carried result cannot move into the call's
                        // assistant row), so the result keeps its own row as
                        // a bare result part — no toolName, output only —
                        // and to_common pairs it back by id.
                        Some((id, _call)) => {
                            rows[i].1.push(json!({
                                "kind": "tool",
                                "id": id,
                                "output": output,
                                "status": status,
                            }));
                        }
                        None => {
                            rows[i].1.push(json!({
                                "kind": "tool",
                                "id": tool_use_id,
                                "toolName": "unknown",
                                "input": {},
                                "output": output,
                                "status": status,
                            }));
                        }
                    }
                }
                // Images and artifacts have no Freebuff part shape; they are
                // dropped from the rows (documented loss).
                Block::Image { .. } | Block::Artifact { .. } => {}
            }
        }
    }
    rows.into_iter()
        .filter(|(_, parts, _)| !parts.is_empty())
        .enumerate()
        .map(|(seq, (role, parts, ts))| MessageRow {
            seq: i64::try_from(seq + 1).unwrap_or(i64::MAX),
            request_id: None,
            input_id: None,
            origin: None,
            steering: false,
            role: role_to_string(role).to_string(),
            parts_json: serde_json::to_string(&parts).unwrap_or_else(|_| "[]".to_string()),
            attachments_json: default_empty_array(),
            metrics_json: default_empty_object(),
            ts: datetime_to_millis(ts),
            extra: Map::new(),
        })
        .collect()
}

fn role_to_string(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

/// A canonical tool output and error flag as the app's `output`/`status`
/// fields: text stays text, structured output is serialized into the
/// string cell the schema stores.
fn tool_result_fields(content: &ToolOutput, is_error: bool) -> (Value, &'static str) {
    let output = match content {
        ToolOutput::Text(text) => Value::String(text.clone()),
        ToolOutput::Json(value) => Value::String(serde_json::to_string(value).unwrap_or_default()),
    };
    (output, if is_error { "failure" } else { "success" })
}

// ── tool normalization (freebuff ↔ canonical) ─────────────────────────

/// Freebuff's codebuff-style tool names onto the Claude convention. Only
/// obvious shell/file pairs are typed; everything else (`list_directory`,
/// `glob`, MCP tools, …) passes through as [`Tool::Raw`] under its native
/// name — always lossless.
fn normalize_tool(name: &str, input: Value) -> Tool {
    match name {
        "bash" | "run_command" | "run_terminal_command" => Tool::from_canonical("Bash", input),
        "view_file" | "read_file" => Tool::from_canonical("Read", input),
        "write_to_file" | "write_file" => Tool::from_canonical("Write", input),
        "replace_file_content" | "edit_file" => Tool::from_canonical("Edit", input),
        other => Tool::from_canonical(other, input),
    }
}

/// Inverse of [`normalize_tool`]: the native tool name and input to store.
fn denormalize_tool(tool: &Tool) -> (String, Value) {
    let (name, input) = tool.to_canonical();
    match name.as_str() {
        "Bash" => ("bash".into(), input),
        "Read" => ("view_file".into(), input),
        "Write" => ("write_to_file".into(), input),
        "Edit" => ("replace_file_content".into(), input),
        _ => (name, input),
    }
}

// ── text codec ─────────────────────────────────────────────────────────

/// A Freebuff session's text form is the [`Session`] document itself — the
/// thread row plus its message rows, JSON-serialized. SQLite stays the
/// resume carrier; this projection is what the WASM text API and file-based
/// exchange use.
impl TextCodec for Freebuff {
    fn from_text(text: &str) -> Result<Transcript<Self>> {
        let body: Session = serde_json::from_str(text)?;
        if body.thread.id.is_empty() {
            return Err(Error::Malformed {
                harness: Freebuff::NAME,
                detail: "thread row has no id".to_string(),
            });
        }
        let meta = meta_from_thread(&body.thread);
        Ok(Transcript::new(meta, body))
    }

    fn to_text(transcript: &Transcript<Self>) -> Result<String> {
        Ok(serde_json::to_string_pretty(&transcript.body)?)
    }
}

// ── store ──────────────────────────────────────────────────────────────

/// The columns this codec types on a `threads` row; anything else the
/// database carries rides in [`ThreadRow::extra`].
#[cfg(feature = "opencode")]
const THREAD_COLUMNS: &[&str] = &[
    "id",
    "project_id",
    "project_path",
    "title",
    "status",
    "harness_id",
    "model",
    "branch",
    "agent_mode",
    "execution_mode",
    "created_at",
    "updated_at",
];

/// The columns this codec types on a `messages` row; anything else rides in
/// [`MessageRow::extra`].
#[cfg(feature = "opencode")]
const MESSAGE_COLUMNS: &[&str] = &[
    "seq",
    "request_id",
    "input_id",
    "origin",
    "steering",
    "role",
    "parts_json",
    "attachments_json",
    "metrics_json",
    "ts",
];

/// Reads and writes Freebuff threads in the app's per-project databases
/// (default `~/.config/freebuff-desktop/projects/`, one `desktop-v2.db`
/// per project directory).
///
/// The store scans every project database for discovery and loads by thread
/// id, which is a UUID unique across projects. Saves target the project
/// database whose `project.json` names the transcript's working directory,
/// creating the project entry the way the app does (including its
/// `<root>/.freebuff/project-id` identity file) when needed.
#[derive(Debug, Clone)]
pub struct FreebuffStore {
    /// The directory of per-project databases (the app's `projects/`).
    pub projects_dir: std::path::PathBuf,
    /// Set for explicit root overrides (`--out`, tests): the directory is
    /// treated as a detached projects root and the project's real
    /// `.freebuff` state is never touched.
    pub isolated: bool,
}

impl FreebuffStore {
    pub fn new(projects_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            projects_dir: projects_dir.into(),
            isolated: false,
        }
    }

    /// The default projects directory, honoring `FREEBUFF_PROJECTS_DIR` and
    /// the app's own `FREEBUFF_DESKTOP_STATE_PATH` override (which names the
    /// `state.json` file; projects live beside it).
    #[must_use]
    pub fn default_root() -> Option<Self> {
        if let Ok(dir) = std::env::var("FREEBUFF_PROJECTS_DIR")
            && !dir.is_empty()
        {
            return Some(Self {
                projects_dir: std::path::PathBuf::from(dir),
                isolated: true,
            });
        }
        let state_dir = std::env::var("FREEBUFF_DESKTOP_STATE_PATH")
            .ok()
            .filter(|p| !p.trim().is_empty())
            .map(std::path::PathBuf::from)
            .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
            .or_else(|| {
                super::home_dir().map(|home| home.join(".config").join("freebuff-desktop"))
            });
        state_dir.map(|dir| Self {
            projects_dir: dir.join("projects"),
            isolated: false,
        })
    }

    /// Every project database on disk, sorted; unreadable directories list
    /// nothing.
    #[cfg(feature = "opencode")]
    fn databases(&self) -> Vec<std::path::PathBuf> {
        let Ok(entries) = std::fs::read_dir(&self.projects_dir) else {
            return Vec::new();
        };
        let mut dbs: Vec<std::path::PathBuf> = entries
            .flatten()
            .map(|entry| entry.path().join("desktop-v2.db"))
            .filter(|path| path.is_file())
            .collect();
        dbs.sort();
        dbs
    }

    /// The project database directory whose `project.json` names `cwd`.
    #[cfg(feature = "opencode")]
    fn project_dir_for(&self, cwd: &str) -> Option<std::path::PathBuf> {
        let entries = std::fs::read_dir(&self.projects_dir).ok()?;
        for entry in entries.flatten() {
            let dir = entry.path();
            if let Some(path) = project_json_path(&dir)
                && let Some(project_path) = project_json_cwd(&path)
                && same_project_path(&project_path, cwd)
            {
                return Some(dir);
            }
        }
        None
    }

    /// Resolve (or, when not [`FreebuffStore::isolated`], create) the project
    /// directory a save into `cwd` lands in, mirroring the app's own layout:
    /// `<readable-name>-<identity>/` with a `project.json` beside the
    /// database.
    #[cfg(feature = "opencode")]
    fn resolve_project_dir(&self, cwd: &str) -> Result<std::path::PathBuf> {
        if let Some(dir) = self.project_dir_for(cwd) {
            return Ok(dir);
        }
        let identity = self.project_identity(cwd)?;
        // A directory for this identity may exist under a different project
        // path (the project moved); it is still this project's directory.
        if let Some(dir) = self.identity_dir(&identity) {
            rewrite_project_json(&dir, &identity, cwd)?;
            return Ok(dir);
        }
        let dir = self
            .projects_dir
            .join(format!("{}-{identity}", readable_project_name(cwd)));
        std::fs::create_dir_all(&dir)?;
        rewrite_project_json(&dir, &identity, cwd)?;
        Ok(dir)
    }

    /// The project's identity UUID: read from the root's
    /// `.freebuff/project-id` when the app wrote one, minting and writing it
    /// otherwise — the same file the app consults, so a database created
    /// here is adopted on the app's next open. Isolated stores never write
    /// into the project root and mint a throwaway identity instead.
    #[cfg(feature = "opencode")]
    fn project_identity(&self, cwd: &str) -> Result<String> {
        let root = std::path::Path::new(cwd);
        if self.isolated || !root.is_dir() {
            return Ok(Uuid::new_v4().to_string());
        }
        let marker = root.join(".freebuff").join("project-id");
        if let Ok(text) = std::fs::read_to_string(&marker) {
            let id = text.trim();
            if Uuid::parse_str(id).is_ok() {
                return Ok(id.to_string());
            }
        }
        let identity = Uuid::new_v4().to_string();
        std::fs::create_dir_all(root.join(".freebuff"))?;
        std::fs::write(&marker, format!("{identity}\n"))?;
        Ok(identity)
    }

    /// The directory whose name carries `identity` as its `-<uuid>` suffix.
    #[cfg(feature = "opencode")]
    fn identity_dir(&self, identity: &str) -> Option<std::path::PathBuf> {
        let suffix = format!("-{identity}");
        std::fs::read_dir(&self.projects_dir)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .find(|dir| {
                dir.is_dir()
                    && dir
                        .file_name()
                        .and_then(std::ffi::OsStr::to_str)
                        .is_some_and(|name| name.ends_with(&suffix))
            })
    }
}

/// The project directory's metadata document, as the app writes it.
#[cfg(feature = "opencode")]
fn project_json_path(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let path = dir.join("project.json");
    path.is_file().then_some(path)
}

#[cfg(feature = "opencode")]
fn project_json_cwd(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    str_field(&value, "projectPath")
}

#[cfg(feature = "opencode")]
fn rewrite_project_json(dir: &std::path::Path, identity: &str, cwd: &str) -> Result<()> {
    let doc = json!({
        "version": 1,
        "projectId": identity,
        "projectPath": cwd,
        "database": "desktop-v2.db",
    });
    std::fs::write(
        dir.join("project.json"),
        serde_json::to_string_pretty(&doc)?,
    )?;
    Ok(())
}

/// Whether two project paths name the same directory: byte-equal after
/// trimming a trailing separator, or both resolvable to the same path.
#[cfg(feature = "opencode")]
fn same_project_path(a: &str, b: &str) -> bool {
    if a.trim_end_matches('/') == b.trim_end_matches('/') {
        return true;
    }
    match (
        std::path::Path::new(a).canonicalize(),
        std::path::Path::new(b).canonicalize(),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// A filesystem-safe project directory label from a root path: the
/// lowercased basename with foreign characters collapsed to `-`. Cosmetic
/// only — matching goes through `project.json`, never the name.
#[cfg(feature = "opencode")]
fn readable_project_name(cwd: &str) -> String {
    let base = std::path::Path::new(cwd)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("project");
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('-');
    if cleaned.is_empty() {
        "project".to_string()
    } else {
        cleaned.to_ascii_lowercase()
    }
}

// ── store: SQLite backend ─────────────────────────────────────────────

/// The project database file name, as the app names it.
#[cfg(feature = "opencode")]
const DB_FILENAME: &str = "desktop-v2.db";

#[cfg(feature = "opencode")]
impl Store for FreebuffStore {
    type H = Freebuff;
    type Ref = String;

    /// One pass per project database, reading `threads` rows only — no
    /// message parsing. Unreadable or foreign databases are skipped, not
    /// fatal, so a half-written or locked store still lists the rest.
    fn discover(&self) -> Result<Vec<Discovered<String>>> {
        let mut out = Vec::new();
        for db in self.databases() {
            let Ok(conn) = open_ro(&db) else {
                continue;
            };
            if !has_table(&conn, "threads").unwrap_or(false) {
                continue;
            }
            let Ok(mut stmt) = conn.prepare("SELECT * FROM threads") else {
                continue;
            };
            let rows = stmt.query_map([], row_to_map).map_err(sqlite_err)?;
            for map in rows.flatten() {
                let thread = thread_from_map(&map);
                if thread.id.is_empty() {
                    continue;
                }
                out.push(Discovered {
                    meta: meta_from_thread(&thread),
                    reference: thread.id,
                });
            }
        }
        Ok(out)
    }

    /// Thread ids are UUIDs, unique across every project database, so the
    /// reference is the id alone and the store searches each database for
    /// it. The first hit wins.
    fn load(&self, reference: &String) -> Result<Transcript<Freebuff>> {
        super::checked_id_component(Freebuff::NAME, reference)?;
        for db in self.databases() {
            let Ok(conn) = open_ro(&db) else {
                continue;
            };
            if !has_table(&conn, "threads").unwrap_or(false) {
                continue;
            }
            let Ok(mut stmt) = conn.prepare("SELECT * FROM threads WHERE id = ?1") else {
                continue;
            };
            let Ok(map) = stmt.query_row(params![reference], row_to_map) else {
                continue;
            };
            let thread = thread_from_map(&map);
            let messages = read_messages(&conn, reference)?;
            return Ok(Transcript::new(
                meta_from_thread(&thread),
                Session { thread, messages },
            ));
        }
        Err(Error::Malformed {
            harness: Freebuff::NAME,
            detail: format!("no such thread: {reference}"),
        })
    }

    /// Saves into the project database the transcript's working directory
    /// maps to, creating the project directory, its `project.json`, and a
    /// schema-compatible database when the app has none yet — so a thread
    /// written here appears in the app on its next open. The thread row is
    /// replaced wholesale and its messages rewritten in one transaction.
    fn save(&self, transcript: &Transcript<Freebuff>) -> Result<Saved<String>> {
        let mut session = transcript.body.clone();
        let id = [session.thread.id.clone(), transcript.meta.id.clone()]
            .into_iter()
            .find(|candidate| !candidate.is_empty())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        super::checked_id_component(Freebuff::NAME, &id)?;
        session.thread.id.clone_from(&id);

        // The project root the transcript belongs to: the metadata's cwd,
        // else the thread row's own project path. The app keys `projects`
        // by the path (observed rows) and `threads.project_id` references
        // it, so both columns carry the path, not the directory-label UUID.
        let cwd = transcript
            .meta
            .cwd
            .clone()
            .filter(|c| !c.is_empty())
            .or_else(|| Some(session.thread.project_path.clone()).filter(|p| !p.is_empty()))
            .unwrap_or_default();
        session.thread.project_path.clone_from(&cwd);
        session.thread.project_id.clone_from(&cwd);

        let db_path = self.resolve_project_dir(&cwd)?.join(DB_FILENAME);
        let conn = Connection::open(&db_path).map_err(sqlite_err)?;
        conn.execute_batch(CORE_SCHEMA).map_err(sqlite_err)?;

        // The projects row must exist before the thread that references it,
        // whatever the connection's foreign_keys pragma.
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, root_path, default_branch, created_at)
             VALUES (?1, ?1, 'main', ?2)",
            params![cwd, datetime_to_millis(Utc::now())],
        )
        .map_err(sqlite_err)?;

        // Deterministic replace: explicit message deletes first (REPLACE's
        // cascade depends on the pragma), then the thread row, then the
        // rewritten messages with their seq values preserved.
        conn.execute("DELETE FROM messages WHERE thread_id = ?1", params![id])
            .map_err(sqlite_err)?;
        insert_row(
            &conn,
            "threads",
            thread_values(&session.thread),
            &session.thread.extra,
            THREAD_COLUMNS,
        )?;
        for row in &session.messages {
            insert_message(&conn, &id, row)?;
        }
        Ok(Saved {
            id: id.clone(),
            reference: id,
        })
    }

    /// Removes the thread and its messages from whichever project database
    /// holds them. Errors when no database knows the id.
    fn delete(&self, reference: &String) -> Result<()> {
        super::checked_id_component(Freebuff::NAME, reference)?;
        for db in self.databases() {
            let Ok(conn) = Connection::open(&db) else {
                continue;
            };
            if !has_table(&conn, "threads").unwrap_or(false) {
                continue;
            }
            conn.execute(
                "DELETE FROM messages WHERE thread_id = ?1",
                params![reference],
            )
            .map_err(sqlite_err)?;
            let deleted = conn
                .execute("DELETE FROM threads WHERE id = ?1", params![reference])
                .map_err(sqlite_err)?;
            if deleted > 0 {
                return Ok(());
            }
        }
        Err(Error::Malformed {
            harness: Freebuff::NAME,
            detail: format!("no such thread: {reference}"),
        })
    }

    /// The thread's `updated_at` per project database — the app's own
    /// change cursor. Unreadable databases contribute empty fingerprints.
    fn fingerprints(&self, refs: &[String]) -> Result<HashMap<String, String>> {
        let mut out: HashMap<String, String> = HashMap::with_capacity(refs.len());
        for db in self.databases() {
            let Ok(conn) = open_ro(&db) else {
                continue;
            };
            if !has_table(&conn, "threads").unwrap_or(false) {
                continue;
            }
            let Ok(mut stmt) = conn.prepare("SELECT id, updated_at FROM threads") else {
                continue;
            };
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                ))
            });
            for (id, updated) in rows.into_iter().flatten().flatten() {
                if let Some(id) = id {
                    out.insert(id, updated.unwrap_or_default().to_string());
                }
            }
        }
        for r in refs {
            out.entry(r.clone()).or_default();
        }
        Ok(out)
    }
}

#[cfg(not(feature = "opencode"))]
impl Store for FreebuffStore {
    type H = Freebuff;
    type Ref = String;

    fn discover(&self) -> Result<Vec<Discovered<String>>> {
        Ok(Vec::new())
    }

    fn load(&self, _reference: &String) -> Result<Transcript<Freebuff>> {
        Err(sqlite_unavailable())
    }

    fn save(&self, _transcript: &Transcript<Freebuff>) -> Result<Saved<String>> {
        Err(sqlite_unavailable())
    }

    fn delete(&self, _reference: &String) -> Result<()> {
        Err(sqlite_unavailable())
    }
}

#[cfg(not(feature = "opencode"))]
fn sqlite_unavailable() -> Error {
    Error::Unconvertible {
        harness: Freebuff::NAME,
        detail: "Freebuff store support requires the `opencode` feature for SQLite".to_string(),
    }
}

// ── SQLite plumbing ──────────────────────────────────────────────────

#[cfg(feature = "opencode")]
#[allow(clippy::needless_pass_by_value)] // map_err(sqlite_err) call-site convention, as in cursor_desktop/antigravity
fn sqlite_err(e: rusqlite::Error) -> Error {
    Error::Malformed {
        harness: Freebuff::NAME,
        detail: format!("database error: {e}"),
    }
}

/// A read-only connection: discovery and loading never write, so a live
/// app holding the store never trips over a scanner.
#[cfg(feature = "opencode")]
fn open_ro(db_path: &std::path::Path) -> Result<Connection> {
    Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(sqlite_err)
}

#[cfg(feature = "opencode")]
fn has_table(conn: &Connection, name: &str) -> Result<bool> {
    conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![name],
        |row| row.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .map_err(sqlite_err)
}

/// Every `messages` row of one thread, in `seq` order.
#[cfg(feature = "opencode")]
fn read_messages(conn: &Connection, thread_id: &str) -> Result<Vec<MessageRow>> {
    let mut stmt = conn
        .prepare("SELECT * FROM messages WHERE thread_id = ?1 ORDER BY seq")
        .map_err(sqlite_err)?;
    let rows = stmt
        .query_map(params![thread_id], row_to_map)
        .map_err(sqlite_err)?;
    Ok(rows.flatten().map(|map| message_from_map(&map)).collect())
}

/// The core of the app's own migration, verbatim from its bundled
/// orchestrator: `projects`, `threads`, `messages` and their indexes.
/// Everything else the migration creates (FTS search, queue and receipt
/// tables, its triggers) is deliberately left out — the app runs the same
/// `IF NOT EXISTS` statements on every open, so it adds what it needs, and
/// FTS5 may not be compiled into this build's SQLite.
#[cfg(feature = "opencode")]
const CORE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS projects (
    id             TEXT PRIMARY KEY,
    root_path      TEXT NOT NULL,
    default_branch TEXT NOT NULL DEFAULT 'main',
    created_at     INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS threads (
    id                    TEXT PRIMARY KEY,
    project_id            TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    project_path          TEXT NOT NULL,
    title                 TEXT NOT NULL DEFAULT 'New thread',
    status                TEXT NOT NULL DEFAULT 'open',
    harness_id            TEXT,
    model                 TEXT,
    byok_connection       TEXT,
    reasoning_effort      TEXT,
    agent_mode            TEXT NOT NULL DEFAULT 'build',
    execution_mode        TEXT NOT NULL DEFAULT 'local',
    branch                TEXT,
    worktree_path         TEXT,
    source_branch         TEXT,
    source_ref            TEXT,
    base_ref              TEXT,
    last_seen_head        TEXT,
    turn_state            TEXT NOT NULL DEFAULT 'idle',
    queue_paused          INTEGER NOT NULL DEFAULT 0,
    auto_run              INTEGER NOT NULL DEFAULT 0,
    auto_run_scope        INTEGER NOT NULL DEFAULT 3,
    auto_run_effort       INTEGER NOT NULL DEFAULT 3,
    auto_run_prompt       TEXT,
    auto_run_started_at       INTEGER,
    auto_run_pass_count       INTEGER NOT NULL DEFAULT 0,
    auto_run_refinement_count INTEGER NOT NULL DEFAULT 0,
    auto_run_decision_count   INTEGER NOT NULL DEFAULT 0,
    pending_briefs                TEXT,
    pending_briefs_diagnostic_key TEXT,
    world_snapshot                TEXT,
    workspace_epoch       INTEGER NOT NULL DEFAULT 0,
    last_prompt_at        INTEGER,
    last_turn_finished_at INTEGER,
    last_turn_outcome     TEXT,
    attention_revision              INTEGER NOT NULL DEFAULT 0,
    attention_acknowledged_revision INTEGER NOT NULL DEFAULT 0,
    attention_reason                TEXT,
    attention_at                    INTEGER,
    auto_run_stopped_note          TEXT,
    auto_run_stopped_at            INTEGER,
    auto_run_stopped_outcome       TEXT,
    harness_state         TEXT,
    harness_state_id      TEXT,
    freebuff_instance_id  TEXT,
    freebuff_receipt_claim_id TEXT,
    sponsored             TEXT,
    sponsored_run_token   TEXT,
    sponsored_settled_at  INTEGER,
    sponsored_terminal_reports TEXT,
    sponsored_terminal_ack_at  INTEGER,
    sponsored_merge_watch      TEXT,
    sponsored_pending_accept   TEXT,
    fork_source_thread_id TEXT REFERENCES threads(id) ON DELETE SET NULL,
    fork_request_id       TEXT,
    created_at            INTEGER NOT NULL,
    updated_at            INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_threads_project ON threads(project_id, created_at);
CREATE TABLE IF NOT EXISTS messages (
    seq              INTEGER PRIMARY KEY AUTOINCREMENT,
    thread_id        TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    request_id       TEXT,
    input_id         TEXT,
    origin           TEXT,
    steering         INTEGER NOT NULL DEFAULT 0,
    role             TEXT NOT NULL,
    parts_json       TEXT NOT NULL DEFAULT '[]',
    attachments_json TEXT NOT NULL DEFAULT '[]',
    metrics_json     TEXT NOT NULL DEFAULT '{}',
    ts               INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_thread ON messages(thread_id, seq);
CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_request
    ON messages(thread_id, request_id) WHERE request_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_input
    ON messages(thread_id, input_id) WHERE input_id IS NOT NULL;
";

/// All cells of one row as JSON values keyed by column name, so both typed
/// fields and future columns come back in one pass. BLOB and non-UTF-8
/// cells are skipped (the schema's columns are TEXT/INTEGER only).
#[cfg(feature = "opencode")]
fn row_to_map(row: &rusqlite::Row) -> rusqlite::Result<Map<String, Value>> {
    let mut out = Map::new();
    for idx in 0..row.as_ref().column_count() {
        let name = row.as_ref().column_name(idx)?.to_string();
        let value = match row.get_ref(idx)? {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(i) => json!(i),
            ValueRef::Real(f) => json!(f),
            ValueRef::Text(t) => match std::str::from_utf8(t) {
                Ok(s) => json!(s),
                Err(_) => continue,
            },
            ValueRef::Blob(_) => continue,
        };
        out.insert(name, value);
    }
    Ok(out)
}

#[cfg(feature = "opencode")]
fn cell_str(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(Value::as_str).map(str::to_string)
}

#[cfg(feature = "opencode")]
fn cell_i64(map: &Map<String, Value>, key: &str) -> i64 {
    map.get(key).and_then(Value::as_i64).unwrap_or_default()
}

/// The cells this codec doesn't type, under their column names.
#[cfg(feature = "opencode")]
fn extra_of(map: &Map<String, Value>, known: &[&str]) -> Map<String, Value> {
    map.iter()
        .filter(|(key, _)| !known.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// A `threads` row from its raw cells: typed fields with the schema's
/// defaults, unknown columns under `extra`.
#[cfg(feature = "opencode")]
fn thread_from_map(map: &Map<String, Value>) -> ThreadRow {
    ThreadRow {
        id: cell_str(map, "id").unwrap_or_default(),
        project_id: cell_str(map, "project_id").unwrap_or_default(),
        project_path: cell_str(map, "project_path").unwrap_or_default(),
        title: cell_str(map, "title").unwrap_or_default(),
        status: cell_str(map, "status").unwrap_or_else(default_thread_status),
        harness_id: cell_str(map, "harness_id"),
        model: cell_str(map, "model"),
        branch: cell_str(map, "branch"),
        agent_mode: cell_str(map, "agent_mode").unwrap_or_else(default_agent_mode),
        execution_mode: cell_str(map, "execution_mode").unwrap_or_else(default_execution_mode),
        created_at: cell_i64(map, "created_at"),
        updated_at: cell_i64(map, "updated_at"),
        extra: extra_of(map, THREAD_COLUMNS),
    }
}

/// A `messages` row from its raw cells. `thread_id` is the parent key the
/// store supplies on write, never conversation data, so it never rides in
/// `extra`.
#[cfg(feature = "opencode")]
fn message_from_map(map: &Map<String, Value>) -> MessageRow {
    let mut known: Vec<&str> = MESSAGE_COLUMNS.to_vec();
    known.push("thread_id");
    MessageRow {
        seq: cell_i64(map, "seq"),
        request_id: cell_str(map, "request_id"),
        input_id: cell_str(map, "input_id"),
        origin: cell_str(map, "origin"),
        steering: cell_i64(map, "steering") != 0,
        role: cell_str(map, "role").unwrap_or_default(),
        parts_json: cell_str(map, "parts_json").unwrap_or_else(default_empty_array),
        attachments_json: cell_str(map, "attachments_json").unwrap_or_else(default_empty_array),
        metrics_json: cell_str(map, "metrics_json").unwrap_or_else(default_empty_object),
        ts: cell_i64(map, "ts"),
        extra: extra_of(map, &known),
    }
}

/// The typed `threads` cells for one row, in the schema's column set.
#[cfg(feature = "opencode")]
fn thread_values(thread: &ThreadRow) -> Vec<(String, SqlValue)> {
    vec![
        ("id".into(), SqlValue::Text(thread.id.clone())),
        (
            "project_id".into(),
            SqlValue::Text(thread.project_id.clone()),
        ),
        (
            "project_path".into(),
            SqlValue::Text(thread.project_path.clone()),
        ),
        ("title".into(), SqlValue::Text(thread.title.clone())),
        ("status".into(), SqlValue::Text(thread.status.clone())),
        ("harness_id".into(), text_opt(thread.harness_id.as_ref())),
        ("model".into(), text_opt(thread.model.as_ref())),
        ("branch".into(), text_opt(thread.branch.as_ref())),
        (
            "agent_mode".into(),
            SqlValue::Text(thread.agent_mode.clone()),
        ),
        (
            "execution_mode".into(),
            SqlValue::Text(thread.execution_mode.clone()),
        ),
        ("created_at".into(), SqlValue::Integer(thread.created_at)),
        ("updated_at".into(), SqlValue::Integer(thread.updated_at)),
    ]
}

/// One `messages` row, parented to `thread_id`.
#[cfg(feature = "opencode")]
fn insert_message(conn: &Connection, thread_id: &str, row: &MessageRow) -> Result<()> {
    let typed = vec![
        ("seq".into(), SqlValue::Integer(row.seq)),
        ("thread_id".into(), SqlValue::Text(thread_id.to_string())),
        ("request_id".into(), text_opt(row.request_id.as_ref())),
        ("input_id".into(), text_opt(row.input_id.as_ref())),
        ("origin".into(), text_opt(row.origin.as_ref())),
        (
            "steering".into(),
            SqlValue::Integer(i64::from(row.steering)),
        ),
        ("role".into(), SqlValue::Text(row.role.clone())),
        ("parts_json".into(), SqlValue::Text(row.parts_json.clone())),
        (
            "attachments_json".into(),
            SqlValue::Text(row.attachments_json.clone()),
        ),
        (
            "metrics_json".into(),
            SqlValue::Text(row.metrics_json.clone()),
        ),
        ("ts".into(), SqlValue::Integer(row.ts)),
    ];
    insert_row(conn, "messages", typed, &row.extra, MESSAGE_COLUMNS)
}

#[cfg(feature = "opencode")]
fn text_opt(value: Option<&String>) -> SqlValue {
    value.map_or(SqlValue::Null, |text| SqlValue::Text(text.clone()))
}

/// A JSON cell as a SQLite value: scalars natively, everything else as the
/// TEXT the schema stores JSON in.
#[cfg(feature = "opencode")]
fn json_to_sql(value: &Value) -> SqlValue {
    match value {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(i64::from(*b)),
        Value::Number(n) => n.as_i64().map_or_else(
            || SqlValue::Real(n.as_f64().unwrap_or_default()),
            SqlValue::Integer,
        ),
        Value::String(s) => SqlValue::Text(s.clone()),
        other => SqlValue::Text(serde_json::to_string(other).unwrap_or_default()),
    }
}

/// `INSERT OR REPLACE` with the typed cells plus any `extra` column the
/// table actually carries — additive schema columns round-trip when the
/// database has them and are dropped when it doesn't.
#[cfg(feature = "opencode")]
fn insert_row(
    conn: &Connection,
    table: &'static str,
    mut typed: Vec<(String, SqlValue)>,
    extra: &Map<String, Value>,
    known: &[&str],
) -> Result<()> {
    let columns = table_columns(conn, table)?;
    for (name, value) in extra {
        if known.contains(&name.as_str()) || !columns.contains(name) {
            continue;
        }
        typed.push((name.clone(), json_to_sql(value)));
    }
    if typed.is_empty() {
        return Ok(());
    }
    let names = typed
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let placeholders = (1..=typed.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("INSERT OR REPLACE INTO {table} ({names}) VALUES ({placeholders})");
    conn.execute(
        &sql,
        rusqlite::params_from_iter(typed.iter().map(|(_, v)| v)),
    )
    .map_err(sqlite_err)?;
    Ok(())
}

/// The column names one of this module's tables has on disk. Only called
/// with the module's own fixed table names, never user input.
#[cfg(feature = "opencode")]
fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(sqlite_err)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(sqlite_err)?;
    Ok(rows.flatten().collect())
}
