#!/usr/bin/env bash
# Customer demo: an Antigravity session that needs what an *earlier* Antigravity
# session already found, handed to Freebuff with Jev deciding which history
# matters and the optimizer compressing it to a budget.
#
#   ./demo/run.sh            hermetic: every artifact lands under demo/.state
#   ./demo/run.sh --live     write into the real Antigravity / Freebuff stores,
#                            so the two agent CLIs themselves can resume them
#   ./demo/run.sh --reset    delete demo/.state first
#
# Everything printed below is the program's own output; nothing is staged.
#
# The pipeline, in the order the script runs it:
#   1. two sample Antigravity CLI (`agy`) sessions are written into the
#      Antigravity store (Simple document -> native SQLite conversation): the
#      earlier incident that found the root cause, and tonight's repeat, which
#      does not know it yet
#   2. `context` asks history directly: Jev ranks the candidates from both
#      sessions, the optimizer fits the kept set to a budget   (read-only)
#   3. `continue --retrieve --jev --with freebuff` runs *from tonight's
#      session* — which is excluded from its own retrieval, so every chunk
#      comes from the earlier incident — and writes a compressed handoff
#      thread into Freebuff
#   4. `list` + `view` prove the Freebuff thread exists, that no chunk came
#      from the session being continued, and what the handoff contains

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STATE="$REPO/demo/.state"
WORK="$STATE/work/checkout-api"
SEED_EARLIER="$REPO/demo/seed/antigravity-checkout-incident.json"
SEED_TODAY="$REPO/demo/seed/antigravity-checkout-recurrence.json"
QUERY='what caused the checkout p99 latency spike and what fix was applied'
JEV_QUERY='redis connection pool exhaustion checkout p99 latency'
HANDOFF_BUDGET=1200
CONTEXT_BUDGET=2000

LIVE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --live) LIVE=1 ;;
    --reset) rm -rf "$STATE" ;;
    -h | --help)
      sed -n '2,20p' "${BASH_SOURCE[0]}"
      exit 0
      ;;
    *)
      echo "unknown flag: $1 (try --help)" >&2
      exit 2
      ;;
  esac
  shift
done

# ── presentation ──────────────────────────────────────────────────────────
if [ -t 1 ]; then BOLD=$'\033[1m'; DIM=$'\033[2m'; CYAN=$'\033[36m'; OFF=$'\033[0m'
else BOLD=''; DIM=''; CYAN=''; OFF=''; fi

act() { printf '\n%s%s%s\n%s%s%s\n' "$BOLD" "── $1 ──" "$OFF" "$DIM" "$2" "$OFF"; }
say() { printf '%s%s%s\n' "$DIM" "$1" "$OFF"; }
run() {
  printf '%s$ %s%s\n' "$CYAN" "$*" "$OFF"
  "$@"
}
# How many messages `view` renders for a session (the `── #n ──` headers).
count_view() { "$BIN" view "$1" --from "$2" | grep -c '^── #'; }
# The Antigravity session ids in `list` order. `awk` rather than `head`: it
# reads to the end of its input, so `set -o pipefail` never sees a SIGPIPE.
ag_ids() { "$BIN" list --from antigravity | grep -oE '[0-9a-f]{8}-([0-9a-f]{4}-){3}[0-9a-f]{12}'; }
first_line() { awk 'NR==1'; }

# ── preflight ─────────────────────────────────────────────────────────────
if [ -f "$REPO/.env" ]; then
  set -a
  # shellcheck disable=SC1091
  . "$REPO/.env"
  set +a
fi
KEY="${JEV_API_KEY:-${TYPESAFE_API_KEY:-}}"
if [ -z "$KEY" ]; then
  echo "error: no Jev key. Put JEV_API_KEY (or TYPESAFE_API_KEY) in $REPO/.env" >&2
  echo "       Jev is the ranking stage; without it retrieval has no intelligence layer." >&2
  exit 1
fi

