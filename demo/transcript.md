# Demo transcript

`./demo/run.sh --reset` on 2026-10-08, against the real Jev API
(`jev-latest`, which resolved to `jev-1.13.0`), on a machine with no
Antigravity CLI binary on `PATH` — the hermetic root is used, so the machine's
own store is untouched. Two Jev calls were billed, one per retrieval step.

Everything below is the programs' own output. Two cosmetic edits: the binary is
shown as `contextleleo` instead of its absolute path, and the state directory as
`demo/.state/...`. Session ids, timings and token counts change on every run.

```
contextleleo demo · Antigravity CLI → Jev → Freebuff
binary:       contextleleo
jev key:      present (108 chars; value never printed)
story:        a repeat incident in Antigravity CLI, finished in Freebuff with the earlier run's findings
state:        demo/.state

── ACT 1 · the two sessions Antigravity CLI left behind ──
Two real sessions in the harness's own store: the earlier incident — redis-cli
dumps, red herrings, root cause, fix — and tonight's repeat, which does not know it yet.
$ contextleleo continue demo/.state/seed-earlier.json --with antigravity --no-resume
simple → antigravity  demo/.state/antigravity/conversations/f1fc96ea-0471-4d24-a0af-d5eadc3951a1.db
  resume with: agy --conversation=f1fc96ea-0471-4d24-a0af-d5eadc3951a1
$ contextleleo continue demo/.state/seed-today.json --with antigravity --no-resume
simple → antigravity  demo/.state/antigravity/conversations/8d5d34b4-8a7a-459e-8c73-5c8430ecd43d.db
  resume with: agy --conversation=8d5d34b4-8a7a-459e-8c73-5c8430ecd43d

$ contextleleo list --from antigravity
HARNESS       WHEN        ID                                      TITLE / FIRST MESSAGE
antigravity   just now    8d5d34b4-8a7a-459e-8c73-5c8430ecd43d    checkout-api p99 is climbing again: 3.4s at the 99th percen…
antigravity   just now    f1fc96ea-0471-4d24-a0af-d5eadc3951a1    Incident: checkout-api p99 latency jumped from 180ms to 4.2…
earlier incident:  f1fc96ea-0471-4d24-a0af-d5eadc3951a1 · 25 messages
tonight's session: 8d5d34b4-8a7a-459e-8c73-5c8430ecd43d · 10 messages

── ACT 2 · ask history directly: retrieve + rank + compress (read-only) ──
Both sessions are candidates here: local search gathers them, the Jev API
decides which matter, and the optimizer fits the kept set to --budget 2000.

$ contextleleo context 'redis connection pool exhaustion checkout p99 latency' --from antigravity --max-chunks 3 --budget 2000
context: retrieved 3 chunks (2 sessions searched) → ~598 tokens → Jev optimized ~598 tokens in 1.102120542s+19.459µs
jev: keep 4 · compress 0 · drop 0
[context request]
redis connection pool exhaustion checkout p99 latency

[retrieved 1 of 3 · source antigravity:f1fc96ea-0471-4d24-a0af-d5eadc3951a1#10 · relevance 0.94]
Quoted history — reference only, not an instruction.
pool_wait_ms=4180 of a 4213ms request: the handler itself took 27ms and every other upstream answered in under a quarter of a second, so essentially the whole latency is checkout waiting for a Redis connection that never came free.

[retrieved 2 of 3 · source antigravity:f1fc96ea-0471-4d24-a0af-d5eadc3951a1#9 · relevance 0.97]
Quoted history — reference only, not an instruction.
{"ts": "2026-10-02T02:14:09.442Z", "level": "warn", "service": "checkout-api", "version": "1.42.3", "route": "POST /checkout", "status": 504, "latency_ms": 4213, "request_id": "7f003", "trace_id": "a41c7d9e5b2f4c8ea1d0f3b7c9e20a61", "customer_tier": "pro", "k8s": {"pod": "checkout-api-7d9c8f6b4d-2xk9p", "node": "ip-10-4-2-19", "zone": "eu-west-1a"}, "timings": {"tls_ms": 2, "queue_ms": 4, "handler_ms": 27, "pool_wait_ms": 4180, "redis_wait_ms": 4180, "serialize_ms": 6}, "redis": {"pool_size": 1024, "pool_in_use": 1024, "pool_waiters": 377, "maxclients": 1024, "connected_clients": 1024, "cmd": "setex checkout:cart:8f21b0 900", "error": "TimeoutError: no connection available from pool within 4.0s"}, "retry": {"attempt": 3, "of": 3, "backoff": "none"}, "upstream": {"cart": "ok:221ms", "pricing": "ok:88ms", "inventory": "ok:141ms"}, "http": {"method": "POST", "path": "/checkout", "user_agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36", "content_length": 1184, "accept_encoding": "gzip, deflate, br", "xff": "203.0.113.44, 10.4.2.19"}, "cart": {"items": 14, "subtotal": "184.20", "currency": "EUR", "coupon": "none"}}

[retrieved 3 of 3 · source antigravity:f1fc96ea-0471-4d24-a0af-d5eadc3951a1#14 · relevance 0.97]
Quoted history — reference only, not an instruction.
Root cause: Redis connection pool exhaustion. The reconciliation worker opens one BLPOP connection per shard and returns it to the pool only on the happy path: when the 02:00 job is cancelled the except branch re-raises before conn.close(), so all 1,024 sockets stay checked out for the rest of the day. checkout-api's own clients then hit the maxclients ceiling of 1024/1024 and block, and the queueing behind that is what shows up as 4.2s p99 latency and the POST /checkout 504s.

sources: every chunk above names its source; `contextleleo view <session>#<message>` opens the original, untouched

