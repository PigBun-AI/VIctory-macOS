//! `victory-census mscs` (mac S5, mesh shaders as compute): every mesh program of the game package through
//! the translator's compute form and fetch vertex program (the `VMCS` part, `docs/metal/mesh-compute.md`
//! section 8), then Metal Shader Converter and Metal, as the back builds them (`mesh_emu::Emulated::build`,
//! step by step so the two failures stay apart).
//!
//! ```text
//! victory-census mscs extract --game DIR [--census DIR] --out DIR   the mesh containers, once
//! victory-census mscs run [--pairs] [--isolate] [--census DIR] --out DIR [--limit N] [--timeout S] [--tag T]
//! ```
//!
//! `run`: per mesh program, translated with the host asking for the part (mode All): is there a part (and
//! if not, why), does the compute form build a compute pipeline, does the fetch vertex program build a
//! render pipeline with the default pixel shader. `run --pairs`: per (mesh, pixel) pair of the pipeline
//! book (`--book DIR`), the mesh translated with the pixel shader's reads (S1), the fetch vertex program
//! paired with the real pixel shader. Rows go to `mscs[-pairs]<tag>.jsonl`; `--isolate` runs the work in a
//! child that is restarted after an abort or a hang (MSC 4.0.3 and Apple's compiler service do both), the
//! shader it died on becoming a row.
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{Value, json};
use victory_metal_back::device::Device;
use victory_metal_back::mesh_compute::Layout;
use victory_metal_back::mesh_emu::{Bundle, extend_root};
use victory_metal_back::msc::{CompileOptions, MscError};
use victory_metal_back::pipeline_defaults::{Component, Stages, Variant, default_graphics};
use victory_shader::emit::{HostShader, wire};

use crate::convert::classify;
use crate::dxc::{self, Dxc};
use crate::root::{root_signature, visibility};
use crate::{arg, defaults, flag, jobs, jsonl, out_dir, project_root};

fn census_dir(args: &[String]) -> PathBuf {
    arg(args, "--census").map(PathBuf::from).unwrap_or_else(|| project_root().join("victory-data").join("census"))
}

fn host_settings() {
    victory_shader::emit::set_host_max_wave(defaults::HOST_WAVE);
    victory_shader::emit::set_view_heap_limit(defaults::VIEW_LIMIT);
    victory_shader::emit::set_raw_views(defaults::RAW_TWINS);
    victory_shader::emit::set_host_rdna(false);
    victory_shader::emit::set_host_mesh_compute(victory_shader::emit::MeshComputeMode::All);
}

pub fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("extract") => extract(&args[1..]),
        Some("run") => run_items(&args[1..]),
        _ => Err("mscs extract|run".into()),
    }
}

/// The mesh containers of the package, by the census hash, written under `out/ms/`.
fn extract(args: &[String]) -> Result<(), String> {
    let game = PathBuf::from(arg(args, "--game").ok_or("--game")?);
    let out = out_dir(args);
    std::fs::create_dir_all(out.join("ms")).map_err(|e| e.to_string())?;
    let wanted: HashSet<String> = jsonl::read(&census_dir(args).join("translate.jsonl"))
        .iter()
        .filter(|r| r["ok"] == true && r["stage"] == "Ms")
        .filter_map(|r| r["hash"].as_str().map(str::to_owned))
        .collect();
    println!("mscs extract: {} mesh programs wanted", wanted.len());
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(&game)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && e.metadata().map_or(0, |m| m.len()) <= crate::translate::MAX_FILE)
        .map(|e| e.into_path())
        .collect();
    files.sort();
    let mut got = HashSet::new();
    for f in &files {
        let Ok(data) = std::fs::read(f) else { continue };
        for s in crate::translate::sources(&data) {
            let Some(bytes) = s.bytes() else { continue };
            let hash = blake3::hash(&bytes).to_hex()[..16].to_owned();
            if wanted.contains(&hash) && got.insert(hash.clone()) {
                std::fs::write(out.join("ms").join(format!("{hash}.dxbc")), &bytes).map_err(|e| e.to_string())?;
            }
        }
    }
    println!("mscs extract: {} of {} found", got.len(), wanted.len());
    Ok(())
}

fn book_pairs(dir: &Path) -> Result<BTreeSet<(String, String)>, String> {
    let bytes = std::fs::read(dir.join("known-pipelines.bin")).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut out = BTreeSet::new();
    let mut at = 0;
    while let Some(head) = bytes.get(at..at + 4) {
        let size = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let Some(e) = bytes.get(at + 4..at + 4 + size) else { break };
        at += 4 + size;
        let Some(&count) = e.get(16) else { continue };
        let (mut ms, mut ps) = (None, None);
        let mut p = 17;
        for _ in 0..count {
            let Some(s) = e.get(p..p + 33) else { break };
            p += 33;
            let hex: String = s[1..9].iter().map(|b| format!("{b:02x}")).collect();
            match s[0] {
                7 => ms = Some(hex),
                1 => ps = Some(hex),
                _ => {}
            }
        }
        if let (Some(m), Some(p)) = (ms, ps) {
            out.insert((m, p));
        }
    }
    Ok(out)
}

