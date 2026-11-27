#!/bin/sh
# ABI smoke test of the front (d3d12.dll / dxgi.dll) under Wine.
#
# Builds the dlls and front-smoke.exe for x86_64-pc-windows-gnullvm, then runs
# the exe twice in the scratch prefix research/m0-prefix (never wineprefix-play):
#   1. WINEDLLOVERRIDES=...d3d12,dxgi=b      Wine's builtin D3DMetal   -> gold
#   2. our dlls copied over system32 + d3d12,dxgi=n,b                  -> ours
# and diffs the "GOLD " lines. The recorded golden of the builtin run is
# metal/front/tests/golden/d3dmetal-wine11.txt; the live run is compared with
# it too. Our run uses `--ours` (adds bad-input cases D3DMetal cannot take) and
# the mock Link (VICTORY_METAL_LINK=mock): no GPU, no back.
#
# Known, accepted differences are listed in tools/metal/front-smoke.known-diff
# (one exact GOLD line per row, "- " builtin side / "+ " ours).
# Never run next to another Wine test: tools/macos/victory.sh kills the wineserver.
# Usage: tools/metal/test-front-smoke.sh [--no-build] [--keep]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$here/../.." && pwd)
. "$here/env.sh"

build=1
for a in "$@"; do
    case "$a" in
        --no-build) build=0 ;;
        --keep) ;;
        *) echo "usage: $0 [--no-build]" >&2; exit 2 ;;
    esac
done

T=x86_64-pc-windows-gnullvm
REL="$ROOT/metal/target/$T/release"
if [ "$build" = 1 ]; then
    (cd "$ROOT/metal" && cargo build --release --locked --target $T \
        -p victory-metal-front-d3d12 -p victory-metal-front-dxgi -p victory-metal-front-smoke)
fi
for f in d3d12.dll dxgi.dll front-smoke.exe; do
    [ -f "$REL/$f" ] || { echo "missing $REL/$f" >&2; exit 1; }
done

WINE="${VICTORY_WINE_DIR:?set VICTORY_WINE_DIR}/bin/wine"
export WINEPREFIX="${WINEPREFIX:-$GTA6_ROOT/research/m0-prefix}"
export WINEDEBUG=-all
export ROSETTA_ADVERTISE_AVX=1
SYS32="$WINEPREFIX/drive_c/windows/system32"
WORK=$(mktemp -d)
trap 'restore; rm -rf "$WORK"' EXIT

# Make sure the prefix exists.
BASE="mscoree,mshtml=;winedbg.exe=d"
WINEDLLOVERRIDES="$BASE;d3d12,d3d12core,dxgi=b" "$WINE" cmd /c exit >/dev/null 2>&1 || true

restore() {
    for n in d3d12 dxgi; do
        if [ -f "$SYS32/$n.dll.m1-orig" ]; then
            rm -f "$SYS32/$n.dll"
            mv "$SYS32/$n.dll.m1-orig" "$SYS32/$n.dll"
        fi
    done
}

cp "$REL/front-smoke.exe" "$WORK/front-smoke.exe"
TWINE_EXE=$(WINEDLLOVERRIDES="$BASE;d3d12,d3d12core,dxgi=b" "$WINE" winepath -w "$WORK/front-smoke.exe" 2>/dev/null | tr -d '\r')

echo "== run 1: Wine builtin (D3DMetal)"
WINEDLLOVERRIDES="$BASE;d3d12,d3d12core,dxgi=b" "$WINE" "$TWINE_EXE" >"$WORK/gold.out" 2>"$WORK/gold.err" || echo "(exit status $?)"
grep '^GOLD ' "$WORK/gold.out" >"$WORK/gold.txt" || true

echo "== run 2: our dlls"
for n in d3d12 dxgi; do
    [ -f "$SYS32/$n.dll" ] && mv "$SYS32/$n.dll" "$SYS32/$n.dll.m1-orig"
    cp "$REL/$n.dll" "$SYS32/$n.dll"
done
VICTORY_METAL_LINK=mock WINEDLLOVERRIDES="$BASE;d3d12,dxgi=n,b" "$WINE" "$TWINE_EXE" --ours >"$WORK/ours.out" 2>"$WORK/ours.err" || echo "(exit status $?)"
restore
grep '^GOLD ' "$WORK/ours.out" >"$WORK/ours.txt" || true

echo "== INFO lines of our run"
grep '^INFO ' "$WORK/ours.out" || true

G="$ROOT/metal/front/tests/golden/d3dmetal-wine11.txt"
# The live builtin run is compared with the recorded golden: a Wine or macOS update shows up here.
grep '^GOLD ' "$G" >"$WORK/recorded.txt"
if ! diff -u "$WORK/recorded.txt" "$WORK/gold.txt" >"$WORK/rec.diff"; then
    echo "NOTE: the live builtin run differs from the recorded golden (environment changed?):"
    cat "$WORK/rec.diff"
fi

KNOWN="$here/front-smoke.known-diff"
echo "== diff builtin vs ours (GOLD lines)"
# Lines that exist only in --ours (bad-input cases) are not in the builtin run.
diff "$WORK/gold.txt" "$WORK/ours.txt" | grep '^[<>]' | sed 's/^< /- /; s/^> /+ /' >"$WORK/diff.txt" || true
if [ -f "$KNOWN" ]; then
    grep -vxFf "$KNOWN" "$WORK/diff.txt" >"$WORK/unexpected.txt" || true
else
    cp "$WORK/diff.txt" "$WORK/unexpected.txt"
fi
total=$(wc -l <"$WORK/ours.txt" | tr -d ' ')
echo "ours: $total GOLD lines, $(wc -l <"$WORK/diff.txt" | tr -d ' ') differ, $(wc -l <"$WORK/unexpected.txt" | tr -d ' ') not in known-diff"
if [ -s "$WORK/unexpected.txt" ]; then
    cat "$WORK/unexpected.txt"
    echo "stderr of our run:"; head -40 "$WORK/ours.err"
    echo "FAIL"
    exit 1
fi
echo "PASS"
