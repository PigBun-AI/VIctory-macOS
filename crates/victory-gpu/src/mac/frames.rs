//! Frames named by the world clock (docs/mac/PATCHES.md, patch I2): the
//! `w<N>` spelling of `VICTORY_RECORD_AT` and `VICTORY_CAPTURE_FRAMES`.
//!
//! `w<N>` is the N-th world frame *after* world frame 1 (`w0` is world frame
//! 1 itself, `w300` is world frame 301), so it names the same moment in
//! every run, where world frame 1 itself falls on a different present
//! number from run to run (788 in one, 1209 in the next). The clock is
//! [`super::world`]; this file only parses the spelling and answers "has the
//! target been reached" so the upstream recorder and capture code ask
//! instead of counting.
//!
//! Nothing here runs unless one of the two variables carries a `w<N>`.

/// A frame to act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The title's present number (`f<N>` for the recorder, a bare number in
    /// `VICTORY_CAPTURE_FRAMES`).
    Present(u64),
    /// `w<N>`: world frame `1 + N`, the N-th present after world frame 1.
    World(u64),
}

/// `w<N>` (or `W<N>`): the N in it. Nothing else matches: not `w`, not
/// `w-1`, not `w1.5`, not `wx`.
pub fn parse_world(text: &str) -> Option<u64> {
    let n = text.trim().strip_prefix(['w', 'W'])?;
    // `parse` takes a leading `+`; the spelling does not.
    n.bytes().all(|b| b.is_ascii_digit()).then(|| n.parse().ok()).flatten()
}

/// One item of `VICTORY_CAPTURE_FRAMES`: a present number, or `w<N>`.
pub fn parse_item(text: &str) -> Option<Target> {
    let text = text.trim();
    match parse_world(text) {
        Some(n) => Some(Target::World(n)),
        None => text.parse().ok().map(Target::Present),
    }
}

/// `VICTORY_CAPTURE_FRAMES`: comma-separated items; one that is neither is
/// skipped, as it always was.
pub fn parse_list(list: &str) -> Vec<Target> {
    list.split(',').filter_map(parse_item).collect()
}

/// Whether present number `present` is one of `targets`, given the present
/// number of world frame 1 (`None` while the world has not started: a `w<N>`
/// names nothing yet). Every present after world frame 1 is one world
/// frame, so `w<N>` is present `first + N`.
pub fn listed(targets: &[Target], present: u64, first_present: Option<u64>) -> bool {
    targets.iter().any(|t| match *t {
        Target::Present(n) => n == present,
        Target::World(n) => first_present.is_some_and(|first| first.checked_add(n) == Some(present)),
    })
}

/// Whether world frame `1 + n` has been reached, given the clock's count
/// (`world_frame()`: 0 before the world). The recorder's "is it time" for
/// `w<n>`.
pub fn reached(world_frame: u64, n: u64) -> bool {
    world_frame > n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mac::world::Clock;

    #[test]
    fn parses_the_world_spelling() {
        assert_eq!(parse_world("w300"), Some(300));
        assert_eq!(parse_world(" W5 "), Some(5));
        assert_eq!(parse_world("w0"), Some(0));
        for bad in ["w", "w-1", "w+1", "w1.5", "wx", "300", "f300", "", "ww3", "w 3"] {
            assert_eq!(parse_world(bad), None, "{bad:?}");
        }
        // Too large for a frame count: no match, not a panic.
        assert_eq!(parse_world("w99999999999999999999999"), None);
    }

    #[test]
    fn capture_list_takes_both_spellings() {
        assert_eq!(
            parse_list("300, w12,x,w,64"),
            vec![Target::Present(300), Target::World(12), Target::Present(64)]
        );
        assert_eq!(parse_list(""), Vec::new());
    }

    #[test]
    fn a_world_target_names_the_same_world_frame_in_every_run() {
        let targets = parse_list("100,w3");
        // A run whose world frame 1 is present 788 ...
        assert!(listed(&targets, 100, None));
        assert!(!listed(&targets, 791, None), "no world yet: w3 names nothing");
        assert!(listed(&targets, 791, Some(788)));
        assert!(!listed(&targets, 790, Some(788)));
        // ... and one whose is present 1209.
        assert!(listed(&targets, 1212, Some(1209)));
        assert!(!listed(&targets, 791, Some(1209)));
        // `w0` is world frame 1.
        assert!(listed(&[Target::World(0)], 788, Some(788)));
    }

    #[test]
    fn the_world_clock_reaching_the_target_triggers() {
        // The recorder asks `reached(world_frame, n)` at every present.
        let run = |first: u64| -> Option<u64> {
            let clock = Clock::new();
            for present in 1..2000 {
                // Few draws until `first`, then plenty (the threshold is 500).
                let draws = if present == first { 1000 } else { 10 };
                clock.advance(present, draws, 500);
                if reached(clock.world_frame(), 5) {
                    return Some(present);
                }
            }
            None
        };
        // w5 is world frame 6: five presents after world frame 1, wherever
        // the world starts.
        assert_eq!(run(788), Some(793));
        assert_eq!(run(1209), Some(1214));
        // Before the world nothing is reached, for any n.
        assert!(!reached(0, 0));
        assert!(reached(1, 0));
        assert!(!reached(5, 5));
        assert!(reached(6, 5));
    }
}
