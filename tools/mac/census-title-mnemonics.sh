#!/bin/sh
# Static census of the title's instruction FORMS (mnemonic + operand shape), the complement of census-title-isa.sh (which
# counts only the Rosetta-relevant classes). Answers: which x86-64 instruction forms does the title contain, how often, and how
# many of them are covered by what the Rosetta probes (tools/mac/rosetta-probe) have proven. CPU only, the game is not started.
# usage: tools/mac/census-title-mnemonics.sh [image] [outdir]
#   image   default: the cached mapped title image victory-image-b99f876b74d4a77f.dll (build 1.0.2.1), as census-title-isa.sh
#   outdir  default: $HOME/Documents/GTA6Project/research/census/title-mnemonics-<date> (outside the repo: game-derived)
# Writes: forms.tsv (form, mnemonic, count, ymm), mnemonics.tsv (mnemonic, count, forms, ymm_count), total.txt.
# Needs llvm-objdump (llvm-mingw's from VIctory-macOS/target/toolchain, or on PATH) and python3. Takes a few minutes: run niced.
set -eu
root=$HOME/Documents/GTA6Project
img=${1:-${VICTORY_CACHE_DIR:-$root/data/shared/cache}/images/victory-image-b99f876b74d4a77f.dll}
out=${2:-$root/research/census/title-mnemonics-$(date +%Y%m%d)}
objdump=$(command -v llvm-objdump || ls "$root"/VIctory-macOS/target/toolchain/llvm-mingw*/bin/llvm-objdump | head -1)
[ -f "$img" ] || { echo "no image $img" >&2; exit 1; }
mkdir -p "$out"
"$objdump" -d --x86-asm-syntax=intel -j .text "$img" | python3 -c '
import sys, re, collections
out = sys.argv[1]
forms = collections.Counter(); ymm = collections.Counter(); total = 0
pat = re.compile(r"^\s*[0-9a-f]+:\s+(?:[0-9a-f]{2} )+\s*\t(.*)$")
GP = {}
for n, names in ((64, "rax rbx rcx rdx rsi rdi rbp rsp rip"), (32, "eax ebx ecx edx esi edi ebp esp eip"),
                 (16, "ax bx cx dx si di bp sp"), (8, "al bl cl dl ah bh ch dh sil dil bpl spl")):
    for r in names.split():
        GP[r] = "r%d" % n
for i in range(8, 16):
    for suf, n in (("", 64), ("d", 32), ("w", 16), ("b", 8)):
        GP["r%d%s" % (i, suf)] = "r%d" % n
SIZES = {"byte": "m8", "word": "m16", "dword": "m32", "qword": "m64", "xmmword": "m128", "ymmword": "m256", "zmmword": "m512",
         "tbyte": "m80", "xword": "m128", "oword": "m128", "fword": "m48", "ptr": "m"}
def shape(tok):
    tok = tok.strip()
    if "ptr" in tok or tok.startswith("["):
        w = tok.split()[0]
        return SIZES.get(w, "m")
    if tok in GP: return GP[tok]
    if re.fullmatch(r"xmm\d+", tok): return "x"
    if re.fullmatch(r"ymm\d+", tok): return "y"
    if re.fullmatch(r"zmm\d+", tok): return "z"
    if re.fullmatch(r"k[0-7]", tok): return "k"
    if re.fullmatch(r"mm\d", tok): return "mm"
    if re.fullmatch(r"st(\(\d\))?", tok): return "st"
    if tok in ("cs","ds","es","fs","gs","ss"): return "seg"
    if re.fullmatch(r"-?(0x[0-9a-f]+|\d+)", tok): return "i"
    if re.fullmatch(r"[a-z]+\d*", tok): return "cr"
    return "l"
prefix = {"rep", "repe", "repne", "repz", "repnz", "bnd", "notrack", "xacquire", "xrelease", "data16"}
pend = False
for line in sys.stdin:
    m = pat.match(line)
    if not m: continue
    txt = m.group(1).split("#")[0].strip()
    if not txt: continue
    total += 1
    parts = txt.split(None, 1); mn = parts[0]; ops = parts[1] if len(parts) > 1 else ""
    lk = pend; pend = False
    pre = ""
    while True:
        if mn == "lock":
            lk = True
            p2 = ops.split(None, 1)
            if not p2: pend = True; mn = ""; break
            mn = p2[0]; ops = p2[1] if len(p2) > 1 else ""
        elif mn in prefix:
            pre = mn + " "
            p2 = ops.split(None, 1)
            if not p2: break
            mn = p2[0]; ops = p2[1] if len(p2) > 1 else ""
        else:
            break
    if not mn: continue
    sig = ",".join(shape(t) for t in re.split(r",\s*", ops)) if ops else ""
    f = ("lock " if lk else "") + pre + mn + " " + sig
    forms[f.strip()] += 1
    if "ymm" in ops: ymm[f.strip()] += 1
by = collections.defaultdict(lambda: [0, 0, 0])
with open(out + "/forms.tsv", "w") as fh:
    fh.write("form\tmnemonic\tcount\tymm\n")
    for f, n in forms.most_common():
        mn = f.split()[0]
        if mn in ("lock",) or mn in prefix: mn = " ".join(f.split()[:2]) if f.split()[0] == "lock" else f.split()[1]
        base = f.split(" ")[0] if not f.startswith(("lock ", "rep")) else f.split(" ")[1]
        fh.write("%s\t%s\t%d\t%d\n" % (f, base, n, ymm[f]))
        by[base][0] += n; by[base][1] += 1; by[base][2] += ymm[f]
with open(out + "/mnemonics.tsv", "w") as fh:
    fh.write("mnemonic\tcount\tforms\tymm_count\n")
    for mn, (n, k, y) in sorted(by.items(), key=lambda kv: -kv[1][0]):
        fh.write("%s\t%d\t%d\t%d\n" % (mn, n, k, y))
open(out + "/total.txt", "w").write("instructions %d\nforms %d\nmnemonics %d\n" % (total, len(forms), len(by)))
print("instructions", total, "forms", len(forms), "mnemonics", len(by), "->", out)
' "$out"
