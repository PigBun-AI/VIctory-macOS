#!/bin/bash
# Batch acceptance, one command, unattended: after each merged block (a Metal patch, an S-series fix ...) run ONE game round
# through tools/mac/run.sh and write the numbers that go to the lead: did it live, how far did it get, frame times, where the
# draws went (drop-stats), and the difference to the last acceptance. Docs: docs/mac/RUN.md ("Batch acceptance").
#
#   tools/mac/accept-batch.sh --purpose "<column>: <node id>,..." [--exe PATH] [--stage DIR|NAME] [--label TEXT] [--world N] [--timeout SEC]
#                             [--accept-dir DIR] [--lock-timeout SEC] [--no-compare]
#   tools/mac/accept-batch.sh --summarize RUN_DIR --out DIR [--label TEXT] [--world N]   (no game: summarise a finished run)
#
# The round: run.sh --backend metal --perf --new-game --no-watchdog --frametime-log --until-world N --timeout 720, the DEFAULT
# keys and input (Enter every 3 s, the neutral replay: the prologue runs by itself), VICTORY_MAC_DROP_STATS=2 (G8: per-present
# lines, the shared layer's reasons) and VICTORY_METAL_DROPSTATS=<run>/dropstats (the back's counts and reasons). It ends as
# soon as world frame N (default 300) is logged; --timeout (default 720 s = 12 minutes) is only the cap. Nothing else is
# started, nothing is captured as a picture. The game lock is run.sh's (one game at a time; this queues if it is held).
#
#   --exe PATH         victory.exe to run (default: the release exe of worktrees/mac/int; an error if there is none)
#   --stage DIR|NAME   the Metal stage: a directory, or a name under research/metal-stage/ (default `current`)
#   --label TEXT       a note for summary.md (what was merged)
#   --world N          the world frame to run to and to count to (default 300)
#   --timeout SEC      the cap of the round (default 720)
#   --accept-dir DIR   where results go (default research/accept under the GTA6 root; VICTORY_ACCEPT_DIR)
#   --lock-timeout SEC how long to queue for the game lock (default 0 = for ever)
#   --no-compare       do not look for an earlier acceptance
#   --summarize RUN    only write the summary of an existing run folder (needs --out)
#   --out DIR          with --summarize: the result folder
#
# Result: <accept-dir>/<yyyymmdd-hhmmss>-<short commit of mac/livetr>/ with run/ (run.sh's logs, dropstats/), summary.md,
# summary.json (the numbers the next acceptance compares with). Exit status: 0 the round reached world frame N and the title
# was alive; 1 it did not (died, or the cap came first, or run.sh could not start it; summary.md says which and how far);
# 2 bad options or no exe/stage; 4 the game lock wait ran out.
# Earlier acceptances are found by their summary.json carrying "accept_batch": 1 (other tools may write into the same folder).
# Reads only: the game, the data seed (run.sh clones it), the stage and the exe. run.sh is used as it is (no options added).
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
case "$ROOT" in */worktrees/mac/*) _g="$ROOT/../../.." ;; *) _g="$ROOT/.." ;; esac
G="${GTA6_ROOT:-$(cd "$_g" && pwd)}"
die() { echo "tools/mac/accept-batch.sh: $*" >&2; exit 2; }
usage() { sed -n '2,/^set -u/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' >&2; exit 2; }

exe=""; stage="current"; label=""; world=300; tmo=720; accept="${VICTORY_ACCEPT_DIR:-$G/research/accept}"; lock_tmo=0
source "$ROOT/tools/mac/purpose.sh"
compare=1; summarize=""; out=""; purpose=""
while [ $# -gt 0 ]; do
    case "$1" in
        --exe) exe=${2:?--exe PATH}; shift ;; --stage) stage=${2:?--stage DIR|NAME}; shift ;;
        --label) label=${2:?--label TEXT}; shift ;; --world) world=${2:?--world N}; shift ;;
        --timeout) tmo=${2:?--timeout SEC}; shift ;; --accept-dir) accept=${2:?--accept-dir DIR}; shift ;;
        --lock-timeout) lock_tmo=${2:?--lock-timeout SEC}; shift ;; --no-compare) compare=0 ;;
        --summarize) summarize=${2:?--summarize RUN_DIR}; shift ;; --out) out=${2:?--out DIR}; shift ;;
        --purpose) [ $# -gt 1 ] || { purpose_check ''; exit "$PURPOSE_EXIT"; }; purpose=$2; shift ;;
        -h|--help) usage ;;
        *) echo "unknown option $1" >&2; usage ;;
    esac
    shift
done
case "$world" in ''|0|*[!0-9]*) die "--world takes a world frame number, 1 or more" ;; esac
case "$tmo" in ''|0|*[!0-9]*) die "--timeout takes whole seconds, 1 or more (a round needs a cap)" ;; esac
case "$lock_tmo" in ''|*[!0-9]*) die "--lock-timeout takes whole seconds" ;; esac

short="$(git -C "$ROOT" rev-parse --short=8 mac/livetr 2>/dev/null || git -C "$ROOT" rev-parse --short=8 HEAD 2>/dev/null || echo unknown)"
stamp="$(date +%Y%m%d-%H%M%S)"

if [ -n "$summarize" ]; then
    [ -d "$summarize" ] || die "--summarize: $summarize is not a folder"
    [ -n "$out" ] || die "--summarize needs --out DIR"
    RUN="$(cd "$summarize" && pwd)"; mkdir -p "$out"; out="$(cd "$out" && pwd)"; rc=-1
    exe_used=""; stage_real=""
else
    [ -z "$out" ] || die "--out is for --summarize (the result folder is made under --accept-dir)"
    purpose_check "$purpose" || exit "$PURPOSE_EXIT"
    # the exe: given, or the latest release build of the integration worktree
    [ -n "$exe" ] || exe="$G/worktrees/mac/int/target/x86_64-pc-windows-gnullvm/release/victory.exe"
    [ -f "$exe" ] || die "no exe $exe (--exe PATH; the default is the release build of worktrees/mac/int, tools/mac/check.sh builds it)"
    exe="$(cd "$(dirname "$exe")" && pwd)/$(basename "$exe")"
    # the stage: a name under research/metal-stage/ or a directory
    case "$stage" in */*) sdir="$stage" ;; *) sdir="$G/research/metal-stage/$stage" ;; esac
    [ -d "$sdir" ] || die "no Metal stage $sdir (--stage DIR|NAME)"
    [ -f "$sdir/front/d3d12.dll" ] || die "$sdir is not a Metal stage (no front/d3d12.dll)"
    stage_real="$(cd "$sdir" && pwd -P)"
    mkdir -p "$accept" || die "cannot make $accept"; accept="$(cd "$accept" && pwd)"
    out="$accept/$stamp-$short"; n=1
    while [ -e "$out" ]; do n=$((n + 1)); out="$accept/$stamp-$short-$n"; done
    mkdir -p "$out/run/dropstats" || die "cannot make $out"
    RUN="$out/run"; exe_used="$exe"
    echo "accept-batch: mac/livetr $short, stage $stage_real, exe $exe"
    echo "accept-batch: one round to world frame $world (cap ${tmo} s), results in $out"
    # the round: the options are run.sh's own; the two switches below are the drop statistics of the two layers
    export VICTORY_MAC_DROP_STATS="${VICTORY_MAC_DROP_STATS:-2}"
    export VICTORY_METAL_DROPSTATS="$RUN/dropstats"
    export VICTORY_METAL_OUT="$stage_real"
    "$ROOT/tools/mac/run.sh" --purpose "$purpose" --backend metal --perf --new-game --no-watchdog --frametime-log \
        --exe "$exe" --out "$RUN" --until-world "$world" --timeout "$tmo" --lock-timeout "$lock_tmo" \
        >"$out/run.sh.out" 2>&1
    rc=$?
    echo "accept-batch: run.sh exit status $rc"
    tail -n 3 "$out/run.sh.out" | sed 's/^/  | /'
