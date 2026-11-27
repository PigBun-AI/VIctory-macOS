#!/bin/bash
# A/B of where the title's draws go on the D3DMetal route (docs/mac/PATCHES.md, G8):
# a WARM run (pipeline book + `victory warm` made before) against a COLD run (new,
# empty cache), the same input in both, then drop-stats-report.sh on the two.
# Runs one game each, one after the other (the GPU is shared; never start it while
# another run is going). Needs a victory.exe built from mac/drop-stats (tools/mac/check.sh).
#
#   tools/mac/drop-ab.sh [--out DIR] [--base DIR] [--frames N] [--warm-jobs N]
#                        [--make-book] [--warm-only|--cold-only] [--before-cold CMD]
#   tools/mac/drop-ab.sh --report RUN_DIR [RUN_DIR ...] [--frames N]     (logs only, no run)
#
#   --out DIR       where everything goes (default target/drop-ab/<time>): warm/ and cold/ each
#                   hold data/ and cache/ (their own copies), run -> the run.sh log folder
#   --base DIR      the data folder copied for both runs (default: $VICTORY_DATA_DIR, else
#                   victory-data-livetr/data next to this checkout, as run.sh); the warm run
#                   also copies the `cache` folder beside it, the cold run starts with an EMPTY one
#   --frames N      the report counts world frames 1..N (default 600)
#   --warm-jobs N   processes of `victory warm` (default: its own, two thirds of the cores)
#   --make-book     if the warm cache has no known-pipelines.bin, first make one with a normal run
#                   (VICTORY_PIPELINE_BOOK=1, same input); else that is an error
#   --before-cold   a shell command to run just before the cold run (to empty a driver cache you
#                   know the place of; the D3DMetal compiled-pipeline cache is outside the cache
#                   folder and nothing here empties it -- the cold run goes FIRST for that reason)
#
# Both runs: VICTORY_MAC_DROP_STATS=2 (per-present lines with the host pipelines that ran),
# VICTORY_INPUT_REPLAY=tools/mac/input/world-neutral.txt (relative: cwd is this checkout),
# run.sh's own key script (Enter every 3 s from 8 s), run.sh's timeout (900 s). tools/mac/run.sh is
# called with NO arguments: data folder, cache folder (= `cache` beside the data folder),
# wine and game folders go in by environment (VICTORY_DATA_DIR, GTA6_ROOT, GAME_DIR,
# VICTORY_MAC_WINE_DIR as run.sh reads them), and it is found again by the folder it creates in
# target/mac-runs. Its stdout is stamped with the time each line was seen (stdout.ts.log), to tie
# D3DMetal's `marking PSO(n) as no-op` lines to presents.
#
# `victory warm` (apps/victory/src/warm.rs): `victory.exe warm [--jobs N] [--limit N] [--again]
# [--used]` makes the host pipelines of every pipeline in <cache>/pipelines/known-pipelines.bin (written
# by runs with VICTORY_PIPELINE_BOOK=1) in child processes (`warm --these ...`), so that the driver's own
# cache has them; it resumes where it stopped (<cache>/pipelines/warm-done.txt; warm-bad.txt lists the
# ones the compiler hung on or crashed in). It must see the same Wine prefix, D3DMetal and
# translator as the run: "done" is keyed on the fingerprint (translation + driver).
set -u
SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
G="${GTA6_ROOT:-$(cd "$ROOT/.." && pwd)}"
cd "$ROOT" || exit 1
REPORT="$ROOT/tools/mac/drop-stats-report.sh"
out=""; base=""; frames=600; wjobs=""; book=0; only=""; before=""; report_only=()
while [ $# -gt 0 ]; do
    case "$1" in
        --out) out=$2; shift ;; --base) base=$2; shift ;; --frames) frames=$2; shift ;;
        --warm-jobs) wjobs=$2; shift ;; --make-book) book=1 ;;
        --warm-only) only=warm ;; --cold-only) only=cold ;; --before-cold) before=$2; shift ;;
        --report) shift; while [ $# -gt 0 ] && [ "${1#--}" = "$1" ]; do report_only+=("$1"); shift; done; continue ;;
        -h|--help) sed -n "2,41p" "$SELF"; exit 0 ;;
        *) echo "usage: see the head of $0" >&2; exit 2 ;;
    esac
    shift
