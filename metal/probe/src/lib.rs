//! M0 probe: the measurements of `docs/metal/m0-results.md`, written once
//! against `victory_metal_bridge::abi::Link`.

pub mod platform;
pub mod stats;
pub mod suite;

#[cfg(windows)]
pub mod layer;
