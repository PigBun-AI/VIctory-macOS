//! Mac patches of victory-gpu (docs/mac/PATCHES.md, patches G1-G3). The
//! Windows exe also runs on a Mac, under Wine and Rosetta 2; what differs
//! there is decided at run time in [`rosetta`] and hooked into the Windows
//! line's files in as few places as possible.

pub mod adapter;
pub mod frames;
pub mod hlsl;
pub mod rosetta;
pub mod world;
#[cfg(windows)]
pub(crate) mod safecopy;
#[cfg(windows)]
pub(crate) mod pso_grave;

#[cfg(not(windows))]
pub(crate) mod nonwindows;
