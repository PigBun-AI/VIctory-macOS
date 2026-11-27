#!/bin/bash
# Batch acceptance of the ray-tracing structure builder (docs/mac/rt-builder-acceptance.md):
#   tools/mac/rt-builder-accept.sh run   [--out DIR] [--timeout SEC] [--backends "d3dmetal metal"] [--baseline] [-- run.sh options]
#   tools/mac/rt-builder-accept.sh check LOGDIR_OR_stderr.log [...]     (logs only; exit 0 = every check passed)
#   tools/mac/rt-builder-accept.sh print                                  (the run.sh command lines, nothing started)
# `run` starts one game per backend, one after the other (never two at once), with
# VICTORY_BVH_BUILDER=1 VICTORY_BVH_QUARANTINE=1 VICTORY_MAC_DROP_STATS=1 and run.sh --perf, then checks each.
# --baseline adds a run per backend with the builder off (same input) for the drop counts to compare.
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
RUN="$ROOT/tools/mac/run.sh"

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
        [ "${rb:-0}" = 0 ] && say PASS "rt_builder_off=0 (whole run)" || say FAIL "rt_builder_off=$rb dispatches in the whole run (early ones before the builder was whole count; see per-present lines)"
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
    out="$ROOT/target/mac-runs/rt-builder-$(date +%Y%m%d-%H%M%S)"; tmo=900; backends="d3dmetal metal"; base=0; extra=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --out) out=$2; shift ;; --timeout) tmo=$2; shift ;; --backends) backends=$2; shift ;;
            --baseline) base=1 ;; --) shift; extra=("$@"); break ;;
            *) echo "unknown option $1" >&2; exit 2 ;;
        esac; shift
    done
    rc=0
    runs=(); for b in $backends; do runs+=("$b:on"); [ $base = 1 ] && runs+=("$b:off"); done
    for r in "${runs[@]}"; do
        b=${r%%:*}; s=${r##*:}; dir="$out/$b-builder-$s"
        env=(VICTORY_MAC_DROP_STATS=1)
        [ "$s" = on ] && env+=(VICTORY_BVH_BUILDER=1 VICTORY_BVH_QUARANTINE=1)
        cmd=(env "${env[@]}" "$RUN" --backend "$b" --perf --timeout "$tmo" --sample 10 --frametime-log --out "$dir" ${extra[@]+"${extra[@]}"})
        if [ "$mode" = print ]; then printf '%q ' "${cmd[@]}"; echo; continue; fi
        mkdir -p "$dir"; echo "== $b builder $s -> $dir"
        "${cmd[@]}" || echo "run.sh exited $?"
        [ "$s" = on ] && { check_one "$dir/stderr.log" | tee "$dir/accept.txt" || rc=1; }
    done
    exit $rc ;;
  *) sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
