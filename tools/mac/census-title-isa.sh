#!/bin/sh
# Static census of the title's instruction classes (Rosetta-relevant), per class and per function.
# usage: tools/mac/census-title-isa.sh [image] [outdir]
# image  default: the cached mapped title image victory-image-b99f876b74d4a77f.dll (build 1.0.2.1) under
#        $VICTORY_CACHE_DIR/images or data/shared/cache/images
# outdir default: $HOME/Documents/GTA6Project/research/census/title-isa-<date> (outside the repo: game-derived)
# Needs llvm-objdump (llvm-mingw's from VIctory-macOS/target/toolchain, or on PATH) and python3. The game directory is only read.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$HOME/Documents/GTA6Project
img=${1:-${VICTORY_CACHE_DIR:-$root/data/shared/cache}/images/victory-image-b99f876b74d4a77f.dll}
out=${2:-$root/research/census/title-isa-$(date +%Y%m%d)}
objdump=$(command -v llvm-objdump || ls "$root"/VIctory-macOS/target/toolchain/llvm-mingw*/bin/llvm-objdump | head -1)
[ -f "$img" ] || { echo "no image $img" >&2; exit 1; }
"$objdump" -d --x86-asm-syntax=intel -j .text "$img" | python3 "$here/census-title-isa.py" "$img" "$out"
