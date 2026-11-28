//! Mac patches of the loader (docs/mac/PATCHES.md, patches L1-L7), kept apart
//! from the Windows line's files: edits to patch the title's code that
//! exist only because the Windows exe runs on Wine + Rosetta 2 on a Mac.
//! Ported from VIctory-macOS (origin/main 7772921b); nothing here runs unless
//! `apps/victory/src/mac.rs` asks for it, and it asks only under Rosetta.
//!
//! - [`masked`]: AVX masked stores redone as plain stores (Rosetta cannot
//!   resume a fault in the middle of one).
//! - [`wide`]: 256-bit stores redone as two 128-bit ones, from a list made by
//!   `cargo xtask wide-stores`.
//! - [`resolution`]: the title's output size (1280x720 by default on a Mac, L3).
//! - [`output_chain`]: which output chain a run takes: native 1440p (the perf profile's default,
//!   `apps/victory/src/native_1440p.rs`), L3's output, or the title's own 4K (fidelity) (X13).
//! - [`clwb`]: `clwb` write-back loops turned into NOPs (a fault per loop
//!   under Rosetta).
//! - [`cmpxchg16`]: a diagnostic for 16-bit `lock cmpxchg` at addresses = 15 mod 16
//!   (L6; off by default, `VICTORY_MAC_CMPXCHG16_CHECK=1`).
//! - [`int3dump`]: a dump of the pool at the title's own `int3` assertions (L7; off by default,
//!   `VICTORY_MAC_INT3_DUMP=<rva>[,<rva>...]`).
//! - [`watchdog`]: the title's hang watchdog turned off (L5; off by default,
//!   `VICTORY_MAC_NO_WATCHDOG=1`).

pub mod clwb;
pub mod cmpxchg16;
pub mod int3dump;
pub mod masked;
pub mod output_chain;
pub mod resolution;
pub mod prestream; // mac L8
pub mod pool_owner; // mac L9
pub mod watchdog;
pub mod wide;

#[cfg(windows)]
pub use crate::threads::in_title_image;

/// `IMAGE_SECTION_HEADER::Characteristics` flag (VIctory-macOS has this in
/// victory-pe's layout.rs; kept here so the Windows line's file stays as it is).
pub(crate) const MEM_EXECUTE: u32 = 0x2000_0000;
