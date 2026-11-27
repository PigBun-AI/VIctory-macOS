#!/bin/sh
# Summary of VICTORY_METAL_DROPSTATS runs (docs/metal/dropstats.md): the program is the Python below
# (a .py file is not on the asset policy's allowlist). usage: dropstats-report.sh --out summary.md NAME=RUNDIR ...
exec python3 - "$@" <<'PYEOF'
#!/usr/bin/env python3
"""Summary of VICTORY_METAL_DROPSTATS runs (docs/metal/dropstats.md).

usage: dropstats-report.py --out summary.md [--min-frames N] NAME=RUNDIR [NAME=RUNDIR ...]

A RUNDIR is what tools/metal/run.sh --out wrote (stderr.log, notimpl.log,
shots/) with the statistics in RUNDIR/dropstats/ (front.csv, front-present.csv,
back.csv, front-pso.csv, back-pso.csv).

Two layers are told apart:

* OURS (measured): the front and the back count every draw and dispatch of the
  command lists the title executed, and what they did not run, with the reason
  and the pipeline. A row is a frame (a present).
* victory-gpu (ESTIMATE): the shared layer drops most draws before they reach
  the front. Its per-reason numbers come from its own 1 s log lines
  (`VICTORY_DIAGNOSTICS=1` build), per-frame averages over a second, aligned
  to ours by time. Its pipeline hashes are mapped to ours by the order of the
  refusals ("host pipeline refused" lines against back-pso.csv).

World frames are the frames from the `world frame 1` log line on.
"""
import argparse
import csv
import datetime as dt
import math
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path


def pct(values, q):
    """Nearest-rank percentile (q in 0..100); 0 for no values."""
    if not values:
        return 0
    v = sorted(values)
    return v[min(len(v) - 1, max(0, math.ceil(q / 100 * len(v)) - 1))]


def med(values):
    return pct(values, 50)


def mean(values):
    return sum(values) / len(values) if values else 0.0


def ts(line):
    """Epoch seconds of a tracing line's UTC ISO timestamp, or None."""
    m = re.match(r"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?)Z", line)
    if not m:
        return None
    return dt.datetime.fromisoformat(m.group(1)).replace(tzinfo=dt.timezone.utc).timestamp()


def read_csv(path):
    try:
        with open(path, newline="") as f:
            return list(csv.DictReader(f))
    except FileNotFoundError:
        return []


def pairs(text):
    """`a=1;b=2` (the last `=` splits) as (key, n) pairs."""
    out = []
    for p in (text or "").split(";"):
        if p:
            k, _, n = p.rpartition("=")
            if n.isdigit():
                out.append((k, int(n)))
    return out


def ints(row, names):
    return {n: int(row[n]) for n in names}


