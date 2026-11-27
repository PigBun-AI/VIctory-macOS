#!/bin/sh
# Swap chain test through Wine: a window, a flip-model swap chain, a different
# clear colour per frame, a resize, and the drawable read back.
#
# Runs front-present.exe (metal/front/present) in a scratch prefix (never the
# game's), three ways on our dlls over the Metal back:
#
#   rgba      R8G8B8A8_UNORM buffers
#   bgra      B8G8R8A8_UNORM buffers, with a frame-latency waitable object
#
# with VICTORY_METAL_PROBE_PRESENT=1: our front then asks the back, after every
# present, for the texel in the middle of the drawable (read from the Metal
# drawable's texture), and prints it on stderr. This script checks that the
# texel and the size of every frame are the ones the exe said it painted
# ("EXPECT" lines), and that the exe's own checks (readback of the back buffer,
# GetBuffer/GetDesc1/ResizeBuffers semantics) passed. The same exe also runs on
# D3DMetal (--with-d3dmetal) to see that its checks are the API's, not ours.
#
#   --screenshot FILE   also run a window that holds one colour for ~15 s and
#                       take a screenshot of the screen into FILE (keep FILE
#                       outside the repository and delete it after looking);
#                       needs Screen Recording permission for the terminal.
#
# Usage: tools/metal/test-present.sh [--no-build] [--with-d3dmetal] [--screenshot FILE]
# Prefix: research/metal-test-prefix-present (WINEPREFIX overrides). A lock
# refuses two runs in one prefix.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck disable=SC1091
. "$here/wine-lib.sh"

build=1; d3dmetal=0; shot=""
while [ $# -gt 0 ]; do
    case "$1" in
        --no-build) build=0 ;;
        --with-d3dmetal) d3dmetal=1 ;;
        --screenshot) shot=${2:?--screenshot FILE}; shift ;;
        *) echo "usage: $0 [--no-build] [--with-d3dmetal] [--screenshot FILE]" >&2; exit 2 ;;
    esac
    shift
done

if [ -z "${WINEPREFIX:-}" ]; then
    . "$here/env.sh"
    WINEPREFIX="$GTA6_ROOT/research/metal-test-prefix-present"
fi
metal_wine_init
[ "$build" = 1 ] && "$here/build-unixlib.sh" >/dev/null
[ -f "$STAGE/front-present.exe" ] || { echo "missing $STAGE/front-present.exe" >&2; exit 1; }
OUTDIR=$(mktemp -d "${TMPDIR:-/tmp}/metal-present.XXXXXX")
WINESERVER="$(dirname "$WINE")/wineserver"
METAL_EXTRA_CLEANUP='WINEPREFIX="$WINEPREFIX" "$WINESERVER" -k >/dev/null 2>&1 || true; [ "${KEEP:-0}" = 1 ] || rm -rf "$OUTDIR"'

status=0
# run NAME BACKEND EXE-ARGS...
run() {
    name=$1; backend=$2; shift 2
    ov=$(metal_overrides "$backend")
    if [ "$backend" = metal ]; then
        WINEDLLPATH="$STAGE" WINEDLLOVERRIDES="$ov" VICTORY_METAL_PROBE_PRESENT=1 VICTORY_METAL_LOG=1 \
            "$WINE" "$STAGE/front-present.exe" "$@" >"$OUTDIR/$name.out" 2>"$OUTDIR/$name.err" || true
    else
        WINEDLLOVERRIDES="$ov" "$WINE" "$STAGE/front-present.exe" "$@" >"$OUTDIR/$name.out" 2>"$OUTDIR/$name.err" || true
    fi
    sum=$(grep '^PRESENT SUMMARY' "$OUTDIR/$name.out" || echo "PRESENT SUMMARY (none: the exe did not finish)")
    echo "== $name ($backend): $sum"
    case "$sum" in
        *"fail=0") ;;
        *) status=1; grep '^PRESENT FAIL' "$OUTDIR/$name.out" | sed 's/^/      /'; head -10 "$OUTDIR/$name.err" | sed 's/^/      stderr: /' ;;
    esac
}

# compare NAME: every EXPECT line has its probe line on stderr.
compare() {
    python3 - "$OUTDIR/$1.out" "$OUTDIR/$1.err" <<'PY' || status=1
import re, sys
out, err = open(sys.argv[1]).read(), open(sys.argv[2]).read()
exp = re.findall(r"^EXPECT frame (\d+) texel (0x[0-9a-f]+) size (\d+)x(\d+)", out, re.M)
got = {m[0]: m for m in re.findall(r"victory-metal present: frame (\d+) size (\d+)x(\d+) probe \((\d+),(\d+)\) texel (0x[0-9a-f]+) surface-format (\d+)", err)}
bad = 0
for n, texel, w, h in exp:
    g = got.get(n)
    if not g:
        print(f"      frame {n}: no probe line"); bad += 1; continue
    t = int(g[5], 16) & 0xffffffff
    if t != int(texel, 16) or (g[1], g[2]) != (w, h):
        print(f"      frame {n}: drawable {t:#010x} {g[1]}x{g[2]}, painted {texel} {w}x{h}"); bad += 1
print(f"   drawable check: {len(exp) - bad}/{len(exp)} frames carry the painted colour and size" + ("" if exp else " (NO FRAMES)"))
sys.exit(1 if bad or not exp else 0)
PY
}

metal_install_front
run rgba metal --frames 6
compare rgba
run bgra metal --frames 5 --bgra --waitable
compare bgra
if [ -n "$shot" ]; then
    echo "== screenshot: holding a window for 15 s"
    WINEDLLPATH="$STAGE" WINEDLLOVERRIDES="$(metal_overrides metal)" VICTORY_METAL_LOG=1 \
        "$WINE" "$STAGE/front-present.exe" --frames 2 --hold 15 >"$OUTDIR/hold.out" 2>"$OUTDIR/hold.err" &
    pid=$!
    sleep 9
    screencapture -x "$shot" || status=1
    wait "$pid" || true
    grep -E '^(INFO|PRESENT FAIL)' "$OUTDIR/hold.out" | sed 's/^/   /'
    echo "   screenshot: $shot"
fi
metal_restore_front
if [ "$d3dmetal" = 1 ]; then
    run d3dmetal-rgba d3dmetal --frames 6
    run d3dmetal-bgra d3dmetal --frames 5 --bgra --waitable
fi
[ "$status" = 0 ] && echo "PASS" || echo "FAIL"
exit "$status"
