//! Which output chain a run takes: the title's own 4K (fidelity), native 1440p
//! (the perf profile's default), or the old 1280x720 output patch (L3, asked
//! for). Patch X13 in docs/mac/PATCHES.md.
//!
//! The two ways to a smaller output are **alternatives**, not layers:
//!
//! - *Native 1440p* (`apps/victory/src/native_1440p.rs`, written into the image
//!   before it is mapped): the title's SDR display mode record
//!   (+0x0615c100, the one place its 3840x2160 comes from), the scanout
//!   reservation (+0x0247ad4a, which also sizes the back buffer pool its
//!   left-over check wants used up), the temporal scene scale (+0x028f1e7e) and
//!   the Settings preview viewport (+0x058048d4). The title then makes a
//!   2560x1440 output and runs no 4K pass.
//! - *L3* ([`super::resolution`], written into the mapped image): the
//!   settings' size handed to the screen size (+0x224352b), the pool check
//!   jumped over (+0x247af25) and, for `VICTORY_RENDER_SCALE`, the scale's
//!   immediate (+0x28f1e78..+0x28f1e82).
//!
//! L3's output patch would write 720p over a title whose display mode record
//! and scanout buffers are 1440p, and its scale site shares four bytes with
//! the native one (+0x28f1e7e): with native 1440p on, the output, the pool
//! check that comes with the output and the scale go through the native chain
//! only ([`Decision::l3_output`], [`Decision::l3_scale`]); L3's upscaler
//! picker, an explicit pool check skip and the divide clamp are other sites
//! and stay available. `VICTORY_OUTPUT=720` (or `1280x720`) asks for L3, and
//! native 1440p is then off unless `VICTORY_NATIVE_1440P=1` asks for it as
//! well (it wins, and says so).
//!
//! The profile comes from `tools/mac/run.sh` (`VICTORY_MAC_PROFILE`, which
//! `run.sh` exports; before that it passed only `VICTORY_OUTPUT=0` for
//! fidelity): the fidelity profile keeps the title's own 4K output and turns
//! native 1440p off even when `VICTORY_NATIVE_1440P=1` / `VICTORY_RENDER_1080P=1`
//! are set, with one log line saying why.

use super::resolution::{Asked, asked_output_on, on_a_mac, parse_asked};
use std::sync::OnceLock;

/// The profile of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Series X, the title's own output (4K); nothing about the output is patched.
    Fidelity,
    /// Speed: a smaller output (native 1440p by default).
    Perf,
}

/// What a run's environment says (the text of each variable, `None` when unset).
#[derive(Debug, Clone, Copy, Default)]
pub struct Env<'a> {
    /// `VICTORY_MAC_PROFILE`: `fidelity` or `perf` (exported by `tools/mac/run.sh`).
    pub profile: Option<&'a str>,
    /// `VICTORY_OUTPUT`: a mode, or `0` / `none` (fidelity), or unset.
    pub output: Option<&'a str>,
    /// `VICTORY_NATIVE_1440P`: `1` on, `0` off, unset: the profile decides.
    pub native_1440p: Option<&'a str>,
    /// `VICTORY_RENDER_1080P=1`: a 1920x1080 scene scaled to 1440p.
    pub render_1080p: Option<&'a str>,
    /// `VICTORY_RENDER_SCALE`.
    pub render_scale: Option<&'a str>,
    /// The host is a Mac (Wine on Darwin).
    pub mac: bool,
    /// The console is a Series X (native 1440p is only known for its image).
    pub series_x: bool,
}

impl Env<'static> {
    /// The environment of this process, leaked into `'static` strings (once).
    fn from_process(series_x: bool) -> Env<'static> {
        fn get(name: &str) -> Option<&'static str> {
            std::env::var(name).ok().map(|v| &*Box::leak(v.into_boxed_str()))
        }
        Env {
            profile: get("VICTORY_MAC_PROFILE"),
            output: get("VICTORY_OUTPUT"),
            native_1440p: get("VICTORY_NATIVE_1440P"),
            render_1080p: get("VICTORY_RENDER_1080P"),
            render_scale: get("VICTORY_RENDER_SCALE"),
            mac: on_a_mac(),
            series_x,
        }
    }
}