fn comp(c: u32) -> Component {
    match c {
        1 => Component::Uint,
        2 => Component::Sint,
        _ => Component::Float,
    }
}

fn short(e: &MscError) -> String {
    let (_, class) = classify(e);
    let text: String = e.to_string().replace('\n', " ").chars().take(160).collect();
    format!("{class} :: {text}")
}

/// One mesh program (and its pixel shader, real or the default one): the row.
fn one(
    device: &Device,
    key: &str,
    container: &[u8],
    ps: Option<&HostShader>,
    default_ps: &[u8],
    mark: &dyn Fn(&str),
) -> Value {
    let mut row = json!({"key": key});
    mark("translate");
    let reads = ps.map(|p| p.vertex_reads.clone()).unwrap_or_default();
    let started = Instant::now();
    let made = std::panic::catch_unwind(|| victory_shader::emit::translate_with(container, &reads));
    row["translate_ms"] = json!(started.elapsed().as_millis() as u64);
    let ms = match made {
        Ok(Ok(m)) => m,
        Ok(Err(e)) => {
            row["vmcs"] = json!(false);
            row["why"] = json!(format!("mesh translation: {e}"));
            return row;
        }
        Err(_) => {
            row["vmcs"] = json!(false);
            row["why"] = json!("mesh translation: PANIC");
            return row;
        }
    };
    let bundle = match Bundle::find(&ms.dxil) {
        Ok(Some(b)) => b,
        Ok(None) => {
            // Why: the compute form's own verdict.
            row["vmcs"] = json!(false);
            row["why"] = match victory_shader::emit::translate_mesh_compute(container, &reads) {
                Ok(_) => json!("compute form made but no part"),
                Err(e) => json!(e.to_string()),
            };
            return row;
        }
        Err(e) => {
            row["vmcs"] = json!(false);
            row["why"] = json!(e);
            return row;
        }
    };
    row["vmcs"] = json!(true);
    row["cs_bytes"] = json!(bundle.compute.len());
    row["vs_bytes"] = json!(bundle.vertex.len());
    let layout = match Layout::parse(bundle.layout) {
        Ok(l) => l,
        Err(e) => {
            row["cs"] = json!(format!("layout: {e}"));
            return row;
        }
    };
    row["group_bytes"] = json!(layout.group_stride);
    let mut stages: Vec<(&HostShader, _)> = vec![(&ms, visibility(ms.stage))];
    if let Some(p) = ps {
        stages.push((p, visibility(p.stage)));
    }
    let root = root_signature(&stages, false);
    // The compute form.
    mark("cs");
    let mut cs_root = extend_root(&root, &layout, true, true);
    // (`extend_root` makes the parameters visible to the compute stage, not the static samplers: with
    // `--mesh-samplers` they stay as the mesh shader had them, which is what the back does today.)
    if !std::env::var("MSCS_MESH_SAMPLERS").is_ok() {
        for s in &mut cs_root.static_samplers {
            s.visibility = victory_metal_back::msc::Visibility::All;
        }
    }
    let started = Instant::now();
    match device.compute_pipeline_from_dxil(bundle.compute, Some(&cs_root), &CompileOptions::default()) {
        Ok(made) => {
            row["cs"] = json!(if made.reflection.threadgroup_size == layout.threads {
                "ok".to_owned()
            } else {
                format!("threads {:?} != layout {:?}", made.reflection.threadgroup_size, layout.threads)
            })
        }
        Err(e) => row["cs"] = json!(short(&e)),
    }
    row["cs_ms"] = json!(started.elapsed().as_millis() as u64);
    // The fetch vertex program with the pixel shader.
    mark("vs");
    let (ps_dxil, targets, depth): (&[u8], Vec<Component>, bool) = match ps {
        Some(p) => {
            let (_, outputs) = crate::sig::of_container(&p.dxil);
            let mut targets: Vec<Component> = Vec::new();
            for e in outputs.iter().filter(|e| e.name.eq_ignore_ascii_case("SV_Target")) {
                let slot = e.index as usize;
                if targets.len() <= slot {
                    targets.resize(slot + 1, Component::Float);
                }
                targets[slot] = comp(e.comp);
            }
            let depth = outputs.iter().any(|e| e.name.starts_with("SV_Depth") || e.name == "SV_StencilRef");
            (&p.dxil, targets, depth)
        }
        None => (default_ps, vec![Component::Float], false),
    };
    let gfx_root = extend_root(&root, &layout, false, false);
    let desc = default_graphics(
        &gfx_root,
        Stages { vs: Some(bundle.vertex), ps: Some(ps_dxil), ..Stages::default() },
        &[],
        &targets,
        depth,
        Variant::Plain,
    );
    let started = Instant::now();
    match device.graphics_pipeline_from_dxil(&desc) {
        Ok(_) => row["vs"] = json!("ok"),
        Err(e) => row["vs"] = json!(short(&e)),
    }
    row["vs_ms"] = json!(started.elapsed().as_millis() as u64);
    row
}

