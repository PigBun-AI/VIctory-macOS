#!/usr/bin/env bash
# Merges the translation matrices, prints the coverage summary and writes
#   docs/metal/translation-unproven.md  (used=yes and status != proven, by freq)
#   docs/metal/translation-stubbed.md   (disposition=stubbed, by freq)
#   docs/metal/translation-priority.md  (used=yes/unknown, not proven, by priority = log10(freq+1) x max(symptom) / cost; format in
#                                        docs/metal/symptom-format.md)
# Inputs: docs/metal/translation-matrix.csv.txt (this line) and the Mac lead's
# docs/mac/translation-matrix-mac.csv, looked for in this order:
#   MATRIX_MAC=<file>            that file
#   MAC_MATRIX_REF=<git ref>     `git show <ref>:docs/mac/translation-matrix-mac.csv`
#                                (default ref: fork/mac/livetr; unreadable = next)
#   the file in this checkout    docs/mac/translation-matrix-mac.csv
#   none of them                 skipped, said in the first line
# Columns are read by header name; a column the Mac file lacks is empty there.
# A row id in both files: the Mac file wins, including its symptom and cost.
# Usage: tools/metal/matrix.sh [--check]   (--check: fail if the .md files are not current)
set -euo pipefail
here="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$here"
check=0; [ "${1:-}" = "--check" ] && check=1
export MATRIX_MAC="${MATRIX_MAC:-}" MAC_MATRIX_REF="${MAC_MATRIX_REF:-fork/mac/livetr}" CHECK=$check
exec python3 - <<'PY'
import csv, os, sys, collections, re, math

MAIN = 'docs/metal/translation-matrix.csv.txt'
import io, subprocess
MAC_FILE = os.environ['MATRIX_MAC']
MAC_REF = os.environ['MAC_MATRIX_REF']
MAC_PATH = 'docs/mac/translation-matrix-mac.csv'
check = os.environ['CHECK'] == '1'
COLS = ['id','chain','hop','unit','owner','used','freq','freq_source','tests','reference','reference_detail',
        'status','disposition','xbox_hw','deviation','restore_plan','notes','symptom','cost']

def parse(f, path):
    rd = csv.DictReader(f)
    missing = [c for c in ('id','chain','owner','used','status','symptom','cost') if c not in (rd.fieldnames or [])]
    if missing:
        sys.exit(f'{path}: missing columns {missing}')
    return [{c: (row.get(c) or '') for c in COLS} for row in rd]

def load(path):
    with open(path, newline='') as f:
        return parse(f, path)

def load_mac():
    """(rows, where from) of the Mac lead's matrix, or ([], None)."""
    if MAC_FILE:
        return load(MAC_FILE), MAC_FILE
    # The repo's asset policy has no .csv suffix, so either name is accepted.
    for path in (MAC_PATH + '.txt', MAC_PATH):
        g = subprocess.run(['git', 'show', f'{MAC_REF}:{path}'], capture_output=True, text=True)
        if g.returncode == 0:
            return parse(io.StringIO(g.stdout, newline=''), f'{MAC_REF}:{path}'), f'{MAC_REF}:{path}'
    for path in (MAC_PATH + '.txt', MAC_PATH):
        if os.path.exists(path):
            return load(path), path
    return [], None

rows = {r['id']: r for r in load(MAIN)}
mac_rows, mac_from = load_mac()
mac_n = 0
if mac_rows:
    # skeleton rows (notes start with SKELETON) only stand in until the Mac lead's file exists
    rows = {i: r for i, r in rows.items() if not r['notes'].startswith('SKELETON')}
for r in mac_rows:
    mac_n += 1
    rows[r['id']] = r          # the Mac file wins
rows = list(rows.values())
# owner=ref-only: rows about D3DMetal's own behaviour (it is a reference, not a target; user decision
# 2026-11-28 00:06). disposition is n/a; they are not gaps: left out of the coverage, the unproven
# list and the stubbed list, and counted on the first line only.
ref_only = [r for r in rows if r['owner'] == 'ref-only']
rows = [r for r in rows if r['owner'] != 'ref-only']

