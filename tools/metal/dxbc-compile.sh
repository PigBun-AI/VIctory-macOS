#!/bin/sh
# usage: tools/metal/dxbc-compile.sh <hlsl file> <out file> <profile, e.g. cs_5_1>
# HLSL to DXBC the way the game layer's runtime shaders are made: D3DCompile in
# Wine. Builds a small exe with the repository's llvm-mingw on first use
# (target/ of the VIctory-macOS checkout, or DXBC_TOOL_DIR), then runs it under
# the checkout's Wine. Used by the backend tests through VICTORY_DXBC_TOOL
# (they print SKIPPED without it); the output is a build product, not stored.
# Environment: VIC_MAC (the VIctory-macOS checkout, default
# ~/Documents/GTA6Project/VIctory-macOS), WINEPREFIX (default: a private one
# under the temp folder, created on first use).
set -e
here="$(cd "$(dirname "$0")" && pwd)"
mac="${VIC_MAC:-$HOME/Documents/GTA6Project/VIctory-macOS}"
tool_dir="${DXBC_TOOL_DIR:-${TMPDIR:-/tmp}/victory-dxbc-tool}"
exe="$tool_dir/dxbc-compile.exe"
if [ ! -f "$exe" ] || [ "$here/dxbc-compile.c" -nt "$exe" ]; then
    mkdir -p "$tool_dir"
    mingw=$(ls -d "$mac"/target/toolchain/llvm-mingw-*/ | head -1)
    "$mingw/bin/x86_64-w64-mingw32-clang" -O1 "$here/dxbc-compile.c" -o "$exe" -ld3dcompiler_47
fi
export WINEPREFIX="${WINEPREFIX:-$tool_dir/prefix}"
export WINEDEBUG="${WINEDEBUG:--all}"
export WINEDLLOVERRIDES="mscoree,mshtml=;winedbg.exe=d"
exec "$mac/target/wine-cx/bin/wine" "$exe" "$1" "$2" "$3"
