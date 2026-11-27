//! The world-frame clock (docs/mac/PATCHES.md, patch I1): a frame count that
//! does not depend on how long the menus and loading screens ran, so that
//! input scripts (`victory_hle`'s `VICTORY_INPUT_RECORD` / `VICTORY_INPUT_REPLAY`)
//! and frame captures can name the same frame in every run, on the D3DMetal
//! host path and on our Metal backend alike.
//!
//! `world_frame()` is 0 until the title presents the first frame in which it
//! recorded [`threshold`] draws or more since the previous present (500 by
//! default, `VICTORY_WORLD_DRAWS=<n>` changes it): that present is world frame
//! 1, and every later present adds 1.
//!
//! Draws are counted where the title records them (`list::count_draw`), not
//! where the host runs them. The host side lags a present when submissions go
//! ahead, drops draws it cannot translate yet, and differs between hosts;
//! the title's own count does not, so both backends latch on the same frame.
//!
//! Always counting, never acting: nothing here changes what the title or the
//! host sees; the count is read by the consumers named above.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::LazyLock;

/// Draws recorded by the title so far (every kind).
static DRAWS: AtomicU64 = AtomicU64::new(0);
/// `DRAWS` at the previous present.
static AT_LAST_PRESENT: AtomicU64 = AtomicU64::new(0);
static CLOCK: Clock = Clock::new();
/// Whether a consumer asked for the latch to be logged.
static LOG: AtomicBool = AtomicBool::new(false);

/// The latch and the count (a type so that tests own theirs: the process-wide
/// one is driven by whatever presents in the same test binary).
pub(crate) struct Clock {
    world_frame: AtomicU64,
    /// The title's present number (`queue::FRAMES`) of world frame 1.
    first_present: AtomicU64,
}

impl Clock {
    pub(crate) const fn new() -> Self {
        Self { world_frame: AtomicU64::new(0), first_present: AtomicU64::new(0) }
    }

    /// One present: `present` is its number, `draws` those since the last one.
    /// Returns whether this present is world frame 1.
    pub(crate) fn advance(&self, present: u64, draws: u64, threshold: u64) -> bool {
        let now = self.world_frame.load(Relaxed);
        if now > 0 {
            self.world_frame.store(now + 1, Relaxed);
            false
        } else if draws >= threshold {
            self.first_present.store(present, Relaxed);
            self.world_frame.store(1, Relaxed);
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    pub(crate) fn world_frame(&self) -> u64 {
        self.world_frame.load(Relaxed)
    }
}

/// Draws that make a presented frame count as the 3D world.
const DEFAULT_THRESHOLD: u64 = 500;

/// The draws a frame needs to be the world's first (`VICTORY_WORLD_DRAWS`).
pub fn threshold() -> u64 {
    static THRESHOLD: LazyLock<u64> = LazyLock::new(|| {
        std::env::var("VICTORY_WORLD_DRAWS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_THRESHOLD)
    });
    *THRESHOLD
}

/// `0` on menus and loading screens, `1` on the first world frame, then 1
/// more per presented frame.
pub fn world_frame() -> u64 {
    CLOCK.world_frame.load(Relaxed)
}

/// The title's present number at which [`world_frame`] became 1, if it has
/// (the number the frame log and `VICTORY_CAPTURE_FRAMES` use).
pub fn first_present() -> Option<u64> {
    Some(CLOCK.first_present.load(Relaxed)).filter(|&n| n != 0)
}

/// Whether world frame `1 + n` has been reached: the hook the recorder
/// asks at every present for `VICTORY_RECORD_AT=w<n>` (patch I2,
/// [`super::frames`]).
pub fn reached(n: u64) -> bool {
    super::frames::reached(world_frame(), n)
}

/// Asks for a log line when the world starts (set by the consumers, so a
/// run that uses none of them logs nothing new).
pub fn log_latch() {
    LOG.store(true, Relaxed);
}

/// The title recorded a draw (any kind): called from `list::count_draw`.
pub(crate) fn draw() {
    super::render_size::draw();
    DRAWS.fetch_add(1, Relaxed);
}

/// The title presents; `present` is its number of presents so far, this one
/// included: called from `queue::present`.
pub(crate) fn on_present(present: u64) {
    let draws = DRAWS.load(Relaxed);
    let since = draws.saturating_sub(AT_LAST_PRESENT.swap(draws, Relaxed));
    let latched = CLOCK.advance(present, since, threshold());
    super::render_size::on_present(world_frame());
    if latched && LOG.load(Relaxed) {
        tracing::info!(
            present,
            draws = since,
            "world frame 1 (input scripts and world-indexed captures count from here)"
        );
    }
}

/// BLAKE3 digest (hex) of a script's bytes, to tie logs and timings to the
/// exact input that drove them.
pub fn digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latches_on_the_first_frame_of_enough_draws_then_counts_presents() {
        let clock = Clock::new();
        let frame = || clock.world_frame.load(Relaxed);
        // Menus and loading: few draws per present, however many presents.
        for present in 1..=50 {
            assert!(!clock.advance(present, 499, 500));
            assert_eq!(frame(), 0);
        }
        // The first frame with 500 draws is world frame 1, whatever its
        // present number.
        assert!(clock.advance(51, 500, 500));
        assert_eq!(frame(), 1);
        assert_eq!(clock.first_present.load(Relaxed), 51);
        // Afterwards every present counts, even one with no draws.
        assert!(!clock.advance(52, 0, 500));
        assert!(!clock.advance(53, 3000, 500));
        assert_eq!(frame(), 3);
        assert_eq!(clock.first_present.load(Relaxed), 51);
    }

    #[test]
    fn digest_is_blake3() {
        assert_eq!(digest(b""), "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262");
    }
}
