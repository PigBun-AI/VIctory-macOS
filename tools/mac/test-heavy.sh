#!/bin/bash
# Tests of tools/mac/heavy.sh with sleep and fake loads: a state directory and a game lock of their own (VICTORY_HEAVY_DIR,
# VICTORY_GAME_LOCK in a temp dir), nothing of the real machine's slots or lock is read or touched, and only processes this script
# started (sleep with a number that is unique to this run) are ever signalled. Takes about a minute.
#   tools/mac/test-heavy.sh [TMPDIR]
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
T="${1:-$(mktemp -d "${TMPDIR:-/tmp}/heavy-test.XXXXXX")}"
mkdir -p "$T"; T="$(cd "$T" && pwd)"
export VICTORY_HEAVY_DIR="$T/heavy" VICTORY_GAME_LOCK="$T/game.lock" VICTORY_HEAVY_BUDGET=4 VICTORY_HEAVY_POLL=1 VICTORY_HEAVY_QUEUE_POLL=0.2
unset VICTORY_HEAVY_SLOT VICTORY_HEAVY_JOBS
H="$ROOT/tools/mac/heavy.sh"
TOK=$((200000 + $$))      # `sleep <TOK+n>` is how the fake loads are found; no other process has these numbers
. "$ROOT/tools/mac/game-lock.sh"   # (the lock functions: the test takes the fake game lock itself)
mine() { ps -axo pid=,args= | awk -v n="$((TOK + $1))" '$2 == "sleep" && $3 == n {print $1; exit}'; }   # the pid of the fake load n (never heavy.sh)
state() { ps -o stat= -p "$1" 2>/dev/null | tr -d ' '; }
bgpids=()
cleanup() {
    gl_release 2>/dev/null; rm -rf "$VICTORY_GAME_LOCK" "$VICTORY_GAME_LOCK.queue"
    for p in ${bgpids[@]+"${bgpids[@]}"}; do kill -TERM "$p" 2>/dev/null; done
    sleep 1
    for n in 0 1 2 3 4 5 6; do p=$(mine $n); [ -z "$p" ] || { kill -CONT $p 2>/dev/null; kill -KILL $p 2>/dev/null; }; done
}
trap cleanup EXIT
fails=0; n=0
ok() { n=$((n + 1)); echo "ok $n - $1"; }
bad() { n=$((n + 1)); fails=$((fails + 1)); echo "NOT OK $n - $1"; }
expect() { if eval "$2"; then ok "$1"; else bad "$1   [$2]"; fi; }
waitfor() { # DESC CMD SECONDS
    local i=0; while ! eval "$2"; do sleep 0.2; i=$((i + 1)); [ $i -lt $(($3 * 5)) ] || { bad "$1 (not within $3 s: $2)"; return 1; }; done; ok "$1"
}
nslots() { ls "$VICTORY_HEAVY_DIR/slots" 2>/dev/null | grep -vc '\.tmp\.'; }
slotfield() { sed -n "s/^$2=//p" "$VICTORY_HEAVY_DIR/slots/$1"; }
# max of the running sum of the "+ UNITS" / "- UNITS" lines of a file, and how many starts
maxunits() { awk '$1=="+" {s += $2; if (s > m) m = s} $1=="-" {s -= $2} END {print m + 0}' "$1"; }
starts() { grep -c '^+' "$1"; }

echo "== environment, exit status, slot file, nice"
out=$(bash "$H" --jobs 3 --label envtest -- bash -c 'echo "J=$CARGO_BUILD_JOBS R=$RAYON_NUM_THREADS M=$MAKEFLAGS H=$VICTORY_HEAVY_JOBS S=$VICTORY_HEAVY_SLOT N=$(ps -o nice= -p $$ | tr -d " ")"; cat "$VICTORY_HEAVY_DIR/slots/$VICTORY_HEAVY_SLOT"; exit 7' 2>"$T/env.err"); rc=$?
expect "exit status of the command is passed on" '[ $rc = 7 ]'
expect "CARGO_BUILD_JOBS RAYON_NUM_THREADS MAKEFLAGS VICTORY_HEAVY_JOBS are K" 'echo "$out" | grep -q "^J=3 R=3 M=-j3 H=3 S="'
expect "VICTORY_HEAVY_SLOT is set" 'echo "$out" | grep -q " S=[0-9][0-9.]*-[0-9]* "'
expect "the command is niced to 19" 'echo "$out" | grep -q " N=\(19\|20\)$"'
expect "the slot file has id owner jobs label start cwd (pid and pgid are filled in a moment later)" 'echo "$out" | grep -q "^owner=[0-9]" && echo "$out" | grep -q "^jobs=3" && echo "$out" | grep -q "^label=envtest" && echo "$out" | grep -q "^start=20" && echo "$out" | grep -q "^cwd="'
expect "the slot is gone afterwards" '[ "$(nslots)" = 0 ]'
rc=0; bash "$H" -- /nonexistent/cmd 2>/dev/null || rc=$?
expect "a missing command: 127, no slot left" '[ $rc = 127 ] && [ "$(nslots)" = 0 ]'

