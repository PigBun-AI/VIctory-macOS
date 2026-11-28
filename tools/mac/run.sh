#!/bin/bash
# The one way to start victory.exe on this Mac: CrossOver Wine, with either
# Apple's D3DMetal or our Metal backend, scripted keys, logs. Not a build step
# (tools/mac/check.sh builds). Full reference: docs/mac/RUN.md.
#
#   tools/mac/run.sh --purpose "<map column>: <node id>,..." [options] [-- arguments for the title]
#   tools/mac/run.sh --print-env [options]     print the environment and the command, run nothing
#   tools/mac/run.sh --exec [options] -- ARGS  run `victory.exe ARGS` (for example `warm --jobs 4`) instead of the title, in
#                                              the very same environment and prefix; no keys, no time limit, no log folder
#
# --purpose is REQUIRED (not for --print-env and --help): which column of the translation map this run fills and for which nodes, for example
#   --purpose "freq: mac.isa.lock_cmpxchg_flags"      --purpose "callstats,freq: S-G-009,S-D-001"
# The column is one or more of freq, callstats, new-nodes, corpus, perf-hotspot, separated by commas; after the colon at least one node id.
# Without one, or one that does not fit, run.sh refuses to start with exit status 7 (no other exit status of run.sh is 7). The text is kept as it
# is: first line of <out>/RUN.txt, in the game lock (`tools/mac/game-lock.sh status` shows it). Rule of the integration lead: until the map is
# filled in, no run is made to look at the picture. play.sh and the other wrappers pass --purpose through.
#
# What a run is made of (each has an option; the options only ever choose, the
# rest below is the same for every combination):
#   --backend metal|d3dmetal     (VICTORY_MAC_BACKEND, default metal: our Metal backend is the one main path) the ONLY thing
#                                the backends differ in: the prefix, the dlls in it, WINEDLLPATH/WINEDLLOVERRIDES,
#                                VICTORY_METAL_LOG/_NOTIMPL_LOG. d3dmetal is a reference-only backend: opening and menu
#                                scenes it draws correctly (pixel reference) and the final performance comparison; it
#                                is installed and selectable, never the default (its world is ~750 no-op pipelines)
#   --profile fidelity|perf      (VICTORY_MAC_PROFILE, default perf; --fidelity / --perf)
#       fidelity  Series X, VICTORY_OUTPUT=0: no resolution patch. The title outputs 4K by itself (neither its settings.xml
#                 nor the reported display mode change that; docs/mac/PATCHES.md P5), as on a Series X, where the system scales
#                 it: the host shows it at most 2560x1440 (VICTORY_PRESENT_MAX and VICTORY_WINDOW_MAX default to it; --present-max
#                 none for the whole picture). settings.xml's resolutionFullscreen is still written (2560x1440), not read for the
#                 output. Internal size, dynamic resolution and TAA upscaling are the title's. Own data directory.
#       perf      the output patched to 1280x720 (VICTORY_OUTPUT=1280x720; patch L3), logs
#                 `non-fidelity: output patched to 1280x720`. Not a fidelity result.
#   --present-max WxH|none       VICTORY_PRESENT_MAX: smaller host window and swap chain, same title output (P1)
#   --pool-check skip|keep       VICTORY_POOL_CHECK: jump over the back buffer pool's check without patching sizes (L3)
#   --log-render-size            VICTORY_LOG_RENDER_SIZE=1: the title's internal size in the log (P2); costs a lock
#                                per draw, so leave it off for timing runs
#   --exe PATH --out DIR --timeout SEC (0 = none) --cache DIR --data DIR --prefix DIR
#   --seed DIR                   the read-only seed the data directories are cloned from (default data/livetr under the GTA6 root)
#   --fresh-data                 clone the run's data directory from the seed again, whatever is there (reproducible start)
#   --new-game                   replace only the run's data/ (profile and saves) with a clone of the seed's, keeping the warm cache/
#                                and dxc/ of the run's directory: the title starts a NEW game, with no world save, but no cold compile
#   --worldsave                  start from a WORLD SAVE: seed = data/worldsave-seed (Windows save, read-only) and an own data
#                                directory data/livetr-<profile>-<backend>-worldsave. Never implied: the default seed data/livetr
#                                holds no world save (run.sh refuses a seed that does). The save world is very slow on our back
#                                and turns white (known, being fixed): for watching progress only
#   --keys SPEC | --no-keys      VICTORY_KEYS, default 8-3600/3:Enter (Enter every 3 s from 8 s)
#   --input FILE|none            VICTORY_INPUT_REPLAY, default tools/mac/input/world-neutral.txt (I1)
#   --record FILE                VICTORY_INPUT_RECORD (relative: under --out); turns the default replay off
#   --capture-frames LIST        VICTORY_CAPTURE_FRAMES (diagnostics builds): comma list of present numbers and/or w<N>, the N-th
#                                world frame after world frame 1 (I2), for example 64,w12. Bad items are refused here (the build skips them)
#   --record-at f<N>|w<N>|SEC    VICTORY_RECORD_AT (diagnostics builds): when the frame recorder starts, as f<N> (present number),
#                                w<N> (N-th world frame after world frame 1, I2) or seconds. Needs VICTORY_RECORD=<folder> (passed
#                                through from the environment) to record anything
#   --no-watchdog                VICTORY_MAC_NO_WATCHDOG=1 (L5): the title's own hang watchdog off. NON-FIDELITY (logged). A run that
#                                really hangs no longer crashes, so it ends only by --timeout: keep a limit (--timeout 0 warns)
#   --corpus                     preset for capture-corpus runs: --record-at <--corpus-frame> plus --no-watchdog (an explicit
#                                --record-at wins over the preset's). NON-FIDELITY. D3DMetal usually crashes within ~10 frames of
#                                entering the world, so the default frame is w5
#   --corpus-frame w<N>|N        the frame --corpus records at (default w5; a bare N means w<N>)
#   --frametime-log              VICTORY_FRAMETIME_LOG=<out>/frametime.log
#   --tracking 0|1               VICTORY_FAULT_TRACKING (default 1, as the code's default)
#   --masked SPEC                VICTORY_PATCH_MASKED (default: not set, the code follows fault tracking)
#   --console series-x|series-s  the title's console model (default series-x: no --console is passed)
#   --build                      (metal) run tools/metal/build-unixlib.sh first
#   --sample SEC                 every SEC seconds append memory, swap and GPU lines to <out>/mem.csv
#   --exec                       run `victory.exe <the arguments after -->` in the foreground (stdout/stderr stay on the terminal,
#                                the exit status is victory.exe's) with the environment of a run: same prefix, DLLs, data and cache
#                                folders, DXC front end. For `victory.exe warm` (tools/mac/drop-ab.sh). Needs at least one argument.
#   --allow-concurrent           start although another victory.exe is running (two can run the GPU out of memory). Only with
#                                --i-have-approval (or VICTORY_MAC_CONCURRENT_APPROVED=1): the Mac integration lead approves it
#   --i-have-approval            the approval --allow-concurrent needs
#   --lock-timeout SEC           how long to queue for the game lock (default 0 = for ever); exit status 4 when it runs out
#
# STOP AS SOON AS YOU HAVE THE DATA (all optional, they stack, the first one that holds ends the run; --timeout is only the cap):
#   --until-capture              the frames of --capture-frames are written (PNG done) and, with --record-at/--corpus and
#                                VICTORY_RECORD, the recorder has captured its frame
#   --until-log ERE              a line of stdout.log or stderr.log matches the extended regular expression (repeatable)
#   --until-world N              the log has `world frame N` (= --until-log 'world frame N\b'; sets VICTORY_LOG_WORLD=1 so the
#                                title logs every world frame)
#   --until-present N            the title's present number is N or more (turns --frametime-log on; read from frametime.log)
#   --linger SEC                 keep running SEC more seconds after the condition holds (default 0)
#   It ends by the same way --timeout does (the title stopped, `wineserver -k` of this prefix, the lock let go) with exit status 0
#   and `until 条件满足：<which> 在 <seconds> s` in start.txt, run-env.txt, stderr.log. Not met by --timeout (or the title ending
#   first): `until 未满足`, exit status 6. Without any --until-* nothing changes. docs/mac/RUN.md has the examples.
#
# ONE GAME AT A TIME on this Mac: before it starts victory.exe (the title, or --exec; not --print-env) run.sh takes the
# machine-wide game lock (tools/mac/game-lock.sh: a directory next to all checkouts, shared by every worktree), queues if
# another run holds it (printing the holder every 30 s), takes over a lock whose holder is gone, and lets it go when the
# run ends, times out or is interrupted. LOCK FIRST, THEN ANY SIDE EFFECT: everything that changes a data directory, a cache, a
# prefix or a log folder (the clone from the seed, --fresh-data, --new-game, --worldsave, settings.xml, the dlls put into the prefix's
# system32, `wineserver -k`, mkdir of the data, cache and out folders) is done only once the lock is held; a run that queues has
# touched nothing yet. --print-env takes no lock and changes nothing. With --allow-concurrent (no lock) nothing protects the data
# directories: give every concurrent run its own (docs/mac/RUN.md). `tools/mac/game-lock.sh status` shows holder and queue. Heavy CPU work (builds,
# tests) goes through tools/mac/heavy.sh, which drops its budget to 1 job while the lock is held (docs/mac/RUN.md "Heavy tasks"). Every run logs the load
# average every 10 s to <out>/load.log (and at the start in start.txt), to tell a clean frame-time run from a noisy one.
# Once it holds the lock, run.sh starts tools/mac/heavy-guard.sh as its own child (`run --parent <pid>`; its pid in <lock>/guard.pid) and ends it,
# with SIGCONT of what it stopped, before it lets the lock go; if run.sh is killed with SIGKILL the guard notices within seconds and ends by
# itself. Not with --allow-concurrent, not when a guard already runs, not with VICTORY_RUN_GUARD=0 (tests).
#
# Other VICTORY_* / WINE* variables in the environment pass through unchanged.
# The Metal stage (the directory with front/, x86_64-windows/ and x86_64-unix/) is found in this order:
#   1. $VICTORY_METAL_OUT (environment)
#   2. $ROOT/metal/target/wine-stage (this checkout's own build, if it has a stage)
#   3. VICTORY_METAL_OUT from tools/mac/run.local.env (untracked, in .gitignore; machine-specific values)
# tools/mac/run.local.env is sourced at the start if it exists; a variable already set in the environment wins over the
# file. With no stage found, the Metal backend stops with an error (set VICTORY_METAL_OUT, write run.local.env, or use
# --backend d3dmetal). VICTORY_METAL_ARCHIVE (read-only archive of compiled Metal libraries) passes through the same way.
# The stage's capability file CAPS (optional, in the stage directory): one capability per line, a line that starts with # is a comment. With a line that is
# exactly `mesh_compute` run.sh exports VICTORY_METAL_MESH_COMPUTE_SUPPORTED=1 to the title (the back of that stage can run the mesh->compute form: S5 appends a
# VMCS part to a mesh container over 32 KB, which changes the container's hash and so no longer matches an old back's AGX crash list). With no CAPS file, or none
# of that line, nothing is exported: a new exe with an old stage never appends the part. A value already set in the environment is kept as it is. --print-env says which.
# Wine: VICTORY_MAC_WINE_DIR, else VIctory-macOS/target/wine-cx under the GTA6 root (the parent of the main checkouts; three levels up from worktrees/mac/<name>).
# Builds (once) the DXC front end tools/macos/dxc-d3dmetal.sh and sets VICTORY_DXC_HELPER / DXC_LIB /
# VICTORY_DXC_DIR for the helper shaders (docs/mac/PATCHES.md, G4). Only the Wine processes of the
# chosen prefix are ever stopped.
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
case "$ROOT" in */worktrees/mac/*|*/worktrees/metal/*) _g="$ROOT/../../.." ;; *) _g="$ROOT/.." ;; esac   # worktrees/{mac,metal}/<name> or next to the main checkouts
G="${GTA6_ROOT:-$(cd "$_g" && pwd)}"
WDIR="${VICTORY_MAC_WINE_DIR:-$G/VIctory-macOS/target/wine-cx}"
WINE="$WDIR/bin/wine"; SERVER="$WDIR/bin/wineserver"

die() { echo "tools/mac/run.sh: $*" >&2; exit 2; }

# machine-specific settings: tools/mac/run.local.env (untracked). The environment wins over the file; the file's stage
# is only the last place to look (see the head of this file), so it is kept apart from the environment's.
env_stage="${VICTORY_METAL_OUT:-}"; local_stage=""
local_env="$ROOT/tools/mac/run.local.env"
if [ -f "$local_env" ]; then
    saved=()
    for n in $(sed -n 's/^[[:space:]]*\(export[[:space:]]\{1,\}\)\{0,1\}\([A-Za-z_][A-Za-z0-9_]*\)=.*/\2/p' "$local_env" | sort -u); do
        if [ -n "${!n+x}" ]; then saved+=("$n=${!n}"); fi
    done
    unset VICTORY_METAL_OUT
    # shellcheck disable=SC1090
    set -a; . "$local_env"; set +a
    local_stage="${VICTORY_METAL_OUT:-}"
    unset VICTORY_METAL_OUT
    for kv in ${saved[@]+"${saved[@]}"}; do export "${kv%%=*}=${kv#*=}"; done
    [ -z "$env_stage" ] || export VICTORY_METAL_OUT="$env_stage"