fi

prev=""
if [ "$compare" = 1 ] && [ -d "$accept" ]; then
    prev="$(ls -1d "$accept"/*/ 2>/dev/null | sed 's:/$::' | while read -r d; do
        [ "$d" != "$out" ] && [ -f "$d/summary.json" ] && grep -q '"accept_batch": 1' "$d/summary.json" && echo "$d"; done | LC_ALL=C sort | tail -n 1)"
fi
[ -z "$summarize" ] || [ -z "$prev" ] || [ "$prev" != "$out" ] || prev=""

export ACC_RUN="$RUN" ACC_OUT="$out" ACC_RC="$rc" ACC_WORLD="$world" ACC_TMO="$tmo" ACC_LABEL="$label" ACC_PREV="$prev" \
       ACC_SHORT="$short" ACC_STAGE_ARG="$stage" ACC_STAGE_REAL="$stage_real" ACC_EXE="$exe_used" ACC_ROOT="$ROOT" \
       ACC_REPORT="$ROOT/tools/mac/drop-stats-report.sh" ACC_SELF_COMMIT="$(git -C "$ROOT" rev-parse --short=8 HEAD 2>/dev/null)"
python3 - <<'PYTHON_END'
import csv, json, os, re, subprocess, sys, time, calendar
from collections import Counter

