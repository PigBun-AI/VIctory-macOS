//! Translated shaders kept on disk (decision 0003): under the configured
//! cache folder, keyed by the container's content and by the translator
//! (the BLAKE3 of its sources) and its settings, so a changed translator
//! never serves an old translation. Entries go through
//! `victory-pipeline`'s content-addressed cache, which writes them
//! atomically; an entry that does not read back is translated again.
//!
//! The entry is our own encoding of [`HostShader`]: a field added to it
//! must be added to `encode` and `decode` too (entries of older builds
//! are ignored anyway).

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use victory_pipeline::{Cache, Converter};
use victory_shader::emit::wire::{decode, encode};
use victory_shader::emit::HostShader;

static CACHE: OnceLock<Cache> = OnceLock::new();
static HITS: AtomicU64 = AtomicU64::new(0);
static MISSES: AtomicU64 = AtomicU64::new(0);

/// Keeps translated shaders in `dir` from now on.
pub fn use_shader_cache(dir: PathBuf) {
    crate::live::start_from_env();
    match Cache::open(dir) {
        Ok(cache) => {
            let _ = CACHE.set(cache);
        }
        Err(e) => tracing::warn!(error = %e, "no shader cache"),
    }
}

/// Shaders found in the cache and translated since the start.
pub(crate) fn counts() -> (u64, u64) {
    (HITS.load(Ordering::Relaxed), MISSES.load(Ordering::Relaxed))
}

/// What identifies a translation beside its container: the translator
/// (the BLAKE3 of its sources, `build.rs`), the settings it runs with, and
/// what it was told of the host. Not our executable: a rebuild of anything
/// else used to translate every shader again.
fn build() -> [u8; 32] {
    // The view heap's null slot differs by host GPU: a translation cached
    // without the clamp must not be served (`host_settings` below).
    crate::descriptor::host::probe_view_limit();
    // One per live-toggle generation (`victory_shader::emit::live`): the switches below are read from
    // the snapshot this thread works under, so the key and the translation agree.
    let snap = victory_shader::emit::live::snapshot();
    let fixed = snap.key.get_or_init(|| victory_shader::emit::live::scoped(&snap, || {
        let limit = victory_shader::emit::loop_limit();
        if limit != victory_shader::emit::DEFAULT_LOOP_LIMIT {
            tracing::warn!(
                limit,
                "VICTORY_SHADER_LOOP_LIMIT: translated loops stop sooner"
            );
        }
        let mut bytes = env!("VICTORY_TRANSLATOR").as_bytes().to_vec();
        bytes.extend_from_slice(&limit.to_le_bytes());
        // Which group barriers wait is part of the translation too.
        bytes.push(victory_shader::emit::group_sync());
        // So are the switches that pick the form of a wave64 program:
        // without them here a run with `VICTORY_WAVE_WIDE=0` was
        // served the translations an earlier run of the same
        // executable had cached with the switch unset.
        for name in [
            "VICTORY_WAVE_WIDE",
            "VICTORY_WAVE_DEEP_SKIPS",
            "VICTORY_WAVE_OWN_BITS",
            "VICTORY_WAVE_LOCAL",
            "VICTORY_HELPER_OWN",
            "VICTORY_FIRSTLANE_OWN",
            "VICTORY_FIRST_HALF",
            "VICTORY_STRICT_JOINS",
            "VICTORY_MESH_OUTPUT_STRICT",
            "VICTORY_BVH_FENCES",
            "VICTORY_BVH_BOUNDS",
            "VICTORY_F2_READ_FIRST", // mac S8
            // The tone-mapping pixel program's samples clamped alike (one program; the key is global here).
            "VICTORY_TM_CLAMP_HDR",
        ] {
            bytes.extend_from_slice(name.as_bytes());
            bytes.push(b'=');
            if let Some(value) = victory_shader::emit::live::var_os(name) {
                bytes.extend_from_slice(value.as_encoded_bytes());
            }
            bytes.push(0);
        }
        // 16-bit float results rounded to nearest as the console does (mac S7), default on: keyed
        // by the effective state like the one below.
        bytes.extend_from_slice(b"VICTORY_F16_NEAREST=");
        bytes.push(victory_shader::emit::f16_nearest() as u8);
        bytes.push(0);
        bytes.extend_from_slice(b"VICTORY_F16_NATIVE=");
        bytes.push(victory_shader::emit::f16_native() as u8);
        bytes.push(0);
        // Colour exports clamped to +-65504, NaN -> 0 (emit::lower::sunclamp), default on:
        // keyed by the effective state, so unset and "1" share entries and "0" differs.
        bytes.extend_from_slice(b"VICTORY_EXPORT_F16_SAT=");
        bytes.push(victory_shader::emit::export_sat() as u8);
        bytes.push(0);
        // Switches keyed by their effective value (RT_PAGECACHE is on unless "0"; the others
        // are off unless "1"), so an unset variable and its default share entries.
        for (name, on) in [
            // Ray-tracing programs keep their last page-directory entries in group-shared memory.
            ("VICTORY_RT_PAGECACHE", victory_shader::emit::rt_pagecache()),
            // The same for the other compute programs.
            ("VICTORY_PAGECACHE_ALL", victory_shader::emit::pagecache_all()),
            // The legacy multiply written shorter where provably the same.
            ("VICTORY_BVH_BOUNDS", victory_shader::emit::legacy_mul_fast()),
            // Buffer-view addresses picked by a computed index come from the host table.
            ("VICTORY_VIEWADDR_TABLE", victory_shader::emit::viewaddr_table()),
        ] {
            bytes.extend_from_slice(name.as_bytes());
            bytes.push(b'=');
            bytes.push(on as u8);
            bytes.push(0);
        }
        // And which clean-up passes ran over the translated code.
        bytes.extend_from_slice(&victory_shader::emit::shader_opt().to_le_bytes());
        *blake3::hash(&bytes).as_bytes()
    }));
    let mut hash = blake3::Hasher::new();
    hash.update(fixed);
    // A live translator library (`VICTORY_LIVE_TRANSLATOR`): its bytes' hash; the same library, the
    // same entries; another, its own. (Not part of `fixed`: that is per switch snapshot, this per
    // generation.)
    if let Some(id) = snap.translator.get() {
        hash.update(b"VICTORY_LIVE_TRANSLATOR=");
        hash.update(id);
    }
    for value in victory_shader::emit::host_settings() {
        hash.update(&value.to_le_bytes());
    }
    *hash.finalize().as_bytes()
}