def num(x):
    try: return float(x)
    except ValueError: return None

problems = []
mac_ids = {r['id'] for r in mac_rows}
warnings = []
for r in rows:
    problems_before = len(problems)
    if r['used'] == 'yes' and num(r['freq']) is None:
        problems.append(f"{r['id']}: used=yes without a numeric freq")
    if r['disposition'] in ('emulated','stubbed') and r['owner'] != 'mac' and not (r['deviation'] and r['restore_plan']):
        problems.append(f"{r['id']}: {r['disposition']} without deviation/restore_plan")
    if r['reference'] in ('hw','spec','d3dmetal') and r['status'] == 'proven' and not r['tests']:
        problems.append(f"{r['id']}: proven without tests")
    if r['id'] in mac_ids and len(problems) > problems_before:
        # rows of the Mac lead's file are reported, not failed on
        warnings.extend(problems[problems_before:]); del problems[problems_before:]

SYM_RE = re.compile(r'^geom([0-3]) color([0-3]) flicker([0-3]) slow([0-3]) crash([0-3])((?: (?:估|freq=floor|closed|summary))*)$')
COST_RE = re.compile(r'^(\d+(?:\.\d+)?)(估?)$')
for r in rows:
    if r['id'] in mac_ids:
        continue
    if not SYM_RE.match(r['symptom']):
        problems.append(f"{r['id']}: symptom '{r['symptom']}' is not 'geomN colorN flickerN slowN crashN [估] [freq=floor] [closed]'")
    if r['cost'] and not COST_RE.match(r['cost']):
        problems.append(f"{r['id']}: cost '{r['cost']}' is not hours (number, 估 after it when estimated)")
    if r['status'] != 'proven' and r['used'] != 'no' and not r['cost']:
        problems.append(f"{r['id']}: not proven and used but no cost")

out = []
P = out.append
P(f"rows: {len(rows)} counted + {len(ref_only)} ref-only, not counted (this line {MAIN}: {len(load(MAIN))}; Mac file {mac_from or 'not found, skipped'}: {mac_n}; skeleton rows of this file dropped when the Mac file is present)")

def stats(sel):
    used = [r for r in sel if r['used'] == 'yes']
    c = collections.Counter(r['status'] for r in used)
    return dict(total=len(sel), used=len(used), proven=c['proven'], unproven=c['unproven'], gap=c['gap'],
                other=len(used)-c['proven']-c['unproven']-c['gap'],
                unknown_used=sum(1 for r in sel if r['used'] == 'unknown'))
hdr = f"{'group':<22}{'units':>6}{'used=yes':>9}{'proven':>8}{'unproven':>9}{'gap':>5}{'used=unknown':>13}"
P(''); P('coverage (proven/unproven/gap are counted among used=yes rows)'); P(hdr)
def line(name, sel):
    s = stats(sel)
    P(f"{name:<22}{s['total']:>6}{s['used']:>9}{s['proven']:>8}{s['unproven']:>9}{s['gap']:>5}{s['unknown_used']:>13}")
line('ALL', rows)
for k in ('owner','chain'):
    for v in sorted({r[k] for r in rows}):
        line(f'{k}={v}', [r for r in rows if r[k] == v])
for o in sorted({r['owner'] for r in rows}):
    for c in sorted({r['chain'] for r in rows}):
        sel = [r for r in rows if r['owner'] == o and r['chain'] == c]
        if sel: line(f'{o}/{c}', sel)

# ---- unproven list
unp = [r for r in rows if r['used'] == 'yes' and r['status'] != 'proven']
unp.sort(key=lambda r: -(num(r['freq']) or 0))
def fmt(x):
    n = num(x)
    return '' if n is None else f'{int(n):,}'
md = ['# Used but not proven (generated by tools/metal/matrix.sh — do not edit)', '',
      'The only source of bug-fix work: rows with `used=yes` and `status` not `proven`, by `freq` (descending).',
      'freq is measured per row in its own unit (calls at seam A, programs of the package, log lines): see `freq_source`; numbers of different sources are not comparable with each other.', '',
      '| # | id | owner | chain | status | disp | freq | source | unit | tests | reference |', '|---|---|---|---|---|---|---|---|---|---|---|']
