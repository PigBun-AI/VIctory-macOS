//! Swap chain test under Wine.
//!
//! Makes a window and a flip-model swap chain on a direct queue, then for each
//! frame fills the current back buffer with one colour (an upload buffer
//! copied in with `CopyTextureRegion`), reads the back buffer back through a
//! readback buffer (what the application can see), and presents. Half way the
//! window grows and `ResizeBuffers(0, 0, 0, ...)` follows it.
//!
//! Output, one line each:
//!
//! * `EXPECT frame N texel 0xAARRGGBB size WxH` before each present: what the
//!   drawable must hold (`VICTORY_METAL_PROBE_PRESENT=1` makes our front print
//!   `victory-metal present: frame N ... texel ...` on stderr, which
//!   `tools/metal/test-present.sh` compares to these lines);
//! * `PRESENT PASS name` / `PRESENT FAIL name: why`;
//! * `PRESENT SUMMARY pass=… fail=…`.
//!
//! Options: `--frames N` (per phase, default 6), `--bgra` (B8G8R8A8 buffers),
//! `--hold SECONDS` (after the frames keep presenting one colour, for a
//! screenshot), `--waitable` (frame-latency waitable object).

#[cfg(not(windows))]
fn main() {
    eprintln!("front-present runs on Windows targets (under Wine) only");
}

#[cfg(windows)]
fn main() {
    std::process::exit(present::run());
}

#[cfg(windows)]
mod present;
