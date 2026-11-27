//! Metal side of the backend (layer L3). Only this crate may name Metal,
//! objc2 or QuartzCore types.
//!
//! M0 holds the bring-up subset used by the probe: a device, a queue,
//! no-copy wrapping of caller memory, three tiny compute kernels, empty
//! command buffers on the classic queue and on the Metal 4 queue, a
//! residency set, and a clear-and-present on a caller-supplied
//! `CAMetalLayer`. The proto interpreter comes with M1.

#[cfg(target_vendor = "apple")]
pub mod m0;
