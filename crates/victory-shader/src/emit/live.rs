//! Translation switches that can change while the title runs
//! (`VICTORY_LIVE_FILE`, see `victory-gpu`'s `live`).
//!
//! Every switch that shapes a translation is read through [`var_os`] /
//! [`var`] instead of `std::env`. The values live in an immutable
//! [`Snapshot`]: the environment at start, with the switches a live file
//! sets laid over it. A change makes a new snapshot with the next
//! generation; the old ones stay valid (a translation or a host pipeline
//! that began under one finishes under it: [`scoped`]).
//!
//! Cost on the hot path: a read of one atomic and a thread-local compare
//! (the snapshot is fetched from the lock only after a change), then a
//! lookup in a table of about twenty names.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

/// The switches a live file may set: every switch that is part of the
/// translation (the shader cache's key) and read through this module.
/// Anything else is refused by [`parse`].
pub const WHITELIST: &[&str] = &[
    "VICTORY_RT_PAGECACHE",
    "VICTORY_PAGECACHE_ALL",
    "VICTORY_LEGACY_MUL_FAST",
    "VICTORY_EXPORT_F16_SAT",
    "VICTORY_TM_CLAMP_HDR",
    "VICTORY_WAVE_WIDE",
    "VICTORY_WAVE_DEEP_SKIPS",
    "VICTORY_WAVE_OWN_BITS",
    "VICTORY_WAVE_LOCAL",
    "VICTORY_F16_NEAREST",
    "VICTORY_F16_NATIVE", // mac S7
    "VICTORY_F2_READ_FIRST", // mac S8
    "VICTORY_HELPER_OWN",
    "VICTORY_FIRSTLANE_OWN",
    "VICTORY_FIRST_HALF",
    "VICTORY_STRICT_JOINS",
    "VICTORY_MESH_OUTPUT_STRICT",
    "VICTORY_BVH_FENCES",
    "VICTORY_BVH_BOUNDS",
    "VICTORY_GROUP_SYNC",
    "VICTORY_SHADER_OPT",
    "VICTORY_VIEWADDR_TABLE",
    "VICTORY_SKIP_STALL", // mac S3 (from the Windows line 9da1676e)
    "VICTORY_DIV_FIXUP", // mac X4 (from the Windows line 07f56f5a)
    "VICTORY_SCALAR_BOUNDS", // mac X5 (from the Windows line d040bcb3)
];

/// One generation's values of the switches in [`WHITELIST`] (`None`: unset).
#[derive(Debug)]
pub struct Snapshot {
    pub generation: u64,
    values: BTreeMap<&'static str, Option<OsString>>,
    /// What the shader cache computed from this snapshot (its key), once.
    pub key: OnceLock<[u8; 32]>,
    /// The live translator library this generation translates with (the BLAKE3 of its file), if
    /// `VICTORY_LIVE_TRANSLATOR` is on; `None`: the translator linked into the executable. Kept out
    /// of `key` (computed once from the switches) and mixed into the cache key beside it.
    pub translator: OnceLock<[u8; 32]>,
}

impl Snapshot {
    /// The value of `name` in this snapshot.
    pub fn get(&self, name: &str) -> Option<&OsString> {
        match self.values.get(name) {
            Some(v) => v.as_ref(),
            None => None,
        }
    }

    /// Every switch of the whitelist as `(NAME, value)`, an unset one with an empty value: what a
    /// translator library is given (`snapshot_of` of it has this snapshot's values).
    pub fn settings(&self) -> Vec<(String, String)> {
        self.values
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.as_ref().map_or(String::new(), |v| v.to_string_lossy().into_owned())))
            .collect()
    }

    /// `NAME=value` for the switches that differ from `other`.
    pub fn diff(&self, other: &Snapshot) -> Vec<String> {
        let mut out = Vec::new();
        for (k, v) in &self.values {
            if other.values.get(k) != Some(v) {
                out.push(format!("{k}={}", v.as_ref().map_or("<unset>".into(), |v| v.to_string_lossy().into_owned())));
            }
        }
        out
    }

    /// Every set switch as `NAME=value`, for logs.
    pub fn describe(&self) -> String {
        let set: Vec<String> = self
            .values
            .iter()
            .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k}={}", v.to_string_lossy())))
            .collect();
        if set.is_empty() { "(all default)".into() } else { set.join(" ") }
    }
}

fn env_values() -> BTreeMap<&'static str, Option<OsString>> {
    WHITELIST.iter().map(|&n| (n, std::env::var_os(n))).collect()
}

struct State {
    /// The environment at start: what a switch the live file does not name is.
    base: BTreeMap<&'static str, Option<OsString>>,
    current: RwLock<Arc<Snapshot>>,
}

