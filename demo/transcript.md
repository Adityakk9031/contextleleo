# Demo transcript

`./demo/run.sh --reset` on 2026-10-08, against the real Jev API
(`jev-latest`, which resolved to `jev-1.13.0`), on a machine with no
Antigravity CLI installed. Two Jev calls were billed, one per retrieval step.

Everything below is the programs' own output. Two cosmetic edits: the binary is
shown as `contextleleo` instead of its absolute path, and the state directory as
`demo/.state/...`. Session ids, timings and token counts change on every run.

```
contextleleo demo · Antigravity CLI → Jev → Freebuff
binary:       contextleleo
jev key:      present (108 chars; value never printed)
story:        a production incident debugged in Antigravity CLI, finished in Freebuff
state:        demo/.state

── ACT 1 · the session Antigravity CLI left behind ──
A real incident-debugging session — redis-cli dumps, a couple of red herrings,
a root cause and the fix — written into the harness's own store, not a summary.
$ contextleleo continue demo/.state/seed.json --with antigravity --no-resume
simple → antigravity  demo/.state/antigravity/conversations/6aac96ab-0b11-4753-ae80-54107e3db487.db
  resume with: agy --conversation=6aac96ab-0b11-4753-ae80-54107e3db487

$ contextleleo list --from antigravity
HARNESS       WHEN        ID                                      TITLE / FIRST MESSAGE
antigravity   just now    6aac96ab-0b11-4753-ae80-54107e3db487    Incident: checkout-api p99 latency jumped from 180ms to 4.2…
antigravity session: 6aac96ab-0b11-4753-ae80-54107e3db487 · 25 messages

── ACT 2 · retrieve + rank + compress (read-only) ──
Ask for history: local search gathers candidates, the Jev API decides which
...matter and the optimizer fits the kept set to --budget 2000.

$ contextleleo context 'redis connection pool exhaustion checkout p99 latency' --from antigravity --max-chunks 3 --budget 2000
context: retrieved 3 chunks (1 sessions searched) → ~558 tokens → Jev optimized ~558 tokens in 752.326792ms+24.917µs
jev: keep 4 · compress 0 · drop 0
[context request]
redis connection pool exhaustion checkout p99 latency

[retrieved 1 of 3 · source antigravity:6aac96ab-0b11-4753-ae80-54107e3db487#10 · relevance 0.95]
pool_wait_ms=4180 of a 4213ms request: the handler itself took 27ms and every other upstream answered in under a quarter of a second, so essentially the whole latency is checkout waiting for a Redis connection that never came free.

[retrieved 2 of 3 · source antigravity:6aac96ab-0b11-4753-ae80-54107e3db487#9 · relevance 0.97]
{"ts": "2026-10-02T02:14:09.442Z", "level": "warn", "service": "checkout-api", "version": "1.42.3", "route": "POST /checkout", "status": 504, "latency_ms": 4213, "request_id": "7f003", "trace_id": "a41c7d9e5b2f4c8ea1d0f3b7c9e20a61", "customer_tier": "pro", "k8s": {"pod": "checkout-api-7d9c8f6b4d-2xk9p", "node": "ip-10-4-2-19", "zone": "eu-west-1a"}, "timings": {"tls_ms": 2, "queue_ms": 4, "handler_ms": 27, "pool_wait_ms": 4180, "redis_wait_ms": 4180, "serialize_ms": 6}, "redis": {"pool_size": 1024, "pool_in_use": 1024, "pool_waiters": 377, "maxclients": 1024, "connected_clients": 1024, "cmd": "setex checkout:cart:8f21b0 900", "error": "TimeoutError: no connection available from pool within 4.0s"}, "retry": {"attempt": 3, "of": 3, "backoff": "none"}, "upstream": {"cart": "ok:221ms", "pricing": "ok:88ms", "inventory": "ok:141ms"}, "http": {"method": "POST", "path": "/checkout", "user_agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36", "content_length": 1184, "accept_encoding": "gzip, deflate, br", "xff": "203.0.113.44, 10.4.2.19"}, "cart": {"items": 14, "subtotal": "184.20", "currency": "EUR", "coupon": "none"}}

[retrieved 3 of 3 · source antigravity:6aac96ab-0b11-4753-ae80-54107e3db487#14 · relevance 0.97]
Root cause: Redis connection pool exhaustion. The reconciliation worker opens one BLPOP connection per shard and returns it to the pool only on the happy path: when the 02:00 job is cancelled the except branch re-raises before conn.close(), so all 1,024 sockets stay checked out for the rest of the day. checkout-api's own clients then hit the maxclients ceiling of 1024/1024 and block, and the queueing behind that is what shows up as 4.2s p99 latency and the POST /checkout 504s.

sources: every chunk above names its source; `contextleleo view <session>#<message>` opens the original, untouched

── ACT 3 · carry it to Freebuff ──
The same session continues in Freebuff: retrieved context is prepended, then
the whole handoff is optimised to --budget 1200 tokens.

$ contextleleo continue 6aac96ab-0b11-4753-ae80-54107e3db487 --retrieve what caused the checkout p99 latency spike and what fix was applied --jev --budget 1200 --with freebuff --no-resume
retrieved 7 historical context chunks
antigravity → freebuff  demo/.state/freebuff-projects

── ACT 4 · the Freebuff thread that now exists ──
A new session in Freebuff's own store, written by the handoff above.
$ contextleleo list --from freebuff -n 3
HARNESS       WHEN        ID                                      TITLE / FIRST MESSAGE
freebuff      just now    f8b8413a-b60c-4c2e-ae5d-d8a60dc2e769    Incident: checkout-api p99 latency jumped from 180ms to 4.2…

the handoff starts with the retrieved context — every chunk names its source:

