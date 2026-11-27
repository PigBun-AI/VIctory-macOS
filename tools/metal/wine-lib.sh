# Sourced by the tools/metal scripts that run Wine in a scratch prefix
# (test-front-smoke.sh, test-e2e.sh, test-wine.sh). Not meant to be run.
#
# What it gives the caller:
#   metal_wine_init        sets ROOT, WINE, WINEPREFIX (default: research/m0-prefix, never
#                          the game's wineprefix-play), WINEDEBUG, STAGE (build-unixlib.sh's
#                          output), SYS32; makes sure the prefix exists; takes a lock on it
#   metal_overrides d3dmetal|metal     the WINEDLLOVERRIDES value for a backend
#   metal_install_front    puts our d3d12.dll / dxgi.dll in the prefix's system32 (the
#                          Wine placeholders are kept as *.metal-orig)
#   metal_restore_front    puts the placeholders back (also what the EXIT trap does)
#
# Safety, in the order a mistake would cost:
#   * a prefix called wineprefix-play is refused;
#   * a crash between install and restore leaves *.metal-orig behind: the next
#     install first puts those back, so our dll is never saved as "the original";
#   * the lock is per prefix (mkdir), so two of our scripts never swap dlls under
#     each other; tools/macos/victory.sh runs `wineserver -k` for ITS prefix
#     only, so a different prefix is safe, but never point WINEPREFIX at a prefix
#     another Wine process is using.

metal_wine_init() {
    here=${here:-$(cd "$(dirname "$0")" && pwd)}
    ROOT=${ROOT:-$(cd "$here/../.." && pwd)}
    # shellcheck disable=SC1091
    . "$here/env.sh"
    STAGE="${VICTORY_METAL_OUT:-$ROOT/metal/target/wine-stage}"
    WINE="${VICTORY_WINE_DIR:?set VICTORY_WINE_DIR (a CrossOver Wine build, see tools/metal/env.sh)}/bin/wine"
    [ -x "$WINE" ] || { echo "no wine at $WINE" >&2; exit 1; }
    export WINEPREFIX="${WINEPREFIX:-$GTA6_ROOT/research/m0-prefix}"
    case "$WINEPREFIX" in
        *wineprefix-play*) echo "refusing to use the game prefix $WINEPREFIX" >&2; exit 2 ;;
    esac
    export WINEDEBUG="${WINEDEBUG:--all}"
    export ROSETTA_ADVERTISE_AVX=1
    SYS32="$WINEPREFIX/drive_c/windows/system32"
    METAL_BASE_OVERRIDES="mscoree,mshtml=;winedbg.exe=d"
    METAL_LOCK="$WINEPREFIX.metal-lock"
    if ! mkdir "$METAL_LOCK" 2>/dev/null; then
        echo "another run holds $METAL_LOCK (remove it if no tools/metal script is running)" >&2
        exit 3
    fi
    METAL_INSTALLED=0
    trap 'metal_cleanup' EXIT
    # A prefix that does not exist yet is made here, without Mono/Gecko prompts.
    if [ ! -d "$SYS32" ]; then
        WINEDLLOVERRIDES="$METAL_BASE_OVERRIDES" "$WINE" cmd /c exit >/dev/null 2>&1 || true
    fi
    metal_recover
}

# The override string of a backend. d3dmetal is what tools/macos/victory.sh and
# test-wine.sh use (builtin = Wine's D3DMetal); metal puts our native dlls first.
metal_overrides() {
    case "$1" in
        d3dmetal) echo "$METAL_BASE_OVERRIDES;d3d12,d3d12core,dxgi=b" ;;
        metal) echo "$METAL_BASE_OVERRIDES;d3d12,dxgi=n,b" ;;
        *) echo "unknown backend '$1' (d3dmetal | metal)" >&2; return 1 ;;
    esac
}

# Leftovers of a run that died between install and restore.
metal_recover() {
    for n in d3d12 dxgi; do
        if [ -f "$SYS32/$n.dll.metal-orig" ]; then
            [ "${1:-}" = quiet ] || echo "note: restoring $n.dll from an interrupted run" >&2
            rm -f "$SYS32/$n.dll"
            mv "$SYS32/$n.dll.metal-orig" "$SYS32/$n.dll"
        fi
    done
}

metal_install_front() {
    for f in d3d12.dll dxgi.dll; do
        [ -f "$STAGE/front/$f" ] || { echo "missing $STAGE/front/$f: run tools/metal/build-unixlib.sh" >&2; exit 1; }
    done
    [ -d "$STAGE/x86_64-unix" ] || { echo "no unixlib in $STAGE: run tools/metal/build-unixlib.sh" >&2; exit 1; }
    metal_recover
    for n in d3d12 dxgi; do
        [ -f "$SYS32/$n.dll" ] && mv "$SYS32/$n.dll" "$SYS32/$n.dll.metal-orig"
        cp "$STAGE/front/$n.dll" "$SYS32/$n.dll"
    done
    METAL_INSTALLED=1
}

metal_restore_front() {
    if [ "${METAL_INSTALLED:-0}" = 1 ]; then
        metal_recover quiet
        METAL_INSTALLED=0
    fi
}

metal_cleanup() {
    metal_restore_front
    [ -n "${METAL_EXTRA_CLEANUP:-}" ] && eval "$METAL_EXTRA_CLEANUP"
    rmdir "${METAL_LOCK:-/nonexistent}" 2>/dev/null || true
}
