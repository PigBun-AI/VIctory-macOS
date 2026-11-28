#!/bin/bash
# Tests of the --until-* options of tools/mac/run.sh with a FAKE Wine (no game, no GPU, nothing of the real prefix or lock is
# touched): a stand-in `wine` prints what the case needs and then waits to be stopped, a stand-in `wineserver` records its calls,
# VICTORY_MAC_EXE_PATTERN keeps run.sh's "another victory.exe is running" check away from a real game, and the game lock is a
# directory of its own (VICTORY_GAME_LOCK). Takes about a minute.
#   tools/mac/test-run-until.sh [TMPDIR]        (TMPDIR is kept: it holds every case's logs)
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
T="${1:-$(mktemp -d "${TMPDIR:-/tmp}/run-until-test.XXXXXX")}"
mkdir -p "$T/wine/bin" "$T/game" "$T/root" "$T/cap"
T="$(cd "$T" && pwd)"   # (no double slash from a TMPDIR that ends in one: run.sh prints the cleaned path)
# the pattern of run.sh's "another victory.exe is running" check (and of no_fake_left): the fake exe's path in THIS test's own directory, so neither a
# real game nor another copy of this test running at the same time can match it (VICTORY_MAC_EXE_PATTERN is a regular expression)
FAKE_PAT="${T//./\\.}/victory-fake-until-test\\.exe"
# the fake Wine: FAKE_MODE says what the "title" does; it ends when it is stopped (TERM), like the real one
cat >"$T/wine/bin/wine" <<'EOF'
#!/bin/bash
trap 'exit 143' TERM
echo "fake title started (prefix $WINEPREFIX)"
case "${FAKE_MODE:-idle}" in
    log) sleep 1; echo "stdout line"; sleep 1; echo "2026 INFO victory: RENDER-READY frame=3" >&2 ;;
    world) sleep 1; echo "2026 INFO victory_gpu::mac::world: world frame 1 (input scripts) present=40 draws=600" >&2
           for n in 2 3 4 5 6 7 8 9 10 11 12 50; do sleep 0.3; echo "2026 INFO victory_gpu::mac::world: world frame $n present=$((39 + n))" >&2; done ;;
    capture-log) sleep 1; echo "2026 INFO victory_gpu::capture: frame saved path=/x/cap\\frame-1.png" >&2
                 sleep 1; echo "2026 INFO victory_gpu::capture: frame saved path=/x/cap\\frame-64.png" >&2 ;;
    capture-file) sleep 1; : >"$VICTORY_CAPTURE/frame-64.png"; printf 'abc' >"$VICTORY_CAPTURE/frame-64.png"; sleep 1; printf 'defg' >>"$VICTORY_CAPTURE/frame-64.png" ;;
    capture-w) sleep 1; echo "2026 INFO victory_gpu::mac::world: world frame 1 (input scripts) present=100 draws=600" >&2
               sleep 1; echo "2026 INFO victory_gpu::capture: frame saved path=/x/cap\\frame-112.png" >&2 ;;
    record) sleep 1; mkdir -p "$VICTORY_RECORD/capture-1"; : >"$VICTORY_RECORD/capture-1/frame.bin"
            echo "2026 INFO victory_gpu::replay::snap: frame recorder: the frame is captured" >&2 ;;
    present) for n in 1 2 3 4 5 6 7 8 9 10; do sleep 0.4; echo "$n,16000,5" >>"$VICTORY_FRAMETIME_LOG"; done ;;
    quick) echo "title ended by itself" ; exit 0 ;;
esac
[ "${FAKE_MODE:-idle}" = quick ] && exit 0
while :; do sleep 0.2; done
EOF
cat >"$T/wine/bin/wineserver" <<'EOF'
#!/bin/bash
echo "wineserver $* prefix=${WINEPREFIX:-unset}" >>"$FAKE_SERVER_CALLS"
exit 0
EOF
chmod +x "$T/wine/bin/wine" "$T/wine/bin/wineserver"
echo fake >"$T/victory-fake-until-test.exe"
# run.sh's DXC front end: an existing, newer file, so that nothing is built
mkdir -p "$ROOT/target"; [ -x "$ROOT/target/dxc-d3dmetal" ] || { printf '#!/bin/sh\n' >"$ROOT/target/dxc-d3dmetal"; chmod +x "$ROOT/target/dxc-d3dmetal"; }