E = os.environ
RUN, OUT = E['ACC_RUN'], E['ACC_OUT']
RC = int(E['ACC_RC']); W = int(E['ACC_WORLD'])
TS = re.compile(r'^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?)Z')


def read(path):
    try:
        return open(path, errors='replace').read()
    except OSError:
        return ''


def utc_epoch(line):
    m = TS.match(line)
    if not m:
        return None
    t = m.group(1)
    base = calendar.timegm(time.strptime(t[:19], '%Y-%m-%dT%H:%M:%S'))
    return base + (float('0' + t[19:]) if len(t) > 19 else 0.0)


def pctile(xs, p):
    if not xs:
        return None
    xs = sorted(xs)
    k = (len(xs) - 1) * p / 100.0
    lo = int(k); hi = min(lo + 1, len(xs) - 1)
    return xs[lo] + (xs[hi] - xs[lo]) * (k - lo)


def f(x, nd=1, unit=''):
    return '-' if x is None else f'{x:.{nd}f}{unit}'


def pct(n, of):
    return None if not of else 100.0 * n / of


# ---- the run's own files ------------------------------------------------------------------------------------------
start = read(f'{RUN}/start.txt'); end = read(f'{RUN}/end.txt'); runenv = read(f'{RUN}/run-env.txt')
stderr = read(f'{RUN}/stderr.log'); stdout = read(f'{RUN}/stdout.log')
until_met = read(f'{RUN}/until-met.txt').strip()
start_lines = start.splitlines()
t_start = None
if start_lines:
    try:
        t_start = time.mktime(time.strptime(start_lines[0].strip()[:19], '%Y-%m-%dT%H:%M:%S'))
    except ValueError:
        pass
m = re.search(r'^exit (\d+)', end, re.M); title_exit = int(m.group(1)) if m else None
timed_out = '== time limit' in stderr
m = re.search(r'^exe (\w+) (\S+)', start, re.M); exe_sha, exe_time = (m.group(1), m.group(2)) if m else ('?', '?')
m = re.search(r'^checkout (\S+) \((\d+) changed', start, re.M); checkout = f'{m.group(1)} ({m.group(2)} changed files)' if m else '?'
m = re.search(r'^# metal: stage (\S+)', runenv, re.M); stage_run = m.group(1) if m else ''
m = re.search(r'^load average at start: (.*)$', start, re.M); load_start = m.group(1) if m else '?'
envs = {}
for line in runenv.splitlines():
    mm = re.match(r'^(VICTORY_[A-Z0-9_]*|WINE[A-Z_]*)=(.*)$', line)
    if mm:
        envs[mm.group(1)] = mm.group(2)