fi
usage() { sed -n '2,/^set -u/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' >&2; exit 2; }
# a Windows path for a Wine process: Z: is the Unix root
winpath() { printf 'Z:%s' "$(printf '%s' "$1" | tr / '\\')"; }

# ---- options ---------------------------------------------------------------
backend="${VICTORY_MAC_BACKEND:-metal}"; profile="${VICTORY_MAC_PROFILE:-perf}"
exe="$ROOT/target/x86_64-pc-windows-gnullvm/release/victory.exe"
out=""; tmo=900; cache=""; data=""; prefix=""; tracking=""; masked=""; build=0; sample=0; concurrent=0; fresh=0; seed=""; newgame=0; worldsave=0
keys="8-3600/3:Enter"; input="$ROOT/tools/mac/input/world-neutral.txt"; record=""; capture=""
record_at=""; no_watchdog=0; corpus=0; corpus_frame="w5"
until_logs=(); until_world=""; until_present=""; until_capture=0; linger=0; linger_set=0
purpose=""; lock_tmo=0; approved=0; present_max=""; pool_check=""; render_size=0; frametime=0; console=""; print_env=0; execmode=0; settings_output="2560x1440"
while [ $# -gt 0 ]; do
    case "$1" in
        --backend) backend=${2:?--backend metal|d3dmetal}; shift ;;
        --profile) profile=${2:?--profile fidelity|perf}; shift ;;
        --fidelity) profile=fidelity ;; --perf) profile=perf ;;
        --exe) exe=${2:?}; shift ;; --out) out=${2:?}; shift ;; --timeout) tmo=${2:?}; shift ;;
        --cache) cache=${2:?}; shift ;; --data) data=${2:?}; shift ;; --prefix) prefix=${2:?}; shift ;;
        --seed) seed=${2:?}; shift ;; --fresh-data) fresh=1 ;;
        --new-game) newgame=1 ;; --worldsave) worldsave=1 ;;
        --keys) keys=${2:?}; shift ;; --no-keys) keys=none ;;
        --input) input=${2:?}; shift ;; --record) record=${2:?}; shift ;;
        --capture-frames) capture=${2:?}; shift ;;
        --record-at) record_at=${2:?--record-at f<N>|w<N>|SECONDS}; shift ;;
        --no-watchdog) no_watchdog=1 ;;
        --corpus) corpus=1 ;; --corpus-frame) corpus_frame=${2:?--corpus-frame w<N>}; shift ;;
        --tracking) tracking=${2:?}; shift ;; --masked) masked=${2:?}; shift ;;
        --present-max) present_max=${2:?}; shift ;; --pool-check) pool_check=${2:?}; shift ;;
        --settings-output) settings_output=${2:?}; shift ;;
        --log-render-size) render_size=1 ;; --frametime-log) frametime=1 ;;
        --console) console=${2:?}; shift ;; --build) build=1 ;; --sample) sample=${2:?}; shift ;;
        --allow-concurrent) concurrent=1 ;; --i-have-approval) approved=1 ;;
        --lock-timeout) lock_tmo=${2:?--lock-timeout SECONDS}; shift ;;
        --until-capture) until_capture=1 ;;
        --until-log) until_logs+=("${2:?--until-log ERE}"); shift ;;
        --until-world) until_world=${2:?--until-world N}; shift ;;
        --until-present) until_present=${2:?--until-present N}; shift ;;
        --linger) linger=${2:?--linger SECONDS}; linger_set=1; shift ;;
        --purpose) purpose=${2?--purpose "<map column>: <node id>,..."}; shift ;;
        --print-env|--dry-run) print_env=1 ;;
        --exec) execmode=1 ;;
        -h|--help) usage ;;
        --) shift; break ;;
        *) echo "unknown option $1" >&2; usage ;;
    esac
    shift
