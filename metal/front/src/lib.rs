//! D3D12 / DXGI front end of the Metal backend.
//!
//! Two halves:
//!
//! * the **core** (everything but `shell`): D3D12 semantics in pure Rust,
//!   no `windows` crate, so `cargo test` runs natively on macOS. Resource
//!   descriptions and `GetCopyableFootprints`, root-signature
//!   serialization (byte-identical to D3DMetal's), resource-state tracking
//!   and the state → stage-mask table, descriptor handle arithmetic,
//!   `CheckFeatureSupport` answers. Command lists are encoded straight into
//!   `victory-metal-proto` blocks and sync calls go through its `Link`;
//! * the **shell** (`cfg(windows)`): COM objects (`#[implement]`) that only
//!   do ABI, reference counting and `QueryInterface`, and call the core.
//!
//! Versions (checked 2026-11-27): `windows` / `windows-core` 0.62.2.

pub mod adapter;
pub mod caps;
pub mod connect;
pub mod convert;
pub mod descriptor;
pub mod device;
pub mod fence_wait;
pub mod footprint;
pub mod format;
pub mod gpu;
pub mod hresult;
pub mod indirect;
pub mod library;
pub mod mock;
pub mod pso;
pub mod resource;
pub mod rootsig;
pub mod state;
pub mod swapchain;
pub mod view;

#[cfg(windows)]
pub mod shell;
