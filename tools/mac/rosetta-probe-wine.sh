#!/bin/sh
# Cross-builds tools/mac/rosetta-probe/probe-wine.c to a Windows exe (llvm-mingw) and runs every case under CrossOver Wine
# (Rosetta underneath) with a vectored exception handler, one process per case and mode. CPU only: no game, no GPU.
#
#   tools/mac/rosetta-probe-wine.sh [full|quick] [output file]
#   tools/mac/rosetta-probe-wine.sh case <probe-wine.exe arguments...>      e.g.  case vpmaskmovd_ymm cross mask=hi
#   tools/mac/rosetta-probe-wine.sh isa [output file]    numeric differential batches only (rcp/rsqrt, vcvt*), PROBE_JOBS parallel processes (default 4, never more than
#                                                        the job units heavy.sh gave: VICTORY_HEAVY_JOBS); PROBE_FAST=n divides every pass by 2^n (smoke test); PROBE_NEG=1 is the negative control
#
# Output (default research/census/rosetta-probe-wine-<date>.txt outside the repo, plus a .md summary next to it):
# one "RES ..." line per case and mode. A case that kills the process (Rosetta "EmulateForward", a hang) leaves no RES line
# of its own; this script writes one with status=FATAL / HANG / CRASH.
# Wine prefix: target/wineprefix-probe (WINEPREFIX overrides; the prefix of a running game is refused). Only that prefix's
# wineserver is ever stopped (`wineserver -k` with that WINEPREFIX), so a game on another prefix is not touched.
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# A heavy job: only through tools/mac/heavy.sh (budget, nice 19, 1 job while a game runs; docs/mac/RUN.md "Heavy tasks"). The isa mode asks
# for PROBE_JOBS units (default 4) because it starts that many probe-wine.exe at once; every other mode runs one at a time.
if [ -z "${VICTORY_HEAVY_SLOT:-}" ]; then
    case "${1:-}" in isa) _hj="${PROBE_JOBS:-4}" ;; *) _hj=1 ;; esac
    exec bash "$ROOT/tools/mac/heavy.sh" --jobs "$_hj" --label rosetta-probe-wine -- sh "$ROOT/tools/mac/rosetta-probe-wine.sh" "$@"