/// [`build`] for a translation that hands per-vertex values on (mac S1: a mesh shader with the reads
/// of the pixel shader it is paired with); `build` itself when there are none.
fn build_as(reads: &[victory_shader::emit::VertexRead], rect_clip: bool) -> [u8; 32] {
    let base = build();
    if reads.is_empty() && !rect_clip {
        return base;
    }
    let mut hash = blake3::Hasher::new();
    hash.update(&base);
    hash.update(b"VICTORY_VERTEX_READS=");
    hash.update(&victory_shader::emit::vertex_flat::encode(reads));
    if rect_clip {
        hash.update(b"VICTORY_RECT_CLIP"); // mac S4
    }
    *hash.finalize().as_bytes()
}

/// What tells one set of translations from another ([`build`]).
pub(crate) fn translation() -> [u8; 32] {
    build()
}

/// [`translation`] as it is under snapshot (a live-toggle generation).
pub(crate) fn translation_of(snapshot: &std::sync::Arc<victory_shader::emit::live::Snapshot>) -> [u8; 32] {
    victory_shader::emit::live::scoped(snapshot, build)
}

/// Puts the translation of `container` into the cache before any title asks
/// for it (`victory shaders`): whether it was there already, or why it does
/// not translate. The host is opened first, as a run does: what the card
/// can do decides the form of some programs.
pub fn precompile(container: &[u8]) -> Result<bool, String> {
    let _ = crate::host::host();
    let misses = MISSES.load(Ordering::Relaxed);
    host_shader(container)?;
    // (Counts other threads' misses too: the caller only adds them up.)
    Ok(MISSES.load(Ordering::Relaxed) == misses)
}

/// The translation of a console shader container, from the cache when
/// there is one.
pub(crate) fn host_shader(container: &[u8]) -> Result<HostShader, String> {
    host_shader_with(container, &[]) // mac S1
}

/// [`host_shader`] of a mesh shader that hands the values of `reads` to its pixel shader (mac S1,
/// `victory_shader::emit::translate_with`). The live translator library takes no reads: with some, the
/// linked translator runs.
pub(crate) fn host_shader_with(container: &[u8], reads: &[victory_shader::emit::VertexRead]) -> Result<HostShader, String> {
    host_shader_as(container, reads, false)
}

