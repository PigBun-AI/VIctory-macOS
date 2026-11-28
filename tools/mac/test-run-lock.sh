#!/bin/bash
# Tests of "the game lock first, then any side effect" in tools/mac/run.sh, with a FAKE Wine (no game, no GPU, nothing of the real prefix,
# data directories or lock is touched): two run.sh instances are started at once with --new-game on the SAME data directory.
#   (a) the second one touches nothing (data directory, prefix, out folder, wineserver) before it has the lock: the first one's marker file
#       in data/ stays for the whole of its run, and is replaced only when the second one has the lock
#   (b) the two runs are serial (start/end events in order), both succeed
#   (c) an interrupted run lets the lock go (holder, and a waiter in the queue), the game is stopped, the data are not touched by a waiter
# also: --print-env takes no lock and writes nothing; --allow-concurrent (approved) does not take the lock and does not wait.
# The heavy-guard is run.sh's child (the last section): (a) it runs once run.sh holds the lock and its pid is in the lock directory, (b) it ends with a
# normal end of run.sh and continues what it stopped (a fake heavy process), (c) a run.sh killed with SIGKILL: the guard ends within 15 s and
# continues the fake heavy process, nothing is left, (d) two run.sh queued for the lock: one guard at any time (and none when one runs by hand),
# (e) --allow-concurrent starts none. The guard only sees the descendants of this script (VICTORY_GUARD_SCOPE_PID), load 70 injected.
# VICTORY_GAME_LOCK, GTA6_ROOT, the Wine directory, VICTORY_HEAVY_DIR and VICTORY_MAC_EXE_PATTERN all point into a temporary directory. Takes about 80 s.
#   tools/mac/test-run-lock.sh [TMPDIR]        (TMPDIR is kept: it holds every instance's logs)
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
T="${1:-$(mktemp -d "${TMPDIR:-/tmp}/run-lock-test.XXXXXX")}"
RUN="${TEST_RUN_SH:-$ROOT/tools/mac/run.sh}"      # (a knob to point the test at another copy of run.sh, for example one with the lock moved)
mkdir -p "$T"; T="$(cd "$T" && pwd)"
mkdir -p "$T/wine/bin" "$T/game" "$T/root/data/livetr/data/title1/storage" "$T/root/data/livetr/cache" "$T/root/data/livetr/dxc" \
         "$T/stage/front" "$T/stage/x86_64-windows"
echo seed >"$T/root/data/livetr/data/title1/seed.txt"; echo warm >"$T/root/data/livetr/cache/warm.txt"
for n in d3d12 dxgi; do echo "stage $n" >"$T/stage/front/$n.dll"; done; echo bridge >"$T/stage/x86_64-windows/victory_metal_bridge.dll"
# the fake Wine: FAKE_ID names the instance, FAKE_HOLD how many seconds the "title" runs (it also ends when it is stopped, TERM)
cat >"$T/wine/bin/wine" <<'EOF'
#!/bin/bash
trap 'echo "$FAKE_ID term" >>"$FAKE_EVENTS"; exit 143' TERM
d="$VICTORY_DATA_DIR"
echo "$FAKE_ID start" >>"$FAKE_EVENTS"
[ -e "$d/title1/seed.txt" ] && echo "$FAKE_ID saw-seed" >>"$FAKE_EVENTS"
for other in A B C D; do [ "$other" != "$FAKE_ID" ] && [ -e "$d/marker-$other" ] && echo "$FAKE_ID saw-marker-$other" >>"$FAKE_EVENTS"; done
: >"$d/marker-$FAKE_ID"
end=$((SECONDS + ${FAKE_HOLD:-3}))
while [ $SECONDS -lt $end ]; do
    [ -e "$d/marker-$FAKE_ID" ] || { echo "$FAKE_ID marker-lost" >>"$FAKE_EVENTS"; break; }
    sleep 0.2
