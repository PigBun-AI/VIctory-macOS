#!/usr/bin/env python3
"""Golden file for S-I-001 (docs/mac/PATCHES.md G12): what LLVM's AMDGPU disassembler (gfx1030 RDNA2 or gfx1010 RDNA1, an
independent implementation of the GFX10 encodings) makes of 65,536 instruction heads, one or more per value of the top 16
bits of the first dword (which holds every encoding prefix and nearly every opcode field), with the rest of the
words filled from a fixed generator, so that the Rust test (`gfx10/llvm_tests.rs`) feeds the same words to our decoder.

  gen.py gfx1030 [outdir [llvm-mc]]   writes outdir/gfx10_llvm_gfx1030_{0..3}.txt   (and gfx1010)

Line: `<first top16 hex> <count> <variant> <mnemonic> <dwords>` (a run of consecutive heads), one per distinct (mnemonic, length) of the variants LLVM decodes
for that head (variant: how the other words are filled, `words()`); heads LLVM does not decode get no line. The mnemonic has
LLVM's `_e32 _e64 _dpp _sdwa` suffixes removed. Needs LLVM with the AMDGPU target (Homebrew `llvm`).
"""
import os, re, subprocess, sys, tempfile

CPU = sys.argv[1] if len(sys.argv) > 1 else "gfx1030"
OUT = sys.argv[2] if len(sys.argv) > 2 else "crates/victory-shader/tests/data"
LLVM = sys.argv[3] if len(sys.argv) > 3 else os.environ.get("LLVM_MC", "/opt/homebrew/opt/llvm/bin/llvm-mc")
SWEEP = 14  # the variants from this one on only exist where a sweep varies bits the head does not hold (see words())


def sampled(t, variant):
    """The MIMG sweeps (variants 16 and up) only make sense for MIMG heads."""
    return variant < 16 or 0xF000 <= t < 0xF400


def samples():
    return [(t, v) for v in range(VARIANTS) for t in range(65536) if sampled(t, v)]


VARIANTS = 16 + 64
M64 = (1 << 64) - 1


def splitmix(x):
    x = (x + 0x9E3779B97F4A7C15) & M64
    z = x
    z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & M64
    z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & M64
    return z ^ (z >> 31)


def words(top16, variant):
    """Six dwords: the head, then what the variant says: 0 zeros, 1 small register numbers, 2 generated, 3 flat with saddr off."""
    if variant == 0:
        return [top16 << 16] + [0] * 5
    if variant == 3:  # FLAT/GLOBAL/SCRATCH with saddr = off (0x7d), the only saddr the flat segment takes
        return [top16 << 16, 0x007D0000] + [0] * 4
    if variant == 14:  # SOP1 sweep: sdst = t >> 8 & 0x7f, opcode = t & 0xff (the opcode is in the low half of SOP1), ssrc0 = s2
        return [0xBE800000 | (top16 >> 8 & 0x7F) << 16 | (top16 & 0xFF) << 8 | 2] + [0] * 5
    if variant == 15:  # VOP1 sweep: vdst = t >> 8, opcode = t & 0xff, src0 = v0
        return [0x7E000000 | (top16 >> 8) << 17 | (top16 & 0xFF) << 9 | 0x100] + [0] * 5
    if variant >= 16:  # MIMG: the low six bits (the dimension and NSA bits) swept, dmask 0xf, address v4
        return [top16 << 16 | 0x9F00 | (variant - 16), 0x00020004] + [0] * 4
    if variant in (4, 5):  # the same with segment bits 15:14 = scratch (1) / global (2)
        return [top16 << 16 | (0x4000 if variant == 4 else 0x8000), 0x007D0000] + [0] * 4
    if 6 <= variant < 14:  # MIMG with DIM = 0..7 (word 0 bits 4:2), small register numbers
        return [top16 << 16 | (variant - 6) << 2 | 0x0100, 0x00010101] + [0] * 4
    if variant == 1:
        return [top16 << 16 | 0x0101] + [0x01010101] + [0] * 4
    w0 = top16 << 16 | splitmix(top16 * 8) & 0xFFFF
    return [w0] + [splitmix(top16 * 8 + k) & 0xFFFFFFFF for k in range(1, 6)]


