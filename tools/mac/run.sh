#!/bin/bash
# The one way to start victory.exe on this Mac: CrossOver Wine, with either
# Apple's D3DMetal or our Metal backend, scripted keys, logs. Not a build step
# (tools/mac/check.sh builds). Full reference: docs/mac/RUN.md.
#
#   tools/mac/run.sh [options] [-- arguments for the title]
#   tools/mac/run.sh --print-env [options]     print the environment and the command, run nothing
#   tools/mac/run.sh --exec [options] -- ARGS  run `victory.exe ARGS` (for example `warm --jobs 4`) instead of the title, in
#                                              the very same environment and prefix; no keys, no time limit, no log folder
#
# What a run is made of (each has an option; the options only ever choose, the
# rest below is the same for every combination):
#   --backend d3dmetal|metal     (VICTORY_MAC_BACKEND, default d3dmetal) the ONLY thing the backends differ in:
#                                the prefix, the dlls in it, WINEDLLPATH/WINEDLLOVERRIDES, VICTORY_METAL_LOG/_NOTIMPL_LOG
#   --profile fidelity|perf      (VICTORY_MAC_PROFILE, default perf; --fidelity / --perf)
#       fidelity  Series X, the title's own output size from its settings (2560x1440, written into the DATA
#                 directory's copy of settings.xml before the run), VICTORY_OUTPUT=0: no resolution patch;
#                 internal size, dynamic resolution and TAA upscaling are the title's. Own data directory.
#       perf      the output patched to 1280x720 (VICTORY_OUTPUT=1280x720; patch L3), logs
#                 `non-fidelity: output patched to 1280x720`. Not a fidelity result.
#   --present-max WxH|none       VICTORY_PRESENT_MAX: smaller host window and swap chain, same title output (P1)
#   --pool-check skip|keep       VICTORY_POOL_CHECK: jump over the back buffer pool's check without patching sizes (L3)
#   --log-render-size            VICTORY_LOG_RENDER_SIZE=1: the title's internal size in the log (P2); costs a lock
#                                per draw, so leave it off for timing runs
#   --exe PATH --out DIR --timeout SEC (0 = none) --cache DIR --data DIR --prefix DIR
#   --seed DIR                   the read-only seed the data directories are cloned from (default ../victory-data-livetr)
#   --fresh-data                 clone the run's data directory from the seed again, whatever is there (reproducible start)
#   --keys SPEC | --no-keys      VICTORY_KEYS, default 8-3600/3:Enter (Enter every 3 s from 8 s)
#   --input FILE|none            VICTORY_INPUT_REPLAY, default tools/mac/input/world-neutral.txt (I1)
#   --record FILE                VICTORY_INPUT_RECORD (relative: under --out); turns the default replay off
#   --capture-frames LIST        VICTORY_CAPTURE_FRAMES (diagnostics builds)
#   --frametime-log              VICTORY_FRAMETIME_LOG=<out>/frametime.log
#   --tracking 0|1               VICTORY_FAULT_TRACKING (default 1, as the code's default)
#   --masked SPEC                VICTORY_PATCH_MASKED (default: not set, the code follows fault tracking)
#   --console series-x|series-s  the title's console model (default series-x: no --console is passed)
#   --build                      (metal) run tools/metal/build-unixlib.sh first
#   --sample SEC                 every SEC seconds append memory, swap and GPU lines to <out>/mem.csv
#   --exec                       run `victory.exe <the arguments after -->` in the foreground (stdout/stderr stay on the terminal,
#                                the exit status is victory.exe's) with the environment of a run: same prefix, DLLs, data and cache
#                                folders, DXC front end. For `victory.exe warm` (tools/mac/drop-ab.sh). Needs at least one argument.
#   --allow-concurrent           start although another victory.exe is running (two can run the GPU out of memory)
#
# Other VICTORY_* / WINE* variables in the environment pass through unchanged.
# Wine: VICTORY_MAC_WINE_DIR, else ../VIctory-macOS/target/wine-cx next to this checkout.
# Builds (once) the DXC front end tools/macos/dxc-d3dmetal.sh and sets VICTORY_DXC_HELPER / DXC_LIB /
# VICTORY_DXC_DIR for the helper shaders (docs/mac/PATCHES.md, G4). Only the Wine processes of the
# chosen prefix are ever stopped.
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
G="${GTA6_ROOT:-$(cd "$ROOT/.." && pwd)}"
WDIR="${VICTORY_MAC_WINE_DIR:-$G/VIctory-macOS/target/wine-cx}"
WINE="$WDIR/bin/wine"; SERVER="$WDIR/bin/wineserver"

die() { echo "tools/mac/run.sh: $*" >&2; exit 2; }
usage() { sed -n '2,/^set -u/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' >&2; exit 2; }
# a Windows path for a Wine process: Z: is the Unix root
winpath() { printf 'Z:%s' "$(printf '%s' "$1" | tr / '\\')"; }

