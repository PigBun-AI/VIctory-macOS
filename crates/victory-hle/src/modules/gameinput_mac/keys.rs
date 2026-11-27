//! `VICTORY_KEYS`: the scripted keys of the Windows line (`<seconds>:<key>` and
//! `<from>-<to>:<key>`), extended with repeats and holds as VIctory-macOS has
//! them (its #392): `<from>-<to>/<every>[+<hold>]:<key>` presses the key again
//! every `<every>` seconds, each press `<hold>` long (0.2 s by default), and
//! times may use scientific notation. Every entry the Windows line's parser
//! accepts parses to the same schedule here (`legacy_entries_parse_unchanged`).
//!
//! Times are seconds of wall clock since the first poll of input, as before:
//! `VICTORY_KEYS` is not indexed by world frame (that is what
//! `VICTORY_INPUT_REPLAY` is for).

// Only the Windows host reads `VICTORY_KEYS`; natively the parser is for tests.
#![cfg_attr(not(any(test, windows)), allow(dead_code))]

#[cfg(windows)]
use std::sync::LazyLock;
use std::time::Duration;
#[cfg(windows)]
use std::time::Instant;

/// How long each scripted press is held by default.
pub(super) const PRESS: Duration = Duration::from_millis(200);

/// One scripted key: held from `from` until `to`; with `every`, pressed
/// again each `every` (each press lasts `hold`).
#[derive(Debug, PartialEq)]
pub(super) struct Scripted {
    from: Duration,
    to: Duration,
    every: Option<Duration>,
    /// How long each repeated press is held (`+<hold>`; [`PRESS`] by default).
    hold: Duration,
    vk: u8,
}

/// The virtual key a script names: a letter, a digit, or `Enter`, `Esc`,
/// `Tab`, `Back`, `Up`, `Down`, `Left`, `Right`.
pub(super) fn key_named(name: &str) -> Option<u8> {
    Some(match name.trim() {
        "Enter" => 0x0D,
        "Esc" => 0x1B,
        "Tab" => 0x09,
        "Back" => 0x08,
        "Up" => 0x26,
        "Down" => 0x28,
        "Left" => 0x25,
        "Right" => 0x27,
        k if k.len() == 1 && k.as_bytes()[0].is_ascii_alphanumeric() => {
            k.as_bytes()[0].to_ascii_uppercase()
        }
        _ => return None,
    })
}

/// One complete number at the start of `s`, and the rest of `s`: digits with
/// an optional decimal point and an optional exponent (`[eE][+-]?` plus
/// digits), so scientific notation (`1e-3`, `1E+3`) is one number and its
/// `-`/`+` is part of the number, not a range or hold separator. The number
/// must run up to one of `delims` or to the end of `s` — trailing characters
/// (`5x`, `1e`, `2extra`) invalidate the entry instead of leaving a shorter
/// number — and must convert to a finite, non-negative [`Duration`] (`1e309`
/// overflows; `NaN`, `inf`, `-1` never scan as a number at all).
fn take_time<'a>(s: &'a str, delims: &str) -> Option<(Duration, &'a str)> {
    let s = s.trim_start();
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && b[i] == b'+' {
        i += 1;
    }
    let mut digits = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let exponent = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > exponent {
            i = j; // an exponent with digits belongs to the number
        }
    }
    if digits == 0 {
        return None;
    }
    let rest = s[i..].trim_start();
    if !rest.is_empty() && !delims.as_bytes().contains(&rest.as_bytes()[0]) {
        return None;
    }
    let secs = s[..i].parse::<f64>().ok()?;
    if !secs.is_finite() || secs < 0.0 {
        return None;
    }
    Some((Duration::try_from_secs_f64(secs).ok()?, rest))
}

