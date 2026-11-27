#!/bin/sh
# End-to-end test of the Metal backend through Wine, and the ExecuteCommandLists
# round-trip comparison with D3DMetal.
#
# Runs front-e2e.exe (metal/front/e2e: device, queue, committed upload / default /
# readback buffers, Map, CopyBufferRegion, barriers, ExecuteCommandLists, fences,
# descriptors, root signatures; every result checked against a CPU model) in the
# scratch prefix research/m0-prefix (never the game's), RUNS times per backend:
#
#   d3dmetal   Wine's builtin D3DMetal          WINEDLLOVERRIDES=...d3d12,d3d12core,dxgi=b
#   metal      our d3d12.dll/dxgi.dll over the  WINEDLLOVERRIDES=...d3d12,dxgi=n,b
#              Metal back (real back, no mock)  WINEDLLPATH=<stage>
#
# and prints the PASS/FAIL lines of each backend, then the PERF table: median of
# the per-process p50 / p99 over the runs. The front-e2e.exe of one backend must
# pass entirely for the run to count. our dlls are put into the prefix's system32
# for the metal runs and taken out again at the end (tools/metal/wine-lib.sh).
#
# Usage: tools/metal/test-e2e.sh [--no-build] [--runs N] [--only d3dmetal|metal] [--no-perf]
# Do not run next to another Wine test in the same prefix (a lock refuses).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck disable=SC1091
. "$here/wine-lib.sh"

build=1; runs=3; only=""; perf=1
while [ $# -gt 0 ]; do
    case "$1" in
        --no-build) build=0 ;;
        --runs) runs=$2; shift ;;
        --only) only=$2; shift ;;
        --no-perf) perf=0 ;;
        *) echo "usage: $0 [--no-build] [--runs N] [--only d3dmetal|metal] [--no-perf]" >&2; exit 2 ;;
    esac
    shift
done

metal_wine_init
[ "$build" = 1 ] && "$here/build-unixlib.sh" >/dev/null
[ -f "$STAGE/front-e2e.exe" ] || { echo "missing $STAGE/front-e2e.exe" >&2; exit 1; }
OUTDIR=$(mktemp -d "${TMPDIR:-/tmp}/metal-e2e.XXXXXX")
METAL_EXTRA_CLEANUP='[ "${KEEP:-0}" = 1 ] || rm -rf "$OUTDIR"'

exe_args=""
[ "$perf" = 0 ] && exe_args="--no-perf"
status=0
for backend in d3dmetal metal; do
    [ -n "$only" ] && [ "$only" != "$backend" ] && continue
    ov=$(metal_overrides "$backend")
    [ "$backend" = metal ] && metal_install_front
    echo "== $backend: $runs run(s)"
    i=1
    while [ "$i" -le "$runs" ]; do
        # shellcheck disable=SC2086
        if [ "$backend" = metal ]; then
            WINEDLLPATH="$STAGE" WINEDLLOVERRIDES="$ov" "$WINE" "$STAGE/front-e2e.exe" $exe_args >"$OUTDIR/$backend.$i.out" 2>"$OUTDIR/$backend.$i.err" || true
        else
            WINEDLLOVERRIDES="$ov" "$WINE" "$STAGE/front-e2e.exe" $exe_args >"$OUTDIR/$backend.$i.out" 2>"$OUTDIR/$backend.$i.err" || true
        fi
        sum=$(grep '^E2E SUMMARY' "$OUTDIR/$backend.$i.out" || echo "E2E SUMMARY (none: the exe did not finish)")
        echo "   run $i: $sum"
        case "$sum" in *"fail=0 "*) ;; *) status=1 ;; esac
        i=$((i + 1))
    done
    echo "-- $backend, run 1:"
    grep -E '^(E2E (PASS|FAIL|SKIP)|INFO)' "$OUTDIR/$backend.1.out" | sed 's/^/   /'
    if [ -s "$OUTDIR/$backend.1.err" ]; then
        echo "-- $backend stderr (first 20 lines):"
        head -20 "$OUTDIR/$backend.1.err" | sed 's/^/   /'
    fi
    [ "$backend" = metal ] && metal_restore_front
done

if [ "$perf" = 1 ] && [ -z "$only" ]; then
    echo
    echo "== ExecuteCommandLists round trip, microseconds: median over $runs processes of each process's p50 (p99)"
    python3 - "$OUTDIR" "$runs" <<'PY'
import re, statistics, sys
out, runs = sys.argv[1], int(sys.argv[2])
def load(backend):
    data = {}
    for i in range(1, runs + 1):
        try:
            text = open(f"{out}/{backend}.{i}.out").read()
        except OSError:
            continue
        for m in re.finditer(r"^PERF (\S+) p50_us=([\d.]+) p99_us=([\d.]+) min_us=([\d.]+) n=(\d+)", text, re.M):
            data.setdefault(m.group(1), []).append((float(m.group(2)), float(m.group(3)), float(m.group(4))))
    return data
d, m = load("d3dmetal"), load("metal")
print(f"{'case':<24}{'D3DMetal p50':>14}{'(p99)':>10}{'ours p50':>12}{'(p99)':>10}{'ours/D3DMetal':>15}")
for name in sorted(set(d) | set(m)):
    def med(rows, k):
        return statistics.median(r[k] for r in rows) if rows else None
    a50, a99, b50, b99 = med(d.get(name, []), 0), med(d.get(name, []), 1), med(m.get(name, []), 0), med(m.get(name, []), 1)
    f = lambda v: f"{v:.1f}" if v is not None else "-"
    ratio = f"{b50 / a50:.2f}x" if a50 and b50 else "-"
    print(f"{name:<24}{f(a50):>14}{'(' + f(a99) + ')':>10}{f(b50):>12}{'(' + f(b99) + ')':>10}{ratio:>15}")
print("\nper-process p50s:")
for name in sorted(set(d) | set(m)):
    print(f"  {name:<24} D3DMetal {[r[0] for r in d.get(name, [])]}   ours {[r[0] for r in m.get(name, [])]}")
PY
fi
[ "$status" = 0 ] && echo "PASS" || echo "FAIL"
exit "$status"
