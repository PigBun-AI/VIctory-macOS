#!/bin/bash
# mac S6: victory-shader's hardware proofs on the Mac GPU, and their tally.
#
# The tests of crates/victory-shader (valu, wave64, permlane, waterfall,
# intersect, size, cube_lod, ...) run the translated DXIL on the device. On a
# Mac that is the executor `victory-shader-exec` of the Metal back (DXIL ->
# Metal Shader Converter -> Metal 4 compute -> read back), built from the
# `harness-back` branch's metal/shader-exec. Usage:
#   tools/mac/shader-hw.sh [--back DIR] [--filter SUBSTR] [--]  [cargo test args]
# --back: the metal/ directory of a victory-metal checkout (default
# $VICTORY_METAL_DIR, else ../metal-livetr/metal next to the worktrees).
# Without an executor the tests skip (that is what tools/mac/check.sh sees).
#
# Prints: passed / failed / skipped counts and each failure's class from
# tools/mac/shader-hw-known.txt (an unlisted failure is "UNCLASSIFIED").
# Exit 0 only when every failure is listed in the table as base / semantic
# (a "bug" row is a translator bug handed over, and keeps the exit at 1).
# Rules: cargo runs niced (19) with 1 job (the machine is shared); no game.
set -u
cd "$(dirname "$0")/../.."
. tools/mac/env.sh || exit 1
BACK="${VICTORY_METAL_DIR:-}"
FILTER=""
while [ $# -gt 0 ]; do
    case "$1" in
        --back) BACK=$2; shift 2 ;;
        --filter) FILTER=$2; shift 2 ;;
        --) shift; break ;;
        *) break ;;
    esac
done
[ -n "$BACK" ] || { echo "shader-hw: name the metal/ directory with --back or VICTORY_METAL_DIR" >&2; exit 2; }
# The game lock is held while a victory.exe runs: no compiling or testing then (the machine is shared).
if [ -z "${SHADER_HW_IGNORE_LOCK:-}" ] && tools/mac/game-lock.sh held; then
    echo "shader-hw: the game lock is held; not building or testing (SHADER_HW_IGNORE_LOCK=1 overrides)" >&2
    exit 3
fi
echo "== build the executor ($BACK)"
( cd "$BACK" && . ../tools/metal/env.sh && nice -n 19 cargo build -p victory-metal-shader-exec -j 1 ) || exit 2
EXEC="$BACK/target/debug/victory-shader-exec"
LOG="$(pwd)/${LOGS:-target/mac-logs}/shader-hw"
mkdir -p "$LOG"
rm -f "$LOG/exec.tsv"
echo "== run the tests"
VICTORY_SHADER_EXEC="$EXEC" VICTORY_SHADER_EXEC_LOG="$LOG/exec.tsv" \
    nice -n 19 cargo test -p victory-shader --lib -j 1 -- $FILTER "$@" 2>&1 | tee "$LOG/test.txt" | grep -E '^test result|FAILED'
python3 tools/mac/shader-hw-tally.py "$LOG/test.txt" "$LOG/exec.tsv" tools/mac/shader-hw-known.txt
rc=$?
# The wide-load pass the test asks for (its doc comment): once with VICTORY_SHADER_OPT=127.
if [ -z "$FILTER" ]; then
    echo "== run scalar_buffer_loads with VICTORY_SHADER_OPT=127"
    rm -f "$LOG/exec127.tsv"
    VICTORY_SHADER_OPT=127 VICTORY_SHADER_EXEC="$EXEC" VICTORY_SHADER_EXEC_LOG="$LOG/exec127.tsv" \
        nice -n 19 cargo test -p victory-shader --lib -j 1 -- scalar_buffer_loads 2>&1 | tee "$LOG/test127.txt" | grep -E '^test result|FAILED'
    python3 tools/mac/shader-hw-tally.py "$LOG/test127.txt" "$LOG/exec127.tsv" tools/mac/shader-hw-known.txt @opt127 || rc=1
fi
exit $rc
