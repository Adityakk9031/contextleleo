# Usage reference

[Back to the README](../README.md)

## CLI

Discover local sessions and continue one in any harness:

```sh
contextleleo list                             # local sessions across every harness
    [--from <harness>]                    #   only this harness's sessions
    [--cwd <dir>]                         #   only sessions recorded under <dir>
    [-n <N>]                              #   at most N sessions
    [--since <when>] [--until <when>]     #   bound the session start time
contextleleo continue <id>[#range]            # continue <id>, then launch its harness
    [--with <harness>]                    #   ...continuing in <harness> instead
    [--from <harness>]                    #   scope the id lookup to one harness
    [--out <dir>]                         #   write under <dir>; implies --no-resume
    [--no-resume]                         #   write the session but don't launch
contextleleo continue <file|->[#range]        # continue a Simple document instead:
    --with <harness> [...]                #   a file, or stdin (`-`), from any agent
contextleleo crop <id>[#range]                # interactively cut messages and save a copy
    [--with <harness>]                    #   optionally convert the cropped copy
    [--from <harness>]                    #   scope the source lookup
contextleleo view <id>[#range]                # view a session; compact text when piped
    [--from <harness>]                    #   scope the id lookup to one harness
    [--no-pager]                          #   print the terminal view directly
contextleleo export <id>[#range]              # write a session as a Simple document
    [--from <harness>]                    #   scope the id lookup to one harness
    [--out <file>]                        #   write to <file> instead of stdout
```

A session id is any unambiguous prefix of the full id, or the session's exact title. `contextleleo resume` is an alias for `continue`. `--since` and `--until` take RFC 3339 timestamps or bare `YYYY-MM-DD` dates.

`continue` writes the session where the target harness keeps its sessions, then launches that harness on it, handing over the terminal:

- Same-harness: resumes the original in place.
- Cross-harness (`--with`): rewrites the session into the target's native format. What is written is always a copy; the source session is never modified or removed.
- A [Simple](formats/simple.md) document instead of an id — `contextleleo continue ./run.json --with claude_code`, or `my-agent | contextleleo continue - --with claude_code` — brings any agent's transcript in the same way; `--with` is required since a document has no harness of its own.
- The launch command is per-harness and overridable: set `TRANSCRIPT_<HARNESS>_RESUME_CMD` to a `{id}` template, e.g. `TRANSCRIPT_CODEX_RESUME_CMD="codex resume {id}"`.

`view` in a terminal opens a built-in pager: `u`, `a`, `t`, and `r` hide or show user messages, assistant messages, tool calls, and reasoning; `]` and `[` jump between messages; `/` searches what is shown. Images are drawn inline on terminals that can show them (Ghostty, kitty, WezTerm, Konsole). Set `CONTEXTLELEO_PAGER` to use an external pager instead, or pass `--no-pager` to print the view directly. Piped or redirected, `view` prints the same compact text the MCP server serves. Either way each message is numbered by a `── #N ──` rule, and `#range` selects messages by those printed ordinals, 1-based and inclusive:

- `abc#7`: message 7 only
- `abc#5-12`: messages 5 through 12
- `abc#5-`: message 5 to the end
- `abc#-10`: start through message 10

`continue` accepts the same suffix and continues just those messages as a new session. `crop` opens an interactive editor over the session, in the spirit of a video editor's timeline: every message starts out kept, and you remove the ones you don't want from anywhere in the conversation, not just the ends. Move with `j`/`k` or the arrow keys and press Space to remove the message under the cursor (or restore it). To work on a stretch at once, press `v`, move to the other end, then `x` to remove it, `r` to restore it, or `t` to keep only that stretch; `:3-10` selects a range by number and `:42` jumps to a message. `e` opens the message under the cursor in your editor (`$VISUAL`, `$EDITOR`, or `vi`) as plain text, one heading per block: change the text, trim a tool result, or empty a block to drop it, then save and quit to apply. A terminal editor runs in a pane beside or under the transcript; `E` gives it the whole terminal instead, and an editor that opens its own window is waited for. `u` undoes, `U` redoes, `?` lists every key. Removed messages collapse to their header, edited ones say so, and an overview of the whole session, one cell per message, runs down the right edge or along the bottom depending on the window's shape. Enter saves the kept messages, edits included, as a new session; `q` leaves without saving. A `#range` is optional and opens the editor with only that range kept. The copy defaults to the source harness unless `--with` selects another one, and the source is never modified. A tool call and its result are always removed or restored together, so the saved copy never splits them.