/// What the run does about the output.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    /// `None`: not told (a PC, or a Mac outside `run.sh` with nothing to infer it from).
    pub profile: Option<Profile>,
    /// The image is patched for a 2560x1440 output (`native_1440p::prepare`).
    pub native_1440p: bool,
    /// What L3's output patch is asked: with native 1440p on, never a size.
    pub l3_output: Asked,
    /// Whether L3's scale site (`VICTORY_RENDER_SCALE`) is used: not with native
    /// 1440p, whose chain owns the scale (and shares four bytes with that site).
    pub l3_scale: bool,
    /// The lines to print once, in order.
    pub log: Vec<String>,
}

/// `1` / `true` / `on` and `0` / `false` / `off`; anything else is not a switch.
fn switch(text: Option<&str>) -> Option<bool> {
    match text?.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
}

/// The profile: `VICTORY_MAC_PROFILE` first, then what `VICTORY_OUTPUT` says
/// (`0` / `none`: the title's own size = fidelity; a size = perf), then the
/// host (a Mac is perf, as L3's 720p default always was; a PC: not told).
pub fn profile_of(env: &Env) -> Option<Profile> {
    match env.profile.map(|p| p.trim().to_ascii_lowercase()).as_deref() {
        Some("fidelity") => return Some(Profile::Fidelity),
        Some("perf") => return Some(Profile::Perf),
        _ => {}
    }
    match env.output.map(|o| o.trim()) {
        Some("0") => return Some(Profile::Fidelity),
        Some(o) if o.eq_ignore_ascii_case("none") => return Some(Profile::Fidelity),
        Some(_) => return Some(Profile::Perf),
        None => {}
    }
    env.mac.then_some(Profile::Perf)
}

/// `VICTORY_OUTPUT` read with the short forms `720`, `720p`, ... of the title's modes.
fn parse_output_text(text: &str) -> Result<Asked, String> {
    let t = text.trim().to_ascii_lowercase();
    let height = t.strip_suffix('p').unwrap_or(&t);
    let by_height = super::resolution::MODES
        .iter()
        .find(|(_, h)| h.to_string() == height && height.len() >= 3);
    match by_height {
        Some(&size) => Ok(Asked::Size(size)),
        None => parse_asked(text),
    }
}

