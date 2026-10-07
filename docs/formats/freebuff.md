# Freebuff

Freebuff is a desktop coding-agent client; contextleleo reads and writes its
conversation store directly, so a thread can be pulled out of the app,
converted into any other harness, and a conversation from another harness can
be written back as a thread the app lists under its project. The app is
closed-source in the usual sense (the conversation logic lives in its bundled
orchestrator bundle, where the schema strings are visible), and the store has
no public documentation: everything below is **reverse-engineered** from the
bundled orchestrator and observed databases on disk, October 2026.

Unlike the CLI harnesses, Freebuff keeps **one SQLite database per project**
under its state directory, not one global store:

```
~/.config/freebuff-desktop/
├── state.json
└── projects/
    └── <readable-name>-<identity-uuid>/    # one directory per project root
        ├── project.json                    # {version, projectId, projectPath, database}
        └── desktop-v2.db                   # the resume carrier
              projects        # id = the project's absolute path (observed)
              threads         # one row per conversation (id = UUID)
              messages        # the conversation, one row per message
              queue_items, auto_run_decision_receipts, sponsored_runs, …
              freebuff_conversation_search   # FTS5 mirror
```

## On disk

The state root is `~/.config/freebuff-desktop` — overridden by the app's own
`FREEBUFF_DESKTOP_STATE_PATH` (which names `state.json`; the projects directory
is `dirname(statePath)/projects`). contextleleo additionally honors
`FREEBUFF_PROJECTS_DIR`, which points the store at an alternate projects root
and treats it as isolated (the project's `.freebuff` marker is never touched).
Discovery opens every `desktop-v2.db` under the projects root **read-only**;
databases that aren't Freebuff-shaped are silently skipped.

Identity is three-layered, and matching a save to the right database goes
through the layers in order:

| Layer | What it is | The store's use |
| --- | --- | --- |
| `project.json.projectPath` | The project root the app wrote | First match: same path (byte-equal or canonicalized) → that database |
| directory name suffix `-<uuid>` | The app's identity label for the project | Same identity → same project (the path may have moved; `project.json` is rewritten) |
| `<root>/.freebuff/project-id` | The bare UUID the app consults on the project root | Created (minted and written) when no match exists, so the app adopts the database on its next open |

Thread ids are UUIDs, unique across every project database, so a session
reference is the id alone and the store searches each database for it.

## Dissection of a transcript

Names below are the app's own columns and part kinds.

| Their name | What it is | Maps to |
| --- | --- | --- |
| `threads.id` | The conversation id (UUID) | `Meta.id` |
| `threads.title` | The thread's display title | `Meta.title` |
| `threads.project_path` | The project root | `Meta.cwd` |
| `threads.branch` | The branch the thread runs on | `Meta.git_branch` |
| `threads.model` | The model tag (e.g. `m-7e20df6765`) | `Meta.model` |
| `threads.created_at` / `updated_at` | Integer **milliseconds** | `Meta.timestamp` |
| `messages.seq` | Strict conversation order | message order |
| `messages.role` | `user` or `assistant` (observed) | `Role` |
| `messages.parts_json` | An array of typed parts (below) | message `Block`s |
| `messages.ts` | Integer milliseconds | `Message.timestamp` |
| `threads.*` (the other ~50 columns) | App bookkeeping: turn state, auto-run, sponsored receipts | `extra` on the row; preserved verbatim on save |

`parts_json` kinds:

| Part kind | Fields | Maps to |
| --- | --- | --- |
| `text` | `text` | `Block::Text` |
| `reasoning` | `id`, `text`, `open`, `collapse`, optional `signature`/`encrypted` | `Block::Thinking` (id and display flags are lost) |
| `tool` | `id`, `toolName`, `input`, and, once finished, `output` + `status` (`success`/`failure`) + optional `exitCode` | `Block::ToolUse` with the result riding the same part → a call/result block pair |
| `compaction` | `receipt` with `summary`, `trigger`, `thresholdTokens`, … | the summary as `Block::Text` |
| `ad`, `changes` | display-only records | not conversation; body-only |
| unknown kinds | — | not conversation; body-only |

Tool names follow the model's conventions (`bash`, `view_file`,
`write_to_file`, `replace_file_content`, plus MCP names); the obvious
shell/file pairs normalize onto the Claude names and everything else passes
through as a raw tool. A `failure` status or non-zero `exitCode` marks the
result an error.

## Round-trip behavior and losses

Through [`Common`]:

- The `ad` and `changes` part kinds, reasoning ids and display flags, and the
  compaction receipt's bookkeeping (only its summary crosses) are dropped.
- Inline images and artifacts have no Freebuff part shape and are dropped
  from the rows.
- Canonical results arriving on a different message than their call (legal in
  Claude-style transcripts) keep their own row as a bare result part —
  `toolName`-less, output only — and pair back by id, because rows are
  role-typed and a user-carried result cannot move into the call's assistant
  row. Same-message results ride the call's part, the app's own shape.
- A same-format round trip also drops `ad`/`changes` parts: unmodeled records
  survive only in the body itself.

On save the store creates the schema-compatible core tables (the app's own
`CREATE IF NOT EXISTS` migration completes whatever contextleleo's minimal
schema leaves out — its FTS mirror, queue and receipt tables) and replaces the
thread's rows wholesale, so a saved thread appears in the app once it rescans.
There is no CLI resume: the `resume` command opens the app itself
(`open -a Freebuff` on macOS), which lists the thread under its project.
