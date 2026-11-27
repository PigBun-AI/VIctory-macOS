//! `VICTORY_LIVE_FILE=<path>`: translation switches that change while the title runs.
//!
//! A thread looks at the file's modification time every second. The file is
//! one `KEY=VALUE` a line (`#` comments; an empty value is "unset"), only
//! switches in [`victory_shader::emit::live::WHITELIST`]; a switch it does
//! not name is the environment's value from the start. A change makes the
//! next generation (`victory_shader::emit::live`): the translation's key
//! changes with the switches, so the shader cache and the host pipelines
//! (`pipeline::Pipeline::host_now`) look under the new key, miss, and
//! translate again as the title draws. What was made under the old key is
//! kept (a switch put back finds its pipelines at once, and the GPU may
//! still run an old one: nothing is freed).
//!
//! The window's caption gets `[L<generation>]` ([`tag`]).

use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use victory_shader::emit::live;

static ON: AtomicBool = AtomicBool::new(false);

/// ` [L3]` for the caption while `VICTORY_LIVE_FILE` is on, else nothing.
pub fn tag() -> String {
    if ON.load(Ordering::Relaxed) {
        format!(" [L{}]", live::generation())
    } else {
        String::new()
    }
}

/// Starts the watcher if `VICTORY_LIVE_FILE` is set (once).
pub fn start_from_env() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let Some(path) = std::env::var_os("VICTORY_LIVE_FILE").filter(|p| !p.is_empty()) else {
            return;
        };
        let path = std::path::PathBuf::from(path);
        ON.store(true, Ordering::Relaxed);
        tracing::info!(file = %path.display(), switches = ?live::WHITELIST, "live toggles on: the file is looked at every second");
        let _ = std::thread::Builder::new().name("victory-live".into()).spawn(move || watch(&path));
    });
}

/// What identifies a version of the file.
fn stamp(path: &std::path::Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// Reads the file and applies it; the new generation's description if it
/// changed anything. A bad file changes nothing.
pub fn load(path: &std::path::Path) -> Result<Option<(u64, String)>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let settings = live::parse(&text)?;
    let named: Vec<String> = settings.iter().map(|(k, v)| format!("{k}={v}")).collect();
    Ok(live::apply(&settings)?.map(|snap| {
        let shown = if named.is_empty() { "(none: all back to the start's values)".to_owned() } else { named.join(" ") };
        (snap.generation, shown)
    }))
}