`export` writes the session as a [Simple](formats/simple.md) document, to stdout or `--out <file>`. The document is the full rendering of the canonical model — everything `continue` carries between harnesses — detached from any harness's store, so it moves between machines as a file:

```sh
contextleleo export 0dc114bf --out session.json       # on this machine
contextleleo continue ./session.json --with claude_code   # on the other one
```

The recorded working directory is kept when it exists on the importing machine and otherwise replaced by the directory `continue` runs in. `export` accepts the same `#range` suffix and `--from` scope as `view`.

### Search

```sh
contextleleo query 'relay bug'                # one-shot: ranked hits, highlighted
contextleleo query                            # interactive picker; Enter continues
    [--from <harness>]                   #   search only <harness> (default: all)
    [--with <harness>]                   #   continue the pick in <harness>
    [--cwd <dir>]                        #   only sessions recorded under <dir>
    [--git-branch <branch>]              #   only sessions on this branch (exact)
    [--model <substr>]                   #   only sessions using this model (case-insensitive substring)
    [--since <when>]                     #   only sessions started at or after <when>
    [--until <when>]                     #   only sessions started at or before <when>
```

A pattern matches literally and case-insensitively: `relay bug` finds lines containing that exact text, spaces and all.

`--since` / `--until` accept RFC3339 timestamps (`2025-06-01T00:00:00Z`) or bare dates (`2025-06-01`); bare dates mean local midnight (for `--since`) or end of local day (for `--until`).

In the picker, type to filter, arrows / ctrl-p/n to move, Enter to continue the selection in its own harness (or `--with`), Esc to cancel. Every row shows which kind of content matched: user text, assistant text, thinking, tool use, tool output, or session metadata.

Without a cache, every run re-reads every session. Pass `--cache <path>` (or set `CONTEXTLELEO_CACHE`) to keep a persistent search cache at that path, so `query` and the MCP search tool re-read only the sessions that changed since the last run. The flag is accepted by every subcommand.

### Context retrieval

```sh
contextleleo context '<query>'                # rank matching history, then print it
    [--budget <tokens>]                   #   optimize the assembled context to fit
    [--max-chunks <n>]                    #   cap chunks in the selected set (default 12)
    [--max-tokens <tokens>]               #   hard cap on retrieved tokens
    [--from <harness>]                    #   retrieve from one harness only
    [--cwd <dir>]                         #   only sessions recorded under <dir>
    [--quiet]                             #   print only the optimized context
    [--cache <path>]                      #   reuse the persistent search cache
```

Ranking itself runs through the external **Jev API** — TypeSafe AI's System One decision model — while local search only gathers candidates:

```sh
export JEV_API_KEY=...   # your TypeSafe (Jev) API key; TYPESAFE_API_KEY also works
```

Two overrides exist and carry these defaults: `JEV_API_URL` (`https://api.typesafe.ai/v1/systemone`) and `JEV_MODEL` (`jev-latest`).

Local search generates up to 64 candidate chunks (`--max-chunks` may widen that); each travels to Jev as its `session#message` locator plus a bounded excerpt — never the full history. The call sends one `noul` (yes/no) question per candidate against a shared state (the task plus every excerpt); Jev returns a calibrated probability per candidate, and those at or above 0.5 are kept — below that they are ignored. Only kept chunks are printed or optimized, and `--max-chunks` / `--max-tokens` cap that selected set. Without a key, `context` and `--retrieve` fail with an error naming exactly what to export; `list`, `view`, `query`, `export`, and plain `continue` are unaffected.

