//! How much of real DXIL Metal Shader Converter accepts: converts every
//! translated shader in a VIctory shader cache, and loads the result into
//! Metal. Ignored by default (it needs the user's cache, which is derived
//! from game data, so it is read in place and never copied; only
//! aggregate numbers leave this test):
//!
//! ```text
//! VICTORY_MSC_SURVEY_CACHE=<cache dir>/shaders/<translator folder> \
//! VICTORY_MSC_SURVEY_OUT=<per-shader results, outside the repository> \
//!   cargo test -p victory-metal-back --release --test msc_survey -- --ignored --nocapture
//! ```
//!
//! Optional: `VICTORY_MSC_SURVEY_DXIL_DIR` instead of the cache: every `*.dxil`
//! file in the folder (synthetic programs), converted with the root signature
//! the file's own `RTS0` part says is absent: a compute shader with the
//! user-data constants only.
//! `VICTORY_MSC_SURVEY_START` / `_LIMIT` pick a range of the sorted list.
//!
//! The cache entry's layout is `victory-gpu`'s `shader_cache.rs` (magic
//! `VSH3`, little-endian words and length-prefixed lists); the reader below
//! is this test's own. The root signature is the one the host builds for
//! the shader alone (`pipeline/host.rs`): user data as root constants
//! `b0, space 0`, root CBV/SRV/UAV descriptors in space 1, the title-memory
//! tables in space 2, static samplers `s<n>`, and both heaps indexed.
#![cfg(target_vendor = "apple")]

mod common_cache;
use common_cache::*;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

use victory_metal_back::device::Device;
use victory_metal_back::msc::{CompileOptions, Msc, MscError, dxil};

/// The result classes, for the aggregate.
#[derive(Default)]
struct Tally {
    n: usize,
    ok: usize,
    classes: BTreeMap<String, usize>,
    by_stage: BTreeMap<String, (usize, usize)>,
    ms: Vec<f64>,
    metallib: Vec<usize>,
    metal_lib_fail: usize,
    pso_ok: usize,
    pso_fail: usize,
    pso_ms: Vec<f64>,
    refused: usize,
    unreadable: usize,
}

