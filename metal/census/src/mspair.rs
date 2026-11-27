//! `victory-census mspair`: the mesh pipelines the title really made, from
//! its pipeline book (`VICTORY_PIPELINE_BOOK=1`: `known-pipelines.bin` and
//! `known-shaders.bin` in the cache folder, written by
//! `crates/victory-gpu/src/pipeline/known.rs`; read only), built in Metal
//! with the real pixel shader, for the mesh shaders refused for their total
//! output size.
//!
//! Per (mesh shader, pixel shader) pair: as the translator made it, with the
//! outputs packed (`mesh_fit`, no pixel shader known), and with the outputs
//! the pixel shader does not read dropped too. Mesh shaders the book never
//! pairs are listed apart (the title may never use them).
//!
//! Options: `--book DIR` (default `victory-data-drops-warm/cache/pipelines`),
//! `--hashes FILE` (default: the meshes of `convert-ms.jsonl` that Metal
//! refused for their total size), `--all` (every mesh pipeline of the book),
//! `--jobs N`, `--tag T`. Rows go to `mspair<T>.jsonl`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use victory_metal_back::device::Device;
use victory_metal_back::mesh_fit::{self, Consumer};
use victory_metal_back::pipeline_defaults::{Component, Stages, Variant, default_graphics};
use victory_shader::emit::{HostShader, wire};

use crate::convert::classify;
use crate::root::{root_signature, visibility};
use crate::sig;
use crate::{arg, flag, jobs, jsonl, out_dir, project_root};

const MESH: u8 = 7;
const PIXEL: u8 = 1;

/// The mesh pipelines of the book: (mesh hash, pixel hash or none), 16 hex
/// digits each (the census names a container by the first 16 digits of its
/// blake3).
fn book_pairs(dir: &Path) -> Result<BTreeSet<(String, Option<String>)>, String> {
    let bytes = std::fs::read(dir.join("known-pipelines.bin"))
        .map_err(|e| format!("{}: {e}", dir.join("known-pipelines.bin").display()))?;
    let mut out = BTreeSet::new();
    let mut at = 0;
    while let Some(head) = bytes.get(at..at + 4) {
        let size = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let Some(e) = bytes.get(at + 4..at + 4 + size) else {
            break;
        };
        at += 4 + size;
        let Some(&count) = e.get(16) else { continue };
        let (mut ms, mut ps) = (None, None);
        let mut p = 17;
        for _ in 0..count {
            let Some(s) = e.get(p..p + 33) else { break };
            p += 33;
            let hex: String = s[1..9].iter().map(|b| format!("{b:02x}")).collect();
            match s[0] {
                MESH => ms = Some(hex),
                PIXEL => ps = Some(hex),
                _ => {}
            }
        }
        if let Some(m) = ms {
            out.insert((m, ps));
        }
    }
    Ok(out)
}

fn shader(out: &Path, hash: &str) -> Option<HostShader> {
    let bytes = std::fs::read(out.join("vsh").join(format!("{hash}.vsh"))).ok()?;
    wire::decode(&bytes)
}

struct Built {
    ok: bool,
    class: String,
    stride: Option<u32>,
    dropped: usize,
}

fn comp(c: u32) -> Component {
    match c {
        1 => Component::Uint,
        2 => Component::Sint,
        _ => Component::Float,
    }
}

/// A mesh pipeline of `ms` (a container) and `ps` made like the census's own.
fn build(
    device: &Device,
    ms_shader: &HostShader,
    ms: &[u8],
    ps_shader: Option<&HostShader>,
) -> Result<(), victory_metal_back::msc::MscError> {
    let mut stages: Vec<(&HostShader, _)> = vec![(ms_shader, visibility(ms_shader.stage))];
    let mut targets: Vec<Component> = Vec::new();
    let mut depth = false;
    if let Some(p) = ps_shader {
        stages.push((p, visibility(p.stage)));
        let (_, outputs) = sig::of_container(&p.dxil);
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
        depth = outputs
            .iter()
            .any(|e| e.name.starts_with("SV_Depth") || e.name == "SV_StencilRef");
    }
    let root = root_signature(&stages, false);
    let desc = default_graphics(
        &root,
        Stages {
            ps: ps_shader.map(|p| p.dxil.as_slice()),
            ms: Some(ms),
            ..Stages::default()
        },
        &[],
        &targets,
        depth,
        Variant::Plain,
    );
    device.graphics_pipeline_from_dxil(&desc).map(|_| ())
}