key_env = {}
for k in ('VICTORY_WAVE_WIDE', 'VICTORY_METAL_MSC_DENYLIST', 'VICTORY_METAL_ARCHIVE', 'VICTORY_METAL_SKIP_GS_INDIRECT',
          'VICTORY_MAC_DROP_STATS', 'VICTORY_METAL_DROPSTATS', 'VICTORY_OUTPUT', 'VICTORY_KEYS', 'VICTORY_INPUT_REPLAY',
          'VICTORY_MAC_NO_WATCHDOG'):
    key_env[k] = envs.get(k, os.environ.get(k) if k in ('VICTORY_WAVE_WIDE', 'VICTORY_METAL_MSC_DENYLIST') and not envs else None)
for k in sorted(envs):
    if k.startswith('VICTORY_METAL_') and k not in key_env:
        key_env[k] = envs[k]

# world frames and times
worlds = {}   # n -> (epoch, present)
for line in (stderr + '\n' + stdout).splitlines():
    t = utc_epoch(line)
    if t is None:
        continue
    mm = re.search(r'\bworld frame (\d+)\b', line)
    if not mm:
        continue
    n = int(mm.group(1)); pp = re.search(r'present=(\d+)', line)
    if n not in worlds or t < worlds[n][0]:
        worlds[n] = (t, int(pp.group(1)) if pp else None)
reached = max(worlds) if worlds else 0
p1 = worlds.get(1, (None, None))[1]
t_w1 = worlds.get(1, (None, None))[0]
secs_to_world = (t_w1 - t_start) if (t_w1 and t_start) else None
t_last = worlds[reached][0] if reached else None
secs_w1_to_last = (t_last - t_w1) if (t_w1 and t_last and reached > 1) else None
secs_total = None
if until_met:
    try:
        secs_total = float(until_met.split('\t')[1])
    except (IndexError, ValueError):
        pass

# the verdict
if RC == 0 and until_met:
    state = f'alive; reached world frame {W}'; ok = True
elif RC in (-1,) and reached >= W:
    state = f'reached world frame {reached} (summarised from logs)'; ok = True
elif RC == 4:
    state = 'not started: the game lock wait ran out'; ok = False
elif RC in (1, 2, 3) and not os.path.exists(f'{RUN}/end.txt'):
    state = f'not started: run.sh stopped with status {RC} (see run.sh.out)'; ok = False
elif timed_out:
    state = (f'alive at the cap, reached world frame {reached} of {W}' if reached else 'alive at the cap, never reached the world (no world frame 1)')
    ok = False
else:
    state = f'DIED: the title ended by itself (status {title_exit}) at world frame {reached}' if reached else \
            f'DIED before the world: the title ended by itself (status {title_exit})'
    ok = False
gpu_err = ''
for line in read(f'{RUN}/summary.txt').splitlines():
    if line.startswith('GPU errors'):
        gpu_err = line
crash_lines = [l for l in (stderr + stdout).splitlines()
               if re.search(r'Unhandled exception|c0000005|panicked at|Fatal:|stack overflow', l)][:3]

# ---- frame times -----------------------------------------------------------------------------------------------------
rows = []
for r in csv.reader(open(f'{RUN}/frametime.log', errors='replace')) if os.path.exists(f'{RUN}/frametime.log') else []:
    try:
        rows.append((int(r[0]), int(r[1])))
    except (ValueError, IndexError):
        pass


