#!/usr/bin/env python3
"""Summarises the RES lines of rosetta-probe-wine.sh into markdown tables (stdout)."""
import re, sys, collections

def parse(line):
    d = {}
    m = re.search(r'status=(\w+)((?: [a-z_]+)*)\s*$', line)
    d['why'] = m.group(2).strip().replace(' ', '+') if m else ''
    for m in re.finditer(r'(\w[\w.]*)=("([^"]*)"|\S+)', line):
        d[m.group(1)] = m.group(3) if m.group(3) is not None else m.group(2)
    return d

rows, header = [], []
for l in open(sys.argv[1], errors='replace'):
    if l.startswith('#'): header.append(l.strip())
    elif l.startswith('RES '): rows.append((parse(l), l.strip()))
MODES = ['inpage', 'fault', 'ro', 'cross', 'crossro', 'crossfirst', 'crossfirstro', 'split']
print('\n'.join(header)); print()
store = collections.OrderedDict()
batches, other, fam_rows = [], [], []
for d, raw in rows:
    if 'fam' in d: fam_rows.append(d); continue
    if 'batch' in d: batches.append(d); continue
    if 'case' in d and d.get('mode') in MODES:
        store.setdefault(d['case'], collections.OrderedDict()).setdefault(d['mode'], []).append(d)
    else: other.append(raw)
def cell(rs):
    st = collections.Counter(r.get('status', '?') for r in rs)
    skip = [r for r in rs if 'SKIP' in str(r)]
    n = len(rs)
    if set(st) == {'PASS'}: return 'ok %d/%d' % (n, n)
    bad = [r for r in rs if r.get('status') != 'PASS']
    def tag(r):
        t = r.get('mask') or ''
        if r.get('back') and r.get('mode', '').startswith('cross') and r.get('back') != '16': t += ('' if not t else ',') + 'back' + r['back']
        return t
    parts = collections.Counter(r.get('status', '?') + ('(' + (r.get('reason') or r.get('why') or '') + ')' if (r.get('reason') or r.get('why')) else '') for r in bad)
    desc = ', '.join('%s x%d' % (k, v) for k, v in parts.items())
    tags = ';'.join(filter(None, (tag(r) for r in bad)))[:60]
    return '**%s** %d/%d bad%s' % (desc, len(bad), n, (' [' + tags + ']') if tags else '')
print('## Fault-resume matrix (one process per case/mode/mask; status of the VEH retry run)\n')
print('| case | ' + ' | '.join(MODES[:6] + MODES[7:]) + ' |'); print('|---|' + '---|' * 7)
for c, ms in store.items():
    print('| %s | ' % c + ' | '.join(cell(ms[m]) if m in ms else '-' for m in MODES[:6] + MODES[7:]) + ' |')
print()
# exception facts seen by the VEH
cnt = collections.Counter(); why = collections.Counter()
for c, ms in store.items():
    for m, rs in ms.items():
        for r in rs:
            if r.get('faults') not in (None, '0'):
                cnt[(r.get('addr'), r.get('type'), r.get('ip'))] += 1
print('## VEH view of the exceptions (cases that faulted and were retried)\n')
print('| fault address vs expected | access type | exception address == instruction | count |'); print('|---|---|---|---|')
for (a, t, i), n in sorted(cnt.items(), key=lambda x: -x[1]): print('| %s | %s | %s | %d |' % (a, t, i, n))
print()
fails = [(c, m, r) for c, ms in store.items() for m, rs in ms.items() for r in rs if r.get('status') != 'PASS']
print('## Non-PASS runs (%d)\n' % len(fails))
for c, m, r in fails:
    extra = ' '.join('%s=%s' % (k, v) for k, v in r.items() if k in ('mask', 'back', 'b', 'gran', 'faults', 'exp_faults', 'type', 'addr', 'ip', 'data', 'flags', 'reason', 'detail', 'rax_upper_on_hit', 'why'))
    print('- `%s %s` %s: %s' % (c, m, r.get('status'), extra[:420]))
print()
print('## Batches (numeric compare against the software reference)\n')
print('| batch | rm | n | mismatches | status | detail |'); print('|---|---|---|---|---|---|')
for b in batches:
    det = b.get('first', '') or ''
    extra = ' '.join('%s=%s' % (k, v) for k, v in b.items() if k.startswith('flag_verdict') or k.startswith('rax_upper') or k == 'result_mismatches')
    print('| %s | %s | %s | %s | %s | %s |' % (b.get('batch'), b.get('rm', ''), b.get('n'), b.get('mismatches', b.get('result_mismatches', '')), b.get('status'), extra or det))
print()
print('## Other\n')
for r in other: print('- ' + r[:400])

