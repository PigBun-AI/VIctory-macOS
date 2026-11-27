# metal/

The Metal backend work (route S: Wine stays; a PE `d3d12.dll` calls Metal
through a Wine unixlib). A separate cargo workspace; the root workspace does
not know about it, and it depends on the root's crates only by `path`.

- `proto/`  seam B, the contract between front and back: handles, POD types,
  the command stream and sync calls, `Link`, `SurfaceProvider`. `no_std` +
  alloc, no dependencies, names no D3D12 or Metal type; API reference in
  `docs/metal/proto.md`, rules for changing it in `proto/README.md`.
- `back/`   Metal, through `objc2-metal` (the only crate that may name Metal).
- `bridge/` the call between the PE and the unixlib (`Link`): `WineUnixlib`
  in the PE, `InProcess` natively, and the Wine-only layer lookup.
- `probe/`  the M0 measurements; results in `docs/metal/m0-results.md`.

Setup: the root `rust-toolchain.toml` applies; add the targets once with
`rustup target add x86_64-pc-windows-gnullvm x86_64-apple-darwin`.

    tools/metal/check.sh                  # the gate: everything below, run before every commit
    tools/metal/check-layers.sh           # layering rules (--self-test proves them)
    tools/metal/check-touches.sh          # nothing upstream touched except documented hooks
                                          # (metal/UPSTREAM-TOUCHES.md; currently 0)
    cd metal && cargo test && cargo clippy --all-targets -- -D warnings
    tools/metal/build-unixlib.sh          # stage PE + unixlib for Wine
    tools/metal/run-m0.sh --layer         # run the probe under Wine