fn items(args: &[String], out: &Path) -> Result<Vec<(String, String, Option<String>)>, String> {
    // (key, mesh hash, pixel hash)
    let only: Option<HashSet<String>> = arg(args, "--only").map(|l| {
        std::fs::read_to_string(&l)
            .unwrap_or(l)
            .split([',', '\n'])
            .filter_map(|x| x.split_whitespace().next().map(str::to_owned))
            .collect()
    });
    let mut v: Vec<(String, String, Option<String>)> = if flag(args, "--pairs") {
        let book = arg(args, "--book").map(PathBuf::from).unwrap_or_else(|| {
            project_root().join("victory-data-drops-warm").join("cache").join("pipelines")
        });
        book_pairs(&book)?
            .into_iter()
            .filter(|(m, _)| out.join("ms").join(format!("{m}.dxbc")).exists())
            .map(|(m, p)| (format!("{m}:{p}"), m, Some(p)))
            .collect()
    } else {
        let mut hashes: Vec<String> = std::fs::read_dir(out.join("ms"))
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".dxbc").map(str::to_owned))
            .collect();
        hashes.sort();
        hashes.into_iter().map(|h| (h.clone(), h, None)).collect()
    };
    if let Some(o) = &only {
        v.retain(|(k, ..)| o.contains(k));
    }
    if let Some((k, n)) = arg(args, "--shard").as_deref().and_then(|s| s.split_once('/')) {
        let (k, n): (usize, usize) = (k.parse().map_err(|_| "--shard K/N")?, n.parse().map_err(|_| "--shard K/N")?);
        let mut i = 0;
        v.retain(|_| { i += 1; (i - 1) % n == k });
    }
    let every: usize = arg(args, "--every").and_then(|e| e.parse().ok()).unwrap_or(1).max(1);
    let limit: usize = arg(args, "--limit").and_then(|l| l.parse().ok()).unwrap_or(usize::MAX);
    Ok(v.into_iter().enumerate().filter(|(i, _)| i % every == 0).map(|(_, x)| x).take(limit).collect())
}

fn rows_path(args: &[String], out: &Path) -> PathBuf {
    let tag = arg(args, "--tag").unwrap_or_default();
    out.join(format!("mscs{}{tag}.jsonl", if flag(args, "--pairs") { "-pairs" } else { "" }))
}

fn run_items(args: &[String]) -> Result<(), String> {
    let out = out_dir(args);
    if flag(args, "--isolate") {
        return isolate(args, &out);
    }
    // A census finds what aborts: the back's denylist must not hide it.
    // SAFETY: before any thread of this process starts.
    unsafe { std::env::remove_var(victory_metal_back::msc::guard::DENYLIST_ENV) };
    host_settings();
    std::panic::set_hook(Box::new(|_| {}));
    let census = census_dir(args);
    let device = Device::new().map_err(|e| e.to_string())?;
    let dxc = Dxc::new(&out)?;
    let default_ps = dxc.compile("ps_6_6", dxc::DEFAULT_PS)?;
    let path = rows_path(args, &out);
    let resume = flag(args, "--resume");
    if !resume {
        std::fs::write(&path, "").map_err(|e| e.to_string())?;
    }
    let seen = jsonl::seen(&path, "key");
    let rows = jsonl::Appender::open(&path)?;
    let mark_file = arg(args, "--mark").map(PathBuf::from);
    let todo: Vec<_> = items(args, &out)?.into_iter().filter(|(k, ..)| !seen.contains(k)).collect();
    println!("mscs run: {} items ({} threads)", todo.len(), jobs(args));
    let next = std::sync::atomic::AtomicUsize::new(0);
    // `--budget SECONDS`: stop taking items after that long (a rerun with `--resume` goes on).
    let budget = arg(args, "--budget").and_then(|b| b.parse().ok()).map(std::time::Duration::from_secs);
    let started = Instant::now();
    let threads = jobs(args).min(todo.len().max(1));
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                while let Some((key, m, p)) = todo.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) {
                    if budget.is_some_and(|b| started.elapsed() > b) {
                        return;
                    }
                    let Ok(container) = std::fs::read(out.join("ms").join(format!("{m}.dxbc"))) else { continue };
                    let ps = p.as_ref().and_then(|p| {
                        wire::decode(&std::fs::read(census.join("vsh").join(format!("{p}.vsh"))).ok()?)
                    });
                    if p.is_some() && ps.is_none() {
                        rows.push(&json!({"key": key, "vmcs": false, "why": "no translation of the pixel shader"}));
                        continue;
                    }
                    let mark = |stage: &str| {
                        if let Some(f) = &mark_file {
                            let _ = std::fs::write(f, format!("{key}\t{stage}"));
                        }
                    };
                    let row = one(&device, key, &container, ps.as_ref(), &default_ps, &mark);
                    rows.push(&row);
                }
            });
        }
    });
    rows.flush();
    summary(&path);
    Ok(())
}

