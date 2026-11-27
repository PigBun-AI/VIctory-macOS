#!/bin/sh
# Report of drop-ab.sh: see the docstring below (python3, inline: the repository policy has no .py files).
exec python3 - "$@" <<'PYTHON_END'
"""Reads the logs of runs made with VICTORY_MAC_DROP_STATS=2 (docs/mac/PATCHES.md, G8)
and says where the title's draws went in the first N world frames.

  tools/mac/drop-stats-report.sh RUN_DIR [RUN_DIR ...] [--frames N] [--json]

RUN_DIR holds stderr.log (the title's log: `drop-stats ...` lines), stdout.log
(D3DMetal's `marking PSO(n) as no-op` lines) and, if drop-ab.sh made it,
stdout.ts.log (the same lines with the epoch second each was seen at). With
two or more directories the shares are printed side by side.

What is counted, per run, over world frames 1..N (world frame 1 = the first
present with >= VICTORY_WORLD_DRAWS draws; `mac::world`):
  recorded   draws + dispatches the title recorded (ExecuteIndirect counts as a draw)
  ran        those the shared layer handed to the host (this is the shared layer's
             "drawn"; it includes draws on pipelines D3DMetal marked no-op)
  dropped    those it did not run, split by class of reason:
             compiling | failed | never_requested | rt_builder_off | skip_list |
             kind_mismatch | bind_failed | bind_or_index_failed | null_pipeline | ind
             (ind = ExecuteIndirect: the detail is in the `top` lines)
  by kind    mesh | vs | vs+rect_gs | vs+gs | tess | compute, dropped draws by pipeline type
  notimpl    the title's calls of the unimplemented slots 0x1c8 and 0x218 (no pipeline, so
             no draw is ever recorded for them)
  no-op      D3DMetal's `marking PSO(n) as no-op`: how many, and -- if the numbers can be
             tied to our host pipelines -- the draws that ran with them ("noop_draws")
             and which shaders they are. If they cannot, the number per world frame.
"""
import json
import re
import sys
from collections import Counter, defaultdict
from datetime import datetime, timezone

KV = re.compile(r'(\w+)=("(?:[^"\\]|\\.)*"|\[[^\]]*\]|\S+)')
TS = re.compile(r'^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?)Z')
PRESENT = re.compile(
    r'drop-stats present=(\d+) world_frame=(\d+) recorded=(\d+)\+(\d+) ran=(\d+) dropped=(\d+) '
    r'notimpl\[0x1c8=(\d+) 0x218=(\d+)\] why\[([^\]]*)\] kind\[([^\]]*)\] psoran\[([^\]]*)\]')
TOP = re.compile(r'drop-stats top span=(\w+) n=(\d+) op=(\S+) kind=(\S+) origin=(\S+) why=(\S+) e\.g\. (.*)$')
NOOP = re.compile(r'marking PSO\((\d+)\) as no-op')


def epoch(line):
    m = TS.match(line)
    if not m:
        return None
    t = m.group(1)
    fmt = '%Y-%m-%dT%H:%M:%S.%f' if '.' in t else '%Y-%m-%dT%H:%M:%S'
    return datetime.strptime(t, fmt).replace(tzinfo=timezone.utc).timestamp()


def pairs(text):
    out = Counter()
    for item in text.split():
        if '=' in item:
            k, n = item.rsplit('=', 1)
            out[k] += int(n)
    return out


def unq(v):
    return v[1:-1] if v.startswith('"') and v.endswith('"') else v


def load(run):
    d = {'run': run, 'presents': [], 'psos': [], 'top': {}, 'noops': []}
    try:
        err = open(f'{run}/stderr.log', errors='replace').read().splitlines()
    except OSError as e:
        raise SystemExit(f'{run}: no stderr.log ({e})')
    for line in err:
        ts = epoch(line)
        m = PRESENT.search(line)
        if m:
            g = m.groups()
            d['presents'].append({
                'ts': ts, 'present': int(g[0]), 'world': int(g[1]), 'draws': int(g[2]), 'dispatches': int(g[3]),
                'ran': int(g[4]), 'dropped': int(g[5]), 'notimpl': (int(g[6]), int(g[7])),
                'why': pairs(g[8]), 'kind': pairs(g[9]),
                'psoran': {int(a): int(b) for a, b in (x.split(':') for x in g[10].split())},
            })
            continue
        if 'drop-stats pso seq=' in line:
            kv = {k: unq(v) for k, v in KV.findall(line.split('drop-stats pso', 1)[1])}
            d['psos'].append({'ts': ts, 'seq': int(kv['seq']), 'gseq': int(kv['gseq']), 'kind': kv['kind'],
                              'shaders': kv.get('shaders', ''), 'key': kv.get('key', '')})
            continue
        if 'drop-stats pso refused' in line:
            kv = {k: unq(v) for k, v in KV.findall(line.split('refused', 1)[1])}
            d.setdefault('refused', []).append(kv)
            continue
        m = TOP.search(line)
        if m:
            span, n, op, kind, origin, why, e = m.groups()
            # the summaries are cumulative: the last one of each span wins
            d['top'].setdefault(span, {}).setdefault('_stamp', 0)
            if d['top'][span].get('_last') != ts:
                d['top'][span] = {'_last': ts, '_stamp': 0}
            d['top'][span][(op, kind, origin, why)] = (int(n), e)
    # D3DMetal's no-op lines: with times if drop-ab.sh stamped them.
    try:
        for line in open(f'{run}/stdout.ts.log', errors='replace'):
            t, _, rest = line.partition('\t')
            m = NOOP.search(rest)
            if m:
                d['noops'].append((float(t), int(m.group(1))))
    except OSError:
        try:
            for line in open(f'{run}/stdout.log', errors='replace'):
                m = NOOP.search(line)
                if m:
                    d['noops'].append((None, int(m.group(1))))
        except OSError:
            pass
    return d


