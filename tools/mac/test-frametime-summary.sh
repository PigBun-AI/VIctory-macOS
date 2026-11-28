#!/bin/bash
# Tests of the frame time / load marking: tools/mac/frametime-summary.sh on made-up logs (the numbers are exact), and run.sh's summary.txt with
# a FAKE Wine (as test-run-until.sh: no game, no GPU, own lock, own prefix) whose "title" writes frametime.log in real time while a fake
# `uptime` (first in PATH) reports the load the test says. Takes about a minute.
#   tools/mac/test-frametime-summary.sh [TMPDIR]
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
T="${1:-$(mktemp -d "${TMPDIR:-/tmp}/frametime-test.XXXXXX")}"
mkdir -p "$T/wine/bin" "$T/game" "$T/root" "$T/bin"
T="$(cd "$T" && pwd)"
S="$ROOT/tools/mac/frametime-summary.sh"
fails=0; n=0
ok() { n=$((n + 1)); echo "ok $n - $1"; }
bad() { n=$((n + 1)); fails=$((fails + 1)); echo "NOT OK $n - $1"; }
expect() { if eval "$2"; then ok "$1"; else bad "$1   [$2]"; fi; }

# ---- 1. the script on made-up logs -------------------------------------------------------------------------------
# mk DIR SEGMENTS...: SEGMENTS are "COUNT:FRAME_US:LOAD"; frames in order, the log ends at END (fixed); load.log has a line every 2 s
# whose load is that of the segment the line's time falls in (5 for times before the first frame)
mk() {
    local d=$1; shift
    mkdir -p "$d"
    python3 - "$d" "$@" <<'PY'
import sys, time
d = sys.argv[1]; segs = [tuple(map(float, s.split(":"))) for s in sys.argv[2:]]
END = time.mktime(time.strptime("2026-11-28T10:00:40", "%Y-%m-%dT%H:%M:%S"))
rows = []   # (frame, us, load of the segment)
f = 100
rows.append((f, 0, segs[0][2]))
for cnt, us, ld in segs:
    for _ in range(int(cnt)):
        f += 1; rows.append((f, int(us), ld))
t = END; times = [0] * len(rows)
for k in range(len(rows) - 1, -1, -1):
    times[k] = t; t -= rows[k][1] / 1e6
open(d + "/frametime.log", "w").write("frame,frame_us,draws\n" + "".join("%d,%d,%d\n" % (a, b, 7) for a, b, _ in rows))
# load.log: every 2 s from the first frame's time; each line's load = that of the segment its time is in
start = times[0]
lines = []
x = start
while x <= END:
    k = max([i for i, tt in enumerate(times) if tt <= x] or [0])
    lines.append("%s load average: %.2f 20.00 10.00" % (time.strftime("%Y-%m-%dT%H:%M:%S", time.localtime(x)), rows[k][2]))
    x += 2
open(d + "/load.log", "w").write("\n".join(lines) + "\n")
open(d + "/END", "w").write("%f" % END)
PY
}
val() { grep "^frametime valid" "$1" | sed 's/.*: //'; }   # "median X ms, p90 Y ms"

echo "== frametime-summary.sh: a mostly quiet run (20 slow frames in a load of 50)"
mk "$T/u1" "100:100000:5" "20:300000:50" "100:100000:5"
sh "$S" "$T/u1" --end "$(cat "$T/u1/END")" >"$T/u1/out.txt"; rc=$?
expect "u1: exit 0" '[ $rc = 0 ]'
expect "u1: not more than 20% invalid: no 数据无效 line, the first line is the frame count line" 'head -1 "$T/u1/out.txt" | grep -q "^frametime: 220 frames" && ! grep -q "数据无效" "$T/u1/out.txt"'
expect "u1: the 20 slow frames (and a few at the edges) are invalid" 'inv=$(sed -n "s/.* valid, \([0-9]*\) invalid.*/\1/p" "$T/u1/out.txt"); [ "$inv" -ge 20 ] && [ "$inv" -le 40 ]'
expect "u1: the statistics are those of the valid frames only: median 100.00 ms, p90 100.00 ms" '[ "$(val "$T/u1/out.txt")" = "median 100.00 ms, p90 100.00 ms" ]'
expect "u1: the all-frames line is there for comparison" 'grep -q "^frametime all frames" "$T/u1/out.txt"'
expect "u1: frametime-annot.csv: most slow frames are valid=0 (a load line covers the 2 s after it, so the first ones can still sit in the old line's), most normal ones 1" 'awk -F, "NR>1 && \$2==300000 {s++; if (\$6==0) z++} NR>1 && \$2==100000 && \$6==1 {g++} END {exit !(s==20 && z>=10 && g>=170)}" "$T/u1/frametime-annot.csv"'

