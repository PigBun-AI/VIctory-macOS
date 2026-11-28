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
    tools/metal/test-e2e.sh               # front-e2e under Wine: D3DMetal, then our back (timings)
    tools/metal/test-present.sh [--with-d3dmetal] [--screenshot FILE]   # swap chain under Wine: colour per frame, resize
    tools/metal/run.sh [--timeout SEC] [--fidelity|--perf] [-- args]   # thin wrapper: tools/mac/run.sh --backend metal (docs/mac/RUN.md)
    tools/metal/test-front-smoke.sh [--real]   # the front's ABI against D3DMetal's (mock / real back)
    tools/metal/test-wine.sh --backend d3dmetal|metal   # victory's cargo tests under Wine on either backend
    tools/metal/compare-tests.sh --run    # both of those, a per-test table; known gaps in
                                          # metal/conformance/expected-fail.txt
    tools/metal/notimpl-stats.sh          # which unimplemented D3D12 methods the tests call, by count
    tools/metal/gen-front-stubs.sh        # regenerate front/src/shell/stubs.rs (--check in check.sh)

## Private SDK (GDKX)

GDKX (the Xbox GDK, under NDA) may be used freely while developing: look up API semantics, take structure layouts as a
reference, generate oracle numbers, use the constants and structures we need. One limit: do not commit whole headers,
documentation or the `private/` directory (`~/Documents/GTA6Project/private/gdkx/`) into the repository.