static STATE: OnceLock<State> = OnceLock::new();
/// The current generation, for a read without the lock.
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn state() -> &'static State {
    STATE.get_or_init(|| {
        let base = env_values();
        let first = Arc::new(Snapshot { generation: 0, values: base.clone(), key: OnceLock::new(), translator: OnceLock::new() });
        State { base, current: RwLock::new(first) }
    })
}

thread_local! {
    /// The generation this thread last saw, and its snapshot.
    static SEEN: RefCell<Option<Arc<Snapshot>>> = const { RefCell::new(None) };
    /// A snapshot this thread works under whatever the current one is ([`scoped`]).
    static SCOPE: RefCell<Option<Arc<Snapshot>>> = const { RefCell::new(None) };
}

/// The snapshot a read on this thread sees: the scoped one, else the current.
pub fn snapshot() -> Arc<Snapshot> {
    if let Some(s) = SCOPE.with(|s| s.borrow().clone()) {
        return s;
    }
    current()
}

/// The newest snapshot.
pub fn current() -> Arc<Snapshot> {
    let generation = GENERATION.load(Ordering::Acquire);
    SEEN.with(|seen| {
        let mut seen = seen.borrow_mut();
        if let Some(s) = &*seen {
            if s.generation == generation {
                return s.clone();
            }
        }
        let s = state().current.read().unwrap_or_else(|e| e.into_inner()).clone();
        *seen = Some(s.clone());
        s
    })
}