/// The entries of a `VICTORY_KEYS` list; one that does not parse is left
/// out. An entry is `<span>[/<every>[+<hold>]]`, then `:`, then a key name:
/// `<span>` is `<from>-<to>` (bounds; empty parts default to the start, 0,
/// or no end), and `/<every>[+<hold>]` presses the key again each `<every>`
/// seconds, each press `<hold>` long (0.2 s by default). A plain
/// `<seconds>:<key>` is one 0.2 s press at that time.
pub(super) fn parse_script(list: &str) -> Vec<Scripted> {
    list.split(',')
        .filter_map(|entry| {
            let (at, key) = entry.split_once(':')?;
            let vk = key_named(key)?;
            // `<from>-<to>/<every>[+<hold>]`: the `/<every>` part is optional,
            // and `+<hold>` (shorter than `<every>`) lengthens each press,
            // e.g. for prompts that skip only while the key is held.
            let (span, every, hold) = match at.split_once('/') {
                Some((span, rest)) => {
                    let (every, rest) = take_time(rest, "+")?;
                    let hold = match rest.strip_prefix('+') {
                        Some(hold) => take_time(hold, "")?.0,
                        None => PRESS,
                    };
                    if hold.is_zero() || every <= hold {
                        return None;
                    }
                    (span, Some(every), hold)
                }
                None => (at, None, PRESS),
            };
            let (from, rest) = if span.trim().is_empty() {
                every?;
                (Duration::ZERO, span)
            } else {
                take_time(span, "-")?
            };
            let to = match rest.trim_start().strip_prefix('-') {
                Some(to) => take_time(to, "")?.0,
                None => match every {
                    // `<seconds>:<key>`: one press, held the default time.
                    None => from.checked_add(PRESS)?,
                    // A repeat with no end bound runs for as long as input does.
                    Some(_) => Duration::MAX,
                },
            };
            Some(Scripted {
                from,
                to,
                every,
                hold,
                vk,
            })
        })
        .collect()
}

/// The keys of `script` held `now` after input started.
pub(super) fn held_at(script: &[Scripted], now: Duration) -> Vec<u8> {
    script
        .iter()
        .filter(|k| {
            if !(k.from..k.to).contains(&now) {
                return false;
            }
            match k.every {
                // Held for the whole span, or pressed again each period.
                None => true,
                Some(every) => (now - k.from).as_nanos() % every.as_nanos() < k.hold.as_nanos(),
            }
        })
        .map(|k| k.vk)
        .collect()
}