# ---------------------------------------------------------------------------------------------------------------------
# Numeric ISA families (rosetta-probe-wine.sh isa): one verdict per family. PASS = every check of the family matched the x86
# specification (rcp/rsqrt: relative error <= 1.5 * 2^-12 and the special-value rules; vcvt*: the bit-exact software model in
# softref-cvt.h, which includes the x86 integer indefinite, RC, DAZ/FTZ and NaN rules). GAP = at least one input class differs.
def clsparse(c):
    out = collections.OrderedDict()
    for part in (c or '').split(';'):
        m = re.match(r'\s*(~?[\w+]+) (\d+)/(\d+)', part)
        if m: out[m.group(1)] = (int(m.group(2)), int(m.group(3)))
    return out
if fam_rows:
    print('# Numeric ISA families\n')
    crash = [d for d in fam_rows if d.get('status') == 'CRASH']
    rr = [d for d in fam_rows if d['fam'] == 'rcp_rsqrt']
    forms = collections.OrderedDict()
    for d in rr: forms.setdefault(d['batch'], {})[d['mode']] = d
    print('## rcp_rsqrt: vrcpps / vrsqrtps (xmm, ymm) and vrcpss / vrsqrtss\n')
    print('| form | inputs checked | max rel. error (x 2^-12; spec bound 1.5) | at x -> y | special values (bad/n) | MXCSR FTZ/DAZ change a result | upper lanes (scalar) | verdict |'); print('|---|---|---|---|---|---|---|---|')
    verdicts = {}
    for f, modes in forms.items():
        dm, ind, sh = modes.get('default'), modes.get('mxcsr_independent'), modes.get('shape')
        if not dm: print('| %s | - | - | - | no result (%s) | - | - | - | CRASH |' % (f, ' '.join('%s=%s' % (k, v) for k, v in list(modes.values())[0].items() if k in ('status', 'rc', 'stderr')))); verdicts[f] = False; continue
        cl = clsparse(dm.get('cls'))
        spec = '; '.join('%s %d/%d' % (k, b, n) for k, (b, n) in cl.items())
        ok = dm.get('status') == 'PASS' and ind is not None and ind.get('status') == 'PASS' and dm.get('scalar_upper_lanes_bad', '0') == '0'
        verdicts[f] = ok
        print('| %s | %s | %s | %s -> %s | %s | %s | %s | **%s** |' % (f, dm.get('n'), dm.get('max_rel_err_x2pow12'), dm.get('at_x'), dm.get('y'), spec, ('NO: ' + ind.get('cls', '')) if ind and ind.get('status') != 'PASS' else ('no (0 of 3 x 2^32)' if ind else 'n/a'),
              dm.get('scalar_upper_lanes_bad', '-') if f.endswith('ss') else '-', 'PASS' if ok else 'GAP'))
    print()
    for f, modes in forms.items():
        sh = modes.get('shape')
        if sh: print('- `%s` shape: %s; sample points %s' % (f, ' '.join('%s=%s' % (k, v) for k, v in sh.items() if k.startswith('E1')), sh.get('values', '')))
        bad = [(m, d) for m, d in modes.items() if d.get('status') == 'FAIL']
        for m, d in bad: print('- **%s %s FAIL**: %s; first: %s' % (f, m, d.get('cls'), d.get('first', '')))
    print()
    pk = [f for f in verdicts if f.endswith(('ps_xmm', 'ps_ymm'))]; sc = [f for f in verdicts if f.endswith('ss')]
    for name, grp in (('mac.isa.avx.rcp_rsqrt_packed', pk), ('mac.isa.avx.rcp_rsqrt_scalar', sc)):
        if not grp: continue
        v = all(verdicts[f] for f in grp) and len(grp) == (4 if name.endswith('packed') else 2)
        mx = max([float(forms[f]['default'].get('max_rel_err_x2pow12', 0)) for f in grp if 'default' in forms[f]] or [0])
        print('**Verdict %s: %s** (%d forms; worst relative error %.4f x 2^-12 against the 1.5 bound; special values and MXCSR independence %s)\n' % (name, 'PASS: within the x86 architectural precision' if v else 'GAP', len(grp), mx, 'all as specified' if v else 'see failing rows above'))
    cv = [d for d in fam_rows if d['fam'] == 'cvt']
    if cv:
        print('## cvt: conversions (per form: all passes, every MXCSR variant)\n')
        agg = collections.OrderedDict()
        for d in cv:
            a = agg.setdefault(d['batch'], {'passes': 0, 'n': 0, 'bad': 0, 'cls': collections.OrderedDict(), 'bad_modes': [], 'first': ''})
            a['passes'] += 1; a['n'] += int(d.get('n', 0)); a['bad'] += int(d.get('mismatches', 0))
            for k, (b, n) in clsparse(d.get('cls')).items():
                x = a['cls'].get(k, (0, 0)); a['cls'][k] = (x[0] + b, x[1] + n)
            if d.get('status') != 'PASS': a['bad_modes'].append(d.get('mode', '?')); a['first'] = a['first'] or d.get('first', '')
        print('| form | passes | elements checked | mismatches | by input class (bad/n) | verdict |'); print('|---|---|---|---|---|---|')
        for f, a in agg.items():
            print('| %s | %d | %d | %d | %s | **%s** |' % (f, a['passes'], a['n'], a['bad'], '; '.join('%s %d/%d' % (k, b, n) for k, (b, n) in a['cls'].items() if not k.startswith('~')), 'PASS' if a['bad'] == 0 and not a['bad_modes'] else 'GAP'))
        print()
        gaps = [(f, a) for f, a in agg.items() if a['bad'] or a['bad_modes']]
        for f, a in gaps: print('- **%s GAP** in modes %s; first: %s' % (f, ', '.join(a['bad_modes'][:6]), a['first']))
        grp = {'vcvttps2dq': 'vcvttps2dq', 'vcvtps2dq': 'vcvtps2dq', 'vcvttss2si/vcvtss2si': ('vcvttss2si', 'vcvtss2si'), 'vcvttsd2si/vcvtsd2si': ('vcvttsd2si', 'vcvtsd2si'), 'vcvttpd2dq/vcvtpd2dq': ('vcvttpd2dq', 'vcvtpd2dq'),
               'vcvtdq2ps': 'vcvtdq2ps', 'vcvtsi2ss/vcvtsi2sd': ('vcvtsi2ss', 'vcvtsi2sd'), 'vcvtdq2pd': 'vcvtdq2pd', 'vcvtps2pd/vcvtss2sd': ('vcvtps2pd', 'vcvtss2sd'), 'vcvtpd2ps/vcvtsd2ss': ('vcvtpd2ps', 'vcvtsd2ss')}
        print('\n**Verdicts per conversion group:**\n')
        for g, pre in grp.items():
            pre = (pre,) if isinstance(pre, str) else pre
            fs = [f for f in agg if f.rsplit('_', 1)[0] in pre or f in pre or any(f == p or f.startswith(p + '_') for p in pre)]
            if not fs: print('- %s: no result' % g); continue
            nb = sum(agg[f]['bad'] + len(agg[f]['bad_modes']) for f in fs); n = sum(agg[f]['n'] for f in fs)
            print('- %s: **%s** (%d forms, %d elements, %d mismatches)' % (g, 'PASS' if nb == 0 else 'GAP', len(fs), n, sum(agg[f]['bad'] for f in fs)))
        allbad = sum(a['bad'] + len(a['bad_modes']) for a in agg.values())
        print('\n**Verdict mac.isa.avx.convert: %s** (%d forms, %d elements)\n' % ('PASS: bit-exact against the x86 model in every RC / DAZ / FTZ variant' if not allbad else 'GAP', len(agg), sum(a['n'] for a in agg.values())))
    # generic instruction families of isa-diff.h (avx_move, avx_fparith, ...): one table row per op, one verdict per family
    for fam in collections.OrderedDict.fromkeys(d['fam'] for d in fam_rows if d['fam'] not in ('rcp_rsqrt', 'cvt')):
        fr = [d for d in fam_rows if d['fam'] == fam]
        agg = collections.OrderedDict()
        for d in fr:
            a = agg.setdefault(d['batch'], {'passes': 0, 'el': 0, 'rec': 0, 'bad': 0, 'badmodes': [], 'first': '', 'strad': 0})
            a['passes'] += 1; a['rec'] += int(d.get('records', d.get('n', 0))); a['el'] += int(d.get('elements', d.get('n', 0))); a['bad'] += int(d.get('mismatches', 0))
            if d.get('mode', '').startswith('straddle'): a['strad'] += int(d.get('n', 0))
            if d.get('status') != 'PASS': a['badmodes'].append(d.get('mode', '?')); a['first'] = a['first'] or d.get('first', '')
        print('## %s (per op: all passes, every MXCSR variant)\n' % fam)
        print('| op | passes | records | elements | page-straddle cases | mismatches | verdict |'); print('|---|---|---|---|---|---|---|')
        for f, a in agg.items():
            print('| %s | %d | %d | %d | %d | %d | **%s** |' % (f, a['passes'], a['rec'], a['el'], a['strad'], a['bad'], 'PASS' if a['bad'] == 0 and not a['badmodes'] else 'GAP'))
        print()
        for f, a in agg.items():
            if a['bad'] or a['badmodes']: print('- **%s GAP** in modes %s; first: %s' % (f, ', '.join(a['badmodes'][:6]), a['first']))
        nb = sum(a['bad'] + len(a['badmodes']) for a in agg.values())
        print('\n**Verdict %s: %s** (%d ops, %d records, %d elements, %d page-straddle cases, %d mismatches)\n' % (fam, 'PASS' if not nb else 'GAP', len(agg), sum(a['rec'] for a in agg.values()), sum(a['el'] for a in agg.values()), sum(a['strad'] for a in agg.values()), sum(a['bad'] for a in agg.values())))
    for d in crash: print('- **CRASH** %s %s: %s' % (d.get('fam'), d.get('batch'), d.get('stderr', '')))
