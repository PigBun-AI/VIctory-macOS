#!/bin/sh
# Runs the game (or any victory.exe command) on our Metal backend: our
# d3d12.dll / dxgi.dll, the bridge and the Metal back, in a prefix of their
# own, through the upstream tools/macos/victory.sh (which stays as it is).
#
#   tools/metal/run.sh [options] [-- victory.exe arguments]
#
#   --no-build        use the stage of the last tools/metal/build-unixlib.sh
#   --timeout SEC     stop the run after SEC seconds (default 600; 0 = no limit)
#   --out DIR         where the logs go (default research/game-runs/<time>-metal,
#                     outside the repository: game-derived data never goes in it)
#   --lenient         set VICTORY_METAL_LENIENT=1 (see below)
#   --keys SPEC       VICTORY_KEYS for this run (default below; `none` = no scripted keys)
#   --input FILE      VICTORY_INPUT_REPLAY for this run (default below; `none` = off)
#   --exec            no timer, no log files, no summary: `exec` victory.sh as it is
#                     (what the first version of this script did)
#
# Without arguments after `--` it runs the game:
#   run --execute $GAME_DIR/game_scarlett_master.exe
# GAME_DIR defaults to ~/Documents/GTA6Project/GTA6.Xbox.v1.0.2.1 (read only here).
#
# What it sets:
#   WINEPREFIX=research/metal-play-prefix   made on first use (wineboot); our dlls are
#                    installed into its system32, so NEVER the D3DMetal baseline prefix
#                    VIctory-macOS/target/wineprefix-play (refused)
#   WINEDLLOVERRIDES="d3d12,dxgi=n,b"       victory.sh puts its own `d3d12,d3d12core,dxgi=b`
#                    before it; the last one wins, so d3d12 and dxgi are ours
#   WINEDLLPATH=<stage>                     where the bridge's dll and unixlib are
#   VICTORY_METAL_LOG=1                     back/front/bridge errors on stderr
#   VICTORY_METAL_NOTIMPL_LOG=<file>        one line per call of an unimplemented method
#   VICTORY_METAL_LENIENT=1 (--lenient)     survey mode: a list command without an encoding is skipped
#                    instead of failing the list, so the game runs on and the gap list is complete
#   VICTORY_KEYS=8-900/3:Enter              presses Enter (= pad A through the keyboard's stand-in pad)
#                    every 3 s from 8 s on, so the intro and account screens go on by themselves
#   VICTORY_INPUT_REPLAY=tools/metal/input/world-neutral.txt   once in the world (world_frame >= 1)
#                    the replay owns the pad: neutral, the same input every run (fixed-scene benchmarks)
#   VICTORY_PATCH_MASKED=all VICTORY_FAULT_TRACKING=1   the game flags NOTES-victory-macos.md used
#                    (a value already in the environment wins)
# victory.toml (git-ignored, repository root) is written if missing: game dir, cache and data under
# ~/Documents/GTA6Project/victory-data, all outside the repository.
#
# Output in the --out directory: stdout.log, stderr.log (Wine and victory.exe; the
# "victory-metal ..." lines), notimpl.log, summary.txt (what was reached, exit status,
# unimplemented methods by number of calls, the back's errors). After the time is up
# the Wine processes of THIS prefix are stopped (wineserver -k with WINEPREFIX set);
# other prefixes are never touched.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$here/../.." && pwd)
# shellcheck disable=SC1091
. "$here/env.sh"

build=1; timeout=600; out=""; plain=0; lenient=0
keys="${VICTORY_KEYS:-8-900/3:Enter}"; input="${VICTORY_INPUT_REPLAY:-$here/input/world-neutral.txt}"
while [ $# -gt 0 ]; do
    case "$1" in
        --no-build) build=0 ;;
        --timeout) timeout=${2:?--timeout SEC}; shift ;;
        --out) out=${2:?--out DIR}; shift ;;
        --lenient) lenient=1 ;;
        --keys) keys=${2:?--keys SPEC|none}; shift ;;
        --input) input=${2:?--input FILE|none}; shift ;;
        --exec) plain=1 ;;
        --) shift; break ;;
        *) echo "usage: $0 [--no-build] [--timeout SEC] [--out DIR] [--lenient] [--keys SPEC|none] [--input FILE|none] [--exec] [-- victory.exe args]" >&2; exit 2 ;;
    esac
    shift
done
GAME_DIR="${GAME_DIR:-$GTA6_ROOT/GTA6.Xbox.v1.0.2.1}"
[ $# -gt 0 ] || set -- run --execute "$GAME_DIR/game_scarlett_master.exe"

WINEPREFIX="${WINEPREFIX:-$GTA6_ROOT/research/metal-play-prefix}"
case "$WINEPREFIX" in
    *wineprefix-play*) echo "refusing to use the D3DMetal baseline prefix $WINEPREFIX" >&2; exit 2 ;;
esac
export WINEPREFIX
WINE="$VICTORY_WINE_DIR/bin/wine"
[ -x "$WINE" ] || { echo "no Wine at $WINE (see tools/metal/env.sh)" >&2; exit 1; }
WINESERVER="$VICTORY_WINE_DIR/bin/wineserver"
STAGE="${VICTORY_METAL_OUT:-$ROOT/metal/target/wine-stage}"

