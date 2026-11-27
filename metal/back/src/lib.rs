//! Metal side of the backend (layer L3). Only this crate may name Metal,
//! objc2 or QuartzCore types.
//!
//! Two parts:
//!
//! * [`m0`]: the M0 bring-up subset used by the probe and the bridge (it
//!   keeps a classic queue for control measurements; it is the only file
//!   that may name Metal 3 command objects).
//! * the M1 executor core, Metal 4 only (architecture note 2.6, rules 6
//!   and 11): [`device`], [`memory`], [`residency`], [`descriptors`],
//!   [`record`], [`queue`], [`pipeline`], [`typed`], [`render`] (graphics and
//!   mesh pipelines), with the pure pieces [`stages`], [`pages`], [`barrier`].
//!
//! The API of those modules is plain Rust: no wire protocol types.
//! [`exec`] is the proto interpreter: the only module that names
//! `victory-metal-proto`; it maps proto's handles (`ResId` and friends) to
//! the objects made by the modules above and runs sync calls and command
//! streams on them.

#[cfg(target_vendor = "apple")]
pub mod archive;
#[cfg(target_vendor = "apple")]
pub mod barrier;
#[cfg(target_vendor = "apple")]
pub mod bitcode;
#[cfg(target_vendor = "apple")]
pub mod clear;
#[cfg(target_vendor = "apple")]
pub mod copy;
#[cfg(target_vendor = "apple")]
pub mod descriptors;
#[cfg(target_vendor = "apple")]
pub mod device;
#[cfg(target_vendor = "apple")]
pub mod draw;
pub mod dump;
#[cfg(target_vendor = "apple")]
pub mod dropstats;
#[cfg(target_vendor = "apple")]
pub mod error;
#[cfg(target_vendor = "apple")]
pub mod exec;
#[cfg(target_vendor = "apple")]
pub mod expand;
#[cfg(target_vendor = "apple")]
pub mod format;
#[cfg(target_vendor = "apple")]
pub mod heap;
#[cfg(target_vendor = "apple")]
pub mod indirect;
#[cfg(target_vendor = "apple")]
pub mod m0;
#[cfg(target_vendor = "apple")]
pub mod memory;
#[cfg(target_vendor = "apple")]
pub mod mesh_size;
#[cfg(target_vendor = "apple")]
pub mod mirror;
#[cfg(target_vendor = "apple")]
pub mod msc;
#[cfg(target_vendor = "apple")]
pub mod pages;
#[cfg(target_vendor = "apple")]
pub mod pipeline;
#[cfg(target_vendor = "apple")]
pub mod pipeline_defaults;
#[cfg(target_vendor = "apple")]
pub mod present;
#[cfg(target_vendor = "apple")]
pub mod queue;
#[cfg(target_vendor = "apple")]
pub mod record;
#[cfg(target_vendor = "apple")]
pub mod render;
#[cfg(target_vendor = "apple")]
pub mod residency;
#[cfg(target_vendor = "apple")]
pub mod sampler;
#[cfg(target_vendor = "apple")]
pub mod stages;
#[cfg(target_vendor = "apple")]
pub mod texture;
#[cfg(target_vendor = "apple")]
pub mod typed;

#[cfg(target_vendor = "apple")]
pub use error::{Error, Result};
