//! C-V-008 (docs/mac/PATCHES.md G12): what the shader cache serves is what it stored. A hit is the translation, byte for
//! byte; an entry damaged in any way (a byte flipped anywhere, cut at any length, extended, an old entry without
//! the trailer, empty) is a miss that is translated again; a different build or container never meets another's entry.
//!
//! Reference: the cache's contract (shader_cache.rs header, decision 0003): "an entry that does not read back is
//! translated again"; BLAKE3 as the integrity code (the reference implementation's test vectors are in the crate).
use super::*;
use std::sync::atomic::AtomicU64;
use victory_shader::emit::{StaticSampler, UserData};
use victory_shader::{HwStage, RootType};

fn shader(seed: u8) -> HostShader {
    HostShader {
        stage: HwStage::Cs,
        dxil: (0..300u32).map(|i| (i as u8).wrapping_mul(7).wrapping_add(seed)).collect(),
        user_data: vec![UserData { index: 4, slot: 2, root: RootType::Constants, dword: u32::from(seed) }],
        samplers: vec![[1, 2, 3, 4], [5, 6, 7, 8]],
        static_samplers: vec![StaticSampler::from_words([0x3088, 0x0054_0100, 0x1400_0080, 0x8000_0000])],
        root_cbvs: vec![1],
        root_srvs: vec![2, 3],
        root_uavs: vec![4],
        root_indexed_views: Vec::new(),
        vertex_reads: Vec::new(),
    }
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
fn an_entry_damaged_in_any_way_is_translated_again_and_never_served() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(dir.path()).unwrap();
    let build = [3u8; 32];
    let container = b"a console container";
    let runs = AtomicU64::new(0);
    let translate = |_: &[u8]| {
        runs.fetch_add(1, Ordering::Relaxed);
        Ok(shader(9))
    };
    assert_eq!(cached(&cache, &build, container, translate), Ok(shader(9)));
    let files = walk(dir.path());
    assert_eq!(files.len(), 1);
    let good = std::fs::read(&files[0]).unwrap();
    assert_eq!(good.len(), encode(&shader(9)).len() + SEAL);
    assert_eq!(cached(&cache, &build, container, translate), Ok(shader(9)));
    assert_eq!(runs.load(Ordering::Relaxed), 1, "a hit is not translated");

    let mut expected_runs = 1;
    let mut check = |damaged: Vec<u8>, what: String| {
        std::fs::write(&files[0], damaged).unwrap();
        expected_runs += 1;
        assert_eq!(cached(&cache, &build, container, translate), Ok(shader(9)), "{what}");
        assert_eq!(runs.load(Ordering::Relaxed), expected_runs, "{what}: translated again");
        assert_eq!(std::fs::read(&files[0]).unwrap(), good, "{what}: stored again as it was");
    };
    // every byte flipped (all 8 bits of a sample of positions, one bit everywhere else)
    for i in 0..good.len() {
        let mut d = good.clone();
        d[i] ^= 1 << (i % 8);
        check(d, format!("byte {i} flipped"));
    }
    // cut at every length, and extended
    for n in 0..good.len() {
        check(good[..n].to_vec(), format!("cut at {n}"));
    }
    check([good.as_slice(), &[0]].concat(), "one byte more".into());
    // an entry of an earlier build: the encoding without the trailer
    check(encode(&shader(9)), "no trailer".into());
    // an intact entry that sits under another key's name
    check(seal(encode(&shader(9)), &[&build[..], b"another container".as_slice()].concat()), "an entry sealed for another key".into());
}

#[test]
fn a_hit_is_exactly_the_translation_for_many_shapes() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(dir.path()).unwrap();
    for seed in 0..40u8 {
        let mut s = shader(seed);
        s.dxil.truncate(seed as usize * 7);
        s.samplers.truncate(usize::from(seed % 3));
        s.root_srvs = (0..seed % 5).collect();
        let want = s.clone();
        let container = vec![seed; 10 + usize::from(seed)];
        let first = cached(&cache, &[1; 32], &container, |_| Ok(want.clone())).unwrap();
        let second = cached(&cache, &[1; 32], &container, |_| panic!("a hit must not translate")).unwrap();
        assert_eq!((&first, &second), (&want, &want), "seed {seed}");
        // another build or another container is another entry
        assert_eq!(cached(&cache, &[2; 32], &container, |_| Ok(shader(200))).unwrap(), shader(200));
    }
}

#[test]
fn decoding_never_panics_and_what_it_takes_is_what_encoding_gives() {
    // random bytes and mutations of real encodings: `decode` is `None`, or a shader that encodes back to the same bytes
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let good = encode(&shader(5));
    let mut taken = 0;
    for round in 0..20_000u32 {
        let mut d = if round % 2 == 0 { good.clone() } else { (0..(next() % 400) as usize).map(|_| next() as u8).collect() };
        for _ in 0..(next() % 4) {
            if !d.is_empty() {
                let at = (next() as usize) % d.len();
                d[at] = next() as u8;
            }
        }
        if let Some(s) = decode(&d) {
            taken += 1;
            assert_eq!(encode(&s), d, "round {round}: decoded bytes that are not the encoding of what they decode to");
        }
    }
    assert!(taken > 0);
}