fn attempt(
    device: &Device,
    ms: &HostShader,
    ps: Option<&HostShader>,
    consumer: Consumer<'_>,
) -> Built {
    let (dxil, stride, dropped) = match mesh_fit::repack_outputs(&ms.dxil, consumer) {
        Ok(Some(f)) => (f.dxil, Some(f.stride_after), f.dropped),
        Ok(None) => (ms.dxil.clone(), None, 0),
        Err(e) => {
            return Built {
                ok: false,
                class: format!("repack: {e}"),
                stride: None,
                dropped: 0,
            };
        }
    };
    match build(device, ms, &dxil, ps) {
        Ok(()) => Built {
            ok: true,
            class: String::new(),
            stride,
            dropped,
        },
        Err(e) => Built {
            ok: false,
            class: classify(&e).1,
            stride,
            dropped,
        },
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let out = out_dir(args);
    let tag = arg(args, "--tag").unwrap_or_default();
    let book = arg(args, "--book").map(PathBuf::from).unwrap_or_else(|| {
        project_root()
            .join("victory-data-drops-warm")
            .join("cache")
            .join("pipelines")
    });
    let wanted: Option<HashSet<String>> = if flag(args, "--all") {
        None
    } else if let Some(list) = arg(args, "--hashes") {
        let text = std::fs::read_to_string(&list).map_err(|e| format!("{list}: {e}"))?;
        Some(
            text.lines()
                .filter_map(|l| l.split('\t').next())
                .filter(|h| h.len() == 16)
                .map(str::to_owned)
                .collect(),
        )
    } else {
        Some(
            jsonl::read(&out.join("convert-ms.jsonl"))
                .iter()
                .filter(|r| {
                    r["ok"] == false
                        && r["fail_class"]
                            .as_str()
                            .is_some_and(|c| c.contains("total-mesh-size"))
                })
                .filter_map(|r| r["hash"].as_str().map(str::to_owned))
                .collect(),
        )
    };
    let pairs: Vec<(String, Option<String>)> = book_pairs(&book)?
        .into_iter()
        .filter(|(m, _)| wanted.as_ref().is_none_or(|w| w.contains(m)))
        .collect();
    let device = Device::new().map_err(|e| e.to_string())?;
    let rows_path = out.join(format!("mspair{tag}.jsonl"));
    std::fs::write(&rows_path, "").map_err(|e| e.to_string())?;
    let rows = jsonl::Appender::open(&rows_path)?;
    let next = AtomicUsize::new(0);
    println!("mspair: {} pairs from {}", pairs.len(), book.display());
    std::thread::scope(|s| {
        for _ in 0..jobs(args).min(pairs.len().max(1)) {
            s.spawn(|| {
                while let Some((m, p)) = pairs.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let mut row = json!({"ms": m, "ps": p});
                    let (Some(ms), ps) =
                        (shader(&out, m), p.as_ref().and_then(|p| shader(&out, p)))
                    else {
                        row["result"] = json!("no translation of the mesh shader");
                        rows.push(&row);
                        continue;
                    };
                    if p.is_some() && ps.is_none() {
                        row["result"] = json!("no translation of the pixel shader");
                        rows.push(&row);
                        continue;
                    }
                    let plain = match build(&device, &ms, &ms.dxil, ps.as_ref()) {
                        Ok(()) => (true, String::new()),
                        Err(e) => (false, classify(&e).1),
                    };
                    row["plain_ok"] = json!(plain.0);
                    row["plain"] = json!(plain.1);
                    // The size model's verdict whatever else stops the pair (the
                    // 520 all declare 128 vertices and have no primitive outputs).
                    let consumer = ps
                        .as_ref()
                        .map_or(Consumer::NoPixelShader, |p| Consumer::Pixel(&p.dxil));
                    let model = match mesh_fit::repack_outputs(&ms.dxil, consumer) {
                        Ok(Some(f)) => Some((f.stride_after, f.dropped)),
                        _ => None,
                    };
                    row["model_pruned_stride"] = json!(model.map(|m| m.0));
                    row["model_pruned_fits"] =
                        json!(model.is_some_and(|m| 128 * u64::from(m.0) <= 32768));
                    // Not refused for its size: nothing to do for this pair.
                    if plain.0 || !plain.1.contains("total-mesh-size") {
                        row["result"] = json!(if plain.0 { "builds" } else { "other" });
                        rows.push(&row);
                        continue;
                    }
                    let packed = attempt(&device, &ms, ps.as_ref(), Consumer::Unknown);
                    row["packed_ok"] = json!(packed.ok);
                    row["packed"] = json!(packed.class);
                    row["packed_stride"] = json!(packed.stride);
                    let mut result = if packed.ok { "packed" } else { "" };
                    if !packed.ok {
                        let pruned = attempt(&device, &ms, ps.as_ref(), consumer);
                        row["pruned_ok"] = json!(pruned.ok);
                        row["pruned"] = json!(pruned.class);
                        row["pruned_stride"] = json!(pruned.stride);
                        row["dropped"] = json!(pruned.dropped);
                        result = if pruned.ok { "pruned" } else { "refused" };
                    }
                    row["result"] = json!(result);
                    rows.push(&row);
                }
            });
        }
    });
    rows.flush();
    // The summary, per pair and per mesh shader.
    let all = jsonl::read(&rows_path);
    let mut by_result: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_ms: HashMap<String, Vec<String>> = HashMap::new();
    for r in &all {
        let res = r["result"].as_str().unwrap_or("").to_owned();
        *by_result.entry(res.clone()).or_default() += 1;
        if let Some(m) = r["ms"].as_str() {
            per_ms.entry(m.to_owned()).or_default().push(res);
        }
    }
    println!("pairs by result: {by_result:?}");
    let ok = |r: &String| matches!(r.as_str(), "packed" | "pruned" | "builds");
    let all_ok = per_ms.values().filter(|v| v.iter().all(ok)).count();
    let any_ok = per_ms.values().filter(|v| v.iter().any(ok)).count();
    let mut line = format!(
        "mesh shaders in the book: {}; every pair builds {all_ok}; at least one builds {any_ok}",
        per_ms.len()
    );
    if let Some(w) = &wanted {
        let missing: Vec<&String> = w.iter().filter(|h| !per_ms.contains_key(*h)).collect();
        line += &format!(
            "; of the {} asked for, not in the book at all: {}",
            w.len(),
            missing.len()
        );
        let mut m: Vec<&&String> = missing.iter().collect();
        m.sort();
        let list = m.iter().map(|h| h.as_str()).collect::<Vec<_>>().join("\n");
        std::fs::write(out.join(format!("mspair-missing{tag}.txt")), list + "\n")
            .map_err(|e| e.to_string())?;
    }
    println!("{line}");
    Ok(())
}
