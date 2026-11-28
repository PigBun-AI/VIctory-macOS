#!/bin/bash
# Tests of tools/mac/heavy-guard.sh with fake "heavy processes" (a sleep that calls itself victory-census), a fake game lock and a
# load injected through VICTORY_GUARD_LOAD. Nothing of the machine's real lock, slots or processes is touched: the state directory and
# the lock are in a temp dir, and every scan is limited to the descendants of this script (VICTORY_GUARD_SCOPE_PID), so a real
# cargo or probe-wine.exe of someone else is never seen. The launchd agent is not installed (the plist is only printed and checked).
#   tools/mac/test-heavy-guard.sh [TMPDIR]
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
T="${1:-$(mktemp -d "${TMPDIR:-/tmp}/heavy-guard-test.XXXXXX")}"
mkdir -p "$T"; T="$(cd "$T" && pwd)"
export VICTORY_HEAVY_DIR="$T/heavy" VICTORY_GAME_LOCK="$T/game.lock" VICTORY_HEAVY_POLL=1 VICTORY_HEAVY_QUEUE_POLL=0.2
export VICTORY_GUARD_SCOPE_PID=$$ VICTORY_GUARD_PLIST_DIR="$T/LaunchAgents"
unset VICTORY_HEAVY_SLOT VICTORY_GUARD_LOAD
mkdir -p "$VICTORY_HEAVY_DIR"
G="$ROOT/tools/mac/heavy-guard.sh"; H="$ROOT/tools/mac/heavy.sh"
TOK=$((300000 + $$))
. "$ROOT/tools/mac/game-lock.sh"
mine() { ps -axo pid=,args= | awk -v n="$((TOK + $1))" '$NF == n && $2 !~ /(^|\/)(bash|sh|perl|nice)$/ {print $1; exit}'; }
state() { ps -o stat= -p "$1" 2>/dev/null | tr -d ' '; }
nice_of() { ps -o ni= -p "$1" 2>/dev/null | tr -d ' '; }
# the fake game: a process of its own holds the fake lock (the guard never touches what runs below the holder, so the fake heavy
# processes, children of this script, must not be below it); releasing = it ends (a lock whose holder is gone is stale = free)
HOLDER=""
hold() {
    rm -f "$T/held.flag"
    bash -c '. "$1"; gl_acquire test x 5 || exit 1; echo ok >"$2"; exec sleep 99999' _ "$ROOT/tools/mac/game-lock.sh" "$T/held.flag" >/dev/null 2>&1 & HOLDER=$!
    local i=0; while [ ! -f "$T/held.flag" ] && [ $i -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
}
release() { [ -z "$HOLDER" ] || { kill -KILL "$HOLDER" 2>/dev/null; wait "$HOLDER" 2>/dev/null; HOLDER=""; }; rm -rf "$VICTORY_GAME_LOCK" "$VICTORY_GAME_LOCK.queue"; }
bgpids=()
cleanup() {
    release; rm -rf "$VICTORY_GAME_LOCK" "$VICTORY_GAME_LOCK.queue"
    for p in ${bgpids[@]+"${bgpids[@]}"}; do kill -TERM "$p" 2>/dev/null; done
    for k in 0 1 2 3 4 5 6 7; do p=$(mine $k); [ -z "$p" ] || { kill -CONT "$p" 2>/dev/null; kill -KILL "$p" 2>/dev/null; }; done
}
trap cleanup EXIT
fails=0; n=0
ok() { n=$((n + 1)); echo "ok $n - $1"; }
bad() { n=$((n + 1)); fails=$((fails + 1)); echo "NOT OK $n - $1"; }
expect() { if eval "$2"; then ok "$1"; else bad "$1   [$2]"; fi; }
waitfor() { local i=0; while ! eval "$2"; do sleep 0.2; i=$((i + 1)); [ $i -lt $(($3 * 5)) ] || { bad "$1 (not within $3 s: $2)"; return 1; }; done; ok "$1"; }
scan() { VICTORY_GUARD_LOAD=$1 bash "$G" once; }
logn() { grep -c "$1" "$VICTORY_HEAVY_DIR/guard.log" 2>/dev/null || true; }
# a fake heavy process called NAME ("victory-census"), N-th unique sleep: argv[0] is the name, the last word the number
fake() { # N NAME [ARGV0 extra words]
    ( exec -a "${2:-victory-census}" sleep $((TOK + $1)) ) & bgpids+=($!)
}

echo "== lock free: the guard does nothing"
fake 1; sleep 0.4; P1=$(mine 1)
expect "the fake process is found by its number" '[ -n "$P1" ]'
scan 99; sleep 0.2
expect "lock free, load 99: not reniced, not stopped, nothing logged" '[ "$(nice_of $P1)" = 0 ] && ! state $P1 | grep -q T && [ ! -s "$VICTORY_HEAVY_DIR/guard.log" ]'

echo "== lock held: renice 19; load > 30: SIGSTOP; load < 20 or lock released: SIGCONT"
hold
scan 10
expect "lock held, load 10: reniced to 19, not stopped" '[ "$(nice_of $P1)" = 19 ] && ! state $P1 | grep -q T'
expect "the log has time, pid, command, action, load, lock" 'grep -Eq "^20[0-9-]+T[0-9:]+ pid=$P1 cmd=.victory-census [0-9]+. action=renice 19 .was 0. load=10 lock=held" "$VICTORY_HEAVY_DIR/guard.log"'
scan 10
expect "a second scan does not renice again" '[ "$(logn "action=renice")" = 1 ]'
scan 45
expect "lock held, load 45: stopped" 'state $P1 | grep -q T && grep -q "pid=$P1 .*action=SIGSTOP load=45 lock=held" "$VICTORY_HEAVY_DIR/guard.log"'
scan 25
expect "load 25 (between 20 and 30): stays stopped" 'state $P1 | grep -q T && [ "$(logn "action=SIGCONT")" = 0 ]'
fake 2; sleep 0.4; P2=$(mine 2)
scan 25
expect "load 25 after a stop (hysteresis: stopping stays on until the load is under 20): a new heavy process is reniced and stopped too" 'state $P2 | grep -q T && [ "$(nice_of $P2)" = 19 ]'
scan 15
expect "load 15: both continue (SIGCONT), still reniced" '! state $P1 | grep -q T && ! state $P2 | grep -q T && [ "$(logn "action=SIGCONT")" = 2 ]'
scan 45
expect "load 45 again: stopped again" 'state $P1 | grep -q T && state $P2 | grep -q T'
release
scan 45
expect "lock released: continued whatever the load, nothing new stopped" '! state $P1 | grep -q T && ! state $P2 | grep -q T'
kill -KILL $P1 $P2; sleep 0.2

echo "== processes that are never touched"
: >"$VICTORY_HEAVY_DIR/guard.log"; rm -f "$VICTORY_HEAVY_DIR/guard.mode"
hold
( exec -a "victory-census victory.exe run --execute" sleep $((TOK + 3)) ) & bgpids+=($!)
mkdir -p "$T/fake"
printf '#!/bin/bash\n( exec -a victory-census sleep %s ) & wait\n' $((TOK + 4)) >"$T/fake/run.sh"
bash "$T/fake/run.sh" & bgpids+=($!)
( exec -a victory-census sleep $((TOK + 5)) ) & bgpids+=($!)      # a third one that is not protected: the control
( exec -a sleep sleep $((TOK + 6)) ) & bgpids+=($!)               # not a heavy name
sleep 0.6
P3=$(mine 3); P4=$(mine 4); P5=$(mine 5); P6=$(mine 6)
scan 60
expect "the control (a victory-census outside everything) is reniced and stopped" '[ "$(nice_of $P5)" = 19 ] && state $P5 | grep -q T'
expect "the game itself (victory.exe run --execute) is left alone" '[ "$(nice_of $P3)" = 0 ] && ! state $P3 | grep -q T'
expect "everything below a run.sh is left alone" '[ "$(nice_of $P4)" = 0 ] && ! state $P4 | grep -q T'
expect "a process with another name is left alone" '[ "$(nice_of $P6)" = 0 ] && ! state $P6 | grep -q T'
kill -KILL $P3 $P4 $P6 2>/dev/null
scan 5; kill -CONT $P5 2>/dev/null; kill -KILL $P5 2>/dev/null

echo "== a process in a heavy.sh slot is left alone (its job is heavy.sh's); one outside is not"
bash "$H" --jobs 1 --label inslot -- bash -c "exec -a victory-census sleep $((TOK + 1))" 2>/dev/null & bgpids+=($!)
fake 2; sleep 1.2
Pin=$(mine 1); Pout=$(mine 2)
scan 80
expect "in a slot: not reniced beyond heavy.sh's own nice, not stopped" '! state $Pin | grep -q T && ! grep -q "pid=$Pin " "$VICTORY_HEAVY_DIR/guard.log"'
expect "outside a slot: stopped" 'state $Pout | grep -q T'
scan 5
expect "load back: the one outside continues" '! state $Pout | grep -q T'
kill -KILL $Pout 2>/dev/null
release
kill -TERM "${bgpids[$((${#bgpids[@]} - 2))]}" 2>/dev/null

echo "== the loop (run) and its end"
bgpids=()
fake 1; sleep 0.4; P1=$(mine 1)
hold
rm -f "$VICTORY_HEAVY_DIR/guard.log" "$VICTORY_HEAVY_DIR/guard.mode" "$VICTORY_HEAVY_DIR/guard.stopped"
VICTORY_GUARD_LOAD=70 VICTORY_GUARD_INTERVAL=1 bash "$G" run >/dev/null 2>&1 & GP=$!; bgpids+=($GP)
waitfor "run: a scan within a second or two stops it" 'state $P1 | grep -q T' 8
kill -TERM $GP; wait $GP 2>/dev/null
expect "run ended by TERM: it continues what it stopped" '! state $P1 | grep -q T'
expect "the loop logged its start" 'grep -q "heavy-guard started" "$VICTORY_HEAVY_DIR/guard.log"'
release; kill -KILL $P1 2>/dev/null

echo "== launchd plist (not installed here)"
bash "$G" install --dry-run >"$T/plist.xml" 2>&1
expect "install --dry-run prints a valid plist" 'plutil -lint "$T/plist.xml" >/dev/null'
expect "the plist: label, KeepAlive, RunAtLoad, run, this script's path" 'grep -q "<string>com.gta6project.heavy-guard</string>" "$T/plist.xml" && grep -q "<key>KeepAlive</key><true/>" "$T/plist.xml" && grep -q "<key>RunAtLoad</key><true/>" "$T/plist.xml" && grep -q "<string>run</string>" "$T/plist.xml" && grep -q "$ROOT/tools/mac/heavy-guard.sh" "$T/plist.xml"'
expect "dry-run wrote nothing" '[ ! -e "$VICTORY_GUARD_PLIST_DIR" ]'
LOADED=$(launchctl list | awk 'NR > 1 && $1 != "-" {print $3; exit}')
VICTORY_GUARD_LABEL=$LOADED bash "$G" install >"$T/inst.out" 2>&1; rc=$?
expect "install refuses when an agent of that label is already loaded (checked with the label of a running system agent), and writes nothing" '[ $rc = 1 ] && grep -q "already loaded" "$T/inst.out" && [ ! -e "$VICTORY_GUARD_PLIST_DIR" ]'
expect "status works with no agent" 'bash "$G" status | grep -q "not loaded"'
expect "no argument: usage, exit 2" 'bash "$G" 2>/dev/null; [ $? = 2 ]'
echo
echo "$((n - fails)) of $n passed (logs in $T)"
[ "$fails" = 0 ]
