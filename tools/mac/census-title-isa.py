#!/usr/bin/env python3
"""Static census of the title's x86-64 instruction classes that matter under Rosetta 2.

Reads `llvm-objdump -d --x86-asm-syntax=intel -j .text <image>` from stdin (the mapped copy
VIctory writes to its cache, victory-image-<hash>.dll) and the image path as argv[1] (for .pdata).
Writes, to the output directory argv[2] (outside the repository: game-derived data):
  isa-counts.tsv      class, count, functions, top-function-share
  isa-functions.tsv   class, function begin RVA, count        (per-function distribution)
  isa-sites-<class>.txt  RVAs (hex) of each site, for the classes that matter
Only counts go into the repository (docs/mac/translation-matrix-mac.csv).
There is no execution profile here: 'hot path' needs a runtime sampler (see translation-map-mac.md).
"""
import sys, re, struct, bisect, collections, os

image, outdir = sys.argv[1], sys.argv[2]
os.makedirs(outdir, exist_ok=True)
d = open(image, 'rb').read()
pe = struct.unpack_from('<I', d, 0x3c)[0]
nsec = struct.unpack_from('<H', d, pe + 6)[0]
opt = struct.unpack_from('<H', d, pe + 20)[0]
base = struct.unpack_from('<Q', d, pe + 48)[0]
secs = {}
for i in range(nsec):
    a = pe + 24 + opt + 40 * i
    secs[d[a:a + 8].rstrip(b'\0').decode()] = struct.unpack_from('<IIII', d, a + 8)  # vsize, va, rsize, raw