# victory.toml: outside-the-repo paths (the repository refuses paths inside it).
if [ ! -f "$ROOT/victory.toml" ]; then
    cat >"$ROOT/victory.toml" <<TOML
[paths]
game_dir = '$GAME_DIR'
cache_dir = '$GTA6_ROOT/victory-data/cache'
data_dir = '$GTA6_ROOT/victory-data/data'
test_titles = []
TOML
    echo "wrote $ROOT/victory.toml (git-ignored)"
fi

[ "$build" = 1 ] && "$here/build-unixlib.sh" >/dev/null
[ -f "$STAGE/front/d3d12.dll" ] || { echo "no stage in $STAGE: run tools/metal/build-unixlib.sh" >&2; exit 1; }

# The prefix: made on first use, without Mono/Gecko prompts.
SYS32="$WINEPREFIX/drive_c/windows/system32"
if [ ! -d "$SYS32" ]; then
    echo "making the prefix $WINEPREFIX"
    WINEDLLOVERRIDES="mscoree,mshtml=;winedbg.exe=d" WINEDEBUG=-all "$WINE" wineboot -u >/dev/null 2>&1 || true
    WINEDLLOVERRIDES="mscoree,mshtml=;winedbg.exe=d" WINEDEBUG=-all "$WINE" cmd /c exit >/dev/null 2>&1 || true
    [ -d "$SYS32" ] || { echo "the prefix was not made" >&2; exit 1; }
fi
# Our dlls over the placeholders (kept once as *.metal-orig); this prefix is ours alone.
for n in d3d12 dxgi; do
    [ -f "$SYS32/$n.dll.metal-orig" ] || { [ -f "$SYS32/$n.dll" ] && cp "$SYS32/$n.dll" "$SYS32/$n.dll.metal-orig"; }
    cp "$STAGE/front/$n.dll" "$SYS32/$n.dll"
done
cp "$STAGE/x86_64-windows/victory_metal_bridge.dll" "$SYS32/victory_metal_bridge.dll"

export WINEDLLPATH="$STAGE${WINEDLLPATH:+:$WINEDLLPATH}"
export WINEDLLOVERRIDES="d3d12,dxgi=n,b"
export VICTORY_METAL_LOG=1
[ "$lenient" = 1 ] && export VICTORY_METAL_LENIENT=1
if [ "$keys" = none ]; then unset VICTORY_KEYS; else export VICTORY_KEYS="$keys"; fi
if [ "$input" = none ]; then unset VICTORY_INPUT_REPLAY; else export VICTORY_INPUT_REPLAY="$input"; fi
export VICTORY_PATCH_MASKED="${VICTORY_PATCH_MASKED:-all}"
export VICTORY_FAULT_TRACKING="${VICTORY_FAULT_TRACKING:-1}"
# upstream's script, untouched; it enters its own checkout (this worktree) and builds victory.exe there
VICTORY_SH="$ROOT/tools/macos/victory.sh"
WINE="$WINE"; export WINE

if [ "$plain" = 1 ]; then
    exec "$VICTORY_SH" "$@"
fi

[ -n "$out" ] || out="$GTA6_ROOT/research/game-runs/$(date +%Y%m%d-%H%M%S)-metal"
mkdir -p "$out"
# a Windows path for the Wine process: Z: is the Unix root
VICTORY_METAL_NOTIMPL_LOG="Z:$(printf '%s' "$out/notimpl.log" | tr / '\\')"
export VICTORY_METAL_NOTIMPL_LOG
: >"$out/notimpl.log"
echo "run: $* (limit ${timeout}s), logs in $out"
start=$(date +%s)
"$VICTORY_SH" "$@" >"$out/stdout.log" 2>"$out/stderr.log" &
pid=$!
if [ "$timeout" -gt 0 ]; then
    (
        sleep "$timeout"
        echo "== time limit of ${timeout}s reached: stopping this prefix's Wine" >>"$out/stderr.log"
        kill -TERM "$pid" 2>/dev/null || true
        WINEPREFIX="$WINEPREFIX" "$WINESERVER" -k >/dev/null 2>&1 || true
    ) &
    wd=$!
fi

status=0
wait "$pid" || status=$?
if [ -n "${wd:-}" ]; then
    pkill -P "$wd" 2>/dev/null || true
    kill "$wd" 2>/dev/null || true
fi
# whatever Wine left of THIS prefix
WINEPREFIX="$WINEPREFIX" "$WINESERVER" -k >/dev/null 2>&1 || true
end=$(date +%s)
{
    echo "command: $*"
    echo "seconds: $((end - start)) (limit $timeout); exit status: $status"
    echo
    echo "== the back's and the front's errors (first 40 distinct lines of stderr.log)"
    grep -a '^victory-metal' "$out/stderr.log" | sort | uniq -c | sort -rn | head -40
    echo
    echo "== unimplemented methods, by number of calls"
    "$here/notimpl-stats.sh" "$out/notimpl.log" 2>&1 | head -60
} >"$out/summary.txt"
cat "$out/summary.txt"
exit "$status"
