# Video script — “contextleleo searches. Jev decides.”

Target: **4:30**, one screen, terminal only (plus one optional app shot).
Real output to point at: [transcript.md](transcript.md). Runner: `demo/run.sh`.

## Before you hit record

1. `./demo/run.sh --reset` once so the stores are clean, then `clear`.
2. Terminal: dark theme, 16–18 pt mono, window ~110×34, no wrap of the long
   JSON line if you can help it. Prompt trimmed to `$`.
3. Confirm the key is present but **never visible**: the script prints
   `jev key: present (108 chars; value never printed)` and never the value. Do
   not `cat .env` on camera.
4. Have the numbers below cold — the video's credibility is that the scores and
   token counts are on screen, not in the narration.
5. The two Jev calls take ~1s each; that is the only real-time latency. Keep the
   prompts simple: two retrieval calls per run.

**Numbers to say out loud (all real, all on screen):** two Antigravity sessions —
25 messages of the earlier incident, 10 of tonight's repeat · **2 sessions
searched** · 3 chunks kept, relevances 0.94 / 0.97 / 0.97, all three from the
earlier incident · 7 chunks retrieved for the handoff · 18-message Freebuff copy ·
5 folds, the largest replacing ~107 tokens.

---

## 0:00 — Cold open (no terminal)

> “Every coding agent has the same amnesia. You debug something hard in one tool,
> open another, and it knows nothing. You either re-paste a 4,000-token
> transcript, or you start over.”

Caption: **Your context shouldn't be trapped in one agent's session store.**

> “This is contextleleo. It reads what the other tool already wrote down, asks a
> decision model what actually matters for the task in front of you, and carries
> only that across.”

Caption: **Retrieval finds. Jev decides. The optimizer compresses.**

## 0:20 — Act 1 · the sessions that already exist

Type `./demo/run.sh --reset` and let Act 1 run. Stop talking while the seeds are
written.

> “Act one is what you never do on camera: I’m not pasting anything. Two real
> sessions go into Antigravity CLI's SQLite store. The first is an incident from
> a couple of weeks ago — redis-cli dumps, a couple of red herrings, the root
> cause and the fix. The second is tonight: the same p99 spike on checkout-api,
> and no idea yet why.”

Point at the two `list` rows and at
`earlier incident: … · 25 messages` / `tonight's session: … · 10 messages`.

Caption: **Real `conversations/*.db`. No export, no copy-paste.**

## 0:45 — Act 2 · retrieval, with Jev’s scores visible

Act 2 runs automatically; slow down and point as the lines appear.

> “Now the interesting part. I ask for history with a task description. Local
> search proposes candidates from both sessions — cheap, local, no vector
> database, no network. Then the Jev API scores each one against *this* task.”

Point at, in order:

- `context: retrieved 3 chunks (2 sessions searched) → ~598 tokens`
- the three `[retrieved n of 3 · source antigravity:…#10 · relevance 0.94]` lines
  — “and notice which session answered: all three come from the earlier
  incident, because that is where the answer lives.”
- the third chunk, the single-line structured log — “this is one *line* of a
  real log, and it is a chunk; that's why the token math matters.”

> “Everything under a 0.5 relevance is dropped. Nothing here was summarised by a
> model and re-uploaded — these are the original messages, each naming exactly
> where it came from.”

Caption: **Every chunk is traceable: `harness:session#message`.**

## 1:45 — Act 3 · the handoff, one command

Act 3 runs. Point at the command line as it appears (it is long — that is the
point) and then at the two result lines.

> “This is the whole cross-tool story in one command: continue *tonight's*
> session, retrieve relevant history for the task, plan the handoff with Jev to a
> 1,200-token budget — and write it into Freebuff.”

The line to land is the one the script prints as it starts:

> “A session is not its own memory. Tonight's session is excluded from its own
> retrieval, so everything Jev considers comes from the *other* sessions — the
> earlier incident's root cause and fix, not a copy of the messages already on
> this session's screen.”

Point at `retrieved 7 historical context chunks` and
`antigravity → freebuff  demo/.state/freebuff-projects`.

