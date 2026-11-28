#!/bin/bash
# Tests of tools/mac/accept-batch.sh with a FAKE Wine (no game, no GPU; nothing of the real prefix, lock, data or the
# acceptance folder is touched): the stand-in `wine` plays a round the way the case needs (FAKE_MODE), writes the title's log
# lines, VICTORY_FRAMETIME_LOG and the back's dropstats/back.csv. run.sh is the real one, with a fake stage, prefix and seed.
# Takes about a minute.   tools/mac/test-accept-batch.sh [TMPDIR]
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
T="${1:-$(mktemp -d "${TMPDIR:-/tmp}/accept-batch-test.XXXXXX")}"
mkdir -p "$T/wine/bin" "$T/game" "$T/root/data/livetr/data/t1/saves" "$T/root/data/livetr/cache" "$T/root/data/livetr/dxc" \
         "$T/stage/front" "$T/stage/x86_64-windows" "$T/prefix/drive_c/windows/system32" "$T/accept"
: >"$T/stage/front/d3d12.dll"; : >"$T/stage/front/dxgi.dll"; : >"$T/stage/x86_64-windows/victory_metal_bridge.dll"
cat >"$T/wine/bin/wine" <<'EOF'
#!/bin/bash
trap 'exit 143' TERM
ts() { date -u +%Y-%m-%dT%H:%M:%S.000000Z; }
echo "fake title started"
mode="${FAKE_MODE:-ok}"
[ "$mode" = die ] && { sleep 1; echo "$(ts)  INFO fake: Unhandled exception c0000005" >&2; exit 3; }
echo "frame,frame_us,draws" >"$VICTORY_FRAMETIME_LOG"
mkdir -p "$VICTORY_METAL_DROPSTATS"
echo "frame,epoch_ms,draw,draw_indexed,mesh,indirect_draw,dispatch,indirect_dispatch,submitted_draw,submitted_dispatch,dropped_draw,dropped_dispatch,noop,sub_vs,sub_ms,sub_gs,sub_cs,sub_none,drop_vs,drop_ms,drop_gs,drop_cs,drop_none,reasons,pipelines,extras" >"$VICTORY_METAL_DROPSTATS/back.csv"
last=12; [ "$mode" = slow ] && last=4
base=${FAKE_BASE_US:-16000}
for p in $(seq 1 39); do echo "$p,$base,2" >>"$VICTORY_FRAMETIME_LOG"; done
sleep 1
for n in $(seq 1 $last); do
    p=$((39 + n)); w=$n
    if [ $n = 1 ]; then echo "$(ts)  INFO victory_gpu::mac::world: world frame 1 (input scripts and world-indexed captures count from here) present=$p draws=600" >&2
    else echo "$(ts)  INFO victory_gpu::mac::world: world frame $n present=$p" >&2; fi
    us=$base; [ $n = 7 ] && us=6000000
    echo "$p,$us,100" >>"$VICTORY_FRAMETIME_LOG"
    echo "$(ts)  INFO victory_gpu::mac::drop_stats: drop-stats present=$p world_frame=$w recorded=90+10 ran=${FAKE_RAN:-80} dropped=${FAKE_DROP:-20} notimpl[0x1c8=3 0x218=1] why[no_host=12 refused=8] kind[vs=15 mesh=5] psoran[]" >&2
    echo "$(ts)  INFO victory_gpu::mac::drop_stats: drop-stats top span=world n=$((n * 12)) op=draw kind=vs origin=graphics@0x50 why=compiling e.g. vertex abc" >&2
    echo "$p,1795791683600,50,20,10,5,10,3,70,12,8,2,0,50,10,0,12,0,8,2,0,2,0,mesh refused (too big)=6;indirect GS draw skipped=4,," >>"$VICTORY_METAL_DROPSTATS/back.csv"
    sleep 0.3
done
while :; do sleep 0.2; done
EOF
cat >"$T/wine/bin/wineserver" <<'EOF'
#!/bin/bash
exit 0
EOF
chmod +x "$T/wine/bin/wine" "$T/wine/bin/wineserver"
echo fake >"$T/victory-fake-accept-test.exe"
mkdir -p "$ROOT/target"; [ -x "$ROOT/target/dxc-d3dmetal" ] || { printf '#!/bin/sh\n' >"$ROOT/target/dxc-d3dmetal"; chmod +x "$ROOT/target/dxc-d3dmetal"; }

