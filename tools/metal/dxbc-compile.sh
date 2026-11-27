#!/bin/sh
# usage: tools/metal/dxbc-compile.sh <hlsl file> <out file> <profile, e.g. cs_5_1>
# HLSL to DXBC the way the game layer's runtime shaders are made: D3DCompile in
# Wine. Builds a small exe with the repository's llvm-mingw on first use
# (DXBC_TOOL_DIR, default a folder under the temp dir; the C source is in this
# script), then runs it under the checkout's Wine. Used by the backend tests through VICTORY_DXBC_TOOL
# (they print SKIPPED without it); the output is a build product, not stored.
# Environment: VIC_MAC (the VIctory-macOS checkout, default
# ~/Documents/GTA6Project/VIctory-macOS), WINEPREFIX (default: a private one
# under the temp folder, created on first use).
set -e
mac="${VIC_MAC:-$HOME/Documents/GTA6Project/VIctory-macOS}"
tool_dir="${DXBC_TOOL_DIR:-${TMPDIR:-/tmp}/victory-dxbc-tool}"
exe="$tool_dir/dxbc-compile.exe"
if [ ! -f "$exe" ] || [ "$0" -nt "$exe" ]; then
    mkdir -p "$tool_dir"
    mingw=$(ls -d "$mac"/target/toolchain/llvm-mingw-*/ | head -1)
    cat > "$tool_dir/dxbc-compile.c" <<'EOF'
// HLSL file -> DXBC file with D3DCompile (Wine's d3dcompiler_47, vkd3d-shader),
// for the backend's DXBC tests. Built and run by tools/metal/dxbc-compile.sh.
// usage: dxbc-compile.exe <hlsl> <out> <profile>   (entry point `main`)
#include <windows.h>
#include <d3dcompiler.h>
#include <stdio.h>
#include <string.h>
int main(int argc, char **argv) {
    if (argc < 4) return 2;
    FILE *f = fopen(argv[1], "rb"); if (!f) return 3;
    static char src[65536]; size_t n = fread(src, 1, sizeof src, f); fclose(f);
    ID3DBlob *code = NULL, *err = NULL;
    HRESULT hr = D3DCompile(src, n, "t.hlsl", NULL, NULL, "main", argv[3], 0, 0, &code, &err);
    if (FAILED(hr)) {
        printf("D3DCompile failed %08lx: %.*s\n", hr, err ? (int)err->lpVtbl->GetBufferSize(err) : 0, err ? (char*)err->lpVtbl->GetBufferPointer(err) : "");
        return 1;
    }
    f = fopen(argv[2], "wb");
    fwrite(code->lpVtbl->GetBufferPointer(code), 1, code->lpVtbl->GetBufferSize(code), f);
    fclose(f);
    printf("ok %zu bytes\n", (size_t)code->lpVtbl->GetBufferSize(code));
    return 0;
}
EOF
    "$mingw/bin/x86_64-w64-mingw32-clang" -O1 "$tool_dir/dxbc-compile.c" -o "$exe" -ld3dcompiler_47
fi
export WINEPREFIX="${WINEPREFIX:-$tool_dir/prefix}"
export WINEDEBUG="${WINEDEBUG:--all}"
export WINEDLLOVERRIDES="mscoree,mshtml=;winedbg.exe=d"
exec "$mac/target/wine-cx/bin/wine" "$exe" "$1" "$2" "$3"
