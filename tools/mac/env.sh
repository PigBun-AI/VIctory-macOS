# Source this on a Mac to build and test mac/livetr without installing a
# second toolchain:  . tools/mac/env.sh
#
# Reuses the Rust toolchain and llvm-mingw of a VIctory-macOS checkout
# (its target/toolchain, made by its setup.sh). Where that is: the
# VICTORY_MAC_TOOLCHAIN variable, else VIctory-macOS/target/toolchain under the GTA6
# root (the parent of the main checkouts; three levels up from worktrees/mac/<name>). Nothing is copied or written there.
#
# RUSTUP_TOOLCHAIN is pinned to the version of rust-toolchain.toml so that
# rustup does not try to install that file's Windows msvc target (the
# cross target here is x86_64-pc-windows-gnullvm, linked by llvm-mingw).
_here="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")/../.." && pwd)"
case "$_here" in */worktrees/mac/*) _g="$_here/../../.." ;; *) _g="$_here/.." ;; esac   # worktrees/mac/<name> or next to the main checkouts
VICTORY_MAC_TOOLCHAIN="${VICTORY_MAC_TOOLCHAIN:-$_g/VIctory-macOS/target/toolchain}"
if [ ! -d "$VICTORY_MAC_TOOLCHAIN/cargo" ]; then
    echo "tools/mac/env.sh: no toolchain at $VICTORY_MAC_TOOLCHAIN (set VICTORY_MAC_TOOLCHAIN, or run VIctory-macOS/setup.sh)" >&2
    return 1 2>/dev/null || exit 1
fi
export CARGO_HOME="$VICTORY_MAC_TOOLCHAIN/cargo"
export RUSTUP_HOME="$VICTORY_MAC_TOOLCHAIN/rustup"
export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-$(sed -n 's/^channel *= *"\(.*\)"/\1/p' "$_here/rust-toolchain.toml")}"
export LLVM_MINGW="$(ls -d "$VICTORY_MAC_TOOLCHAIN"/llvm-mingw-*-macos-universal 2>/dev/null | tail -n 1)"
export PATH="$CARGO_HOME/bin:$LLVM_MINGW/bin:$PATH"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER="$_here/tools/mac/cc.sh"
unset _here _g