/// [`host_shader_with`], and with `rect_clip`: a vertex shader of a rectangle list whose clip distances
/// are plain outputs (mac S4, `victory_shader::emit::translate_variant`). Also translated by the linked
/// translator, as the reads are.
pub(crate) fn host_shader_as(container: &[u8], reads: &[victory_shader::emit::VertexRead], rect_clip: bool) -> Result<HostShader, String> {
    // The generation this translation belongs to: its switches, and the translator library
    // (`VICTORY_LIVE_TRANSLATOR`) that is part of the key `build` computes from the same snapshot.
    let snap = victory_shader::emit::live::snapshot();
    let translate = |c: &[u8]| -> Result<HostShader, String> {
        #[cfg(windows)]
        if let Some(id) = snap.translator.get()
            && reads.is_empty()
            && !rect_clip
        {
            return crate::live_translator::translate(id, c, &snap);
        }
        victory_shader::emit::live::scoped(&snap, || victory_shader::emit::translate_variant(c, reads, rect_clip)).map_err(|e| e.to_string())
    };
    match CACHE.get() {
        Some(cache) => cached(cache, &build_as(reads, rect_clip), container, translate),
        None => translate(container),
    }
}

/// mac G12: an entry carries the first 16 bytes of the BLAKE3 of its key (the build and the container) and its
/// encoding. The entry is published by an atomic rename, so a half-written file is never seen, but nothing checked
/// that what is read is what was written: a flipped byte inside the DXIL (a disk error, a copied cache folder) decoded
/// as a different, valid shader and was served, and so would an intact entry that sits under another key. An entry
/// without a good trailer for its key (all entries of earlier builds) is a miss.
const SEAL: usize = 16;

fn trailer(key: &[u8], body: &[u8]) -> [u8; SEAL] {
    let mut h = blake3::Hasher::new();
    h.update(&(key.len() as u64).to_le_bytes());
    h.update(key);
    h.update(body);
    h.finalize().as_bytes()[..SEAL].try_into().expect("16 bytes")
}

fn seal(mut encoded: Vec<u8>, key: &[u8]) -> Vec<u8> {
    let t = trailer(key, &encoded);
    encoded.extend_from_slice(&t);
    encoded
}

fn unseal<'a>(bytes: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let (body, t) = bytes.split_at_checked(bytes.len().checked_sub(SEAL)?)?;
    (trailer(key, body) == *t).then_some(body)
}

/// The cache entry's converter: the build and the container in, the
/// encoded translation out. `ran` tells a miss from a hit.
struct Translate<F> {
    translate: F,
    ran: AtomicBool,
}

impl<F: Fn(&[u8]) -> Result<HostShader, String> + Send + Sync> Converter for Translate<F> {
    fn id(&self) -> &'static str {
        "shader.dxil"
    }
    fn version(&self) -> u32 {
        // The build fingerprint in the input does the versioning.
        1
    }
    fn convert(&self, input: &[u8]) -> anyhow::Result<Vec<u8>> {
        self.ran.store(true, Ordering::Relaxed);
        let shader = (self.translate)(&input[32..]).map_err(anyhow::Error::msg)?;
        Ok(seal(encode(&shader), input))
    }
}

fn cached(
    cache: &Cache,
    build: &[u8; 32],
    container: &[u8],
    translate: impl Fn(&[u8]) -> Result<HostShader, String> + Send + Sync,
) -> Result<HostShader, String> {
    let input = [&build[..], container].concat();
    let converter = Translate {
        translate,
        ran: AtomicBool::new(false),
    };
    for _ in 0..2 {
        let path = cache
            .get_or_convert(&converter, &input)
            .map_err(|e| format!("{e:#}"))?;
        if let Some(shader) = std::fs::read(&path).ok().and_then(|b| unseal(&b, &input).and_then(decode)) { // mac G12
            let (counter, what) = if converter.ran.load(Ordering::Relaxed) {
                (&MISSES, "translated and cached")
            } else {
                (&HITS, "from the cache")
            };
            counter.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(path = %path.display(), "shader {what}");
            return Ok(shader);
        }
        tracing::debug!(path = %path.display(), "unreadable shader cache entry; translating again");
        let _ = std::fs::remove_file(&path);
    }
    MISSES.fetch_add(1, Ordering::Relaxed);
    (converter.translate)(container)
}

#[cfg(test)]
#[path = "mac/prove/shader_cache_damage.rs"]
mod prove_damage; // mac G12

#[cfg(test)]
mod tests {
    use super::*;
    use victory_shader::emit::{StaticSampler, UserData};
    use victory_shader::resource::RootIndexedView;
    use victory_shader::{HwStage, RootType};

