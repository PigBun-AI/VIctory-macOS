#!/usr/bin/env python3
"""Validate Mac matrix scores and write the ranked TSV to stdout. Read by header, not index."""
import argparse
import csv
import math
from pathlib import Path
import re
import sys

KINDS = ('geom', 'color', 'flicker', 'slow', 'crash')
SYM = re.compile(' '.join(k + '([0-3])' for k in KINDS) + r'((?: (?:估|freq=floor|closed|summary))*)')
COST = re.compile(r'(\d+(?:\.\d+)?)(估?)')
FIELDS = ['rank', 'id', 'status', 'freq', 'symptom_max', 'cost', 'score', '下一步动作']
MATRIX = Path(__file__).resolve().parents[2] / 'docs/mac/translation-matrix-mac.csv.txt'


def rank_rows(rows):
    ranked, skipped, seen = [], [], set()
    for r in rows:
        i = r['id']
        if not i or i in seen or None in r or None in r.values():
            raise ValueError(f'missing, duplicate, or ragged row: {i}')
        seen.add(i)
        m = SYM.fullmatch(r['symptom'])
        if not m:
            raise ValueError(f'{i}: invalid symptom')
        mx = max(map(int, m.groups()[:5]))
        tags = m[6].split()
        if len(tags) != len(set(tags)):
            raise ValueError(f'{i}: duplicate symptom tag')
        cost = r['cost']
        if r['status'] == 'proven':
            if cost:
                raise ValueError(f'{i}: proven row has a cost')
            continue
        cm = COST.fullmatch(cost)
        if not cm:
            raise ValueError(f'{i}: invalid cost')
        try:
            freq = float(r['freq'])
        except ValueError:
            freq = 10000 if 'freq=floor' in tags else 1
            note = 'freq=floor' if 'freq=floor' in tags else 'freq=unknown'
        else:
            if not math.isfinite(freq) or freq < 0 or 'freq=floor' in tags:
                raise ValueError(f'{i}: invalid numeric freq or redundant floor')
            note = ''
        skip = next((reason for reason in ('closed', 'summary') if reason in tags), '')
        if not skip and r['used'] == 'no':
            skip = 'used=no'
        if not skip and (r['owner'] == 'ref-only' or '仅参照（D3DMetal）' in r['notes']):
            skip = '仅参照（D3DMetal）'
        score = math.log10(freq + 1) * mx / max(float(cm[1]), 0.25)
        action = r['restore_plan'] or r['tests'] or '补离线最小复现，对照测试再判定。'
        entry = (score, [None, i, r['status'], f'{freq:g}' + (f' ({note})' if note else ''), mx, cost,
                         '' if skip else f'{score:.6f}', skip or ' '.join(action.split())])
        (skipped if skip else ranked).append(entry)
    ranked.sort(key=lambda e: (-e[0], e[1][1]))
    for n, (_, row) in enumerate(ranked, 1):
        row[0] = n
        yield row
    for _, row in sorted(skipped, key=lambda e: e[1][1]):
        row[0] = '-'
        yield row


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('matrix', nargs='?', type=Path, default=MATRIX)
    args = ap.parse_args()
    with args.matrix.open(encoding='utf-8', newline='') as f:
        reader = csv.DictReader(f)
        required = {'id','status','owner','used','freq','notes','restore_plan','tests','symptom','cost'}
        if not required <= set(reader.fieldnames or ()):
            ap.error('missing required matrix columns')
        rows = list(rank_rows(reader))
    w = csv.writer(sys.stdout, delimiter='\t', lineterminator='\n')
    w.writerow(FIELDS)
    w.writerows(rows)


if __name__ == '__main__':
    main()