/// The keys of `VICTORY_KEYS` held now (seconds since the first call).
/// Called by `gameinput`'s host input in place of its own parser.
#[cfg(windows)]
pub(in crate::modules::gameinput) fn scripted() -> Vec<u8> {
    static SCRIPT: LazyLock<(Instant, Vec<Scripted>)> = LazyLock::new(|| {
        let list = std::env::var("VICTORY_KEYS").unwrap_or_default();
        (Instant::now(), parse_script(&list))
    });
    let (start, keys) = &*SCRIPT;
    held_at(keys, start.elapsed())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(t: f64) -> Duration {
        Duration::from_secs_f64(t)
    }

    #[test]
    fn key_names() {
        assert_eq!(key_named("Enter"), Some(0x0D));
        assert_eq!(key_named(" Esc "), Some(0x1B));
        assert_eq!(key_named(" esc "), None); // names are case-sensitive
        assert_eq!(key_named("x"), Some(b'X'));
        assert_eq!(key_named("7"), Some(b'7'));
        assert_eq!(key_named("Ent"), None);
        assert_eq!(key_named(""), None);
        assert_eq!(key_named("Enter2"), None);
    }

    #[test]
    fn scientific_notation_is_one_number() {
        // `1e-3`/`1E-3` must not split into a `1e`-to-`3` range (which would
        // fail to parse and drop the key), and `1e+2` is 100, not `1e` + `2`.
        let script = parse_script("1e-3:Enter,1E-3:Esc,1e+2:Tab,2.5e-1-3:X");
        assert_eq!(script.len(), 4);
        assert_eq!(
            script[0],
            Scripted {
                from: s(0.001),
                to: s(0.201),
                every: None,
                hold: PRESS,
                vk: 0x0D
            }
        );
        assert_eq!(script[1].from, s(0.001));
        assert_eq!(script[2].from, s(100.0));
        assert_eq!(script[3].from, s(0.25));
        assert_eq!(script[3].to, s(3.0));
        assert_eq!(held_at(&script, s(0.0015)), [0x0D, 0x1B]);
        assert_eq!(held_at(&script, s(0.0005)), Vec::<u8>::new());
        assert_eq!(held_at(&script, s(0.3)), [b'X']);
        assert_eq!(held_at(&script, s(100.05)), [0x09]);
    }

    #[test]
    fn plain_presses_hold_200ms() {
        let script = parse_script("5:Enter,bad,1/x:Up");
        assert_eq!(script.len(), 1);
        assert_eq!(held_at(&script, s(4.9)), Vec::<u8>::new());
        assert_eq!(held_at(&script, s(5.0)), [0x0D]);
        assert_eq!(held_at(&script, s(5.1)), [0x0D]);
        assert_eq!(held_at(&script, s(5.2)), Vec::<u8>::new());
    }

    #[test]
    fn ranges_hold_the_whole_span() {
        let script = parse_script("2-10:Down,1e-3-2:Enter");
        assert_eq!(script.len(), 2);
        assert_eq!(held_at(&script, s(1.9)), [0x0D]);
        assert_eq!(held_at(&script, s(1.999)), [0x0D]);
        assert_eq!(held_at(&script, s(2.0)), [0x28]);
        assert_eq!(held_at(&script, s(9.999)), [0x28]);
        assert_eq!(held_at(&script, s(10.0)), Vec::<u8>::new());
    }

    #[test]
    fn repeats_press_each_period() {
        let script = parse_script("2-10/1:Down");
        assert_eq!(script.len(), 1);
        // Pressed at 2.0, 3.0, ... for 0.2 s each.
        assert_eq!(held_at(&script, s(1.95)), Vec::<u8>::new());
        assert_eq!(held_at(&script, s(2.1)), [0x28]);
        assert_eq!(held_at(&script, s(2.5)), Vec::<u8>::new());
        assert_eq!(held_at(&script, s(3.0)), [0x28]);
        assert_eq!(held_at(&script, s(9.05)), [0x28]);
        assert_eq!(held_at(&script, s(10.05)), Vec::<u8>::new());
    }

    #[test]
    fn unbounded_repeat_from_start_or_a_bound() {
        let script = parse_script("/0.5:Up,7.3/1:W");
        assert_eq!(script.len(), 2);
        // `/<every>:<key>` repeats from the start, with no end bound.
        assert_eq!(held_at(&script, s(0.1)), [0x26]);
        assert_eq!(held_at(&script, s(0.3)), Vec::<u8>::new());
        assert_eq!(held_at(&script, s(100.6)), [0x26]);
        // A lone start with a period repeats from there, unbounded.
        assert_eq!(held_at(&script, s(7.35)), [b'W']);
        assert_eq!(held_at(&script, s(7.6)), [0x26]);
        assert_eq!(held_at(&script, s(1000.35)), [b'W']);
    }

    #[test]
    fn held_repeats_and_refused_periods() {
        // A hold as long as the period (or zero) would merge the presses
        // into one, so those entries are refused.
        let script = parse_script("2-20/3+2.5:Enter,2-20/1+1:A,2-20/1+0:A,2-20/0:B");
        assert_eq!(script.len(), 1);
        assert_eq!(held_at(&script, s(2.0)), [0x0D]);
        assert_eq!(held_at(&script, s(4.4)), [0x0D]);
        assert_eq!(held_at(&script, s(4.5)), Vec::<u8>::new());
        assert_eq!(held_at(&script, s(4.6)), Vec::<u8>::new());
        assert_eq!(held_at(&script, s(5.1)), [0x0D]);
    }

    #[test]
    fn mixed_entries() {
        let script = parse_script("5:Enter, 2-10/1:Down,3-4:A,bad,7/0.1:Up,1/x:Up");
        assert_eq!(script.len(), 3);
        assert_eq!(held_at(&script, s(5.1)), [0x0D, 0x28]);
        assert_eq!(held_at(&script, s(5.3)), Vec::<u8>::new());
        assert_eq!(held_at(&script, s(3.5)), [b'A']);
        assert_eq!(held_at(&script, s(3.05)), [0x28, b'A']);
    }

    /// The Windows line's own parser, as it was before this patch.
    fn legacy(list: &str) -> Vec<(Duration, Duration, u8)> {
        list.split(',')
            .filter_map(|entry| {
                let (at, key) = entry.split_once(':')?;
                let vk = match key.trim() {
                    "Enter" => 0x0D,
                    "Esc" => 0x1B,
                    "Tab" => 0x09,
                    "Back" => 0x08,
                    "Up" => 0x26,
                    "Down" => 0x28,
                    "Left" => 0x25,
                    "Right" => 0x27,
                    k if k.len() == 1 && k.as_bytes()[0].is_ascii_alphanumeric() => {
                        k.as_bytes()[0].to_ascii_uppercase()
                    }
                    _ => return None,
                };
                let time = |t: &str| Duration::try_from_secs_f64(t.trim().parse().ok()?).ok();
                let (from, to) = match at.split_once('-') {
                    Some((from, to)) => (time(from)?, time(to)?),
                    None => (time(at)?, time(at)? + Duration::from_millis(200)),
                };
                Some((from, to, vk))
            })
            .collect()
    }

    /// Everything the Windows line accepted still parses, to the same
    /// schedule: same start, end and key, plain (no repeat) and held for the
    /// whole span.
    #[test]
    fn legacy_entries_parse_unchanged() {
        for list in [
            "5:Enter",
            "0:Enter,35:X,40:Esc",
            "8-900:Enter",
            "2-120:Down",
            " 3 - 4 : Tab ",
            "0.5:Back,1.25-2.5:Left,3e1:Right,1e2-2e2:A",
            "+5:Enter,5.:Up,.5:Down",
            "7:7,1-2:z",
            "bad,5:Enter,1-:A,-1:A,:Enter,2:Enterx,2:Ent",
            "NaN:A,inf:A,-0:A,1-NaN:A,9-1:Enter",
            "",
        ] {
            let old = legacy(list);
            let new = parse_script(list);
            // Whatever the old parser dropped may parse now (`1e-3`, `/1`,
            // ...): only what it kept must be there, unchanged.
            for (from, to, vk) in old {
                assert!(
                    new.iter().any(|k| k == &Scripted { from, to, every: None, hold: PRESS, vk }),
                    "{list:?}: {from:?}-{to:?} {vk:#x} lost or changed"
                );
                for t in [from, to, from + (to.saturating_sub(from)) / 2] {
                    let held_old = (from..to).contains(&t);
                    assert_eq!(held_at(&new, t).contains(&vk), held_old, "{list:?} at {t:?}");
                }
            }
        }
    }

    #[test]
    fn the_task_example_repeats_enter_every_three_seconds() {
        let script = parse_script("8-900/3:Enter");
        assert_eq!(held_at(&script, s(7.9)), Vec::<u8>::new());
        assert_eq!(held_at(&script, s(8.0)), [0x0D]);
        assert_eq!(held_at(&script, s(8.19)), [0x0D]);
        assert_eq!(held_at(&script, s(8.3)), Vec::<u8>::new());
        assert_eq!(held_at(&script, s(11.1)), [0x0D]);
        assert_eq!(held_at(&script, s(899.1)), [0x0D]);
        assert_eq!(held_at(&script, s(900.1)), Vec::<u8>::new());
    }

    #[test]
    fn invalid_times_schedule_nothing() {
        // No falling back to a shorter number, and no unconsumed trailing
        // characters: each of these entries is refused whole.
        for bad in [
            ":Enter",         // missing timestamp with no repeat
            " :Enter",        // whitespace-only timestamp with no repeat
            "+:Enter",        // bare leading plus
            "1e309:Enter",    // overflows Duration
            "5garbage:Enter", // trailing characters
            "1e:Enter",       // exponent without digits
            "NaN:Enter",
            "inf:Enter",
            "-1:Enter", // negative
            "1-2extra:Enter",
            "/1+0.2extra:Enter",
        ] {
            assert!(parse_script(bad).is_empty(), "{bad} parsed");
        }
        // Leading `+` and scientific notation still parse cleanly.
        let script = parse_script("1e-3:Enter,/1e+3+0.5:X,2.5e-1-3:A,+5:Enter");
        assert_eq!(script.len(), 4);
        assert_eq!(script[0].from, s(0.001));
        assert_eq!(script[0].to, s(0.201));
        assert_eq!(script[1].every, Some(s(1000.0)));
        assert_eq!(script[1].hold, s(0.5));
        assert_eq!(script[2].from, s(0.25));
        assert_eq!(script[2].to, s(3.0));
        assert_eq!(script[3].from, s(5.0));
        assert_eq!(script[3].to, s(5.2));
    }
}