# ---- options ---------------------------------------------------------------
backend="${VICTORY_MAC_BACKEND:-d3dmetal}"; profile="${VICTORY_MAC_PROFILE:-perf}"
exe="$ROOT/target/x86_64-pc-windows-gnullvm/release/victory.exe"
out=""; tmo=900; cache=""; data=""; prefix=""; tracking=""; masked=""; build=0; sample=0; concurrent=0; fresh=0; seed=""
keys="8-3600/3:Enter"; input="$ROOT/tools/mac/input/world-neutral.txt"; record=""; capture=""
present_max=""; pool_check=""; render_size=0; frametime=0; console=""; print_env=0; execmode=0; settings_output="2560x1440"
while [ $# -gt 0 ]; do
    case "$1" in
        --backend) backend=${2:?--backend d3dmetal|metal}; shift ;;
        --profile) profile=${2:?--profile fidelity|perf}; shift ;;
        --fidelity) profile=fidelity ;; --perf) profile=perf ;;
        --exe) exe=${2:?}; shift ;; --out) out=${2:?}; shift ;; --timeout) tmo=${2:?}; shift ;;
        --cache) cache=${2:?}; shift ;; --data) data=${2:?}; shift ;; --prefix) prefix=${2:?}; shift ;;
        --seed) seed=${2:?}; shift ;; --fresh-data) fresh=1 ;;
        --keys) keys=${2:?}; shift ;; --no-keys) keys=none ;;
        --input) input=${2:?}; shift ;; --record) record=${2:?}; shift ;;
        --capture-frames) capture=${2:?}; shift ;;
        --tracking) tracking=${2:?}; shift ;; --masked) masked=${2:?}; shift ;;
        --present-max) present_max=${2:?}; shift ;; --pool-check) pool_check=${2:?}; shift ;;
        --settings-output) settings_output=${2:?}; shift ;;
        --log-render-size) render_size=1 ;; --frametime-log) frametime=1 ;;
        --console) console=${2:?}; shift ;; --build) build=1 ;; --sample) sample=${2:?}; shift ;;
        --allow-concurrent) concurrent=1 ;;
        --print-env|--dry-run) print_env=1 ;;
        --exec) execmode=1 ;;
        -h|--help) usage ;;
        --) shift; break ;;
        *) echo "unknown option $1" >&2; usage ;;
    esac
    shift
done
title_args=("$@")
case "$backend" in d3dmetal|metal) ;; *) die "--backend is d3dmetal or metal, not '$backend'" ;; esac
case "$profile" in fidelity|perf) ;; *) die "--profile is fidelity or perf, not '$profile'" ;; esac
case "$settings_output" in 1280x720|1664x936|1920x1080|2560x1440|3840x2160) ;;
    *) die "--settings-output is one of the title's modes: 1280x720 1664x936 1920x1080 2560x1440 3840x2160" ;; esac
case "$console" in ''|series-x|series-s) ;; *) die "--console is series-x or series-s" ;; esac
case "$pool_check" in ''|skip|keep) ;; *) die "--pool-check is skip or keep" ;; esac
case "$tmo" in ''|*[!0-9]*) die "--timeout takes whole seconds" ;; esac
case "$sample" in ''|*[!0-9]*) die "--sample takes whole seconds" ;; esac

# ---- paths -----------------------------------------------------------------
time_name="$(date +%Y%m%d-%H%M%S)"
[ "$print_env" = 1 ] && time_name="<time>"
[ -n "$out" ] || out="$ROOT/target/mac-runs/$time_name"
# One data directory per profile AND backend, each cloned (APFS clone) from the same read-only seed on first use, so two
# runs of different backends start from the same state and can run side by side. The seed is never written.
[ -n "$seed" ] || seed="$G/victory-data-livetr"
base="$G/victory-data-livetr-$profile-$backend"
derived=1; { [ -n "${VICTORY_DATA_DIR:-}" ] || [ -n "$data" ] || [ -n "$cache" ]; } && derived=0
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
stage="${VICTORY_METAL_OUT:-$ROOT/metal/target/wine-stage}"
SYS32="$prefix/drive_c/windows/system32"

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
[ "$frametime" = 0 ] || export VICTORY_FRAMETIME_LOG="$out/frametime.log"
# the title's output: the profile decides, and says so in the log
if [ "$profile" = fidelity ]; then
    export VICTORY_OUTPUT=0
else
    export VICTORY_OUTPUT="${VICTORY_OUTPUT:-1280x720}"
fi
case "$present_max" in none) UNSET+=(VICTORY_PRESENT_MAX); unset VICTORY_PRESENT_MAX ;; '') ;; *) export VICTORY_PRESENT_MAX="$present_max" ;; esac
case "$pool_check" in skip) export VICTORY_POOL_CHECK=skip ;; keep) UNSET+=(VICTORY_POOL_CHECK); unset VICTORY_POOL_CHECK ;; esac
[ "$render_size" = 0 ] || export VICTORY_LOG_RENDER_SIZE=1
# DXC front end for the helper shaders (x86_64, run under Rosetta), the same for every run
helper="$ROOT/target/dxc-d3dmetal"
export VICTORY_DXC_HELPER="$helper" VICTORY_DXC_DIR="$(dirname "$cache")/dxc"
export DXC_LIB="$WDIR/lib/external/D3DMetal.framework/Versions/A/Resources/libdxcompiler.dylib"

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
        echo "# fidelity: Series X; settings.xml in the data directory is set to $settings_output (video/resolutionFullscreen) before the run; no resolution patch (VICTORY_OUTPUT=0)"
    else
        echo "# perf: non-fidelity: output patched to ${VICTORY_OUTPUT} (the title's code, patch L3)"
    fi
    if [ "$backend" = metal ]; then
        echo "# metal: stage $stage; front/d3d12.dll front/dxgi.dll x86_64-windows/victory_metal_bridge.dll go into the prefix's system32 (originals kept as .metal-orig)"
    else
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