export GTA6_ROOT="$T/root" VICTORY_MAC_WINE_DIR="$T/wine" VICTORY_GAME_DIR="$T/game" VICTORY_GAME_LOCK="$T/game.lock"
export VICTORY_MAC_EXE_PATTERN="$FAKE_PAT" FAKE_SERVER_CALLS="$T/server.calls" VICTORY_MAC_UNTIL_POLL=0.25
unset VICTORY_CAPTURE VICTORY_RECORD VICTORY_CAPTURE_FRAMES VICTORY_LOG_WORLD
fails=0; n=0
ok() { n=$((n + 1)); echo "ok $n - $1"; }
bad() { n=$((n + 1)); fails=$((fails + 1)); echo "NOT OK $n - $1"; }
expect() { if eval "$2"; then ok "$1"; else bad "$1   [$2]"; fi; }
# run NAME MODE ARGS... : one run into $T/<NAME>; status in $rc, time in $secs
run() {
    name=$1; mode=$2; shift 2
    : >"$T/server.calls"
    t_start=$(date +%s)
    FAKE_MODE=$mode bash "$ROOT/tools/mac/run.sh" --purpose "freq: mac.test.until" --backend d3dmetal --exe "$T/victory-fake-until-test.exe" --prefix "$T/prefix-$name" \
        --out "$T/$name" --no-keys --input none "$@" >"$T/$name.console" 2>"$T/$name.console.err"; rc=$?
    secs=$(( $(date +%s) - t_start ))
}
lock_free() { ! [ -d "$VICTORY_GAME_LOCK" ]; }
no_fake_left() { ! pgrep -f "$FAKE_PAT" >/dev/null; }

echo "== --until-log: a matching line ends the run, exit 0, the lock is let go"
run log log --timeout 30 --until-log 'RENDER-READY frame=[0-9]+'
expect "until-log: exit 0" '[ $rc = 0 ]'
expect "until-log: ended long before --timeout" '[ $secs -lt 12 ]'
expect "until-log: lock released" lock_free
expect "until-log: the fake title is gone" no_fake_left
expect "until-log: start.txt says which and when" "grep -q \"until 条件满足：--until-log 'RENDER-READY frame=\\[0-9\\]+' 在 [0-9]* s\" $T/log/start.txt"
expect "until-log: run-env.txt says it" "grep -q '^# until 条件满足' $T/log/run-env.txt"
expect "until-log: stderr.log says it" "grep -q '^== until 条件满足' $T/log/stderr.log"
expect "until-log: wineserver -k only for this prefix" "grep -q 'wineserver -k prefix=$T/prefix-log' $T/server.calls && ! grep -v 'prefix=$T/prefix-log' $T/server.calls | grep -q ."
expect "until-log: no time limit line" "! grep -q '== time limit' $T/log/stderr.log"

echo "== --until-log: several patterns, the second one matches; the first never does"
run log2 log --timeout 30 --until-log 'NEVER-HAPPENS' --until-log 'stdout line'
expect "two --until-log: exit 0 on the second" '[ $rc = 0 ] && [ $secs -lt 10 ]'
expect "two --until-log: names the pattern that matched" "grep -q \"until-log 'stdout line'\" $T/log2/start.txt"

echo "== --until-world N: the log has world frame N (and 5 is not 50)"
run world world --timeout 30 --until-world 5
expect "until-world: exit 0" '[ $rc = 0 ] && [ $secs -lt 12 ]'
expect "until-world: names it" "grep -q 'until-world 5' $T/world/start.txt"
expect "until-world: asked the title for the per-frame lines" "grep -q '^VICTORY_LOG_WORLD=1' $T/world/run-env.txt"
expect "until-world: lock released" lock_free

