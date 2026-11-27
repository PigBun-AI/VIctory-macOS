# victory-metal-proto

Seam B of the Metal backend: handles, POD types, the command stream, the sync
calls, `Link` and `SurfaceProvider`. `no_std` + `alloc`, no dependencies; builds
for the PE and Mach-O halves. Names no D3D12, DXGI or Metal type.

* What is in it, every call and opcode with its fields: [`docs/metal/proto.md`](../../docs/metal/proto.md) (the tables are generated from the code).
* Overview and version rules: the crate docs (`cargo doc -p victory-metal-proto --open`).

## Changing the protocol

Layering rule 4: anything that changes what crosses the seam (a field, a
payload size, a new or removed opcode or sync op, a header) is done in one
commit that also:

1. raises `PROTO_VERSION` in `src/lib.rs` by one;
2. raises the opcode's version in `cmd::opcode_version` (or the sync block's `VERSION` in `src/sync.rs`) when an existing one changes shape; a new one starts at 1;
3. updates the round-trip tests (`tests/roundtrip.rs`, `tests/common/mod.rs`: one sample per opcode, one `check_block` per sync op) and regenerates the golden files:
   `UPDATE_GOLDEN=1 cargo test -p victory-metal-proto --test golden`;
4. adds an arm with the new `layout_fingerprint()` to `tests/version.rs` (the old arms stay; they say what released versions looked like);
5. regenerates the tables: `cargo run -p victory-metal-proto --example api_table -- --write`.

`tests/version.rs` fails when the format changed and the version did not;
`tests/docs.rs` fails when the document is stale. Do not add pointer fields or
D3D12/Metal-shaped fields.

## Tests

`cargo test -p victory-metal-proto`: round trip of every opcode and sync op,
refusal of unknown/reserved opcodes, unknown versions, truncated and
out-of-bounds streams, bad enum values, a random-bytes and mutation test that
must not panic, golden bytes of a synthetic compute stream, a dump file, a root
signature and a sync block (`tests/golden/`), the version pin, the docs check.