if [ "$print_env" = 1 ]; then print_environment; exit 0; fi

# ---- from here on: side effects --------------------------------------------
[ -x "$WINE" ] || { echo "no Wine at $WINE" >&2; exit 1; }
[ -f "$exe" ] || { echo "no exe $exe" >&2; exit 1; }
if [ "$concurrent" = 0 ]; then
    others=$(pgrep -f 'victory\.exe' | tr '\n' ' ')
    [ -z "$others" ] || { echo "another victory.exe is running (pid $others): two at once can run the GPU out of memory; stop it or use --allow-concurrent" >&2; exit 3; }
fi

# the title's data: cloned from the seed (which is only read) on first use, or again with --fresh-data
if [ "$derived" = 1 ]; then
    if [ "$fresh" = 1 ] && [ -d "$base" ]; then
        echo "--fresh-data: replacing $base"
        case "$base" in "$G"/victory-data-livetr-*-*) rm -rf "$base" ;; *) die "will not delete $base" ;; esac
    fi
    if [ ! -d "$base" ]; then
        echo "making $base from the seed $seed (clone)"
        mkdir -p "$base"
        for d in data cache dxc; do [ -d "$seed/$d" ] && { cp -c -R "$seed/$d" "$base/$d" || cp -R "$seed/$d" "$base/$d"; }; done
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
    "${cmd[@]}"; st=$?
    "$SERVER" -k 2>/dev/null
    exit "$st"
fi

# what this run is, kept with its logs
print_environment >"$out/run-env.txt"
{
    date +%FT%T
    echo "backend $backend  profile $profile"
    echo "checkout $(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null) ($(git -C "$ROOT" status --porcelain 2>/dev/null | wc -l | tr -d ' ') changed files)"
    echo "exe $(shasum -a 256 "$exe" | cut -c1-16) $(stat -f %Sm -t %FT%T "$exe")"
    [ "$profile" = fidelity ] && echo "settings.xml $(grep -A3 '<resolutionFullscreen>' "$settings" | tr -d ' \n')" || echo "non-fidelity: output patched to ${VICTORY_OUTPUT}"
} >"$out/start.txt"
cat "$out/start.txt"

# ---- the run ---------------------------------------------------------------
"$SERVER" -k 2>/dev/null; "$SERVER" -w 2>/dev/null
"${cmd[@]}" >"$out/stdout.log" 2>"$out/stderr.log" &
pid=$!
if [ "$tmo" -gt 0 ]; then
    ( sleep "$tmo"; echo "== time limit ${tmo}s reached" >>"$out/stderr.log"; kill -TERM $pid 2>/dev/null; "$SERVER" -k 2>/dev/null ) 2>/dev/null &
    wd=$!
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
start=$(date +%s)
wait $pid; st=$?
{
    [ -z "${wd:-}" ] || { pkill -P $wd; kill $wd; wait $wd; }
    [ -z "${sm:-}" ] || { kill $sm; wait $sm; }
} 2>/dev/null
"$SERVER" -k 2>/dev/null
end=$(date +%s)
{ date +%FT%T; echo "exit $st"; } >"$out/end.txt"
{
    echo "backend $backend, profile $profile, $((end - start)) s (limit $tmo), exit status $st"
    for pat in 'non-fidelity' '^resolution:' 'presenting to a window' 'display mode reported' 'VICTORY_PRESENT_MAX' 'render size:' 'world frame 1'; do
        grep -a -m3 -h "$pat" "$out/stdout.log" "$out/stderr.log" 2>/dev/null
    done
    echo "ms/frame lines: $(grep -a -c 'ms/frame' "$out/stdout.log" "$out/stderr.log" 2>/dev/null | awk -F: '{s+=$2} END {print s+0}'); last:"
    grep -a -h 'ms/frame' "$out/stdout.log" "$out/stderr.log" 2>/dev/null | tail -2
    echo "GPU errors (kIOGPU/Hang/SubmissionsIgnored/device removed): $(grep -a -ci 'kIOGPU\|hang\|SubmissionsIgnored\|device removed\|DXGI_ERROR_DEVICE' "$out/stdout.log" "$out/stderr.log" 2>/dev/null | awk -F: '{s+=$2} END {print s+0}')"
    [ "$backend" != metal ] || echo "victory-metal lines: $(grep -a -c '^victory-metal' "$out/stderr.log")"
} >"$out/summary.txt"
cat "$out/summary.txt"
echo "logs in $out (exit $st)"
exit "$st"
