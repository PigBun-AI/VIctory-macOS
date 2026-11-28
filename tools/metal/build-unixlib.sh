#!/bin/sh
# Builds the Metal backend pieces and lays them out the way Wine finds a dll
# with a unixlib:
#
#   $OUT/x86_64-windows/victory_metal_bridge.dll   bridge PE, marked "Wine builtin DLL"
#   $OUT/x86_64-unix/victory_metal_bridge.so       bridge unixlib: x86_64 Mach-O with the
#                                                  Metal back inside (Wine runs under Rosetta)
#   $OUT/front/d3d12.dll, $OUT/front/dxgi.dll      our front, plain PE: copy into a prefix's
#                                                  system32 and set WINEDLLOVERRIDES=d3d12,dxgi=n,b
#                                                  (tools/metal/test-wine.sh, test-front-smoke.sh do)
#   $OUT/m0-probe.exe, front-smoke.exe, front-e2e.exe   the PEs that call them
#
# Run with WINEDLLPATH=$OUT: the front finds victory_metal_bridge.dll (and
# through it the unixlib) there. No C and no winegcc are needed: both halves
# are Rust, and winebuild only sets the builtin mark on the PE.
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

# PE half and the probe, with the gnullvm linker (env.sh sets CARGO_TARGET_..._LINKER to tools/macos/gnullvm-cc.sh; mac/livetr has no .cargo/config.toml entry for it).
cargo build --release --locked --target x86_64-pc-windows-gnullvm \
    -p victory-metal-bridge -p victory-metal-probe \
    -p victory-metal-front-d3d12 -p victory-metal-front-dxgi \
    -p victory-metal-front-smoke -p victory-metal-front-e2e \
    -p victory-metal-front-present
# Unixlib: x86_64, because Wine runs under Rosetta and loads x86_64 code.
cargo build --release --locked --target x86_64-apple-darwin -p victory-metal-bridge

mkdir -p "$OUT/x86_64-windows" "$OUT/x86_64-unix" "$OUT/front"
cp "$PE/victory_metal_bridge.dll" "$OUT/x86_64-windows/victory_metal_bridge.dll"
# Unmarked copy: used as a native stand-in for d3d12/dxgi in the override test.
cp "$PE/victory_metal_bridge.dll" "$OUT/standin.dll"
"$WINEBUILD" --builtin "$OUT/x86_64-windows/victory_metal_bridge.dll"
cp "$MACHO/libvictory_metal_bridge.dylib" "$OUT/x86_64-unix/victory_metal_bridge.so"
cp "$PE/m0-probe.exe" "$OUT/m0-probe.exe"
cp "$PE/d3d12.dll" "$PE/dxgi.dll" "$OUT/front/"
cp "$PE/front-smoke.exe" "$PE/front-e2e.exe" "$PE/front-present.exe" "$OUT/"
# CAPS: what this stage's back can do, one capability per line (docs/metal/mesh-compute.md, 8.10). tools/mac/run.sh
# turns the lines into environment variables the translator's host hooks read, so an old stage run with a new
# exe (or the other way round) does not get what it cannot take. Written last, so a half-made stage has none.
# mesh_compute: the back builds the emulation of a mesh shader from the VMCS part (mesh_emu.rs, interface v1).
printf '%s\n' mesh_compute > "$OUT/CAPS"

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
# The front must export what victory-gpu loads by name.
for sym in D3D12CreateDevice D3D12SerializeRootSignature D3D12SerializeVersionedRootSignature; do
    strings -a "$OUT/front/d3d12.dll" | grep -qx "$sym" || { echo "front d3d12.dll does not export $sym" >&2; exit 1; }
done
for sym in CreateDXGIFactory CreateDXGIFactory1 CreateDXGIFactory2; do
    strings -a "$OUT/front/dxgi.dll" | grep -qx "$sym" || { echo "front dxgi.dll does not export $sym" >&2; exit 1; }
done
echo "staged in $OUT"