echo "== the budget is never exceeded"
: >"$T/ev1"
job='echo "+ $1" >>"$2"; sleep 1.2; echo "- $1" >>"$2"'
for i in 1 2 3 4 5 6 7 8; do bash "$H" --jobs 1 --label b$i -- bash -c "$job" _ 1 "$T/ev1" 2>/dev/null & done; wait
expect "8 jobs of 1 unit: all ran" '[ "$(starts "$T/ev1")" = 8 ]'
expect "never more than 4 units at once (and 4 were reached)" '[ "$(maxunits "$T/ev1")" = 4 ]'
: >"$T/ev2"
bash "$H" --jobs 3 -- bash -c "$job" _ 3 "$T/ev2" 2>/dev/null & sleep 0.4
bash "$H" --jobs 2 -- bash -c "$job" _ 2 "$T/ev2" 2>/dev/null & sleep 0.2
bash "$H" --jobs 1 -- bash -c "$job" _ 1 "$T/ev2" 2>/dev/null & wait
expect "3+2+1 units: 3 and 2 never together (max 4), all ran" '[ "$(starts "$T/ev2")" = 3 ] && [ "$(maxunits "$T/ev2")" -le 4 ]'
out=$(bash "$H" --jobs 9 -- bash -c 'echo $VICTORY_HEAVY_JOBS' 2>"$T/big.err")
expect "--jobs above the whole budget runs with the budget" '[ "$out" = 4 ] && grep -q "more than the whole budget" "$T/big.err"'
out=$(VICTORY_HEAVY_BUDGET=2 bash "$H" --jobs 4 -- bash -c 'echo "$VICTORY_HEAVY_JOBS"' 2>/dev/null)
expect "VICTORY_HEAVY_BUDGET changes the budget" '[ "$out" = 2 ]'
out=$(env -u VICTORY_HEAVY_BUDGET bash -c ". $H; hv_base_budget")
expect "default budget = number of cores" '[ "$out" = "$(sysctl -n hw.ncpu)" ]'

echo "== queue: messages, --timeout, --run-timeout"
bash "$H" --jobs 4 --label full -- sleep $((TOK + 1)) 2>/dev/null & bgpids+=($!)
waitfor "a job fills the budget" '[ "$(nslots)" = 1 ]' 5
t0=$SECONDS; rc=0
bash "$H" --jobs 1 --label waiter --timeout 2 -- true 2>"$T/q.err" || rc=$?
expect "--timeout: gives up with exit 4 after about 2 s" '[ $rc = 4 ] && [ $((SECONDS - t0)) -ge 2 ] && [ $((SECONDS - t0)) -lt 8 ]'
expect "the queue says where it stands" 'grep -q "queued (place 1; wants 1 of budget 4" "$T/q.err"'
expect "a waiter that gave up leaves no queue entry" '[ -z "$(ls "$VICTORY_HEAVY_DIR/queue")" ]'
kill -TERM "${bgpids[0]}"; wait "${bgpids[0]}" 2>/dev/null; bgpids=()
waitfor "the full job's TERM removed its slot and its sleep" '[ "$(nslots)" = 0 ] && [ -z "$(mine 1)" ]' 8
t0=$SECONDS; rc=0
bash "$H" --run-timeout 2 -- sleep $((TOK + 2)) 2>"$T/rt.err" || rc=$?
expect "--run-timeout: exit 124 after about 2 s, sleep gone, slot gone" '[ $rc = 124 ] && [ $((SECONDS - t0)) -ge 2 ] && [ $((SECONDS - t0)) -lt 12 ] && [ -z "$(mine 2)" ] && [ "$(nslots)" = 0 ]'

