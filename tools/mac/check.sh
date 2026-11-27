#!/bin/sh
# Native build and tests, then the cross build of victory.exe (release,
# x86_64-pc-windows-gnullvm). Logs go to $LOGS (default: ./target/mac-logs).
# Usage: tools/mac/check.sh        (from anywhere in the checkout)
set -eu
cd "$(dirname "$0")/../.."
. tools/mac/env.sh
LOGS="${LOGS:-target/mac-logs}"
mkdir -p "$LOGS"
echo "== native build";  cargo build --workspace --locked 2>&1 | tee "$LOGS/native-build.log" | tail -n 3
echo "== native tests";  cargo test --workspace --locked --no-fail-fast 2>&1 | tee "$LOGS/native-test.log" | grep -E '^test result|FAILED|^error' || true
echo "== cross build";   cargo build --release -p victory --target x86_64-pc-windows-gnullvm --locked 2>&1 | tee "$LOGS/cross-build.log" | tail -n 3
ls -l target/x86_64-pc-windows-gnullvm/release/victory.exe
