# contextleleo — project brief

*A shareable summary of what this project is, why it exists, how it works, and where it
stands. Written to be pasted into another coding agent's context.*

---

## The idea

**contextleleo is persistent, portable memory for coding agents.** Every AI coding tool
keeps its own session history in a private store — SQLite databases, JSONL rollouts,
desktop-app state. That history is trapped inside the tool it was made in. contextleleo
reads those real stores, finds the history that actually matters for a new task, and
writes it into a *different* tool's own store as a native session — no exports, no
copy-paste, no re-explaining your project.

Two stages make the "what matters" decision honest:

1. **contextleleo searches** — fast, local, deterministic retrieval over the agent's
   indexed history. Broad and cheap; proposes candidates.
2. **Jev decides** — the [Jev API (TypeSafe "System One")](https://docs.typesafe.ai) is an
   external decision model: one yes/no (`noul`) question per candidate, its probability is
   the relevance. Only what clears the threshold carries over.

The tagline says it: **"contextleleo searches. Jev decides."**

Then the **optimizer** (pre-existing, unchanged) folds oversized tool output into
auditable stand-ins and fits the handoff to a token budget. Every chunk and every stand-in
names its origin (`session-id#message`), so a compressed handoff is always traceable back
to the untouched original.

It is a **Rust library + CLI** (typical source of truth: this repo, `context.md` is the
full living project log). The folder is named `txcript-main` for legacy reasons — the
product name is **contextleleo** everywhere (crate, CLI binary, docs). Remote:
`github.com/Adityakk9031/contextleleo`.

## Why it matters

- Session history is siloed per tool; switching agents means losing context.
- Pasting transcripts burns tokens and destroys provenance.
- Local retrieval alone can't tell what's relevant to *this* task; running an LLM over the
  entire history is slow and expensive. Jev ranks only bounded candidate excerpts.
- The handoff lands *inside* the receiving tool — it looks native because it is native
  (a real row in a real store that the app itself reads).

## The pipeline

```
   Agent A's real store          contextleleo                        Agent B's real store
   ────────────────────          ──────────────                      ────────────────────
   sessions (SQLite/JSONL)  →  local candidate retrieval  →  Jev API ranks each candidate
                               (deterministic, no vectors)      (relevance ≥ 0.5 ⇒ keep)
                                                             →  optimizer: keep / compress
                                                                / drop to --budget
                                                             →  write a native session into
                                                                Agent B's store, provenance
                                                                on every chunk
```

- `RETRIEVE_THRESHOLD = 0.5`, candidates capped (`JEV_CANDIDATE_CHUNKS = 64`).
- `--retrieve` **excludes the session being continued** (`RetrievalOptions.exclude_sessions`):
  a session is not its own memory, so the handoff draws on the *other* sessions' history.
- Candidate excerpts are **credential-redacted and fenced as quoted data** before they leave
  the machine (the local original keeps the real value); retrieved history is assembled as
  assistant-role "quoted history", never as the recipient user's own words.
- **Fail-hard without a key:** Jev paths exit 1 with a clear error; there is no silent
  local fallback. Non-Jev commands (`list`, `view`, `query`, `export`, plain `continue`)
  work without a key.
- **No key, no network:** the retrieval stage itself never opens a socket; only the opt-in
  Jev ranking step (`context`, `--retrieve --jev`) talks to anything.
- Config: `JEV_API_KEY` (or the vendor alias `TYPESAFE_API_KEY`), `JEV_API_URL`
  (default `https://api.typesafe.ai/v1/systemone`), `JEV_MODEL` (default `jev-latest`).
  The key lives in a git-ignored `.env`; it is never logged or stored.
- Wire contract is confined to `src/jev_api.rs` (one place to audit).

## CLI surface

| Command | What it does |
|---|---|
| `list --from <harness>` | read any installed agent's history |
| `view <id>#<n>` | open one message from the original, untouched |
| `context "<task>" --from <harness>` | retrieve + Jev-rank + print (read-only, 1 Jev call) |
| `continue <id> --retrieve "<task>" --jev --budget N --with <harness>` | the handoff: retrieve, rank, compress, write into the target's own store (1 Jev call) |
| `export`, `crop`, `query` | format conversion / selection utilities |

Root overrides make everything redirectable (used for hermetic testing):
`CONTEXTLELEO_ANTIGRAVITY_ROOT`, `FREEBUFF_PROJECTS_DIR`, `FREEBUFF_DESKTOP_STATE_PATH`.

## Supported agents (18 and counting)

Read + write: Claude Code, Codex, OpenCode, Cursor CLI, Cursor desktop, pi, Campfire,
Cowork, Grok CLI, fx, **Antigravity**, **Freebuff**. Read-only or special: Grok Bot,
Hermes, Amp, Cloud Cowork, Claude Chat, ChatGPT.

The two most recent additions — Antigravity and Freebuff — matter because they are
**desktop apps**: contextleleo writes into the SQLite stores the installed apps actually
use (`~/.gemini/antigravity/conversations/<id>.db` + `brain/<id>/` logs; Freebuff's
`~/.config/freebuff-desktop/projects/<name>-<uuid>/desktop-v2.db`), so a handed-off session
can be shown *inside the app*.

## The demo (customer-facing, recordable)

One story, end to end: **tonight's checkout-api recurrence in Antigravity starts with what
an earlier session already found, and is handed to Freebuff as ranked, compressed context —
with Jev deciding what matters.**

- Act 1 seeds **two** real sessions: the earlier incident (redis-cli dumps, red herrings, a
  1.9k-token `CLIENT LIST` dump, the root cause, the fix — 25 messages) and tonight's repeat
  (same symptom, a deliberately wrong first hypothesis, no answer yet — 10 messages).
- Act 2 `context` retrieves read-only across **both** sessions: 3 chunks, ~598 tokens,
  relevances 0.94 / 0.97 / 0.97 — all three from the earlier incident.
- Act 3 `continue <tonight> --retrieve --jev --with freebuff` writes the handoff: tonight's
  session is excluded from its own retrieval, so all 7 retrieved chunks come from the earlier
  incident; copy `of=18`, 5 folds, largest ~107 tokens.
- Act 4 `list`/`view` proves the new Freebuff thread exists, **asserts** that no retrieved
  chunk came from the session being continued, and shows the Antigravity original untouched.

Everything is **real**: real stores, real Jev calls, real compression. Only the *story
content* is seeded. Runs are **hermetic by default** (everything under `demo/.state/`,
`rm -rf` to clean); `--live` writes into the installed apps' real stores and prints undo
commands first. Artifacts: `demo/README.md`, `demo/VIDEO_SCRIPT.md` (timecoded),
`demo/transcript.md` (a real captured run), `demo/run.sh`.

## The desktop-app index limitation (found and fixed)

Writing into a desktop app's store is not enough for it to *list* the session. Measured
against Antigravity's own language server (`--headless`, `HOME` redirected at a copy of a
real store):

