//! Ledger S6 (wave64 programs on 32-lane hosts, ported from foxhound 454ffea and
//! VIctory-NVIDIA eeb17cc; GPL): the title's own programs from a run's corpus
//! (the folder named by `VICTORY_LIVE_CORPUS`, saved with `VICTORY_SHADER_SAVE`), translated for a host of
//! 32-lane waves with each switch on and off. Each test checks that the switch reaches the
//! program it is for (the translation changes, or a counter of the check moves) and that the
//! program still translates; tests whose corpus file is missing say so and pass.

use std::sync::atomic::Ordering;

use super::{HostShader, live, lower};
use crate::ShaderError;

/// The corpus folder (`VICTORY_LIVE_CORPUS`).
fn corpus_file(name: &str) -> Option<Vec<u8>> {
    // (mac: no default folder; without the variable the test says so and passes)
    let Some(dir) = std::env::var_os("VICTORY_LIVE_CORPUS").map(std::path::PathBuf::from) else {
        eprintln!("VICTORY_LIVE_CORPUS not set; not run");
        return None;
    };
    let p = dir.join(format!("{name}.compute.cdxil"));
    match std::fs::read(&p) {
        Ok(b) => Some(b),
        Err(_) => {
            eprintln!("{} not there; not run", p.display());
            None
        }
    }
}

/// One translation at a time: the host wave and the counters are shared.
static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `bytes` translated for a host of 32-lane waves under `settings` (live switches).
pub(super) fn translate32(bytes: &[u8], settings: &[(&str, &str)]) -> Result<HostShader, ShaderError> {
    let snap = live::snapshot_of(&settings.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect::<Vec<_>>())
        .expect("whitelisted switches");
    let was = lower::TEST_HOST_WAVE.replace(Some(32));
    let r = live::scoped(&snap, || super::translate(bytes));
    lower::TEST_HOST_WAVE.set(was);
    r
}

/// `water_shading:CS_TransmissionVolumetrics_Tiled` (corpus 5dc3b25986622949, digest
/// 24126c14dc7619b0): wave64, groups of 64.
const WATER_TILED: &str = "5dc3b25986622949";
/// `_Tiled_LocalLights` (corpus 6f225b641c5b0708, digest 7099a57fded14261).
const WATER_LOCAL: &str = "6f225b641c5b0708";

/// `VICTORY_SKIP_STALL` (foxhound ir.rs:840-860): in the widened water program the skip at
/// 0x2da8 around the loop at 0x2e60 is kept (the counter moves with the switch on, not off),
/// and the program translates either way.
#[test]
fn skip_stall_keeps_the_water_programs_loop_skip() {
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(bytes) = corpus_file(WATER_TILED) else { return };
    let wide = ("VICTORY_WAVE_WIDE", "24126c14dc7619b0");
    let kept = || crate::ir::SKIPS_KEPT.load(Ordering::Relaxed);
    let before = kept();
    let on = translate32(&bytes, &[wide, ("VICTORY_SKIP_STALL", "1")]);
    let with = kept() - before;
    let before = kept();
    let off = translate32(&bytes, &[wide, ("VICTORY_SKIP_STALL", "0")]);
    let without = kept() - before;
    eprintln!("skips kept: on {with}, off {without}; on ok {}, off ok {}", on.is_ok(), off.is_ok());
    assert!(with > 0, "the stall check fires for the water program");
    assert_eq!(without, 0, "switched off it does not");
    let on = on.expect("translates with the skip kept");
    let off = off.expect("translates without");
    assert_ne!(on.dxil, off.dxil, "the kept skip changes the translation");
}

