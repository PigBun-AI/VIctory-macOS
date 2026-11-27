#!/bin/sh
# Runs the game (or any victory.exe command) on our Metal backend. A thin wrapper: it only turns
# arguments into those of the one launcher, tools/mac/run.sh, and `exec`s
#
#   tools/mac/run.sh --backend metal "$@"
#
# It sets NO game environment (DXC helper, keys, input replay, fault tracking, masked, resolution,
# WINEPREFIX, WINEDLLPATH/WINEDLLOVERRIDES, VICTORY_METAL_* ...): that is the launcher's, so the two
# backends cannot drift (docs/mac/RUN.md). Debug switches of the back (VICTORY_METAL_LENIENT, _TRACE,
# _SKIP_GS_INDIRECT, _DROPSTATS ...) are only passed through from the environment by the launcher.
#
#   tools/metal/run.sh [options] [-- victory.exe arguments]
#
# Old option          now
#   --timeout SEC     --timeout SEC (the launcher's default is 900, the old one was 600)
#   --out DIR         --out DIR. Without it: research/game-runs/<time>-metal (outside the repository;
#                     the launcher's own default is target/mac-runs/<time>)
#   --keys SPEC|none  --keys SPEC (`none` = --no-keys)
#   --input FILE|none --input FILE|none
#   --exec            --exec (now: `victory.exe <arguments after -->` in the run's environment, as the launcher defines it)
#   --no-build        nothing (the launcher builds only with --build); this wrapper adds --build unless
#                     --no-build is given, as the old script always built first
#   GTA6_ROOT         the only variable set: ~/Documents/GTA6Project if unset (location of checkouts, seed data, Wine, research/)
# Removed, no counterpart in the launcher:
#   --lenient         (survey mode) set VICTORY_METAL_LENIENT=1 in the environment if needed
#   victory.toml writing, the own summary, the own watchdog (the launcher writes summary.txt and stops
#   only the wineserver of the run's own prefix)
# Removed defaults: VICTORY_METAL_SKIP_GS_INDIRECT=1 (the default must not deviate from native behaviour;
#   the GS skip is a registered skip node in the matrix; pass it by hand: VICTORY_METAL_SKIP_GS_INDIRECT=1 tools/metal/run.sh ...)
# Every other option of the launcher (--fidelity, --perf, --prefix, --allow-concurrent, --print-env,
# --capture-frames, --frametime-log ...) is passed on as it is.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$here/../.." && pwd)
# Where the checkouts, the seed data, Wine and research/ live (a location, not game configuration; env.sh uses
# the same default). The launcher takes the parent of the checkout otherwise, which is worktrees/ here.
GTA6_ROOT="${GTA6_ROOT:-$HOME/Documents/GTA6Project}"; export GTA6_ROOT

# Pass 1: what was asked (options before `--` only).
build=1; have_out=0; dry=0
for a in "$@"; do
    case "$a" in
        --) break ;;
        --no-build) build=0 ;;
        --out) have_out=1 ;;
        --print-env|--dry-run) dry=1 ;;
    esac
done

# Pass 2: rotate the arguments through "$@", renaming the old ones.
n=$#; rest=0
while [ "$n" -gt 0 ]; do
    a=$1; shift; n=$((n - 1))
    if [ "$rest" = 1 ]; then set -- "$@" "$a"; continue; fi
    case "$a" in
        --no-build) ;;
        --keys)
            [ "$n" -gt 0 ] || { echo "tools/metal/run.sh: --keys needs SPEC|none" >&2; exit 2; }
            v=$1; shift; n=$((n - 1))
            if [ "$v" = none ]; then set -- "$@" --no-keys; else set -- "$@" --keys "$v"; fi ;;
        --lenient)
            echo "tools/metal/run.sh: --lenient is gone; set VICTORY_METAL_LENIENT=1 in the environment if you need it" >&2
            exit 2 ;;
        --) rest=1; set -- "$@" "$a" ;;
        *) set -- "$@" "$a" ;;
    esac
done

pre=""
[ "$build" = 1 ] && [ "$dry" = 0 ] && pre="--build"
if [ "$have_out" = 0 ] && [ "$dry" = 0 ]; then
    out="$GTA6_ROOT/research/game-runs/$(date +%Y%m%d-%H%M%S)-metal"
    # shellcheck disable=SC2086
    exec "$ROOT/tools/mac/run.sh" --backend metal $pre --out "$out" "$@"
fi
# shellcheck disable=SC2086
exec "$ROOT/tools/mac/run.sh" --backend metal $pre "$@"