Caption: **retrieve → optimize → write into the other harness's own format.**

## 2:30 — Act 4 · proof

Four things to point at, in order:

1. `list --from freebuff` — “a brand-new Freebuff thread, in Freebuff's store.”
2. `view <id>#1-6` — the retrieved head: **“the new session starts with exactly
   the context Jev decided mattered, each chunk stamped with its source and its
   relevance.”** And `of=18`: 1 request + 7 chunks + tonight's own 10.
3. `retrieved from antigravity:<earlier id> — the earlier incident, never the
   session being continued` — “that line is not narration; the script checks it
   and fails the run if a single chunk came from the session being continued.”

   > “This is the demo's own regression test. If the exclusion ever breaks, this
   > run goes red instead of quietly handing you your own messages back.”
4. The compressed chunk — `view <id>#5`:

> “And here's the compression. The retrieved chunks are cut to a preview plus a
> stand-in that carries the size of what it replaced — that is how the handoff
> fits a 1,200-token budget while keeping seven findings from another session.”

5. Finish on the untouched original: `view <earlier-id>#10 --from antigravity`.

> “The source session is exactly what it was. We never edit history — we quote
> it.”

Caption: **7 chunks from another session · 5 folds · both originals untouched.**

## 4:00 — Close

> “Two different agent CLIs, one continuous thread of memory, and a decision
> layer that tells the difference between the root cause and a CSS tangent. That
> is contextleleo plus Jev: **it searches locally, Jev decides, the optimizer
> compresses.** The whole run is in the repo — `demo/run.sh`.”

Caption: **github.com/Adityakk9031/contextleleo · `./demo/run.sh`**

---

## Optional B-roll (adds ~1:00, needs `--live`)

- `./demo/run.sh --live` once. It prints the Antigravity store this machine
  actually has — `~/.gemini/antigravity` for the desktop app,
  `~/.gemini/antigravity-cli` for the standalone CLI — and writes both seeded
  sessions into that one. Recorded after the run, not during it: the app
  indexes sessions only when it launches, so open (or relaunch) the desktop app
  and show the two seeded sessions in its list; if the standalone CLI is
  installed too, `agy --conversation=<seeded id>` works as well.
- Open the Freebuff app and show the handoff thread in the sidebar — the same
  session the terminal wrote. Its bundle has no directory watcher either, so
  open it after the run.
- Cut between the two apps on the beat “same context, two tools”.

## Optional extra beat: a big fold (zero Jev calls)

The biggest fold in the *retrieval* handoff is small because a chunk is one
search-hit line. If a large fold makes a better shot, continue the earlier
incident itself — with no retrieval, so it costs no Jev calls — and the
1,943-token `CLIENT LIST` dump gets folded:

```sh
./target/release/contextleleo continue <earlier-id> --jev --budget 1200 \
  --with freebuff --no-resume
```

That prints stand-ins for the session's own oversized messages, including
`~1943 tokens`, and — because nothing was prepended — its `view` pointer
resolves to the real original (`<earlier-id>#7`).

## Editing notes

- Trim the ~1s Jev latency only if it feels slow; the score appearing after a
  beat is the visual proof that a decision model is in the loop.
- Never show `.env`. If you leaked it while fumbling, blur or re-shoot.
- Keep the numbers in captions *as printed*; do not round them to prettier
  values — the point of the demo is that they are measured.
- If a run prints different UUIDs or timings, that is expected: ids, timings and
  token estimates change per run. Scores land in the same ranges.
- Stand-in pointers were fixed after this script was recorded (they now name the
  stored source session, also under `--retrieve`), but the captured run predates
  the fix. Before clicking one on camera, re-run the demo and open it once
  yourself; the chunk header above each stand-in is the always-accurate provenance.
- Chapter titles for YouTube: `0:00 context dies when you switch tools` ·
  `0:20 the sessions already on disk` · `0:45 retrieval ranked by Jev` ·
  `1:45 the one-command handoff` · `2:30 the compressed thread` · `4:00 close`.
