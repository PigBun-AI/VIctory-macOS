#!/bin/bash
# Play the title by hand: a thin wrapper over tools/mac/run.sh (the game lock, the data directories and all checks stay run.sh's).
#   tools/mac/play.sh [run.sh options] [-- arguments for the title]
# What it sets, in front of your own options (a later option wins, so any of them can be overridden):
#   --fidelity     Series X, 2560x1440 (the profile of the formal tests; --perf for the speed profile)
#   --no-keys      no scripted Enter every 3 s (VICTORY_KEYS): you press the keys
#   --input none   no neutral replay (VICTORY_INPUT_REPLAY): from world frame 1 the replay is EXCLUSIVE and would swallow
#                  your keyboard and pad, so it must be off for hand play
#   --no-watchdog  the title's hang watchdog off (L5, non-fidelity, logged), so a slow frame in the world does not kill it
#   --timeout 14400 (4 h; PLAY_TIMEOUT=SEC changes it, 0 = none). With the watchdog off a real hang ends only by this limit.
# PLAY_KEEP_WATCHDOG=1 leaves the watchdog on. Keyboard map: docs/mac/RUN.md "Playing by hand".
# --exe / --prefix default to this checkout's target/; when this is a worktree without a build, to the main checkout's
# (VIctory-livetr next to it), so the release exe and the Metal prefix are not built twice.
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
case "$ROOT" in */worktrees/mac/*) _g="$ROOT/../../.." ;; *) _g="$ROOT/.." ;; esac   # worktrees/mac/<name> or next to the main checkouts
G="${GTA6_ROOT:-$(cd "$_g" && pwd)}"
EXE_REL=target/x86_64-pc-windows-gnullvm/release/victory.exe
extra=()
if [ ! -f "$ROOT/$EXE_REL" ] && [ -f "$G/VIctory-livetr/$EXE_REL" ]; then
    extra+=(--exe "$G/VIctory-livetr/$EXE_REL")
    [ -d "$ROOT/target/wineprefix-livetr-metal" ] || [ ! -d "$G/VIctory-livetr/target/wineprefix-livetr-metal" ] \
        || extra+=(--prefix "$G/VIctory-livetr/target/wineprefix-livetr-metal")
fi
wd=(--no-watchdog)
[ "${PLAY_KEEP_WATCHDOG:-0}" != 1 ] || wd=()
exec "$ROOT/tools/mac/run.sh" --fidelity --no-keys --input none ${wd[@]+"${wd[@]}"} \
    --timeout "${PLAY_TIMEOUT:-14400}" ${extra[@]+"${extra[@]}"} "$@"
