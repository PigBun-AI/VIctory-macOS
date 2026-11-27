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
