#!/bin/sh
# Runs `victory replay` (docs/replay.md) of a frame capture on our Metal backend: the
# diagnostics victory.exe under Wine with our d3d12.dll / dxgi.dll, in the play prefix of this
# line (research/metal-play-prefix-livetr by default). The first step of the frame workbench
# (docs/metal/hot-reload.md).
#
#   tools/metal/replay.sh [--no-build] [--prefix DIR] [--out DIR] [--timeout SEC] CAPTURE [-- replay arguments]
#
# CAPTURE is a capture folder made by VICTORY_RECORD (outside the repository: game-derived data). The
# replay writes `replay.png` into it, so the script works on a copy-on-write clone of the folder
# (`cp -c`, APFS) under --out and leaves the original alone. stdout/stderr go to --out as well.
# Default replay arguments: --loops 3. Never touches another prefix's Wine.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$here/../.." && pwd)
# shellcheck disable=SC1091
. "$here/env.sh"

build=1; timeout=900; out=""; prefix=""
while [ $# -gt 0 ]; do
    case "$1" in
        --no-build) build=0 ;;
        --prefix) prefix=${2:?--prefix DIR}; shift ;;
        --out) out=${2:?--out DIR}; shift ;;
        --timeout) timeout=${2:?--timeout SEC}; shift ;;
        *) break ;;
    esac
    shift
done
cap=${1:?usage: replay.sh [options] CAPTURE [-- replay arguments]}; shift
[ "${1:-}" = -- ] && shift
[ $# -gt 0 ] || set -- --loops 3

WINEPREFIX="${prefix:-${WINEPREFIX:-$GTA6_ROOT/research/metal-play-prefix-livetr}}"
case "$WINEPREFIX" in
    *wineprefix-play*) echo "refusing to use the D3DMetal baseline prefix $WINEPREFIX" >&2; exit 2 ;;
esac
export WINEPREFIX
WINE="$VICTORY_WINE_DIR/bin/wine"; WINESERVER="$VICTORY_WINE_DIR/bin/wineserver"
[ -x "$WINE" ] || { echo "no Wine at $WINE" >&2; exit 1; }
STAGE="${VICTORY_METAL_OUT:-$ROOT/metal/target/wine-stage}"
TARGET=x86_64-pc-windows-gnullvm
EXE="$ROOT/target/$TARGET/diag/victory.exe"
if [ "$build" = 1 ]; then
    "$here/build-unixlib.sh" >/dev/null
    (cd "$ROOT" && cargo build --locked --profile diag -p victory --features diagnostics --target "$TARGET" >/dev/null)
fi
[ -f "$EXE" ] || { echo "no $EXE (cargo build --profile diag -p victory --features diagnostics)" >&2; exit 1; }
[ -f "$STAGE/front/d3d12.dll" ] || { echo "no stage in $STAGE: run tools/metal/build-unixlib.sh" >&2; exit 1; }

SYS32="$WINEPREFIX/drive_c/windows/system32"
[ -d "$SYS32" ] || { echo "no prefix $WINEPREFIX (tools/metal/run.sh makes it on first use)" >&2; exit 1; }
for n in d3d12 dxgi; do
    [ -f "$SYS32/$n.dll.metal-orig" ] || { [ -f "$SYS32/$n.dll" ] && cp "$SYS32/$n.dll" "$SYS32/$n.dll.metal-orig"; }
    cp "$STAGE/front/$n.dll" "$SYS32/$n.dll"
done
cp "$STAGE/x86_64-windows/victory_metal_bridge.dll" "$SYS32/victory_metal_bridge.dll"

[ -n "$out" ] || out="$GTA6_ROOT/research/captures/replays/$(date +%Y%m%d-%H%M%S)-metal"
mkdir -p "$out"
work="$out/capture"
[ -d "$work" ] || cp -c -R "$cap" "$work"
cap_dir="$work"
# `victory replay` takes the folder given as VICTORY_RECORD or a capture
[ -f "$work/frame.bin" ] || cap_dir=$(dirname "$(ls "$work"/capture-*/frame.bin | tail -n 1)")
winpath() { printf 'Z:%s' "$(printf '%s' "$1" | tr / '\\')"; }

# helper shaders: compiled by D3DMetal's DXC (docs/mac/PATCHES.md, G4), as tools/mac/run.sh does
helper="$ROOT/target/dxc-d3dmetal"
[ -x "$helper" ] || sh "$ROOT/tools/macos/dxc-d3dmetal.sh" "$helper" || helper=""
if [ -n "$helper" ]; then
    export VICTORY_DXC_HELPER="$helper" VICTORY_DXC_DIR="$GTA6_ROOT/victory-data-metal-livetr/dxc"
    export DXC_LIB="$VICTORY_WINE_DIR/lib/external/D3DMetal.framework/Versions/A/Resources/libdxcompiler.dylib"
    mkdir -p "$VICTORY_DXC_DIR"
fi
export WINEDEBUG="${WINEDEBUG:--all}"
export WINEDLLPATH="$STAGE${WINEDLLPATH:+:$WINEDLLPATH}"
export WINEDLLOVERRIDES="mscoree,mshtml=;winedbg.exe=d;d3d12,dxgi=n,b"
export ROSETTA_ADVERTISE_AVX=1 VICTORY_SKIP_CPU_CHECK=1 VICTORY_METAL_LOG="${VICTORY_METAL_LOG:-1}"
export VICTORY_GAME_DIR="${GAME_DIR:-$GTA6_ROOT/GTA6.Xbox.v1.0.2.1}"
export VICTORY_CACHE_DIR="$GTA6_ROOT/victory-data-metal-livetr/cache" VICTORY_DATA_DIR="$GTA6_ROOT/victory-data-metal-livetr/data"
export VICTORY_PATCH_MASKED="${VICTORY_PATCH_MASKED:-all}"
echo "replay: $cap_dir (limit ${timeout}s), logs in $out"
"$WINE" "$EXE" replay "$(winpath "$cap_dir")" "$@" >"$out/stdout.log" 2>"$out/stderr.log" &
pid=$!
(
    sleep "$timeout"
    echo "== time limit of ${timeout}s reached: stopping this prefix's Wine" >>"$out/stderr.log"
    kill -TERM "$pid" 2>/dev/null || true
    WINEPREFIX="$WINEPREFIX" "$WINESERVER" -k >/dev/null 2>&1 || true
) &
wd=$!
status=0
wait "$pid" || status=$?
pkill -P "$wd" 2>/dev/null || true; kill "$wd" 2>/dev/null || true
WINEPREFIX="$WINEPREFIX" "$WINESERVER" -k >/dev/null 2>&1 || true
echo "exit status $status; replay.png: $cap_dir/replay.png"
exit "$status"