- The app keeps a sidebar index (`conversation_summaries.db`) that it **rebuilds from
  `conversations/*.db` only when it starts** — no directory watcher. Log:
  `summary store: starting background reconciliation (trigger=startup)` →
  `[Summaries] reconcile checked 15 conversations, restored 1, cached 14, skipped 0, dropped 0`.
- A session written **before** launch is found and indexed (preview, step count, workspace
  all derived from the db). One written 25 s **after** launch is ignored.
- Writing index rows by hand is pointless (they get re-derived); deleting a db does not
  prune its row (so undo must delete both).

Fix shipped: `save` prints a one-line **stderr** note when the target root is app-owned
(`app_owns_session_list`, keyed on the index file); the demo's `--live` banner tells the
presenter to *run first, then open/relaunch the app*, and its undo cleans the index row too.
**Freebuff's live pickup is still unmeasured** (its local API requires the app's own auth
token) — its bundle ships no watcher either, so the docs say "reopen it after the run".

## Current status (2026-10-08)

- **v0.14.4**, release binary builds `--locked`; **564 tests pass / 0 failed**; `cargo fmt`
  and both clippy invocations (all-features/all-targets, lib no-default-features) green.
- Demo rehearsed end-to-end against the **real Jev API**: exit 0; artifacts match the
  captured transcript.
- Committed on `main` as `fd3f109` (the Jev stage + demo kit) and `25233d2` (this brief) —
  both local, not pushed at the time of writing. The two-session demo rebuild and the
  retrieval-exclusion / prompt-hardening change are working-tree changes.

## Open items

1. Stand-in `view` pointers under `--retrieve` — **fixed 2026-10-09** (`jev::apply_with` + origin
   map); re-measured in a live demo run on 2026-10-09 (pointers match chunk headers); `demo/transcript.md` predates it.
2. Freebuff live sidebar pickup: unmeasured (see above).
3. B-roll rehearsal: run `--live`, then open both apps and confirm the seeded session and
   handoff appear.
4. No evidence yet that retrieval beats `rg` on a labelled task set (no-retrieval vs
   local-only vs Jev-filtered) — the reviewer's point, and the honest next experiment.
5. `docs/assets/demo.gif` still shows the old project name — needs a re-record.
6. No CI smoke test for the demo (would need a mock Jev endpoint to run keyless).
7. Publishing to crates.io / npm not done (package names are already `contextleleo`).
8. Remaining review items not yet done: partial Jev answers (retry/backoff is done), block-level
   chunks with neighbouring lines, dropping the local `min_relevance` pre-gate before Jev,
   moving `jev_api` out of default features.

## Where the code lives

| Path | Contents |
|---|---|
| `src/retrieval.rs` | candidate generation, assembly, Jev ranking integration, pipeline |
| `src/jev_api.rs` | the Jev wire contract (client, candidates, reconciliation) |
| `src/harness/*.rs` | one adapter per agent — reads and writes its real store format |
| `src/harness/antigravity.rs` / `freebuff.rs` | the two desktop-app stores, incl. the index fix |
| `cli/src/lib.rs` | `context`, `continue --retrieve`, `list`/`view` wiring |
| `demo/*` | the recordable demo kit |
| `context.md` | the full living project log (§18 = Jev, §19 = demo) |
| `docs/formats/*.md` | per-agent format references |
