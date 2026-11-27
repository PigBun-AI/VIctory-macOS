#!/bin/sh
# Runs the tests of this checkout under CrossOver Wine + D3DMetal (it calls
# tools/macos/test-wine.sh) with what the helper-shader tests need:
#
#   tools/mac/wine-tests.sh [cargo test arguments]     e.g.  -p victory-gpu
#
# - The DXC front end over D3DMetal's libdxcompiler.dylib (tools/macos/dxc-d3dmetal.sh) is
#   built once into target/dxc-d3dmetal and named by VICTORY_DXC_HELPER / DXC_LIB /
#   VICTORY_DXC_DIR (results kept in target/dxc-cache), so that `mac::hlsl::d3d_compile`
#   (docs/mac/PATCHES.md, G4) compiles the runtime's helper shaders. Without it Wine's
#   d3dcompiler_47 fails on them and ~8 tests of victory-gpu fail (indirect clamp, ambient, ...).
#   VICTORY_MAC_HLSL=0 turns it off.
# - Its own Wine prefix, target/wineprefix-tests (WINEPREFIX overrides it). It never uses
#   target/wineprefix-play, and it only stops the wineserver of the prefix it used: other
#   prefixes may be running a game.
# - Wine: VICTORY_MAC_WINE_DIR, else ../VIctory-macOS/target/wine-cx next to this checkout, else
#   what tools/macos/test-wine.sh finds.
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
[ -f tools/mac/env.sh ] && . tools/mac/env.sh
WDIR="${VICTORY_MAC_WINE_DIR:-$ROOT/../VIctory-macOS/target/wine-cx}"
[ -x "$WDIR/bin/wine" ] || WDIR="$HOME/Library/Caches/victory-macos/target/wine-cx"
[ -x "$WDIR/bin/wine" ] && export WINE="${WINE:-$WDIR/bin/wine}"
export WINEPREFIX="${WINEPREFIX:-$ROOT/target/wineprefix-tests}"
case "$WINEPREFIX" in
    *wineprefix-play*) echo "wine-tests: refusing $WINEPREFIX (the prefix a game may be using)" >&2; exit 2 ;;
esac
DXC_LIB="$WDIR/lib/external/D3DMetal.framework/Versions/A/Resources/libdxcompiler.dylib"
if [ "${VICTORY_MAC_HLSL:-1}" != 0 ] && [ -f "$DXC_LIB" ]; then
    helper="$ROOT/target/dxc-d3dmetal"
    if [ ! -x "$helper" ] || [ "$ROOT/tools/macos/dxc-d3dmetal.sh" -nt "$helper" ]; then
        sh "$ROOT/tools/macos/dxc-d3dmetal.sh" "$helper" || helper=""
    fi
    if [ -n "$helper" ]; then
        mkdir -p "$ROOT/target/dxc-cache"
        export DXC_LIB VICTORY_DXC_HELPER="$helper" VICTORY_DXC_DIR="$ROOT/target/dxc-cache"
    fi
fi
sh tools/macos/test-wine.sh "$@"
rc=$?
if [ -x "$WDIR/bin/wineserver" ]; then "$WDIR/bin/wineserver" -k 2>/dev/null; fi
exit $rc
