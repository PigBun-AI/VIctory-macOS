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