done
if [ ${#report_only[@]} -gt 0 ]; then exec python3 "$REPORT" "${report_only[@]}" --frames "$frames"; fi

[ -n "$base" ] || base="${VICTORY_DATA_DIR:-$G/victory-data-livetr/data}"
[ -d "$base" ] || { echo "no data folder $base (--base)" >&2; exit 1; }
basecache="$(dirname "$base")/cache"
[ -n "$out" ] || out="$ROOT/target/drop-ab/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$out"; out="$(cd "$out" && pwd)"
WDIR="${VICTORY_MAC_WINE_DIR:-$G/VIctory-macOS/target/wine-cx}"
EXE="$ROOT/target/x86_64-pc-windows-gnullvm/release/victory.exe"
PREFIX="$ROOT/target/wineprefix-livetr"          # run.sh's default prefix
[ -f "$EXE" ] || { echo "no exe $EXE (tools/mac/check.sh builds it)" >&2; exit 1; }
[ -x "$WDIR/bin/wine" ] || { echo "no Wine at $WDIR" >&2; exit 1; }
{
    echo "drop-ab $(date +%FT%T)"
    echo "checkout $ROOT @ $(git rev-parse --short HEAD) $(git rev-parse --abbrev-ref HEAD)"
    echo "exe $(ls -l "$EXE" | awk '{print $5, $6, $7, $8}')"
    echo "base data $base"; echo "base cache $basecache"
    echo "input tools/mac/input/world-neutral.txt sha1 $(shasum tools/mac/input/world-neutral.txt | cut -c1-12)"
    echo "frames $frames"
} >"$out/drop-ab.txt"

# env of both runs (VICTORY_INPUT_REPLAY relative, as the supervisor's runs)
export VICTORY_MAC_DROP_STATS=2 VICTORY_INPUT_REPLAY="tools/mac/input/world-neutral.txt"

# one run.sh run, no arguments: $1 = label, the rest = extra env assignments
run_game() {
    local label=$1; shift
    local dir="$out/$label" marker="$out/.marker-$label" log="$out/$label.run.sh.out" stamp=""
    : >"$marker"; sleep 1
    echo "== $label: run.sh (data $dir/data, cache $dir/cache)"
    env VICTORY_DATA_DIR="$dir/data" "$@" "$ROOT/tools/mac/run.sh" >"$log" 2>&1 &
    local pid=$! found=""
    for _ in $(seq 1 120); do
        found=$(find "$ROOT/target/mac-runs" -maxdepth 2 -name stdout.log -newer "$marker" 2>/dev/null | head -n 1)
        [ -n "$found" ] && break
        kill -0 $pid 2>/dev/null || break
        sleep 1
    done
    if [ -n "$found" ]; then
        local rd; rd="$(dirname "$found")"; ln -sfn "$rd" "$dir/run"
        tail -n +1 -F "$found" 2>/dev/null | python3 -u -c 'import sys,time
for l in sys.stdin: print("%.3f\t%s" % (time.time(), l), end="", flush=True)' >"$rd/stdout.ts.log" &
        stamp=$!
    fi
    wait $pid; local st=$?
    if [ -n "$stamp" ]; then sleep 2; pkill -f "tail -n +1 -F $found" 2>/dev/null; kill $stamp 2>/dev/null; fi
    tail -n 1 "$log"
    [ -e "$dir/run" ] || { echo "$label: no run folder found (see $log)" >&2; return 1; }
    local n; n=$(grep -a -c "drop-stats present=" "$dir/run/stderr.log" 2>/dev/null || true)
    echo "$label: exit $st, ${n:-0} drop-stats present lines"
    case "$label" in book) ;; *) [ "${n:-0}" -gt 0 ] || echo "$label: WARNING no drop-stats lines: is the exe built from mac/drop-stats?" >&2 ;; esac
    return 0
}