def correlate(d):
    """Which numbering of D3DMetal's PSO(n) fits our host pipelines: the n-th created (all kinds),
    or the n-th graphics/mesh one. A numbering fits when the pipeline a no-op names was created
    shortly before the no-op line (D3DMetal compiles right after the create call)."""
    timed = [(t, n) for t, n in d['noops'] if t is not None]
    result = {}
    if not timed or not d['psos']:
        return result
    for name, field in (('seq', 'seq'), ('gseq', 'gseq')):
        by = {p[field]: p for p in d['psos'] if p[field]}
        lags = []
        for t, n in timed:
            p = by.get(n)
            if p and p['ts']:
                lags.append(t - p['ts'])
        ok = sum(1 for l in lags if -2.0 <= l <= 10.0)
        lags.sort()
        result[name] = {
            'matched': len(lags), 'of': len(timed), 'plausible': ok,
            'median_lag_s': lags[len(lags) // 2] if lags else None,
        }
    best = max(result, key=lambda k: result[k]['plausible'])
    result['best'] = best
    result['correlated'] = result[best]['plausible'] >= 0.9 * len(timed)
    return result


def summarize(d, frames):
    sel = [p for p in d['presents'] if 1 <= p['world'] <= frames]
    s = Counter()
    why, kind, noop_by_frame = Counter(), Counter(), Counter()
    ran_by_gseq = Counter()
    for p in sel:
        s['draws'] += p['draws']
        s['dispatches'] += p['dispatches']
        s['ran'] += p['ran']
        s['dropped'] += p['dropped']
        s['notimpl_1c8'] += p['notimpl'][0]
        s['notimpl_218'] += p['notimpl'][1]
        why.update(p['why'])
        kind.update(p['kind'])
        ran_by_gseq.update(p['psoran'])
    out = {
        'run': d['run'], 'world_frames': len(sel),
        'recorded': s['draws'] + s['dispatches'], 'draws': s['draws'], 'dispatches': s['dispatches'],
        'ran': s['ran'], 'dropped': s['dropped'], 'why': dict(why), 'kind': dict(kind),
        'notimpl_0x1c8': s['notimpl_1c8'], 'notimpl_0x218': s['notimpl_218'],
        'notimpl_0x1c8_total': sum(p['notimpl'][0] for p in d['presents']),
        'notimpl_0x218_total': sum(p['notimpl'][1] for p in d['presents']),
        'psos_made': len(d['psos']), 'psos_refused': len(d.get('refused', [])),
        'noops_total': len(d['noops']),
    }
    first_world = next((p for p in d['presents'] if p['world'] == 1), None)
    last_sel = sel[-1] if sel else None
    out['noops_before_world'] = out['noops_in_world_window'] = None
    if first_world and first_world['ts'] and any(t is not None for t, _ in d['noops']):
        t0 = first_world['ts']
        t1 = last_sel['ts'] if last_sel else t0
        out['noops_before_world'] = sum(1 for t, _ in d['noops'] if t is not None and t < t0)
        out['noops_in_world_window'] = sum(1 for t, _ in d['noops'] if t is not None and t0 <= t <= t1)
        # per world frame, by the time each line was seen (the line's own lag is a second or so)
        bounds = [(p['ts'], p['world']) for p in sel]
        for t, _ in d['noops']:
            if t is None or t < t0:
                continue
            for pts, w in bounds:
                if pts is not None and t <= pts:
                    noop_by_frame[w] += 1
                    break
        out['noops_by_world_frame'] = dict(sorted(noop_by_frame.items())) if noop_by_frame else {}
    cor = correlate(d)
    out['correlation'] = cor
    if cor.get('correlated') and ran_by_gseq:
        field = cor['best']
        by = {p[field]: p for p in d['psos'] if p[field]}
        gs = {p['seq']: p['gseq'] for p in d['psos']}
        noop_g = set()
        for _, n in d['noops']:
            p = by.get(n)
            if p and p['gseq']:
                noop_g.add(p['gseq'])
        hit = sum(c for g, c in ran_by_gseq.items() if g in noop_g)
        out['noop_pipelines'] = len(noop_g)
        out['noop_draws'] = hit
        names = Counter()
        for g in noop_g:
            p = next(q for q in d['psos'] if q['gseq'] == g)
            names[(p['kind'], p['shaders'][:90], ran_by_gseq.get(g, 0))] += 1
        out['noop_top'] = [(k[0], k[1], k[2]) for k, _ in sorted(names.items(), key=lambda kv: -kv[0][2])[:15]]
    elif ran_by_gseq:
        out['noop_draws'] = None
    top = d['top'].get('world') or d['top'].get('all') or {}
    out['top_cumulative_world'] = sorted(
        ((k, v) for k, v in top.items() if not k in ('_last', '_stamp')), key=lambda kv: -kv[1][0])[:15]
    return out


def pct(n, of):
    return f'{100.0 * n / of:5.1f}%' if of else '    - '


def show(sums, frames):
    rows = []
    cols = [s['run'].rstrip('/').split('/')[-1] for s in sums]
    if len(set(cols)) < len(cols):   # drop-ab.sh's folders are all called run: add the folder above (cold/run, warm/run)
        cols = ['/'.join(s['run'].rstrip('/').split('/')[-2:]) for s in sums]
    print(f'first {frames} world frames (frames actually logged: {[s["world_frames"] for s in sums]})')
    w = 24
    print(f'{"":{w}}' + ''.join(f'{c[:26]:>28}' for c in cols))

    def line(label, f):
        print(f'{label:{w}}' + ''.join(f'{f(s):>28}' for s in sums))
    line('recorded', lambda s: f'{s["recorded"]}')
    line('ran (shared "drawn")', lambda s: f'{s["ran"]} {pct(s["ran"], s["recorded"])}')
    line('dropped', lambda s: f'{s["dropped"]} {pct(s["dropped"], s["recorded"])}')
    classes = sorted({c for s in sums for c in s['why']})
    for c in classes:
        line(f'  {c}', lambda s, c=c: f'{s["why"].get(c, 0)} {pct(s["why"].get(c, 0), s["recorded"])}')
    kinds = sorted({c for s in sums for c in s['kind']})
    line('dropped by pipeline kind', lambda s: '')
    for c in kinds:
        line(f'  {c}', lambda s, c=c: f'{s["kind"].get(c, 0)} {pct(s["kind"].get(c, 0), s["recorded"])}')
    line('notimpl 0x1c8 (window)', lambda s: f'{s["notimpl_0x1c8"]}')
    line('notimpl 0x218 (window)', lambda s: f'{s["notimpl_0x218"]}')
    line('notimpl 0x1c8 (run)', lambda s: f'{s["notimpl_0x1c8_total"]}')
    line('notimpl 0x218 (run)', lambda s: f'{s["notimpl_0x218_total"]}')
    line('host psos made/refused', lambda s: f'{s["psos_made"]}/{s["psos_refused"]}')
    line('D3DMetal no-op lines', lambda s: f'{s["noops_total"]}')
    line('  before world frame 1', lambda s: f'{s["noops_before_world"]}')
    line('  during the window', lambda s: f'{s["noops_in_world_window"]}')

    def noop(s):
        c = s['correlation']
        if not c:
            return 'no timed no-op lines'
        if not c.get('correlated'):
            return 'not tied to PSOs'
        return f'{c["best"]} numbering'
    line('  PSO(n) ties to pipeline', noop)
    line('  draws on no-op PSOs', lambda s: (f'{s.get("noop_draws")} {pct(s.get("noop_draws") or 0, s["recorded"])}'
                                              if s.get('noop_draws') is not None else '-'))
    for s in sums:
        print(f'\n[{s["run"]}] correlation detail: {json.dumps(s["correlation"])}')
        if s.get('noops_by_world_frame'):
            frames_with = s['noops_by_world_frame']
            print(f'  no-op lines per world frame (nonzero only): {frames_with}')
        for k, v in s['top_cumulative_world']:
            print(f'  top (cumulative, world): n={v[0]} op={k[0]} kind={k[1]} origin={k[2]} why={k[3]}  e.g. {v[1][:100]}')
        for t in s.get('noop_top', [])[:10]:
            print(f'  no-op pipeline: kind={t[0]} draws_in_window={t[2]} {t[1]}')


def main(argv):
    frames, as_json, runs = 600, False, []
    it = iter(argv)
    for a in it:
        if a == '--frames':
            frames = int(next(it))
        elif a == '--json':
            as_json = True
        elif a in ('-h', '--help'):
            print(__doc__)
            return 0
        else:
            runs.append(a)
    if not runs:
        print(__doc__)
        return 2
    sums = [summarize(load(r), frames) for r in runs]
    if as_json:
        print(json.dumps(sums, indent=1, default=str))
    else:
        show(sums, frames)
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
PYTHON_END