def ft_stats(sel):
    ms = [us / 1000.0 for _, us in sel]
    return {'n': len(ms), 'median_ms': pctile(ms, 50), 'p90_ms': pctile(ms, 90), 'max_ms': max(ms) if ms else None,
            'long_gt5s': sum(1 for x in ms if x > 5000.0)}


win_rows = [(fr, us) for fr, us in rows if p1 is not None and p1 <= fr < p1 + W]
ft_win = ft_stats(win_rows); ft_all = ft_stats(rows)

# load average: 1-minute figure
loads = []
for line in read(f'{RUN}/load.log').splitlines():
    mm = re.search(r'load averages?: ([\d.]+)', line)
    if mm:
        loads.append(float(mm.group(1)))
load = {'start': load_start, 'samples': len(loads), 'max': max(loads) if loads else None,
        'mean': sum(loads) / len(loads) if loads else None, 'median': pctile(loads, 50)}

# ---- the shared layer's drop statistics (G8), classes by drop-stats-report.sh -------------------------------------------
shared = None
n_ds = len(re.findall(r'drop-stats present=', stderr))
if n_ds:
    try:
        res = subprocess.run([E['ACC_REPORT'], RUN, '--frames', str(W), '--json'], capture_output=True, text=True, timeout=300)
        shared = json.loads(res.stdout)[0]
    except Exception as e:  # report failed: say so, do not stop
        shared = {'error': str(e)}

# ---- the back's drop statistics (VICTORY_METAL_DROPSTATS) ------------------------------------------------------------------
back = None
bpath = f'{RUN}/dropstats/back.csv'
if os.path.exists(bpath):
    tot = Counter(); reasons = Counter(); frames_n = 0; kinds = Counter()
    for r in csv.DictReader(open(bpath, errors='replace')):
        try:
            fr = int(r['frame'])
        except (ValueError, KeyError):
            continue
        if p1 is None or not (p1 <= fr < p1 + W):
            continue
        frames_n += 1
        for k in ('submitted_draw', 'submitted_dispatch', 'dropped_draw', 'dropped_dispatch', 'noop'):
            tot[k] += int(r.get(k) or 0)
        for k in ('drop_vs', 'drop_ms', 'drop_gs', 'drop_cs', 'drop_none'):
            kinds[k[5:]] += int(r.get(k) or 0)
        for text, n in re.findall(r'(.*?)=(\d+)(?:;|$)', r.get('reasons') or ''):
            reasons[text.strip()] += int(n)
    back = {'frames': frames_n, **dict(tot), 'dropped_by_pipeline_type': dict(kinds),
            'reasons': reasons.most_common(10)}


def rate_shared(s):
    return pct(s['dropped'], s['recorded']) if s and 'recorded' in s else None


def rate_back(b):
    if not b:
        return None
    sub = b['submitted_draw'] + b['submitted_dispatch']; drp = b['dropped_draw'] + b['dropped_dispatch']
    return pct(drp, sub + drp)


metrics = {
    'ok': ok, 'state': state, 'world_reached': reached, 'world_target': W,
    'secs_to_world1': secs_to_world, 'secs_w1_to_last': secs_w1_to_last, 'secs_total': secs_total,
    'ft_median_ms': ft_win['median_ms'], 'ft_p90_ms': ft_win['p90_ms'], 'ft_long_gt5s_window': ft_win['long_gt5s'],
    'ft_long_gt5s_run': ft_all['long_gt5s'], 'ft_frames_window': ft_win['n'],
    'drop_rate_shared_pct': rate_shared(shared), 'drop_rate_back_pct': rate_back(back),
    'load_max': load['max'], 'load_mean': load['mean'],
}

# ---- the comparison with the last acceptance ---------------------------------------------------------------------------
prev_dir = E.get('ACC_PREV', ''); prev = None
if prev_dir:
    try:
        prev = json.load(open(f'{prev_dir}/summary.json'))
        prev['metrics']['state']
    except Exception:
        prev = None


