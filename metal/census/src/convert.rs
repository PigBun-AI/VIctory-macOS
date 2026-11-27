//! Stage 2 and 3: every translated shader through Metal Shader Converter and
//! into Metal, one pipeline kind at a time.
//!
//! | kind | what is built |
//! |---|---|
//! | `cs` | MSC with the host's root signature, library, compute pipeline |
//! | `vs` | vertex shader + default pixel shader (one constant output): render pipeline |
//! | `ps` | pixel shader + a full-screen-triangle vertex shader that writes the pixel shader's inputs: render pipeline, targets as the pixel shader writes them |
//! | `ms` | mesh shader + the default pixel shader: mesh render pipeline (no amplification program exists in the package) |
//! | `gs` | vertex shader + the translator's rectangle-list geometry shader (`rect_list_gs`) + default pixel shader: geometry pipeline (the back emulates it with an object and a mesh stage) |
//!
//! Rows go to `convert-<kind>.jsonl` under `--out`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use serde_json::{Value, json};
use victory_metal_back::archive;
use victory_metal_back::device::Device;
use victory_metal_back::msc::{Compat, CompileOptions, Msc, MscError, ShaderStage};
use victory_metal_back::pipeline_defaults::{
    Component, DefaultInput, Stages, Variant, default_graphics,
};
use victory_metal_back::render::prepare_graphics;
use victory_shader::emit::{HostShader, wire};

use crate::dxc::{self, Dxc};
use crate::root::{root_signature, visibility};
use crate::sig::{self, Element};
use crate::{arg, flag, jobs, jsonl, out_dir};

/// The kinds, in the order they run.
const KINDS: [&str; 5] = ["cs", "vs", "ps", "ms", "gs"];

fn kind_stage(kind: &str) -> &'static str {
    match kind {
        "cs" => "Cs",
        "ps" => "Ps",
        "ms" => "Ms",
        _ => "Vs",
    }
}

/// Where a failure happened and what kind it is, from the backend's error.
pub fn classify(e: &MscError) -> (&'static str, String) {
    match e {
        MscError::Library { .. } => ("setup", "MscLibrary".into()),
        MscError::RootSignature { code, .. } => ("msc", format!("RootSignature:{}", code.name())),
        MscError::Rejected { code, .. } => ("msc", format!("Rejected:{}", code.name())),
        MscError::NoOutput(_) => ("msc", "NoOutput".into()),
        MscError::BadInput(_) => ("msc", "BadInput".into()),
        MscError::Mismatch(_) => ("msc", "StageMismatch".into()),
        MscError::Limit(m) => ("metal-limit", limit_class(m)),
        MscError::Unsupported(m) => ("backend", format!("Unsupported:{}", first_words(m))),
        MscError::Metal(inner) => {
            let m = inner.to_string();
            if m.starts_with("metallib") {
                ("metal-library", "LibraryLoad".into())
            } else {
                ("metal-pipeline", "PipelineCompile".into())
            }
        }
    }
}

/// The limit a Metal limit message names, with its numbers left out so that
/// the messages of one limit group.
fn limit_class(m: &str) -> String {
    let m = &m.to_lowercase();
    for k in [
        "mesh threads per group",
        "amplification threads per group",
        "mesh output vertices",
        "mesh output primitives",
        "payload",
        "total mesh size",
        "mesh output",
    ] {
        if m.contains(k) {
            return k.replace(' ', "-");
        }
    }
    "other".into()
}

fn first_words(m: &str) -> String {
    m.split(|c: char| c.is_ascii_digit())
        .next()
        .unwrap_or("")
        .trim()
        .chars()
        .take(60)
        .collect()
}

fn one_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(400).collect()
}

struct Ctx<'a> {
    device: &'a Device,
    msc: Msc,
    dxc: Dxc,
    default_ps: Vec<u8>,
    variant: Variant,
    compat: Compat,
}

