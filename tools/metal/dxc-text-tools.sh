#!/usr/bin/env bash
# Builds the two DXC text tools of the mesh-to-compute prototype
# (docs/metal/mesh-compute.md): `dxcdis` (DXIL container to LLVM text) and
# `dxcasm` (LLVM text to DXIL container). They are x86_64 programs that load the
# DXC library of D3DMetal (DXC_LIB, docs/metal/msc.md) and run under Rosetta.
# Their sources are the research repository's `attr-vertex/agx-crash/{dxcdis,dxcasm}.cpp`
# (C++ is not on the asset policy's allowlist, so they are not copied here).
#
#   tools/metal/dxc-text-tools.sh [OUT_DIR]      (default /tmp/victory-dxc-text)
#
# Then: export VICTORY_DXC_TEXT_TOOLS=OUT_DIR  (tests/mesh_compute.rs reads it).
set -euo pipefail
src="${GTA6_ROOT:-$HOME/Documents/GTA6Project}/research/attr-vertex/agx-crash"
out="${1:-/tmp/victory-dxc-text}"
mkdir -p "$out"
for t in dxcdis dxcasm; do
    clang++ -arch x86_64 -std=c++17 -O1 -o "$out/$t" "$src/$t.cpp" -ldl
done
echo "built $out/dxcdis and $out/dxcasm"
