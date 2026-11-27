//! D3D12 / DXGI front end of the Metal backend.
//!
//! Two halves:
//!
//! * the **core** (everything but `shell`): D3D12 semantics in pure Rust,
//!   no `windows` crate, so `cargo test` runs natively on macOS. Resource
//!   descriptions and `GetCopyableFootprints`, root-signature
//!   serialization (byte-identical to D3DMetal's), resource-state tracking
//!   and the state → stage-mask table, descriptor handle arithmetic,
//!   `CheckFeatureSupport` answers, and the thin [`backend::Backend`] /
//!   [`backend::Recorder`] seam that stands in for the proto layer until it
//!   exists;
//! * the **shell** (`cfg(windows)`): COM objects (`#[implement]`) that only
//!   do ABI, reference counting and `QueryInterface`, and call the core.
//!
//! Versions (checked 2026-11-27): `windows` / `windows-core` 0.62.2.

pub mod adapter;
pub mod backend;
pub mod caps;
pub mod descriptor;
pub mod device;
pub mod fake;
pub mod footprint;
pub mod format;
pub mod hresult;
pub mod resource;
pub mod rootsig;
pub mod state;

#[cfg(windows)]
pub mod shell;