class Run:
    def __init__(self, name, path):
        self.name = name
        self.path = Path(path)
        d = self.path / "dropstats"
        self.back = read_csv(d / "back.csv")
        self.front = read_csv(d / "front.csv")
        self.presents = read_csv(d / "front-present.csv")
        self.back_pso = read_csv(d / "back-pso.csv")
        self.front_pso = read_csv(d / "front-pso.csv")
        self.log = (self.path / "stderr.log").read_text(errors="replace").splitlines()
        self.t0 = None  # epoch s of world frame 1
        self.world_present = None
        for line in self.log:
            m = re.search(r"world frame 1 .*present=(\d+)", line)
            if m:
                self.t0 = ts(line)
                self.world_present = int(m.group(1))
                break
        self.load()

    # -- our layer ---------------------------------------------------------
    def load(self):
        t0ms = (self.t0 or 0) * 1000
        self.world = [r for r in self.back if int(r["epoch_ms"]) >= t0ms] if self.t0 else []
        # front rows (one per ExecuteCommandLists) into the frames the presents end
        ends = [int(p["epoch_ms"]) for p in self.presents]
        self.front_frames = defaultdict(Counter)
        import bisect

        for r in self.front:
            i = bisect.bisect_left(ends, int(r["epoch_ms"]))
            c = self.front_frames[i + 1]
            for k in ("submitted_draw", "submitted_dispatch", "dropped_draw", "dropped_dispatch"):
                c[k] += int(r[k])
            for k, n in pairs(r["reasons"]):
                c["R:" + k] += n

    def frames(self):
        out = []
        for r in self.world:
            f = int(r["frame"])
            sd, sx = int(r["submitted_draw"]), int(r["submitted_dispatch"])
            dd, dx = int(r["dropped_draw"]), int(r["dropped_dispatch"])
            out.append((f, sd, sx, dd, dx, r))
        return out

    # -- victory-gpu layer ---------------------------------------------------
    def gpu_windows(self):
        """1 s windows of victory-gpu from the world on: dict with the printed
        numbers and the reason lines that followed."""
        wins = []
        prev = None
        for line in self.log:
            t = ts(line)
            m = re.search(
                r"([\d.]+) fps, ([\d.]+) ms/frame.* (\d+) draws/frame, dropped (\d+) without a host pipeline "
                r"\((\d+) being made\) and (\d+) indirect",
                line,
            )
            if m and t:
                fps = float(m.group(1))
                frames = fps * (t - prev) if prev else fps
                prev = t
                if self.t0 and t >= self.t0:
                    wins.append(
                        dict(t=t, fps=fps, frames=frames, ran=int(m.group(3)), nohost=int(m.group(4)),
                             made=int(m.group(5)), indirect=int(m.group(6)), reasons=[])
                    )
                continue
            m = re.search(r"dropped ([\d.]+)/frame without a host pipeline: (.*)$", line)
            if m and wins and t and t >= self.t0:
                wins[-1]["reasons"].append((m.group(2).strip(), float(m.group(1))))
        return wins

    def refusal_map(self):
        """gpu 'mesh H, pixel H' key -> (our hash, kind, reason), by the order of
        the refusals; also the number of refusals on each side."""
        lines = []
        for line in self.log:
            m = re.search(r"host (?:mesh )?pipeline refused .*shaders=\[([^\]]*)\]", line)
            if m:
                lines.append((ts(line), m.group(1)))
        refused = [r for r in self.back_pso if r["result"] != "ok"]
        out = {}
        worst = 0.0
        n = min(len(lines), len(refused))
        for (t, key), r in list(zip(lines, refused))[:n]:
            out[key] = (r["hash"], r["kind"], r["reason"])
            worst = max(worst, abs(t - int(r["epoch_us"]) / 1e6) if t else 0)
        return out, len(lines), len(refused), worst


def cause(reason):
    """A short class of a back refusal text."""
    if "attributeAtVertex" in reason:
        return "MSC refuses attributeAtVertex"
    if "Metal limit" in reason and "mesh" in reason:
        return "mesh over Metal's 32 KB limit"
    if "MSC refused" in reason:
        return "MSC refused (other construct)"
    return "back refused (other)"


def kind_type(kind):
    return {"graphics-ms": "MS", "graphics-vs": "VS", "graphics-gs": "GS", "compute": "CS"}.get(kind, "?")


def fmt(x, d=1):
    return f"{x:,.{d}f}" if isinstance(x, float) else f"{x:,}"


def table(head, rows):
    out = ["| " + " | ".join(head) + " |", "|" + "|".join("---" for _ in head) + "|"]
    for r in rows:
        out.append("| " + " | ".join(str(c) for c in r) + " |")
    return out


