#!/bin/sh
# Runs VIctory's Windows tests (cargo test, gnullvm) under Wine on one of two
# D3D12 backends, in a scratch prefix. Same approach as tools/macos/test-wine.sh
# (which stays as it is; this is the Metal-backend variant, upstream files are
# not edited):
#
#   tools/metal/test-wine.sh --backend d3dmetal|metal [--no-build] [-- cargo test args]
#
#   --backend d3dmetal   Wine's builtin D3DMetal, the way tools/macos/victory.sh
#                        runs the game:  d3d12,d3d12core,dxgi=b
#   --backend metal      our d3d12.dll / dxgi.dll (metal/front) over the bridge,
#                        the Wine unixlib and the Metal back:  d3d12,dxgi=n,b
#                        with WINEDLLPATH=<stage>. They are copied over the
#                        prefix's system32 for the run and put back at the end
#                        (also after Ctrl-C; a crash is repaired by the next run).
#   --no-build           use the stage from the last tools/metal/build-unixlib.sh
#
# The cargo test arguments default to `-p victory-shader --no-fail-fast`; give
# your own after `--` (for example `-- -p victory-shader --no-fail-fast -- --test-threads=1`).
#
# The prefix is research/metal-test-prefix (WINEPREFIX overrides), never the
# game's wineprefix-play. A lock refuses two runs in one prefix. About
# `wineserver -k`: tools/macos/victory.sh stops a prefix's wineserver; this
# script stops only the wineserver of ITS OWN prefix (at the start, so no old
# server holds the dlls, and at the end), by naming WINEPREFIX explicitly.
# Wine processes of other prefixes, the game's included, are never touched.
#
# Environment: WINE (default: the Wine of tools/metal/env.sh), VICTORY_HELPER_DXC
# and DXC_LIB (default: the DXC of D3DMetal's library, front end built into
# target/dxc-d3dmetal like victory.sh does), WINEDEBUG (default -all).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
backend=""
build=1
while [ $# -gt 0 ]; do
    case "$1" in
        --backend) backend=${2:-}; shift ;;
        --no-build) build=0 ;;
        --) shift; break ;;
        *) echo "usage: $0 --backend d3dmetal|metal [--no-build] [-- cargo test args]" >&2; exit 2 ;;
    esac
    shift
done
case "$backend" in
    d3dmetal|metal) ;;
    *) echo "usage: $0 --backend d3dmetal|metal [--no-build] [-- cargo test args]" >&2; exit 2 ;;
esac
[ $# -gt 0 ] || set -- -p victory-shader --no-fail-fast

# shellcheck disable=SC1091
. "$here/wine-lib.sh"
# A prefix of its own, not research/m0-prefix (the e2e scripts' one).
if [ -z "${WINEPREFIX:-}" ]; then
    . "$here/env.sh"
    WINEPREFIX="$GTA6_ROOT/research/metal-test-prefix"
fi
metal_wine_init

TARGET=x86_64-pc-windows-gnullvm
cd "$ROOT"
WINESERVER="$(dirname "$WINE")/wineserver"
stop_our_wineserver() {
    # Only this prefix's server: WINEPREFIX is set explicitly for the call.
    [ -x "$WINESERVER" ] && WINEPREFIX="$WINEPREFIX" "$WINESERVER" -k >/dev/null 2>&1 || true
}
RUNNER_DIR="$(mktemp -d /tmp/victory-wine.XXXXXX)"
# A wrapper rather than a symlink: some Wine builds find their loader next to the path they were started from.
printf '#!/bin/sh\nexec "$WINE" "$@"\n' >"$RUNNER_DIR/wine"
chmod +x "$RUNNER_DIR/wine"
METAL_EXTRA_CLEANUP='stop_our_wineserver; rm -rf "$RUNNER_DIR"'
stop_our_wineserver

# The DXC of D3DMetal for the shader tests, as tools/macos/victory.sh builds it.
DXC_LIB="$(dirname "$WINE")/../lib/external/D3DMetal.framework/Versions/A/Resources/libdxcompiler.dylib"
if [ -z "${VICTORY_HELPER_DXC:-}" ] && [ -f "$DXC_LIB" ]; then
    helper="$ROOT/target/dxc-d3dmetal"
    mkdir -p "$ROOT/target"
    if [ ! -x "$helper" ] || [ tools/macos/dxc-d3dmetal.sh -nt "$helper" ]; then
        sh tools/macos/dxc-d3dmetal.sh "$helper" || helper=
    fi
    [ -n "$helper" ] && export DXC_LIB VICTORY_HELPER_DXC="$helper"
fi

export WINE
export CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_RUNNER="$RUNNER_DIR/wine"
export WINEDLLOVERRIDES
WINEDLLOVERRIDES=$(metal_overrides "$backend")
export ROSETTA_ADVERTISE_AVX=1

if [ "$backend" = metal ]; then
    [ "$build" = 1 ] && "$here/build-unixlib.sh" >/dev/null
    metal_install_front
    export WINEDLLPATH="$STAGE"
    echo "== backend: metal (our d3d12.dll/dxgi.dll in $SYS32, WINEDLLPATH=$STAGE)"
else
    echo "== backend: d3dmetal (Wine builtin)"
fi
echo "== prefix: $WINEPREFIX"
echo "== cargo test --target $TARGET $*"
set +e
cargo test --target "$TARGET" "$@"
code=$?
set -e
exit "$code"
