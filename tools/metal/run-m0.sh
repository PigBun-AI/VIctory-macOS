#!/bin/sh
# Runs the staged M0 probe under Wine (x86-64, Rosetta) in a prefix of its
# own, never the game's. Arguments go to m0-probe.exe (--quick, --layer,
# --hold-ms N, --which NAME.dll).
#
#   WINEPREFIX            default $GTA6_ROOT/research/m0-prefix (made on first use)
#   WINEDLLOVERRIDES      added to the Wine defaults for this prefix
#   VICTORY_METAL_OUT     where build-unixlib.sh staged things
set -eu
here=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$here/../.." && pwd)
. "$here/env.sh"
OUT="${VICTORY_METAL_OUT:-$ROOT/metal/target/wine-stage}"
WINE="${VICTORY_WINE_DIR:?set VICTORY_WINE_DIR}/bin/wine"
export WINEPREFIX="${WINEPREFIX:-$GTA6_ROOT/research/m0-prefix}"
export WINEDLLPATH="$OUT"
export WINEDEBUG="${WINEDEBUG:--all}"
export WINEDLLOVERRIDES="mscoree,mshtml=;winedbg.exe=d${WINEDLLOVERRIDES:+;$WINEDLLOVERRIDES}"
export ROSETTA_ADVERTISE_AVX=1
exec "$WINE" "$OUT/m0-probe.exe" "$@"
