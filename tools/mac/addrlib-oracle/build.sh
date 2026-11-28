#!/bin/sh
# Builds the addrlib oracle (oracle.cpp) against Mesa's src/amd/addrlib (MIT).
#   tools/mac/addrlib-oracle/build.sh [<mesa checkout>] [<out binary>]
# Without a checkout it makes a sparse one of mesa's main branch in $TMPDIR/mesa-addrlib (needs the network).
# The commit used is printed; the golden tables in crates/victory-formats say which one they came from.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
mesa=${1:-${TMPDIR:-/tmp}/mesa-addrlib/mesa}
out=${2:-${TMPDIR:-/tmp}/addrlib-oracle}
if [ ! -d "$mesa/src/amd/addrlib" ]; then
    mkdir -p "$(dirname "$mesa")"
    git clone --depth 1 --filter=blob:none --sparse https://gitlab.freedesktop.org/mesa/mesa.git "$mesa"
    git -C "$mesa" sparse-checkout set src/amd/addrlib
fi
git -C "$mesa" log -1 --format='mesa %H' 2>/dev/null || true
A="$mesa/src/amd/addrlib"
c++ -std=c++17 -O2 -DLITTLEENDIAN_CPU -DHAVE_TSERVER -DADDR_FASTCALL= -DADDR_ALLOW_SIMD=1 -DDEBUG=0 -w \
    -I"$A/inc" -I"$A/src" -I"$A/src/core" -I"$A/src/gfx9" -I"$A/src/gfx10" -I"$A/src/gfx11" -I"$A/src/gfx12" \
    -I"$A/src/r800" -I"$A/src/chip/gfx9" -I"$A/src/chip/gfx10" -I"$A/src/chip/gfx11" -I"$A/src/chip/gfx12" -I"$A/src/chip/r800" \
    "$here/oracle.cpp" "$A"/src/addrinterface.cpp "$A"/src/core/*.cpp "$A"/src/gfx9/*.cpp "$A"/src/gfx10/*.cpp \
    "$A"/src/gfx11/*.cpp "$A"/src/gfx12/*.cpp "$A"/src/r800/*.cpp -o "$out"
echo "built $out"
