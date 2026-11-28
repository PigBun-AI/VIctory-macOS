#!/bin/sh
# Native build and tests, then the cross build of victory.exe (release,
# x86_64-pc-windows-gnullvm). Logs go to $LOGS (default: ./target/mac-logs).
# Usage: tools/mac/check.sh        (from anywhere in the checkout)
#
# Exit status: 0 only when every step passed. Otherwise 1, after all steps
# have run, with the failed steps named on the last lines ("check FAILED: ...").
# Each step's output goes to its own log first and is summarised afterwards, so
# no pipe sits between cargo and the status (a `cargo ... | tee | tail` pipeline
# returns the last command's status and used to hide a failed build). Plain sh:
# sync-windows.sh runs this with `sh`.
set -u
SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
# Every heavy job of this Mac goes through tools/mac/heavy.sh (docs/mac/RUN.md "Heavy tasks"): a machine-wide budget of job units
# (the number of cores, 1 while a game holds the game lock, tools/mac/game-lock.sh), nice 19, CARGO_BUILD_JOBS / RAYON_NUM_THREADS /
# MAKEFLAGS from the slot. Already inside a slot (VICTORY_HEAVY_SLOT set) this just goes on.
if [ -z "${VICTORY_HEAVY_SLOT:-}" ]; then
    exec bash "$(dirname "$SELF")/heavy.sh" --jobs 2 --label check -- sh "$SELF" "$@"
fi
cd "$(dirname "$0")/../.."
. tools/mac/env.sh || { echo "check FAILED: environment (tools/mac/env.sh)" >&2; exit 1; }
LOGS="${LOGS:-target/mac-logs}"
mkdir -p "$LOGS"
FAILED=""

# step NAME LOG SHOW -- COMMAND...   SHOW: tail (last 3 lines) or tests (result lines)
step() {
    name=$1; log=$2; show=$3; shift 4
    echo "== $name"
    "$@" >"$log" 2>&1; rc=$?
    case "$show" in
        tests) grep -E '^test result|FAILED|^error' "$log" ;;
        *) tail -n 3 "$log" ;;
    esac
    if [ "$rc" -ne 0 ]; then
        echo "!! step failed: $name (exit $rc), log $log"
        [ "$show" = tests ] || tail -n 20 "$log"
        FAILED="${FAILED:+$FAILED, }$name"
    fi
    return 0
}

step "native build" "$LOGS/native-build.log" tail -- cargo build --workspace --locked
step "native tests" "$LOGS/native-test.log" tests -- cargo test --workspace --locked --no-fail-fast
step "cross build"  "$LOGS/cross-build.log" tail -- cargo build --release -p victory --target x86_64-pc-windows-gnullvm --locked
EXE=target/x86_64-pc-windows-gnullvm/release/victory.exe
# a stale exe from an earlier build must not pass for this one
case "$FAILED" in
    *"cross build"*) ;;
    *) if ls -l "$EXE"; then :; else echo "!! step failed: victory.exe missing"; FAILED="${FAILED:+$FAILED, }victory.exe missing"; fi ;;
esac

if [ -n "$FAILED" ]; then
    echo "check FAILED: $FAILED"
    exit 1
fi
echo "check OK: native build, native tests, cross build"
