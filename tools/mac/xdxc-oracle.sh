#!/bin/bash
set -eo pipefail
cd "$(dirname "$0")/../.."
export GTA6_ROOT="${GTA6_ROOT:-$HOME/Documents/GTA6Project}"
. tools/mac/env.sh
exec "$GTA6_ROOT/VIctory-livetr/tools/mac/heavy.sh" --jobs 2 --label xdxc --run-timeout 540 -- cargo test -p victory-shader --lib "xdxc_oracle::tests::${1:-}" -- --nocapture --test-threads=1
