#!/usr/bin/env python3
"""mac S6: passed / failed / skipped of the hardware proofs, from the libtest
output and the executor log (VICTORY_SHADER_EXEC_LOG: outcome, request, test
thread, text). A test that passed without one request reaching the device is
SKIPPED (no executor, no device, a refused program), not passed.

usage: shader-hw-tally.py test.txt exec.tsv known.tsv
known.tsv: test <TAB> class <TAB> reason; class is base | semantic | bug.
"""
import re, sys, collections

test_txt, exec_tsv, known_tsv = sys.argv[1:4]
suffix = sys.argv[4] if len(sys.argv) > 4 else ""  # a pass with other settings, e.g. @opt127
status = {}
for line in open(test_txt, errors="replace"):
    m = re.match(r"test (\S+) \.\.\. (ok|FAILED|ignored)", line)
    if m:
        status[m.group(1) + suffix] = m.group(2)
dev = collections.defaultdict(lambda: collections.Counter())
try:
    for line in open(exec_tsv, errors="replace"):
        p = line.rstrip("\n").split("\t")
        if len(p) >= 3:
            dev[p[2] + suffix][p[0]] += 1
except FileNotFoundError:
    pass
known = {}
for line in open(known_tsv):
    if line.startswith("#") or not line.strip():
        continue
    p = line.rstrip("\n").split("\t")
    known[p[0]] = (p[1], p[2] if len(p) > 2 else "")

ran, skipped, partial, failed, ignored = [], [], [], [], []
hw = lambda t: any(k in t for k in ("::valu_tests", "::wave64_tests", "::permlane_tests", "::waterfall_tests", "::intersect_tests", "::size_tests", "::cube_lod_tests", "::probe_tests", "::s3_tests", "::diff_tests"))
host_only = []
for t, s in sorted(status.items()):
    if s == "ignored":
        ignored.append(t); continue
    c = dev.get(t, {})
    on_device = c.get("ok", 0) + c.get("failed", 0)
    if s == "FAILED":
        failed.append(t)
    elif on_device:
        (partial if c.get("unsupported") else ran).append(t)
    elif hw(t) or c:
        skipped.append(t)
    else:
        host_only.append(t)  # a CPU-only test of the translator
print("---- hardware proofs on the Mac GPU")
print(f"passed on the device : {len(ran) + len(partial)}  (of which {len(partial)} had a program MSC refused: that part skipped)")
print(f"failed               : {len(failed)}")
print(f"skipped (no device)  : {len(skipped)}")
print(f"ignored              : {len(ignored)}")
print(f"CPU-only passed      : {len(host_only)}")
bad = 0
for t in failed:
    cls, why = known.get(t, ("UNCLASSIFIED", ""))
    print(f"  FAILED [{cls}] {t}  {why}")
    if cls in ("UNCLASSIFIED", "bug"):
        bad = 1
for t in skipped:
    print(f"  skipped {t}")
unsup = collections.Counter()
for t, c in dev.items():
    if c.get("unsupported"):
        unsup[t] = c["unsupported"]
for t, n in sorted(unsup.items()):
    print(f"  program refused by MSC x{n}: {t}")
sys.exit(bad)
