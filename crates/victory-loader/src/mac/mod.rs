//! Mac patches of the loader (docs/mac/PATCHES.md, patches L1-L5), kept apart
//! from the Windows line's files: edits to patch the title's code that
//! exist only because the Windows exe runs on Wine + Rosetta 2 on a Mac.
//! Ported from VIctory-macOS (origin/main 7772921b); nothing here runs unless
//! `apps/victory/src/mac.rs` asks for it, and it asks only under Rosetta.
//!
//! - [`masked`]: AVX masked stores redone as plain stores (Rosetta cannot
//!   resume a fault in the middle of one).
//! - [`wide`]: 256-bit stores redone as two 128-bit ones, from a list made by
//!   `cargo xtask wide-stores`.
//! - [`resolution`]: the title's output size (1280x720 by default on a Mac).
//! - [`clwb`]: `clwb` write-back loops turned into NOPs (a fault per loop
//!   under Rosetta).
//! - [`watchdog`]: the title's hang watchdog turned off (L5; off by default,
//!   `VICTORY_MAC_NO_WATCHDOG=1`).

pub mod clwb;
pub mod masked;
pub mod resolution;
pub mod watchdog;
pub mod wide;

#[cfg(windows)]
pub use crate::threads::in_title_image;

/// `IMAGE_SECTION_HEADER::Characteristics` flag (VIctory-macOS has this in
/// victory-pe's layout.rs; kept here so the Windows line's file stays as it is).
pub(crate) const MEM_EXECUTE: u32 = 0x2000_0000;