done
title_args=("$@")
# --purpose: required for every run that would start the game (not --print-env); exit status 7
. "$ROOT/tools/mac/purpose.sh"
if [ "$print_env" = 0 ]; then purpose_check "$purpose" || { echo "tools/mac/run.sh: refused" >&2; exit "$PURPOSE_EXIT"; }; fi
case "$backend" in d3dmetal|metal) ;; *) die "--backend is d3dmetal or metal, not '$backend'" ;; esac
case "$profile" in fidelity|perf) ;; *) die "--profile is fidelity or perf, not '$profile'" ;; esac
case "$settings_output" in 1280x720|1664x936|1920x1080|2560x1440|3840x2160) ;;
    *) die "--settings-output is one of the title's modes: 1280x720 1664x936 1920x1080 2560x1440 3840x2160" ;; esac
case "$console" in ''|series-x|series-s) ;; *) die "--console is series-x or series-s" ;; esac
case "$pool_check" in ''|skip|keep) ;; *) die "--pool-check is skip or keep" ;; esac
if [ -n "$capture" ]; then
    oldifs=$IFS; IFS=,
    for item in $capture; do
        [[ "$item" =~ ^[wW]?[0-9]+$ ]] || { IFS=$oldifs; die "--capture-frames: '$item' is neither a present number nor w<N> (the build would skip it)"; }
    done
    IFS=$oldifs
fi
case "$corpus_frame" in [0-9]*) corpus_frame="w$corpus_frame" ;; esac
[[ "$corpus_frame" =~ ^[wW][0-9]+$ ]] || die "--corpus-frame is w<N> or a whole number N, not '$corpus_frame'"
if [ "$corpus" = 1 ]; then
    no_watchdog=1
    [ -n "$record_at" ] || record_at="$corpus_frame"
fi
[ -z "$record_at" ] || [[ "$record_at" =~ ^([wWf][0-9]+|[0-9]+(\.[0-9]+)?)$ ]] \
    || die "--record-at is f<N>, w<N> or seconds, not '$record_at' (the build would take anything else as 'wait for the trigger file')"
case "$tmo" in ''|*[!0-9]*) die "--timeout takes whole seconds" ;; esac
case "$sample" in ''|*[!0-9]*) die "--sample takes whole seconds" ;; esac
case "$lock_tmo" in ''|*[!0-9]*) die "--lock-timeout takes whole seconds" ;; esac
# ---- --until-*: stop as soon as the data is there ------------------------------------------------------------------
case "$linger" in ''|*[!0-9]*) die "--linger takes whole seconds" ;; esac
[ -z "$until_world" ] || case "$until_world" in ''|0|*[!0-9]*) die "--until-world takes a world frame number, 1 or more" ;; esac
[ -z "$until_present" ] || case "$until_present" in ''|0|*[!0-9]*) die "--until-present takes a present number, 1 or more" ;; esac
for _re in ${until_logs[@]+"${until_logs[@]}"}; do
    [ -n "$_re" ] || die "--until-log: an empty pattern matches every line"
    printf '' | grep -E -e "$_re" >/dev/null 2>&1; [ $? -le 1 ] || die "--until-log: '$_re' is not an extended regular expression (grep -E)"