`context` searches every stored session for candidate chunks — verbatim keyword overlap, file paths, symbol overlap, error lines, tool names, and a small recency tiebreaker — then asks the Jev API which of them matter for this query, and prints the selected set. Each chunk names its `session#message` source, so every line stays traceable back to the untouched original, and `contextleleo view <id>#<n>` opens one. With `--budget`, the assembled context runs through Jev's optimizer, which keeps, compresses, or drops each chunk to fit and reports the count of each; without it, the selected chunks print as returned.

Retrieval is read-only: no stored session is modified. Two library options — `--min-relevance` and `--one-chunk-per-session` — are exposed on `RetrievalOptions` but are not CLI flags.

`continue` can lead a handoff with the same lookup:

```sh
contextleleo continue <id> --with codex --jev --retrieve 'relay timeout' --budget 800
```

`--retrieve` prepends the retrieved context to the handoff, then `--jev` optimizes the whole to `--budget`. It requires `--jev`, and retrieval is refused for a document-sourced (`./run.json`) session.

Add `--task "what the next agent will do"` to `continue --jev` and the Jev API rates every message of the session being handed off for relevance to that task (one call per 64 messages). Jev's score decides keep / compress / drop for each message you did not write: 0.7 or more is kept in full (even big tool output), below 0.3 is dropped, anything between becomes a stand-in with a `view` link to the original. `--budget` is optional and still trims further, least relevant first. Your messages and error results are never lowered, whatever Jev says, and without `--task` the default rules decide exactly as before. `--task` requires `--jev` and a key, and with `--retrieve` only the continued session's own messages are re-scored: the prepended chunks already carry the relevance retrieval ranked them by.

`./demo/run.sh` runs the whole pipeline end to end — an Antigravity CLI session retrieved from, ranked by Jev, compressed, and written into Freebuff — and `demo/` ships the captured transcript and a video script. It is hermetic (both harness roots are redirected under `demo/.state`), which is also what `CONTEXTLELEO_ANTIGRAVITY_ROOT` exists for.

### MCP server

```sh
contextleleo mcp                              # stdio transport
```

Exposes three read-only tools; their optional filters match the CLI:

- `list_sessions(from?, cwd?, limit?, offset?)`
- `search_sessions(pattern, from?, cwd?, git_branch?, model?, since?, until?)`
- `read_session(id, from?)`

<sub>\* Omitting `from` includes every harness; omitting `cwd` applies no directory filter. Sessions without a recorded working directory match only when `cwd` is omitted. `git_branch` is exact; `model` is a case-insensitive substring; `since`/`until` are RFC3339 strings or bare dates.</sub>

`list_sessions` pages with `limit` and `offset` and reports the total before paging; the live Claude Chat and ChatGPT sources are never listed. `read_session` takes the same `#range` suffix as `view` and returns the same compact text; a read too large to return whole is refused with suggested sub-ranges. `--cache` applies to the server too.


### Shell integration

```sh
eval "$(contextleleo init zsh)"                      # in ~/.zshrc; or: contextleleo init bash
```

`init` prints completions plus a ctrl+shift+r binding that opens the picker scoped to sessions recorded in the current folder. For completions alone, `completion` covers bash, elvish, fish, powershell, and zsh:

```sh
contextleleo completion zsh > ~/.zfunc/_contextleleo      # or wherever your fpath looks
source <(contextleleo completion bash)               # bash, ad hoc
contextleleo completion fish > ~/.config/fish/completions/contextleleo.fish
```

## Rust crate

```toml
[dependencies]
contextleleo = "0.14"
# Codecs only: drops the SQLite-backed stores, the live Claude Chat and
# ChatGPT sources, and search. Every codec stays available.
# contextleleo = { version = "0.14", default-features = false }
```

