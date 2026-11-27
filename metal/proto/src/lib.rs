//! `victory-metal-proto`: seam B of the VIctory Metal backend.
//!
//! Everything the D3D12 front and the Metal back say to each other, and
//! nothing else: handles, POD enums and descriptions, a command stream, a few
//! synchronous calls, and the two traits ([`Link`], [`SurfaceProvider`]) they
//! meet at. `#![no_std]` + `alloc`, no dependencies, so it builds for both the
//! PE (x86_64-pc-windows-gnullvm) and the Mach-O (aarch64/x86_64-apple-darwin)
//! halves. It names no D3D12, DXGI or Metal type: format ids are DXGI numbers
//! passed through as `u32`, stage masks and barrier classes are ours.
//!
//! # What is in it
//! * [`ids`]: handle newtypes ([`ResId`], [`HeapId`], [`RootSigId`], [`PsoId`], [`FenceId`], [`QueueId`] ...).
//! * [`types`]: buffer/heap/view kinds, [`ViewDesc`], [`SamplerDesc`], stage and visibility masks, [`Caps`], format ids.
//! * [`root`]: [`RootSigDesc`] (constants, root descriptors, tables with ranges, static samplers).
//! * [`pso`]: [`GraphicsPsoDesc`](pso::GraphicsPsoDesc), the graphics pipeline description (version 2).
//! * [`sync`]: [`SyncOp`] and one `repr(C)` argument block per op ([`Link::call`]).
//! * [`cmd`], [`stream`]: opcodes, payloads, [`Encoder`], zero-copy [`Decoder`] ([`Link::submit`]).
//! * [`mod@file`]: the dump/replay file (`FileHeader` + blocks).
//! * [`link`]: [`Link`], [`LinkExt`], [`SurfaceProvider`], [`Status`].
//!
//! # Version rules (layering rule 4)
//! Changing what goes over the seam means, in the same commit:
//! 1. raise [`PROTO_VERSION`] by one (any change to a payload, a sync block,
//!    a header, a descriptor record, or a new/removed opcode or sync op);
//! 2. raise the opcode's version byte (`cmd::opcode_version`) or the sync
//!    block's `VERSION` when an existing one changes shape; new ones start at 1;
//! 3. update the round-trip tests and the golden files in `tests/golden/`
//!    (`UPDATE_GOLDEN=1 cargo test -p victory-metal-proto --test golden`);
//! 4. pin the new `layout_fingerprint()` in `tests/version.rs` (keep the old
//!    arms: they say what each released version looked like);
//! 5. regenerate `docs/metal/proto.md` (`cargo run -p victory-metal-proto --example api_table`).
//!
//! `tests/version.rs` fails when the format changed but the version did not.
//! Fields that hold pointers or D3D12/Metal-shaped data may not be added.
//!
//! Readers refuse what they do not know: unknown or reserved opcodes,
//! unknown versions, lengths that run past the block, bad enum values: an
//! [`Error`], never a panic.
#![no_std]
#![deny(unsafe_code)]

extern crate alloc;

pub mod cmd;
pub mod file;
pub mod ids;
pub mod link;
pub mod pso;
pub mod registry;
pub mod root;
pub mod status;
pub mod stream;
pub mod sync;
pub mod types;
pub mod wire;

/// Version of the whole protocol. Written into every stream and file header;
/// readers accept exactly their own.
pub const PROTO_VERSION: u32 = 3;

pub use cmd::{Command, Opcode};
pub use file::{FileReader, FileWriter};
pub use ids::{FenceId, HeapId, ListId, MemHeapId, PsoId, QueueId, ResId, RootSigId, SurfaceId};
pub use link::{Link, LinkExt, SurfaceProvider, WindowRef};
pub use root::RootSigDesc;
pub use status::{Error, Status};
pub use stream::{Decoder, Encoder};
pub use sync::{SyncArgs, SyncOp};
pub use types::{Caps, SamplerDesc, ViewDesc};