/// The current generation (0 until a live change).
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// Runs `f` with reads on this thread answered from `snapshot`.
pub fn scoped<R>(snapshot: &Arc<Snapshot>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Arc<Snapshot>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            SCOPE.with(|s| *s.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(SCOPE.with(|s| s.borrow_mut().replace(snapshot.clone())));
    f()
}

/// A switch's value: the live snapshot's for a switch in [`WHITELIST`],
/// the environment's for any other.
pub fn var_os(name: &str) -> Option<OsString> {
    if WHITELIST.contains(&name) {
        return snapshot().get(name).cloned();
    }
    std::env::var_os(name)
}

/// As [`var_os`], as text (`None` when unset or not Unicode).
pub fn var(name: &str) -> Option<String> {
    var_os(name).and_then(|v| v.into_string().ok())
}

/// Lays `settings` over the environment at start and makes that the
/// current snapshot, unless it equals the current one. Switches not named
/// go back to the environment's value. Returns the new snapshot if it
/// changed. Names outside [`WHITELIST`] are an error.
pub fn apply(settings: &[(String, String)]) -> Result<Option<Arc<Snapshot>>, String> {
    let st = state();
    let mut values = st.base.clone();
    for (k, v) in settings {
        let Some(&name) = WHITELIST.iter().find(|n| **n == k.as_str()) else {
            return Err(format!("{k} is not a switch a live file may set"));
        };
        // An empty value is "unset".
        values.insert(name, if v.is_empty() { None } else { Some(OsString::from(v)) });
    }
    let mut cur = st.current.write().unwrap_or_else(|e| e.into_inner());
    if cur.values == values {
        return Ok(None);
    }
    let translator = OnceLock::new();
    if let Some(t) = cur.translator.get() {
        let _ = translator.set(*t);
    }
    let next = Arc::new(Snapshot { generation: cur.generation + 1, values, key: OnceLock::new(), translator });
    *cur = next.clone();
    GENERATION.store(next.generation, Ordering::Release);
    Ok(Some(next))
}

/// A snapshot of the start's values with settings laid over, not installed as the current one
/// (generation 0): for [`scoped`], so a test or a tool can translate under switches without
/// touching the process.
pub fn snapshot_of(settings: &[(String, String)]) -> Result<Arc<Snapshot>, String> {
    let mut values = state().base.clone();
    for (k, v) in settings {
        let Some(&name) = WHITELIST.iter().find(|n| **n == k.as_str()) else {
            return Err(format!("{k} is not a switch a live file may set"));
        };
        values.insert(name, if v.is_empty() { None } else { Some(OsString::from(v)) });
    }
    Ok(Arc::new(Snapshot { generation: 0, values, key: OnceLock::new(), translator: OnceLock::new() }))
}

/// The translator library the run starts with (generation 0; set once, before the first
/// translation). Later libraries come with [`set_translator`].
pub fn init_translator(id: [u8; 32]) {
    let _ = state().current.read().unwrap_or_else(|e| e.into_inner()).translator.set(id);
}

/// A new generation with the same switches and the translator library `id`.
pub fn set_translator(id: [u8; 32]) -> Arc<Snapshot> {
    let st = state();
    let mut cur = st.current.write().unwrap_or_else(|e| e.into_inner());
    let translator = OnceLock::new();
    let _ = translator.set(id);
    let next = Arc::new(Snapshot { generation: cur.generation + 1, values: cur.values.clone(), key: OnceLock::new(), translator });
    *cur = next.clone();
    GENERATION.store(next.generation, Ordering::Release);
    next
}

/// Reads a live file: one `KEY=VALUE` a line; blank lines and `#` comments
/// are skipped. A name outside [`WHITELIST`] fails the whole file (nothing
/// is applied), so a typo is not silently ignored.
pub fn parse(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            return Err(format!("line {}: not KEY=VALUE: {line}", n + 1));
        };
        let (k, v) = (k.trim(), v.trim());
        if !WHITELIST.contains(&k) {
            return Err(format!("line {}: {k} is not a switch a live file may set", n + 1));
        }
        out.push((k.to_owned(), v.to_owned()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sun-speed switches are read through the snapshot: a scope with them set sees them on, one
    /// without sees the defaults (RT_PAGECACHE on, the others off).
    #[test]
    fn page_cache_and_legacy_mul_switches_follow_the_snapshot() {
        let on = snapshot_of(&[
            ("VICTORY_PAGECACHE_ALL".into(), "1".into()),
            ("VICTORY_LEGACY_MUL_FAST".into(), "1".into()),
            ("VICTORY_RT_PAGECACHE".into(), "0".into()),
        ])
        .unwrap();
        let off = snapshot_of(&[("VICTORY_PAGECACHE_ALL".into(), "0".into()), ("VICTORY_LEGACY_MUL_FAST".into(), "0".into())]).unwrap();
        scoped(&on, || {
            assert!(crate::emit::pagecache_all() && crate::emit::legacy_mul_fast() && !crate::emit::rt_pagecache());
        });
        scoped(&off, || {
            assert!(!crate::emit::pagecache_all() && !crate::emit::legacy_mul_fast());
        });
    }

    #[test]
    fn parse_takes_whitelisted_lines_only() {
        let ok = parse("# c
VICTORY_RT_PAGECACHE=1

 VICTORY_EXPORT_F16_SAT = 0 
").unwrap();
        assert_eq!(ok, [("VICTORY_RT_PAGECACHE".into(), "1".into()), ("VICTORY_EXPORT_F16_SAT".into(), "0".into())]);
        assert!(parse("VICTORY_GPU_SPLIT=1").is_err());
        assert!(parse("nonsense").is_err());
    }

    /// Process-wide state: one test drives the whole sequence.
    #[test]
    fn snapshots_follow_the_file_and_scopes_hold_their_own() {
        let first = current();
        let a = apply(&[("VICTORY_LEGACY_MUL_FAST".into(), "1".into())]).unwrap().unwrap();
        assert_eq!(a.generation, generation());
        assert_eq!(var_os("VICTORY_LEGACY_MUL_FAST").unwrap(), "1");
        // Same content again: no new generation.
        assert!(apply(&[("VICTORY_LEGACY_MUL_FAST".into(), "1".into())]).unwrap().is_none());
        let b = apply(&[("VICTORY_LEGACY_MUL_FAST".into(), "0".into())]).unwrap().unwrap();
        assert_eq!(b.generation, a.generation + 1);
        // A scope keeps answering from its snapshot.
        scoped(&a, || assert_eq!(var_os("VICTORY_LEGACY_MUL_FAST").unwrap(), "1"));
        assert_eq!(var_os("VICTORY_LEGACY_MUL_FAST").unwrap(), "0");
        // A switch the file does not name goes back to the start's value.
        let c = apply(&[]).unwrap().unwrap();
        assert_eq!(var_os("VICTORY_LEGACY_MUL_FAST"), first.get("VICTORY_LEGACY_MUL_FAST").cloned());
        assert!(c.generation > b.generation);
        assert!(apply(&[("VICTORY_NOPE".into(), "1".into())]).is_err());
        // A translator library is a generation of its own and rides along with later switch changes.
        let t = set_translator([7; 32]);
        assert_eq!(t.generation, generation());
        assert_eq!(t.translator.get(), Some(&[7; 32]));
        let d = apply(&[("VICTORY_LEGACY_MUL_FAST".into(), "1".into())]).unwrap().unwrap();
        assert_eq!(d.translator.get(), Some(&[7; 32]));
        assert_eq!(d.generation, t.generation + 1);
        // The switches as a library is given them reproduce the snapshot.
        assert_eq!(snapshot_of(&d.settings()).unwrap().settings(), d.settings());
    }
}