Default features: `opencode` (the SQLite stores: OpenCode, both Cursors, Antigravity), `hermes`, `claude_chat`, `cowork_remote`, `chatgpt`, and `search`.

Three layers, smallest to largest:

- `Codec`: `to_common` / `from_common` per harness; `convert::<A, B>` chains them through the canonical model.
- `TextCodec`: `from_text` / `to_text` to parse and render a harness's native session text, no I/O.
- `Store`: discover/load/save against a real backend (session directories, or SQLite DBs for OpenCode, Hermes, both Cursors, and Antigravity).

Convert in memory (no filesystem):

```rust
use contextleleo::harness::{claude_code, codex};
use contextleleo::{Codec, TextCodec, convert};

let claude = claude_code::ClaudeCode::from_text(jsonl_text)?;          // Transcript<ClaudeCode>
let codex = convert::<claude_code::ClaudeCode, codex::Codex>(&claude)?; // Transcript<Codex>
let codex_text = codex::Codex::to_text(&codex)?;                       // native rollout JSONL
```

Or go through disk with a `Store`:

```rust
use contextleleo::harness::{claude_code, codex};
use contextleleo::{Store, convert};

let store = claude_code::ClaudeStore::default_root().expect("home dir");
let found = store.discover()?;                       // cheap metadata scan
let claude = store.load(&found[0].reference)?;       // Transcript<ClaudeCode>

let codex = convert::<_, codex::Codex>(&claude)?;
codex::CodexStore::default_root().expect("home dir").save(&codex)?;  // resumable on disk
```

The canonical model is `Transcript<Common>`: `Meta` + `Vec<Message>`, where a `Message` holds typed `Block`s (`Text`, `Thinking`, `ToolUse`, `ToolResult`, `Image`) and a typed `Tool` enum.

Crop a canonical transcript in memory without changing the source:

```rust
use contextleleo::{Span, Transcript, Common};

let cropped: Transcript<Common> = common.crop(&Span(4..12))?;
let spliced: Transcript<Common> = common.crop_to(&[Span(0..2), Span(10..40)])?;
```

`Span` is zero-based and half-open in the Rust API. `crop` keeps one range;
`crop_to` keeps the union of several, in order, closing the cuts between
them. Both preserve metadata, copy only the selected messages, reject empty
or out-of-bounds ranges, and refuse to separate a complete tool call from its
result. `CropError` exposes the nearest valid span when expanding the
selection can preserve that pair, and `tool_pairs` lists the call/result
pairs so an editor can keep them together up front.

Slash commands the user ran at the harness (`/release patch`) are canonical too: a `Tool::Command` call on the user turn, paired with what the command printed back as its `ToolResult`.

### Search (feature `search`, on by default)

`contextleleo::search` supports fuzzy (fzf-style syntax) and substring search over transcripts. One-shot search:

```rust
use contextleleo::search::{Query, search};

let hits = search(&common, &Query::substring("relay bug"));  // or Query::fuzzy for fzf syntax
for hit in hits {
    // hit.origin: User | Assistant | Thinking | ToolUse | ToolResult | Meta
    // hit.span addresses the message; hit.highlights are char ranges into hit.line
    let messages = common.fragment(&hit.span);            // zero-copy: Option<&[Message]>
}
```

For picker-style search, build an `Index` once and query it per keystroke:

```rust
use contextleleo::search::{DocKey, Index, Query};

let mut index = Index::new();
index.insert(DocKey { harness, id }, &common);   // re-insert replaces; caller owns refresh
let matches = index.query(&Query::fuzzy("srch")); // ranked docs, best lines as hits
```

An empty pattern returns documents newest-first. Tool outputs are excluded by default; use `Origin::ALL` to include them. `Query.harnesses`, `Query.limit`, and `Query.hits_per_doc` narrow results.

### Text projection