BIN="$REPO/target/release/contextleleo"
if [ ! -x "$BIN" ]; then
  if [ -x "$REPO/target/debug/contextleleo" ]; then
    BIN="$REPO/target/debug/contextleleo"
  else
    say "building the CLI (first run only)…"
    # shellcheck disable=SC1091
    [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
    (cd "$REPO" && cargo build --release --locked -p contextleleo-cli >/dev/null)
    BIN="$REPO/target/release/contextleleo"
  fi
fi

mkdir -p "$WORK"
seed_earlier="$STATE/seed-earlier.json"
seed_today="$STATE/seed-today.json"
sed "s|__DEMO_CWD__|$WORK|" "$SEED_EARLIER" >"$seed_earlier"
sed "s|__DEMO_CWD__|$WORK|" "$SEED_TODAY" >"$seed_today"

if [ "$LIVE" = 1 ]; then
  # Which Antigravity store this machine actually has: the standalone CLI, the
  # desktop app, or the older IDE build. Same layout, same code path.
  AG_LIVE=''
  for r in "$HOME/.gemini/antigravity-cli" "$HOME/.gemini/antigravity" "$HOME/.gemini/antigravity-ide"; do
    [ -d "$r" ] && {
      AG_LIVE="$r"
      break
    }
  done
  FB_LIVE="${FREEBUFF_PROJECTS_DIR:-$HOME/.config/freebuff-desktop/projects}"
  # A desktop-app store also has the app's own conversation list beside
  # `conversations/`, rebuilt from the databases only when the app launches
  # (measured: no watcher, reconcile at startup), and not pruned when a
  # database is deleted — so the app-side notes below are added only there.
  AG_INDEX=''
  if [ -n "$AG_LIVE" ] && [ -f "$AG_LIVE/conversation_summaries.db" ]; then
    AG_INDEX="$AG_LIVE/conversation_summaries.db"
  fi
  if [ -n "$AG_INDEX" ]; then
    AG_INDEX_NOTE="  ${DIM}Antigravity indexes sessions when it launches — it lists this one from its
  next start, so relaunch the app after this run and before the B-roll.${OFF}"
    AG_INDEX_UNDO="  ${DIM}sqlite3 $AG_INDEX \"delete from conversation_summaries where conversation_id='<id>';\"${OFF}"
  else
    AG_INDEX_NOTE=''
    AG_INDEX_UNDO=''
  fi
  cat <<EOF

${BOLD}LIVE MODE${OFF} — the sample session goes into your real agent stores, so the
apps themselves can open it. Nothing here is read by the script afterwards.
  ${DIM}antigravity: ${AG_LIVE:-(none installed — will be created at ~/.gemini/antigravity-cli)}${OFF}
  ${DIM}freebuff:    $FB_LIVE  (as a new checkout-api-<id> thread)${OFF}
$AG_INDEX_NOTE
Undo, using the two ids Act 1 prints:
  ${DIM}rm -f   <antigravity root>/conversations/<id>.db  and brain/<id>/  (both ids)${OFF}
$AG_INDEX_UNDO
  ${DIM}rm -rf  $FB_LIVE/checkout-api-*${OFF}
EOF
else
  # Hermetic: the Antigravity root and the Freebuff projects dir are both
  # redirected into demo/.state, so nothing on this machine is touched.
  export CONTEXTLELEO_ANTIGRAVITY_ROOT="$STATE/antigravity"
  export FREEBUFF_PROJECTS_DIR="$STATE/freebuff-projects"
fi

echo
echo "${BOLD}contextleleo demo${OFF} · Antigravity CLI → Jev → Freebuff"
say "binary:       $BIN"
say "jev key:      present (${#KEY} chars; value never printed)"
say "story:        a repeat incident in Antigravity CLI, finished in Freebuff with the earlier run's findings"
say "state:        $STATE"

# ── act 1: two real Antigravity CLI sessions ──────────────────────────────
act "ACT 1 · the two sessions Antigravity CLI left behind" \
  "Two real sessions in the harness's own store: the earlier incident — redis-cli"
say "dumps, red herrings, root cause, fix — and tonight's repeat, which does not know it yet."
run "$BIN" continue "$seed_earlier" --with antigravity --no-resume
AG_EARLIER="$(ag_ids | first_line)"
run "$BIN" continue "$seed_today" --with antigravity --no-resume
AG_TODAY="$(ag_ids | grep -v "^$AG_EARLIER$" | first_line)"
if [ -z "$AG_TODAY" ]; then
  echo "error: the second session did not land in the Antigravity store" >&2
  exit 1
fi
echo
run "$BIN" list --from antigravity
say "earlier incident:  $AG_EARLIER · $(count_view "$AG_EARLIER" antigravity) messages"
say "tonight's session: $AG_TODAY · $(count_view "$AG_TODAY" antigravity) messages"

# ── act 2: retrieval + Jev, read-only ─────────────────────────────────────
act "ACT 2 · ask history directly: retrieve + rank + compress (read-only)" \
  "Both sessions are candidates here: local search gathers them, the Jev API"
say "decides which matter, and the optimizer fits the kept set to --budget $CONTEXT_BUDGET."
echo
printf '%s$ %s%s\n' "$CYAN" "$BIN context '$JEV_QUERY' --from antigravity --max-chunks 3 --budget $CONTEXT_BUDGET" "$OFF"
CONTEXT_OUT="$("$BIN" context "$JEV_QUERY" --from antigravity --max-chunks 3 --budget "$CONTEXT_BUDGET")"
printf '%s\n' "$CONTEXT_OUT"
# The first printed chunk names exactly where it came from; keep that locator
# so the last act can open the original message it points at. The printed form
# is `harness:session#message`; `view` takes the session and the message, so
# the harness prefix comes off here.
TOP_REF="$(printf '%s\n' "$CONTEXT_OUT" | grep -oE 'antigravity:[0-9a-f-]+#[0-9]+' | first_line)"
TOP_REF="${TOP_REF:-antigravity:$AG_EARLIER#1}"
TOP_VIEW="${TOP_REF#*:}"

# ── act 3: the cross-session handoff ──────────────────────────────────────
act "ACT 3 · carry it to Freebuff, from tonight's session" \
  "The session being continued is excluded from its own retrieval — a session is"
say "not its own memory — so what gets prepended is the earlier incident's findings."
echo
run "$BIN" continue "$AG_TODAY" --retrieve "$QUERY" --jev --budget "$HANDOFF_BUDGET" \
  --with freebuff --no-resume

# ── act 4: proof ──────────────────────────────────────────────────────────
FB_ID="$("$BIN" list --from freebuff -n 1 | grep -oE '[0-9a-f]{8}-([0-9a-f]{4}-){3}[0-9a-f]{12}' | first_line)"
act "ACT 4 · the Freebuff thread that now exists" \
  "A new session in Freebuff's own store, written by the handoff above."
run "$BIN" list --from freebuff -n 3
echo
say "the handoff starts with the retrieved context — every chunk names its source:"
echo
run "$BIN" view "${FB_ID}#1-6" --from freebuff
FB_FULL="$(run "$BIN" view "$FB_ID" --from freebuff)"
# The demo's own acceptance check, asserted rather than narrated: the handoff
# must draw on the earlier incident and must never retrieve the session it is
# continuing. If the exclusion regresses, this run fails instead of lying.
RETRIEVED_SOURCES="$(printf '%s\n' "$FB_FULL" | grep -oE 'source antigravity:[0-9a-f-]+' | sort -u)"
case "$RETRIEVED_SOURCES" in
  *"antigravity:$AG_TODAY"*)
    printf 'error: the handoff retrieved the session it was continuing (%s)\n' \
      "$AG_TODAY" >&2
    exit 1
    ;;