fn comp(c: u32) -> Component {
    match c {
        1 => Component::Uint,
        2 => Component::Sint,
        _ => Component::Float,
    }
}

/// The step timings of one item, milliseconds.
struct Timing {
    msc: f64,
    lib: f64,
    pso: f64,
}

impl Timing {
    fn put(&self, row: &mut Value) {
        row["msc_ms"] = json!(self.msc);
        row["lib_ms"] = json!(self.lib);
        row["pso_ms"] = json!(self.pso);
    }
}

fn fail(row: &mut Value, t: &Timing, e: &MscError) {
    let (stage, class) = classify(e);
    row["ok"] = json!(false);
    row["fail_stage"] = json!(stage);
    row["fail_class"] = json!(class);
    row["error"] = json!(one_line(&e.to_string()));
    t.put(row);
}

fn compute(ctx: &Ctx, hash: &str, s: &HostShader) -> Value {
    let mut row = json!({"hash": hash, "kind": "cs"});
    let mut t = Timing {
        msc: 0.0,
        lib: 0.0,
        pso: 0.0,
    };
    let root = root_signature(&[(s, visibility(s.stage))], true);
    let started = Instant::now();
    let converted = ctx.msc.compile(
        &s.dxil,
        Some(&root),
        &CompileOptions {
            compat: ctx.compat,
            ..CompileOptions::default()
        },
    );
    t.msc = started.elapsed().as_secs_f64() * 1000.0;
    let converted = match converted {
        Ok(c) => c,
        Err(e) => {
            fail(&mut row, &t, &e);
            return row;
        }
    };
    row["metallib_bytes"] = json!(converted.metallib.len());
    row["dxil_bytes"] = json!(s.dxil.len());
    if converted.reflection.stage != ShaderStage::Compute {
        let e = MscError::BadInput(format!(
            "not a compute shader: {:?}",
            converted.reflection.stage
        ));
        fail(&mut row, &t, &e);
        return row;
    }
    let started = Instant::now();
    let lib = ctx.device.library_from_metallib(&converted.metallib);
    t.lib = started.elapsed().as_secs_f64() * 1000.0;
    let lib = match lib {
        Ok(l) => l,
        Err(e) => {
            fail(&mut row, &t, &MscError::Metal(e));
            return row;
        }
    };
    let started = Instant::now();
    let pso = ctx
        .device
        .compute_pipeline(&lib, &converted.reflection.entry);
    t.pso = started.elapsed().as_secs_f64() * 1000.0;
    match pso {
        Ok(_) => {
            row["ok"] = json!(true);
            t.put(&mut row);
        }
        Err(e) => fail(&mut row, &t, &MscError::Metal(e)),
    }
    row
}

