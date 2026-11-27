#!/bin/sh
# Which unimplemented D3D12/DXGI methods does a program call, and how often?
#
#   tools/metal/notimpl-stats.sh [-- cargo test args]      run the Windows tests of victory-shader (or the
#                                                         given cargo test arguments) on the Metal backend
#                                                         with VICTORY_METAL_NOTIMPL_LOG set
#   tools/metal/notimpl-stats.sh LOGFILE                   count an existing log
#
# Every stub of the front (and the few places that return E_NOTIMPL / UNSUPPORTED on purpose:
# textures, typed buffer views) appends one line `Interface::Method(detail)` to the file named by
# VICTORY_METAL_NOTIMPL_LOG. This prints the lines sorted by number of calls.
# Caveat: a test stops at its first failed call, so what it would have called next is not counted.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
if [ $# -ge 1 ] && [ "$1" != "--" ] && [ -f "$1" ]; then
    log=$1
else
    [ "${1:-}" = "--" ] && shift
    [ $# -gt 0 ] || set -- -p victory-shader --no-fail-fast -- --test-threads=1
    dir=$(mktemp -d "${TMPDIR:-/tmp}/metal-notimpl.XXXXXX")
    log=$dir/notimpl.log
    # a Windows path for the Wine process: Z: is the Unix root
    VICTORY_METAL_NOTIMPL_LOG="Z:$(printf '%s' "$log" | tr / '\\')" \
        "$here/test-wine.sh" --backend metal -- "$@" >"$dir/test.log" 2>&1 || true
    echo "test log: $dir/test.log"
fi
[ -f "$log" ] || { echo "no unimplemented call was made (no $log)"; exit 0; }
echo "calls  method"
sort "$log" | uniq -c | sort -rn | awk '{ n = $1; $1 = ""; printf "%5d %s\n", n, substr($0, 2); total += n } END { printf "%5d total\n", total }'