    fn shader() -> HostShader {
        HostShader {
            stage: HwStage::Ps,
            dxil: b"DXBC and the rest".to_vec(),
            user_data: vec![UserData {
                index: 8,
                slot: 1,
                root: RootType::Table,
                dword: 0,
            }],
            samplers: vec![[0x400, 0xc0_0000, 0x18f0_0000, 0]],
            static_samplers: vec![StaticSampler::from_words([
                0x400,
                0xc0_0000,
                0x18f0_0000,
                0,
            ])],
            root_cbvs: vec![2],
            root_srvs: vec![0, 3],
            root_uavs: vec![1],
            root_indexed_views: vec![RootIndexedView {
                table_slot: 1,
                constant_slot: 2,
                constant_dword: 3,
                byte_offset: 32,
                shift: 5,
            }],
            vertex_reads: Vec::new(), // mac S1
        }
    }

    #[test]
    fn translations_come_back_from_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let build = [7u8; 32];
        let container = b"a console container".as_slice();
        let runs = AtomicU64::new(0);
        let translate = |_: &[u8]| {
            runs.fetch_add(1, Ordering::Relaxed);
            Ok(shader())
        };
        assert_eq!(cached(&cache, &build, container, translate), Ok(shader()));
        assert_eq!(cached(&cache, &build, container, translate), Ok(shader()));
        assert_eq!(runs.load(Ordering::Relaxed), 1, "the second came from disk");

        // Another build translates again.
        assert_eq!(cached(&cache, &[8; 32], container, translate), Ok(shader()));
        assert_eq!(runs.load(Ordering::Relaxed), 2);

        // A damaged entry is a miss, translated and stored again.
        for e in walk(dir.path()) {
            std::fs::write(e, b"VSH1 broken").unwrap();
        }
        assert_eq!(cached(&cache, &build, container, translate), Ok(shader()));
        assert_eq!(runs.load(Ordering::Relaxed), 3);
        assert_eq!(cached(&cache, &build, container, translate), Ok(shader()));
        assert_eq!(runs.load(Ordering::Relaxed), 3);

        // Failures are not stored.
        let fail = |_: &[u8]| Err::<HostShader, _>("unsupported".to_owned());
        assert!(cached(&cache, &build, b"other", fail).is_err());
    }

    /// A change of a live switch changes the key (`translation_of`); the entry under the first key
    /// is still there when the switch is put back: translated once per state, not per switch.
    #[test]
    fn live_switches_key_the_cache_and_put_back_hits() {
        use victory_shader::emit::live::snapshot_of;
        let on = snapshot_of(&[("VICTORY_BVH_BOUNDS".into(), "1".into())]).unwrap();
        let off = snapshot_of(&[("VICTORY_BVH_BOUNDS".into(), "0".into())]).unwrap();
        let none = snapshot_of(&[]).unwrap();
        let none_again = snapshot_of(&[("VICTORY_BVH_BOUNDS".into(), String::new())]).unwrap();
        let (k_on, k_off, k_none) = (translation_of(&on), translation_of(&off), translation_of(&none));
        assert!(k_on != k_off && k_on != k_none && k_off != k_none);
        assert_eq!(k_none, translation_of(&none_again), "empty is unset");
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let runs = AtomicU64::new(0);
        let translate = |_: &[u8]| {
            runs.fetch_add(1, Ordering::Relaxed);
            Ok(shader())
        };
        let c = b"a console container".as_slice();
        let at = |key: &[u8; 32]| cached(&cache, key, c, translate).unwrap();
        at(&k_none);
        at(&k_on);
        at(&k_off);
        assert_eq!(runs.load(Ordering::Relaxed), 3, "each state translates once");
        at(&k_none);
        at(&k_on);
        at(&k_off);
        at(&k_on);
        assert_eq!(runs.load(Ordering::Relaxed), 3, "put back: all hit");
    }

    /// The translator library is part of the key: another library, another entry; the same library
    /// (same bytes' hash), the same entries; none, the executable's own translator.
    #[test]
    fn the_translator_library_keys_the_cache() {
        use victory_shader::emit::live::snapshot_of;
        let with = |id: Option<u8>| {
            let s = snapshot_of(&[]).unwrap();
            if let Some(id) = id {
                s.translator.set([id; 32]).unwrap();
            }
            translation_of(&s)
        };
        let (none, a, a_again, b) = (with(None), with(Some(1)), with(Some(1)), with(Some(2)));
        assert_eq!(a, a_again);
        assert!(none != a && none != b && a != b);
    }

    fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            if e.path().is_dir() {
                files.extend(walk(&e.path()));
            } else {
                files.push(e.path());
            }
        }
        files
    }

    #[test]
    fn entries_round_trip_and_reject_damage() {
        let bytes = encode(&shader());
        assert_eq!(decode(&bytes), Some(shader()));
        assert_eq!(decode(&bytes[..bytes.len() - 1]), None);
        assert_eq!(decode(&[bytes.as_slice(), &[0]].concat()), None);
        assert_eq!(decode(b"VSH0"), None);
    }
}