esac
case "$RETRIEVED_SOURCES" in
  *"antigravity:$AG_EARLIER"*) ;;
  *)
    echo "error: the handoff retrieved nothing from the earlier incident" >&2
    exit 1
    ;;
esac
say "retrieved from $(printf '%s' "$RETRIEVED_SOURCES" | sed 's/^source //' | tr '\n' ' ') — the earlier incident, never the session being continued."
echo
STANDINS="$(printf '%s\n' "$FB_FULL" | grep -c 'jev: compressed')"
# The fold that replaced the most context, and the copy's own message number
# for it (the stand-in's own `message N` is an index into this copy, so the
# view is N+1), so the demo can open that fold back up.
BIG_REF="$(printf '%s\n' "$FB_FULL" | grep -o 'message [0-9]* (~[0-9]* tokens)' \
  | sort -t'~' -k2 -rn | first_line)"
BIG_MSG="$(printf '%s' "$BIG_REF" | grep -o '[0-9]*' | first_line)"
BIG_TOKENS="$(printf '%s' "$BIG_REF" | grep -o '~[0-9]*' | first_line)"
if [ -z "$BIG_MSG" ]; then
  echo "error: no compressed stand-in in the handoff — the budget forced no folding" >&2
  exit 1
fi
if [ "$BIG_TOKENS" = "~0" ]; then
  echo "error: the largest fold replaced nothing" >&2
  exit 1
fi
say "to fit the budget, the optimizer cut retrieved chunks to a preview plus a stand-in."
say "The largest fold replaced $BIG_TOKENS tokens, at the copy's message #$((BIG_MSG + 1)):"
echo
run "$BIN" view "${FB_ID}#$((BIG_MSG + 1))" --from freebuff
echo
say "and the message that chunk came from, in the earlier incident, untouched:"
echo
run "$BIN" view "$TOP_VIEW" --from antigravity
printf '\n%sfolds in the handoff: %s · earlier incident: %s messages · tonight: %s messages%s\n' \
  "$BOLD" "$STANDINS" "$(count_view "$AG_EARLIER" antigravity)" \
  "$(count_view "$AG_TODAY" antigravity)" "$OFF"

cat <<EOF

${BOLD}done.${OFF} What happened:
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

Hermetic run: delete everything with  rm -rf "$STATE"
EOF
