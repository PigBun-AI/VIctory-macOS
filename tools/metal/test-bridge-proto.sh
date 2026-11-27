#!/bin/sh
# The proto Link over the Wine unixlib: builds the staged unixlib and PE, runs
# `m0-probe.exe --proto` in the scratch prefix (research/m0-prefix). It checks
# both entry points (OP_PROTO_SUBMIT, OP_PROTO_CALL) and the proto Status <->
# NTSTATUS mapping through Wine against the back's own Executor (refusals, then a real
# HELLO, QUERY_CAPS, SHUTDOWN).
# Do not run next to another Wine test.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
"$here/build-unixlib.sh" >/dev/null
out=$("$here/run-m0.sh" --proto 2>&1) || { echo "$out"; echo "FAIL"; exit 1; }
echo "$out"
echo "$out" | grep -q '^checks failed: 0$' && echo PASS