echo "== --until-capture: a frame saved line, a PNG of stable size, a w<N> frame, a recording"
run capl capture-log --timeout 30 --capture-frames 1,64 --until-capture
expect "until-capture (log): exit 0 after the last frame" '[ $rc = 0 ] && [ $secs -lt 12 ]'
expect "until-capture (log): not before the last frame" "grep -q 'frame-64' $T/capl/stderr.log"
VICTORY_CAPTURE="$T/cap" run capf capture-file --timeout 30 --capture-frames 64 --until-capture
expect "until-capture (file): exit 0 once the PNG is there and stays the same size for 2 s" '[ $rc = 0 ] && [ $secs -ge 4 ] && [ $secs -lt 14 ]'
expect "until-capture (file): the PNG was complete (7 bytes)" "[ \$(stat -f %z $T/cap/frame-64.png) = 7 ]"
run capw capture-w --timeout 30 --capture-frames w12 --until-capture
expect "until-capture (w12 = world frame 1's present 100 + 12): exit 0" '[ $rc = 0 ] && [ $secs -lt 12 ]'
expect "until-capture (w12): per-frame world lines asked for" "grep -q '^VICTORY_LOG_WORLD=1' $T/capw/run-env.txt"
VICTORY_RECORD="$T/rec" run rec record --timeout 30 --record-at w5 --until-capture
expect "until-capture (recording): exit 0 on the captured frame" '[ $rc = 0 ] && [ $secs -lt 12 ]'
expect "until-capture: needs something to wait for" "! bash $ROOT/tools/mac/run.sh --print-env --until-capture >/dev/null 2>&1"

echo "== --until-present N"
run pres present --timeout 30 --until-present 6
expect "until-present: exit 0 (frametime.log turned on)" '[ $rc = 0 ] && [ $secs -lt 12 ]'
expect "until-present: frametime.log asked for" "grep -q '^VICTORY_FRAMETIME_LOG=' $T/pres/run-env.txt"

echo "== not met: --timeout ends it, exit 6, 'until 未满足' in the log"
run unmet idle --timeout 3 --until-log 'NEVER-HAPPENS'
expect "unmet: exit 6" '[ $rc = 6 ]'
expect "unmet: took about --timeout" '[ $secs -ge 3 ] && [ $secs -lt 12 ]'
expect "unmet: the log says so" "grep -q '^== until 未满足' $T/unmet/stderr.log && grep -q '^== time limit 3s reached' $T/unmet/stderr.log"
expect "unmet: start.txt says so" "grep -q 'until 未满足' $T/unmet/start.txt"
expect "unmet: lock released, title gone" 'lock_free && no_fake_left'

echo "== --linger: the run goes on for N seconds after the condition"
run ling log --timeout 30 --until-log 'RENDER-READY' --linger 4
expect "linger: exit 0" '[ $rc = 0 ]'
expect "linger: the run lasted 4 s longer than the match (about 2 s in)" '[ $secs -ge 6 ]'
expect "linger: start.txt keeps the time of the match, not of the end" "grep -q 'linger 4 s' $T/ling/start.txt"
run nolinger log --timeout 30 --until-log 'RENDER-READY'
expect "no linger: shorter than with it" '[ $secs -lt 6 ]'

