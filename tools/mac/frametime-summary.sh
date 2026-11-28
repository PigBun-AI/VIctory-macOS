#!/bin/sh
# Frame times of a run, with the frames that were run on a loaded machine marked invalid (python3, inline: the repository policy has no .py files).
#
#   tools/mac/frametime-summary.sh RUN_DIR [--load-limit 30] [--ratio 0.20] [--end EPOCH]
#
# Reads RUN_DIR/frametime.log (frame,frame_us,draws: one row per presented frame, frame_us the time since the previous row; the first row's
# is 0 and is left out) and RUN_DIR/load.log (run.sh: a line `<local time> load average: <1 min> <5 min> <15 min>` every 10 s). A frame has no
# clock time in the log, so it is placed by adding up the frame times backwards from the moment the log was last written (the file's modification
# time; --end EPOCH replaces it): a row's time = that moment minus the frame times of all later rows. A load.log line at T covers the 10 s from T
# to the next line. A frame in a stretch whose 1-minute load average is above the limit (default 30; VICTORY_LOAD_INVALID) is INVALID; the
# statistics (median, p90) use only the valid frames. When more than the ratio (default 0.20; VICTORY_INVALID_RATIO) of the frames are invalid
# the first line of the output is `数据无效：load 超过 30`.
# Output (stdout; run.sh keeps it as RUN_DIR/frametime-summary.txt and puts the first line first in summary.txt) and RUN_DIR/frametime-annot.csv
# (frame,frame_us,draws,time,load,valid). No load.log: nothing can be judged, every frame counts and the output says so.
exec python3 - "$@" <<'PYTHON_END'
import os, sys, re, time, math, statistics, bisect

def main(argv):
    if not argv or argv[0].startswith("-"):
        print("usage: frametime-summary.sh RUN_DIR [--load-limit 30] [--ratio 0.20] [--end EPOCH]", file=sys.stderr); return 2
    run = argv[0]
    limit = float(os.environ.get("VICTORY_LOAD_INVALID", "30")); ratio = float(os.environ.get("VICTORY_INVALID_RATIO", "0.20"))
    end = None
    i = 1
    while i < len(argv):
        if argv[i] == "--load-limit": limit = float(argv[i + 1]); i += 2
        elif argv[i] == "--ratio": ratio = float(argv[i + 1]); i += 2
        elif argv[i] == "--end": end = float(argv[i + 1]); i += 2
        else: print("frametime-summary.sh: unknown option " + argv[i], file=sys.stderr); return 2
    ftp = os.path.join(run, "frametime.log")
    if not os.path.isfile(ftp):
        print("frametime-summary.sh: no frametime.log in " + run, file=sys.stderr); return 1
    rows = []
    for line in open(ftp, errors="replace"):
        p = line.strip().split(",")
        if len(p) < 3 or not p[0].isdigit() or not p[1].isdigit(): continue
        rows.append((int(p[0]), int(p[1]), p[2]))
    if end is None: end = os.path.getmtime(ftp)
    # row times: backwards from the last write
    times = [0.0] * len(rows)
    t = end
    for k in range(len(rows) - 1, -1, -1):
        times[k] = t
        t -= rows[k][1] / 1e6
    # load.log: (epoch, load1)
    loads = []
    lp = os.path.join(run, "load.log")
    if os.path.isfile(lp):
        for line in open(lp, errors="replace"):
            m = re.match(r"^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)\s+load averages?:\s*([0-9]+(?:[.,][0-9]+)?)", line)
            if not m: continue
            try: ts = time.mktime(time.strptime(m.group(1), "%Y-%m-%dT%H:%M:%S"))
            except ValueError: continue
            loads.append((ts, float(m.group(2).replace(",", "."))))
    loads.sort()
    stamps = [ts for ts, _ in loads]
    def slot_of(x):   # the index of the last load.log line at or before x (0 for frames before the log began), None without a log
        if not loads: return None
        return max(0, bisect.bisect_right(stamps, x) - 1)
    body = rows[1:]                 # the first row's frame time is 0: left out
    btimes = times[1:]
    valid_us, all_us, ann = [], [], []
    bad_periods = set()
    for (fr, us, dr), tt in zip(body, btimes):
        idx = slot_of(tt)
        ld = None if idx is None else loads[idx][1]
        ok = ld is None or ld <= limit
        if not ok: bad_periods.add(idx)
        all_us.append(us)
        if ok: valid_us.append(us)
        ann.append("%d,%d,%s,%s,%s,%d" % (fr, us, dr, time.strftime("%Y-%m-%dT%H:%M:%S", time.localtime(tt)), "" if ld is None else ("%g" % ld), 1 if ok else 0))
    with open(os.path.join(run, "frametime-annot.csv"), "w") as f:
        f.write("frame,frame_us,draws,time,load,valid\n"); f.write("\n".join(ann) + ("\n" if ann else ""))
    n = len(body); nv = len(valid_us); ni = n - nv
    def pct(v, q):
        s = sorted(v); return s[max(0, math.ceil(q * len(s)) - 1)]
    def stats(v):
        if not v: return "no frames"
        return "median %.2f ms, p90 %.2f ms" % (statistics.median(v) / 1000.0, pct(v, 0.9) / 1000.0)
    out = []
    frac = (ni / n) if n else 0.0
    if n and frac > ratio:
        out.append("数据无效：load 超过 %g" % limit)
    if not loads:
        out.append("frametime: %d frames (the first row left out); no load.log, so the load was not checked: every frame counts" % n)
    else:
        out.append("frametime: %d frames (the first row left out): %d valid, %d invalid = %.1f%% (load average > %g in %d of %d ten-second stretches of load.log)" % (n, nv, ni, 100.0 * frac, limit, len(bad_periods), len(loads)))
    out.append("frametime valid frames: %s" % stats(valid_us))
    if ni: out.append("frametime all frames (for comparison): %s" % stats(all_us))
    print("\n".join(out))
    return 0

sys.exit(main(sys.argv[1:]))
PYTHON_END