esc = lambda s: s.replace('|', '\\|').replace('\n', ' ')
for i, r in enumerate(unp, 1):
    md.append(f"| {i} | {r['id']} | {r['owner']} | {r['chain']} | {r['status']} | {r['disposition']} | {fmt(r['freq'])} | {esc(r['freq_source'])} | {esc(r['unit'])} | {esc(r['tests'] or '—')} | {r['reference']}: {esc(r['reference_detail'])} |")
# blocks
blocks = collections.defaultdict(lambda: [0, 0.0, 0.0])
for r in unp:
    b = blocks[(r['chain'], r['hop'])]
    b[0] += 1; b[1] += num(r['freq']) or 0; b[2] = max(b[2], num(r['freq']) or 0)
top = sorted(blocks.items(), key=lambda kv: -kv[1][1])[:10]
md += ['', '## Largest unproven blocks (a block = chain + hop; estimate = sum of freq of its unproven rows = freq x units where freqs are equal)', '',
       '| # | chain | hop | unproven units | estimate (sum freq) | highest single freq |', '|---|---|---|---|---|---|']
for i, ((c, h), (n, s, mx)) in enumerate(top, 1):
    md.append(f'| {i} | {c} | {esc(h)} | {n} | {int(s):,} | {int(mx):,} |')
P(''); P('largest unproven blocks (chain / hop: units, sum freq):')
for i, ((c, h), (n, s, mx)) in enumerate(top, 1):
    P(f'{i:>3}. {c} / {h}: {n} units, {int(s):,}')
P(''); P('top unproven rows by freq:')
for r in unp[:10]:
    P(f"  {r['id']} {fmt(r['freq']):>9} {r['status']:<9} {r['unit'][:90]}")

# ---- stubbed list
stub = [r for r in rows if r['disposition'] == 'stubbed']
stub.sort(key=lambda r: (num(r['freq']) is None, -(num(r['freq']) or 0)))
sm = ['# Stubbed rows (generated by tools/metal/matrix.sh — do not edit)', '',
      'Every row with `disposition=stubbed`, by in-game frequency (rows whose frequency is not measured last). "How to break through" is the row\'s `restore_plan`.',
      'Fidelity first: costs are recorded, not a reason to choose. Sources and dates are inside each plan.', '']
for i, r in enumerate(stub, 1):
    sm += [f"## {i}. {r['id']} — {r['unit']}", '',
           f"- owner / chain / hop: {r['owner']} / {r['chain']} / {r['hop']}",
           f"- freq: {fmt(r['freq']) or 'not measured'} ({r['freq_source'] or 'none'}); used={r['used']}; status={r['status']}",
           f"- Xbox hardware / driver does: {r['xbox_hw'] or '—'}",
           f"- deviation: {r['deviation'] or '—'}",
           f"- restore_plan (how to break through): {r['restore_plan'] or '—'}",
           f"- tests: {r['tests'] or '—'}",
           f"- notes: {r['notes'] or '—'}", '']
P(''); P(f'stubbed rows: {len(stub)}')
for r in stub:
    P(f"  {r['id']} {fmt(r['freq']) or 'n/m':>9}  {r['unit'][:100]}")
if warnings:
    P(''); P('WARNINGS (rows of the Mac file, not failed on):'); out.extend('  ' + w for w in warnings)
if problems:
    P(''); P('PROBLEMS:'); out.extend('  ' + p for p in problems)