done
echo "$FAKE_ID end" >>"$FAKE_EVENTS"
EOF
cat >"$T/wine/bin/wineserver" <<'EOF'
#!/bin/bash
echo "wineserver $* prefix=${WINEPREFIX:-unset}" >>"$FAKE_SERVER_CALLS"
exit 0
EOF
chmod +x "$T/wine/bin/wine" "$T/wine/bin/wineserver"
echo fake >"$T/victory-fake-lock-test.exe"
# the pattern of run.sh's "another victory.exe is running" check (and of no_fake_left): the fake exe in THIS test's own directory, so neither a real
# game nor another copy of this test running at the same time can match it
FAKE_PAT="${T//./\\.}/victory-fake-lock-test\\.exe run --execute"
mkdir -p "$ROOT/target"; [ -x "$ROOT/target/dxc-d3dmetal" ] || { printf '#!/bin/sh\n' >"$ROOT/target/dxc-d3dmetal"; chmod +x "$ROOT/target/dxc-d3dmetal"; }

export GTA6_ROOT="$T/root" VICTORY_MAC_WINE_DIR="$T/wine" VICTORY_GAME_DIR="$T/game" VICTORY_GAME_LOCK="$T/game.lock" VICTORY_METAL_OUT="$T/stage"
export VICTORY_MAC_EXE_PATTERN="$FAKE_PAT" FAKE_SERVER_CALLS="$T/server.calls" FAKE_EVENTS="$T/events" GL_POLL=0.2 GL_REPORT=1
export VICTORY_HEAVY_DIR="$T/heavy" VICTORY_GUARD_SCOPE_PID=$$ VICTORY_GUARD_LOAD=70 VICTORY_GUARD_INTERVAL=2 VICTORY_GUARD_PATTERN='^x-no-guard-by-hand$'
unset VICTORY_CAPTURE VICTORY_RECORD VICTORY_CAPTURE_FRAMES VICTORY_LOG_WORLD VICTORY_DATA_DIR VICTORY_CACHE_DIR VICTORY_RENDER_SCALE VICTORY_UPSCALER VICTORY_OUTPUT
: >"$FAKE_EVENTS"; : >"$FAKE_SERVER_CALLS"
fails=0; n=0
ok() { n=$((n + 1)); echo "ok $n - $1"; }
bad() { n=$((n + 1)); fails=$((fails + 1)); echo "NOT OK $n - $1"; }
expect() { if eval "$2"; then ok "$1"; else bad "$1   [$2]"; fi; }
BASE="$T/root/data/livetr-perf-metal"; DATA="$BASE/data"
mkprefix() { mkdir -p "$T/prefix-$1/drive_c/windows/system32"; echo "original $1" >"$T/prefix-$1/drive_c/windows/system32/d3d12.dll"; echo "original $1" >"$T/prefix-$1/drive_c/windows/system32/dxgi.dll"; }
# start ID HOLD [run.sh options]: one run.sh in the background (its pid in $pid_ID), its own prefix and out folder
start() {
    local id=$1 hold=$2; shift 2
    [ -d "$T/prefix-$id" ] || mkprefix "$id"
    FAKE_ID=$id FAKE_HOLD=$hold bash "$RUN" --purpose "freq: mac.test.$id" --exe "$T/victory-fake-lock-test.exe" --prefix "$T/prefix-$id" --out "$T/out-$id" \
        --no-keys --input none --timeout 60 "$@" >"$T/$id.console" 2>"$T/$id.console.err" &
    eval "pid_$id=$!"
}
wait_for() { local i; for i in $(seq 1 ${2:-100}); do eval "$1" && return 0; sleep 0.1; done; return 1; }   # wait_for CONDITION [x 0.1 s]
event() { grep -q -x -F "$1" "$FAKE_EVENTS"; }
lock_free() { ! [ -d "$VICTORY_GAME_LOCK" ]; }
queue_empty() { [ -z "$(ls "$VICTORY_GAME_LOCK.queue" 2>/dev/null)" ]; }
no_fake_left() { ! pgrep -f "$FAKE_PAT" >/dev/null; }
tree() { (cd "$1" 2>/dev/null && find . | LC_ALL=C sort | tr '\n' ' '); }

