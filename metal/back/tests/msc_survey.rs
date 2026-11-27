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

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use victory_metal_back::device::Device;
use victory_metal_back::msc::{
    CompileOptions, Msc, MscError, Param, ParamKind, RootSignatureDesc, StaticSampler, Visibility,
    dxil, flags,
};

/// What the cache entry says the host binds.
#[derive(Default)]
struct Entry {
    stage: u32,
    dxil: Vec<u8>,
    user_data_dwords: u32,
    static_samplers: Vec<StaticSampler>,
    root_cbvs: Vec<u8>,
    root_srvs: Vec<u8>,
    root_uavs: Vec<u8>,
    memory: bool,
    memory_writes: bool,
    view_addresses: bool,
    view_marks: bool,
}

struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (h, r) = self.0.split_at_checked(n)?;
        self.0 = r;
        Some(h)
    }
    fn word(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn list(&mut self) -> Option<Vec<u8>> {
        let n = self.word()? as usize;
        Some(self.take(n)?.to_vec())
    }
}

enum Parsed {
    Entry(Box<Entry>),
    /// The translator refused the container; the cache keeps the reason.
    Refused,
    Unreadable,
}

fn parse(bytes: &[u8]) -> Parsed {
    if bytes.starts_with(b"VSHE") {
        return Parsed::Refused;
    }
    let Some(e) = parse_entry(bytes) else {
        return Parsed::Unreadable;
    };
    Parsed::Entry(Box::new(e))
}

fn parse_entry(bytes: &[u8]) -> Option<Entry> {
    let mut r = Reader(bytes);
    if r.take(4)? != b"VSH3" {
        return None;
    }
    let mut e = Entry {
        stage: r.word()?,
        dxil: r.list()?,
        ..Default::default()
    };
    let n = r.word()?;
    let mut max = 0;
    for _ in 0..n {
        let index = r.word()?;
        r.take(12)?; // slot, root type, dword
        max = max.max(index + 1);
    }
    e.user_data_dwords = max.next_multiple_of(4);
    let n = r.word()?;
    r.take(16 * n as usize)?; // the sampler words
    let n = r.word()?;
    for i in 0..n {
        let filter = r.word()?;
        let address = [r.word()?, r.word()?, r.word()?];
        let mip_lod_bias = f32::from_bits(r.word()?);
        let max_anisotropy = r.word()?;
        let comparison = r.word()?;
        let border = r.word()?;
        let min_lod = f32::from_bits(r.word()?);
        let max_lod = f32::from_bits(r.word()?);
        e.static_samplers.push(StaticSampler {
            filter,
            address,
            mip_lod_bias,
            max_anisotropy,
            comparison,
            border,
            min_lod,
            max_lod,
            register: i,
            space: 0,
            visibility: Visibility::All,
        });
    }
    e.root_cbvs = r.list()?;
    e.root_srvs = r.list()?;
    e.root_uavs = r.list()?;
    let n = r.word()?;
    r.take(8 * n as usize)?; // root SRV byte ends
    let bits = r.word()?;
    e.memory = bits & 1 != 0;
    e.memory_writes = bits & 2 != 0;
    e.view_addresses = bits & 4 != 0;
    e.view_marks = bits & 32 != 0;
    let n = r.word()?;
    r.take(8 * n as usize)?; // half views
    r.0.is_empty().then_some(e)
}

/// The stages in the cache entry's order: Ls, Hs, Es, Gs, Vs, Ps, Ms, Cs.
fn visibility(stage: u32) -> Visibility {
    match stage {
        1 => Visibility::Hull,
        3 => Visibility::Geometry,
        4 => Visibility::Vertex,
        5 => Visibility::Pixel,
        6 => Visibility::Mesh,
        _ => Visibility::All,
    }
}

const STAGE_NAMES: [&str; 8] = ["Ls", "Hs", "Es", "Gs", "Vs", "Ps", "Ms", "Cs"];

fn root_signature(e: &Entry) -> RootSignatureDesc {
    let vis = visibility(e.stage);
    let mut params = Vec::new();
    if e.user_data_dwords > 0 {
        params.push(Param {
            kind: ParamKind::Constants {
                register: 0,
                space: 0,
                num_32bit: e.user_data_dwords,
            },
            visibility: vis,
        });
    }
    let desc = |kind: fn(u32, u32) -> ParamKind, slots: &[u8], space: u32, out: &mut Vec<Param>| {
        for &s in slots {
            out.push(Param {
                kind: kind(u32::from(s), space),
                visibility: vis,
            });
        }
    };
    let cbv = |register, space| ParamKind::Cbv { register, space };
    let srv = |register, space| ParamKind::Srv { register, space };
    let uav = |register, space| ParamKind::Uav { register, space };
    desc(cbv, &e.root_cbvs, 1, &mut params);
    desc(srv, &e.root_srvs, 1, &mut params);
    desc(uav, &e.root_uavs, 1, &mut params);
    if e.memory {
        desc(srv, &[0], 2, &mut params);
    }
    if e.memory_writes {
        desc(uav, &[0], 2, &mut params);
    }
    if e.view_addresses {
        desc(srv, &[1], 2, &mut params);
    }
    if e.view_marks {
        // VIEW_MARKS = 2 (SRV), VIEW_WRITES = 1 (UAV), both in space 2.
        desc(srv, &[2], 2, &mut params);
        desc(uav, &[1], 2, &mut params);
    }
    let mut f = flags::HEAP_INDEXED | flags::SAMPLER_HEAP_INDEXED;
    if e.stage != 7 {
        f |= flags::ALLOW_INPUT_LAYOUT;
    }
    let mut samplers = e.static_samplers.clone();
    for s in &mut samplers {
        s.visibility = vis;
    }
    RootSignatureDesc {
        flags: f,
        params,
        static_samplers: samplers,
    }
}

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

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

fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn first_line(s: &str) -> String {
    let l = s.lines().next().unwrap_or("");
    l.chars().take(160).collect::<String>().replace('\t', " ")
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