echo "== no --until-*: as before (the committed run.sh, in a tree of its own, against this one)"
mkdir -p "$T/before/tools"
if git -C "$ROOT" show HEAD:tools/mac/run.sh >"$T/run-before.sh" 2>/dev/null && ! git -C "$ROOT" diff --quiet HEAD -- tools/mac/run.sh; then
    mkdir -p "$T/before/tools/mac"; cp "$T/run-before.sh" "$T/before/tools/mac/run.sh"
    ln -sfn "$ROOT/tools/macos" "$T/before/tools/macos"; ln -sfn "$ROOT/target" "$T/before/target"
    # (every other script of tools/mac that run.sh sources or calls: game-lock.sh, purpose.sh, collect-title-crash.sh ...)
    for f in "$ROOT"/tools/mac/*; do
        case "${f##*/}" in run.sh|run.local.env) continue ;; esac
        ln -sfn "$f" "$T/before/tools/mac/${f##*/}"
    done
    # (the machine's untracked settings are part of the environment both copies print: the before tree reads the same file)
    [ ! -f "$ROOT/tools/mac/run.local.env" ] || cp -p "$ROOT/tools/mac/run.local.env" "$T/before/tools/mac/run.local.env"
    for who in before new; do
        script="$T/before/tools/mac/run.sh"; [ $who = new ] && script="$ROOT/tools/mac/run.sh"
        : >"$T/server.calls"
        FAKE_MODE=log bash "$script" --purpose "freq: mac.test.until" --backend d3dmetal --exe "$T/victory-fake-until-test.exe" --prefix "$T/prefix-d" --out "$T/default-$who" \
            --no-keys --input none --timeout 4 >"$T/default-$who.console" 2>"$T/default-$who.console.err"; echo $? >"$T/default-$who.rc"
        cp "$T/server.calls" "$T/default-$who.calls"
    done
    strip() { grep -a -v -E '^(checkout|exe |20[0-9]{2}-|load average|logs in )|^[0-9]{4}-|load average at start|[0-9]+ s \(limit' "$1" | sed -E 's#default-(before|new)#D#g'; }
    expect "default: same exit status" "[ \$(cat $T/default-before.rc) = \$(cat $T/default-new.rc) ]"
    expect "default: exit status is the title's (stopped by the limit)" "[ \$(cat $T/default-new.rc) != 0 ] && [ \$(cat $T/default-new.rc) != 6 ]"
    expect "default: same files" "[ \"\$(cd $T/default-before && ls | tr '\\n' ' ')\" = \"\$(cd $T/default-new && ls | tr '\\n' ' ')\" ]"
    expect "default: no until text anywhere" "! grep -rqiE '(== |# |^)until |until-(met|failed)|条件' $T/default-new/start.txt $T/default-new/stderr.log $T/default-new/summary.txt $T/default-new/end.txt $T/default-new/run-env.txt"
    expect "default: the stop path is the time limit's" "grep -q '^== time limit 4s reached' $T/default-new/stderr.log"
    expect "default: same wineserver calls" "diff $T/default-before.calls $T/default-new.calls >/dev/null"
    expect "default: same console output apart from times" "diff <(strip $T/default-before.console) <(strip $T/default-new.console) >/dev/null"
    pe() { bash "$1" --print-env --backend d3dmetal --exe /x --prefix /p --out /o 2>&1 | sed "s#$T/before#R#g; s#$ROOT#R#g; s#${ROOT/#$HOME/~}#R#g"; }   # (run.sh prints the home as ~)
    expect "default: print-env is unchanged" "diff <(pe $T/before/tools/mac/run.sh) <(pe $ROOT/tools/mac/run.sh) >/dev/null"
    expect "default: no time limit line when the title ended first" "! grep -q 'time limit' $T/nolinger/stderr.log"
else
    echo "(skipped: run.sh is the committed one; the comparison needs the change uncommitted)"