echo "== two --new-game runs started at once on one data directory"
start A 6 --new-game
wait_for 'event "A start"' 100 || bad "A never started"
start B 3 --new-game
sleep 2.5      # B has been queued for a while; A is half way through
expect "(a) A holds the lock, B waits for it" "[ \"\$(cat $VICTORY_GAME_LOCK/pid)\" = \$pid_A ] && grep -q 'game-lock: waiting' $T/B.console.err"
expect "(a) game-lock.sh status shows A's purpose, and B's in the queue" "bash $ROOT/tools/mac/game-lock.sh status | grep -q 'purpose: freq: mac.test.A' && bash $ROOT/tools/mac/game-lock.sh status | grep -q 'purpose \"freq: mac.test.B\"'"
expect "(a) A's marker is still in the data directory while B waits" "[ -e $DATA/marker-A ]"
expect "(a) B has not touched the data directory: no clone, no replacement (the seed's files and A's marker only)" "[ \"\$(tree $DATA)\" = \"\$(echo '. ./marker-A ./title1 ./title1/seed.txt ./title1/storage ')\" ] && [ ! -e $BASE/data.new ] && ! ls $BASE | grep -q part"
expect "(a) B has not touched its prefix: no dll deployed, no .metal-orig" "[ \"\$(cat $T/prefix-B/drive_c/windows/system32/d3d12.dll)\" = 'original B' ] && [ ! -e $T/prefix-B/drive_c/windows/system32/d3d12.dll.metal-orig ] && [ ! -e $T/prefix-B/drive_c/windows/system32/victory_metal_bridge.dll ]"
expect "(a) B has not made its out folder, nor started a game, nor stopped a wineserver" "[ ! -e $T/out-B ] && ! event 'B start' && ! grep -q 'prefix=$T/prefix-B' $T/server.calls"
expect "(a) A's own prefix got the stage's dlls (the deploy is A's)" "[ \"\$(cat $T/prefix-A/drive_c/windows/system32/d3d12.dll)\" = 'stage d3d12' ] && [ -e $T/prefix-A/drive_c/windows/system32/d3d12.dll.metal-orig ]"
# --print-env while A holds the lock and B queues: no lock, no queue entry, no change
before="$(tree "$T/root/data"; tree "$T/prefix-C" 2>&1; ls "$VICTORY_GAME_LOCK.queue" | wc -l)"
bash "$RUN" --print-env --new-game --prefix "$T/prefix-C" --out "$T/out-C" >"$T/printenv.out" 2>"$T/printenv.err"; rc=$?
after="$(tree "$T/root/data"; tree "$T/prefix-C" 2>&1; ls "$VICTORY_GAME_LOCK.queue" | wc -l)"
expect "--print-env: exit 0 at once, not queued, nothing written (no prefix, out folder or data change)" "[ $rc = 0 ] && [ \"\$before\" = \"\$after\" ] && [ ! -e $T/out-C ] && [ ! -e $T/prefix-C ] && grep -q '^# command' $T/printenv.out"
wait "$pid_A"; rc_A=$?
wait_for 'event "B end"' 150
wait "$pid_B"; rc_B=$?
expect "(b) both runs succeeded" "[ $rc_A = 0 ] && [ $rc_B = 0 ]"
expect "(b) serial: A start, A end, then B start, B end" "[ \"\$(grep -E '^[AB] (start|end)\$' $FAKE_EVENTS | tr '\\n' ',')\" = 'A start,A end,B start,B end,' ]"
expect "(a) A kept its marker for the whole of its run (never lost)" "! event 'A marker-lost' && ! event 'B marker-lost'"
expect "(a) B's --new-game replaced A's data: B saw the seed, did not see A's marker; only B's marker is left" "event 'B saw-seed' && ! event 'B saw-marker-A' && [ -e $DATA/marker-B ] && [ ! -e $DATA/marker-A ]"
expect "(b) the warm cache of the directory stayed (--new-game keeps cache/)" "[ -e $BASE/cache/warm.txt ]"
expect "(b) B's prefix was deployed only by B, after A (its dll is the stage's now)" "[ \"\$(cat $T/prefix-B/drive_c/windows/system32/d3d12.dll)\" = 'stage d3d12' ]"
expect "(b) lock, queue and fake titles all gone" "lock_free && queue_empty && no_fake_left"
expect "(b) no half-made clone left behind" "! ls $BASE | grep -q 'part\\|data.new'"

