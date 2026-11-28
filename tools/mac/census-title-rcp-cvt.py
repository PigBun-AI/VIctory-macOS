#!/usr/bin/env python3
"""Static context census of the title's reciprocal-approximation and float->int conversion sites.

Reads `llvm-objdump -d --x86-asm-syntax=intel -j .text <image>` from stdin and the image path as argv[1]
(for .pdata), writes to the output directory argv[2] (outside the repository: game-derived data):
  rcp-context.tsv    one line per vrcpps/vrsqrtps/vrcpss/vrsqrtss site: rva, function, mnemonic, width, class
  rcp-summary.txt    per mnemonic: counts per class (newton / mul_chain / single_mul / other), top functions
  rcp-unrefined-sites.txt  RVAs (hex) of the sites that are NOT followed by a refinement step
  rcp-samples.txt    disassembly windows (3 before, 12 after) for the densest functions and for a few unrefined sites
  cvt-context.tsv    one line per float->int conversion site (vcvt[t]ps2dq, vcvt[t]ss2si, vcvt[t]sd2si, vcvt[t]pd2dq)
  cvt-summary.txt    per mnemonic: guarded by min/max/compare within 8 instructions before, followed by a compare
                     with 0x80000000 (an 'integer indefinite' test), neither

The classification is a window heuristic, not a data-flow proof (there is no execution profile either):
  newton      >= 2 multiply/FMA instructions in the dependency chain that starts at the approximation result within
              12 instructions, and one of them reads the source register (or a copy of it): a Newton-Raphson step
  mul_chain   >= 2 multiply/FMA instructions in the chain, none reads the source (or an alias)
  single_mul  exactly 1 multiply/FMA in the chain (approximate sqrt = x*rsqrt(x), approximate division a*rcp(b))
  feeds_approx the result is the source of another approximation (vrcp(vrsqrt(x)) = approximate sqrt, unrefined)
  other       none (stored, compared, converted, selected)
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
    secs[d[a:a + 8].rstrip(b'\0').decode()] = struct.unpack_from('<IIII', d, a + 8)
vs, va, rs, raw = secs['.pdata']
starts = sorted({struct.unpack_from('<I', d, raw + 12 * i)[0] for i in range(min(vs, rs) // 12)})
del d
def func_of(rva):
    i = bisect.bisect_right(starts, rva) - 1
    return starts[i] if i >= 0 else 0

RCP = {'vrcpps', 'vrsqrtps', 'vrcpss', 'vrsqrtss'}
CVT = {'vcvtps2dq', 'vcvttps2dq', 'vcvtss2si', 'vcvttss2si', 'vcvtsd2si', 'vcvttsd2si', 'vcvtpd2dq', 'vcvttpd2dq'}
MULS = re.compile(r'^(vmul(ps|ss|pd|sd)|vf(n?m)(add|sub)(132|213|231)(ps|ss|pd|sd))$')
ARITH = re.compile(r'^v(add|sub|mul|div|max|min)(ps|ss|pd|sd)$|^vf(n?m)(add|sub)(132|213|231)(ps|ss|pd|sd)$')
MOVS = {'vmovaps', 'vmovups', 'vmovdqa', 'vmovdqu', 'vmovapd', 'vmovupd'}
CLAMP = re.compile(r'^v(min|max)(ps|ss|pd|sd)$|^vp(min|max)[su]?[dwb]$|^vcmp|^vpcmp')
ENDS = {'ret', 'retq', 'jmp', 'call', 'int3', 'ud2'}
WIN = int(os.environ.get('RCP_WINDOW', '24'))

def regs(op):
    # xmm3 / ymm3 -> 'r3' (same architectural register), gpr names are left alone
    out = []
    for m in re.finditer(r'\b([xy]mm)(\d+)\b', op):
        out.append('r' + m.group(2))
    return out

pat2 = re.compile(r'^\s*([0-9a-f]+):\s+(?:[0-9a-f]{2} )+\s*\t(.*)$')
class Ins:
    __slots__ = ('rva', 'mn', 'ops', 'opl', 'txt')
    def __init__(s, rva, txt):
        s.rva = rva; s.txt = txt
        p = txt.split(None, 1); s.mn = p[0]; s.ops = p[1] if len(p) > 1 else ''
        s.opl = [o.strip() for o in re.split(r',(?![^\[]*\])', s.ops)] if s.ops else []

before = collections.deque(maxlen=8)
pending = []          # [site dict, remaining, taint set, nmul, reads_x, alias set]
rcp_rows, cvt_rows = [], []
rcp_cls = collections.defaultdict(collections.Counter); rcp_fn = collections.defaultdict(collections.Counter)
cvt_cls = collections.defaultdict(collections.Counter)
unrefined = []
windows = {}      # rva -> (before list, after list)
cvt_pending = []  # [site, remaining]
total = 0

def finish(site):
    n = site['nmul']
    c = 'newton' if n >= 2 and site['readsx'] else ('mul_chain' if n >= 2 else ('single_mul' if n == 1 else 'other'))
    if c == 'other' and site.get('feeds'): c = 'feeds_approx'
    site['cls'] = c
    rcp_cls[site['mn']][c] += 1; rcp_fn[site['mn']][site['fn']] += 1
    rcp_rows.append(site)
    if c != 'newton': unrefined.append(site['rva'])

def step_pending(ins):
    keep = []
    for p in pending:
        site = p['site']; p['after'].append(ins.txt)
        if ins.mn in RCP and ins.opl and set(regs(ins.opl[-1])) & p['taint']: site['feeds'] = True
        stop = ins.mn in ENDS
        if not stop:
            rr = set(regs(ins.ops))
            dst = regs(ins.opl[0]) if ins.opl else []
            tainted_read = bool(rr & p['taint'])
            # a result that overwrites a tainted register with something unrelated ends that taint
            if tainted_read and ARITH.match(ins.mn) and dst:
                p['taint'].add(dst[0])
                if MULS.match(ins.mn):
                    p['nmul'] += 1
                    if rr & p['xalias'] or p['xmem'] and p['xmem'] in ins.ops: site['readsx'] = True
                    site['nmul'] = p['nmul']
            elif tainted_read and ins.mn in MOVS and dst:
                p['taint'].add(dst[0])
            elif dst and not tainted_read and not ins.mn.startswith(('vcmp', 'vucomi', 'vcomi', 'vptest', 'vtest')) and dst[0] in p['taint'] and ins.opl and not ins.opl[0].endswith(']'):
                p['taint'].discard(dst[0])
        p['left'] -= 1
        if stop or p['left'] <= 0:
            finish(site)
            if site['rva'] in windows: windows[site['rva']] = (windows[site['rva']][0], p['after'])
        else:
            keep.append(p)
    pending[:] = keep

def step_cvt(ins):
    keep = []
    for c in cvt_pending:
        c['left'] -= 1
        if re.match(r'^cmp$', ins.mn) and re.search(r'0x80000000|0xffffffff80000000|-2147483648|0x8000000000000000', ins.ops, re.I): c['site']['indef'] = True
        if c['left'] <= 0 or ins.mn in ENDS:
            s = c['site']; cls = 'indefinite_test_follows' if s.get('indef') else ('guarded_by_min_max_or_compare' if s['guard'] else 'unguarded')
            s['cls'] = cls; cvt_cls[s['mn']][cls] += 1; cvt_rows.append(s)
        else: keep.append(c)
    cvt_pending[:] = keep

for line in sys.stdin:
    m = pat2.match(line)
    if not m: continue
    rva = int(m.group(1), 16) - base
    txt = m.group(2).split('#')[0].strip()
    if not txt: continue
    total += 1
    ins = Ins(rva, txt)
    if pending: step_pending(ins)
    if cvt_pending: step_cvt(ins)
    mn = ins.mn
    if mn in RCP:
        dst = regs(ins.opl[0])[0] if ins.opl and regs(ins.opl[0]) else None
        scal = mn.endswith('ss')
        srcop = ins.opl[-1] if len(ins.opl) > 1 else ''
        xreg = regs(srcop)
        xmem = srcop if srcop.endswith(']') else None
        alias = set(xreg)
        for b in before:
            if b.mn in MOVS and len(b.opl) == 2:
                a, c = regs(b.opl[0]), regs(b.opl[1])
                if a and c and (c[0] in alias or a[0] in alias): alias.update([a[0], c[0]])
        if scal and len(ins.opl) == 3: pass
        site = {'rva': rva, 'fn': func_of(rva), 'mn': mn, 'width': 'ymm' if 'ymm' in ins.ops else 'xmm', 'nmul': 0, 'readsx': False, 'srcmem': bool(xmem)}
        windows[rva] = ([b.txt for b in list(before)[-3:]] + [txt], [])
        if dst: pending.append({'site': site, 'left': WIN, 'taint': {dst}, 'nmul': 0, 'xalias': alias, 'xmem': xmem, 'after': []})
        else: finish(site)
    elif mn in CVT:
        guard = any(CLAMP.match(b.mn) for b in before)
        cvt_pending.append({'site': {'rva': rva, 'fn': func_of(rva), 'mn': mn, 'guard': guard, 'width': 'ymm' if 'ymm' in ins.ops else 'xmm'}, 'left': 4})
    before.append(ins)
for p in pending: finish(p['site'])
for c in cvt_pending:
    s = c['site']; cls = 'indefinite_test_follows' if s.get('indef') else ('guarded_by_min_max_or_compare' if s['guard'] else 'unguarded')
    s['cls'] = cls; cvt_cls[s['mn']][cls] += 1; cvt_rows.append(s)

with open(os.path.join(outdir, 'rcp-context.tsv'), 'w') as f:
    f.write('rva\tfunction\tmnemonic\twidth\tsource_in_memory\tclass\n')
    for s in sorted(rcp_rows, key=lambda s: s['rva']): f.write(f"{s['rva']:x}\t{s['fn']:x}\t{s['mn']}\t{s['width']}\t{int(s['srcmem'])}\t{s['cls']}\n")
with open(os.path.join(outdir, 'rcp-unrefined-sites.txt'), 'w') as f:
    f.write('\n'.join(f'{r:x}' for r in sorted(unrefined)) + '\n')
with open(os.path.join(outdir, 'cvt-context.tsv'), 'w') as f:
    f.write('rva\tfunction\tmnemonic\twidth\tclass\n')
    for s in sorted(cvt_rows, key=lambda s: s['rva']): f.write(f"{s['rva']:x}\t{s['fn']:x}\t{s['mn']}\t{s['width']}\t{s['cls']}\n")
out = [f'{total} instructions in {os.path.basename(image)}', '']
out.append('== reciprocal approximations: class per mnemonic (window %d instructions)' % WIN)
allc = collections.Counter()
for mn in sorted(rcp_cls):
    t = sum(rcp_cls[mn].values()); allc.update(rcp_cls[mn])
    out.append(f"{mn}\t{t}\t" + '  '.join(f'{k}={v}' for k, v in rcp_cls[mn].most_common()))
t = sum(allc.values()); out.append(f"all\t{t}\t" + '  '.join(f'{k}={v} ({100*v/t:.1f}%)' for k, v in allc.most_common()))
out.append('')
funcs = collections.Counter(); fcls = collections.defaultdict(collections.Counter)
for s in rcp_rows: funcs[s['fn']] += 1; fcls[s['fn']][s['cls']] += 1
out.append(f'functions containing a site: {len(funcs)}; top 15 by site count:')
for fn, n in funcs.most_common(15): out.append(f'  fn {fn:#x}: {n} sites  ' + '  '.join(f'{k}={v}' for k, v in fcls[fn].most_common()))
out.append('')
out.append('== float->int conversions: context per mnemonic')
allv = collections.Counter()
for mn in sorted(cvt_cls):
    t = sum(cvt_cls[mn].values()); allv.update(cvt_cls[mn])
    out.append(f"{mn}\t{t}\t" + '  '.join(f'{k}={v}' for k, v in cvt_cls[mn].most_common()))
t = sum(allv.values()); out.append(f"all\t{t}\t" + '  '.join(f'{k}={v} ({100*v/t:.1f}%)' for k, v in allv.most_common()))
open(os.path.join(outdir, 'rcp-cvt-summary.txt'), 'w').write('\n'.join(out) + '\n')
# sample windows: densest functions first (any class), then a few unrefined ones
samp = []
def add(rva, tag):
    if rva in windows and len(samp) < 40:
        b, a = windows[rva]; samp.append(f'--- {tag} site {rva:#x}\n' + '\n'.join('    ' + x for x in b + a))
bysite = {s['rva']: s for s in rcp_rows}
for fn, n in funcs.most_common(6):
    for s in [x for x in rcp_rows if x['fn'] == fn][:2]: add(s['rva'], f"[{s['cls']}] fn {fn:#x} ({n} sites) {s['mn']}")
seen = set()
for s in rcp_rows:
    if s['cls'] != 'newton' and (s['mn'], s['cls']) not in seen:
        seen.add((s['mn'], s['cls'])); add(s['rva'], f"[{s['cls']}] {s['mn']}")
for s in rcp_rows[::max(1, len(rcp_rows) // 12)]: add(s['rva'], f"[{s['cls']}] sample {s['mn']}")
open(os.path.join(outdir, 'rcp-samples.txt'), 'w').write('\n'.join(samp) + '\n')
print('\n'.join(out))
