#!/bin/sh
# The gate for metal/ (layering rule 8): run it before every commit.
#
#   1. check-layers.sh --self-test, check-layers.sh      layering rules
#   2. check-touches.sh --self-test, check-touches.sh    nothing upstream touched
#   3. cargo test  in metal/: native aarch64, then x86_64-apple-darwin (Rosetta)
#   4. cargo clippy --all-targets -D warnings:
#        aarch64-apple-darwin, x86_64-apple-darwin, x86_64-pc-windows-gnullvm
#   5. cargo doc of proto with warnings denied (broken links)
#   6. cargo fmt --check
#   7. gen-front-stubs.py --check    the generated front shell stubs are current
#
# Every step runs even when an earlier one failed; a passing step prints one
# line, a failing one prints its whole output. -v prints the output of every step.
# Toolchain and targets: tools/metal/env.sh, metal/README.md. Exit 0 = all green.
#
#   tools/metal/check.sh [-v]
set -u
here=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$here/../.." && pwd)
# shellcheck disable=SC1091
. "$here/env.sh"

verbose=0
[ "${1:-}" = "-v" ] && verbose=1

LOGDIR=$(mktemp -d "${TMPDIR:-/tmp}/metal-check.XXXXXX")
trap 'rm -rf "$LOGDIR"' EXIT
n=0
failed_steps=""
tests_total=0

# step NAME COMMAND...: run, keep the output, print one line (all of the
# output when it failed, or with -v).
step() {
    name=$1
    shift
    n=$((n + 1))
    log="$LOGDIR/$n.log"
    start=$(date +%s)
    "$@" >"$log" 2>&1
    code=$?
    secs=$(($(date +%s) - start))
    if [ "$code" = 0 ]; then
        # Count the tests that passed in this step.
        passed=$(sed -n 's/^test result: ok\. \([0-9][0-9]*\) passed.*/\1/p' "$log" | awk '{ s += $1 } END { print s + 0 }')
        extra=""
        if [ "$passed" -gt 0 ]; then
            tests_total=$((tests_total + passed))
            extra=", $passed tests passed"
        fi
        echo "PASS  $name (${secs}s$extra)"
        [ "$verbose" = 1 ] && sed 's/^/      | /' "$log"
    else
        echo "FAIL  $name (exit $code, ${secs}s)"
        sed 's/^/      | /' "$log"
        failed_steps="$failed_steps
  - $name"
    fi
}

cd "$ROOT" || exit 2
echo "metal check: $(git rev-parse --short HEAD 2>/dev/null) on $(git rev-parse --abbrev-ref HEAD 2>/dev/null), rustc $(rustc --version 2>/dev/null | cut -d' ' -f2)"

step "layers: self-test"    tools/metal/check-layers.sh --self-test
step "layers"               tools/metal/check-layers.sh
step "touches: self-test"   tools/metal/check-touches.sh --self-test
step "touches"              tools/metal/check-touches.sh

cd "$ROOT/metal" || exit 2
host=$(rustc -vV | sed -n 's/^host: //p')
step "cargo test (native, $host)"  cargo test --workspace --locked
if [ "$host" = "x86_64-apple-darwin" ]; then
    echo "SKIP  cargo test (x86_64-apple-darwin): the host is x86_64 already"
else
    step "cargo test (x86_64-apple-darwin, Rosetta)" cargo test --workspace --locked --target x86_64-apple-darwin
fi
for target in aarch64-apple-darwin x86_64-apple-darwin x86_64-pc-windows-gnullvm; do
    step "cargo clippy ($target)" cargo clippy --workspace --all-targets --locked --target "$target" -- -D warnings
done
step "cargo doc (proto)"    env RUSTDOCFLAGS="-D warnings" cargo doc -p victory-metal-proto --no-deps --locked
# Not --all: that would also format the upstream crates metal/ depends on by path.
step "cargo fmt --check"    cargo fmt --check
cd "$ROOT" || exit 2
step "front stubs current"  python3 tools/metal/gen-front-stubs.py --check

echo
if [ -z "$failed_steps" ]; then
    echo "metal check: ALL PASSED ($n steps, $tests_total tests)"
    exit 0
fi
echo "metal check: FAILED$failed_steps"
exit 1