/// Decides the chain. Pure: both profiles, the switches and their precedence are
/// tested without an image or an environment.
///
/// Precedence for native 1440p, strongest first: the fidelity profile (always
/// off); an explicit `VICTORY_NATIVE_1440P=0|1`; an explicit `VICTORY_OUTPUT`
/// size (the L3 chain); `VICTORY_RENDER_1080P=1` or a readable
/// `VICTORY_RENDER_SCALE` (a scale of the 1440p output, as on the Windows line);
/// the profile's default (perf: on, on a Series X).
pub fn decide(env: &Env) -> Decision {
    let profile = profile_of(env);
    let native_asked = switch(env.native_1440p);
    let r1080 = switch(env.render_1080p) == Some(true);
    let scale_given = env
        .render_scale
        .is_some_and(|s| crate::render_scale::parse_scale(s).is_some());
    let output_text = env.output.map(|o| o.trim()).filter(|o| !o.is_empty());
    let output_parsed = output_text.map(parse_output_text);
    let explicit_size = matches!(output_parsed, Some(Ok(Asked::Size(_))));
    let mut log = Vec::new();

    // The L3 output as it asks for itself (the Mac default, the title's own
    // size elsewhere), with the short forms and the profile read in.
    let l3_own = match &output_parsed {
        Some(Ok(asked)) => *asked,
        Some(Err(why)) => {
            log.push(format!("VICTORY_OUTPUT={}: {why}", output_text.unwrap_or_default()));
            asked_output_on(None, env.mac)
        }
        None => asked_output_on(None, env.mac),
    };

    if profile == Some(Profile::Fidelity) {
        // Enforce stock output here too: the launcher is not the only entry point.
        if native_asked == Some(true) || r1080 {
            let which: Vec<&str> = [
                (native_asked == Some(true)).then_some("VICTORY_NATIVE_1440P=1"),
                r1080.then_some("VICTORY_RENDER_1080P=1"),
            ]
            .into_iter()
            .flatten()
            .collect();
            log.push(format!(
                "output chain: native 1440p stays off: the fidelity profile keeps the title's own 4K output ({} ignored)",
                which.join(", ")
            ));
        } else {
            log.push("output chain: fidelity: the title's own 4K output, nothing about the output patched".to_owned());
        }
        return Decision { profile, native_1440p: false, l3_output: Asked::Title, l3_scale: false, log };
    }

    // Not fidelity: perf, or not told.
    let profile_default = profile == Some(Profile::Perf);
    let (mut native, why) = match native_asked {
        Some(on) => (on, "VICTORY_NATIVE_1440P"),
        None if explicit_size => (false, "VICTORY_OUTPUT names a size (the L3 chain)"),
        None if r1080 || scale_given => (true, "a render scale of the 1440p output was asked"),
        None if profile_default => (true, "the perf profile's default"),
        None => (false, "off unless asked"),
    };
    if native && !env.series_x {
        // Only the Series X image is known. Asked outright it is an error for run.rs to
        // raise; as a default or implied by a scale it is a plain "not here".
        if native_asked == Some(true) {
            log.push("output chain: native 1440p asked for on a console that is not a Series X: the run refuses".to_owned());
        } else {
            native = false;
            log.push("output chain: native 1440p is for the Series X image only: off for this console".to_owned());
        }
    }
    if !native {
        let l3 = match l3_own {
            Asked::Size((w, h)) => format!("L3 output {w}x{h}"),
            Asked::Title => "the title's own output".to_owned(),
        };
        log.push(format!("output chain: native 1440p off ({why}); {l3}"));
        return Decision { profile, native_1440p: false, l3_output: l3_own, l3_scale: true, log };
    }

    let mut line = format!("output chain: native 1440p on ({why})");
    if let Some(Asked::Size((w, h))) = Some(l3_own).filter(|_| explicit_size) {
        line.push_str(&format!("; VICTORY_OUTPUT={w}x{h} (L3) not used: its output, pool check and scale sites stay unpatched"));
    } else if env.mac && output_text.is_none() {
        line.push_str("; the 1280x720 default of L3 is not used");
    }
    log.push(line);
    Decision { profile, native_1440p: true, l3_output: Asked::Title, l3_scale: false, log }
}

static CURRENT: OnceLock<Decision> = OnceLock::new();

/// Decides from this process's environment once and keeps it: `run.rs` calls it
/// first (with the console) and prints the lines; `resolution::apply_asked` asks
/// for the same decision after the image is mapped.
pub fn decide_once(series_x: bool) -> &'static Decision {
    CURRENT.get_or_init(|| decide(&Env::from_process(series_x)))
}