/// The graphics kinds: one pipeline with a default partner stage.
fn graphics(ctx: &Ctx, kind: &str, hash: &str, s: &HostShader) -> Value {
    let mut row = json!({"hash": hash, "kind": kind});
    let mut t = Timing {
        msc: 0.0,
        lib: 0.0,
        pso: 0.0,
    };
    let (inputs, outputs): (Vec<Element>, Vec<Element>) = sig::of_container(&s.dxil);
    row["dxil_bytes"] = json!(s.dxil.len());
    // The partner stage and the render targets.
    let (partner, targets, depth): (Vec<u8>, Vec<Component>, bool) = if kind == "ps" {
        let vs = match ctx.dxc.compile("vs_6_6", &dxc::vertex_for(&inputs)) {
            Ok(v) => v,
            Err(why) => {
                row["ok"] = json!(false);
                row["fail_stage"] = json!("census");
                row["fail_class"] = json!("DefaultStage");
                row["error"] = json!(one_line(&why));
                return row;
            }
        };
        let mut targets: Vec<Component> = Vec::new();
        for e in outputs
            .iter()
            .filter(|e| e.name.eq_ignore_ascii_case("SV_Target"))
        {
            let slot = e.index as usize;
            if targets.len() <= slot {
                targets.resize(slot + 1, Component::Float);
            }
            targets[slot] = comp(e.comp);
        }
        let depth = outputs
            .iter()
            .any(|e| e.name.starts_with("SV_Depth") || e.name == "SV_StencilRef");
        (vs, targets, depth)
    } else {
        (ctx.default_ps.clone(), vec![Component::Float], false)
    };
    row["targets"] = json!(targets.len());
    // Vertex inputs (vertex shaders): typed as the signature says.
    let vs_inputs: Vec<DefaultInput> = if kind == "vs" || kind == "gs" {
        inputs
            .iter()
            .filter(|e| e.system_value == 0)
            .map(|e| DefaultInput {
                semantic: e.name.clone(),
                index: e.index,
                component: comp(e.comp),
                width: e.width(),
            })
            .collect()
    } else {
        Vec::new()
    };
    // The translator's rectangle-list geometry shader of this vertex shader.
    let rect_gs = if kind == "gs" {
        let started = Instant::now();
        let made = victory_shader::emit::rect_list_gs(&s.dxil, None);
        row["gen_ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
        match made {
            Ok(g) => Some(g),
            Err(e) => {
                row["ok"] = json!(false);
                row["fail_stage"] = json!("translate");
                row["fail_class"] = json!("RectListGs");
                row["error"] = json!(one_line(&e.to_string()));
                return row;
            }
        }
    } else {
        None
    };
    let root = root_signature(&[(s, visibility(s.stage))], false);
    let stages = match kind {
        "vs" => Stages {
            vs: Some(&s.dxil),
            ps: Some(&partner),
            ..Stages::default()
        },
        "gs" => Stages {
            vs: Some(&s.dxil),
            ps: Some(&partner),
            gs: rect_gs.as_deref(),
            ..Stages::default()
        },
        "ms" => Stages {
            ps: Some(&partner),
            ms: Some(&s.dxil),
            ..Stages::default()
        },
        _ => Stages {
            vs: Some(&partner),
            ps: Some(&s.dxil),
            ..Stages::default()
        },
    };
    let mut desc = default_graphics(&root, stages, &vs_inputs, &targets, depth, ctx.variant);
    desc.compat = ctx.compat;
    let started = Instant::now();
    let prepared = prepare_graphics(&desc);
    t.msc = started.elapsed().as_secs_f64() * 1000.0;
    let prepared = match prepared {
        Ok(p) => p,
        Err(e) => {
            fail(&mut row, &t, &e);
            return row;
        }
    };
    let started = Instant::now();
    let pso = ctx.device.graphics_pipeline(&prepared, &desc);
    t.pso = started.elapsed().as_secs_f64() * 1000.0;
    match pso {
        Ok(_) => {
            row["ok"] = json!(true);
            t.put(&mut row);
        }
        Err(e) => fail(&mut row, &t, &e),
    }
    row
}

fn items(
    out: &std::path::Path,
    kind: &str,
    limit: usize,
    every: usize,
    only: Option<&std::collections::HashSet<String>>,
) -> Vec<(String, HostShader)> {
    let want = kind_stage(kind);
    let mut hashes: Vec<String> = jsonl::read(&out.join("translate.jsonl"))
        .iter()
        .filter(|r| r["ok"] == true && r["stage"] == want)
        .filter_map(|r| r["hash"].as_str().map(str::to_owned))
        .collect();
    hashes.sort();
    if let Some(only) = only {
        hashes.retain(|h| only.contains(h));
    }
    hashes
        .into_iter()
        .enumerate()
        .filter(|(i, _)| i % every == 0)
        .take(limit)
        .filter_map(|(_, h)| {
            let bytes = std::fs::read(out.join("vsh").join(format!("{h}.vsh"))).ok()?;
            wire::decode(&bytes).map(|s| (h, s))
        })
        .collect()
}

/// Runs the conversion in a child process (one thread), and when Metal
/// aborts the child on an assertion (it does not return an error), records
/// the shader it was on as a failure and starts again after it.
fn isolate(args: &[String], out: &std::path::Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let kind = arg(args, "--kind").ok_or("--isolate needs one --kind")?;
    let tag = arg(args, "--tag").unwrap_or_default();
    let rows_path = out.join(format!("convert-{kind}{tag}.jsonl"));
    let mark = out.join(format!("current-{kind}{tag}.txt"));
    let _ = std::fs::write(&rows_path, "");
    let mut crashes = 0;
    loop {
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("convert");
        let mut skip = false;
        for a in args {
            if skip {
                skip = false;
            } else if a == "--isolate" || a == "--jobs" {
                skip = a == "--jobs";
            } else {
                cmd.arg(a);
            }
        }
        cmd.args(["--jobs", "1", "--resume", "--mark"]).arg(&mark);
        let _ = std::fs::remove_file(&mark);
        let status = cmd.status().map_err(|e| e.to_string())?;
        if status.success() {
            break;
        }
        let Ok(hash) = std::fs::read_to_string(&mark) else {
            return Err(format!(
                "the child failed without naming a shader: {status}"
            ));
        };
        crashes += 1;
        let rows = jsonl::Appender::open(&rows_path)?;
        rows.push(&json!({
            "hash": hash.trim(), "kind": kind, "ok": false, "fail_stage": "metal-abort",
            "fail_class": "ProcessAbort",
            "error": format!("the process aborted ({status}) while building this pipeline"),
        }));
        rows.flush();
    }
    println!("isolate {kind}: {crashes} aborts recorded");
    Ok(())
}

pub fn run(args: &[String]) -> Result<(), String> {
    let out = out_dir(args);
    let kinds: Vec<String> = match arg(args, "--kind").as_deref() {
        None | Some("all") => KINDS.iter().map(|k| (*k).to_owned()).collect(),
        Some(list) => list.split(',').map(str::to_owned).collect(),
    };
    for k in &kinds {
        if !KINDS.contains(&k.as_str()) {
            return Err(format!("unknown kind `{k}`"));
        }
    }
    if flag(args, "--isolate") {
        return isolate(args, &out);
    }
    let resume = flag(args, "--resume");
    let mark = arg(args, "--mark").map(PathBuf::from);
    let threads = jobs(args);
    let limit: usize = arg(args, "--limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(usize::MAX);
    // Every n-th shader only (a sample for the serial timing pass).
    let every: usize = arg(args, "--every")
        .and_then(|l| l.parse().ok())
        .unwrap_or(1)
        .max(1);
    let tag = arg(args, "--tag").unwrap_or_default();
    // `--only LIST`: just these hashes. LIST is hashes separated by commas, or a file with one hash
    // at the start of each line (a `mac-side.tsv` line works).
    let only: Option<std::collections::HashSet<String>> = match arg(args, "--only") {
        None => None,
        Some(list) => {
            let text = std::fs::read_to_string(&list).unwrap_or(list);
            Some(
                text.split([',', '\n'])
                    .filter_map(|l| l.split_whitespace().next())
                    .map(str::to_owned)
                    .collect(),
            )
        }
    };
    let variant = if flag(args, "--blended") {
        Variant::Blended
    } else {
        Variant::Plain
    };
    // `--compat bounds`: bounds-checked buffer and texture access, which changes every
    // converted library: pipelines no earlier run compiled (the cold half of the archive test).
    let compat = match arg(args, "--compat").as_deref() {
        None => Compat::NONE,
        Some("bounds") => Compat::BOUNDS_CHECK,
        Some(other) => return Err(format!("unknown --compat `{other}`")),
    };
    let lookup = arg(args, "--lookup").map(PathBuf::from);
    let archive_dir = flag(args, "--archive").then(|| {
        arg(args, "--archive-dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                crate::project_root()
                    .join("victory-data")
                    .join("metal-archive")
            })
    });
    if archive_dir.is_some() {
        archive::arm_capture();
    }
    let device = Device::new().map_err(|e| e.to_string())?;
    let msc = Msc::get().map_err(|e| e.to_string())?;
    let dxc = Dxc::new(&out)?;
    let default_ps = dxc.compile("ps_6_6", dxc::DEFAULT_PS)?;
    let mut meta = json!({"jobs": threads, "every": every, "compat": format!("{compat:?}"), "variant": format!("{variant:?}")});
    if let Some(path) = &lookup {
        let started = Instant::now();
        let a = archive::open(&device, path).map_err(|e| e.to_string())?;
        meta["archive_open_ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
        meta["archive_bytes"] = json!(std::fs::metadata(path).map_or(0, |m| m.len()));
        archive::set_lookup(&[&a]);
    }
    let ctx = Ctx {
        device: &device,
        msc,
        dxc,
        default_ps,
        variant,
        compat,
    };
    println!(
        "convert: {} on {} ({} threads{})",
        kinds.join(","),
        device.caps().name,
        threads,
        if variant == Variant::Blended {
            ", blended state"
        } else {
            ""
        }
    );
    for kind in &kinds {
        let mut all = items(&out, kind, limit, every, only.as_ref());
        let rows_path = out.join(format!("convert-{kind}{tag}.jsonl"));
        if resume {
            let done = jsonl::seen(&rows_path, "hash");
            all.retain(|(h, _)| !done.contains(h));
        } else {
            std::fs::write(&rows_path, "").map_err(|e| e.to_string())?;
        }
        let rows = jsonl::Appender::open(&rows_path)?;
        let next = AtomicUsize::new(0);
        let started = Instant::now();
        std::thread::scope(|s| {
            for _ in 0..threads.min(all.len().max(1)) {
                s.spawn(|| {
                    while let Some((hash, shader)) = all.get(next.fetch_add(1, Ordering::Relaxed)) {
                        if let Some(m) = &mark {
                            let _ = std::fs::write(m, hash);
                        }
                        let row = match kind.as_str() {
                            "cs" => compute(&ctx, hash, shader),
                            k => graphics(&ctx, k, hash, shader),
                        };
                        rows.push(&row);
                        let n = next.load(Ordering::Relaxed);
                        if n.is_multiple_of(500) {
                            println!(
                                "  {kind}: {n} / {}, {:.0} s",
                                all.len(),
                                started.elapsed().as_secs_f64()
                            );
                        }
                    }
                });
            }
        });
        rows.flush();
        let done = jsonl::read(&rows_path);
        let ok = done.iter().filter(|r| r["ok"] == true).count();
        println!(
            "convert {kind}: {} shaders, {ok} ok, {} failed, {:.0} s",
            done.len(),
            done.len() - ok,
            started.elapsed().as_secs_f64()
        );
    }
    if let Some(dir) = archive_dir {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(
            arg(args, "--archive-name")
                .unwrap_or_else(|| format!("{}{tag}.mtl4a", kinds.join("-"))),
        );
        let _ = std::fs::remove_file(&path);
        let started = Instant::now();
        archive::flush(&path).map_err(|e| e.to_string())?;
        let size = std::fs::metadata(&path).map_or(0, |m| m.len());
        println!(
            "archive: {} ({} bytes), flushed in {:.1} s",
            path.display(),
            size,
            started.elapsed().as_secs_f64()
        );
        meta["archive_flush_s"] = json!(started.elapsed().as_secs_f64());
        meta["archive_bytes"] = json!(size);
        meta["archive_file"] = json!(path.file_name().map(|n| n.to_string_lossy().into_owned()));
    }
    meta["kinds"] = json!(kinds);
    std::fs::write(
        out.join(format!("convert-meta{tag}.json")),
        meta.to_string(),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