echo "== (c) a run interrupted while it holds the lock"
: >"$FAKE_EVENTS"; : >"$T/server.calls"
start C 60 --new-game
wait_for 'event "C start"' 100 || bad "C never started"
expect "(c) C holds the lock" "[ \"\$(cat $VICTORY_GAME_LOCK/pid)\" = \$pid_C ]"
kill -TERM "$pid_C"; wait "$pid_C"; rc_C=$?
expect "(c) C exited 143 on TERM" "[ $rc_C = 143 ]"
expect "(c) the game was stopped with it, the lock is free" "event 'C term' && no_fake_left && lock_free && queue_empty"
expect "(c) wineserver -k only for C's prefix" "grep -q 'wineserver -k prefix=$T/prefix-C' $T/server.calls && ! grep -v 'prefix=$T/prefix-C' $T/server.calls | grep -q ."

echo "== (c) a run interrupted while it waits: the holder keeps the lock, the data stay"
: >"$FAKE_EVENTS"
start A 9 --new-game
wait_for 'event "A start"' 100 || bad "A never started"
start D 3 --new-game
sleep 2
expect "(c) D is in the queue" "[ -n \"\$(ls $VICTORY_GAME_LOCK.queue 2>/dev/null)\" ]"
kill -TERM "$pid_D"; wait "$pid_D"; rc_D=$?
expect "(c) D exited 143, its queue entry is gone, A still holds the lock" "[ $rc_D = 143 ] && queue_empty && [ \"\$(cat $VICTORY_GAME_LOCK/pid)\" = \$pid_A ]"
expect "(c) D touched nothing: A's marker and data are as A left them, D has no out folder" "[ -e $DATA/marker-A ] && [ ! -e $T/out-D ] && ! event 'D start'"
# --allow-concurrent (approved): no lock, no waiting; the data directories are then the caller's business (each its own)
start G 2 --allow-concurrent --i-have-approval --data "$T/gdata" --cache "$T/gcache"
mkdir -p "$T/gdata" "$T/gcache"
wait_for 'event "G end"' 100
wait "$pid_G"; rc_G=$?
expect "--allow-concurrent: G ran while A held the lock (overlap), exit 0, the lock stayed A's" "[ $rc_G = 0 ] && event 'G start' && event 'G end' && ! event 'A end' && [ \"\$(cat $VICTORY_GAME_LOCK/pid)\" = \$pid_A ] && grep -q 'NOT taking the game lock' $T/G.console.err"
wait "$pid_A"; rc_A=$?
expect "after all: A succeeded, lock free, queue empty, nothing left" "[ $rc_A = 0 ] && lock_free && queue_empty && no_fake_left"

echo "== the heavy-guard is run.sh's child"
GL="$VICTORY_GAME_LOCK"; HG="$ROOT/tools/mac/heavy-guard.sh"
bgp=()
guards_of() { pgrep -f "heavy-guard\\.sh run --parent $1\$"; }   # the guards whose parent is that pid
fake_heavy() { ( exec -a victory-census sleep "$1" ) & bgp+=($!); eval "FH=$!"; }
st_of() { ps -o stat= -p "$1" 2>/dev/null | tr -d ' '; }
stopped() { st_of "$1" | grep -q T; }
guard_pid_in_lock() { sed -n 1p "$GL/guard.pid" 2>/dev/null; }
trap 'for p in ${bgp[@]+"${bgp[@]}"}; do kill -CONT "$p" 2>/dev/null; kill -KILL "$p" 2>/dev/null; done' EXIT
: >"$FAKE_EVENTS"; : >"$T/server.calls"
tok=$((500000 + $$))