fi
cd "$ROOT"
HERE="$ROOT/tools/mac/rosetta-probe"
[ -f tools/mac/env.sh ] && . tools/mac/env.sh
MODE=full
case "${1:-}" in full|quick|case|isa) MODE="$1"; shift ;; esac
case "$ROOT" in */worktrees/mac/*) _g="$ROOT/../../.." ;; *) _g="$ROOT/.." ;; esac   # worktrees/mac/<name> or next to the main checkouts
WDIR="${VICTORY_MAC_WINE_DIR:-$_g/VIctory-macOS/target/wine-cx}"
[ -x "$WDIR/bin/wine" ] || WDIR="$HOME/Library/Caches/victory-macos/target/wine-cx"
WINE="${WINE:-$WDIR/bin/wine}"
WSERVER="$(dirname "$WINE")/wineserver"
[ -x "$WINE" ] || { echo "rosetta-probe-wine: no wine at $WINE (set WINE or VICTORY_MAC_WINE_DIR)" >&2; exit 1; }
export WINEPREFIX="${WINEPREFIX:-$ROOT/target/wineprefix-probe}"
case "$WINEPREFIX" in
    *wineprefix-play*|*metal-play*) echo "rosetta-probe-wine: refusing $WINEPREFIX (a game may be using it)" >&2; exit 2 ;;
esac
export WINEDEBUG=-all WINEDLLOVERRIDES="mscoree,mshtml=;winedbg.exe=d" ROSETTA_ADVERTISE_AVX=1
OUTDIR="$ROOT/target/rosetta-probe-wine"; mkdir -p "$OUTDIR"
EXE="$OUTDIR/probe-wine.exe"
if [ ! -x "$EXE" ] || [ "$HERE/probe-wine.c" -nt "$EXE" ] || [ "$HERE/softref.h" -nt "$EXE" ] || [ "$HERE/softref-cvt.h" -nt "$EXE" ] || [ "$HERE/isa-numeric.h" -nt "$EXE" ]; then
    x86_64-w64-mingw32-clang -O1 -Wall -Wno-unused-function -Wno-unused-variable -mavx2 -mfma -mf16c -mbmi -mbmi2 -mlzcnt -mpopcnt -mcx16 \
        "$HERE/probe-wine.c" -o "$EXE" || exit 1
fi
stop_prefix() { "$WSERVER" -k >/dev/null 2>&1 </dev/null; sleep 1; }

tmp="$(mktemp -d)"; n=0; NFATAL=0
trap 'rm -rf "$tmp"; stop_prefix' EXIT
LIMIT="${PROBE_LIMIT:-300}"      # in 0.2 s ticks
run_one() {
    n=$((n+1)); o="$tmp/o.$n"; e="$tmp/e.$n"; : >"$o"; : >"$e"
    "$WINE" "$EXE" "$@" >"$o" 2>"$e" </dev/null &
    pid=$!; t=0; fatal=0
    while kill -0 "$pid" 2>/dev/null; do
        sleep 0.2; t=$((t+1))
        if grep -q "EmulateForward\|rosetta error" "$e" "$o" 2>/dev/null; then fatal=1; break; fi
        [ "$t" -ge "$LIMIT" ] && { fatal=2; break; }
    done
    if [ $fatal -ne 0 ]; then stop_prefix; fi
    { wait "$pid"; } 2>/dev/null; rc=$?
    tr -d '\r' <"$o" | grep -h "^RES\|^  "
    desc="case=$1 mode=${2:-n/a}"; shift 2 2>/dev/null; for a in "$@"; do desc="$desc $a"; done
    if [ $fatal -eq 1 ]; then
        NFATAL=$((NFATAL+1))
        echo "RES $desc status=FATAL reason=EmulateForward detail=\"$(grep -h 'rosetta error' "$e" "$o" | head -1 | sed 's/^rosetta error: //' | cut -c1-230)\""
    elif [ $fatal -eq 2 ]; then
        echo "RES $desc status=HANG reason=no_result_in_$((LIMIT/5))s"
    elif ! grep -q "^RES" "$o"; then
        echo "RES $desc status=CRASH rc=$rc stderr=\"$(tail -c 200 "$e" | tr '\n' ' ')\""
    fi
}

if [ "$MODE" = case ]; then run_one "$@"; exit 0; fi

if [ "$MODE" = isa ]; then
    OUT="${1:-$HOME/Documents/GTA6Project/research/census/rosetta-probe-isa-$(date +%Y%m%d).txt}"
    JOBS="${PROBE_JOBS:-4}"
    # the slot's units are the limit (the game running: 1), whatever PROBE_JOBS says
    [ "$JOBS" -le "${VICTORY_HEAVY_JOBS:-$JOBS}" ] || JOBS="$VICTORY_HEAVY_JOBS"
    # one batch per instruction form (the selector is a substring of the form name); long exhaustive ones first
    LIST="cvt_f2i:vcvtps2dq_ymm cvt_f2i:vcvttps2dq_ymm cvt_f2i:vcvttss2si_r32 cvt_f2i:vcvttss2si_r64 cvt_f2i:vcvtss2si_r32 cvt_f2i:vcvtss2si_r64
rcp_rsqrt:vrcpps_xmm rcp_rsqrt:vrcpps_ymm rcp_rsqrt:vrsqrtps_xmm rcp_rsqrt:vrsqrtps_ymm rcp_rsqrt:vrcpss rcp_rsqrt:vrsqrtss
cvt_i2f:vcvtdq2ps_ymm cvt_i2f:vcvtsi2ss_r32 cvt_i2f:vcvtdq2ps_xmm cvt_i2f:vcvtsi2ss_r64 cvt_i2f:vcvtsi2sd_r32 cvt_i2f:vcvtsi2sd_r64 cvt_i2f:vcvtdq2pd_ymm cvt_i2f:vcvtdq2pd_xmm
cvt_f2f:vcvtps2pd_ymm cvt_f2f:vcvtps2pd_xmm cvt_f2f:vcvtss2sd cvt_f2f:vcvtpd2ps_xmm cvt_f2f:vcvtpd2ps_ymm cvt_f2f:vcvtsd2ss
cvt_f2i:vcvttps2dq_xmm cvt_f2i:vcvtps2dq_xmm cvt_f2i:vcvttsd2si_r32 cvt_f2i:vcvttsd2si_r64 cvt_f2i:vcvtsd2si_r32 cvt_f2i:vcvtsd2si_r64
cvt_f2i:vcvttpd2dq_xmm cvt_f2i:vcvttpd2dq_ymm cvt_f2i:vcvtpd2dq_xmm cvt_f2i:vcvtpd2dq_ymm
cvt_f2i:*_mem cvt_i2f:*_mem cvt_f2f:*_mem
avx_move:*vmovaps avx_move:*vmovups avx_move:*vmovapd avx_move:*vmovupd avx_move:*vmovdqa avx_move:*vmovdqu avx_move:*vlddqu avx_move:*vmovss avx_move:*vmovsd
avx_move:*vmovd_ avx_move:*vmovq_ avx_move:*vmovlp avx_move:*vmovhp avx_move:*vmovlhps avx_move:*vmovhlps avx_move:*dup avx_move:*broadcast avx_move:*vmovmsk avx_move:*vpmovmskb
avx_fparith:*vaddp avx_fparith:*vadds avx_fparith:*vsubp avx_fparith:*vsubs avx_fparith:*vmulp avx_fparith:*vmuls avx_fparith:*vdivp avx_fparith:*vdivs avx_fparith:*vmaxp avx_fparith:*vmaxs avx_fparith:*vminp avx_fparith:*vmins avx_fparith:*vsqrtps_xmm avx_fparith:*vsqrtps_ymm avx_fparith:*vsqrtpd avx_fparith:*vsqrts
x86_int_alu:add_* x86_int_alu:sub_* x86_int_alu:adc_* x86_int_alu:sbb_* x86_int_alu:and_* x86_int_alu:or_* x86_int_alu:xor_* x86_int_alu:cmp_* x86_int_alu:test_* x86_int_alu:inc_* x86_int_alu:dec_* x86_int_alu:neg_* x86_int_alu:not_*
x86_int_move:mov_* x86_int_move:movabs* x86_int_move:movz* x86_int_move:movs* x86_int_move:xchg_* x86_int_move:cmov* x86_int_move:set* x86_int_move:j* x86_int_move:lea_*
avx_logic:v*
avx_shuffle:*
"
    [ -n "${PROBE_LIST:-}" ] && LIST="$PROBE_LIST"   # e.g. PROBE_LIST="cvt_f2i:*_mem cvt_i2f:*_mem" to rerun a subset
    {
    echo "# Rosetta-under-Wine ISA numeric probe $(date '+%Y-%m-%d %H:%M %Z') macOS $(sw_vers -productVersion) $(sysctl -n machdep.cpu.brand_string) jobs=$JOBS fast=${PROBE_FAST:-0} neg=${PROBE_NEG:-0}"
    echo "# wine: $("$WINE" --version 2>/dev/null), prefix $(basename "$WINEPREFIX"), ROSETTA_ADVERTISE_AVX=1"
    n=0
    for j in $LIST; do
        while [ "$(jobs -r | wc -l)" -ge "$JOBS" ]; do sleep 1; done
        n=$((n+1)); b="${j%%:*}"; sel="${j#*:}"
        ( "$WINE" "$EXE" batch "$b" "$sel" >"$tmp/i.$n" 2>"$tmp/ie.$n" </dev/null; rc=$?
          tr -d '\r' <"$tmp/i.$n" | grep '^RES'
          grep -q '^RES' "$tmp/i.$n" || echo "RES fam=$b batch=$sel mode=run status=CRASH rc=$rc stderr=\"$(tail -c 200 "$tmp/ie.$n" | tr '\n"' '  ')\"" ) >"$tmp/io.$n" &
    done
    wait
    i=1; while [ $i -le $n ]; do cat "$tmp/io.$i"; i=$((i+1)); done
    } >"$OUT" 2>&1
    echo "# RES lines: $(grep -c '^RES' "$OUT"), FAIL: $(grep -c 'status=FAIL' "$OUT"), CRASH: $(grep -c 'status=CRASH' "$OUT")" >>"$OUT"
    python3 "$HERE/summarize-wine.py" "$OUT" >"${OUT%.txt}-summary.md"
    echo "$OUT"; exit 0
fi

OUT="${1:-$HOME/Documents/GTA6Project/research/census/rosetta-probe-wine-$(date +%Y%m%d).txt}"
{
echo "# Rosetta-under-Wine probe $(date '+%Y-%m-%d %H:%M %Z') macOS $(sw_vers -productVersion) $(sysctl -n machdep.cpu.brand_string) mode=$MODE"
echo "# wine: $("$WINE" --version 2>/dev/null) (CrossOver Wine build in $(basename "$(dirname "$(dirname "$WINE")")")), prefix $(basename "$WINEPREFIX"), ROSETTA_ADVERTISE_AVX=1"
run_one cpuid; run_one page4k
STORES="$("$WINE" "$EXE" list 2>/dev/null | tr -d '\r' | awk '$1=="store"{print $2" "$3" "$4}')"
while read -r name ml back <&3; do
    # a masked store is run with several masks; plain stores once per mode
    nocross=0; case "$name" in vmovdqa_ymm|vmovaps_ymm|vmovntdq_ymm|vmovntps_ymm|vmovntdq_xmm|movntdq_xmm|movntps_xmm|clwb|clflushopt|clflush|prefetcht0|prefetchw) nocross=1 ;; esac
    if [ "$ml" -gt 0 ]; then
        for m in full zero l0 alt; do run_one "$name" inpage mask=$m; done
        for m in full l0 zero alt hi last; do run_one "$name" fault mask=$m; done
        run_one "$name" ro mask=full
        for m in full lo hi last l0 zero alt; do run_one "$name" cross mask=$m; done
        for m in full lo hi; do run_one "$name" crossro mask=$m; done
        for m in full lo hi; do run_one "$name" crossfirst mask=$m; done
        for m in full hi; do run_one "$name" split mask=$m; done
        if [ "$MODE" = full ]; then
            lane=4; case "$name" in *q_*|*pd_*) lane=8 ;; esac
            sz=$((back*2)); for b2 in 1 $((lane/2)) $((sz-1)) $((sz-lane/2)); do for m in full hi; do run_one "$name" cross mask=$m back=$b2; done; done
        fi
    else
        run_one "$name" inpage; run_one "$name" fault; run_one "$name" ro
        if [ $nocross -eq 0 ]; then
            run_one "$name" cross; run_one "$name" crossro; run_one "$name" crossfirst; run_one "$name" split
            if [ "$MODE" = full ] && [ "$back" -gt 1 ]; then run_one "$name" cross back=1; run_one "$name" cross back=$((back*2-1)); run_one "$name" crossfirst back=1; fi
        fi
    fi
done 3<<EOSTORES
$STORES
EOSTORES
# boundary on a 16 KiB host-page edge, protection of 4 KiB and 16 KiB
for c in "vmovdqu_ymm" "vpmaskmovd_ymm:full" "vpmaskmovd_ymm:hi" "vmaskmovps_ymm:full" "vcvtps2ph_mem_ymm0" "rep_movsb" "lock_cmpxchg32:hit" "lock_xadd64"; do
    nm="${c%%:*}"; mk=""; case "$c" in vp*|vm*) [ "$c" != "${c#*:}" ] && mk="mask=${c#*:}" ;; esac
    for g in 4096 16384; do run_one "$nm" cross b=0x4000 gran=$g $mk; run_one "$nm" fault b=0x4000 gran=$g $mk; done
done
# atomics
for a in cmpxchg8 cmpxchg16 cmpxchg32 cmpxchg64 lock_cmpxchg8 lock_cmpxchg16 lock_cmpxchg32 lock_cmpxchg64; do
    for h in hit miss; do
        run_one "$a:$h" inpage; run_one "$a:$h" fault; run_one "$a:$h" ro
        case "$a" in *8) ;; *) run_one "$a:$h" cross; run_one "$a:$h" crossro; run_one "$a:$h" crossfirst; run_one "$a:$h" split ;; esac
    done
done
for a in xadd8 xadd16 xadd32 xadd64 lock_xadd8 lock_xadd16 lock_xadd32 lock_xadd64; do
    run_one "$a" inpage; run_one "$a" fault; run_one "$a" ro
    case "$a" in *8) ;; *) run_one "$a" cross; run_one "$a" crossro; run_one "$a" split ;; esac
done
for h in hit miss; do run_one "lock_cmpxchg16b:$h" inpage; run_one "lock_cmpxchg16b:$h" fault; run_one "lock_cmpxchg16b:$h" ro; done
run_one cmpxchg16b_unaligned inpage
# numeric batches
LIMIT=6000
for b in atomic_flags atomic_split bmi f16c fma_nan fma fma_ftz; do run_one batch "$b"; done
} >"$OUT" 2>&1
echo "# RES lines: $(grep -c "^RES" "$OUT"), FATAL: $(grep -c "status=FATAL" "$OUT")" >>"$OUT"
python3 "$HERE/summarize-wine.py" "$OUT" >"${OUT%.txt}-summary.md"
echo "$OUT"