export GTA6_ROOT="$T/root" VICTORY_MAC_WINE_DIR="$T/wine" VICTORY_GAME_DIR="$T/game" VICTORY_GAME_LOCK="$T/game.lock"
export VICTORY_MAC_EXE_PATTERN='victory-fake-accept-test\.exe' VICTORY_MAC_UNTIL_POLL=0.25 WINEPREFIX="$T/prefix"
export VICTORY_ACCEPT_DIR="$T/accept"
unset VICTORY_CAPTURE VICTORY_RECORD VICTORY_CAPTURE_FRAMES VICTORY_LOG_WORLD VICTORY_METAL_OUT
fails=0; n=0
ok() { n=$((n + 1)); echo "ok $n - $1"; }
bad() { n=$((n + 1)); fails=$((fails + 1)); echo "NOT OK $n - $1"; }
expect() { if eval "$2"; then ok "$1"; else bad "$1   [$2]"; fi; }
AB="$ROOT/tools/mac/accept-batch.sh"
purpose='perf-hotspot: mac.fault.tracking_decision'
batch() { bash "$AB" --purpose "$purpose" "$@"; }
last_out() { ls -1d "$T"/accept/*/ | sed 's:/$::' | LC_ALL=C sort | tail -n 1; }

echo "== a round that reaches the target"
FAKE_MODE=ok batch --exe "$T/victory-fake-accept-test.exe" --stage "$T/stage" --world 12 --timeout 60 --label "first" >"$T/r1.out" 2>&1; rc=$?
d1="$(last_out)"
expect "exit 0" '[ $rc = 0 ]'
expect "folder named <time>-<short commit>" "basename $d1 | grep -Eq '^[0-9]{8}-[0-9]{6}-[0-9a-f]{7,}$'"
expect "summary.md and summary.json written" "[ -s $d1/summary.md ] && [ -s $d1/summary.json ]"
expect "alive, reached the target" "grep -q 'Result: OK: alive; reached world frame 12' $d1/summary.md"
expect "frame time window: 12 frames, median 16 ms" "grep -q 'world frames 1..12 (12 frames) | median \*\*16.0 ms\*\*' $d1/summary.md"
expect "one frame over 5 s counted" "grep -q 'frames over 5 s | \*\*1\*\* in w1..w12' $d1/summary.md"
expect "shared layer: recorded 1200, dropped 240 = 20 %" "grep -q 'dropped 240 = 20.00 %' $d1/summary.md && grep -q 'recorded 1200' $d1/summary.md"
expect "shared reasons listed" "grep -q '| 1 | \`no_host\` | 144 ' $d1/summary.md"
expect "back: reasons top list with the mesh refusal" "grep -q 'mesh refused (too big)' $d1/summary.md"
expect "back: draws 936" "grep -q 'draws 936 (submitted 840, dropped 96' $d1/summary.md"
expect "first one says baseline" "grep -q 'this is the first (baseline)' $d1/summary.md"
expect "version block names the stage and the exe sha" "grep -q 'stage:' $d1/summary.md && grep -Eq 'sha256 .[0-9a-f]{16}' $d1/summary.md"
expect "WAVE_WIDE shown unset" "grep -q 'VICTORY_WAVE_WIDE.=(unset)' $d1/summary.md"
expect "no game left, lock free" "! pgrep -f 'victory-fake-accept-test\.exe' >/dev/null && ! [ -d $T/game.lock ]"
expect "new game, no watchdog, frametime log, until-world in the run's env" "grep -q 'VICTORY_MAC_NO_WATCHDOG=1' $d1/run/run-env.txt && grep -q 'VICTORY_FRAMETIME_LOG=' $d1/run/run-env.txt && grep -q 'VICTORY_MAC_DROP_STATS=2' $d1/run/run-env.txt && grep -q 'VICTORY_METAL_DROPSTATS=' $d1/run/run-env.txt"
expect "default keys and input kept" "grep -q 'VICTORY_KEYS=8-3600/3:Enter' $d1/run/run-env.txt && grep -q 'world-neutral.txt' $d1/run/run-env.txt"
expect "perf profile, metal backend" "grep -q 'backend metal  profile perf' $d1/run/start.txt"
sleep 1

echo "== a second round: compared with the first"
FAKE_MODE=ok FAKE_BASE_US=20000 FAKE_DROP=40 FAKE_RAN=60 WAVE=1 VICTORY_WAVE_WIDE=1 batch --exe "$T/victory-fake-accept-test.exe" --stage "$T/stage" --world 12 --timeout 60 >"$T/r2.out" 2>&1; rc=$?
d2="$(last_out)"
expect "second: exit 0, a new folder" '[ $rc = 0 ] && [ "$d2" != "$d1" ]'
expect "second: compares with the first folder" "grep -q 'Last: \`$(basename $d1)\`' $d2/summary.md"
expect "second: shared drop rate 20 -> 40 (+20 pp)" "grep -q '20.00 % -> 40.00 %  (\*\*+20.00 pp\*\*)' $d2/summary.md"
expect "second: frame median 16.0 -> 20.0 ms" "grep -q '16.0 ms -> 20.0 ms  (\*\*+4.0 ms\*\*)' $d2/summary.md"
expect "second: WAVE_WIDE=1 shown" "grep -q 'VICTORY_WAVE_WIDE.=1' $d2/summary.md"
sleep 1

echo "== a round where the title dies"
FAKE_MODE=die batch --exe "$T/victory-fake-accept-test.exe" --stage "$T/stage" --world 12 --timeout 60 >"$T/r3.out" 2>&1; rc=$?
d3="$(last_out)"
expect "died: exit 1" '[ $rc = 1 ]'
expect "died: said so, with the status" "grep -q 'DIED before the world' $d3/summary.md"
expect "died: crash line shown" "grep -q 'Unhandled exception' $d3/summary.md"
sleep 1

echo "== a round that is alive but too slow for the cap"
FAKE_MODE=slow batch --exe "$T/victory-fake-accept-test.exe" --stage "$T/stage" --world 12 --timeout 5 --no-compare >"$T/r4.out" 2>&1; rc=$?
d4="$(last_out)"
expect "slow: exit 1" '[ $rc = 1 ]'
expect "slow: alive at the cap, world frame 4 reached" "grep -q 'alive at the cap, reached world frame 4 of 12' $d4/summary.md"
expect "slow: no comparison with --no-compare" "grep -q 'first (baseline)' $d4/summary.md"
expect "slow: the cap stopped it in about 5 s, lock free" "! pgrep -f 'victory-fake-accept-test\.exe' >/dev/null && ! [ -d $T/game.lock ]"

echo "== guards"
expect "purpose recorded verbatim" 'grep -Fq -- "$purpose" "$d1/run/RUN.txt"'
expect "missing purpose: exit 7" 'bash "$AB" >/dev/null 2>&1; [ $? = 7 ]'
expect "malformed purpose: exit 7" 'bash "$AB" --purpose nope >/dev/null 2>&1; [ $? = 7 ]'
expect "no exe: exit 2" "batch --exe $T/nonexistent.exe --stage $T/stage >/dev/null 2>&1; [ \$? = 2 ]"
expect "no stage: exit 2" "batch --exe $T/victory-fake-accept-test.exe --stage $T/nostage >/dev/null 2>&1; [ \$? = 2 ]"
expect "bad --world: exit 2" "bash $AB --world x >/dev/null 2>&1; [ \$? = 2 ]"
expect "default exe missing: exit 2 with the path named" "batch 2>&1 | grep -q 'worktrees/mac/int/target'"
expect "--summarize of an earlier run makes a summary" "bash $AB --summarize $d1/run --out $T/again --world 12 --no-compare >/dev/null 2>&1; grep -q 'Result: OK' $T/again/summary.md"
expect "five folders in the acceptance folder? no: four rounds" "[ \$(ls -1d $T/accept/*/ | wc -l) = 4 ]"
echo
echo "$((n - fails)) of $n passed (logs in $T)"
[ "$fails" = 0 ]
