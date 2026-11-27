# metal/

The Metal backend work (route S: Wine stays; a PE `d3d12.dll` calls Metal
through a Wine unixlib). A separate cargo workspace; the root workspace does
not know about it, and it depends on the root's crates only by `path`.

- `back/`   Metal, through `objc2-metal` (the only crate that may name Metal).
- `bridge/` the call between the PE and the unixlib (`Link`): `WineUnixlib`
  in the PE, `InProcess` natively, and the Wine-only layer lookup.
- `probe/`  the M0 measurements; results in `docs/metal/m0-results.md`.

Setup: the root `rust-toolchain.toml` applies; add the targets once with
`rustup target add x86_64-pc-windows-gnullvm x86_64-apple-darwin`.

    cd metal && cargo test && cargo clippy --all-targets -- -D warnings
    tools/metal/check-layers.sh           # layering rules
    tools/metal/build-unixlib.sh          # stage PE + unixlib for Wine
    tools/metal/run-m0.sh --layer         # run the probe under Wine
