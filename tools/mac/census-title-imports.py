#!/usr/bin/env python3
"""Static census of call sites per imported function of the title (build 1.0.2.1).
stdin: llvm-objdump -d --x86-asm-syntax=intel -j .text of the mapped image; argv[1]: the title exe (read-only,
for its import directory); argv[2]: output directory (outside the repo). Counts `call/jmp qword ptr [rip+x]` whose
target is an import address table slot, and any other RIP-relative reference to a slot (address taken)."""
import sys, re, struct, os, collections
exe, outdir = sys.argv[1], sys.argv[2]
os.makedirs(outdir, exist_ok=True)
d = open(exe, 'rb').read()
pe = struct.unpack_from('<I', d, 0x3c)[0]
nsec = struct.unpack_from('<H', d, pe + 6)[0]; opt = struct.unpack_from('<H', d, pe + 20)[0]
base = struct.unpack_from('<Q', d, pe + 48)[0]
secs = []
for i in range(nsec):
    a = pe + 24 + opt + 40 * i
    vs, va, rs, rp = struct.unpack_from('<IIII', d, a + 8); secs.append((va, max(vs, rs), rp))
def off(rva):
    for va, sz, rp in secs:
        if va <= rva < va + sz: return rp + rva - va
    return None
dd = pe + 24 + 112  # data directories (PE32+)
imp_rva = struct.unpack_from('<I', d, dd + 8)[0]
slots = {}
p = off(imp_rva)
while p is not None:
    oft, _, _, name, ft = struct.unpack_from('<IIIII', d, p)
    if name == 0: break
    dll = d[off(name):d.index(b'\0', off(name))].decode()
    q = off(oft or ft); i = 0
    while True:
        e = struct.unpack_from('<Q', d, q + 8 * i)[0]
        if e == 0: break
        if e >> 63: fn = f'#{e & 0xffff}'
        else:
            o = off(e & 0x7fffffff) + 2; fn = d[o:d.index(b'\0', o)].decode()
        slots[base + ft + 8 * i] = (dll, fn); i += 1
    p += 20
cnt = collections.Counter()
rx = re.compile(r'\b(call|jmp)\s+qword ptr \[rip [+-] 0x[0-9a-f]+\]\s+# (0x[0-9a-f]+)')
rx2 = re.compile(r'\[rip [+-] 0x[0-9a-f]+\]\s+# (0x[0-9a-f]+)')
for line in sys.stdin:
    if 'rip' not in line: continue
    m = rx.search(line)
    if m:
        t = int(m.group(2), 16)
        if t in slots: cnt[slots[t]] += 1
        continue
    m = rx2.search(line)
    if m and int(m.group(1), 16) in slots: cnt[('(address taken) ' + slots[int(m.group(1), 16)][0], slots[int(m.group(1), 16)][1])] += 1
with open(os.path.join(outdir, 'import-callsites.tsv'), 'w') as f:
    f.write('dll\tfunction\tcallsites\n')
    for (dll, fn), n in sorted(cnt.items(), key=lambda kv: -kv[1]): f.write(f'{dll}\t{fn}\t{n}\n')
    for (dll, fn) in slots.values():
        if (dll, fn) not in cnt: f.write(f'{dll}\t{fn}\t0\n')
print(len(slots), 'imports;', sum(cnt.values()), 'call sites')
