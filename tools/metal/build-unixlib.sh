#!/bin/sh
# Builds the M0 pieces and lays them out the way Wine finds a dll with a
# unixlib:
#
#   $OUT/x86_64-windows/victory_metal_bridge.dll   PE, marked "Wine builtin DLL"
#   $OUT/x86_64-unix/victory_metal_bridge.so       x86_64 Mach-O unixlib
#   $OUT/m0-probe.exe                              the PE that calls it
#
# Run it with WINEDLLPATH=$OUT (tools/metal/run-m0.sh does). No C and no
# winegcc are needed: both halves are Rust, and winebuild only sets the
# builtin mark on the PE.
#
# Usage: tools/metal/build-unixlib.sh [OUT]       (default metal/target/wine-stage)
set -eu
here=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$here/../.." && pwd)
. "$here/env.sh"

OUT="${1:-${VICTORY_METAL_OUT:-$ROOT/metal/target/wine-stage}}"
WINEBUILD="${VICTORY_WINE_DIR:-}/bin/winebuild"
[ -x "$WINEBUILD" ] || { echo "no winebuild: set VICTORY_WINE_DIR to a Wine build (setup.sh makes target/wine-cx)" >&2; exit 1; }

cd "$ROOT/metal"
PE=target/x86_64-pc-windows-gnullvm/release
MACHO=target/x86_64-apple-darwin/release

# PE half and the probe, with the repository's gnullvm linker (.cargo/config.toml).
cargo build --release --locked --target x86_64-pc-windows-gnullvm \
    -p victory-metal-bridge -p victory-metal-probe
# Unixlib: x86_64, because Wine runs under Rosetta and loads x86_64 code.
cargo build --release --locked --target x86_64-apple-darwin -p victory-metal-bridge

mkdir -p "$OUT/x86_64-windows" "$OUT/x86_64-unix"
cp "$PE/victory_metal_bridge.dll" "$OUT/x86_64-windows/victory_metal_bridge.dll"
# Unmarked copy: used as a native stand-in for d3d12/dxgi in the override test.
cp "$PE/victory_metal_bridge.dll" "$OUT/standin.dll"
"$WINEBUILD" --builtin "$OUT/x86_64-windows/victory_metal_bridge.dll"
cp "$MACHO/libvictory_metal_bridge.dylib" "$OUT/x86_64-unix/victory_metal_bridge.so"
cp "$PE/m0-probe.exe" "$OUT/m0-probe.exe"

# Checks: the mark is there, and the table Wine looks up is exported.
if ! dd if="$OUT/x86_64-windows/victory_metal_bridge.dll" bs=1 skip=64 count=16 2>/dev/null | grep -q 'Wine builtin DLL'; then
    echo "victory_metal_bridge.dll is not marked as a Wine builtin" >&2
    exit 1
fi
if ! nm -gU "$OUT/x86_64-unix/victory_metal_bridge.so" | grep -q '___wine_unix_call_funcs$'; then
    echo "victory_metal_bridge.so does not export __wine_unix_call_funcs" >&2
    exit 1
fi
lipo -archs "$OUT/x86_64-unix/victory_metal_bridge.so" | grep -qx x86_64 || { echo "unixlib is not x86_64" >&2; exit 1; }
echo "staged in $OUT"
