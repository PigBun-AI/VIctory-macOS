//! The only place that knows about Wine (layer L4).
//!
//! * [`abi`]: the call codes and argument structs shared by both halves; no
//!   Wine, Metal or Windows types.
//! * `pe` (Windows target): finds the unixlib of the Wine-marked dll and
//!   calls through `__wine_unix_call_dispatcher`.
//! * `unix` (Apple targets): the `__wine_unix_call_funcs` table Wine looks
//!   up, and the in-process way to call the same entry points without Wine.

pub mod abi;

#[cfg(windows)]
pub mod pe;

#[cfg(target_vendor = "apple")]
pub mod unix;