def delta(key, unit='', nd=1, pp=False):
    a = metrics.get(key); b = prev['metrics'].get(key) if prev else None
    if a is None or b is None:
        return f'{f(b, nd)} -> {f(a, nd)}  (no difference: one side has no number)'
    d = a - b
    return f'{f(b, nd)}{unit} -> {f(a, nd)}{unit}  (**{d:+.{nd}f}{" pp" if pp else unit}**)'


# ---- summary.md ----------------------------------------------------------------------------------------------------------
L = []
w = L.append
w(f'# Acceptance {os.path.basename(OUT)}')
w('')
if E.get('ACC_LABEL'):
    w(f'**Label:** {E["ACC_LABEL"]}')
    w('')
w(f'**Result: {"OK" if ok else "NOT OK"}: {state}.**')
w('')
w('## Version')
w('')
w(f'- mac/livetr: `{E["ACC_SHORT"]}`; acceptance script checkout `{E["ACC_SELF_COMMIT"]}`; the run was made from {checkout}')
stage_real = E.get('ACC_STAGE_REAL') or stage_run
w(f'- stage: `{E.get("ACC_STAGE_ARG") or "(from the run)"}` -> `{stage_real or "?"}`')
w(f'- exe: `{E.get("ACC_EXE") or "(from the run)"}` sha256 `{exe_sha}` (first 16), built {exe_time}')
w('- key environment: ' + ', '.join(f'`{k}`={"(unset)" if v is None else v}' for k, v in key_env.items() if k in
  ('VICTORY_WAVE_WIDE', 'VICTORY_METAL_MSC_DENYLIST', 'VICTORY_METAL_ARCHIVE', 'VICTORY_METAL_SKIP_GS_INDIRECT')))
if RC >= 0:
    w('- the round: run.sh `--backend metal --perf --new-game --no-watchdog --frametime-log --until-world '
      f'{W} --timeout {E["ACC_TMO"]}` with the default keys and input; `VICTORY_MAC_DROP_STATS={key_env.get("VICTORY_MAC_DROP_STATS")}`, '
      '`VICTORY_METAL_DROPSTATS` on')
else:
    w(f'- summarised from an existing run folder (not started by this script); `VICTORY_MAC_DROP_STATS`={key_env.get("VICTORY_MAC_DROP_STATS") or "(unset)"}, '
      f'`VICTORY_OUTPUT`={key_env.get("VICTORY_OUTPUT") or "(unset)"}')
w(f'- run logs: `{RUN}`')
w('')
w('## The round')
w('')
w('| | |')
w('|---|---|')
w(f'| alive / outcome | {state} |')
w(f'| title exit status | {title_exit if title_exit is not None else "-"}; run.sh exit status {RC if RC >= 0 else "(summarised only)"}{"; the cap was reached" if timed_out else ""} |')
w(f'| highest world frame logged | {reached} (target {W}) |')
w(f'| time from start to world frame 1 | {f(secs_to_world, 0, " s")} (present {p1 if p1 else "-"}) |')
w(f'| world frame 1 -> {reached} | {f(secs_w1_to_last, 0, " s")} |')
w(f'| whole round (run.sh, until met) | {f(secs_total, 0, " s")} |')
w(f'| frame time, world frames 1..{W} ({ft_win["n"]} frames) | median **{f(ft_win["median_ms"], 1, " ms")}**, p90 **{f(ft_win["p90_ms"], 1, " ms")}**, max {f(ft_win["max_ms"], 0, " ms")} |')
w(f'| frames over 5 s | **{ft_win["long_gt5s"]}** in w1..w{W}; {ft_all["long_gt5s"]} in the whole run ({ft_all["n"]} frames) |')
w(f'| frame time, whole run | median {f(ft_all["median_ms"], 1, " ms")}, p90 {f(ft_all["p90_ms"], 1, " ms")} |')
w(f'| load average (1 min) | at start {load["start"]}; during the run mean {f(load["mean"], 1)}, max {f(load["max"], 1)} ({load["samples"]} samples). Frame times are clean below about 30 |')
w(f'| {gpu_err.split(":")[0] if gpu_err else "GPU errors"} | {gpu_err.split(":", 1)[1].strip() if ":" in gpu_err else "-"} |')
w('')
if crash_lines:
    w('First crash-looking lines:')
    w('')
    for l in crash_lines:
        w(f'- `{l[:200]}`')
    w('')