fn watch(path: &std::path::Path) {
    // The file as it is at the start is not applied: the run starts with the environment's
    // switches (a file left from an earlier run must not decide them); a later change is.
    let mut seen = stamp(path);
    // The generation being waited for, when it began, pipelines asked for before it.
    let mut report: Option<(u64, Instant, u64, u64, u32)> = None;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let now = stamp(path);
        if now != seen {
            seen = now;
            if now.is_some() {
                match load(path) {
                    Ok(Some((generation, shown))) => {
                        tracing::info!("live toggles changed: {shown}, generation {generation}");
                        let (created, made) = crate::pipeline::live_progress();
                        report = Some((generation, Instant::now(), created, made, 0));
                    }
                    Ok(None) => tracing::info!("live toggles: the file changed but not the switches; generation stays {}", live::generation()),
                    Err(e) => tracing::warn!(error = %e, "live toggles: the file is not applied (generation stays {})", live::generation()),
                }
            }
        }
        if let Some((generation, began, created0, made0, quiet)) = &mut report {
            let (created, made) = crate::pipeline::live_progress();
            let left = created.saturating_sub(made);
            let asked = created.saturating_sub(*created0);
            let (hits, misses) = crate::shader_cache::counts();
            if left > 0 {
                *quiet = 0;
                tracing::info!(
                    generation = *generation,
                    elapsed_s = began.elapsed().as_secs(),
                    "live toggles: {left} pipelines still being translated again ({} of {asked} asked for are made; shader cache {hits} hits, {misses} translated so far)",
                    made.saturating_sub(*made0)
                );
            } else {
                *quiet += 1;
                // Zero twice in a row, after the title has asked for some (or a while passed).
                if *quiet >= 2 && (asked > 0 || began.elapsed() > Duration::from_secs(10)) {
                    tracing::info!(
                        generation = *generation,
                        elapsed_s = began.elapsed().as_secs(),
                        "live toggles: nothing left to translate again: {asked} pipelines asked for, {} made in {:.1} s since the change (shader cache {hits} hits, {misses} translated in all)",
                        made.saturating_sub(*made0),
                        began.elapsed().as_secs_f64()
                    );
                    report = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file writes switches, a changed file makes a generation, the same file makes none, the
    /// first state again is the first translation key (so its pipelines are found again).
    #[test]
    fn a_changed_file_changes_the_translation_key_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("live.txt");
        let key = |snap: &std::sync::Arc<live::Snapshot>| crate::shader_cache::translation_of(snap);
        let first = live::current();
        let k0 = key(&first);

        std::fs::write(&file, "VICTORY_LEGACY_MUL_FAST=1\n").unwrap();
        let (g1, shown) = load(&file).unwrap().expect("a new generation");
        assert!(shown.contains("VICTORY_LEGACY_MUL_FAST=1"));
        let s1 = live::current();
        assert_eq!(s1.generation, g1);
        let k1 = key(&s1);
        assert_ne!(k0, k1, "the key follows the switch");
        // A read under a snapshot gets that snapshot's value, not the newest.
        assert_eq!(live::scoped(&s1, || live::var_os("VICTORY_LEGACY_MUL_FAST")).unwrap(), "1");
        assert_eq!(live::scoped(&first, || live::var_os("VICTORY_LEGACY_MUL_FAST")), first.get("VICTORY_LEGACY_MUL_FAST").cloned());

        // The same file again: nothing changes.
        assert!(load(&file).unwrap().is_none());
        assert_eq!(key(&live::current()), k1);

        // Another value: another key; the first value again: the first key.
        std::fs::write(&file, "VICTORY_LEGACY_MUL_FAST=0\n").unwrap();
        load(&file).unwrap().unwrap();
        let k2 = key(&live::current());
        assert!(k2 != k0 && k2 != k1);
        std::fs::write(&file, "VICTORY_LEGACY_MUL_FAST=1\n").unwrap();
        load(&file).unwrap().unwrap();
        assert_eq!(key(&live::current()), k1, "back to a state seen: its key, so its entries hit");
        // A pipeline: its own pending host pipeline for the first state, another for a changed one,
        // and the first again when the switch is put back (nothing made, nothing freed).
        #[cfg(windows)]
        {
            let p = crate::pipeline::test_pipeline();
            // (the file now holds =1, k1)
            let at_k1 = crate::pipeline::test_host_address(p);
            std::fs::write(&file, "VICTORY_LEGACY_MUL_FAST=0\n").unwrap();
            load(&file).unwrap().unwrap();
            let at_k2 = crate::pipeline::test_host_address(p);
            assert_ne!(at_k1, at_k2, "a changed switch asks for another host pipeline");
            assert_eq!(crate::pipeline::test_host_address(p), at_k2, "asked twice: the same one");
            std::fs::write(&file, "VICTORY_LEGACY_MUL_FAST=1\n").unwrap();
            load(&file).unwrap().unwrap();
            assert_eq!(crate::pipeline::test_host_address(p), at_k1, "put back: the first one again");
            let (created, made) = crate::pipeline::live_progress();
            assert_eq!((created, made), (1, 0), "one made again was asked for, none was made by the test");
        }
        // A name outside the whitelist applies nothing.
        std::fs::write(&file, "VICTORY_GPU_SPLIT=1\n").unwrap();
        assert!(load(&file).is_err());
        assert_eq!(key(&live::current()), k1);
        // Not set at all: the start's values.
        std::fs::write(&file, "# nothing\n").unwrap();
        load(&file).unwrap().unwrap();
        assert_eq!(key(&live::current()), k0);
    }

    /// The caption's tag.
    #[test]
    fn caption_tag_follows_the_generation() {
        // Off unless VICTORY_LIVE_FILE was set at the start.
        if std::env::var_os("VICTORY_LIVE_FILE").is_none() {
            assert_eq!(tag(), "");
        }
    }
}
