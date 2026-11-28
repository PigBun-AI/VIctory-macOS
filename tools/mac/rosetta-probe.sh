#!/bin/sh
# Builds tools/mac/rosetta-probe/probe.c for x86_64 and runs each case under Rosetta 2, one child per case.
# Output goes to $1 (default: research/census/rosetta-probe-<date>.txt outside the repo). Needs Apple clang + Rosetta.
set -u
here=$(cd "$(dirname "$0")" && pwd)
# a heavy job (a clang build, then every case under Rosetta): only through heavy.sh (docs/mac/RUN.md "Heavy tasks")
[ -n "${VICTORY_HEAVY_SLOT:-}" ] || exec bash "$here/heavy.sh" --jobs 1 --label rosetta-probe -- sh "$here/rosetta-probe.sh" "$@"
out=${1:-$HOME/Documents/GTA6Project/research/census/rosetta-probe-$(date +%Y%m%d).txt}
tmp=$(mktemp -d)
clang -arch x86_64 -O0 -mavx2 -mfma -mbmi -mbmi2 -mf16c -mlzcnt -mpopcnt -mavx512f -mclwb -mclflushopt "$here/rosetta-probe/probe.c" -o "$tmp/probe" || exit 1
{
echo "# Rosetta probe $(date '+%Y-%m-%d %H:%M %Z') macOS $(sw_vers -productVersion) $(sysctl -n machdep.cpu.brand_string)"
for n in cpuid xgetbv rdtsc rdtscp lock_cmpxchg_flags bmi fma f16c gather avx512 xsave rdrand popcnt_lzcnt clzero monitorx pause_mfence; do
  echo "== nonfault $n"; perl -e 'alarm 10; exec @ARGV' "$tmp/probe" nonfault $n 2>&1 | tail -3; echo "exit=$?"
done
for n in $("$tmp/probe" list); do for m in inpage fault cross ro; do
  echo "== $n $m"
  perl -e 'alarm 10; exec @ARGV' "$tmp/probe" $n $m >"$tmp/o" 2>"$tmp/e"; rc=$?
  cat "$tmp/o"; echo "rc=$rc"; tail -c 300 "$tmp/e"
done; done
} >"$out" 2>&1
echo "$out"