fn summary(path: &Path) {
    let rows = jsonl::read(path);
    let mut why: BTreeMap<String, usize> = BTreeMap::new();
    let (mut with, mut cs, mut vs, mut both) = (0, 0, 0, 0);
    for r in &rows {
        if r["vmcs"] == true {
            with += 1;
            let (c, v) = (r["cs"] == "ok", r["vs"] == "ok");
            cs += usize::from(c);
            vs += usize::from(v);
            both += usize::from(c && v);
        } else {
            let w = r["why"].as_str().unwrap_or("?");
            *why.entry(w.chars().take(80).collect()).or_default() += 1;
        }
    }
    println!(
        "mscs: {} rows; translated with a VMCS part {with}; compute pipeline ok {cs}; fetch+PS pipeline ok {vs}; both {both}",
        rows.len()
    );
    for (w, n) in why {
        println!("  no part {n:5}  {w}");
    }
}

fn isolate(args: &[String], out: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let path = rows_path(args, out);
    let tag = arg(args, "--tag").unwrap_or_default();
    let mark = out.join(format!("current-mscs{tag}.txt"));
    let errs = out.join(format!("current-mscs{tag}.stderr"));
    let limit = std::time::Duration::from_secs(arg(args, "--timeout").and_then(|t| t.parse().ok()).unwrap_or(300));
    if !flag(args, "--resume") {
        let _ = std::fs::write(&path, "");
    }
    let mut aborts = 0;
    loop {
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("mscs").arg("run");
        let mut skip = false;
        for a in args {
            if skip {
                skip = false;
            } else if a == "--isolate" || a == "--jobs" || a == "--timeout" {
                skip = a != "--isolate";
            } else {
                cmd.arg(a);
            }
        }
        cmd.args(["--jobs", "1", "--resume", "--mark"]).arg(&mark);
        let _ = std::fs::remove_file(&mark);
        cmd.stderr(std::fs::File::create(&errs).map_err(|e| e.to_string())?);
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        let mut last = (String::new(), Instant::now());
        let status = loop {
            if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
                break Some(s);
            }
            let now = std::fs::read_to_string(&mark).unwrap_or_default();
            if now != last.0 {
                last = (now, Instant::now());
            } else if last.1.elapsed() > limit {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        let said = std::fs::read_to_string(&errs).unwrap_or_default();
        if status.is_some_and(|s| s.success()) {
            break;
        }
        let Ok(m) = std::fs::read_to_string(&mark) else {
            eprint!("{said}");
            return Err(format!("the child failed without naming a shader: {status:?}"));
        };
        let (key, stage) = m.split_once('\t').unwrap_or((m.as_str(), "?"));
        let fatal = said.lines().find(|l| l.contains("metal-shaderconverter") && l.contains("Fatal")).unwrap_or("");
        let class = match (status, fatal.is_empty()) {
            (None, _) => "ProcessTimeout",
            (Some(_), false) => "MscFatal",
            _ => "ProcessAbort",
        };
        aborts += 1;
        let mut row = json!({"key": key, "vmcs": true, "abort_stage": stage, "abort": class, "abort_text": fatal.chars().take(200).collect::<String>()});
        // What the stage before the abort made is lost with the child; the stages after it were not tried.
        match stage {
            "translate" => {
                row["vmcs"] = json!(false);
                row["why"] = json!(class);
            }
            "cs" => row["cs"] = json!(class),
            "vs" => {
                row["vs"] = json!(class);
                row["cs"] = json!("not recorded (the child died after it)");
            }
            _ => {}
        }
        let rows = jsonl::Appender::open(&path)?;
        rows.push(&row);
        rows.flush();
    }
    println!("mscs isolate: {aborts} aborts recorded");
    summary(&path);
    Ok(())
}
