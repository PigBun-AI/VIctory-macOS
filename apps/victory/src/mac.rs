//! Mac patches of `victory run` (docs/mac/PATCHES.md, patches L1-L6): what
//! the Windows exe does at load only when it runs on Wine on a Mac, under
//! Rosetta 2. `run.rs` calls [`plan`] once after `prepare` and [`apply`] once
//! after the title's image is mapped; on a PC neither does anything but print
//! the status lines. Ported from VIctory-macOS `run.rs`.

use victory_loader::mac::{clwb, cmpxchg16, masked, resolution, watchdog};

/// What to do to the mapped image.
pub struct Plan {
    /// `VICTORY_PATCH_MASKED` as it resolves.
    masked: Option<String>,
    /// `VICTORY_MAC_CMPXCHG16_CHECK=1` (L6, a diagnostic, off by default).
    cmpxchg16: bool,
    /// The exe's bytes to scan, kept only when one of the two above wants them.
    original: Vec<u8>,
    /// `VICTORY_MAC_NO_WATCHDOG` (L5): `None` unset (the default), else the
    /// hash of the exe the sites are compared against.
    watchdog: Option<String>,
}

/// Prints the status lines and decides what to patch. Takes the exe's bytes
/// out of `original` (leaving it empty) only when masked stores are to be
/// patched: they are kept just for that (114 MB).
pub fn plan(info: &victory_pe::ImageInfo, original: &mut Vec<u8>, hash: &str) -> Plan {
    use victory_hle::rosetta::{fault_tracking, rosetta};
    // Rosetta cannot resume these when they fault partway (#14, #50).
    let found = masked::masked_stores(info, original);
    println!("masked stores  {}", masked::summary(&found));
    // Under Rosetta, tracking is on unless `VICTORY_FAULT_TRACKING=0`, and the
    // stores it cannot resume are patched whenever it is on (`fault_tracking`).
    println!(
        "fault tracking {}",
        if fault_tracking() {
            "on"
        } else if rosetta() {
            "off (VICTORY_FAULT_TRACKING=0: title memory copied whole, ~15 GB a frame in the world)"
        } else {
            "off (title memory copied whole)"
        }
    );
    // Under Rosetta the patch is on by default, tracking or not (the decision and its
    // four combinations are `masked::decide`, tested natively); the log says which.
    let decision = masked::decide(
        std::env::var("VICTORY_PATCH_MASKED").ok(),
        rosetta(),
        fault_tracking(),
        hash,
    );
    println!("masked stores  patch {}", decision.why);
    let masked = decision.request;
    let cmpxchg16 = cmpxchg16::wanted();
    let original = if masked.is_some() || cmpxchg16 { std::mem::take(original) } else { Vec::new() };
    let watchdog = watchdog::wanted().then(|| hash.to_string());
    Plan { masked, cmpxchg16, original, watchdog }
}

/// Applies the plan to the image mapped at `image.base()`, before any title
/// thread runs.
pub fn apply(plan: Plan, image: &victory_loader::LoadedImage, execute: bool) {
    // clwb faults under Rosetta, an exception per write-back loop (#135).
    // Only there; `VICTORY_NOP_CLWB=0` turns it off and `1` turns it on elsewhere.
    if clwb::wanted(std::env::var("VICTORY_NOP_CLWB").ok().as_deref(), victory_hle::rosetta::rosetta()) {
        println!("{}", clwb::nop_clwb_loops(image.base() as u64, image.info()).line());
    } else {
        println!("{}", clwb::LEFT_AS_THEY_ARE);
    }
    // The title's output size, render scale and upscaler (`VICTORY_OUTPUT`,
    // `VICTORY_RENDER_SCALE`, `VICTORY_UPSCALER`; the output size 1280x720
    // on a Mac without the first, the other two off without theirs).
    if execute {
        let size = image.info().size_of_image as usize;
        // SAFETY: the image stays mapped and nothing of it has run yet.
        unsafe { resolution::apply_asked(image.base(), size) };
    }
    // The title's hang watchdog off (`VICTORY_MAC_NO_WATCHDOG=1`, L5): off by
    // default, and not fidelity. Only for the build the sites were read in,
    // and only if every byte matches; otherwise left on, with the reason.
    if let (true, Some(hash)) = (execute, &plan.watchdog) {
        let size = image.info().size_of_image as usize;
        let done = if hash.eq_ignore_ascii_case(watchdog::BUILD_1_0_2_1) {
            // SAFETY: the image stays mapped and nothing of it has run yet.
            unsafe { watchdog::apply(image.base(), size) }
        } else {
            Err(format!("build {hash} is not Series 1.0.2.1"))
        };
        watchdog::report(&done);
    }
    // Masked stores redone as plain stores Rosetta can resume, so fault
    // tracking (on by default) can run under Rosetta (#50).
    if let Some(which) = &plan.masked {
        let wanted = masked::wanted_from(Some(which));
        let n = masked::patch(image.base() as u64, image.info(), &plan.original, &wanted);
        println!("masked stores  {n} patched to plain stores");
    }
    // L6, a diagnostic: every 16-bit `lock cmpxchg` of the title counts its runs and the ones at
    // an address = 15 mod 16 (Rosetta loses the update of those), reported by a thread.
    if plan.cmpxchg16 {
        let n = cmpxchg16::patch(image.base() as u64, image.info(), &plan.original);
        println!("cmpxchg16      {n} sites counted (VICTORY_MAC_CMPXCHG16_CHECK=1)");
    }
    // Every other 256-bit store of the title split into 128-bit halves, from
    // a list made offline (`cargo xtask wide-stores`): Rosetta cannot resume
    // one that faults in its second page.
    if let Some(list) = std::env::var_os("VICTORY_PATCH_WIDE").filter(|v| !v.is_empty() && v != "0") {
        let (n, refused) = victory_loader::mac::wide::patch(image.base() as u64, std::path::Path::new(&list));
        println!("wide stores    {n} split into 128-bit halves ({refused} refused)");
    }
}
