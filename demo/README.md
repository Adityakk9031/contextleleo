# contextleleo demo — Antigravity CLI → Jev → Freebuff

One story, end to end: **a debugging session that happened in one agent CLI
(Antigravity) becomes usable context in another (Freebuff), without anyone
re-reading or re-pasting the transcript.**

You type one command. Local retrieval gathers candidates from history, the
**Jev API** (TypeSafe System One) decides which candidates actually matter for
the task at hand, and the existing optimizer folds oversized tool output into
stand-ins that point back at the original session — then the result is written
into Freebuff's own store as a new thread.

```
   Antigravity CLI session          contextleleo                    Freebuff
   ────────────────────────         ─────────────                    ────────
   25 messages, ~4.2k tokens   →   local candidates (no vectors)   →   new thread
   redis-cli dumps, red herrings   →   Jev scores each candidate   →   retrieved head
   root cause, the fix             →   keep/compress/drop to budget→   19 stand-ins
```

Every chunk and every stand-in names its source (`antigravity:<id>#14`), so the
compressed handoff is auditable and the original session is never modified.

## Run it

```sh
./demo/run.sh            # hermetic: everything lands in demo/.state/
./demo/run.sh --live     # writes into your real agent stores (see below)
./demo/run.sh --reset    # wipe demo/.state first
```

Requirements:

- the CLI — the script uses `target/release/contextleleo` (or `target/debug`),
  and builds it for you if neither exists;
- a **Jev API key** — put `JEV_API_KEY=…` (or `TYPESAFE_API_KEY=…`) in the
  repo's `.env`, or export it. Get one from
  [typesafe.ai](https://docs.typesafe.ai/introduction/quickstart). Without a
  key the script stops with a configuration error and nothing is written;
  that is intentional — there is no silent fallback on Jev paths.

The script makes **two Jev calls** (one per retrieval step). You pay for the
input tokens it sends; the excerpts it sends are bounded, and output tokens are
free on Jev's side.

## Fetch from your own history

Both harnesses read the store the *installed* apps actually use, so you can try
this on your real conversations with nothing seeded and nothing exported:

```sh
./target/release/contextleleo list --from antigravity   # your Antigravity conversations
./target/release/contextleleo list --from freebuff      # your Freebuff threads

# rank your own history for a task, print the budgeted result (1 Jev call)
./target/release/contextleleo context '<what you are working on>' \
  --from antigravity --max-chunks 3 --budget 2000

# hand a chosen session to the other tool, Jev-ranked and compressed
./target/release/contextleleo continue <session-id> \
  --retrieve '<query>' --jev --budget 1200 --with freebuff --no-resume
```

Antigravity's store is whichever of `~/.gemini/antigravity-cli` (standalone
CLI), `~/.gemini/antigravity` (desktop app) or `~/.gemini/antigravity-ide`
(older IDE build) exists; `CONTEXTLELEO_ANTIGRAVITY_ROOT` pins it. Freebuff
reads `~/.config/freebuff-desktop/projects`. Drop `--no-resume` to launch the
receiving harness on the new session instead of just printing where it landed.

## Reading the output

| Act | Command it runs | What it proves |
|---|---|---|
| 1 | `continue <seed> --with antigravity --no-resume` | The sample session is written into the harness's own SQLite store — contextleleo speaks both CLIs' formats, no exports or copy-paste. |
| 2 | `context "<task>" --from antigravity --max-chunks 3 --budget 2000` | Retrieval + ranking, read-only. Jev's relevance scores are printed per chunk; anything under 0.5 is dropped. |
| 3 | `continue <agy-id> --retrieve "<task>" --jev --budget 1200 --with freebuff --no-resume` | The handoff itself: retrieved context is prepended to the session, then the *whole* thing is optimized to a token budget and written as a new Freebuff thread. |
| 4 | `list`, `view <id>#1-6`, `view <id>#15` | The new thread exists; its first messages are the retrieved context with sources; bulky tool output is a stand-in naming the original message; the Antigravity original is untouched. |

Full real output, line by line, with a glossary: [transcript.md](transcript.md).
Shot-by-shot recording plan: [VIDEO_SCRIPT.md](VIDEO_SCRIPT.md).

## What is real, what is scripted

Real: the pipeline (search → Jev ranking → optimizer → harness write), the Jev
API calls, the relevance scores, the token counts, the compression, both
harness formats, and the stores — the Antigravity session is written as a real
`conversations/*.db` that `agy` would load, and the Freebuff thread is a real
row in a real `desktop-v2.db`.

Scripted: only the session *content* — a checkout-api incident story lives in
[seed/antigravity-checkout-incident.json](seed/antigravity-checkout-incident.json)
as a Simple interchange document, with a root cause, a couple of red herrings
and a multi-kilobyte structured log line so there is something worth ranking
and something worth compressing.

## Hermetic by default

`run.sh` points both harnesses at throwaway roots inside `demo/.state`:

```sh
CONTEXTLELEO_ANTIGRAVITY_ROOT=demo/.state/antigravity
FREEBUFF_PROJECTS_DIR=demo/.state/freebuff-projects
```

so your real Antigravity store and Freebuff app are untouched, and
`rm -rf demo/.state` is a complete cleanup.

`--live` drops both overrides and writes into the stores the installed apps
actually read — whichever of `~/.gemini/antigravity-cli` (standalone CLI),
`~/.gemini/antigravity` (desktop app) or `~/.gemini/antigravity-ide` exists,
plus `~/.config/freebuff-desktop/projects` — which is what you want if you
intend to *show* the two agent surfaces themselves: the seeded session lands in
the store the installed Antigravity app reads (and resumes in
`agy --conversation=<id>` if the standalone CLI is installed), and the handoff
lands among the Freebuff app's threads. It prints both paths and the undo
commands before writing anything, plus a "reopen the app" note when the store
it wrote into is one a desktop app owns.

Antigravity lists sessions from its own conversation index inside that store,
and rebuilds that index from `conversations/*.db` only when the app starts —
the app's language server reconciles at startup and does not watch the
directory, so a session written while it is open shows up in its list after the
next launch (and nothing has to be edited into the index by hand: the app
derives the row — preview, step count, workspace — from the database we write).
So run the script first and open the app for the B-roll: that is the order that
works, live store or not. Freebuff ships no directory watcher either, so treat
it the same way: reopen it after the run too. The terminal (`list` / `view`)
shows the session immediately either way.

## Troubleshooting

- `error: no Jev key` — nothing was sent anywhere. Add `JEV_API_KEY` to `.env`.
- Jev answers but every relevance is dropped — the query and the history may not
  share literal terms; retrieval prefilters on them. Try the words actually in
  the transcript (`redis`, `pool`, `p99`, `latency`).
- `warning: --budget N is below what this handoff can shed` — your messages and
  error results are never dropped by design. Raise `--budget` or narrow the
  handoff with a `#range`.
- Slow first run — the release build takes a minute; later runs are dominated by
  index build (~1s). Pass `--cache <path>` to reuse the index.