done
until_record=0; [ -z "$record_at" ] || [ -z "${VICTORY_RECORD:-}" ] || until_record=1
until_on=0
{ [ "$until_capture" = 1 ] || [ ${#until_logs[@]} -gt 0 ] || [ -n "$until_world" ] || [ -n "$until_present" ]; } && until_on=1
if [ "$until_capture" = 1 ] && [ -z "$capture" ] && [ "$until_record" = 0 ]; then
    die "--until-capture needs something to wait for: --capture-frames LIST, or --record-at/--corpus with VICTORY_RECORD=<folder> in the environment"
fi
if [ "$until_on" = 0 ] && [ "$linger_set" = 1 ]; then
    echo "tools/mac/run.sh: warning: --linger has no effect without --until-*" >&2
fi
[ "$until_on" = 0 ] || [ "$execmode" = 0 ] || die "--until-* is for the title run, not --exec"
until_desc=""
if [ "$until_on" = 1 ]; then
    [ "$until_capture" = 0 ] || until_desc="$until_desc --until-capture"
    for _re in ${until_logs[@]+"${until_logs[@]}"}; do until_desc="$until_desc --until-log '$_re'"; done
    [ -z "$until_world" ] || until_desc="$until_desc --until-world $until_world"
    [ -z "$until_present" ] || until_desc="$until_desc --until-present $until_present"
    until_desc="${until_desc# }"
    # --until-present reads the present number from frametime.log
    [ -z "$until_present" ] || frametime=1
    # --until-world, and --until-capture with a w<N> frame, need the title to log every world frame
    case ",$capture," in *,[wW][0-9]*) _wcap=1 ;; *) _wcap=0 ;; esac
    if [ -n "$until_world" ] || { [ "$until_capture" = 1 ] && [ "$_wcap" = 1 ]; }; then export VICTORY_LOG_WORLD=1; fi
    if [ "$tmo" = 0 ]; then
        echo "tools/mac/run.sh: warning: --until-* without --timeout: nothing ends the run if the condition never comes" >&2
    fi
fi
if [ "$concurrent" = 1 ] && [ "$approved" = 0 ] && [ "${VICTORY_MAC_CONCURRENT_APPROVED:-0}" != 1 ]; then
    die "--allow-concurrent needs the approval of the Mac integration lead: add --i-have-approval (or VICTORY_MAC_CONCURRENT_APPROVED=1) once it is given. Only one game at a time otherwise (the queue is automatic: drop --allow-concurrent)"
fi

# ---- paths -----------------------------------------------------------------
time_name="$(date +%Y%m%d-%H%M%S)"
[ "$print_env" = 1 ] && time_name="<time>"
[ -n "$out" ] || out="$ROOT/target/mac-runs/$time_name"
# One data directory per profile AND backend, each cloned (APFS clone) from the same read-only seed on first use, so two
# runs of different backends start from the same state and can run side by side. The seed is never written.
if [ "$worldsave" = 1 ]; then
    [ -n "$seed" ] || seed="$G/data/worldsave-seed"
    base="$G/data/livetr-$profile-$backend-worldsave"
else
    [ -n "$seed" ] || seed="$G/data/livetr"
    base="$G/data/livetr-$profile-$backend"
fi
[ "$fresh" = 0 ] || [ "$newgame" = 0 ] || die "--fresh-data and --new-game together: use one"
# the default seed must stay a NEW-GAME seed: a world save (a SARC container) in it would put every derived directory in the save world
if [ "$worldsave" = 0 ] && ls -d "$seed"/data/*/saves/*/*/SARC* >/dev/null 2>&1; then
    die "the seed $seed holds a world save (SARC container): the default seed is new-game only; world saves live in $G/data/worldsave-seed (use --worldsave)"
fi
derived=1; { [ -n "${VICTORY_DATA_DIR:-}" ] || [ -n "$data" ] || [ -n "$cache" ]; } && derived=0
[ "$newgame" = 0 ] || [ "$derived" = 1 ] || die "--new-game works on the run's own directory $base only (no --data, --cache or VICTORY_DATA_DIR)"
[ "$fresh" = 0 ] || [ "$derived" = 1 ] || die "--fresh-data works on the run's own directory $base only (no --data, --cache or VICTORY_DATA_DIR)"
[ "$base" != "$seed" ] || die "the run's data directory is the seed"
if [ -n "${VICTORY_DATA_DIR:-}" ] && [ -z "$data" ]; then data="$VICTORY_DATA_DIR"; fi
if [ -z "$data" ] && [ -n "$cache" ]; then data="$(dirname "$cache")/data"; fi
[ -n "$data" ] || data="$base/data"
[ -n "$cache" ] || cache="$(dirname "$data")/cache"
game_dir="${GAME_DIR:-${VICTORY_GAME_DIR:-$G/GTA6.Xbox.v1.0.2.1}}"
# The two backends never share a prefix: ours has our dlls in its system32.
d3dm_prefix="$ROOT/target/wineprefix-livetr-d3dmetal"; legacy_prefix="$ROOT/target/wineprefix-livetr"
metal_prefix="$ROOT/target/wineprefix-livetr-metal"
if [ -z "$prefix" ]; then
    if [ -n "${WINEPREFIX:-}" ]; then prefix="$WINEPREFIX"
    elif [ "$backend" = metal ]; then prefix="$metal_prefix"; else prefix="$d3dm_prefix"; fi
fi
# the Metal stage: environment, then this checkout's own build, then run.local.env (resolved only for the Metal backend)
stage=""; stage_from=""
if [ "$backend" = metal ]; then
    if [ -n "$env_stage" ]; then stage="$env_stage"; stage_from="VICTORY_METAL_OUT (environment)"
    elif [ -f "$ROOT/metal/target/wine-stage/front/d3d12.dll" ]; then stage="$ROOT/metal/target/wine-stage"; stage_from="$ROOT/metal/target/wine-stage"
    elif [ -n "$local_stage" ]; then stage="$local_stage"; stage_from="VICTORY_METAL_OUT in tools/mac/run.local.env"
    else die "no Metal stage: VICTORY_METAL_OUT is not set, $ROOT/metal/target/wine-stage does not exist, and tools/mac/run.local.env has none. Set VICTORY_METAL_OUT, or put it in tools/mac/run.local.env, or use --backend d3dmetal (reference-only)"
    fi
    [ -d "$stage" ] || die "the Metal stage $stage ($stage_from) is not a directory; fix VICTORY_METAL_OUT or tools/mac/run.local.env, or use --backend d3dmetal (reference-only)"
fi
SYS32="$prefix/drive_c/windows/system32"
# the stage's capability file CAPS (docs/mac/RUN.md): a line that is exactly mesh_compute (no comment, no other text) switches the mesh->compute form on
caps_note=""
if [ "$backend" = metal ]; then
    if [ -n "${VICTORY_METAL_MESH_COMPUTE_SUPPORTED:-}" ]; then
        caps_note="VICTORY_METAL_MESH_COMPUTE_SUPPORTED=${VICTORY_METAL_MESH_COMPUTE_SUPPORTED} (from the environment, kept as it is; the stage's CAPS is not read)"
    elif [ -f "$stage/CAPS" ] && tr -d '\r' <"$stage/CAPS" | grep -qxF 'mesh_compute'; then
        export VICTORY_METAL_MESH_COMPUTE_SUPPORTED=1
        caps_note="VICTORY_METAL_MESH_COMPUTE_SUPPORTED=1 (from $stage/CAPS: mesh_compute)"
    elif [ -f "$stage/CAPS" ]; then
        caps_note="VICTORY_METAL_MESH_COMPUTE_SUPPORTED not set ($stage/CAPS has no mesh_compute line)"
    else
        caps_note="VICTORY_METAL_MESH_COMPUTE_SUPPORTED not set (no CAPS file in the stage: an old stage; the mesh container keeps its hash)"
    fi
fi

# ---- checks that need no side effect ----------------------------------------
if [ "$backend" = metal ]; then
    # D3DMetal's baseline prefixes keep Apple's dlls; ours go into the system32 of a prefix of its own.
    case "$prefix" in
        *wineprefix-play*|*wineprefix-livetr-d3dmetal|*wineprefix-livetr|*wineprefix-livetr/) die "refusing to use the D3DMetal prefix $prefix for the Metal backend (default: $metal_prefix)" ;;
    esac
    [ "$prefix" != "$d3dm_prefix" ] && [ "$prefix" != "$legacy_prefix" ] || die "refusing to use the D3DMetal prefix $prefix for the Metal backend"
fi
if [ "$profile" = fidelity ]; then
    [ "$console" != series-s ] || die "the fidelity profile is Series X (no --console series-s)"
    [ -z "${VICTORY_RENDER_SCALE:-}" ] || die "the fidelity profile leaves the render scale to the title: unset VICTORY_RENDER_SCALE"
    [ -z "${VICTORY_UPSCALER:-}" ] || die "the fidelity profile leaves the upscaler to the title: unset VICTORY_UPSCALER"
    case "${VICTORY_OUTPUT:-0}" in 0|none) ;; *) die "the fidelity profile does not patch the output: unset VICTORY_OUTPUT (it is $VICTORY_OUTPUT)" ;; esac
fi

# ---- the environment of the title ------------------------------------------
UNSET=()
export WINEPREFIX="$prefix" WINEDEBUG="${WINEDEBUG:--all}"
export ROSETTA_ADVERTISE_AVX=1 VICTORY_SKIP_CPU_CHECK=1
export VICTORY_GAME_DIR="$game_dir" VICTORY_CACHE_DIR="$cache" VICTORY_DATA_DIR="$data"
base_overrides="mscoree,mshtml=;winedbg.exe=d"
if [ "$backend" = metal ]; then
    export WINEDLLOVERRIDES="$base_overrides;d3d12core=b;d3d12,dxgi=n,b"
    export WINEDLLPATH="$stage${WINEDLLPATH:+:$WINEDLLPATH}"
    export VICTORY_METAL_OUT="$stage"
    export VICTORY_METAL_LOG=1
    export VICTORY_METAL_NOTIMPL_LOG="$(winpath "$out/notimpl.log")"
