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
batches, other = [], []
for d, raw in rows:
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
