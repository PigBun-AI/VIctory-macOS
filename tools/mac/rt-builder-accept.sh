#!/bin/bash
# Batch acceptance of the ray-tracing structure builder (docs/mac/rt-builder-acceptance.md):
#   tools/mac/rt-builder-accept.sh run   --purpose "<map column>: <node id>,..." [--out DIR] [--timeout SEC] [--backends "metal"] [--baseline] [-- run.sh options]
#   tools/mac/rt-builder-accept.sh check LOGDIR_OR_stderr.log [...]     (logs only; exit 0 = every check passed)
#   tools/mac/rt-builder-accept.sh print                                  (the run.sh command lines, nothing started)
# `run` starts one game per backend (default: metal only; --backends "metal d3dmetal" adds the reference-only D3DMetal), one after
# the other (never two at once; run.sh holds the game lock), with
# VICTORY_BVH_BUILDER=1 VICTORY_BVH_QUARANTINE=1 VICTORY_MAC_DROP_STATS=1 and run.sh --perf, then checks each.
# --purpose is required for `run` (run.sh's --purpose, see its head; exit status 7 without it) and is passed to every run.sh. `print` shows it if given.
# --baseline adds a run per backend with the builder off (same input) for the drop counts to compare.
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
RUN="$ROOT/tools/mac/run.sh"
. "$ROOT/tools/mac/purpose.sh"

check_one() { # $1 = stderr.log ; prints PASS/FAIL lines, returns 0 if all pass
    local f=$1 bad=0
    say() { printf '  %-5s %s\n' "$1" "$2"; [ "$1" = FAIL ] && bad=1; return 0; }
    echo "== $f"
    [ -s "$f" ] || { say FAIL "no log"; return 1; }
    # 1. the builder ran: all its programs have host pipelines
    local runs off
    runs=$(grep -a 'the ray-tracing structure builder runs' "$f" | tail -1)
    off=$(grep -a 'the ray-tracing structure builder stays off' "$f" | tail -1)
    if [ -n "$runs" ]; then
        local n; n=$(printf '%s' "$runs" | sed -n 's/.*pipelines=\([0-9]*\).*/\1/p')
        if [ "${n:-0}" = 53 ]; then say PASS "builder runs with pipelines=53"; else say FAIL "builder runs but pipelines=${n:-?} (title creates 53)"; fi
    else
        say FAIL "no 'the ray-tracing structure builder runs' line"
    fi
    if [ -n "$off" ]; then say FAIL "last 'stays off' line: $(printf '%s' "$off" | sed 's/.*missing=/missing=/' | cut -c1-300)"; else say PASS "no 'stays off' line"; fi
    # a stage that could not be bound stops the others
    local rest; rest=$(grep -a -c 'structure builder rests for this frame' "$f")
    [ "$rest" = 0 ] && say PASS "no 'builder rests' line" || say FAIL "'builder rests' x$rest (a dispatch could not be bound)"
    # 2. drop classes: last whole-run summary
    local sum; sum=$(grep -a 'drop-stats summary span=all' "$f" | tail -1)
    if [ -z "$sum" ]; then say FAIL "no drop-stats summary (VICTORY_MAC_DROP_STATS=1 not on, or run too short)"; else
        local rb lo
        rb=$(printf '%s' "$sum" | grep -o 'rt_builder_off=[0-9]*' | cut -d= -f2); lo=$(printf '%s' "$sum" | grep -o 'never_requested=[0-9]*' | cut -d= -f2)
        if [ "${rb:-0}" = 0 ]; then say PASS "rt_builder_off=0 (whole run)"; elif [ -n "$runs" ]; then say INFO "rt_builder_off=$rb in the whole run (early dispatches before the builder was whole; the per-present check below decides)"; else say FAIL "rt_builder_off=$rb and the builder never ran"; fi
        [ "${lo:-0}" = 0 ] && say PASS "never_requested:left_out=0" || say FAIL "never_requested=$lo"
        printf '  info  %s\n' "$(printf '%s' "$sum" | sed 's/.*why\[/why[/; s/\] kind.*/]/')"
    fi
    # ExecuteIndirect drops hide in class "ind": look at the last block of top lines
    local ind; ind=$(grep -a 'drop-stats top span=all' "$f" | tail -20 | grep -a -c 'ind:rt_builder_off')
    [ "$ind" = 0 ] && say PASS "no ind:rt_builder_off among the last top lines" || say FAIL "ind:rt_builder_off among the top lines"
    # the same, after the builder came on: per-present lines after the first 'runs' line
    if [ -n "$runs" ]; then
        local after
        after=$(awk '/the ray-tracing structure builder runs/ {on=1} on && /drop-stats present=/ {print}' "$f" | tail -n +3 | grep -a -c 'rt_builder_off\|left_out')
        [ "${after:-0}" = 0 ] && say PASS "no rt_builder_off / left_out in the presents after the builder ran" || say FAIL "$after presents after the builder ran still name rt_builder_off or left_out"
        grep -a 'drop-stats top' "$f" | grep -a 'rt_builder_off\|left_out' | tail -3 | cut -c1-260 | sed 's/^/  info  /'
    fi
    # 3. garbage into title memory / by-address faults
    local pf; pf=$(grep -a -c 'shaders touched pages that had no copy' "$f")
    printf '  info  by-address page faults warned: %s (compare with the baseline run; a rise = accesses lost)\n' "$pf"
    local crash; crash=$(grep -a -c -i 'access violation\|0xc0000005\|exception\|panicked\|device removed\|kIOGPU\|SubmissionsIgnored' "$f")
    [ "$crash" = 0 ] && say PASS "no crash / GPU error text" || say FAIL "crash or GPU error text x$crash: $(grep -a -i -m2 'access violation\|0xc0000005\|exception\|panicked\|device removed\|kIOGPU' "$f" | cut -c1-200 | tr '\n' '|')"
    grep -a -q 'time limit .*s reached' "$f" && say PASS "ended by --timeout (alive to the end)" || say FAIL "did not reach the time limit (title ended or died earlier)"
    local fr; fr=$(grep -a 'ms/frame' "$f" | tail -1 | sed 's/.*INFO[^:]*: //' | cut -c1-120)
    printf '  info  last frame line: %s\n' "$fr"
    return $bad
}

