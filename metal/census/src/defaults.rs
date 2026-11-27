//! What the census tells the translator about the host: the numbers the
//! Metal backend would report (`victory-gpu` sets them from the host device:
//! `host.rs` wave lanes, `descriptor/host.rs` `probe_view_limit`).

/// Widest wave of an Apple GPU (SIMD width; Metal Shader Converter needs 32).
pub const HOST_WAVE: u32 = 32;
/// `RAW_TWINS` of `victory-gpu` (`descriptor/host.rs`): the host's view heaps
/// take 2^22 views, so the raw twins start at 2^21 and the null slot is the
/// last view of the first half.
pub const RAW_TWINS: u32 = 1 << 21;
/// The null slot view indices are clamped to (`views - 1`).
pub const VIEW_LIMIT: u32 = RAW_TWINS - 1;
