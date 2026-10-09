# contextleleo demo — Antigravity CLI → Jev → Freebuff

One story, end to end: **tonight's debugging session in one agent CLI
(Antigravity) starts with what an earlier session already found — and carries
it into a different agent (Freebuff) without anyone re-reading or re-pasting a
transcript.**

You type one command. The session being continued is excluded from its own
retrieval — a session is not its own memory — so local retrieval gathers
candidates from the *other* sessions' history, the **Jev API** (TypeSafe System
One) decides which candidates actually matter for the task at hand, and the
existing optimizer cuts them to previews and stand-ins that fit the budget.
Then the result is written into Freebuff's own store as a new thread.

```
   Antigravity CLI (two sessions)     contextleleo                       Freebuff
   ──────────────────────────────     ─────────────                       ────────
   earlier incident: red herrings,  → local candidates (no vectors)   →  new thread
   redis dumps, root cause, the fix → Jev scores each candidate       →  retrieved head
   tonight's repeat: symptoms,      → keep/compress/drop to budget    →  fold stand-ins
   no answer yet (excluded from     → assemble with provenance        →  + tonight's own
   its own retrieval)                                                  10 messages
```

Every retrieved chunk's header names its source (`antigravity:<id>#14`), so the
compressed handoff is auditable back to the untouched original. The captured
run predates the fold-pointer fix — see
[transcript.md](transcript.md#known-issue-this-run-exposes).

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
| 1 | `continue <seed> --with antigravity --no-resume` ×2 | Both sample sessions are written into the harness's own SQLite store — contextleleo speaks both CLIs' formats, no exports or copy-paste. |
| 2 | `context "<task>" --from antigravity --max-chunks 3 --budget 2000` | Retrieval + ranking, read-only. Both sessions are candidates here; Jev's relevance scores are printed per chunk and anything under 0.5 is dropped. |
| 3 | `continue <tonight-id> --retrieve "<task>" --jev --budget 1200 --with freebuff --no-resume` | The handoff itself: the session being continued is excluded from retrieval, so the prepended context comes from the earlier incident; then the *whole* thing is optimized to a token budget and written as a new Freebuff thread. |
| 4 | `list`, `view <id>#1-6`, `view <id>#5`, `view <earlier-id>#10` | The new thread exists; its first messages are the retrieved context with sources; the script *asserts* that every source is the earlier incident and never the session being continued; a fold is shown at the copy's own message number; and the Antigravity original is untouched. |

Full real output, line by line, with a glossary: [transcript.md](transcript.md).
Shot-by-shot recording plan: [VIDEO_SCRIPT.md](VIDEO_SCRIPT.md).

## What is real, what is scripted

Real: the pipeline (search → Jev ranking → optimizer → harness write), the Jev
API calls, the relevance scores, the token counts, the compression, both
harness formats, and the stores — the Antigravity session is written as a real
`conversations/*.db` that `agy` would load, and the Freebuff thread is a real
row in a real `desktop-v2.db`.

Scripted: only the session *content* — a checkout-api incident story, in two
Simple interchange documents: [seed/antigravity-checkout-incident.json](seed/antigravity-checkout-incident.json)
(the earlier incident — root cause, the fix, a couple of red herrings, a
multi-kilobyte structured log line) and
[seed/antigravity-checkout-recurrence.json](seed/antigravity-checkout-recurrence.json)
(tonight's repeat — the same symptom, no answer yet, and a deliberately wrong
first hypothesis the earlier session overrides).

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
- `note: no other session matched — a session is not its own memory` — the
  `--retrieve` step found nothing but the session you are continuing. Pick a
  query the *other* sessions answer, or run `context` first to see what is
  retrievable.
- `warning: --budget N is below what this handoff can shed` — your messages and
  error results are never dropped by design. Raise `--budget` or narrow the
  handoff with a `#range`.
- Slow first run — the release build takes a minute; later runs are dominated by
  index build (~1s). Pass `--cache <path>` to reuse the index.