echo "-- (a) run.sh has the lock: the guard runs, its pid is in the lock directory"
fake_heavy $((tok + 1)); FH1=$FH
start A 5 --new-game
wait_for 'event "A start"' 100 || bad "A never started"
wait_for '[ -n "$(guard_pid_in_lock)" ]' 50
GA=$(guard_pid_in_lock)
expect "(a) the lock directory has the guard's pid and start time, the guard is alive and is a child of this run.sh (run --parent <run.sh>)" "[ -n \"\$GA\" ] && kill -0 \$GA && [ -s $GL/guard.pidstart ] && [ \"\$(ps -o ppid= -p \$GA | tr -d ' ')\" = \$pid_A ] && ps -o args= -p \$GA | grep -q 'heavy-guard.sh run --parent '\$pid_A"
expect "(a) it is the only guard of this run, and it says it is running" "[ \"\$(guards_of \$pid_A | wc -l | tr -d ' ')\" = 1 ] && bash $HG running"
wait_for 'stopped $FH1' 80
expect "(a) at work: the fake heavy process (outside the run) is stopped at load 70" 'stopped $FH1'
expect "(a) game-lock.sh status still works with the guard's files in the lock directory" "bash $ROOT/tools/mac/game-lock.sh status | grep -q 'game lock: HELD'"

echo "-- (b) run.sh ends normally: the guard ends and continues what it stopped"
wait "$pid_A"; rc_A=$?
expect "(b) run.sh exit 0, the guard is gone, its lock directory is gone" "[ $rc_A = 0 ] && [ -z \"\$(guards_of \$pid_A)\" ] && lock_free && ! kill -0 \$GA 2>/dev/null"
expect "(b) the fake heavy process got SIGCONT, the stopped list is empty, the log says SIGSTOP then SIGCONT" "! stopped \$FH1 && [ ! -s $VICTORY_HEAVY_DIR/guard.stopped ] && grep -q \"pid=\$FH1 .*action=SIGSTOP\" $VICTORY_HEAVY_DIR/guard.log && grep -q \"pid=\$FH1 .*action=SIGCONT\" $VICTORY_HEAVY_DIR/guard.log"
expect "(b) the guard's pid file is gone with it" "[ ! -e $VICTORY_HEAVY_DIR/guard.pid ]"
kill -KILL $FH1 2>/dev/null

echo "-- (c) run.sh killed with SIGKILL: the guard ends within 15 s, nothing is left stopped"
fake_heavy $((tok + 2)); FH2=$FH
: >"$FAKE_EVENTS"
start C 8 --new-game
wait_for 'event "C start"' 100 || bad "C never started"
wait_for '[ -n "$(guard_pid_in_lock)" ]' 50
GC=$(guard_pid_in_lock)
wait_for 'stopped $FH2' 80
expect "(c) before the kill: the guard of C runs and has stopped the fake heavy process" "kill -0 \$GC && stopped \$FH2"
t0=$SECONDS
kill -KILL "$pid_C"; wait "$pid_C" 2>/dev/null
wait_for '! kill -0 $GC 2>/dev/null' 150
secs=$((SECONDS - t0))
expect "(c) the guard has ended within 15 s of the kill (took $secs s)" "! kill -0 \$GC 2>/dev/null && [ $secs -le 15 ]"
expect "(c) the fake heavy process got SIGCONT, nothing is left stopped, the list is empty, the log says why it ended" "! stopped \$FH2 && [ ! -s $VICTORY_HEAVY_DIR/guard.stopped ] && grep -q 'parent $pid_C is gone' $VICTORY_HEAVY_DIR/guard.log && [ -z \"\$(guards_of $pid_C)\" ] && [ ! -e $VICTORY_HEAVY_DIR/guard.pid ]"
wait_for no_fake_left 150   # (the title of the killed run ends by itself after its 8 s)
kill -KILL $FH2 2>/dev/null
rm -rf "$GL" "$GL.queue"    # (the killed run's lock is stale; the next run would take it over, this is only tidy)