w(f'## Where the draws went (world frames 1..{W})')
w('')
if shared and 'recorded' in shared:
    s = shared
    w(f'**Shared layer (G8 drop-stats, `drop-stats-report.sh`, {s["world_frames"]} world frames logged):**')
    w('')
    w(f'- draws {s["draws"]}, dispatches {s["dispatches"]} (recorded {s["recorded"]}); ran {s["ran"]}; **dropped {s["dropped"]} = {f(pct(s["dropped"], s["recorded"]), 2, " %")}**')
    w(f'- the title\'s calls of the unimplemented pipeline slots 0x1c8 / 0x218: {s["notimpl_0x1c8"]} / {s["notimpl_0x218"]} (no draw is recorded for them)')
    w(f'- host pipelines made / refused: {s["psos_made"]} / {s["psos_refused"]}')
    w('')
    w('Reasons (classes of the report, exact for the window), top 10:')
    w('')
    w('| # | reason | draws+dispatches | share of recorded |')
    w('|---|---|---|---|')
    for i, (k, n) in enumerate(sorted(s['why'].items(), key=lambda kv: -kv[1])[:10], 1):
        w(f'| {i} | `{k}` | {n} | {f(pct(n, s["recorded"]), 2, " %")} |')
    if not s['why']:
        w('| - | (nothing dropped) | 0 | - |')
    w('')
    w('Dropped by pipeline kind: ' + (', '.join(f'{k} {n}' for k, n in sorted(s['kind'].items(), key=lambda kv: -kv[1])) or 'none'))
    w('')
    top = s.get('top_cumulative_world') or []
    if top:
        w('Detail of the reasons (the `drop-stats top` lines, cumulative from world frame 1 to the end of the run, top 10 by count; '
          '`e.g.` are shader hashes):')
        w('')
        w('| n | op | kind | origin | reason | e.g. |')
        w('|---|---|---|---|---|---|')
        for k, v in top[:10]:
            op, kind, origin, why = k if isinstance(k, (list, tuple)) else eval(k)
            w(f'| {v[0]} | {op} | {kind} | {origin} | `{why}` | {str(v[1])[:70]} |')
        w('')
elif shared:
    w(f'Shared layer: drop-stats report failed: {shared.get("error")}')
    w('')
else:
    w('Shared layer: no `drop-stats present=` lines in stderr.log (the round did not get far enough, or the exe is older than G8).')
    w('')
if back:
    b = back
    sub_d, drp_d = b['submitted_draw'], b['dropped_draw']; sub_c, drp_c = b['submitted_dispatch'], b['dropped_dispatch']
    w(f'**Metal back (`VICTORY_METAL_DROPSTATS`, what reached it; {b["frames"]} frames in the window):**')
    w('')
    w(f'- draws {sub_d + drp_d} (submitted {sub_d}, dropped {drp_d} = {f(pct(drp_d, sub_d + drp_d), 2, " %")}); '
      f'dispatches {sub_c + drp_c} (submitted {sub_c}, dropped {drp_c} = {f(pct(drp_c, sub_c + drp_c), 2, " %")}); '
      f'overall drop rate {f(rate_back(b), 2, " %")}; zero-count no-ops {b.get("noop", 0)}')
    w('- dropped by pipeline type: ' + (', '.join(f'{k} {n}' for k, n in b['dropped_by_pipeline_type'].items() if n) or 'none'))
    w('')
    w('Reasons, top 10:')
    w('')
    w('| # | reason | count |')
    w('|---|---|---|')
    for i, (k, n) in enumerate(b['reasons'], 1):
        w(f'| {i} | `{k[:160]}` | {n} |')
    if not b['reasons']:
        w('| - | (nothing dropped) | 0 |')
    w('')
    w('The back counts the frames whose number lies in the world-frame window by present number (front and back frames are '
      'counted by `PRESENT`); an off-by-few at the edges is possible.')
    w('')
