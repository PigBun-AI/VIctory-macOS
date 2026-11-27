# Sourced by the other tools/metal scripts. Sets up Rust and Wine for the
# Metal backend work without touching anything outside the environment.
#
# Override any of these before sourcing:
#   GTA6_ROOT             parent of the checkouts and research/ (default ~/Documents/GTA6Project)
#   RUSTUP_HOME/CARGO_HOME  toolchain with the targets in metal/README.md
#   VICTORY_WINE_DIR      a CrossOver Wine build, x86_64 (setup.sh's target/wine-cx)

GTA6_ROOT="${GTA6_ROOT:-$HOME/Documents/GTA6Project}"

if [ -z "${RUSTUP_HOME:-}" ] && [ -d "$GTA6_ROOT/research/probe-env/rustup" ]; then
    export RUSTUP_HOME="$GTA6_ROOT/research/probe-env/rustup"
    export CARGO_HOME="$GTA6_ROOT/research/probe-env/cargo"
fi
if [ -n "${CARGO_HOME:-}" ]; then
    PATH="$CARGO_HOME/bin:$PATH"
fi
export PATH

if [ -z "${VICTORY_WINE_DIR:-}" ]; then
    for dir in "$GTA6_ROOT/VIctory-macOS/target/wine-cx" \
               "$HOME/Library/Caches/victory-macos/target/wine-cx"; do
        if [ -x "$dir/bin/wine" ]; then
            VICTORY_WINE_DIR="$dir"
            break
        fi
    done
fi
export VICTORY_WINE_DIR
