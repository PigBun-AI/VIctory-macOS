#!/bin/sh
# Static call-site counts per import of the title (HLE frequency, static side). Output outside the repo.
# usage: tools/mac/census-title-imports.sh [exe] [image] [outdir]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$HOME/Documents/GTA6Project
exe=${1:-$root/GTA6.Xbox.v1.0.2.1/game_scarlett_master.exe}
img=${2:-$root/victory-data/cache/images/victory-image-b99f876b74d4a77f.dll}
out=${3:-$root/research/census/title-isa-$(date +%Y%m%d)}
objdump=$(command -v llvm-objdump || ls "$root"/VIctory-macOS/target/toolchain/llvm-mingw*/bin/llvm-objdump | head -1)
"$objdump" -d --x86-asm-syntax=intel -j .text "$img" | python3 "$here/census-title-imports.py" "$exe" "$out"