else:
    w('Metal back: no `dropstats/back.csv` (the stage is older than the DROPSTATS merge, or the world was not reached).')
    w('')
w('The shared layer drops before D3D12 is called (no host pipeline, refused, compiling ...); the back drops what reached it. '
  'They are different draws: the rates are not added up.')
w('')
w('## Against the last acceptance')
w('')
if prev:
    pm = prev['metrics']
    w(f'Last: `{os.path.basename(prev_dir)}`' + (f' ({prev["label"]})' if prev.get('label') else '') +
      f' (mac/livetr `{prev.get("short")}`, stage `{prev.get("stage_real")}`, exe `{prev.get("exe_sha")}`); it was **{pm.get("state")}**.')
    w('')
    w('| | last -> this |')
    w('|---|---|')
    w(f'| highest world frame | {pm.get("world_reached")} -> {metrics["world_reached"]} |')
    w(f'| shared-layer drop rate (%) | {delta("drop_rate_shared_pct", " %", 2, True)} |')
    w(f'| Metal back drop rate (%) | {delta("drop_rate_back_pct", " %", 2, True)} |')
    w(f'| frame time median (ms) | {delta("ft_median_ms", " ms")} |')
    w(f'| frame time p90 (ms) | {delta("ft_p90_ms", " ms")} |')
    w(f'| frames over 5 s (w1..w{W}) | {delta("ft_long_gt5s_window", "", 0)} |')
    w(f'| time to world frame 1 (s) | {delta("secs_to_world1", " s", 0)} |')
    w(f'| load, mean | {delta("load_mean")} |')
    w('')
    if prev.get('exe_sha') == exe_sha and prev.get('stage_real') == stage_real:
        w('Same exe and stage as the last one: differences are noise or machine state (look at the load).')
    else:
        w('Exe or stage differ from the last one. A comparison is only a fair one if both rounds were alive to the same world frame '
          'and ran at a similar load.')
    w('')
else:
    w('No earlier acceptance with a `summary.json` under the acceptance folder: this is the first (baseline).' if not E.get('ACC_PREV') else 'The last acceptance could not be read.')
    w('')
w('## Files')
w('')
w('`summary.json` (the numbers), `run/` (run.sh: start.txt, run-env.txt, stdout.log, stderr.log, frametime.log, load.log, summary.txt, '
  'until-met.txt, `dropstats/`), `run.sh.out`.')
open(f'{OUT}/summary.md', 'w').write('\n'.join(L) + '\n')
json.dump({'accept_batch': 1, 'metrics': metrics, 'label': E.get('ACC_LABEL'), 'short': E['ACC_SHORT'], 'stage_real': stage_real, 'exe_sha': exe_sha,
           'state': state, 'ok': ok, 'key_env': key_env, 'frametime_window': ft_win, 'frametime_run': ft_all, 'load': load,
           'shared': {k: v for k, v in (shared or {}).items() if k in ('recorded', 'draws', 'dispatches', 'ran', 'dropped', 'why', 'kind')},
           'back': back}, open(f'{OUT}/summary.json', 'w'), indent=1, default=str)
print(f'accept-batch: {"OK" if ok else "NOT OK"}: {state}')
print(f'accept-batch: summary {OUT}/summary.md')
sys.exit(0 if ok else 1)
PYTHON_END
st=$?
[ "$rc" != 4 ] || exit 4
exit $st