#[test]
#[ignore = "needs the user's shader cache; see the module docs"]
fn survey() {
    let out_path = std::env::var("VICTORY_MSC_SURVEY_OUT").expect("VICTORY_MSC_SURVEY_OUT");
    let (dir, synthetic) = match (
        std::env::var("VICTORY_MSC_SURVEY_CACHE"),
        std::env::var("VICTORY_MSC_SURVEY_DXIL_DIR"),
    ) {
        (Ok(d), _) => (d, false),
        (_, Ok(d)) => (d, true),
        _ => panic!("set VICTORY_MSC_SURVEY_CACHE or VICTORY_MSC_SURVEY_DXIL_DIR"),
    };
    let mut paths = Vec::new();
    files(Path::new(&dir), &mut paths);
    paths.sort();
    let start: usize = std::env::var("VICTORY_MSC_SURVEY_START")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let limit: usize = std::env::var("VICTORY_MSC_SURVEY_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(usize::MAX);
    let msc = Msc::get().expect("MSC installed");
    let device = Device::new().expect("Metal 4 device");
    let mut out = std::io::BufWriter::new(
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&out_path)
            .unwrap(),
    );
    let mut t = Tally::default();
    let total = Instant::now();
    for (i, p) in paths.iter().enumerate().skip(start).take(limit) {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let bytes = std::fs::read(p).unwrap();
        let (stage, dxil_bytes, root) = if synthetic {
            let e = Entry {
                stage: 7,
                dxil: bytes,
                ..Default::default()
            };
            let root = root_signature(&e);
            (7, e.dxil, root)
        } else {
            match parse(&bytes) {
                Parsed::Entry(e) => {
                    let root = root_signature(&e);
                    (e.stage, e.dxil, root)
                }
                Parsed::Refused => {
                    t.refused += 1;
                    continue;
                }
                Parsed::Unreadable => {
                    t.unreadable += 1;
                    continue;
                }
            }
        };
        t.n += 1;
        let info = dxil::inspect(&dxil_bytes);
        let sm = info
            .and_then(|i| i.shader_model)
            .map_or("?".to_owned(), |(a, b)| format!("{a}.{b}"));
        let feats = info.and_then(|i| i.features).unwrap_or(0);
        let stage_name = STAGE_NAMES.get(stage as usize).copied().unwrap_or("?");
        let started = Instant::now();
        let r = msc.compile(&dxil_bytes, Some(&root), &CompileOptions::default());
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let entry = t.by_stage.entry(stage_name.to_owned()).or_default();
        entry.0 += 1;
        let (status, size, lib, pso, detail) = match r {
            Ok(c) => {
                entry.1 += 1;
                t.ok += 1;
                t.ms.push(ms);
                t.metallib.push(c.metallib.len());
                let lib = device.library_from_metallib(&c.metallib);
                let (lib_s, pso_s) = match &lib {
                    Err(e) => {
                        t.metal_lib_fail += 1;
                        (
                            format!("fail:{}", first_line(&e.to_string())),
                            "-".to_owned(),
                        )
                    }
                    Ok(_) if stage == 7 => {
                        let s = Instant::now();
                        match device
                            .compute_pipeline_from_metallib(&c.metallib, &c.reflection.entry)
                        {
                            Ok(_) => {
                                t.pso_ok += 1;
                                t.pso_ms.push(s.elapsed().as_secs_f64() * 1000.0);
                                ("ok".to_owned(), "ok".to_owned())
                            }
                            Err(e) => {
                                t.pso_fail += 1;
                                (
                                    "ok".to_owned(),
                                    format!("fail:{}", first_line(&e.to_string())),
                                )
                            }
                        }
                    }
                    Ok(_) => ("ok".to_owned(), "-".to_owned()),
                };
                (
                    "ok".to_owned(),
                    c.metallib.len(),
                    lib_s,
                    pso_s,
                    format!("tg={:?}", c.reflection.threadgroup_size),
                )
            }
            Err(e) => {
                let class = match &e {
                    MscError::Rejected { code, .. } => format!("Rejected:{}", code.name()),
                    MscError::RootSignature { code, .. } => {
                        format!("RootSignature:{}", code.name())
                    }
                    MscError::NoOutput(_) => "NoOutput".to_owned(),
                    other => format!("{other:?}").chars().take(30).collect(),
                };
                *t.classes.entry(class.clone()).or_default() += 1;
                let msg = match &e {
                    MscError::Rejected { message, .. }
                    | MscError::RootSignature { message, .. } => first_line(message),
                    other => first_line(&other.to_string()),
                };
                (class, 0, "-".to_owned(), "-".to_owned(), msg)
            }
        };
        writeln!(
            out,
            "{i}\t{name}\t{stage_name}\t{}\tsm{sm}\tsfi={feats:#x}\t{status}\t{ms:.1}\t{size}\tlib={lib}\tpso={pso}\t{detail}",
            dxil_bytes.len()
        )
        .unwrap();
        out.flush().unwrap();
    }
    let wall = total.elapsed().as_secs_f64();
    let mut ms = t.ms.clone();
    let mut sizes: Vec<f64> = t.metallib.iter().map(|&s| s as f64).collect();
    let mut pso = t.pso_ms.clone();
    println!("== MSC survey of {dir}");
    println!(
        "shaders {} (refused by the translator {}, unreadable {})",
        t.n, t.refused, t.unreadable
    );
    println!("converted {} / {}", t.ok, t.n);
    for (k, (n, ok)) in &t.by_stage {
        println!("  stage {k}: {ok} / {n}");
    }
    for (k, n) in &t.classes {
        println!("  failure {k}: {n}");
    }
    println!(
        "conversion ms (ok only): median {:.1}  p95 {:.1}  max {:.1}  sum {:.1} s  wall {wall:.1} s",
        percentile(&mut ms, 0.5),
        percentile(&mut ms, 0.95),
        percentile(&mut ms, 1.0),
        t.ms.iter().sum::<f64>() / 1000.0,
    );
    println!(
        "metallib bytes: median {:.0}  p95 {:.0}  max {:.0}  total {:.1} MB",
        percentile(&mut sizes, 0.5),
        percentile(&mut sizes, 0.95),
        percentile(&mut sizes, 1.0),
        t.metallib.iter().sum::<usize>() as f64 / 1e6,
    );
    println!(
        "Metal: library from metallib failed {}; compute pipelines ok {} failed {}; pipeline ms median {:.1} p95 {:.1}",
        t.metal_lib_fail,
        t.pso_ok,
        t.pso_fail,
        percentile(&mut pso, 0.5),
        percentile(&mut pso, 0.95),
    );
}
