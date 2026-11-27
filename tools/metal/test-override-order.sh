#!/bin/sh
# M0 open point 2: tools/macos/victory.sh sets
#   WINEDLLOVERRIDES="mscoree,mshtml=;winedbg.exe=d;d3d12,d3d12core,dxgi=b;$USER_VALUE"
# Does an entry written after "d3d12,d3d12core,dxgi=b" override it?
#
# A stand-in dll (an unmarked copy of our bridge dll, exporting
# victory_metal_bridge_anchor) is put in the scratch prefix as d3d12.dll and
# dxgi.dll; m0-probe.exe --which loads d3d12.dll and says whether the stand-in
# or Wine's builtin answered. The exact assignment is taken from the current
# victory.sh, not retyped. The prefix is restored afterwards.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$here/../.." && pwd)
. "$here/env.sh"
OUT="${VICTORY_METAL_OUT:-$ROOT/metal/target/wine-stage}"
WINE="${VICTORY_WINE_DIR:?set VICTORY_WINE_DIR}/bin/wine"
export WINEPREFIX="${WINEPREFIX:-$GTA6_ROOT/research/m0-prefix}"
export WINEDLLPATH="$OUT"
export WINEDEBUG=-all
SYS32="$WINEPREFIX/drive_c/windows/system32"

line=$(grep '^export WINEDLLOVERRIDES=' "$ROOT/tools/macos/victory.sh")
echo "victory.sh: $line"

# Make sure the prefix exists, then swap the stand-in in.
"$WINE" "$OUT/m0-probe.exe" --which kernel32.dll >/dev/null 2>&1 || true
restore() {
    for n in d3d12 dxgi; do
        rm -f "$SYS32/$n.dll"
        [ -f "$SYS32/$n.dll.m0-orig" ] && mv "$SYS32/$n.dll.m0-orig" "$SYS32/$n.dll"
    done
}
trap restore EXIT
for n in d3d12 dxgi; do
    [ -f "$SYS32/$n.dll" ] && mv "$SYS32/$n.dll" "$SYS32/$n.dll.m0-orig"
    cp "$OUT/standin.dll" "$SYS32/$n.dll"
done

fail=0
WHICH_FLAGS=
# $1 label, $2 the value victory.sh would append (user's WINEDLLOVERRIDES),
# $3 expected: ours | builtin, $4 raw: use $2 as the whole value instead
try() {
    if [ "${4:-}" = raw ]; then
        WINEDLLOVERRIDES="$2"
    else
        WINEDLLOVERRIDES="$2"
        eval "$line"
    fi
    export WINEDLLOVERRIDES
    result=$("$WINE" "$OUT/m0-probe.exe" --which d3d12.dll $WHICH_FLAGS 2>&1 | grep '^WHICH' || echo "WHICH: no answer")
    case "$result" in
        *"OUR STAND-IN"*) got=ours ;;
        *"Wine builtin"*) got=builtin ;;
        *) got=none ;;
    esac
    verdict=PASS
    [ "$got" = "$3" ] || { verdict=FAIL; fail=1; }
    echo "$verdict  $1"
    echo "      WINEDLLOVERRIDES=$WINEDLLOVERRIDES"
    echo "      expected $3, got $got: $result"
}

try "control: victory.sh alone (=b)"                         ""                       builtin
try "user appends d3d12,dxgi=n,b after =b"                   "d3d12,dxgi=n,b"         ours
try "user appends d3d12,dxgi=n after =b"                     "d3d12,dxgi=n"           ours
try "reverse order: =n,b first, =b last (last must win)"     "d3d12,dxgi=n,b;d3d12,d3d12core,dxgi=b" builtin raw
# Same, loading the way victory-gpu's root.rs does: LOAD_LIBRARY_SEARCH_SYSTEM32.
WHICH_FLAGS=--system32
try "SEARCH_SYSTEM32 load, control (=b)"                      ""                       builtin
try "SEARCH_SYSTEM32 load, user appends d3d12,dxgi=n,b"       "d3d12,dxgi=n,b"         ours
exit $fail