case "${1:-}" in
  check)
    shift; rc=0
    for p in "$@"; do f=$p; [ -d "$p" ] && f="$p/stderr.log"; check_one "$f" || rc=1; done
    exit $rc ;;
  run|print)
    mode=$1; shift
    out="$ROOT/target/mac-runs/rt-builder-$(date +%Y%m%d-%H%M%S)"; tmo=900; backends="metal"; base=0; extra=(); purpose=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --out) out=$2; shift ;; --timeout) tmo=$2; shift ;; --backends) backends=$2; shift ;;
            --baseline) base=1 ;; --purpose) purpose=${2:?--purpose "<map column>: <node id>,..."}; shift ;; --) shift; extra=("$@"); break ;;
            *) echo "unknown option $1" >&2; exit 2 ;;
        esac; shift
    done
    if [ "$mode" = run ]; then purpose_check "$purpose" || exit "$PURPOSE_EXIT"; fi
    pargs=(); [ -z "$purpose" ] || pargs=(--purpose "$purpose")
    rc=0
    runs=(); for b in $backends; do runs+=("$b:on"); [ $base = 1 ] && runs+=("$b:off"); done
    for r in "${runs[@]}"; do
        b=${r%%:*}; s=${r##*:}; dir="$out/$b-builder-$s"
        env=(VICTORY_MAC_DROP_STATS=1)
        if [ "$s" = on ]; then
            env+=(VICTORY_BVH_BUILDER=1 VICTORY_BVH_QUARANTINE=1)
        else
            env+=(VICTORY_BVH_BUILDER=0)
        fi
        cmd=(env "${env[@]}" "$RUN" ${pargs[@]+"${pargs[@]}"} --backend "$b" --perf --timeout "$tmo" --sample 10 --frametime-log --out "$dir" ${extra[@]+"${extra[@]}"})
        if [ "$mode" = print ]; then printf '%q ' "${cmd[@]}"; echo; continue; fi
        mkdir -p "$dir"; echo "== $b builder $s -> $dir"
        "${cmd[@]}" || echo "run.sh exited $?"
        [ "$s" = on ] && { check_one "$dir/stderr.log" | tee "$dir/accept.txt" || rc=1; }
    done
    exit $rc ;;
  *) sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
