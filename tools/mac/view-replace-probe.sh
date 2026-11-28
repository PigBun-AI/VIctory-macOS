#!/bin/sh
# Shows that Wine on macOS (Rosetta) loses a running thread's data when the view it uses is unmapped and mapped again
# (mac.hle.xmem_map_partial_view), with no code of ours, and that native macOS does not. CPU only: no game, no GPU.
#
#   tools/mac/view-replace-probe.sh [iterations]        (default 2000 per run, 3 runs per mode)
#
# Builds tools/mac/rosetta-probe/view-replace.c as a Windows exe (llvm-mingw) and runs it under Wine in its own prefix,
# target/wineprefix-view-replace (only that prefix's wineserver is stopped), modes 0 (the HLE's), 2 and 1 (control);
# then the native control (arm64, and x86_64 under Rosetta without Wine). Prints one "done ..." line per run.
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
. tools/mac/env.sh
N="${1:-2000}"
case "$ROOT" in */worktrees/mac/*) _g="$ROOT/../../.." ;; *) _g="$ROOT/.." ;; esac
WDIR="${VICTORY_MAC_WINE_DIR:-$_g/VIctory-macOS/target/wine-cx}"
[ -x "$WDIR/bin/wine" ] || WDIR="$HOME/Library/Caches/victory-macos/target/wine-cx"
WINE="${WINE:-$WDIR/bin/wine}"
[ -x "$WINE" ] || { echo "view-replace-probe: no wine at $WINE" >&2; exit 1; }
export WINEPREFIX="${WINEPREFIX:-$ROOT/target/wineprefix-view-replace}" WINEDEBUG=-all WINEDLLOVERRIDES="mscoree,mshtml=;winedbg.exe=d" ROSETTA_ADVERTISE_AVX=1
case "$WINEPREFIX" in *wineprefix-play*|*metal-play*) echo "view-replace-probe: refusing $WINEPREFIX" >&2; exit 2 ;; esac
OUT="$ROOT/target/view-replace"; mkdir -p "$OUT"
x86_64-w64-mingw32-clang -O1 -static -o "$OUT/view-replace.exe" tools/mac/rosetta-probe/view-replace.c || exit 1
echo "# wine: $("$WINE" --version 2>/dev/null), load: $(uptime | sed 's/.*load averages*: //')"
for mode in 0 2 1; do
    for i in 1 2 3; do echo "mode $mode: $("$WINE" "$OUT/view-replace.exe" "$N" "$mode" 2>&1 </dev/null | tr -d '\r' | tail -1)"; done
done
"$(dirname "$WINE")/wineserver" -k >/dev/null 2>&1
/usr/bin/clang -O1 -DNATIVE -o "$OUT/view-replace-arm64" tools/mac/rosetta-probe/view-replace.c -lpthread &&
    for i in 1 2 3; do echo "native arm64: $("$OUT/view-replace-arm64" "$N" | tail -1)"; done
/usr/bin/clang -arch x86_64 -O1 -DNATIVE -o "$OUT/view-replace-x86" tools/mac/rosetta-probe/view-replace.c -lpthread &&
    for i in 1 2 3; do echo "native x86_64 (Rosetta): $(arch -x86_64 "$OUT/view-replace-x86" "$N" | tail -1)"; done
exit 0