else
    export WINEDLLOVERRIDES="$base_overrides;d3d12,d3d12core,dxgi=b"
fi
export VICTORY_FAULT_TRACKING="${tracking:-${VICTORY_FAULT_TRACKING:-1}}"
[ -z "$masked" ] || export VICTORY_PATCH_MASKED="$masked"
# keys and input
if [ "$keys" = none ] || [ -z "$keys" ]; then UNSET+=(VICTORY_KEYS); unset VICTORY_KEYS; else export VICTORY_KEYS="$keys"; fi
if [ -n "$record" ]; then
    case "$record" in /*) ;; *) record="$out/$record" ;; esac
    export VICTORY_INPUT_RECORD="$record"
    [ "$input" = "$ROOT/tools/mac/input/world-neutral.txt" ] && input=none
fi
if [ "$input" = none ]; then UNSET+=(VICTORY_INPUT_REPLAY); unset VICTORY_INPUT_REPLAY; else export VICTORY_INPUT_REPLAY="$input"; fi
[ -z "$capture" ] || export VICTORY_CAPTURE_FRAMES="$capture"
[ -z "$record_at" ] || export VICTORY_RECORD_AT="$record_at"
[ "$no_watchdog" = 0 ] || export VICTORY_MAC_NO_WATCHDOG=1
# the watchdog is off by this run's option or by the caller's environment (anything but empty or 0, as the loader reads it)
watchdog_off=0
case "${VICTORY_MAC_NO_WATCHDOG:-}" in ''|0) ;; *) watchdog_off=1 ;; esac
if [ "$watchdog_off" = 1 ] && [ "$tmo" = 0 ] && [ "$execmode" = 0 ]; then
    echo "tools/mac/run.sh: warning: the title's hang watchdog is off and there is no --timeout: a run that really hangs never ends" >&2
fi
if [ -n "$record_at" ] && [ -z "${VICTORY_RECORD:-}" ]; then
    echo "tools/mac/run.sh: warning: VICTORY_RECORD_AT is set but VICTORY_RECORD (the folder) is not: nothing will be recorded" >&2
fi
[ "$frametime" = 0 ] || export VICTORY_FRAMETIME_LOG="$out/frametime.log"
# the title's output: the profile decides, and says so in the log
if [ "$profile" = fidelity ]; then
    export VICTORY_OUTPUT=0
else
    export VICTORY_OUTPUT="${VICTORY_OUTPUT:-1280x720}"
fi
# fidelity: the title outputs 4K by itself, whatever its settings or the reported display mode say (ui-scale experiments
# 2026-11-28, docs/mac/PATCHES.md P5); on a Series X the system scales that to the television. Here the host does: the picture is
# shown at most 2560x1440 (P1 cap on the swap chain; VICTORY_WINDOW_MAX on the title's window). `--present-max none` shows it whole.
fidelity_show="2560x1440"
if [ "$profile" = fidelity ] && [ -z "$present_max" ] && [ -z "${VICTORY_PRESENT_MAX:-}" ]; then present_max="$fidelity_show"; fi
if [ "$profile" = fidelity ] && [ "$present_max" != none ] && [ -z "${VICTORY_WINDOW_MAX:-}" ]; then export VICTORY_WINDOW_MAX="${present_max:-$fidelity_show}"; fi
case "$present_max" in none) UNSET+=(VICTORY_PRESENT_MAX); unset VICTORY_PRESENT_MAX ;; '') ;; *) export VICTORY_PRESENT_MAX="$present_max" ;; esac
case "$pool_check" in skip) export VICTORY_POOL_CHECK=skip ;; keep) UNSET+=(VICTORY_POOL_CHECK); unset VICTORY_POOL_CHECK ;; esac
[ "$render_size" = 0 ] || export VICTORY_LOG_RENDER_SIZE=1
# DXC front end for the helper shaders (x86_64, run under Rosetta), the same for every run
helper="$ROOT/target/dxc-d3dmetal"
export VICTORY_DXC_HELPER="$helper" VICTORY_DXC_DIR="$(dirname "$cache")/dxc"
export DXC_LIB="$WDIR/lib/external/D3DMetal.framework/Versions/A/Resources/libdxcompiler.dylib"

if [ "$execmode" = 0 ] && [ "$until_capture" = 0 ] && { [ -n "$capture" ] || [ "$corpus" = 1 ]; }; then
    echo "tools/mac/run.sh: hint: --until-capture ends the run as soon as the frames are written (--timeout stays the cap)" >&2
fi
if [ "$execmode" = 1 ]; then
    [ ${#title_args[@]} -gt 0 ] || die "--exec needs the victory.exe arguments after --, for example: --exec -- warm"
    cmd=("$WINE" "$exe" "${title_args[@]}")
else
    cmd=("$WINE" "$exe" run --execute "$game_dir/game_scarlett_master.exe")
    [ -z "$console" ] || [ "$console" = series-x ] || cmd+=(--console "$console")
    [ ${#title_args[@]} -eq 0 ] || cmd+=(-- "${title_args[@]}")
fi

describe() {
    echo "# backend: $backend    profile: $profile"
    [ "$execmode" = 0 ] || echo "# --exec: victory.exe is run with these arguments instead of the title (no keys, no time limit, no log folder)"
    echo "# prefix: $prefix"
    if [ "$profile" = fidelity ]; then
        echo "# fidelity: Series X; no resolution patch (VICTORY_OUTPUT=0): the title outputs 4K (3840x2160) by itself, as on a Series X, where the system scales it"
        echo "# fidelity: shown at most ${VICTORY_PRESENT_MAX:-unlimited} (VICTORY_PRESENT_MAX, P1) in a window of at most ${VICTORY_WINDOW_MAX:-the work area}; the title's output is not 1440p"
        echo "# fidelity: settings.xml video/resolutionFullscreen is set to $settings_output, but the title does not take its output size from it (P5); kept for the title's own menus"
    else
        echo "# perf: non-fidelity: output patched to ${VICTORY_OUTPUT} (the title's code, patch L3)"
    fi
    if [ "$watchdog_off" = 1 ]; then
        echo "# non-fidelity: title hang watchdog disabled (VICTORY_MAC_NO_WATCHDOG=${VICTORY_MAC_NO_WATCHDOG}; patch L5): a real hang does not crash, the run ends by --timeout (${tmo} s; 0 = never)"
    fi
    [ "$corpus" = 0 ] || echo "# corpus: --record-at ${record_at} (--corpus-frame ${corpus_frame}) plus --no-watchdog"
    [ "$until_on" = 0 ] || echo "# until: stop at the first of: ${until_desc} (linger ${linger} s); --timeout ${tmo} s is only the cap (not met: exit status 6)"
    if [ "$backend" = metal ]; then
        echo "# metal: stage $stage (from $stage_from); front/d3d12.dll front/dxgi.dll x86_64-windows/victory_metal_bridge.dll go into the prefix's system32 (originals kept as .metal-orig)"
        echo "# metal: caps: $caps_note"
    else
        echo "# d3dmetal: reference-only backend (openings/menus pixel reference, perf comparison)"
        echo "# d3dmetal: any .metal-orig dlls in the prefix's system32 are put back first"
    fi
    for v in "${UNSET[@]+"${UNSET[@]}"}"; do echo "# unset: $v"; done
    echo "# command:"
    printf '#   %s\n' "$(printf '%q ' "${cmd[@]}" | sed 's/ $//')"
    echo "# environment (VICTORY_*, WINE*, ROSETTA_*, DXC_*, D3DM_*, MTL_*), sorted:"
    env | grep -E '^(VICTORY_|WINE|ROSETTA_|DXC_|D3DM_|MTL_)' | LC_ALL=C sort
}
# the user's home as ~ (also in the Z: form of a Wine path), so two outputs can be compared anywhere
print_environment() {
    describe | WIN_HOME="Z:$(printf '%s' "$HOME" | tr / '\\')" awk '
        function swap(line, from, to,    i) { while (from != "" && (i = index(line, from)) > 0) line = substr(line, 1, i - 1) to substr(line, i + length(from)); return line }
        { print swap(swap($0, ENVIRON["WIN_HOME"], "Z:~"), ENVIRON["HOME"], "~") }'
}

if [ "$backend" = d3dmetal ]; then
    echo "d3dmetal: reference-only backend (openings/menus pixel reference, perf comparison)" >&2
fi
if [ "$print_env" = 1 ]; then print_environment; exit 0; fi

# ---- the game lock, cleanup ---------------------------------------------------
# (victory.exe is only ever started below this line; --print-env has left already)
. "$ROOT/tools/mac/game-lock.sh"
GL_WORKTREE="$ROOT"; GL_CMD="${*:-}"; GL_PURPOSE="$purpose"
pid=""; wd=""; sm=""; ld=""; uw=""; guard=""; _cleaned=0
# The heavy-guard (tools/mac/heavy-guard.sh) is this run's child, started once the lock is held and ended before the lock is let go (no launchd
# agent: macOS keeps a launchd job's bash out of ~/Documents). Its pid is in the lock directory (guard.pid, guard.pidstart). Not started with
# --allow-concurrent (no lock), when a guard already runs (one in the lock directory, or one started by hand), or with VICTORY_RUN_GUARD=0.
# If this run.sh is killed with SIGKILL the guard sees its parent gone within a second, continues what it stopped and ends.
guard_start() {
    local g="$ROOT/tools/mac/heavy-guard.sh" gp
    [ "${VICTORY_RUN_GUARD:-1}" != 0 ] && [ -f "$g" ] || return 0
    gp=$(sed -n 1p "$GL_LOCK/guard.pid" 2>/dev/null)
    if [ -n "$gp" ] && gl_alive "$gp" "$(sed -n 1p "$GL_LOCK/guard.pidstart" 2>/dev/null)"; then return 0; fi
    if bash "$g" running; then echo "tools/mac/run.sh: a heavy-guard is already running; not starting another" >&2; return 0; fi
    bash "$g" run --parent $$ </dev/null >/dev/null 2>&1 &
    guard=$!
    printf '%s\n' "$guard" >"$GL_LOCK/guard.pid.tmp" && mv -f "$GL_LOCK/guard.pid.tmp" "$GL_LOCK/guard.pid"
    gl_pidstart "$guard" >"$GL_LOCK/guard.pidstart"
}
# SIGTERM (its trap continues everything it stopped); if it does not end in 5 s, `heavy-guard.sh resume` does the continuing (never SIGKILL: a
# killed guard could not continue anything)
guard_stop() {
    [ -n "$guard" ] || return 0
    local g="$ROOT/tools/mac/heavy-guard.sh" _i
    kill -TERM "$guard" 2>/dev/null
    for _i in $(seq 1 50); do kill -0 "$guard" 2>/dev/null || break; sleep 0.1; done
    if kill -0 "$guard" 2>/dev/null; then bash "$g" resume >/dev/null 2>&1; else wait "$guard" 2>/dev/null; fi
    guard=""
}
part=""   # a clone being made (data directory or a part of it): an interrupted run takes it away, so no half-made directory is left
cleanup() {
    [ "$_cleaned" = 0 ] || return 0
    _cleaned=1
    case "$part" in "$G"/data/livetr-*-*.part.*|"$G"/data/livetr-*-*/data.part.*) chmod -R u+w "$part" 2>/dev/null; rm -rf "$part" ;; esac
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        # interrupted: the game must not outlive the lock
        kill -TERM "$pid" 2>/dev/null; "$SERVER" -k 2>/dev/null
        for _ in 1 2 3 4 5; do kill -0 "$pid" 2>/dev/null || break; sleep 1; done
        kill -KILL "$pid" 2>/dev/null
    fi
    [ -z "$wd" ] || kill "$wd" 2>/dev/null   # (its trap stops its sleep; killing the sleep first made it "reach" the limit)
    for _j in "$sm" "$ld" "$uw"; do
        [ -z "$_j" ] || { pkill -P "$_j" 2>/dev/null; kill "$_j" 2>/dev/null; }
    done
    guard_stop
    gl_release
}
trap cleanup EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM
if [ "$concurrent" = 1 ]; then
    echo "tools/mac/run.sh: warning: --allow-concurrent (approved): NOT taking the game lock; frame times of any run are unreliable" >&2
