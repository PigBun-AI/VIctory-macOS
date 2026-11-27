//! `VICTORY_LIVE_TRANSLATOR=<dll>`: the shader translator is a library the running game swaps.
//!
//! The library is `victory-shader-dylib` (`cargo build --release -p victory-shader-dylib`, or
//! `bench\live-translator.ps1`). The host never loads the file it is given: it copies it to a
//! numbered file of its own (`%TEMP%\victory-live-translator\<pid>\g<N>-<hash>.dll`) and loads
//! that, so a build can overwrite the watched path while an older copy runs. A library's identity is
//! the BLAKE3 of its bytes: the shader cache keys translations by it (`shader_cache::build`), so
//! a library built from the same code is the same entry and a changed one translates again, once.
//! A library stays loaded for the run, as nothing says no pipeline, callback or thread still runs
//! its code. The copies are left on disk (they are locked while the game runs).
//!
//! What a generation translates with is part of its snapshot
//! (`victory_shader::emit::live::Snapshot::translator`); [`translate`] picks the library from there,
//! so a translation that began under generation N finishes with N's library.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime};

use victory_shader::emit::HostShader;
use victory_shader::emit::live::{self, Snapshot};
use victory_shader::emit::livelib::Library;

static LIBRARIES: RwLock<Option<HashMap<[u8; 32], &'static Library>>> = RwLock::new(None);
static COPIES: AtomicU64 = AtomicU64::new(0);
static DIR: OnceLock<PathBuf> = OnceLock::new();

/// The first eight bytes of a library id, for logs and file names.
pub fn short(id: &[u8; 32]) -> String {
    id[..6].iter().map(|b| format!("{b:02x}")).collect()
}

/// What identifies a version of the watched file.
pub fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

fn dir() -> &'static Path {
    DIR.get_or_init(|| std::env::temp_dir().join("victory-live-translator").join(std::process::id().to_string()))
}

/// The library file's bytes once the file has stopped changing (a build or a copy may still be
/// writing it) and can be read.
fn settled_bytes(path: &Path, patience: Duration) -> Result<Vec<u8>, String> {
    let began = Instant::now();
    let mut last = stamp(path);
    loop {
        std::thread::sleep(Duration::from_millis(300));
        let now = stamp(path);
        if now.is_some() && now == last {
            match std::fs::read(path) {
                Ok(bytes) if !bytes.is_empty() => return Ok(bytes),
                Ok(_) => {}
                Err(e) if began.elapsed() > patience => return Err(format!("{}: {e}", path.display())),
                Err(_) => {}
            }
        }
        if began.elapsed() > patience {
            return Err(format!("{}: still changing or unreadable after {} s", path.display(), patience.as_secs()));
        }
        last = now;
    }
}

/// Copies `path` to a file of ours and loads the copy; the library's id and the library. The same
/// bytes as an earlier load give that library again.
pub fn load(path: &Path, patience: Duration) -> Result<([u8; 32], &'static Library, bool), String> {
    let bytes = settled_bytes(path, patience)?;
    let id = *blake3::hash(&bytes).as_bytes();
    if let Some(lib) = LIBRARIES.read().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&id)) {
        return Ok((id, lib, false));
    }
    let dir = dir();
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let copy = dir.join(format!("g{}-{}.dll", COPIES.fetch_add(1, Ordering::Relaxed), short(&id)));
    std::fs::write(&copy, &bytes).map_err(|e| format!("{}: {e}", copy.display()))?;
    let lib: &'static Library = Box::leak(Box::new(Library::load(&copy)?));
    LIBRARIES.write().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(id, lib);
    Ok((id, lib, true))
}

/// Loads the library the run starts with and makes it generation 0's translator.
pub fn start(path: &Path) -> Result<[u8; 32], String> {
    let (id, lib, _) = load(path, Duration::from_secs(20))?;
    live::init_translator(id);
    tracing::info!(file = %path.display(), library = %short(&id), info = %lib.info, "live translator on: translations come from the library");
    Ok(id)
}

/// A translation by the library `id` under `snapshot`'s switches.
pub fn translate(id: &[u8; 32], container: &[u8], snapshot: &Snapshot) -> Result<HostShader, String> {
    let lib = LIBRARIES
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(id).copied())
        .ok_or_else(|| format!("translator library {} is not loaded", short(id)))?;
    lib.translate(container, snapshot, victory_shader::emit::host_settings())
}

/// The file changed: loads it and makes it the next generation, unless its bytes are the ones
/// already translating. `Ok(None)` when nothing changed.
pub fn reload(path: &Path) -> Result<Option<(u64, [u8; 32], String)>, String> {
    let current = live::current().translator.get().copied();
    let (id, lib, _) = load(path, Duration::from_secs(30))?;
    if Some(id) == current {
        return Ok(None);
    }
    let snap = live::set_translator(id);
    Ok(Some((snap.generation, id, lib.info.clone())))
}