def section(run, min_frames, shots):
    L = []
    add = L.append
    fr = run.frames()
    add(f"## Run `{run.name}` ({run.path})")
    add("")
    if not fr:
        add("No world frames (no `world frame 1` line or no rows after it).")
        return L, {}
    ok = len(fr) >= min_frames
    add(f"* World frames: {len(fr)} (back frames {fr[0][0]}..{fr[-1][0]}; world frame 1 is present "
        f"{run.world_present} in victory-gpu's count){'' if ok else f'  **fewer than the required {min_frames}**'}.")
    sub = [f[1] + f[2] for f in fr]
    drops = [f[3] + f[4] for f in fr]
    ratio = [100 * (f[3] + f[4]) / (f[1] + f[2]) if f[1] + f[2] else 0 for f in fr]
    dratio = [100 * f[3] / f[1] if f[1] else 0 for f in fr]
    xratio = [100 * f[4] / f[2] if f[2] else 0 for f in fr]
    add("")
    add("### Our layer (measured): per world frame, what reached the back and what it did not run")
    add("")
    cols = [
        ("draws submitted", [f[1] for f in fr]),
        ("dispatches submitted", [f[2] for f in fr]),
        ("draws+dispatches", sub),
        ("draws dropped", [f[3] for f in fr]),
        ("dispatches dropped", [f[4] for f in fr]),
        ("dropped (all)", drops),
        ("draws dropped %", dratio),
        ("dispatches dropped %", xratio),
        ("dropped % (all)", ratio),
    ]
    rows = []
    for n, v in cols:
        isf = "%" in n
        rows.append((n, fmt(float(mean(v)), 1 if isf else 0 if False else 1), fmt(float(med(v)), 1), fmt(float(pct(v, 95)), 1),
                     fmt(float(max(v)), 1)))
    add("\n".join(table(["per frame", "mean", "median", "p95", "max"], rows)))
    tot_sub_d = sum(f[1] for f in fr)
    tot_sub_x = sum(f[2] for f in fr)
    tot_d = sum(f[3] for f in fr)
    tot_x = sum(f[4] for f in fr)
    add("")
    add(f"Totals over the world frames: {tot_sub_d:,} draws and {tot_sub_x:,} dispatches submitted; "
        f"{tot_d:,} draws ({100 * tot_d / max(1, tot_sub_d):.2f} %) and {tot_x:,} dispatches "
        f"({100 * tot_x / max(1, tot_sub_x):.2f} %) dropped; no-ops (zero-count, not drops): "
        f"{sum(int(f[5]['noop']) for f in fr):,}.")

    # the front's view of the same frames
    fids = {f[0] for f in fr}
    ffs = [run.front_frames[i] for i in sorted(fids)]
    if ffs:
        fd = [c["submitted_draw"] for c in ffs]
        fx = [c["submitted_dispatch"] for c in ffs]
        fdrop = sum(c["dropped_draw"] + c["dropped_dispatch"] for c in ffs)
        add("")
        add(f"Front (what the title executed and the front passed to the back; rows put into frames by the "
            f"present times): draws {mean(fd):.0f}/frame, dispatches {mean(fx):.0f}/frame (medians {med(fd)}, {med(fx)}); "
            f"dropped in the front: {fdrop} (lists whose `Close` failed, `ExecuteCommandLists` refusals).")
        fr_reasons = Counter()
        for c in ffs:
            for k, v in c.items():
                if k.startswith("R:"):
                    fr_reasons[k[2:]] += v
        for k, v in fr_reasons.most_common(10):
            add(f"  * front drop: {k}: {v}")

    # by reason
    reasons = Counter()
    for f in fr:
        for k, n in pairs(f[5]["reasons"]):
            reasons[re.sub(r"[0-9a-f]{16}", "H", k)] += n
    add("")
    add("### Our layer by reason (world frames)")
    add("")
    totdrop = sum(reasons.values())
    rows = [(k, fmt(n), fmt(n / len(fr), 1), f"{100 * n / max(1, totdrop):.1f} %", f"{100 * n / max(1, tot_sub_d + tot_sub_x):.2f} %")
            for k, n in reasons.most_common()]
    if not rows:
        rows = [("(nothing dropped)", 0, 0, "", "")]
    add("\n".join(table(["reason", "ops", "per frame", "share of our drops", "share of submitted"], rows)))

    # by type
    add("")
    add("### Our layer by pipeline type (world frames)")
    add("")
    rows = []
    for t, s, d in (("VS (vertex + pixel)", "sub_vs", "drop_vs"), ("MS (mesh, +amplification)", "sub_ms", "drop_ms"),
                    ("GS (geometry)", "sub_gs", "drop_gs"), ("CS (compute)", "sub_cs", "drop_cs"),
                    ("no pipeline known", "sub_none", "drop_none")):
        sv = sum(int(f[5][s]) for f in fr)
        dv = sum(int(f[5][d]) for f in fr)
        rows.append((t, fmt(sv / len(fr), 1), fmt(dv / len(fr), 1), f"{100 * dv / max(1, sv):.1f} %",
                     f"{100 * dv / max(1, totdrop):.1f} %"))
    add("\n".join(table(["type", "submitted/frame", "dropped/frame", "dropped % of the type", "share of our drops"], rows)))

    # per pipeline
    per = Counter()
    for f in fr:
        for k, n in pairs(f[5]["pipelines"]):
            h, _, rest = k.partition(":")
            ty, _, why = rest.partition(":")
            per[(h, ty, re.sub(r"[0-9a-f]{16}", "H", why))] += n

    # --- victory-gpu layer
    wins = run.gpu_windows()
    gpu = {}
    rmap, n_gpu, n_back, worst = run.refusal_map()
    add("")
    add("### victory-gpu layer (ESTIMATE from its 1 s log lines, aligned by time)")
    add("")
    if wins:
        frames = sum(w["frames"] for w in wins)
        ran = sum(w["ran"] * w["frames"] for w in wins)
        nohost = sum(w["nohost"] * w["frames"] for w in wins)
        made = sum(w["made"] * w["frames"] for w in wins)
        ind = sum(w["indirect"] * w["frames"] for w in wins)
        total = ran + nohost + ind
        gpu = dict(frames=frames, ran=ran, nohost=nohost, made=made, ind=ind, total=total,
                   fps=mean([w["fps"] for w in wins]))
        add(f"{len(wins)} one-second windows from world frame 1 on ({frames:.0f} frames by fps x time). "
            f"Per frame: {ran / frames:.0f} draws ran on the host (the `draws/frame` field; ours submitted "
            f"{tot_sub_d / len(fr):.0f}/frame, so this is what reaches the front), {nohost / frames:.0f} dropped without a host "
            f"pipeline, {ind / frames:.0f} indirect executions not run (the log also prints a count of pipelines being made, "
            f"{made / frames:.1f} on average; that counts pipelines, not draws). "
            f"The title draws that did not run are in the next paragraphs (draws and dispatches are counted together in these two fields).")
        add("")
        add("(`dropped ... without a host pipeline` counts draws, mesh dispatches and dispatches; "
            "the log's `indirect` is executions of ExecuteIndirect not run; their reasons are in the same per-pipeline lines.)")
        # reasons
        cats = Counter()
        keys = Counter()
        for w in wins:
            for why, n in w["reasons"]:
                n *= w["frames"]
                if why == "being made":
                    # The shared layer's own label. In this base nearly all of these are the
                    # ~1000 ray-tracing resolve / ray-activation compute programs it leaves out on
                    # purpose (`make.rs left_out`): never requested, dispatches. A pipeline really
                    # in compilation is told by the queue counter (see below).
                    c = "never requested: RT resolve/activate compute left out (the log calls it 'being made')"
                    key = ("(being made / left out)", "CS", c)
                elif why.startswith("refused ["):
                    k = why[len("refused ["):-1]
                    info = rmap.get(k)
                    if info:
                        c = "refused: " + cause(info[2])
                        key = (k, kind_type(info[1]), c, info[0])
                    else:
                        c = "refused: (no matching back refusal)"
                        key = (k, "MS" if k.startswith("mesh") else "VS", c)
                elif "ray-tracing" in why:
                    c = "never requested here: ray-tracing structure builder (off)"
                    key = (why, "CS", c)
                elif re.match(r"^(compute|vertex|pixel|mesh|amplification|geometry|domain|hull)\b", why):
                    c = "refused: shader translation failed"
                    key = (why, "CS" if why.startswith("compute") else "?", c)
                elif why.startswith("indirect"):
                    c = "refused: " + re.sub(r"[0-9]+", "N", why)
                    key = (why, "?", c)
                else:
                    c = "other: " + re.sub(r"[0-9a-f]{16}", "H", why)
                    key = (why, "?", c)
                cats[c] += n
                keys[key] += n
        seen = sum(cats.values())
        add("")
        add(f"The diagnostics list the first 40 reasons of a second: they cover {seen / frames:.0f} of the "
            f"{(nohost + ind) / frames:.0f} drops per frame ({100 * seen / max(1, nohost + ind):.0f} %); the rest is not itemised.")
        add("")
        rows = [(c, fmt(n / frames, 1), f"{100 * n / max(1, seen):.1f} %", f"{100 * n / max(1, total):.1f} %")
                for c, n in cats.most_common()]
        add("\n".join(table(["reason (victory-gpu layer)", "draws/frame (est.)", "share of itemised drops", "share of title draws"], rows)))
        add("")
        types = Counter()
        for key, n in keys.items():
            types[key[1]] += n
        rows = [({"MS": "MS (mesh pipelines)", "VS": "VS (vertex pipelines)", "CS": "CS (compute)", "?": "unknown / being made"}.get(t, t),
                 fmt(n / frames, 1), f"{100 * n / max(1, seen):.1f} %") for t, n in types.most_common()]
        add("\n".join(table(["type (victory-gpu layer)", "draws/frame (est.)", "share of itemised drops"], rows)))
        gpu["cats"] = cats
        gpu["made_draws"] = cats.get("never requested: RT resolve/activate compute left out (the log calls it 'being made')", 0)
        draws_dropped = sum(n for k, n in keys.items() if k[1] in ("MS", "VS"))
        disp_dropped = sum(n for k, n in keys.items() if k[1] == "CS")
        other_dropped = seen - draws_dropped - disp_dropped
        gpu.update(draws_dropped=draws_dropped, disp_dropped=disp_dropped, other_dropped=other_dropped,
                   title_draws=ran + draws_dropped)
        waiting = [w for w in wins if w["made"] > 0]
        gpu["waiting_windows"] = len(waiting)
        base = [sum(v for k, v in w["reasons"] if k == "being made") for w in wins if w["made"] == 0]
        busy = [sum(v for k, v in w["reasons"] if k == "being made") for w in waiting]
        gpu["made_idle"], gpu["made_busy"] = mean(base), mean(busy)
        add("")
        add(f"Draws (mesh and vertex pipelines) separated from dispatches (compute): itemised drops are "
            f"{draws_dropped / frames:.0f} draws/frame, {disp_dropped / frames:.0f} dispatches/frame, "
            f"{other_dropped / frames:.0f}/frame of unclassified kind. Title draws (ran + dropped draws, a lower bound "
            f"since the diagnostics list only 40 reasons a second) about {(ran + draws_dropped) / frames:.0f}/frame: "
            f"**{100 * draws_dropped / max(1, ran + draws_dropped):.1f} %** of the draws do not reach the front.")
        add("")
        add(f"Compile timing check: the log's count of pipelines queued and not yet made was above 0 in {len(waiting)} of {len(wins)} "
            f"windows. The 'being made' draws per frame were {mean(base):.0f} in windows with nothing queued"
            + (f" and {mean(busy):.0f} in windows with something queued." if waiting else "."))
        gpu["keys"] = keys
        # attributeAtVertex
        attr_keys = {k: n for k, n in keys.items() if "attributeAtVertex" in cause(
            rmap.get(k[0], ("", "", ""))[2] if k[0] in rmap else "") or
            (k[0] in rmap and "attributeAtVertex" in rmap[k[0]][2])}
        attr_draws = sum(attr_keys.values())
        n_attr_pipes = sum(1 for r in run.back_pso if r["result"] != "ok" and "attributeAtVertex" in r["reason"])
        shaders = set()
        for k in rmap:
            if "attributeAtVertex" in rmap[k][2]:
                shaders.add(k.split(",")[0])
        add("")
        add(f"**attributeAtVertex pipelines.** The back refused {n_attr_pipes} pipeline creations over the run "
            f"for `dx.op.attributeAtVertex` ({len(shaders)} distinct mesh/vertex shaders; the lead's 103 is the count of "
            f"distinct shaders/pipelines in an earlier census, ours counts every creation). In the world they account "
            f"for **{attr_draws / frames:.0f} draws/frame, {100 * attr_draws / max(1, ran + draws_dropped):.1f} % of the title's draws "
            f"and {100 * attr_draws / max(1, seen):.1f} % of the itemised drops** (estimate).")
        gpu["attr_draws"] = attr_draws
        gpu["attr_pct_total"] = 100 * attr_draws / max(1, total)
        gpu["attr_pct_drops"] = 100 * attr_draws / max(1, seen)
        gpu["attr_pipes"] = n_attr_pipes
        gpu["attr_shaders"] = len(shaders)
        add("")
        add(f"Mapping of victory-gpu's pipeline hashes to ours: {n_gpu} refusals logged by victory-gpu, {n_back} refused in the back "
            f"({'equal: paired by order' if n_gpu == n_back else 'not equal: paired by order up to the shorter'}; "
            f"largest time difference of a pair {worst:.2f} s).")
        # top 20 across the layers
        cand = []
        for key, n in keys.items():
            cand.append(("victory-gpu", key[0], key[3] if len(key) > 3 else "", key[1], key[2], n / frames))
        for (h, ty, why), n in per.items():
            cand.append(("ours", "", h, ty, why, n / len(fr)))
        cand.sort(key=lambda c: -c[5])
        add("")
        add("### Top 20 dropped pipelines (world frames; draws/dispatches per frame; victory-gpu rows are estimates)")
        add("")
        rows = [(i + 1, c[0], f"`{c[1]}`" if c[1] else "", f"`{c[2]}`" if c[2] else "", c[3], c[4], fmt(c[5], 2))
                for i, c in enumerate(cand[:20])]
        add("\n".join(table(["#", "layer", "victory-gpu shaders", "our hash", "type", "reason", "per frame"], rows)))
    else:
        add("No victory-gpu 1 s lines after world frame 1.")
        cand = sorted(((n / len(fr), h, ty, why) for (h, ty, why), n in per.items()), reverse=True)
        add("")
        add("### Top 20 dropped pipelines (ours)")
        add("")
        add("\n".join(table(["#", "hash", "type", "reason", "per frame"],
                            [(i + 1, f"`{h}`", ty, why, fmt(n, 2)) for i, (n, h, ty, why) in enumerate(cand[:20])])))

    # pipelines created
    add("")
    add("### Pipeline creation (whole run, not only the world)")
    add("")
    rows = []
    for kind in ("graphics-vs", "graphics-ms", "graphics-gs", "compute"):
        made = [r for r in run.back_pso if r["kind"] == kind]
        bad = [r for r in made if r["result"] != "ok"]
        rows.append((kind_type(kind), len(made), len(bad), ", ".join(f"{n}x {c}" for c, n in Counter(cause(r["reason"]) for r in bad).items())))
    add("\n".join(table(["type", "created", "refused by the back", "causes"], rows)))
    comp = [int(r["compile_us"]) for r in run.back_pso if r.get("compile_us") not in (None, "") and r["result"] == "ok" and int(r["compile_us"]) > 0]
    hit = None
    if comp:
        fast = sum(1 for c in comp if c < 5000)
        hit = 100 * fast / len(comp)
        add("")
        add(f"Compiler time per created pipeline (the `MTL4Compiler` call): {len(comp)} compiles, median {med(comp) / 1000:.2f} ms, "
            f"p95 {pct(comp, 95) / 1000:.1f} ms, total {sum(comp) / 1e6:.1f} s. Archive hit rate ESTIMATE "
            f"(a compile under 5 ms; a hit takes about 0.3 ms, a miss 24 ms or more): **{hit:.1f} %** ({fast} of {len(comp)}).")

    # 0x1c8 / 0x218
    add("")
    add("### 0x1c8 / 0x218 (E_NOTIMPL pipeline creation methods), by time")
    add("")
    t_first = None
    ev = []
    for line in run.log:
        m = re.search(r"slot=(0x1c8|0x218) calls=(\d+)", line)
        if m and ts(line):
            ev.append((ts(line), m.group(1), int(m.group(2))))
    if ev:
        start = ev[0][0]
        w = run.t0
        last = {}
        for t, s, n in ev:
            last[s] = (t, n)
        add("Logged at powers of two of the call count: " + "; ".join(
            f"{s}: last logged call {n} at +{t - start:.1f} s after the first" for s, (t, n) in sorted(last.items())) +
            f". World frame 1 is at +{(w - start):.0f} s; logs after world frame 1: {sum(1 for t, _, _ in ev if w and t >= w)}.")
    else:
        add("Not logged.")

    # shots
    add("")
    add("### Screenshots")
    add("")
    for s in shots:
        add(f"* `{s}`")
    if not shots:
        add("(none)")

    summary = dict(
        frames=len(fr), sub_mean=mean(sub), drop_mean=mean(drops),
        drop_med=med(drops), drop_p95=pct(drops, 95), ratio_mean=mean(ratio), ratio_med=med(ratio),
        ratio_p95=pct(ratio, 95), dratio_med=med(dratio), dratio_p95=pct(dratio, 95),
        sub_d=mean([f[1] for f in fr]), sub_x=mean([f[2] for f in fr]),
        drop_d=mean([f[3] for f in fr]), drop_x=mean([f[4] for f in fr]),
        gpu=gpu, hit=hit,
    )
    return L, summary


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--min-frames", type=int, default=100)
    ap.add_argument("--header", default="")
    ap.add_argument("runs", nargs="+")
    a = ap.parse_args()
    runs = []
    for spec in a.runs:
        name, _, path = spec.partition("=")
        runs.append(Run(name, path))
    L = ["# Draws and dispatches dropped in the world, Metal backend", ""]
    if a.header:
        L += [Path(a.header).read_text().rstrip(), ""]
    sums = {}
    bodies = []
    for r in runs:
        shots = sorted(str(p) for p in (r.path / "shots").glob("*.png")) if (r.path / "shots").is_dir() else []
        body, s = section(r, a.min_frames, shots)
        bodies += body + [""]
        sums[r.name] = s
    L += ["## Side by side", ""]
    names = list(sums)
    rows = []

    def row(label, f):
        rows.append([label] + [f(sums[n]) if sums[n] else "-" for n in names])

    row("world frames", lambda s: s["frames"])
    row("ours: draws+dispatches submitted / frame (mean)", lambda s: fmt(s["sub_mean"], 0))
    row("ours: dropped / frame (mean / median / p95)", lambda s: f"{s['drop_mean']:.0f} / {s['drop_med']} / {s['drop_p95']}")
    row("ours: dropped % of submitted (mean / median / p95)", lambda s: f"{s['ratio_mean']:.1f} / {s['ratio_med']:.1f} / {s['ratio_p95']:.1f}")
    row("ours: draws dropped % (median / p95)", lambda s: f"{s['dratio_med']:.1f} / {s['dratio_p95']:.1f}")
    row("victory-gpu (est.): title draws / frame (ran + dropped draws, lower bound)", lambda s: fmt(s["gpu"]["title_draws"] / s["gpu"]["frames"], 0) if s["gpu"] else "-")
    row("victory-gpu (est.): draws run on the host / frame", lambda s: fmt(s["gpu"]["ran"] / s["gpu"]["frames"], 0) if s["gpu"] else "-")
    row("victory-gpu (est.): dropped without a host pipeline / frame", lambda s: fmt(s["gpu"]["nohost"] / s["gpu"]["frames"], 0) if s["gpu"] else "-")
    row("victory-gpu (est.): draws dropped before the front / frame (mesh+vertex pipelines)",
        lambda s: fmt(s["gpu"]["draws_dropped"] / s["gpu"]["frames"], 0) if s["gpu"] else "-")
    row("victory-gpu (est.): draws dropped, % of title draws",
        lambda s: f"{100 * s['gpu']['draws_dropped'] / max(1, s['gpu']['title_draws']):.1f} %" if s["gpu"] else "-")
    row("victory-gpu (est.): dispatches dropped / frame (compute)",
        lambda s: fmt(s["gpu"]["disp_dropped"] / s["gpu"]["frames"], 0) if s["gpu"] else "-")
    row("  of which 'being made' in the log = never requested (RT left out) / frame",
        lambda s: fmt(s["gpu"].get("made_draws", 0) / s["gpu"]["frames"], 0) if s["gpu"] else "-")
    row("  windows with pipelines queued, i.e. compiling",
        lambda s: str(s["gpu"]["waiting_windows"]) if s["gpu"] else "-")
    row("victory-gpu (est.): indirect not run / frame", lambda s: fmt(s["gpu"]["ind"] / s["gpu"]["frames"], 0) if s["gpu"] else "-")
    row("attributeAtVertex pipelines: % of title draws (est.)", lambda s: f"{100 * s['gpu'].get('attr_draws', 0) / max(1, s['gpu']['title_draws']):.1f} %" if s["gpu"] else "-")
    row("archive hit rate (est.)", lambda s: f"{s['hit']:.1f} %" if s["hit"] is not None else "no archive")
    L += table(["", *names], rows)
    L += [""]
    L += bodies
    Path(a.out).write_text("\n".join(L) + "\n")
    print(f"wrote {a.out}")


if __name__ == "__main__":
    sys.exit(main())
PYEOF