echo "== frametime-summary.sh: a noisy run (100 slow of 300 frames: more than 20%)"
mk "$T/u2" "100:100000:5" "100:300000:50" "100:100000:5"
sh "$S" "$T/u2" --end "$(cat "$T/u2/END")" >"$T/u2/out.txt"
expect "u2: the FIRST line is 数据无效：load 超过 30" '[ "$(head -1 "$T/u2/out.txt")" = "数据无效：load 超过 30" ]'
expect "u2: the invalid share is printed and is over 20%" 'p=$(sed -n "s/.*invalid = \([0-9.]*\)%.*/\1/p" "$T/u2/out.txt"); awk -v p="$p" "BEGIN {exit !(p > 20)}"'
expect "u2: all frames (for comparison) would have p90 300.00 ms" 'grep "^frametime all frames" "$T/u2/out.txt" | grep -q "p90 300.00 ms"'
expect "u2: the valid frames' median is still 100.00 ms" '[ "$(val "$T/u2/out.txt")" = "median 100.00 ms, p90 100.00 ms" ]'

echo "== frametime-summary.sh: edges"
mk "$T/u3" "100:100000:5" "100:300000:50"
rm "$T/u3/load.log"
sh "$S" "$T/u3" --end "$(cat "$T/u3/END")" >"$T/u3/out.txt"
expect "no load.log: every frame counts, said so, no 数据无效" 'grep -q "no load.log" "$T/u3/out.txt" && ! grep -q "数据无效" "$T/u3/out.txt" && grep -q "^frametime: 200 frames" "$T/u3/out.txt"'
mk "$T/u4" "100:100000:50"
sed -i '' 's/\([0-9]\)\.\([0-9][0-9]\) 20.00 10.00/\1,\2 20,00 10,00/' "$T/u4/load.log"
sh "$S" "$T/u4" --end "$(cat "$T/u4/END")" >"$T/u4/out.txt"
expect "decimal commas in load.log (other locale): all 100 frames invalid, 数据无效" 'head -1 "$T/u4/out.txt" | grep -q 数据无效 && grep -q "100 invalid" "$T/u4/out.txt" && grep -q "no frames" "$T/u4/out.txt"'
VICTORY_LOAD_INVALID=60 sh "$S" "$T/u4" --end "$(cat "$T/u4/END")" | grep -q "^frametime valid frames: median 100.00" ; r1=$?
sh "$S" "$T/u4" --end "$(cat "$T/u4/END")" --load-limit 60 | grep -q "^frametime valid frames: median 100.00"; r2=$?
expect "the load limit can be changed (VICTORY_LOAD_INVALID, --load-limit)" '[ $r1 = 0 ] && [ $r2 = 0 ]'
mkdir -p "$T/u5"; sh "$S" "$T/u5" 2>/dev/null; rc=$?
expect "no frametime.log: exit 1" '[ $rc = 1 ]'
expect "bash/sh -n" 'sh -n "$S"'

# ---- 2. run.sh's summary with a fake Wine ---------------------------------------------------------------------------
cat >"$T/emit.py" <<'PY'
import sys, time, os
log, loadfile, total, high = sys.argv[1], sys.argv[2], float(sys.argv[3]), float(sys.argv[4])
t0 = time.time(); last = t0
f = open(log, "a", buffering=1)
hi = high > 0
if hi: open(loadfile, "w").write("55.00")
n = 0
while time.time() - t0 < total:
    time.sleep(0.1)
    now = time.time(); n += 1
    f.write("%d,%d,5\n" % (n, int((now - last) * 1e6))); f.flush(); last = now
    if hi and now - t0 >= high:
        open(loadfile, "w").write("3.00"); hi = False
