#!/bin/sh
# Who reads the flags of a lock cmpxchg (matrix row mac.isa.lock_cmpxchg_flags): walks the code after each of the title's
# lock cmpxchg [mem] sites. Output outside the repo (game-derived). About 5 minutes, needs llvm-objdump and python3.
# usage: tools/mac/census-cmpxchg-flags.sh [image] [outdir]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
# a heavy job (objdump of the whole title image, python over it): only through heavy.sh (docs/mac/RUN.md "Heavy tasks")
[ -n "${VICTORY_HEAVY_SLOT:-}" ] || exec bash "$here/heavy.sh" --jobs 1 --label census-cmpxchg-flags -- sh "$here/census-cmpxchg-flags.sh" "$@"
root=$HOME/Documents/GTA6Project
img=${1:-${VICTORY_CACHE_DIR:-$root/data/shared/cache}/images/victory-image-b99f876b74d4a77f.dll}
out=${2:-$root/research/census/title-cmpxchg-flags-$(date +%Y%m%d)}
objdump=$(command -v llvm-objdump || ls "$root"/VIctory-macOS/target/toolchain/llvm-mingw*/bin/llvm-objdump | head -1)
[ -f "$img" ] || { echo "no image $img" >&2; exit 1; }
"$objdump" -d --x86-asm-syntax=intel -j .text "$img" | python3 "$here/census-cmpxchg-flags.py" "$out"
