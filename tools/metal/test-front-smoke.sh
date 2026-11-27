#!/bin/sh
# ABI smoke test of the front (d3d12.dll / dxgi.dll) under Wine.
#
# Builds the dlls and front-smoke.exe (tools/metal/build-unixlib.sh stages them) and
# runs the exe twice in the scratch prefix research/m0-prefix (never wineprefix-play):
#   1. WINEDLLOVERRIDES=...d3d12,dxgi=b      Wine's builtin D3DMetal   -> gold
#   2. our dlls copied over system32 + d3d12,dxgi=n,b                  -> ours
# and diffs the "GOLD " lines. The recorded golden of the builtin run is
# metal/front/tests/golden/d3dmetal-wine11.txt; the live run is compared with
# it too. Our run uses `--ours` (adds bad-input cases D3DMetal cannot take).
#
# Two modes for our run:
#   (default)  VICTORY_METAL_LINK=mock: the front's mock Link, no GPU, no back
#   --real     no mock: the front goes through the bridge PE, the Wine unixlib and
#              the Metal back (WINEDLLPATH=<stage>); then front-e2e.exe (real copies,
#              barriers, fences, checked against CPU models) runs on that stack too.
#              Differences that only the real back shows go in
#              tools/metal/front-smoke-real.known-diff.
#
# Known, accepted differences are listed in tools/metal/front-smoke.known-diff
# (one exact GOLD line per row, "- " builtin side / "+ " ours).
# Never run next to another Wine test in the same prefix (a lock refuses).
# Usage: tools/metal/test-front-smoke.sh [--no-build] [--real]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck disable=SC1091
. "$here/wine-lib.sh"

build=1
real=0
for a in "$@"; do
    case "$a" in
        --no-build) build=0 ;;
        --real) real=1 ;;
        *) echo "usage: $0 [--no-build] [--real]" >&2; exit 2 ;;
    esac
done

metal_wine_init
[ "$build" = 1 ] && "$here/build-unixlib.sh" >/dev/null
WORK=$(mktemp -d)
METAL_EXTRA_CLEANUP='rm -rf "$WORK"'
[ -f "$STAGE/front-smoke.exe" ] || { echo "missing $STAGE/front-smoke.exe" >&2; exit 1; }
EXE="$STAGE/front-smoke.exe"

echo "== run 1: Wine builtin (D3DMetal)"
WINEDLLOVERRIDES="$(metal_overrides d3dmetal)" "$WINE" "$EXE" >"$WORK/gold.out" 2>"$WORK/gold.err" || echo "(exit status $?)"
grep '^GOLD ' "$WORK/gold.out" >"$WORK/gold.txt" || true

if [ "$real" = 1 ]; then
    echo "== run 2: our dlls, real back (bridge -> unixlib -> Metal 4)"
else
    echo "== run 2: our dlls, mock Link"
fi
metal_install_front
if [ "$real" = 1 ]; then
    WINEDLLPATH="$STAGE" WINEDLLOVERRIDES="$(metal_overrides metal)" "$WINE" "$EXE" --ours >"$WORK/ours.out" 2>"$WORK/ours.err" || echo "(exit status $?)"
else
    VICTORY_METAL_LINK=mock WINEDLLOVERRIDES="$(metal_overrides metal)" "$WINE" "$EXE" --ours >"$WORK/ours.out" 2>"$WORK/ours.err" || echo "(exit status $?)"
fi
metal_restore_front
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
KNOWN_REAL="$here/front-smoke-real.known-diff"
echo "== diff builtin vs ours (GOLD lines)"
# Lines that exist only in --ours (bad-input cases) are not in the builtin run.
diff "$WORK/gold.txt" "$WORK/ours.txt" | grep '^[<>]' | sed 's/^< /- /; s/^> /+ /' >"$WORK/diff.txt" || true
cp "$WORK/diff.txt" "$WORK/unexpected.txt"
for k in "$KNOWN" "$( [ "$real" = 1 ] && echo "$KNOWN_REAL" )"; do
    if [ -n "$k" ] && [ -f "$k" ]; then
        grep -vxFf "$k" "$WORK/unexpected.txt" >"$WORK/unexpected.next" || true
        mv "$WORK/unexpected.next" "$WORK/unexpected.txt"
    fi
done
total=$(wc -l <"$WORK/ours.txt" | tr -d ' ')
echo "ours: $total GOLD lines, $(wc -l <"$WORK/diff.txt" | tr -d ' ') differ, $(wc -l <"$WORK/unexpected.txt" | tr -d ' ') not in known-diff"
status=0
if [ -s "$WORK/unexpected.txt" ]; then
    cat "$WORK/unexpected.txt"
    echo "stderr of our run:"; head -40 "$WORK/ours.err"
    status=1
fi

if [ "$real" = 1 ]; then
    echo "== front-e2e.exe on our dlls, real back"
    metal_install_front
    WINEDLLPATH="$STAGE" WINEDLLOVERRIDES="$(metal_overrides metal)" "$WINE" "$STAGE/front-e2e.exe" --no-perf >"$WORK/e2e.out" 2>"$WORK/e2e.err" || true
    metal_restore_front
    grep -E '^(E2E|INFO)' "$WORK/e2e.out" | grep -v '^E2E PASS' || true
    grep -q '^E2E SUMMARY .*fail=0 ' "$WORK/e2e.out" || { status=1; echo "front-e2e failed"; head -20 "$WORK/e2e.err"; }
fi
[ "$status" = 0 ] && echo "PASS" || echo "FAIL"
exit "$status"