$ contextleleo view f8b8413a-b60c-4c2e-ae5d-d8a60dc2e769#1-6 --from freebuff
[session]
id=f8b8413a-b60c-4c2e-ae5d-d8a60dc2e769
started=2026-10-08T13:27:15.027+00:00
title=Incident: checkout-api p99 latency jumped from 180ms to 4.2s at 02:00 UTC and c…
cwd=demo/.state/work/checkout-api
branch=hotfix/redis-pool
fragment=#1-6
of=33

── #1 ──
[user]
[context request]
what caused the checkout p99 latency spike and what fix was applied

── #2 ──
[user]
[retrieved 1 of 7 · source antigravity:6aac96ab-0b11-4753-ae80-54107e3db487#4 · relevance 0.51]
API pods are idling at ~190m — nowhere near their CPU limits. Nothing here is saturated, so the stall is upstream of the process. Checking the Redis the checkout write path depends on.

── #3 ──
[user]
[retrieved 2 of 7 · source antigravity:6aac96ab-0b11-4753-ae80-54107e3db487#3 · relevance 0.71]
checkout-api-7d9c8f6b4d-2xk9p  187m         412Mi

── #4 ──
[user]
[retrieved 3 of 7 · source antigravity:6aac96ab-0b11-4753-ae80-54107e3db487#15 · relevance 0.84]
Write the fix: bound the reconciler pool and always return the connection, then give the API some headroom.

── #5 ──
[user]
[retrieved 4 of 7 · source antigravity:6aac96ab-0b11-4753-ae80-54107e3db487#8 · relevance 0.89]
Every row is the same: age and idle both around 2,800s and cmd=blpop. These are reconciliation workers parked on a queue, not API clients. One more check before we blame Redis: the structured log line for a timed-out checkout says where inside the request the 4.2s actually sat.

── #6 ──
[user]
[retrieved 5 of 7 · source antigravity:6aac96ab-0b11-4753-ae80-54107e3db487#10 · relevance 0.94]
pool_wait_ms=4180 of a 4213ms request: the handler itself took 27ms and every other upstream answered in under a quarter of a second, so essentially the whole latency is checkout waiting for a Redis connection that never came free.

bulk tool output was folded into stand-ins that point back at the original.
The biggest one (~1943 tokens of tool output) is at the copy's message #15:

$ contextleleo view f8b8413a-b60c-4c2e-ae5d-d8a60dc2e769#15 --from freebuff
[session]
id=f8b8413a-b60c-4c2e-ae5d-d8a60dc2e769
started=2026-10-08T13:27:15.027+00:00
title=Incident: checkout-api p99 latency jumped from 180ms to 4.2s at 02:00 UTC and c…
cwd=demo/.state/work/checkout-api
branch=hotfix/redis-pool
fragment=#15
of=33

── #15 ──
[result 1]
id=8100 addr=10.4.9.12:53100 laddr=10.4.9.1:6379 fd=9 name= age=2804 idle=2804 flags=N db=0 sub=0 psub=0 ssub=0 multi=-1 qbuf=26 qbuf-free=20448 argv-mem=10 mul…[jev: truncated]

[user]
[jev: compressed from session `6aac96ab-0b11-4753-ae80-54107e3db487` message 14 (~1943 tokens); view with `contextleleo view 6aac96ab-0b11-4753-ae80-54107e3db487#15`]

and the message that chunk came from, in the original session, untouched:

$ contextleleo view 6aac96ab-0b11-4753-ae80-54107e3db487#10 --from antigravity
[session]
id=6aac96ab-0b11-4753-ae80-54107e3db487
started=2026-10-08T13:27:13.322921+00:00
title=Incident: checkout-api p99 latency jumped from 180ms to 4.2s at 02:00 UTC and c…
cwd=demo/.state/work/checkout-api
branch=hotfix/redis-pool
fragment=#10
of=25

── #10 ──
[assistant]
pool_wait_ms=4180 of a 4213ms request: the handler itself took 27ms and every other upstream answered in under a quarter of a second, so essentially the whole latency is checkout waiting for a Redis connection that never came free.

compressed stand-ins in the handoff: 19 · original session: 25 messages

done. What happened:
  1. An Antigravity CLI session was read from its SQLite store (never modified).
  2. Local search proposed candidate chunks; the Jev API scored them; only the
     ones above the retrieval threshold were kept.
  3. The optimizer folded oversized tool output into stand-ins and dropped the
     irrelevant tangent, to fit the token budget.
  4. The result was written as a NEW Freebuff thread — with every chunk and
     every stand-in naming the session and message it came from, so nothing is
     unauditable.

Hermetic run: delete everything with  rm -rf "demo/.state"
```

## What the numbers mean

| Line | Meaning |
|---|---|
| `retrieved 3 chunks (1 sessions searched)` | Local search proposed candidates from the sessions it indexed; Jev then scored them. Only chunks at or above the 0.5 relevance threshold are kept. |
| `~558 tokens → Jev optimized ~558 tokens` | Estimated size (chars/4) of the kept set, before and after the keep/compress/drop pass. Nothing needed demoting inside the budget here. |
| `jev: keep 4 · compress 0 · drop 0` | The optimizer's decisions. Retrieved chunks are user-role text, which is never dropped — compression is what the *handoff* exercises. |
| `retrieved 7 historical context chunks` | The `continue` step retrieved for its own query, then prepended that context to the session being carried across. |
| `of=33` | The Freebuff copy: 1 retrieved-context message + 7 chunks + the original 25. |
| `19` stand-ins | How many oversized messages were folded instead of carried verbatim. |
| `~1943 tokens` | What the single largest fold replaced — a `redis-cli CLIENT LIST` dump, cut down to one line that names where the original lives. |