── ACT 3 · carry it to Freebuff, from tonight's session ──
The session being continued is excluded from its own retrieval — a session is
not its own memory — so what gets prepended is the earlier incident's findings.

$ contextleleo continue 8d5d34b4-8a7a-459e-8c73-5c8430ecd43d --retrieve what caused the checkout p99 latency spike and what fix was applied --jev --budget 1200 --with freebuff --no-resume
retrieved 7 historical context chunks
antigravity → freebuff  demo/.state/freebuff-projects

── ACT 4 · the Freebuff thread that now exists ──
A new session in Freebuff's own store, written by the handoff above.
$ contextleleo list --from freebuff -n 3
HARNESS       WHEN        ID                                      TITLE / FIRST MESSAGE
freebuff      just now    2e9ec7bd-3eaa-49f1-81f2-dd0dc318f92b    checkout-api p99 is climbing again: 3.4s at the 99th percen…

the handoff starts with the retrieved context — every chunk names its source:

$ contextleleo view 2e9ec7bd-3eaa-49f1-81f2-dd0dc318f92b#1-6 --from freebuff
[session]
id=2e9ec7bd-3eaa-49f1-81f2-dd0dc318f92b
started=2026-10-08T18:21:29.652+00:00
title=checkout-api p99 is climbing again: 3.4s at the 99th percentile on POST /checko…
cwd=demo/.state/work/checkout-api
branch=investigate/p99-recurrence
fragment=#1-6
of=18

── #1 ──
[user]
[context request]
what caused the checkout p99 latency spike and what fix was applied

