#!/bin/bash
# Play the title by hand: a thin wrapper over tools/mac/run.sh (the game lock, the data directories and all checks stay run.sh's).
#   tools/mac/play.sh --purpose "<map column>: <node id>,..." [run.sh options] [-- arguments for the title]
#   --purpose is required here too (run.sh refuses to start without it, exit status 7): rule of the integration lead, until the translation map
#   is filled in no run is made to look at the picture. It goes through to run.sh with the other options.
# What it sets, in front of your own options (a later option wins, so any of them can be overridden):
#   --perf         1280x720 output patch (L3), the speed profile; NON-fidelity. Default since 2026-11-28: the fidelity GPU frames are
#                  6-17 s each on our back. --fidelity (Series X, 2560x1440, the profile of the formal tests) is still an option
#                  and wins when given (a later option wins). The perf UI is 3x too big (the title lays the UI out in 4K pixels)
#   --no-keys      no scripted Enter every 3 s (VICTORY_KEYS): you press the keys
#   --input none   no neutral replay (VICTORY_INPUT_REPLAY): from world frame 1 the replay is EXCLUSIVE and would swallow
#                  your keyboard and pad, so it must be off for hand play
#   --no-watchdog  the title's hang watchdog off (L5, non-fidelity, logged), so a slow frame in the world does not kill it
#   --timeout 14400 (4 h; PLAY_TIMEOUT=SEC changes it, 0 = none). With the watchdog off a real hang ends only by this limit.
# The data directory is a NEW game (no world save): data/livetr-perf-metal, cloned from the seed data/livetr. The title writes its own
# autosave while you play; the next run then starts in that save. --new-game starts over (keeps the warm cache). --worldsave starts
# from the Windows world save in an own directory (data/livetr-perf-metal-worldsave): very slow, turns white, for watching progress only.
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
exec "$ROOT/tools/mac/run.sh" --perf --no-keys --input none ${wd[@]+"${wd[@]}"} \
    --timeout "${PLAY_TIMEOUT:-14400}" ${extra[@]+"${extra[@]}"} "$@"
