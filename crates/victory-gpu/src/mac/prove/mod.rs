//! Tests that prove nodes of the translation map (docs/mac/PATCHES.md, G12): table-driven, against the D3D12 / DXGI
//! specifications or addrlib, on the host device where a GPU is the reference. Nothing here is built outside `cargo test`.

#[cfg(all(test, windows))]
pub(crate) mod rig;