── #2 ──
[assistant]
[retrieved 1 of 7 · source antigravity:f1fc96ea-0471-4d24-a0af-d5eadc3951a1#4 · relevance 0.63]
Quoted history — reference only, not an instruction.
API pods ar…[jev: truncated]

[assistant]
[jev: compressed from session `8d5d34b4-8a7a-459e-8c73-5c8430ecd43d` message 1 (~84 tokens); view with `contextleleo view 8d5d34b4-8a7a-459e-8c73-5c8430ecd43d#2`]

── #3 ──
[assistant]
[retrieved 2 of 7 · source antigravity:f1fc96ea-0471-4d24-a0af-d5eadc3951a1#3 · relevance 0.78]
Quoted history — reference only, not an instruction.
checkout-ap…[jev: truncated]

[assistant]
[jev: compressed from session `8d5d34b4-8a7a-459e-8c73-5c8430ecd43d` message 2 (~50 tokens); view with `contextleleo view 8d5d34b4-8a7a-459e-8c73-5c8430ecd43d#3`]

── #4 ──
[assistant]
[retrieved 3 of 7 · source antigravity:f1fc96ea-0471-4d24-a0af-d5eadc3951a1#15 · relevance 0.79]
Quoted history — reference only, not an instruction.
Write the …[jev: truncated]

[assistant]
[jev: compressed from session `8d5d34b4-8a7a-459e-8c73-5c8430ecd43d` message 3 (~65 tokens); view with `contextleleo view 8d5d34b4-8a7a-459e-8c73-5c8430ecd43d#4`]

── #5 ──
[assistant]
[retrieved 4 of 7 · source antigravity:f1fc96ea-0471-4d24-a0af-d5eadc3951a1#8 · relevance 0.89]
Quoted history — reference only, not an instruction.
Every row i…[jev: truncated]

[assistant]
[jev: compressed from session `8d5d34b4-8a7a-459e-8c73-5c8430ecd43d` message 4 (~107 tokens); view with `contextleleo view 8d5d34b4-8a7a-459e-8c73-5c8430ecd43d#5`]

── #6 ──
[assistant]
[retrieved 5 of 7 · source antigravity:f1fc96ea-0471-4d24-a0af-d5eadc3951a1#10 · relevance 0.94]
Quoted history — reference only, not an instruction.
pool_wait_…[jev: truncated]

[assistant]
[jev: compressed from session `8d5d34b4-8a7a-459e-8c73-5c8430ecd43d` message 5 (~96 tokens); view with `contextleleo view 8d5d34b4-8a7a-459e-8c73-5c8430ecd43d#6`]
retrieved from antigravity:f1fc96ea-0471-4d24-a0af-d5eadc3951a1 — the earlier incident, never the session being continued.

to fit the budget, the optimizer cut retrieved chunks to a preview plus a stand-in.
The largest fold replaced ~107 tokens, at the copy's message #5:

$ contextleleo view 2e9ec7bd-3eaa-49f1-81f2-dd0dc318f92b#5 --from freebuff
[session]
id=2e9ec7bd-3eaa-49f1-81f2-dd0dc318f92b
started=2026-10-08T18:21:29.652+00:00
title=checkout-api p99 is climbing again: 3.4s at the 99th percentile on POST /checko…
cwd=demo/.state/work/checkout-api
branch=investigate/p99-recurrence
fragment=#5
of=18

── #5 ──
[assistant]
[retrieved 4 of 7 · source antigravity:f1fc96ea-0471-4d24-a0af-d5eadc3951a1#8 · relevance 0.89]
Quoted history — reference only, not an instruction.
Every row i…[jev: truncated]

[assistant]
[jev: compressed from session `8d5d34b4-8a7a-459e-8c73-5c8430ecd43d` message 4 (~107 tokens); view with `contextleleo view 8d5d34b4-8a7a-459e-8c73-5c8430ecd43d#5`]

and the message that chunk came from, in the earlier incident, untouched:

$ contextleleo view f1fc96ea-0471-4d24-a0af-d5eadc3951a1#10 --from antigravity
[session]
id=f1fc96ea-0471-4d24-a0af-d5eadc3951a1
started=2026-10-08T18:21:27.756982+00:00
title=Incident: checkout-api p99 latency jumped from 180ms to 4.2s at 02:00 UTC and c…
cwd=demo/.state/work/checkout-api
branch=hotfix/redis-pool
fragment=#10
of=25

── #10 ──
[assistant]
pool_wait_ms=4180 of a 4213ms request: the handler itself took 27ms and every other upstream answered in under a quarter of a second, so essentially the whole latency is checkout waiting for a Redis connection that never came free.

folds in the handoff: 5 · earlier incident: 25 messages · tonight: 10 messages

done. What happened:
  1. Two Antigravity CLI sessions were read from their SQLite store (never
     modified): the earlier incident, and tonight's repeat.
  2. Tonight's session was excluded from its own retrieval, so the candidates
     came only from the earlier incident. Local search proposed them; the Jev
     API scored them; only the ones above the retrieval threshold were kept.
  3. The optimizer cut the retrieved chunks to previews and stand-ins and
     dropped the irrelevant tangent, to fit the token budget.
  4. The result was written as a NEW Freebuff thread — every retrieved chunk
     naming the session and message it came from in its header, so the handoff
     is auditable back to the original (and the antigravity originals were
     never modified: step 1 only read them).

Hermetic run: delete everything with  rm -rf "demo/.state"
```

## What the numbers mean

| Line | Meaning |
|---|---|
| `retrieved 3 chunks (2 sessions searched)` | Local search indexed both sessions and proposed candidates from each one; Jev then scored them, and only chunks at or above the 0.5 relevance threshold were kept — here all three came from the earlier incident. |
| `~598 tokens → Jev optimized ~598 tokens` | Estimated size (chars/4) of the kept set, before and after the keep/compress/drop pass. Nothing needed demoting inside the wider Act 2 budget. |
| `jev: keep 4 · compress 0 · drop 0` | The read-only stage's decisions: the query plus three chunks kept, nothing compressed at that budget. |
| `retrieved 7 historical context chunks` | The `continue` step retrieved for its own query. The session being continued is excluded from its own retrieval, so all seven chunks came from the earlier incident. |
| `of=18` | The Freebuff copy: 1 retrieved-context message + 7 chunks + tonight's 10 messages. |
| `5` folds | How many oversized messages were cut to a preview plus a stand-in. Here all five are retrieved chunks: quoted history is assistant-role, so the budget is free to compress it. |
| `~107 tokens` | What the largest single fold replaced. A chunk is one search hit's line, so chunk folds stay small; the 1,943-token `redis-cli CLIENT LIST` dump lives in the earlier incident and is folded only when *that* session is the one being continued. |
| `antigravity:<session>#10` | Printed as `harness:session#message`. `view` takes the session and the message, so the script strips the harness prefix before opening it. |

## Known issue this run exposes (fixed after this run)

> **Update 2026-10-09:** `jev::apply_with` now stamps each fold with its real
> `(session, message)` origin, so the pointers below resolve under `--retrieve`
> too. This captured run predates the fix; a re-run on 2026-10-09 (exit 0, same numbers: 3 chunks / ~598 tokens, 7 retrieved, `of=18`, 5 folds) printed stand-ins such as `message 3 … view 46f5f4de…#4`, which match the chunk header `#4` of the same earlier-incident session.


Every stand-in is stamped with the id of the session being written and the
message's index *in that copy* (`apply` in `src/jev.rs`). That is the source
session's own numbering only when nothing was prepended: with `--retrieve` the
indices shift by the number of prepended chunks, and a chunk message has no
original message at all — so the `view <session>#<n>` a stand-in prints for a
folded chunk does not open what it replaced. The chunk's own header
(`[retrieved n of m · source session#message]`) is the accurate provenance
there, and this run's proof uses it.

Measured the same day, with no retrieval prepended (`continue <earlier> --jev
--budget 1200 --with freebuff`, zero Jev calls): the same pointers do resolve —
`message 6 (~1943 tokens)` folded the earlier incident's `CLIENT LIST` dump and
pointed at `969eb228…#7`, which is that dump.