else
    GL_CMD="victory.exe $([ "$execmode" = 1 ] && echo "--exec ${title_args[*]}" || echo title) backend $backend"
    gl_acquire "$backend" "$out" "$lock_tmo" || exit 4
fi

# ---- from here on: side effects (the game lock is held, or --allow-concurrent was approved) -----------------------------------
# Nothing above this line has written anything but the lock's own queue entry: not the data directory, not the cache, not the prefix.
[ -x "$WINE" ] || { echo "no Wine at $WINE" >&2; exit 1; }
[ -f "$exe" ] || { echo "no exe $exe" >&2; exit 1; }
if [ "$concurrent" = 0 ]; then
    # (the lock is ours; a victory.exe that is still running was not started through run.sh. Only the title's own command line
    # `victory.exe run --execute ...` counts: a run.sh that is queued for the lock has `--exe ...victory.exe` in ITS command line)
    others=$(pgrep -f "${VICTORY_MAC_EXE_PATTERN:-victory\.exe run --execute}" | tr '\n' ' ')   # (the pattern is a test knob)
    [ -z "$others" ] || { echo "another victory.exe is running (pid $others): two at once can run the GPU out of memory; stop it or use --allow-concurrent" >&2; exit 3; }
    guard_start
fi

# the title's data: cloned from the seed (which is only read) on first use, or again with --fresh-data. A clone is made beside the
# place it goes to ("*.part.<pid>") and renamed into it, so an interrupted run leaves no half-made directory that the next run takes as data.
drop_part() { [ -z "$part" ] || { chmod -R u+w "$part" 2>/dev/null; rm -rf "$part"; }; part=""; }
if [ "$derived" = 1 ]; then
    if [ "$fresh" = 1 ] && [ -d "$base" ]; then
        echo "--fresh-data: replacing $base"
        case "$base" in "$G"/data/livetr-*-*) rm -rf "$base" ;; *) die "will not delete $base" ;; esac
    fi
    if [ ! -d "$base" ]; then
        echo "making $base from the seed $seed (clone)"
        mkdir -p "$(dirname "$base")"
        part="$base.part.$$"; rm -rf "$part"; mkdir -p "$part"
        for d in data cache dxc; do
            [ -d "$seed/$d" ] || continue
            { cp -c -R "$seed/$d" "$part/$d" || { rm -rf "$part/$d"; cp -R "$seed/$d" "$part/$d"; }; } || { drop_part; die "could not clone $seed/$d"; }
        done
        chmod -R u+w "$part"   # the world-save seed is read-only; the run's clone must be writable
        mv "$part" "$base" || { drop_part; die "could not move the clone to $base"; }
        part=""
    elif [ "$newgame" = 1 ]; then
        echo "--new-game: replacing only $base/data with a clone of $seed/data (cache and dxc stay)"
        case "$base" in "$G"/data/livetr-*-*) ;; *) die "will not delete $base/data" ;; esac
        part="$base/data.part.$$"; rm -rf "$part"
        { cp -c -R "$seed/data" "$part" || { rm -rf "$part"; cp -R "$seed/data" "$part"; }; } && chmod -R u+w "$part" || { drop_part; die "could not clone $seed/data"; }
        rm -rf "$base/data"
        mv "$part" "$base/data" || { drop_part; die "could not move the clone to $base/data"; }
        part=""
    fi
    if [ "$worldsave" = 0 ] && ls -d "$base"/data/*/saves/*/*/SARC* >/dev/null 2>&1; then
        echo "note: $base/data holds a world save (the title's own autosave from an earlier run): it starts in the saved world. --new-game starts over" >&2
    fi