vs, va, rs, raw = secs['.pdata']
starts = sorted({struct.unpack_from('<I', d, raw + 12 * i)[0] for i in range(min(vs, rs) // 12)})
del d

def func_of(rva):
    i = bisect.bisect_right(starts, rva) - 1
    return starts[i] if i >= 0 else 0

# (class, predicate on (mnemonic, operands text))
def mem_dest(ops):  # intel syntax: destination first
    return ops.startswith(('byte ptr', 'word ptr', 'dword ptr', 'qword ptr', 'xmmword ptr', 'ymmword ptr', 'zmmword ptr', 'tbyte ptr', 'xword ptr', 'ptr', 'oword ptr'))

LOCKABLE = ('add', 'adc', 'sub', 'sbb', 'and', 'or', 'xor', 'inc', 'dec', 'neg', 'not', 'bts', 'btr', 'btc', 'xadd')
BMI = {'andn', 'bextr', 'blsi', 'blsmsk', 'blsr', 'bzhi', 'pdep', 'pext', 'mulx', 'rorx', 'sarx', 'shlx', 'shrx', 'tzcnt'}
counts = collections.Counter(); perfn = collections.defaultdict(collections.Counter)
sites = collections.defaultdict(list)
ymm_any = 0; total = 0; pending_lock = False; after_cmpxchg = None

def hit(cls, rva):
    counts[cls] += 1
    perfn[cls][func_of(rva)] += 1
    sites[cls].append(rva)

pat = re.compile(r'^\s*([0-9a-f]+):\s+(?:[0-9a-f]{2} )+\s*\t([a-z0-9_.]+)(?:\s+(.*))?$')
pat2 = re.compile(r'^\s*([0-9a-f]+):\s+(?:[0-9a-f]{2} )+\s*\t(.*)$')
for line in sys.stdin:
    m = pat2.match(line)
    if not m:
        continue
    addr = int(m.group(1), 16) - base
    txt = m.group(2).split('#')[0].strip()
    total += 1
    parts = txt.split(None, 1)
    mn = parts[0]; ops = parts[1] if len(parts) > 1 else ''
    locked = pending_lock
    pending_lock = False
    if mn == 'lock':
        locked = True
        p2 = ops.split(None, 1)
        if not p2:  # llvm-objdump prints a lone `lock` line before some instructions
            pending_lock = True
            continue
        mn = p2[0]; ops = p2[1] if len(p2) > 1 else ''
    if mn in ('rep', 'repe', 'repne', 'repz', 'repnz', 'bnd', 'notrack', 'xacquire', 'xrelease', 'data16'):
        p2 = ops.split(None, 1)
        if p2:
            mn = p2[0]; ops = p2[1] if len(p2) > 1 else ''
    if 'zmm' in ops or re.search(r'\bk[0-7]\b', ops):
        hit('avx512_zmm_or_kreg', addr)
    if 'ymm' in ops:
        ymm_any += 1
    if after_cmpxchg is not None:
        zf_only = ('je', 'jz', 'jne', 'jnz', 'sete', 'setz', 'setne', 'setnz', 'cmove', 'cmovz', 'cmovne', 'cmovnz')
        cc = re.match(r'^(j|set|cmov)(?!mp$)[a-z]+$', mn)
        if mn in zf_only: hit('lock_cmpxchg_next_zf_only', after_cmpxchg)
        elif cc: hit('lock_cmpxchg_next_other_flags', after_cmpxchg)
        else: hit('lock_cmpxchg_next_not_a_flag_reader', after_cmpxchg)
        after_cmpxchg = None
    md = mem_dest(ops)
    if mn in ('vpmaskmovd', 'vpmaskmovq', 'vmaskmovps', 'vmaskmovpd'):
        if md:
            hit('masked_store_' + mn + ('_ymm' if 'ymmword' in ops else '_xmm'), addr); hit('masked_store_all', addr)
        else:
            hit('masked_load_' + mn, addr)
    elif mn == 'vcvtps2ph':
        hit('f16c_vcvtps2ph', addr)
        if md: hit('vcvtps2ph_to_memory', addr)
    elif mn == 'vcvtph2ps':
        hit('f16c_vcvtph2ps', addr)
    elif mn in ('vextractf128', 'vextracti128') and md:
        hit('vextract128_to_memory', addr)
    elif mn.startswith('vmovnt') or mn.startswith('movnt') and mn not in ('movntdqa',) :
        if mn in ('vmovntdqa',): pass
        else: hit('nontemporal_store_' + mn, addr); hit('nontemporal_store_all', addr)
    elif mn in ('clwb', 'clflushopt', 'clflush'):
        hit(mn, addr)
    elif mn == 'cmpxchg16b':
        hit('lock_cmpxchg16b' if locked else 'cmpxchg16b_nolock', addr)
    elif mn == 'cmpxchg':
        hit('lock_cmpxchg' if locked else 'cmpxchg_nolock', addr)
        if locked and md: hit('lock_cmpxchg_mem', addr)
        if locked: after_cmpxchg = addr
    elif mn == 'xchg' and md or mn == 'xchg' and ops.rstrip().endswith(']') and 'ptr' in ops:
        hit('xchg_mem_implicit_lock', addr)
    elif locked and mn in LOCKABLE:
        hit('lock_' + mn, addr); hit('lock_other_all', addr)
    elif mn in ('rdtsc', 'rdtscp', 'xgetbv', 'cpuid', 'xsave', 'xsavec', 'xsaveopt', 'xrstor', 'xsaves', 'xrstors', 'rdrand', 'rdseed', 'rdpid', 'rdpru', 'rdpmc', 'lfence', 'sfence', 'mfence', 'pause', 'syscall', 'int', 'ud2', 'wbnoinvd', 'wrfsbase', 'rdfsbase', 'rdgsbase', 'wrgsbase', 'clzero', 'monitorx', 'mwaitx', 'monitor', 'mwait', 'xbegin', 'xend', 'xtest', 'fxsave', 'fxrstor', 'xsetbv', 'in', 'out', 'cli', 'sti', 'hlt', 'rdmsr', 'wrmsr'):
        hit(mn, addr)
    elif mn in BMI:
        hit('bmi_' + mn, addr); hit('bmi_all', addr)
    elif mn in ('lzcnt', 'popcnt'):
        hit(mn, addr)
    elif mn.startswith(('vfmadd', 'vfmsub', 'vfnmadd', 'vfnmsub', 'vfmaddsub', 'vfmsubadd')):
        hit('fma_all', addr)
    elif mn.startswith(('vpgather', 'vgather')):
        hit('avx2_gather', addr)
    elif mn in ('extrq', 'insertq', 'movntss', 'movntsd'):
        hit('sse4a_' + mn, addr)
    elif mn in ('(bad)', '<unknown>') or mn.startswith('.byte'):
        hit('undecoded', addr)
    # 256-bit plain stores (what loader wide.rs splits)
    if md and 'ymmword ptr' in ops and mn.startswith('v') and mn not in ('vpmaskmovd', 'vpmaskmovq', 'vmaskmovps', 'vmaskmovpd', 'vextractf128', 'vextracti128', 'vcvtps2ph') and not mn.startswith('vmovnt'):
        # destination is memory and the source operand is a ymm register
        if re.search(r'\],\s*ymm\d+$', ops):
            hit('store256_plain', addr)
    if mn.startswith('v') and re.match(r'^ymm|ymmword', ops): pass
    if mn.startswith('v'): counts['avx_vex_any'] += 1

counts['instructions_total'] = total
counts['instructions_using_ymm'] = ymm_any
with open(os.path.join(outdir, 'isa-counts.tsv'), 'w') as f:
    f.write('class\tcount\tfunctions\ttop1_fn_share\ttop1_fn_rva\n')
    for cls, n in sorted(counts.items(), key=lambda kv: -kv[1]):
        pf = perfn.get(cls)
        if pf:
            top = pf.most_common(1)[0]
            f.write(f'{cls}\t{n}\t{len(pf)}\t{top[1]/n:.3f}\t{top[0]:#x}\n')
        else:
            f.write(f'{cls}\t{n}\t\t\t\n')
with open(os.path.join(outdir, 'isa-functions.tsv'), 'w') as f:
    f.write('class\tfunction_rva\tcount\n')
    for cls, pf in perfn.items():
        for fn, n in pf.most_common(40):
            f.write(f'{cls}\t{fn:#x}\t{n}\n')
for cls in sites:
    if cls.startswith(('masked_store_all', 'store256_plain', 'vcvtps2ph_to_memory', 'vextract128_to_memory', 'clwb', 'clflushopt', 'lock_cmpxchg', 'rdtsc', 'rdtscp', 'xgetbv', 'cpuid', 'avx512', 'nontemporal_store_all', 'clzero', 'monitorx', 'mwaitx')):
        with open(os.path.join(outdir, f'isa-sites-{cls}.txt'), 'w') as f:
            f.write('\n'.join(f'{r:x}' for r in sorted(sites[cls])) + '\n')
print(f'{total} instructions; wrote {outdir}')