PY
cat >"$T/wine/bin/wine" <<'EOF'
#!/bin/bash
trap 'exit 143' TERM
echo "fake title started"
python3 "$FAKE_EMIT" "$VICTORY_FRAMETIME_LOG" "$FAKE_LOADFILE" "${FAKE_TOTAL:-8}" "${FAKE_HIGH:-0}"
while :; do sleep 0.2; done
EOF
printf '#!/bin/bash\nexit 0\n' >"$T/wine/bin/wineserver"
printf '#!/bin/bash\necho "11:00  up 1 day,  2 users,  load averages: $(cat "$FAKE_LOADFILE") 20.00 10.00"\n' >"$T/bin/uptime"
chmod +x "$T/wine/bin/wine" "$T/wine/bin/wineserver" "$T/bin/uptime"
echo fake >"$T/victory-fake-ft-test.exe"
FAKE_PAT="${T//./\\.}/victory-fake-ft-test\\.exe"   # (this test's own directory: no real game, no other copy of the test can match)
mkdir -p "$ROOT/target"; [ -x "$ROOT/target/dxc-d3dmetal" ] || { printf '#!/bin/sh\n' >"$ROOT/target/dxc-d3dmetal"; chmod +x "$ROOT/target/dxc-d3dmetal"; }
export GTA6_ROOT="$T/root" VICTORY_MAC_WINE_DIR="$T/wine" VICTORY_GAME_DIR="$T/game" VICTORY_GAME_LOCK="$T/game.lock"
export VICTORY_RUN_GUARD=0 VICTORY_MAC_EXE_PATTERN="$FAKE_PAT" FAKE_EMIT="$T/emit.py" FAKE_LOADFILE="$T/load.now" VICTORY_LOAD_LOG_INTERVAL=1
unset VICTORY_CAPTURE VICTORY_RECORD VICTORY_CAPTURE_FRAMES VICTORY_LOG_WORLD
runit() { # NAME TOTAL HIGH
    echo "3.00" >"$T/load.now"
    FAKE_TOTAL=$2 FAKE_HIGH=$3 PATH="$T/bin:$PATH" bash "$ROOT/tools/mac/run.sh" --purpose "perf-hotspot: mac.test.frametime" --backend d3dmetal \
        --exe "$T/victory-fake-ft-test.exe" --prefix "$T/prefix-$1" --out "$T/$1" --no-keys --input none --frametime-log --timeout $(($2 + 3)) \
        >"$T/$1.console" 2>"$T/$1.console.err"
}
echo "== run.sh: load stays low"
runit quiet 8 0
expect "quiet: frametime.log and load.log were written" '[ -s "$T/quiet/frametime.log" ] && [ -s "$T/quiet/load.log" ]'
expect "quiet: summary.txt has the frame time lines and no 数据无效" 'grep -q "^frametime: " "$T/quiet/summary.txt" && grep -q "^frametime valid frames: median" "$T/quiet/summary.txt" && ! grep -q 数据无效 "$T/quiet/summary.txt"'
expect "quiet: nothing invalid" 'grep -q " 0 invalid = 0.0%" "$T/quiet/summary.txt"'
expect "quiet: the first line of summary.txt is the usual one" 'sed -n 1p "$T/quiet/summary.txt" | grep -q "^backend d3dmetal, profile"'
echo "== run.sh: load is high for the first 3 s of 8"
runit noisy 8 3
expect "noisy: the first line of summary.txt is 数据无效：load 超过 30" '[ "$(sed -n 1p "$T/noisy/summary.txt")" = "数据无效：load 超过 30" ]'
expect "noisy: the second line is the usual one, the invalid share follows" 'sed -n 2p "$T/noisy/summary.txt" | grep -q "^backend d3dmetal" && grep -q "invalid = [0-9.]*%" "$T/noisy/summary.txt"'
expect "noisy: the console shows the same summary" 'head -1 "$T/noisy.console" | grep -q . && grep -q "数据无效：load 超过 30" "$T/noisy.console"'
expect "noisy: frames of the first seconds are marked invalid in frametime-annot.csv, the last one valid" 'awk -F, "NR>1 && NR<=40 && \$6==0 {a++} END {exit !(a>0 && \$6==1)}" "$T/noisy/frametime-annot.csv"'
echo "== run.sh: a short load peak (0.4 s of 10): under 20%, the data stay valid"
runit blip 10 0.4
expect "blip: no 数据无效, and the share is printed" '! grep -q 数据无效 "$T/blip/summary.txt" && grep -q "invalid = [0-9.]*%" "$T/blip/summary.txt"'
expect "the fake title and the lock are gone" '! pgrep -f "$FAKE_PAT" >/dev/null && [ ! -d "$VICTORY_GAME_LOCK" ]'
echo
echo "$((n - fails)) of $n passed (logs in $T)"
[ "$fails" = 0 ]