/// The decision already made, or one made now from the environment on a Series X.
pub fn current() -> &'static Decision {
    decide_once(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(profile: Option<&'static str>) -> Env<'static> {
        Env { profile, mac: true, series_x: true, ..Env::default() }
    }

    // ---- 1. perf default -------------------------------------------------------------

    /// perf with nothing set: native 1440p on, L3's 720p not asked, L3's scale site unused.
    #[test]
    fn perf_defaults_to_native_1440p() {
        let d = decide(&env(Some("perf")));
        assert_eq!(d.profile, Some(Profile::Perf));
        assert!(d.native_1440p);
        assert_eq!(d.l3_output, Asked::Title, "the 720p output patch is not asked with native 1440p");
        assert!(!d.l3_scale);
        assert!(d.log[0].contains("the perf profile's default"), "{:?}", d.log);
    }

    /// A Mac outside `run.sh` (no profile exported, no VICTORY_OUTPUT) is perf too, as L3's 720p default always made it.
    #[test]
    fn a_mac_without_run_sh_is_perf() {
        let d = decide(&env(None));
        assert_eq!(d.profile, Some(Profile::Perf));
        assert!(d.native_1440p);
    }

    /// A PC outside `run.sh`: not told, so nothing is on unless asked (the Windows line's behaviour).
    #[test]
    fn a_pc_without_a_profile_is_off_unless_asked() {
        let pc = Env { mac: false, series_x: true, ..Env::default() };
        let d = decide(&pc);
        assert_eq!((d.profile, d.native_1440p), (None, false));
        assert_eq!(d.l3_output, Asked::Title);
        assert!(decide(&Env { native_1440p: Some("1"), ..pc }).native_1440p);
        assert!(decide(&Env { render_1080p: Some("1"), ..pc }).native_1440p);
        assert!(decide(&Env { render_scale: Some("0.75"), ..pc }).native_1440p);
        assert!(!decide(&Env { render_scale: Some("2"), ..pc }).native_1440p, "unreadable: not asked");
    }

    /// The profile `run.sh` does not export yet is read from `VICTORY_OUTPUT`: `0` / `none` fidelity, a size perf.
    #[test]
    fn the_profile_is_read_from_the_output_when_not_exported() {
        assert_eq!(profile_of(&Env { output: Some("0"), ..env(None) }), Some(Profile::Fidelity));
        assert_eq!(profile_of(&Env { output: Some("none"), ..env(None) }), Some(Profile::Fidelity));
        assert_eq!(profile_of(&Env { output: Some("1280x720"), ..env(None) }), Some(Profile::Perf));
        // The exported profile wins over a guess.
        assert_eq!(profile_of(&Env { output: Some("0"), ..env(Some("perf")) }), Some(Profile::Perf));
        assert_eq!(profile_of(&Env { output: Some("720"), ..env(Some("fidelity")) }), Some(Profile::Fidelity));
        // Something that is not a profile is not one.
        assert_eq!(profile_of(&env(Some("turbo"))), Some(Profile::Perf), "falls through to the Mac default");
    }

    // ---- 2. fidelity forced off ------------------------------------------------------

    /// fidelity: off, whatever is set, and the log says why when something asked for it.
    #[test]
    fn fidelity_forces_native_1440p_off_and_says_why() {
        for (native, r1080) in [(Some("1"), None), (None, Some("1")), (Some("1"), Some("1"))] {
            let d = decide(&Env { native_1440p: native, render_1080p: r1080, output: Some("0"), ..env(Some("fidelity")) });
            assert_eq!(d.profile, Some(Profile::Fidelity));
            assert!(!d.native_1440p, "{native:?} {r1080:?}");
            assert_eq!(d.l3_output, Asked::Title);
            assert_eq!(d.log.len(), 1);
            assert!(d.log[0].contains("fidelity profile keeps the title's own 4K output"), "{:?}", d.log);
            assert!(d.log[0].contains("ignored"), "{:?}", d.log);
        }
        let d = decide(&Env { native_1440p: Some("1"), ..env(Some("fidelity")) });
        assert!(d.log[0].contains("VICTORY_NATIVE_1440P=1"), "{:?}", d.log);
        // Nothing asked: still fidelity, and not the 720p default of a Mac.
        let d = decide(&env(Some("fidelity")));
        assert!(!d.native_1440p);
        assert_eq!(d.l3_output, Asked::Title, "a Mac's 720p default is for perf only");
        assert!(d.log[0].contains("nothing about the output patched"), "{:?}", d.log);
        // Through `VICTORY_OUTPUT=0` alone, as run.sh passed the profile before.
        let d = decide(&Env { output: Some("0"), native_1440p: Some("1"), ..env(None) });
        assert!(!d.native_1440p);
        let direct = decide(&Env { output: Some("720"), render_scale: Some("0.5"), ..env(Some("fidelity")) });
        assert_eq!(direct.l3_output, Asked::Title);
        assert!(!direct.l3_scale);
        // =0 in fidelity is no news: no "ignored" line.
        let d = decide(&Env { native_1440p: Some("0"), ..env(Some("fidelity")) });
        assert!(!d.log[0].contains("ignored"), "{:?}", d.log);
    }

    // ---- 3. explicit overrides -------------------------------------------------------

    /// perf with `VICTORY_NATIVE_1440P=0`: off, and L3's own 720p default applies on a Mac.
    #[test]
    fn perf_can_turn_native_1440p_off() {
        let d = decide(&Env { native_1440p: Some("0"), ..env(Some("perf")) });
        assert!(!d.native_1440p);
        assert_eq!(d.l3_output, Asked::Size((1280, 720)));
        assert!(d.l3_scale);
    }

    /// perf with an output size: the L3 chain, native off by default (alternatives); the short forms read.
    #[test]
    fn perf_with_an_output_size_takes_l3() {
        for text in ["720", "720p", "1280x720", " 1280X720 "] {
            let d = decide(&Env { output: Some(text), ..env(Some("perf")) });
            assert!(!d.native_1440p, "{text}");
            assert_eq!(d.l3_output, Asked::Size((1280, 720)), "{text}");
            assert!(d.l3_scale);
        }
        let d = decide(&Env { output: Some("1440"), ..env(Some("perf")) });
        assert_eq!(d.l3_output, Asked::Size((2560, 1440)), "an output of 1440p by L3 is still L3 (settings size), not native");
        assert!(!d.native_1440p);
        // A bare number that is no mode's height is not a size.
        let d = decide(&Env { output: Some("999"), ..env(Some("perf")) });
        assert!(d.log[0].contains("VICTORY_OUTPUT=999"), "{:?}", d.log);
        assert!(d.native_1440p, "wrong value: ignored, the profile's default");
    }

    /// Both asked outright: native 1440p wins, and L3's output is dropped with the reason in the log.
    #[test]
    fn native_1440p_asked_outright_beats_an_l3_output() {
        let d = decide(&Env { native_1440p: Some("1"), output: Some("1280x720"), ..env(Some("perf")) });
        assert!(d.native_1440p);
        assert_eq!(d.l3_output, Asked::Title);
        assert!(!d.l3_scale);
        assert!(d.log[0].contains("VICTORY_OUTPUT=1280x720 (L3) not used"), "{:?}", d.log);
    }

    /// A scale asked for (`VICTORY_RENDER_1080P`, `VICTORY_RENDER_SCALE`) is a scale of the 1440p output in perf.
    #[test]
    fn a_render_scale_in_perf_is_a_scale_of_the_1440p_output() {
        for e in [
            Env { render_1080p: Some("1"), ..env(Some("perf")) },
            Env { render_scale: Some("0.667"), ..env(Some("perf")) },
        ] {
            let d = decide(&e);
            assert!(d.native_1440p && !d.l3_scale && d.l3_output == Asked::Title);
        }
        // With an output size too, L3 (its scale at the L3 output) keeps the scale.
        let d = decide(&Env { render_scale: Some("0.667"), output: Some("720"), ..env(Some("perf")) });
        assert!(!d.native_1440p && d.l3_scale);
    }

    /// Not a Series X: the default is off with a line; asked outright it is left for `run.rs` to refuse.
    #[test]
    fn native_1440p_is_for_the_series_x_image() {
        let s = Env { series_x: false, ..env(Some("perf")) };
        let d = decide(&s);
        assert!(!d.native_1440p);
        assert_eq!(d.l3_output, Asked::Size((1280, 720)), "L3's 720p, as before");
        assert!(d.log.iter().any(|l| l.contains("Series X image only")), "{:?}", d.log);
        let d = decide(&Env { native_1440p: Some("1"), ..s });
        assert!(d.native_1440p, "asked outright: run.rs refuses it with its error");
        assert!(d.log[0].contains("refuses"), "{:?}", d.log);
    }

    /// Anything but a switch word is not an answer.
    #[test]
    fn switch_words() {
        for (t, v) in [("1", Some(true)), ("on", Some(true)), (" True ", Some(true)), ("0", Some(false)), ("off", Some(false)), ("", None), ("maybe", None)] {
            assert_eq!(switch(Some(t)), v, "{t:?}");
        }
        assert_eq!(switch(None), None);
        // A garbled VICTORY_NATIVE_1440P does not turn the default off.
        assert!(decide(&Env { native_1440p: Some("maybe"), ..env(Some("perf")) }).native_1440p);
    }
}
