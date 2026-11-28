#!/bin/bash
# A/B of where the title's draws go (docs/mac/PATCHES.md, G8; default backend metal, --backend d3dmetal as an option):
# a WARM run (pipeline book + `victory warm` made before) against a COLD run (new,
# empty cache), the same input in both, then drop-stats-report.sh on the two.
# Runs one game each, one after the other (the GPU is shared; never start it while
# another run is going). Needs a victory.exe built from mac/drop-stats (tools/mac/check.sh).
#
#   tools/mac/drop-ab.sh --purpose "<map column>: <node id>,..." [--out DIR] [--base DIR] [--frames N] [--warm-jobs N] [--perf|--fidelity] [--backend metal|d3dmetal]
#                        [--make-book] [--warm-only|--cold-only] [--before-cold CMD]
#   tools/mac/drop-ab.sh --report RUN_DIR [RUN_DIR ...] [--frames N]     (logs only, no run)
#
#   --purpose P     REQUIRED (not with --report): run.sh's --purpose (see its head), given to every run.sh of this script; exit status 7 without it
#   --out DIR       where everything goes (default target/drop-ab/<time>): warm/ and cold/ each
#                   hold data/ and cache/ (their own copies) and run/ (run.sh's --out: the logs)
#   --base DIR      the data folder copied (APFS clone) for both runs: default `data` of run.sh's
#                   seed, data/livetr under the GTA6 root (GTA6_ROOT); the warm run
#                   also copies the `cache` folder beside it, the cold run starts with an EMPTY one
#   --perf|--fidelity  run.sh's profile (default --perf: native 1440p, mac X13; --fidelity
#                   is run.sh's fidelity profile, which writes 2560x1440 into the copy's settings.xml)
#   --backend B     run.sh's backend for every run of this script (default metal; d3dmetal is reference-only)
#   --frames N      the report counts world frames 1..N (default 600)
#   --warm-jobs N   processes of `victory warm` (default: its own, two thirds of the cores)
#   --make-book     if the warm cache has no known-pipelines.bin, first make one with a normal run
#                   (VICTORY_PIPELINE_BOOK=1, same input); else that is an error
#   --before-cold   a shell command to run just before the cold run (to empty a driver cache you
#                   know the place of; the D3DMetal compiled-pipeline cache is outside the cache
#                   folder and nothing here empties it -- the cold run goes FIRST for that reason)
#
# Both runs: VICTORY_MAC_DROP_STATS=2 (per-present lines with the host pipelines that ran) and
# `tools/mac/run.sh --backend <metal|d3dmetal> --perf|--fidelity --data D/data --cache D/cache --out D/run`
# (run.sh's own input script world-neutral.txt and Enter keys, its 900 s timeout; the options are
# explicit, nothing of run.sh's environment is copied here). The log folder is the --out given to
# run.sh, so there is nothing to look for; run.sh's own files (start.txt, run-env.txt, summary.txt,
# stdout.log, stderr.log) are in <warm|cold|book>/run. The data folder is the copy given with --data;
# it is not run.sh's per-profile directory (data/livetr-<profile>-<backend>), which is never touched.
# stdout is stamped with the time each line was seen (stdout.ts.log), to tie D3DMetal's
# `marking PSO(n) as no-op` lines to presents.
#
# `victory warm` (apps/victory/src/warm.rs): `victory.exe warm [--jobs N] [--limit N] [--again]
# [--used]` makes the host pipelines of every pipeline in <cache>/pipelines/known-pipelines.bin (written
# by runs with VICTORY_PIPELINE_BOOK=1) in child processes (`warm --these ...`), so that the driver's own
# cache has them; it resumes where it stopped (<cache>/pipelines/warm-done.txt; warm-bad.txt lists the
# ones the compiler hung on or crashed in). It must see the same Wine prefix, D3DMetal and
# translator as the run: "done" is keyed on the fingerprint (translation + driver). It is started as
# `tools/mac/run.sh --exec -- warm ...` with the same backend, profile, --data and --cache as the run, so
# prefix, DLLs, DXC front end and environment are run.sh's own, not copied here.
set -u
SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
case "$ROOT" in */worktrees/mac/*) _g="$ROOT/../../.." ;; *) _g="$ROOT/.." ;; esac   # worktrees/mac/<name> or next to the main checkouts
G="${GTA6_ROOT:-$(cd "$_g" && pwd)}"
cd "$ROOT" || exit 1
REPORT="$ROOT/tools/mac/drop-stats-report.sh"
RUN="$ROOT/tools/mac/run.sh"
profile=perf; backend=metal; purpose=""
. "$ROOT/tools/mac/purpose.sh"
out=""; base=""; frames=600; wjobs=""; book=0; only=""; before=""; report_only=()
while [ $# -gt 0 ]; do
    case "$1" in
        --purpose) purpose=${2:?--purpose "<map column>: <node id>,..."}; shift ;;
        --out) out=$2; shift ;; --base) base=$2; shift ;; --frames) frames=$2; shift ;;
        --warm-jobs) wjobs=$2; shift ;; --make-book) book=1 ;;
        --warm-only) only=warm ;; --cold-only) only=cold ;; --before-cold) before=$2; shift ;;
        --report) shift; while [ $# -gt 0 ] && [ "${1#--}" = "$1" ]; do report_only+=("$1"); shift; done; continue ;;
        --backend) backend=${2:?--backend metal|d3dmetal}; shift ;;
        --perf) profile=perf ;; --fidelity) profile=fidelity ;;
        -h|--help) sed -n '2,/^set -u/p' "$SELF" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "usage: see the head of $0" >&2; exit 2 ;;
    esac
    shift
done
if [ ${#report_only[@]} -gt 0 ]; then exec bash "$ROOT/tools/mac/heavy.sh" --jobs 1 --label drop-stats-report -- "$REPORT" "${report_only[@]}" --frames "$frames"; fi

purpose_check "$purpose" || exit "$PURPOSE_EXIT"
[ -n "$base" ] || base="$G/data/livetr/data"
[ -d "$base" ] || { echo "no data folder $base (--base)" >&2; exit 1; }
basecache="$(dirname "$base")/cache"
[ -n "$out" ] || out="$ROOT/target/drop-ab/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$out"; out="$(cd "$out" && pwd)"
WDIR="${VICTORY_MAC_WINE_DIR:-$G/VIctory-macOS/target/wine-cx}"
EXE="$ROOT/target/x86_64-pc-windows-gnullvm/release/victory.exe"
[ -f "$EXE" ] || { echo "no exe $EXE (tools/mac/check.sh builds it)" >&2; exit 1; }
[ -x "$WDIR/bin/wine" ] || { echo "no Wine at $WDIR" >&2; exit 1; }
{
    echo "drop-ab $(date +%FT%T)"
    echo "checkout $ROOT @ $(git rev-parse --short HEAD) $(git rev-parse --abbrev-ref HEAD)"
    echo "exe $(ls -l "$EXE" | awk '{print $5, $6, $7, $8}')"
    echo "profile $profile (run.sh --backend $backend --$profile)"
    echo "base data $base"; echo "base cache $basecache"
    echo "input tools/mac/input/world-neutral.txt sha1 $(shasum tools/mac/input/world-neutral.txt | cut -c1-12)"
    echo "frames $frames"
} >"$out/drop-ab.txt"

# env of both runs; everything else is run.sh's
export VICTORY_MAC_DROP_STATS=2
# the options every run.sh call of this script has: one place, one backend
RUN_OPTS=(--purpose "$purpose" --backend "$backend" "--$profile" --exe "$EXE")

# a copy of a folder: an APFS clone (no extra space until written), else a plain copy
clone() { cp -c -R "$1" "$2" 2>/dev/null || { rm -rf "$2"; cp -R "$1" "$2"; }; }

# one game run through run.sh: $1 = label, the rest = extra env assignments
run_game() {
    local label=$1; shift
    local dir="$out/$label" log="$out/$label.run.sh.out" stamp="" rd="$out/$label/run"
    echo "== $label: run.sh (data $dir/data, cache $dir/cache, logs $rd)"
    env "$@" "$RUN" "${RUN_OPTS[@]}" --data "$dir/data" --cache "$dir/cache" --out "$rd" >"$log" 2>&1 &
    local pid=$!
    # run.sh makes $rd/stdout.log when it starts the title; -F waits for the file
    tail -n +1 -F "$rd/stdout.log" 2>/dev/null | python3 -u -c 'import sys,time
for l in sys.stdin: print("%.3f\t%s" % (time.time(), l), end="", flush=True)' >"$rd.stdout.ts.log" &
    stamp=$!
    wait $pid; local st=$?
    sleep 2; pkill -f "tail -n +1 -F $rd/stdout.log" 2>/dev/null; kill $stamp 2>/dev/null
    [ -d "$rd" ] && mv "$rd.stdout.ts.log" "$rd/stdout.ts.log" 2>/dev/null
    tail -n 1 "$log"
    [ -f "$rd/stdout.log" ] || { echo "$label: run.sh made no log folder $rd (exit $st, see $log)" >&2; return 1; }
    local n; n=$(grep -a -c "drop-stats present=" "$rd/stderr.log" 2>/dev/null || true)
    echo "$label: exit $st, ${n:-0} drop-stats present lines"
    case "$label" in book) ;; *) [ "${n:-0}" -gt 0 ] || echo "$label: WARNING no drop-stats lines: is the exe built from mac/drop-stats?" >&2 ;; esac
    return 0
}

# `victory warm` through run.sh --exec: the prefix, DLLs, DXC front end and variables of run.sh, on $1's data and cache
run_warm() {
    local dir=$1
    echo "== warm: victory.exe warm ${wjobs:+--jobs $wjobs} (cache $dir/cache/pipelines)"
    "$RUN" "${RUN_OPTS[@]}" --data "$dir/data" --cache "$dir/cache" --exec -- warm ${wjobs:+--jobs $wjobs} >"$dir/warm.log" 2>&1
    local st=$?
    tail -n 3 "$dir/warm.log"; echo "warm: exit $st; files:"
    ls -l "$dir/cache/pipelines" 2>/dev/null | awk 'NR>1 {print "  ", $5, $9}'
    return $st
}

if [ "$only" != "warm" ]; then
    mkdir -p "$out/cold/cache"; clone "$base" "$out/cold/data"
    if [ -n "$before" ]; then echo "== before cold: $before"; sh -c "$before"; fi
    run_game cold || exit 1
fi
if [ "$only" != "cold" ]; then
    mkdir -p "$out/warm"; clone "$base" "$out/warm/data"
    if [ -d "$basecache" ]; then clone "$basecache" "$out/warm/cache"; else mkdir -p "$out/warm/cache"; fi
    if [ ! -f "$out/warm/cache/pipelines/known-pipelines.bin" ]; then
        if [ $book = 1 ]; then
            # a book run uses its own copy, the warm one's cache (run.sh: cache = beside the data folder)
            mkdir -p "$out/book"; clone "$base" "$out/book/data"; rm -rf "$out/book/cache"; clone "$out/warm/cache" "$out/book/cache"
            run_game book VICTORY_PIPELINE_BOOK=1 VICTORY_MAC_DROP_STATS=0 || exit 1
            rm -rf "$out/warm/cache"; clone "$out/book/cache" "$out/warm/cache"
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
    bash "$ROOT/tools/mac/heavy.sh" --jobs 1 --label drop-stats-report -- "$REPORT" "${runs[@]}" --frames "$frames"
} 2>&1 | tee "$out/report.txt"
echo "all in $out"
