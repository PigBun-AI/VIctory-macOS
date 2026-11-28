//! mac: a census of the translator over a folder of containers (`*.cdxil`, e.g. a run's
//! `VICTORY_SHADER_SAVE` corpus): how many translate, how many fail, by stage, and the sum and
//! digest of the translated DXIL. The host is the one `victory-census translate` tells the
//! translator (wave 32, view limit 2^21 - 1, raw views 2^21, no rdna). Used for the before/after
//! of a translator change: run it at both commits, compare the `ok` lines (`docs/mac/PATCHES.md`).
//!
//!   cargo run --release -p victory-shader --example translate_count -- DIR [JOBS]
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

fn main() {
    let mut a = std::env::args().skip(1);
    let dir = a.next().expect("usage: translate_count DIR [JOBS]");
    let jobs: usize = a.next().and_then(|v| v.parse().ok()).unwrap_or(2);
    victory_shader::emit::set_host_max_wave(32);
    victory_shader::emit::set_view_heap_limit((1 << 21) - 1);
    victory_shader::emit::set_raw_views(1 << 21);
    victory_shader::emit::set_host_rdna(false);
    std::panic::set_hook(Box::new(|_| {}));
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(".cdxil")).collect();
    files.sort();
    let files = Arc::new(files);
    let next = Arc::new(Mutex::new(0usize));
    // (file name, stage, ok, dxil bytes, hash of the dxil)
    let rows = Arc::new(Mutex::new(Vec::<(String, String, bool, usize, u64)>::new()));
    let hs: Vec<_> = (0..jobs).map(|_| {
        let (files, next, rows) = (files.clone(), next.clone(), rows.clone());
        std::thread::spawn(move || loop {
            let i = { let mut n = next.lock().unwrap(); let i = *n; *n += 1; i };
            let Some(f) = files.get(i) else { break };
            let name = f.file_name().unwrap().to_string_lossy().into_owned();
            let stage = name.rsplit('.').nth(1).unwrap_or("?").to_owned();
            let bytes = std::fs::read(f).unwrap();
            let r = std::panic::catch_unwind(|| victory_shader::emit::translate(&bytes));
            let row = match r {
                Ok(Ok(s)) => {
                    let mut h = 0xcbf29ce484222325u64;
                    for b in &s.dxil { h = (h ^ *b as u64).wrapping_mul(0x100000001b3); }
                    (name, stage, true, s.dxil.len(), h)
                }
                _ => (name, stage, false, 0, 0),
            };
            rows.lock().unwrap().push(row);
        })
    }).collect();
    for h in hs { h.join().unwrap(); }
    let mut rows = rows.lock().unwrap().clone();
    rows.sort();
    let mut by: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    let mut digest = 0xcbf29ce484222325u64;
    for (name, stage, ok, len, h) in &rows {
        let e = by.entry(stage.clone()).or_default();
        if *ok { e.0 += 1; e.2 += len; } else { e.1 += 1; println!("FAILED {name}"); }
        for b in name.bytes().chain(h.to_le_bytes()) { digest = (digest ^ b as u64).wrapping_mul(0x100000001b3); }
    }
    let (mut ok, mut bad, mut bytes) = (0, 0, 0);
    for (stage, (o, b, n)) in &by { println!("{stage}: ok {o} failed {b} dxil bytes {n}"); ok += o; bad += b; bytes += n; }
    println!("total: ok {ok} failed {bad} dxil bytes {bytes} digest {digest:016x}");
    if let Ok(path) = std::env::var("TRANSLATE_COUNT_ROWS") {
        let s: String = rows.iter().map(|(n, _, ok, len, h)| format!("{n}\t{ok}\t{len}\t{h:016x}\n")).collect();
        std::fs::write(path, s).unwrap();
    }
}
