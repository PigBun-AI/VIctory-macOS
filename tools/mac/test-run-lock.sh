#!/bin/bash
# Tests of "the game lock first, then any side effect" in tools/mac/run.sh, with a FAKE Wine (no game, no GPU, nothing of the real prefix,
# data directories or lock is touched): two run.sh instances are started at once with --new-game on the SAME data directory.
#   (a) the second one touches nothing (data directory, prefix, out folder, wineserver) before it has the lock: the first one's marker file
#       in data/ stays for the whole of its run, and is replaced only when the second one has the lock
#   (b) the two runs are serial (start/end events in order), both succeed
#   (c) an interrupted run lets the lock go (holder, and a waiter in the queue), the game is stopped, the data are not touched by a waiter
# also: --print-env takes no lock and writes nothing; --allow-concurrent (approved) does not take the lock and does not wait.
# VICTORY_GAME_LOCK, GTA6_ROOT, the Wine directory and VICTORY_MAC_EXE_PATTERN all point into a temporary directory. Takes about 40 s.
#   tools/mac/test-run-lock.sh [TMPDIR]        (TMPDIR is kept: it holds every instance's logs)
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
T="${1:-$(mktemp -d "${TMPDIR:-/tmp}/run-lock-test.XXXXXX")}"
RUN="${TEST_RUN_SH:-$ROOT/tools/mac/run.sh}"      # (a knob to point the test at another copy of run.sh, for example one with the lock moved)
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
mkdir -p "$ROOT/target"; [ -x "$ROOT/target/dxc-d3dmetal" ] || { printf '#!/bin/sh\n' >"$ROOT/target/dxc-d3dmetal"; chmod +x "$ROOT/target/dxc-d3dmetal"; }

export GTA6_ROOT="$T/root" VICTORY_MAC_WINE_DIR="$T/wine" VICTORY_GAME_DIR="$T/game" VICTORY_GAME_LOCK="$T/game.lock" VICTORY_METAL_OUT="$T/stage"
export VICTORY_MAC_EXE_PATTERN='victory-fake-lock-test\.exe run --execute' FAKE_SERVER_CALLS="$T/server.calls" FAKE_EVENTS="$T/events" GL_POLL=0.2 GL_REPORT=1
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
    FAKE_ID=$id FAKE_HOLD=$hold bash "$RUN" --exe "$T/victory-fake-lock-test.exe" --prefix "$T/prefix-$id" --out "$T/out-$id" \
        --no-keys --input none --timeout 60 "$@" >"$T/$id.console" 2>"$T/$id.console.err" &
    eval "pid_$id=$!"
}
wait_for() { local i; for i in $(seq 1 ${2:-100}); do eval "$1" && return 0; sleep 0.1; done; return 1; }   # wait_for CONDITION [x 0.1 s]
event() { grep -q -x -F "$1" "$FAKE_EVENTS"; }
lock_free() { ! [ -d "$VICTORY_GAME_LOCK" ]; }
queue_empty() { [ -z "$(ls "$VICTORY_GAME_LOCK.queue" 2>/dev/null)" ]; }
no_fake_left() { ! pgrep -f 'victory-fake-lock-test\.exe run --execute' >/dev/null; }
tree() { (cd "$1" 2>/dev/null && find . | LC_ALL=C sort | tr '\n' ' '); }

echo "== two --new-game runs started at once on one data directory"
start A 6 --new-game
wait_for 'event "A start"' 100 || bad "A never started"
start B 3 --new-game
sleep 2.5      # B has been queued for a while; A is half way through
expect "(a) A holds the lock, B waits for it" "[ \"\$(cat $VICTORY_GAME_LOCK/pid)\" = \$pid_A ] && grep -q 'game-lock: waiting' $T/B.console.err"
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
echo
echo "$((n - fails)) of $n passed (logs in $T)"
[ "$fails" = 0 ]