# ---- priority: log10(freq+1) x max(symptom) / cost (docs/metal/symptom-format.md)
# No secondary score source; the winning row supplies both values.
ranked, skipped, unscored = [], [], []
for r in rows:
    sym, cost = r['symptom'], r['cost']
    m = SYM_RE.match(sym)
    if not m:
        unscored.append(r['id']); continue
    scores = [int(x) for x in m.groups()[:5]]
    tags = m.group(6).split()
    if r['status'] == 'proven':
        continue
    why = ('closed' if 'closed' in tags else 'summary row (its parts are rows)' if 'summary' in tags else
           'used='+r['used'] if r['used'] in ('no', 'unlikely') else
           'reference only (D3DMetal)' if r['notes'].startswith('仅参照（D3DMetal）') else None)
    if why:
        skipped.append((r['id'], r['status'], why)); continue
    fv = num(r['freq']); fnote = ''
    if fv is None:
        fv, fnote = 1.0, 'unknown=1'
    if 'freq=floor' in tags and fnote:      # words only, no number: a measured, reproducible symptom takes the floor
        fv, fnote = 1e4, 'floor 1e4'
    cm = COST_RE.match(cost) if cost else None
    cnote = ''
    if cm:
        ch = max(float(cm.group(1)), 0.25); cnote = '估' if cm.group(2) else ''
    else:
        ch, cnote = 4.0, '估(default 4)'
    mx = max(scores)
    ranked.append(dict(r=r, sym=sym, mx=mx, f=fv, fnote=fnote, cost=ch, cnote=cnote, est='估' in tags, prio=math.log10(fv + 1) * mx / ch))
ranked.sort(key=lambda x: (-x['prio'], x['r']['id']))
pm = ['# Used but not proven, by priority (generated by tools/metal/matrix.sh — do not edit)', '',
      'priority = log10(freq + 1) x max(symptom) / cost. symptom = what a wrong node does, five kinds 0-3 (geom, color, flicker, slow, crash); '
      'cost = estimated person-hours; 估 = estimated. freq unknown or words only counts as 1 (`unknown=1`), measured-and-reproducible symptom rows with no number take the floor 1e4 (`floor 1e4`). '
      'Rows that are proven, used=no, closed, summary rows or reference-only (D3DMetal) rows are not ranked. Format and rules: `docs/metal/symptom-format.md`. '
      'Mac rows use symptom and cost directly from the Mac matrix.', '',
      '| # | id | owner | status | freq | symptom | max | cost h | priority | next step |', '|---|---|---|---|---|---|---|---|---|---|']
for n, x in enumerate(ranked, 1):
    r = x['r']
    pm.append(f"| {n} | {r['id']} | {r['owner']} | {r['status']} | {fmt(x['f'])}{' (' + x['fnote'] + ')' if x['fnote'] else ''} | {esc(x['sym'])} | {x['mx']} | "
              f"{x['cost']:g}{x['cnote']} | {x['prio']:.3f} | {esc((r['restore_plan'] or '—')[:110])} |")
pm += ['', f'ranked: {len(ranked)}; not ranked (closed, summary, used=no, reference only): {len(skipped)}; rows without a parsable symptom: {len(unscored)}', '']
if skipped:
    pm += ['## Not ranked', '', '| id | status | why |', '|---|---|---|'] + [f'| {a} | {b} | {c} |' for a, b, c in skipped] + ['']
P(''); P(f'priority: {len(ranked)} ranked, {len(skipped)} not ranked, {len(unscored)} without symptom (warn)')
for n, x in enumerate(ranked[:10], 1):
    P(f"  {n:>2}. {x['r']['id']} {x['prio']:.3f}  max {x['mx']}  cost {x['cost']:g}{x['cnote']}  freq {fmt(x['f'])}")
if unscored:
    warnings.append('no symptom: ' + ', '.join(unscored))

files = {'docs/metal/translation-unproven.md': '\n'.join(md) + '\n', 'docs/metal/translation-stubbed.md': '\n'.join(sm) + '\n',
         'docs/metal/translation-priority.md': '\n'.join(pm) + '\n'}
bad = False
for p, text in files.items():
    if check:
        if not os.path.exists(p) or open(p).read() != text:
            print(f'{p}: not current (run tools/metal/matrix.sh)'); bad = True
    else:
        open(p, 'w').write(text)
print('\n'.join(out))
if problems or bad: sys.exit(1)
PY
