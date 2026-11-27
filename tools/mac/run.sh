#!/bin/bash
# Runs a victory.exe on this Mac under CrossOver Wine + D3DMetal with scripted
# keys, and keeps logs. Not a build step (tools/mac/check.sh builds).
#
#   tools/mac/run.sh [--exe PATH] [--out DIR] [--timeout SEC] [--cache DIR] [--keys SPEC]
#                    [--tracking 0|1] [--prefix DIR] [--no-keys]
#
# Defaults: this checkout's release exe, out = ./target/mac-runs/<time>, timeout 900 s,
# data = $VICTORY_DATA_DIR, else victory-data-livetr/data next to this checkout; cache = the
# `cache` folder beside it, or --cache (outside the repository: derived game data never
# goes in it), keys = Enter every 3 s from 8 s
# (VICTORY_KEYS takes `<seconds>:<key>,...` or `<from>-<to>:<key>`), prefix =
# target/wineprefix-livetr (copy a working one there first, or let wineboot make it).
# Wine: VICTORY_MAC_WINE_DIR, else ../VIctory-macOS/target/wine-cx next to this checkout.
# Builds (once) the DXC front end tools/macos/dxc-d3dmetal.sh and sets
# VICTORY_DXC_HELPER / DXC_LIB / VICTORY_DXC_DIR for the helper shaders (docs/mac/PATCHES.md, G4).
# Only the Wine processes of the prefix are stopped, never other prefixes.
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
G="${GTA6_ROOT:-$(cd "$ROOT/.." && pwd)}"
WDIR="${VICTORY_MAC_WINE_DIR:-$G/VIctory-macOS/target/wine-cx}"
exe="$ROOT/target/x86_64-pc-windows-gnullvm/release/victory.exe"
out="$ROOT/target/mac-runs/$(date +%Y%m%d-%H%M%S)"; tmo=900
cache=""; prefix="$ROOT/target/wineprefix-livetr"; tracking=""
keys=$(python3 -c "print(','.join(f'{t}:Enter' for t in range(8,3600,3)))")
while [ $# -gt 0 ]; do
    case "$1" in
        --exe) exe=$2; shift ;; --out) out=$2; shift ;; --timeout) tmo=$2; shift ;;
        --cache) cache=$2; shift ;; --keys) keys=$2; shift ;; --no-keys) keys="" ;;
        --tracking) tracking=$2; shift ;; --prefix) prefix=$2; shift ;;
        *) echo "usage: see the head of $0" >&2; exit 2 ;;
    esac
    shift
done
if [ -n "${VICTORY_DATA_DIR:-}" ]; then data="$VICTORY_DATA_DIR"
elif [ -n "$cache" ]; then data="$(dirname "$cache")/data"
else data="$G/victory-data-livetr/data"; fi
[ -n "$cache" ] || cache="$(dirname "$data")/cache"
WINE="$WDIR/bin/wine"; SERVER="$WDIR/bin/wineserver"
[ -x "$WINE" ] || { echo "no Wine at $WINE" >&2; exit 1; }
[ -f "$exe" ] || { echo "no exe $exe" >&2; exit 1; }
export WINEPREFIX="$prefix" WINEDEBUG="${WINEDEBUG:--all}"
export WINEDLLOVERRIDES="mscoree,mshtml=;winedbg.exe=d;d3d12,d3d12core,dxgi=b"
export ROSETTA_ADVERTISE_AVX=1 VICTORY_SKIP_CPU_CHECK=1
export VICTORY_GAME_DIR="${GAME_DIR:-$G/GTA6.Xbox.v1.0.2.1}"
export VICTORY_CACHE_DIR="$cache" VICTORY_DATA_DIR="$data"
export VICTORY_PATCH_MASKED="${VICTORY_PATCH_MASKED:-all}"
[ -n "$tracking" ] && export VICTORY_FAULT_TRACKING="$tracking"
if [ -n "$keys" ]; then export VICTORY_KEYS="$keys"; else unset VICTORY_KEYS; fi
# DXC front end for the helper shaders (x86_64, run under Rosetta)
helper="$ROOT/target/dxc-d3dmetal"
if [ ! -x "$helper" ] || [ "$ROOT/tools/macos/dxc-d3dmetal.sh" -nt "$helper" ]; then
    sh "$ROOT/tools/macos/dxc-d3dmetal.sh" "$helper" || helper=""
fi
if [ -n "$helper" ]; then
    export VICTORY_DXC_HELPER="$helper" VICTORY_DXC_DIR="$(dirname "$cache")/dxc"
    export DXC_LIB="$WDIR/lib/external/D3DMetal.framework/Versions/A/Resources/libdxcompiler.dylib"
fi
mkdir -p "$out" "$VICTORY_DATA_DIR"
"$SERVER" -k 2>/dev/null; "$SERVER" -w 2>/dev/null
date +%FT%T >"$out/start.txt"
"$WINE" "$exe" run --execute "$VICTORY_GAME_DIR/game_scarlett_master.exe" >"$out/stdout.log" 2>"$out/stderr.log" &
pid=$!
( sleep "$tmo"; echo "== time limit ${tmo}s reached" >>"$out/stderr.log"; kill -TERM $pid 2>/dev/null; "$SERVER" -k 2>/dev/null ) &
wd=$!
wait $pid; st=$?
kill $wd 2>/dev/null; pkill -P $wd 2>/dev/null
"$SERVER" -k 2>/dev/null
{ date +%FT%T; echo "exit $st"; } >"$out/end.txt"
echo "logs in $out (exit $st)"