SAMPLES = samples()
with tempfile.NamedTemporaryFile("w", suffix=".hex", delete=False) as f:
    for n, (t, variant) in enumerate(SAMPLES):
        ws = words(t, variant)
        mark = ",".join("0x%02x" % b for b in (0xBF800000 | (n & 0xFFFF)).to_bytes(4, "little"))
        # the sample must not print like its own marker: the nop of stratum 0xBF80 has no such immediate
        assert not (ws[0] >> 16 == 0xBF80 and ws[0] & 0xFFFF == n & 0xFFFF), n
        f.write(",".join("0x%02x" % b for w in ws for b in w.to_bytes(4, "little")) + "\n" + "\n".join([mark] * 5) + "\n")
    path = f.name
out = subprocess.run([LLVM, "-disassemble", "-triple=amdgcn", "-mcpu=" + CPU, "-show-encoding", path],
                     capture_output=True, text=True).stdout
os.unlink(path)
# the marker after sample n is `s_nop n & 0xffff` (five times: a partial instruction at the end of a sample swallows some)
groups, cur = [], []
for line in out.splitlines():
    m = re.match(r"\t(\S+)(.*?)\s*; encoding: \[(.*)\]", line)
    if not m:
        continue
    n = len(groups)
    imm = m.group(2).strip()
    if m.group(1) == "s_nop" and imm in (str(n & 0xFFFF), hex(n & 0xFFFF)):
        groups.append(cur)
        cur = []
    elif m.group(1) == "s_nop" and n > 0 and imm in (str((n - 1) & 0xFFFF), hex((n - 1) & 0xFFFF)):
        pass  # the rest of the five markers
    else:
        cur.append((m.group(1), len(m.group(3).split(",")) // 4, m.group(3).split(",")[:4]))
assert len(groups) == len(SAMPLES), len(groups)
index = {k: i for i, k in enumerate(SAMPLES)}
lines = []
for t in range(65536):
    # every variant of which LLVM decodes the head itself (its first printed instruction must be the head: after an
    # undecodable head it carries on with the next dword), one line per distinct (name, length)
    seen = set()
    for variant in range(VARIANTS):
        if not sampled(t, variant):
            continue
        g = groups[index[(t, variant)]]
        head = ["0x%02x" % b for b in words(t, variant)[0].to_bytes(4, "little")]
        if g and g[0][0] != "v_illegal" and g[0][2] == head:
            name = re.sub(r"(_e32|_e64|_dpp|_sdwa)+$", "", g[0][0])
            if (name, g[0][1]) not in seen:
                seen.add((name, g[0][1]))
                lines.append((t, variant, name, g[0][1]))
# runs: consecutive heads with the same variant, name and length are one line `first count variant name length`
runs = []
for key in sorted(lines, key=lambda l: (l[1], l[2], l[3], l[0])):
    t, v, name, n = key
    if runs and runs[-1][2:5] == [v, name, n] and runs[-1][0] + runs[-1][1] == t and t & 0x3FFF:
        runs[-1][1] += 1
    else:
        runs.append([t, 1, v, name, n])
# four files per target (the asset policy limits a file to 640 KiB): heads t >> 14 = 0..3
parts = [[] for _ in range(4)]
for t, count, v, name, n in sorted(runs):
    parts[t >> 14].append("%04x %d %d %s %d\n" % (t, count, v, name, n))
for k, part in enumerate(parts):
    with open("%s/gfx10_llvm_%s_%d.txt" % (OUT, CPU, k), "w") as f:
        f.writelines(part)