fi
mkdir -p "$data" "$cache"
# a run has a log folder; --exec has none (the metal back's notimpl log path needs its folder)
if [ "$execmode" = 0 ] || [ "$backend" = metal ]; then mkdir -p "$out"; fi
if [ "$profile" = fidelity ]; then
    set -- "$data"/*/storage/settings/settings.xml
    [ $# -eq 1 ] && [ -f "$1" ] || { echo "fidelity: expected exactly one settings.xml under $data/<title id>/storage/settings (found $#): run the title once, or point --data at a copy that has it" >&2; exit 1; }
    settings="$1"
    [ -f "$settings.before-fidelity" ] || cp -p "$settings" "$settings.before-fidelity"
    python3 - "$settings" "${settings_output%x*}" "${settings_output#*x}" <<'PY' || { echo "fidelity: could not set resolutionFullscreen in settings.xml" >&2; exit 1; }
import re, sys
path, w, h = sys.argv[1:4]
text = open(path, encoding="utf-8", newline="").read()
if text.count("<resolutionFullscreen>") != 1:
    sys.exit(3)
pat = re.compile(r'(<resolutionFullscreen>\s*<width value=")(\d+)("\s*/>\s*<height value=")(\d+)("\s*/>\s*</resolutionFullscreen>)')
m = pat.search(text)
if not m:
    sys.exit(3)
print(f"fidelity: settings.xml video/resolutionFullscreen {m.group(2)}x{m.group(4)} -> {w}x{h}")
new = text[:m.start()] + m.group(1) + w + m.group(3) + h + m.group(5) + text[m.end():]
if new != text:
    open(path, "w", encoding="utf-8", newline="").write(new)
PY
fi

helper_ok=1
if [ ! -x "$helper" ] || [ "$ROOT/tools/macos/dxc-d3dmetal.sh" -nt "$helper" ]; then
    sh "$ROOT/tools/macos/dxc-d3dmetal.sh" "$helper" || helper_ok=0
fi
if [ "$helper_ok" = 0 ]; then
    echo "warning: no DXC front end; the helper shaders fall back to Wine's compiler (docs/mac/PATCHES.md G4)" >&2
    unset VICTORY_DXC_HELPER VICTORY_DXC_DIR DXC_LIB
fi

# the prefix: made on first use (the D3DMetal one is cloned from the old shared name if that exists)
make_prefix() {
    [ -d "$SYS32" ] && return 0
    if [ "$backend" = d3dmetal ] && [ "$prefix" = "$d3dm_prefix" ] && [ -d "$legacy_prefix" ]; then
        echo "making the prefix $prefix from $legacy_prefix (clone)"
        cp -c -R "$legacy_prefix" "$prefix" && return 0
    fi
    [ "$backend" = metal ] || return 0   # D3DMetal: Wine makes it when the title starts
    echo "making the prefix $prefix"
    WINEDLLOVERRIDES="mscoree,mshtml=;winedbg.exe=d" "$WINE" wineboot -u >/dev/null 2>&1 || true
    WINEDLLOVERRIDES="mscoree,mshtml=;winedbg.exe=d" "$WINE" cmd /c exit >/dev/null 2>&1 || true
    [ -d "$SYS32" ] || { echo "the prefix was not made" >&2; exit 1; }
}
make_prefix
if [ "$backend" = metal ]; then
    if [ "$build" = 1 ]; then
        [ -x "$ROOT/tools/metal/build-unixlib.sh" ] || { echo "no tools/metal/build-unixlib.sh in this checkout" >&2; exit 1; }
        "$ROOT/tools/metal/build-unixlib.sh" >/dev/null || { echo "build-unixlib.sh failed" >&2; exit 1; }
    fi
    [ -f "$stage/front/d3d12.dll" ] && [ -f "$stage/front/dxgi.dll" ] && [ -f "$stage/x86_64-windows/victory_metal_bridge.dll" ] \
        || { echo "no Metal stage in $stage: run tools/metal/build-unixlib.sh (or give --build)" >&2; exit 1; }
    for n in d3d12 dxgi; do
        [ -f "$SYS32/$n.dll.metal-orig" ] || { [ -f "$SYS32/$n.dll" ] && cp "$SYS32/$n.dll" "$SYS32/$n.dll.metal-orig"; }
        cp "$stage/front/$n.dll" "$SYS32/$n.dll"
    done
    cp "$stage/x86_64-windows/victory_metal_bridge.dll" "$SYS32/victory_metal_bridge.dll"
else
    for n in d3d12 dxgi; do
        [ -f "$SYS32/$n.dll.metal-orig" ] && { mv -f "$SYS32/$n.dll.metal-orig" "$SYS32/$n.dll"; echo "put back the original $n.dll in $prefix"; }
    done
fi
[ "$backend" != metal ] || : >"$out/notimpl.log"

# --exec: victory.exe with other arguments, in the environment above, in the foreground
if [ "$execmode" = 1 ]; then
    "$SERVER" -k 2>/dev/null; "$SERVER" -w 2>/dev/null
    "${cmd[@]}" &
    pid=$!
    wait $pid; st=$?
    "$SERVER" -k 2>/dev/null
    exit "$st"
fi

# what this run is, kept with its logs
{ echo "purpose: $purpose"; echo "$(date +%FT%T) backend $backend profile $profile out $out"; } >"$out/RUN.txt"
print_environment >"$out/run-env.txt"
{
    date +%FT%T
    echo "backend $backend  profile $profile"
    echo "checkout $(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null) ($(git -C "$ROOT" status --porcelain 2>/dev/null | wc -l | tr -d ' ') changed files)"
    echo "exe $(shasum -a 256 "$exe" | cut -c1-16) $(stat -f %Sm -t %FT%T "$exe")"
    [ "$watchdog_off" = 0 ] || echo "non-fidelity: title hang watchdog disabled (VICTORY_MAC_NO_WATCHDOG=${VICTORY_MAC_NO_WATCHDOG})"
    echo "load average at start: $(uptime | sed 's/.*load averages*: //')"
    [ "$profile" = fidelity ] && echo "settings.xml $(grep -A3 '<resolutionFullscreen>' "$settings" | tr -d ' \n')" || echo "non-fidelity: output patched to ${VICTORY_OUTPUT}"
} >"$out/start.txt"
cat "$out/start.txt"

