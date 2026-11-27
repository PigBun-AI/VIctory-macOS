#!/bin/sh
# Per-test comparison of two `cargo test` runs: Wine + D3DMetal against Wine + our Metal backend.
#
#   tools/metal/compare-tests.sh --run [-- cargo test args]    run both (tools/metal/test-wine.sh),
#                                                             keep the logs in $OUT, then compare
#   tools/metal/compare-tests.sh D3DMETAL.log METAL.log        compare two saved logs
#
# Default cargo test arguments of --run: `-p victory-shader --no-fail-fast -- --test-threads=1`.
# $OUT defaults to a new folder under $TMPDIR; set OUT=<dir> to choose (and keep) it.
#
# Output: one table by outcome of the pair (D3DMetal, ours):
#   both pass | only D3DMetal passes | only ours passes | both fail | ignored
# then, for each test that is not "both pass", its first panic line on each side.
#
# metal/conformance/expected-fail.txt registers known differences, one per line:
#     TEST NAME | SIDE | REASON            (# comments and blank lines are skipped)
# SIDE is where the test is known to fail: `both`, `ours` (it passes on D3DMetal) or
# `d3dmetal` (it passes on ours). A registered test is reported as EXPECTED and does not
# fail the comparison; a registered test that now behaves differently (passes where it
# was expected to fail, or fails where the entry says it passes) is reported as STALE.
# Exit status: 0 when every difference is registered (and nothing is stale), 1 otherwise.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$here/../.." && pwd)
EXPECTED=${EXPECTED:-$ROOT/metal/conformance/expected-fail.txt}

if [ "${1:-}" = "--run" ]; then
    shift
    [ "${1:-}" = "--" ] && shift
    [ $# -gt 0 ] || set -- -p victory-shader --no-fail-fast -- --test-threads=1
    OUT=${OUT:-$(mktemp -d "${TMPDIR:-/tmp}/metal-compare.XXXXXX")}
    mkdir -p "$OUT"
    "$here/test-wine.sh" --backend d3dmetal -- "$@" >"$OUT/d3dmetal.log" 2>&1 || true
    "$here/test-wine.sh" --backend metal -- "$@" >"$OUT/metal.log" 2>&1 || true
    echo "logs: $OUT/d3dmetal.log $OUT/metal.log"
    set -- "$OUT/d3dmetal.log" "$OUT/metal.log"
fi
[ $# -eq 2 ] || { echo "usage: $0 --run [-- cargo test args] | $0 D3DMETAL.log METAL.log" >&2; exit 2; }

python3 - "$1" "$2" "$EXPECTED" <<'PY'
import re, sys

def parse(path):
    """name -> 'ok' | 'FAILED' | 'ignored'; first panic line per failed test."""
    res, why = {}, {}
    line_re = re.compile(r"^test (\S+) \.\.\. (ok|FAILED|ignored)")
    cur = None
    in_list, failed_list = False, set()
    unresolved = []
    pending = None  # "test X ... " whose verdict follows output that bypassed the harness
    with open(path, errors="replace") as f:
        for line in f:
            if pending and line.strip() in ("ok", "FAILED", "ignored"):
                res[pending] = line.strip()
                unresolved.remove(pending)
                pending = None
                continue
            m = re.match(r"^test (\S+) \.\.\. (?!ok\b|FAILED\b|ignored\b)", line)
            if m:
                # the verdict follows output that bypassed the harness; if it never
                # comes on a line of its own, the "failures:" list at the end decides
                pending = m.group(1)
                unresolved.append(pending)
                continue
            m = line_re.match(line)
            if m:
                res[m.group(1)] = m.group(2)
                continue
            m = re.match(r"^---- (\S+) stdout ----", line)
            if m:
                cur = m.group(1)
                continue
            if line.startswith("failures:"):
                in_list = True
                continue
            if in_list:
                m = re.match(r"^    (\S+)$", line)
                if m:
                    failed_list.add(m.group(1))
                    continue
                if line.strip() == "" and failed_list:
                    in_list = False
            if cur and cur not in why and "panicked at" in line:
                why[cur] = None  # the message is on the next line
                continue
            if cur and why.get(cur, 1) is None and line.strip():
                why[cur] = line.strip()
    for n in unresolved:
        res[n] = "FAILED" if n in failed_list else "ok"
    return res, why

d_res, d_why = parse(sys.argv[1])
m_res, m_why = parse(sys.argv[2])

expected = {}
try:
    for line in open(sys.argv[3]):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = [p.strip() for p in line.split("|", 2)]
        if len(parts) != 3 or parts[1] not in ("both", "ours", "d3dmetal"):
            sys.exit(f"bad line in {sys.argv[3]}: {line!r} (want: TEST | both|ours|d3dmetal | REASON)")
        expected[parts[0]] = (parts[1], parts[2])
except FileNotFoundError:
    pass

names = sorted(set(d_res) | set(m_res))
cats = {"both pass": [], "only D3DMetal passes": [], "only ours passes": [], "both fail": [], "ignored": [], "missing on one side": []}
for n in names:
    d, m = d_res.get(n), m_res.get(n)
    if d is None or m is None:
        cats["missing on one side"].append(n)
    elif d == "ignored" or m == "ignored":
        cats["ignored"].append(n)
    elif d == "ok" and m == "ok":
        cats["both pass"].append(n)
    elif d == "ok":
        cats["only D3DMetal passes"].append(n)
    elif m == "ok":
        cats["only ours passes"].append(n)
    else:
        cats["both fail"].append(n)

print(f"{'outcome':<26}{'tests':>7}")
for k, v in cats.items():
    print(f"{k:<26}{len(v):>7}")
print(f"{'total':<26}{len(names):>7}")

side_of = {"only D3DMetal passes": "ours", "only ours passes": "d3dmetal", "both fail": "both"}
unexpected, stale = [], []
for cat, side in side_of.items():
    for n in cats[cat]:
        if n in expected and expected[n][0] == side:
            continue
        unexpected.append((cat, n))
for n, (side, reason) in expected.items():
    cat = next((c for c, v in cats.items() if n in v), None)
    if cat is None:
        stale.append((n, "not in either run"))
    elif side_of.get(cat) != side:
        stale.append((n, f"registered as failing on {side}, now: {cat}"))

for cat in ("only D3DMetal passes", "only ours passes", "both fail", "missing on one side"):
    if not cats[cat]:
        continue
    print(f"\n== {cat} ({len(cats[cat])})")
    for n in cats[cat]:
        tag = "EXPECTED" if n in expected and side_of.get(cat) == expected[n][0] else "        "
        why = (m_why.get(n) if cat != "only ours passes" else d_why.get(n)) or ""
        if cat == "both fail" and not why:
            why = d_why.get(n) or ""
        print(f"  {tag} {n}\n           {why[:150]}")
        if tag.strip():
            print(f"           registered: {expected[n][1][:150]}")
if stale:
    print("\n== STALE entries in expected-fail.txt")
    for n, w in stale:
        print(f"  {n}: {w}")
if unexpected:
    print(f"\nUNEXPECTED differences: {len(unexpected)}")
ok = not unexpected and not stale
print("\nCOMPARE " + ("OK" if ok else "FAIL"))
sys.exit(0 if ok else 1)
PY