echo "== dead slots are reclaimed, live ones count"
mkdir -p "$VICTORY_HEAVY_DIR/slots"
sleep 0.1 & dead=$!; wait $dead
writeslot() { # ID OWNER OWNERSTART JOBS
    printf 'id=%s\nowner=%s\nownerstart=%s\npid=%s\npidstart=%s\npgid=\njobs=%s\nlabel=fake\ncwd=/\nstart=2026-11-28T00:00:00\nstartts=1\nstate=running\ncmd=fake\n' "$1" "$2" "$3" "" "" "$4" >"$VICTORY_HEAVY_DIR/slots/$1"
}
writeslot 0000000001.000000-1 "$dead" "Sat Nov 28 00:00:00 2026" 4
t0=$SECONDS
bash "$H" --jobs 4 --timeout 5 -- true 2>/dev/null; rc=$?
expect "a slot of a dead process (all 4 units) does not block, and is removed" '[ $rc = 0 ] && [ $((SECONDS - t0)) -lt 4 ] && [ ! -e "$VICTORY_HEAVY_DIR/slots/0000000001.000000-1" ]'
writeslot 0000000002.000000-2 "$$" "$(gl_pidstart $$)" 4
rc=0; bash "$H" --jobs 1 --timeout 2 -- true 2>/dev/null || rc=$?
expect "a slot of a live process counts (4 of 4 used: exit 4)" '[ $rc = 4 ] && [ -e "$VICTORY_HEAVY_DIR/slots/0000000002.000000-2" ]'
bash "$H" status >"$T/status.out" 2>&1
expect "status lists the slot, the budget and the queue" 'grep -q "budget 4 of 4" "$T/status.out" && grep -q "fake" "$T/status.out" && grep -q "^queue:" "$T/status.out"'
rm -f "$VICTORY_HEAVY_DIR/slots/0000000002.000000-2"
# a pid that was reused: same pid, other start time = dead
writeslot 0000000003.000000-3 "$$" "Mon Jan  1 00:00:00 2001" 4
bash "$H" --jobs 4 --timeout 5 -- true 2>/dev/null; rc=$?
expect "a reused pid (other start time) is a dead slot" '[ $rc = 0 ]'

echo "== nested: a process in a slot does not take a second one"
out=$(bash "$H" --jobs 4 --label outer -- bash -c 'o=$VICTORY_HEAVY_SLOT; n1=$(ls "$VICTORY_HEAVY_DIR/slots" | wc -l | tr -d " "); inner=$(bash "'"$H"'" --jobs 4 --label inner -- bash -c "echo \$VICTORY_HEAVY_SLOT:\$VICTORY_HEAVY_JOBS"); n2=$(ls "$VICTORY_HEAVY_DIR/slots" | wc -l | tr -d " "); echo "$o $inner $n1 $n2"' 2>/dev/null)
expect "nested heavy.sh (4 units inside a 4-unit slot) runs at once: no deadlock, same slot, still 1 slot file" '(set -- $out; [ "$2" = "$1:4" ] && [ "$3" = 1 ] && [ "$4" = 1 ])'

echo "== a game starts: only the first job goes on, the others are stopped; the lock is released: all continue"
# (job 2 is started with SIGINT at its default: background jobs of a script ignore it otherwise, a Ctrl-C reaches a foreground one)
for i in 1 2 3; do
    if [ $i = 2 ]; then perl -e '$SIG{INT} = "DEFAULT"; exec @ARGV' bash "$H" --jobs 1 --label g$i -- sleep $((TOK + i)) 2>"$T/g$i.err" & bgpids+=($!)
    else bash "$H" --jobs 1 --label g$i -- sleep $((TOK + i)) 2>"$T/g$i.err" & bgpids+=($!); fi
    sleep 0.4
done
waitfor "three jobs run" '[ "$(nslots)" = 3 ] && [ -n "$(mine 1)" ] && [ -n "$(mine 2)" ] && [ -n "$(mine 3)" ]' 8
P1=$(mine 1); P2=$(mine 2); P3=$(mine 3)
expect "no game: none stopped" '! state $P1 | grep -q T && ! state $P2 | grep -q T && ! state $P3 | grep -q T'
gl_acquire test x 5 >/dev/null 2>&1
expect "the fake game holds the fake lock (and it is not the real one)" 'gl_state; [ $? = 0 ] && [ "$GL_LOCK" = "$VICTORY_GAME_LOCK" ]'
waitfor "the later two are stopped (T), at most 6 s" 'state $P2 | grep -q T && state $P3 | grep -q T' 8
expect "the first (earliest) job keeps running" '! state $P1 | grep -q T'
expect "the slot files carry pid and pgid of the command" 'for f in "$VICTORY_HEAVY_DIR"/slots/*; do grep -q "^pid=[0-9]" "$f" && grep -q "^pgid=[0-9]" "$f" || exit 1; done' 
expect "their slot files say paused, the first says running" '[ "$(cat "$VICTORY_HEAVY_DIR"/slots/* | grep -c "^state=paused")" = 2 ] && [ "$(cat "$VICTORY_HEAVY_DIR"/slots/* | grep -c "^state=running")" = 1 ]'
bash "$H" status >"$T/status2.out" 2>&1
expect "status shows paused, the budget 1 and the lock" 'grep -q "paused" "$T/status2.out" && grep -q "budget 1 of 4" "$T/status2.out" && grep -q "game lock: HELD" "$T/status2.out"'
rc=0; bash "$H" --jobs 1 --timeout 2 -- true 2>"$T/g.err" || rc=$?
expect "a new job in the meantime queues (budget 1, 3 in use)" '[ $rc = 4 ] && grep -q "a game is running" "$T/g.err"'
gl_release
waitfor "the lock is released: all three run again (no T)" '! state $P2 | grep -q T && ! state $P3 | grep -q T && ! state $P1 | grep -q T' 8
expect "slot files running again" '! grep -l "state=paused" "$VICTORY_HEAVY_DIR"/slots/* >/dev/null 2>&1'