fi
echo "== --purpose: required for a run (exit status 7), not for --print-env and --help"
P="$ROOT/tools/mac/run.sh"
refused() { # NAME ARGS...: run.sh must refuse with exit 7, start nothing (no out folder, no lock), and say why in $T/refused.err
    local name=$1; shift
    : >"$T/server.calls"
    FAKE_MODE=idle bash "$P" --backend d3dmetal --exe "$T/victory-fake-until-test.exe" --prefix "$T/prefix-refused" --out "$T/refused-$name" --no-keys --input none --timeout 3 "$@" >"$T/refused.out" 2>"$T/refused.err"; rc=$?
}
refused none
expect "no --purpose: exit 7, says what is needed" '[ $rc = 7 ] && grep -q -- "--purpose is required" "$T/refused.err"'
expect "no --purpose: nothing started (no out folder, no lock, no wineserver call, no fake title)" '[ ! -e "$T/refused-none" ] && lock_free && [ ! -s "$T/server.calls" ] && no_fake_left'
refused column --purpose "frames: mac.test.node"
expect "a column that is not one of the five: exit 7, names the allowed ones" '[ $rc = 7 ] && grep -q "must be one of freq callstats new-nodes corpus perf-hotspot" "$T/refused.err" && [ ! -e "$T/refused-column" ]'
refused second --purpose "freq,bogus: mac.test.node"
expect "a bad second column: exit 7" '[ $rc = 7 ]'
refused ids --purpose "freq:"
expect "no node id after the colon: exit 7, says so" '[ $rc = 7 ] && grep -q "at least one node id" "$T/refused.err" && [ ! -e "$T/refused-ids" ]'
refused ids2 --purpose "freq:  , "
expect "only separators after the colon: exit 7" '[ $rc = 7 ]'
refused ids3 --purpose "freq: a,"
expect "a trailing comma (an empty node id): exit 7" '[ $rc = 7 ]'
refused nocolon --purpose "freq mac.test.node"
expect "no colon: exit 7" '[ $rc = 7 ] && grep -q "no colon" "$T/refused.err"'
refused empty --purpose ""
expect "an empty --purpose: exit 7" '[ $rc = 7 ]'
refused exec --exec -- warm
expect "--exec needs it too (it holds the lock): exit 7" '[ $rc = 7 ] && lock_free'
expect "--print-env needs none" 'bash "$P" --print-env --backend d3dmetal >/dev/null 2>&1'
expect "--help needs none (it prints the usage, exit 2)" 'bash "$P" --help 2>&1 | grep -q -- "--purpose"'
expect "several columns and several ids are fine" 'bash "$P" --print-env --backend d3dmetal --purpose "freq, callstats: mac.a.b, S-G-009,c_d" >/dev/null 2>&1'
# the accepted forms, as the real thing
run purp idle --timeout 2 --purpose "new-nodes,corpus: mac.isa.x, S-G-009"
expect "(a later --purpose wins; the text is kept as given) RUN.txt starts with the purpose" "[ \"\$(sed -n 1p $T/purp/RUN.txt)\" = 'purpose: new-nodes,corpus: mac.isa.x, S-G-009' ]"
VICTORY_X=1 FAKE_MODE=idle bash "$P" --purpose "perf-hotspot: mac.perf.frame" --backend d3dmetal --exe "$T/victory-fake-until-test.exe" --prefix "$T/prefix-st" --out "$T/st" --no-keys --input none --timeout 8 >"$T/st.out" 2>&1 &
stpid=$!
waitlock=0; while [ ! -f "$VICTORY_GAME_LOCK/purpose" ] && [ $waitlock -lt 100 ]; do sleep 0.1; waitlock=$((waitlock + 1)); done
expect "the lock holds the purpose; game-lock.sh status shows it" "grep -qx 'perf-hotspot: mac.perf.frame' $VICTORY_GAME_LOCK/purpose && bash $ROOT/tools/mac/game-lock.sh status | grep -q 'purpose: perf-hotspot: mac.perf.frame'"
wait $stpid
expect "the purpose is in RUN.txt of that run too" "[ \"\$(sed -n 1p $T/st/RUN.txt)\" = 'purpose: perf-hotspot: mac.perf.frame' ]"
echo "== guards"
expect "--linger alone only warns" "bash $ROOT/tools/mac/run.sh --print-env --backend d3dmetal --linger 3 2>&1 >/dev/null | grep -q 'no effect'"
expect "bad --until-log refused" "! bash $ROOT/tools/mac/run.sh --print-env --until-log '(' >/dev/null 2>&1"
expect "bad --until-world refused" "! bash $ROOT/tools/mac/run.sh --print-env --until-world x >/dev/null 2>&1"
expect "hint for --capture-frames" "bash $ROOT/tools/mac/run.sh --print-env --backend d3dmetal --capture-frames 64 2>&1 >/dev/null | grep -q -- '--until-capture'"
expect "hint for --corpus" "bash $ROOT/tools/mac/run.sh --print-env --backend d3dmetal --corpus 2>&1 >/dev/null | grep -q -- '--until-capture'"
expect "no hint with --until-capture" "! bash $ROOT/tools/mac/run.sh --print-env --backend d3dmetal --capture-frames 64 --until-capture 2>&1 >/dev/null | grep -q hint"
expect "the game lock is free at the end" lock_free
echo
echo "$((n - fails)) of $n passed (logs in $T)"
[ "$fails" = 0 ]
