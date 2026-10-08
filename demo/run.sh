#!/usr/bin/env bash
# Customer demo: hand a real Antigravity CLI session to Freebuff, with Jev
# deciding which history matters and the optimizer compressing it to a budget.
#
#   ./demo/run.sh            hermetic: every artifact lands under demo/.state
#   ./demo/run.sh --live     write into the real Antigravity / Freebuff stores,
#                            so the two agent CLIs themselves can resume them
#   ./demo/run.sh --reset    delete demo/.state first
#
# Everything printed below is the program's own output; nothing is staged.
#
# The pipeline, in the order the script runs it:
#   1. a sample Antigravity CLI (`agy`) incident session is written into the
#      Antigravity store (Simple document -> native SQLite conversation)
#   2. `context` retrieves from it: Jev ranks the candidates, the optimizer
#      fits the kept set to a token budget            (read-only)
#   3. `continue --retrieve --jev --with freebuff` takes that session's context
#      across to Freebuff as a new, compressed handoff thread
#   4. `list` + `view` prove the Freebuff thread exists and what it contains

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STATE="$REPO/demo/.state"
WORK="$STATE/work/checkout-api"
SEED="$REPO/demo/seed/antigravity-checkout-incident.json"
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
seed_doc="$STATE/seed.json"
sed "s|__DEMO_CWD__|$WORK|" "$SEED" >"$seed_doc"

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
Undo, using the id Act 1 prints:
  ${DIM}rm -f   <antigravity root>/conversations/<id>.db  and brain/<id>/${OFF}
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
say "story:        a production incident debugged in Antigravity CLI, finished in Freebuff"
say "state:        $STATE"

# ── act 1: a real Antigravity CLI session ─────────────────────────────────
act "ACT 1 · the session Antigravity CLI left behind" \
  "A real incident-debugging session — redis-cli dumps, a couple of red herrings,"
say "a root cause and the fix — written into the harness's own store, not a summary."
run "$BIN" continue "$seed_doc" --with antigravity --no-resume
AG_ID="$("$BIN" list --from antigravity | grep -oE '[0-9a-f]{8}-([0-9a-f]{4}-){3}[0-9a-f]{12}' | head -1)"
echo
run "$BIN" list --from antigravity
say "antigravity session: $AG_ID · $(count_view "$AG_ID" antigravity) messages"

# ── act 2: retrieval + Jev, read-only ─────────────────────────────────────
act "ACT 2 · retrieve + rank + compress (read-only)" \
  "Ask for history: local search gathers candidates, the Jev API decides which"
say "...matter and the optimizer fits the kept set to --budget $CONTEXT_BUDGET."
echo
printf '%s$ %s%s\n' "$CYAN" "$BIN context '$JEV_QUERY' --from antigravity --max-chunks 3 --budget $CONTEXT_BUDGET" "$OFF"
CONTEXT_OUT="$("$BIN" context "$JEV_QUERY" --from antigravity --max-chunks 3 --budget "$CONTEXT_BUDGET")"
printf '%s\n' "$CONTEXT_OUT"
# The first printed chunk names exactly where it came from; keep that locator
# so the last act can open the original message it points at.
TOP_MSG="$(printf '%s\n' "$CONTEXT_OUT" | grep -oE 'antigravity:[0-9a-f-]+#[0-9]+' | head -1 | sed 's/.*#//')"
TOP_MSG="${TOP_MSG:-1}"

# ── act 3: the cross-harness handoff ──────────────────────────────────────
act "ACT 3 · carry it to Freebuff" \
  "The same session continues in Freebuff: retrieved context is prepended, then"
say "the whole handoff is optimised to --budget $HANDOFF_BUDGET tokens."
echo
run "$BIN" continue "$AG_ID" --retrieve "$QUERY" --jev --budget "$HANDOFF_BUDGET" \
  --with freebuff --no-resume

# ── act 4: proof ──────────────────────────────────────────────────────────
FB_ID="$("$BIN" list --from freebuff -n 1 | grep -oE '[0-9a-f]{8}-([0-9a-f]{4}-){3}[0-9a-f]{12}' | head -1)"
act "ACT 4 · the Freebuff thread that now exists" \
  "A new session in Freebuff's own store, written by the handoff above."
run "$BIN" list --from freebuff -n 3
echo
say "the handoff starts with the retrieved context — every chunk names its source:"
echo
run "$BIN" view "${FB_ID}#1-6" --from freebuff
echo
STANDINS="$(run "$BIN" view "$FB_ID" --from freebuff | grep -c 'jev: compressed')"
# The stand-in that replaced the most context. Its own line names the original
# message it came from, so the demo can open that fold back up.
BIG_REF="$(run "$BIN" view "$FB_ID" --from freebuff \
  | grep -o 'message [0-9]* (~[0-9]* tokens)' | sort -t'~' -k2 -rn | head -1)"
BIG_MSG="$(printf '%s' "$BIG_REF" | grep -o '[0-9]*' | head -1)"
BIG_TOKENS="$(printf '%s' "$BIG_REF" | grep -o '~[0-9]*' | head -1)"
say "bulk tool output was folded into stand-ins that point back at the original."
say "The biggest one ($BIG_TOKENS tokens of tool output) is at the copy's message #$((BIG_MSG + 1)):"
echo
run "$BIN" view "${FB_ID}#$((BIG_MSG + 1))" --from freebuff
echo
say "and the message that chunk came from, in the original session, untouched:"
echo
run "$BIN" view "${AG_ID}#${TOP_MSG}" --from antigravity
printf '\n%scompressed stand-ins in the handoff: %s · original session: 25 messages%s\n' \
  "$BOLD" "$STANDINS" "$OFF"

cat <<EOF

${BOLD}done.${OFF} What happened:
  1. An Antigravity CLI session was read from its SQLite store (never modified).
  2. Local search proposed candidate chunks; the Jev API scored them; only the
     ones above the retrieval threshold were kept.
  3. The optimizer folded oversized tool output into stand-ins and dropped the
     irrelevant tangent, to fit the token budget.
  4. The result was written as a NEW Freebuff thread — with every chunk and
     every stand-in naming the session and message it came from, so nothing is
     unauditable.

Hermetic run: delete everything with  rm -rf "$STATE"
EOF