echo "== interrupt while paused: slots cleaned, nothing left stopped"
gl_acquire test x 5 >/dev/null 2>&1
waitfor "stopped again" 'state $P2 | grep -q T && state $P3 | grep -q T' 8
kill -TERM "${bgpids[2]}"            # job 3 (paused): TERM
kill -INT "${bgpids[1]}"             # job 2 (paused): INT
waitfor "paused jobs killed by TERM/INT are gone (continued, then ended)" '[ -z "$(mine 2)" ] && [ -z "$(mine 3)" ]' 10
waitfor "their slots are removed" '[ "$(nslots)" = 1 ]' 5
expect "no process of the test is left, stopped or not" ' [ -z "$(mine 2)$(mine 3)" ]'
expect "the first job is still running" '[ -n "$(mine 1)" ] && ! state $P1 | grep -q T'
gl_release
kill -TERM "${bgpids[0]}"
waitfor "last job ended: no slot, no sleep" '[ "$(nslots)" = 0 ] && [ -z "$(mine 1)" ]' 10
expect "no queue entry left" '[ -z "$(ls "$VICTORY_HEAVY_DIR/queue" 2>/dev/null)" ]'

echo "== the earliest job ends while a game runs: the next one takes over (and continues)"
bgpids=()
for i in 1 2; do bash "$H" --jobs 1 --label h$i -- sleep $((TOK + i)) 2>/dev/null & bgpids+=($!); sleep 0.4; done
waitfor "two jobs run" '[ -n "$(mine 1)" ] && [ -n "$(mine 2)" ]' 8
P1=$(mine 1); P2=$(mine 2)
gl_acquire test x 5 >/dev/null 2>&1
waitfor "second is stopped" 'state $P2 | grep -q T' 8
kill -TERM "${bgpids[0]}"
waitfor "first ended; the second now is the first job and continues (the game still runs)" '[ -z "$(mine 1)" ] && ! state $P2 | grep -q T' 10
gl_release
kill -TERM "${bgpids[1]}"
waitfor "all ended, no slot" '[ "$(nslots)" = 0 ] && [ -z "$(mine 2)" ]' 10

echo "== a heavy.sh killed with SIGKILL while its job is paused (no cleanup possible): nothing stays stopped, its slot is reclaimed"
bgpids=()
for i in 1 2; do bash "$H" --jobs 1 --label o$i -- sleep $((TOK + i)) 2>/dev/null & bgpids+=($!); sleep 0.4; done
waitfor "two jobs run" '[ -n "$(mine 1)" ] && [ -n "$(mine 2)" ]' 8
P1=$(mine 1); P2=$(mine 2)
gl_acquire test x 5 >/dev/null 2>&1
waitfor "second is stopped" 'state $P2 | grep -q T' 8
kill -KILL "${bgpids[1]}" 2>/dev/null
gl_release
# (the kernel hangs up and continues a stopped process group whose parent died: orphaned process group rule; reaping continues it too)
waitfor "the orphan is not left stopped (hung up by the kernel, or continued by the next reaping)" '[ -z "$(mine 2)" ] || ! state $(mine 2) | grep -q T' 8
bash "$H" --jobs 1 --timeout 5 -- true 2>/dev/null
expect "the next job reaps the dead heavy.sh's slot (only the first job's slot is left)" '[ "$(nslots)" = 1 ] || { [ -n "$(mine 2)" ] && [ "$(nslots)" = 2 ]; }'
kill -KILL "$(mine 2)" 2>/dev/null
kill -TERM "${bgpids[0]}"
waitfor "orphan dead, first ended: no slot left" 'bash "$H" --jobs 4 --timeout 5 -- true 2>/dev/null; [ "$(nslots)" = 0 ]' 15

echo "== usage and syntax"
expect "no command: usage, exit 2" 'bash "$H" 2>/dev/null; [ $? = 2 ]'
expect "bad --jobs: exit 2" 'bash "$H" --jobs x -- true 2>/dev/null; [ $? = 2 ]'
expect "bash -n heavy.sh heavy-guard.sh" 'bash -n "$H" && bash -n "$ROOT/tools/mac/heavy-guard.sh"'
expect "no state lock left behind" '[ ! -e "$VICTORY_HEAVY_DIR/lock.d" ]'
echo
echo "$((n - fails)) of $n passed (logs in $T)"
[ "$fails" = 0 ]