echo "-- (d) two run.sh queued for the lock: one guard at any time"
: >"$FAKE_EVENTS"
start A 6 --new-game
wait_for 'event "A start"' 100 || bad "A never started"
wait_for '[ -n "$(guard_pid_in_lock)" ]' 50
start B 3 --new-game
sleep 3
expect "(d) A holds the lock and has its guard; B queues and has none: exactly one guard of the two" "[ -n \"\$(ls $GL.queue 2>/dev/null)\" ] && [ \"\$(pgrep -f 'heavy-guard\\.sh run --parent ('\$pid_A'|'\$pid_B')\$' | wc -l | tr -d ' ')\" = 1 ] && [ -n \"\$(guards_of \$pid_A)\" ] && [ -z \"\$(guards_of \$pid_B)\" ]"
GA=$(guard_pid_in_lock)
wait "$pid_A"
wait_for 'event "B start"' 150
wait_for '[ -n "$(guard_pid_in_lock)" ]' 50
GB=$(guard_pid_in_lock)
expect "(d) when A has ended B has the lock and its own guard (a new one), A's is gone: one guard" "[ -n \"\$GB\" ] && [ \"\$GB\" != \"\$GA\" ] && ! kill -0 \$GA 2>/dev/null && [ \"\$(pgrep -f 'heavy-guard\\.sh run --parent ('\$pid_A'|'\$pid_B')\$' | wc -l | tr -d ' ')\" = 1 ] && [ -n \"\$(guards_of \$pid_B)\" ]"
wait "$pid_B"; rc_B=$?
expect "(d) B succeeded, no guard of A or B is left, lock free" "[ $rc_B = 0 ] && [ -z \"\$(guards_of \$pid_A; guards_of \$pid_B)\" ] && lock_free && queue_empty"
# a guard started by hand is running: run.sh does not start another
VICTORY_GUARD_LOAD=1 bash "$HG" run >/dev/null 2>&1 & MG=$!; bgp+=($MG)
wait_for 'bash $HG running' 30
: >"$FAKE_EVENTS"
start A 3 --new-game
wait_for 'event "A start"' 100 || bad "A never started"
expect "(d) a guard by hand runs: run.sh starts none (nothing in the lock directory, no child guard), says so" "[ -z \"\$(guard_pid_in_lock)\" ] && [ -z \"\$(guards_of \$pid_A)\" ] && grep -q 'a heavy-guard is already running' $T/A.console.err"
wait "$pid_A"
kill -TERM $MG; wait $MG 2>/dev/null

echo "-- (e) --allow-concurrent: no lock, no guard"
: >"$FAKE_EVENTS"
start A 6 --new-game
wait_for 'event "A start"' 100 || bad "A never started"
wait_for '[ -n "$(guard_pid_in_lock)" ]' 50
GA=$(guard_pid_in_lock)
start G 2 --allow-concurrent --i-have-approval --data "$T/gdata" --cache "$T/gcache"
wait_for 'event "G start"' 100 || bad "G never started"
expect "(e) G (concurrent) started no guard; the lock's guard is still A's, one guard in all" "[ -z \"\$(guards_of \$pid_G)\" ] && [ \"\$(guard_pid_in_lock)\" = \"\$GA\" ] && [ \"\$(pgrep -f 'heavy-guard\\.sh run --parent ('\$pid_A'|'\$pid_G')\$' | wc -l | tr -d ' ')\" = 1 ]"
wait "$pid_G"; wait "$pid_A"
: >"$FAKE_EVENTS"
start G 3 --allow-concurrent --i-have-approval --data "$T/gdata" --cache "$T/gcache"
wait_for 'event "G start"' 100 || bad "G never started"
sleep 1
expect "(e) G alone: no lock directory, no guard" "lock_free && [ -z \"\$(guards_of \$pid_G)\" ] && [ ! -e $VICTORY_HEAVY_DIR/guard.pid ]"
wait "$pid_G"
expect "(e) at the end: no guard left, no fake title, lock free" "[ -z \"\$(pgrep -f 'heavy-guard\\.sh run --parent')\" ] || pgrep -f 'heavy-guard\\.sh run --parent' | while read -r p; do kill -0 \$p; done; lock_free && queue_empty && no_fake_left"
echo
echo "$((n - fails)) of $n passed (logs in $T)"
[ "$fails" = 0 ]