# ---- the run ---------------------------------------------------------------
"$SERVER" -k 2>/dev/null; "$SERVER" -w 2>/dev/null
until_t0=$(date +%s)
rm -f "$out/until-met.txt" "$out/until-failed.txt"
"${cmd[@]}" >"$out/stdout.log" 2>"$out/stderr.log" &
pid=$!
if [ "$tmo" -gt 0 ]; then
    ( sleep "$tmo" & sp=$!; trap 'kill $sp 2>/dev/null; exit 0' TERM; wait $sp
      echo "== time limit ${tmo}s reached" >>"$out/stderr.log"; kill -TERM $pid 2>/dev/null; "$SERVER" -k 2>/dev/null ) 2>/dev/null &
    wd=$!
fi
if [ "$until_on" = 1 ]; then
    # the watcher ends the run the way the time limit does: the title is stopped and the prefix's wineserver with it
    wargs=(--out "$out" --pid "$pid" --t0 "$until_t0" --linger "$linger")
    for _re in ${until_logs[@]+"${until_logs[@]}"}; do wargs+=(--log "$_re"); done
    [ -z "$until_world" ] || wargs+=(--world "$until_world")
    [ -z "$until_present" ] || wargs+=(--present "$until_present")
    [ "$until_capture" = 0 ] || { [ -z "$capture" ] || wargs+=(--capture "$capture"); [ "$until_record" = 0 ] || wargs+=(--record); }
    ( "$ROOT/tools/mac/until-watch.sh" "${wargs[@]}"; rc=$?
      if [ $rc = 0 ] || [ $rc = 3 ]; then kill -TERM $pid 2>/dev/null; "$SERVER" -k 2>/dev/null; fi ) 2>/dev/null &
    uw=$!
fi
if [ "$sample" -gt 0 ]; then
    (
        echo "time,gpu_in_use_bytes,gpu_alloc_bytes,gpu_recovery_count,swap_used,pages_free,pages_active,pages_wired,pages_compressed,victory_rss_kb" >"$out/mem.csv"
        while kill -0 $pid 2>/dev/null; do
            gpu=$(ioreg -r -d 1 -w0 -c IOAccelerator 2>/dev/null)
            g() { printf '%s' "$gpu" | grep -o "\"$1\"=[0-9]*" | head -1 | cut -d= -f2; }
            vm=$(vm_stat)
            v() { printf '%s' "$vm" | awk -F: -v k="$1" '$1==k {gsub(/[ .]/,"",$2); print $2}'; }
            swap=$(sysctl -n vm.swapusage | sed 's/.*used = \([0-9.]*\)M.*/\1/')
            rss=$(ps -axo rss,command | awk '/victory\.exe/ && !/awk/ {s+=$1} END {print s+0}')
            echo "$(date +%FT%T),$(g 'In use system memory'),$(g 'Alloc system memory'),$(g recoveryCount),${swap}M,$(v 'Pages free'),$(v 'Pages active'),$(v 'Pages wired down'),$(v 'Pages occupied by compressor'),$rss" >>"$out/mem.csv"
            sleep "$sample"
        done
    ) 2>/dev/null &
    sm=$!
fi
# the load average every 10 s, next to the frame times: a noisy machine is visible afterwards
(
    while kill -0 $pid 2>/dev/null; do
        echo "$(date +%FT%T) load average: $(uptime | sed 's/.*load averages*: //')" >>"$out/load.log"
        sleep "${VICTORY_LOAD_LOG_INTERVAL:-10}"   # (10 s; the knob is for the tests)
    done
) 2>/dev/null &
ld=$!
start=$(date +%s)
wait $pid; st=$?
{
    [ -z "${uw:-}" ] || wait $uw   # (it sees the title gone within a poll, and looks one last time)
    [ -z "${wd:-}" ] || { kill $wd; wait $wd; }
    [ -z "${sm:-}" ] || { kill $sm; wait $sm; }
    [ -z "${ld:-}" ] || { kill $ld; wait $ld; }
    wd=""; sm=""; ld=""; uw=""
} 2>/dev/null
"$SERVER" -k 2>/dev/null
end=$(date +%s)
{ date +%FT%T; echo "exit $st"; } >"$out/end.txt"
"$ROOT/tools/mac/collect-title-crash.sh" "$data" "$out" "$out/start.txt"   # mac L7: the title's own crash.log into $out/title-crash/ (survives --new-game)
# --until-*: met (exit 0) or not (exit 6), said in the logs and next to the run's settings
final_st=$st; until_msg=""
if [ "$until_on" = 1 ]; then
    if [ -f "$out/until-met.txt" ]; then
        IFS=$'\t' read -r u_which u_secs u_line <"$out/until-met.txt"
        until_msg="until 条件满足：${u_which} 在 ${u_secs} s"
        [ "$linger" = 0 ] || until_msg="$until_msg (linger ${linger} s, run ended at $((end - start)) s)"
        [ -z "${u_line:-}" ] || until_msg="$until_msg: ${u_line}"
        final_st=0
    else
        if [ -f "$out/until-failed.txt" ]; then u_why="$(cat "$out/until-failed.txt")"
        elif [ "$tmo" -gt 0 ] && [ $((end - start)) -ge $((tmo - 1)) ]; then u_why="not seen within --timeout ${tmo} s"
        else u_why="the title ended by itself (status $st) after $((end - start)) s first"; fi
        until_msg="until 未满足：${until_desc}: ${u_why}"
        final_st=6
    fi
    echo "== $until_msg" >>"$out/stderr.log"
    echo "$until_msg" >>"$out/start.txt"; echo "# $until_msg" >>"$out/run-env.txt"; echo "until: $until_msg" >>"$out/end.txt"
    echo "$until_msg"
fi
# frame times against load.log: frames in a stretch with load average > 30 are invalid; the statistics use the valid ones only
# (tools/mac/frametime-summary.sh; docs/mac/RUN.md). More than 20% invalid: the first line of summary.txt says 数据无效：load 超过 30.
if [ -s "$out/frametime.log" ]; then
    bash "$ROOT/tools/mac/frametime-summary.sh" "$out" >"$out/frametime-summary.txt" 2>/dev/null || : >"$out/frametime-summary.txt"
fi
{
    [ ! -s "$out/frametime-summary.txt" ] || grep -m1 '^数据无效' "$out/frametime-summary.txt"
    echo "backend $backend, profile $profile, $((end - start)) s (limit $tmo), exit status $st"
    [ -z "$until_msg" ] || echo "$until_msg (run.sh exit status $final_st)"
    for pat in 'non-fidelity' '^resolution:' 'presenting to a window' 'display mode reported' 'VICTORY_PRESENT_MAX' 'render size:' 'world frame 1'; do
        grep -a -m3 -h "$pat" "$out/stdout.log" "$out/stderr.log" 2>/dev/null
    done
    echo "ms/frame lines: $(grep -a -c 'ms/frame' "$out/stdout.log" "$out/stderr.log" 2>/dev/null | awk -F: '{s+=$2} END {print s+0}'); last:"
    grep -a -h 'ms/frame' "$out/stdout.log" "$out/stderr.log" 2>/dev/null | tail -2
    echo "GPU errors (kIOGPU/Hang/SubmissionsIgnored/device removed): $(grep -a -ci 'kIOGPU\|hang\|SubmissionsIgnored\|device removed\|DXGI_ERROR_DEVICE' "$out/stdout.log" "$out/stderr.log" 2>/dev/null | awk -F: '{s+=$2} END {print s+0}')"
    [ "$backend" != metal ] || echo "victory-metal lines: $(grep -a -c '^victory-metal' "$out/stderr.log")"
    [ ! -s "$out/frametime-summary.txt" ] || grep -v '^数据无效' "$out/frametime-summary.txt"
} >"$out/summary.txt"
cat "$out/summary.txt"
echo "logs in $out (exit $final_st)"
exit "$final_st"