# `victory warm` in the Wine prefix of run.sh, on $1's cache
run_warm() {
    local dir=$1
    export WINEPREFIX="$PREFIX" WINEDEBUG="${WINEDEBUG:--all}"
    export WINEDLLOVERRIDES="mscoree,mshtml=;winedbg.exe=d;d3d12,d3d12core,dxgi=b"
    export ROSETTA_ADVERTISE_AVX=1 VICTORY_SKIP_CPU_CHECK=1
    export VICTORY_GAME_DIR="${GAME_DIR:-$G/GTA6.Xbox.v1.0.2.1}"
    export VICTORY_CACHE_DIR="$dir/cache" VICTORY_DATA_DIR="$dir/data"
    local helper="$ROOT/target/dxc-d3dmetal"
    if [ -x "$helper" ]; then
        export VICTORY_DXC_HELPER="$helper" VICTORY_DXC_DIR="$dir/dxc"
        export DXC_LIB="$WDIR/lib/external/D3DMetal.framework/Versions/A/Resources/libdxcompiler.dylib"
    fi
    "$WDIR/bin/wineserver" -k 2>/dev/null; "$WDIR/bin/wineserver" -w 2>/dev/null
    echo "== warm: victory.exe warm ${wjobs:+--jobs $wjobs} (cache $dir/cache/pipelines)"
    "$WDIR/bin/wine" "$EXE" warm ${wjobs:+--jobs $wjobs} >"$dir/warm.log" 2>&1
    local st=$?
    "$WDIR/bin/wineserver" -k 2>/dev/null
    tail -n 3 "$dir/warm.log"; echo "warm: exit $st; files:"
    ls -l "$dir/cache/pipelines" 2>/dev/null | awk 'NR>1 {print "  ", $5, $9}'
    unset VICTORY_CACHE_DIR VICTORY_DATA_DIR
    return $st
}

if [ "$only" != "warm" ]; then
    mkdir -p "$out/cold/cache"; cp -R "$base" "$out/cold/data"
    if [ -n "$before" ]; then echo "== before cold: $before"; sh -c "$before"; fi
    run_game cold || exit 1
fi
if [ "$only" != "cold" ]; then
    mkdir -p "$out/warm"; cp -R "$base" "$out/warm/data"
    if [ -d "$basecache" ]; then cp -R "$basecache" "$out/warm/cache"; else mkdir -p "$out/warm/cache"; fi
    if [ ! -f "$out/warm/cache/pipelines/known-pipelines.bin" ]; then
        if [ $book = 1 ]; then
            # a book run uses its own copy, the warm one's cache (run.sh: cache = beside the data folder)
            mkdir -p "$out/book"; cp -R "$base" "$out/book/data"; rm -rf "$out/book/cache"; cp -R "$out/warm/cache" "$out/book/cache"
            run_game book VICTORY_PIPELINE_BOOK=1 VICTORY_MAC_DROP_STATS=0 || exit 1
            rm -rf "$out/warm/cache"; cp -R "$out/book/cache" "$out/warm/cache"
        else
            echo "no $out/warm/cache/pipelines/known-pipelines.bin: the cache has no pipeline book (--make-book makes one with a run)" >&2
            exit 1
        fi
    fi
    run_warm "$out/warm" || echo "warm exited non-zero; going on (it resumes where it stopped; see $out/warm/warm.log)"
    run_game warm || exit 1
fi
{
    echo; echo "== report (first $frames world frames)"
    runs=(); [ -e "$out/warm/run" ] && runs+=("$out/warm/run"); [ -e "$out/cold/run" ] && runs+=("$out/cold/run")
    python3 "$REPORT" "${runs[@]}" --frames "$frames"
} 2>&1 | tee "$out/report.txt"
echo "all in $out"