`contextleleo::text::to_text(&common)` is the projection behind [`contextleleo view`](#cli): a one-way, token-conscious rendering of `Transcript<Common>` for use as LLM context. It keeps messages, reasoning text, and compact tool calls/results; replay-only payloads (encrypted reasoning, usage accounting, inline image bytes) are omitted. `to_text_fragment(&common, &span)` renders a `Span` of the body, keeping each message's ordinal in the full session.

## npm package

The npm package ships the codec as prebuilt WASM for Bun and Node. It converts session text in memory; discovering, reading, and writing sessions on disk is the caller's job, so the package has no `Store`.

```ts
import { convert, toCommon, fromCommon, harnesses } from "contextleleo";
import { readFileSync, writeFileSync } from "node:fs";

const input = readFileSync("rollout.jsonl", "utf8");

// native -> native (e.g. a Codex rollout into Claude Code's JSONL)
writeFileSync("session.jsonl", convert(input, "codex", "claude_code"));

// canonical view, and back
const common = JSON.parse(toCommon(input, "codex"));   // { meta, messages }
const pi = fromCommon(JSON.stringify(common), "pi");

harnesses(); // ["claude_code","claude_chat","cowork_remote","chatgpt","codex","opencode","pi","campfire","cursor","cursor_desktop","grok","grok_bot","fx","hermes","amp","antigravity","freebuff","simple","cowork"]
```

Text-in / text-out: `input` is the source harness's native session text and the result is the target's. Invalid harness names or unparseable input throw a JS `Error`.

Search ships too. A query is the JSON form of the crate's `Query`: only `pattern` is required, and `mode` is `"fuzzy"` unless set to `"substring"`:

```ts
import { searchTranscript, Searcher } from "contextleleo";

// one session, one shot: a JSON array of hits
const hits = JSON.parse(searchTranscript(input, "codex", JSON.stringify({ pattern: "relay bug", mode: "substring" })));

// picker-style: index once, query per keystroke
const index = new Searcher();
index.insert("codex", "0dc114bf", input);          // re-insert replaces
const matches = JSON.parse(index.query(JSON.stringify({ pattern: "relay bug" })));
```

| Harness | Session text |
|---|---|
| `claude_code`, `codex`, `pi`, `campfire` | session JSONL |
| `cowork_remote` | a native document with `session` detail response and ordered `events` (source-only) |
| `claude_chat` | one live conversation detail response (source-only; no account export arrays) |
| `chatgpt` | one live conversation detail response (source-only; no account export arrays) |
| `opencode` | `opencode export` JSON |
| `cursor` | JSON export of the session's `store.db` |
| `cursor_desktop` | JSON dump of the session's `state.vscdb` rows |
| `grok` | JSON bundle of the session directory's files |
| `grok_bot` | agent-transcript JSONL (`role`/`message` envelopes) |
| `fx` | JSON bundle of the session directory's files |
| `hermes` | `hermes sessions export` JSON object |
| `amp` | `amp threads export` JSON |
| `antigravity` | JSON dump of the conversation database, protobuf blobs hex-encoded |
| `freebuff` | the session document: the `threads` row plus its `messages` rows, JSON-serialized |
| `simple` | the [Simple](formats/simple.md) interchange JSON document |
| `cowork` | JSON bundle of the session record, Claude Code transcript, and audit log |

To build the wasm from source instead:

```sh
git clone https://github.com/Adityakk9031/contextleleo.git
cd contextleleo
bun run setup        # once: wasm target + wasm-bindgen-cli
bun run build        # produces ./pkg
```

## Development

```sh
cargo test --workspace --all-features               # what CI runs
cargo test -p contextleleo --no-default-features         # codecs only: no SQLite or live stores
bun run build && bun examples/convert.ts <file> <from> <to>
git config core.hooksPath .githooks                 # pre-push runs the CI checks
```

The binary lives in its own workspace crate (`cli/`, package `contextleleo-cli`); the library at the root carries none of its dependencies.
