#!/usr/bin/env bash
# tools/mac/collect-title-crash.sh DATA_DIR OUT_DIR [SINCE_FILE]   (mac L7, run.sh calls it once when a run ends)
#
# Copies the title's own crash log(s) from the run's data directory into the run's directory (OUT_DIR/title-crash/),
# so they survive the next `--new-game`, which replaces the data directory. The title writes
# `<data>/<id>/temp/victory-<date>-32.crash.log` when it dies (Game State, Frame, POOLS: BUILDING 22319/22500, the SRL state, ...);
# the place is looked up, not assumed: `grep -rl 'Game State' DATA_DIR`.
#
# SINCE_FILE (default: none): only logs not older than that file are taken (run.sh passes $out/start.txt, written before the
# title starts), so a log of an earlier run in the same data directory is not mistaken for this run's.
# Never fails the run: a missing directory or log is said on stdout and the exit status is 0.
set -u
data="${1:-}"; out="${2:-}"; since="${3:-}"
if [ -z "$data" ] || [ -z "$out" ]; then echo "usage: collect-title-crash.sh DATA_DIR OUT_DIR [SINCE_FILE]" >&2; exit 2; fi
if [ ! -d "$data" ]; then echo "title crash.log: no data directory $data"; exit 0; fi
if [ ! -d "$out" ]; then echo "title crash.log: no run directory $out"; exit 0; fi
n=0
dest="$out/title-crash"
# (-a: a log with stray bytes still counts as text)
while IFS= read -r f; do
    [ -n "$f" ] || continue
    if [ -n "$since" ] && [ -e "$since" ] && [ "$f" -ot "$since" ]; then continue; fi
    if mkdir -p "$dest" && cp -p "$f" "$dest/$(basename "$f")"; then
        n=$((n + 1))
        echo "title crash.log: $f -> $dest/$(basename "$f")"
    fi
done < <(grep -rla 'Game State' "$data" 2>/dev/null)
[ "$n" -gt 0 ] || echo "title crash.log: none newer than the run's start under $data (the title writes one only when it dies)"
exit 0