/// Upstream 11-28 0745a5d (`VICTORY_SCALAR_BOUNDS`): with wide loads on (`VICTORY_SHADER_OPT`
/// with WIDE; off by default, so the bug is latent there), over the first 300 corpus programs, the
/// translation changes for some (their one-dword scalar loads keep the bounds check) and none
/// fails either way; `=0` gives the old translation (adversarial).
///
/// mac: the corpus is the folder `VICTORY_LIVE_CORPUS` names (the upstream test read a fixed
/// Windows path and returned silently on a Mac); without it the test prints "skipped: corpus not
/// found" and returns, and it must not be read as a pass of the corpus check.
#[test]
fn one_dword_scalar_loads_keep_their_bounds() {
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(dir) = std::env::var_os("VICTORY_LIVE_CORPUS").map(std::path::PathBuf::from) else {
        eprintln!("skipped: corpus not found (VICTORY_LIVE_CORPUS not set)");
        return;
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        eprintln!("skipped: corpus not found ({} unreadable)", dir.display());
        return;
    };
    let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().ends_with(".cdxil")).collect();
    files.sort();
    if files.is_empty() {
        eprintln!("skipped: corpus not found ({} has no .cdxil)", dir.display());
        return;
    }
    let (mut differ, mut ok_on, mut ok_off, mut bad) = (0, 0, 0, Vec::new());
    for f in files.iter().take(300) {
        let bytes = std::fs::read(f).unwrap();
        // Wide loads are off by default (`opt::DEFAULT`); the fix is for when they are on.
        let wide = ("VICTORY_SHADER_OPT", "0xffffffff");
        // Default options (wide loads off): the switch changes nothing (mac census: pass count and DXIL unchanged).
        match (translate32(&bytes, &[]), translate32(&bytes, &[("VICTORY_SCALAR_BOUNDS", "0")])) {
            (Ok(a), Ok(b)) => assert!(a.dxil == b.dxil, "{}: default translation changed", f.display()),
            (Err(_), Err(_)) => {}
            _ => panic!("{}: default translation passes on one side only", f.display()),
        }
        match (translate32(&bytes, &[wide]), translate32(&bytes, &[wide, ("VICTORY_SCALAR_BOUNDS", "0")])) {
            (Ok(a), Ok(b)) => {
                ok_on += 1;
                ok_off += 1;
                differ += usize::from(a.dxil != b.dxil);
            }
            (Err(e), Ok(_)) => bad.push(format!("{} {e}", f.display())),
            (Ok(_), Err(_)) => ok_on += 1,
            (Err(_), Err(_)) => {}
        }
    }
    eprintln!("scalar bounds over {} programs: translate ok on {ok_on} / off {ok_off}; {differ} differ; failures {bad:?}", files.len().min(300));
    assert!(bad.is_empty());
    assert!(differ > 0);
}

/// mac census check for `VICTORY_DIV_FIXUP` (07f56f5a): over the corpus programs (the first
/// `VICTORY_CORPUS_LIMIT`, default 300; every `.cdxil`, all stages) the number that translate is the
/// same with the switch on and off (the fix only reorders a select); how many translations change is
/// printed (the effect itself is proven by the oracle row `v_div_fixup_f32`). Skips (printing "skipped: corpus not found") without `VICTORY_LIVE_CORPUS`.
#[test]
fn div_fixup_keeps_the_translation_pass_count() {
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(dir) = std::env::var_os("VICTORY_LIVE_CORPUS").map(std::path::PathBuf::from) else {
        eprintln!("skipped: corpus not found (VICTORY_LIVE_CORPUS not set)");
        return;
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        eprintln!("skipped: corpus not found ({} unreadable)", dir.display());
        return;
    };
    let limit: usize = std::env::var("VICTORY_CORPUS_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(300);
    let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().ends_with(".cdxil")).collect();
    files.sort();
    if files.is_empty() {
        eprintln!("skipped: corpus not found ({} has no .cdxil)", dir.display());
        return;
    }
    let (mut n, mut ok_on, mut ok_off, mut differ, mut only_one) = (0, 0, 0, 0, Vec::new());
    for f in files.iter().take(limit) {
        let bytes = std::fs::read(f).unwrap();
        n += 1;
        match (translate32(&bytes, &[("VICTORY_DIV_FIXUP", "1")]), translate32(&bytes, &[("VICTORY_DIV_FIXUP", "0")])) {
            (Ok(a), Ok(b)) => {
                ok_on += 1;
                ok_off += 1;
                differ += usize::from(a.dxil != b.dxil);
            }
            (Ok(_), Err(_)) => {
                ok_on += 1;
                only_one.push(f.display().to_string());
            }
            (Err(_), Ok(_)) => {
                ok_off += 1;
                only_one.push(f.display().to_string());
            }
            (Err(_), Err(_)) => {}
        }
    }
    eprintln!("div_fixup census of {n} programs: translate ok on {ok_on} / off {ok_off}; {differ} translations change; passing on one side only: {only_one:?}");
    assert_eq!(ok_on, ok_off);
    assert!(only_one.is_empty());
}
