//! Mac patches of victory-gpu (docs/mac/PATCHES.md, patches G1-G3). The
//! Windows exe also runs on a Mac, under Wine and Rosetta 2; what differs
//! there is decided at run time in [`rosetta`] and hooked into the Windows
//! line's files in as few places as possible.

pub mod adapter;
pub(crate) mod cbv; // mac G15
pub mod attribute_at_vertex; // mac S1
pub mod drop_stats;
pub mod frames;
pub mod hlsl;
pub mod indirect_wide; // mac: 2^20 groups on the Metal chain
pub mod layout_stats; // mac G11
pub mod mesh_compute; // mac S5
pub mod metal_chain; // mac S5, S7
pub mod present_cap;
pub mod prove; // mac G12
pub mod pso_x;
pub mod render_size;
pub mod rosetta;
pub mod world;
#[cfg(windows)]
pub(crate) mod safecopy;
#[cfg(windows)]
pub(crate) mod pso_grave;

#[cfg(not(windows))]
pub(crate) mod nonwindows;

/// Whether this is the Metal chain: `tools/mac/run.sh` exports `VICTORY_METAL_OUT` (the back's directory).
pub fn on_metal_chain() -> bool {
    std::env::var_os("VICTORY_METAL_OUT").is_some_and(|v| !v.is_empty())
}
