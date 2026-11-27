//! How much of the game's real graphics shaders convert and build into Metal
//! render pipelines. Ignored by default (it needs the user's shader cache,
//! which is derived from game data: read in place, never copied; only
//! aggregate numbers leave this test, per shader only an index, the stage,
//! sizes, a result class and times, never a name or a hash).
//!
//! ```text
//! VICTORY_MSC_SURVEY_CACHE=<cache dir>/shaders/<translator folder> \
//! VICTORY_MSC_SURVEY_OUT=<per-shader numbers, outside the repository> \
//! VICTORY_HELPER_DXC=... DXC_LIB=... \
//!   cargo test -p victory-metal-back --release --test gfx_survey -- --ignored --nocapture
//! ```
//!
//! Three questions, one per stage kind of the cache:
//!
//! * pixel shaders alone: MSC converts and Metal loads the library (a Metal
//!   render pipeline always needs a vertex stage, so no pipeline is made);
//! * vertex shaders with a default pixel shader (no inputs, one constant
//!   output; DXC makes it): the full pipeline, every vertex input fed as a
//!   `float4` from one buffer (the cache does not keep the input
//!   signature's component types: a first failure is retried as `uint4`, `float`, `uint`);
//! * mesh shaders with the same default pixel shader (no amplification
//!   stage in the cache): the full mesh pipeline.
//!
//! `VICTORY_MSC_SURVEY_START` / `_LIMIT` pick a range of the sorted list.
#![cfg(target_vendor = "apple")]

mod common_cache;
mod common_msc;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

use common_cache::*;
use objc2_metal::{
    MTLCompareFunction, MTLCullMode, MTLDepthClipMode, MTLPixelFormat, MTLPrimitiveTopologyClass,
    MTLStencilOperation, MTLTriangleFillMode, MTLVertexFormat, MTLWinding,
};
use victory_metal_back::device::Device;
use victory_metal_back::msc::{Compat, Msc, RootSignatureDesc};
use victory_metal_back::render::{
    ColorTarget, DepthStencil, GraphicsDesc, Raster, StencilFace, TargetBlend, VertexElement,
    prepare_graphics,
};

const DEFAULT_PS: &str = "float4 main() : SV_Target0 { return float4(0.25, 0.5, 0.75, 1.0); }\n";

fn state<'a>(
    root: &'a RootSignatureDesc,
    vs: Option<&'a [u8]>,
    ps: Option<&'a [u8]>,
    ms: Option<&'a [u8]>,
    inputs: Vec<VertexElement>,
) -> GraphicsDesc<'a> {
    let keep = StencilFace {
        fail: MTLStencilOperation::Keep,
        depth_fail: MTLStencilOperation::Keep,
        pass: MTLStencilOperation::Keep,
        compare: MTLCompareFunction::Always,
    };
    GraphicsDesc {
        vs,
        ps,
        amp: None,
        ms,
        root,
        inputs,
        targets: vec![ColorTarget {
            format: MTLPixelFormat::RGBA8Unorm,
            integer: false,
            blend: TargetBlend::opaque(),
        }],
        topology: if ms.is_some() {
            MTLPrimitiveTopologyClass::Unspecified
        } else {
            MTLPrimitiveTopologyClass::Triangle
        },
        sample_count: 1,
        sample_mask: u32::MAX,
        alpha_to_coverage: false,
        has_depth_target: false,
        depth_stencil: DepthStencil {
            depth_compare: MTLCompareFunction::Always,
            depth_write: false,
            stencil: false,
            front: keep,
            back: keep,
            read_mask: 0xFF,
            write_mask: 0xFF,
        },
        raster: Raster {
            fill: MTLTriangleFillMode::Fill,
            cull: MTLCullMode::None,
            front_winding: MTLWinding::Clockwise,
            depth_clip: MTLDepthClipMode::Clip,
            depth_bias: 0.0,
            slope_scale: 0.0,
            bias_clamp: 0.0,
        },
        compat: Compat::NONE,
    }
}

/// One element per reflected vertex input, all in slot 0.
fn elements(names: &[String], format: MTLVertexFormat) -> Vec<VertexElement> {
    names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let split = n.trim_end_matches(|c: char| c.is_ascii_digit()).len();
            VertexElement {
                semantic: n[..split].to_owned(),
                semantic_index: n[split..].parse().unwrap_or(0),
                format,
                slot: 0,
                offset: 16 * i as u32,
                per_instance: false,
                step_rate: 1,
            }
        })
        .collect()
}

#[derive(Default)]
struct Kind {
    n: usize,
    convert_ok: usize,
    library_ok: usize,
    pipeline_ok: usize,
    retried_uint: usize,
    classes: BTreeMap<String, usize>,
    /// One full message per class (the numbers in it are the point).
    samples: BTreeMap<String, String>,
    convert_ms: Vec<f64>,
    pipeline_ms: Vec<f64>,
}

impl Kind {
    fn fail(&mut self, what: &str) {
        let class: String = what
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(100)
            .collect();
        // Strip the digits so that the classes group.
        let class: String = class.chars().filter(|c| !c.is_ascii_digit()).collect();
        self.samples.entry(class.clone()).or_insert_with(|| {
            what.lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(200)
                .collect()
        });
        *self.classes.entry(class).or_default() += 1;
    }
}

fn line(k: &Kind, name: &str, wall: f64) {
    let (mut c, mut p) = (k.convert_ms.clone(), k.pipeline_ms.clone());
    println!(
        "{name}: {} shaders; converted {}; library {}; pipeline {}{}; convert ms median {:.1} p95 {:.1}; pipeline ms median {:.1} p95 {:.1}; wall {wall:.0} s",
        k.n,
        k.convert_ok,
        k.library_ok,
        k.pipeline_ok,
        if k.retried_uint > 0 {
            format!(" ({} only with another input format)", k.retried_uint)
        } else {
            String::new()
        },
        percentile(&mut c, 0.5),
        percentile(&mut c, 0.95),
        percentile(&mut p, 0.5),
        percentile(&mut p, 0.95),
    );
    for (class, n) in &k.classes {
        println!("    failure x{n}: {class}");
        println!("        e.g. {}", k.samples[class]);
    }
}

#[test]
#[ignore = "needs the user's shader cache; see the module docs"]
fn survey() {
    let out_path = std::env::var("VICTORY_MSC_SURVEY_OUT").expect("VICTORY_MSC_SURVEY_OUT");
    let dir = std::env::var("VICTORY_MSC_SURVEY_CACHE").expect("VICTORY_MSC_SURVEY_CACHE");
    let default_ps = common_msc::dxil_or_skip("survey_ps", "ps_6_6", DEFAULT_PS)
        .expect("a DXC to make the default pixel shader (VICTORY_HELPER_DXC / DXC_LIB)");
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
    let (mut ps_only, mut vs_ps, mut ms_ps) = (Kind::default(), Kind::default(), Kind::default());
    let mut refused = 0;
    let total = Instant::now();
    for (i, p) in paths.iter().enumerate().skip(start).take(limit) {
        let bytes = std::fs::read(p).unwrap();
        let Parsed::Entry(e) = parse(&bytes) else {
            refused += 1;
            continue;
        };
        if !matches!(e.stage, 4..=6) {
            continue;
        }
        let root = root_signature(&e);
        let stage_name = STAGE_NAMES[e.stage as usize];
        let (kind, row) = match e.stage {
            5 => (&mut ps_only, "ps-alone"),
            4 => (&mut vs_ps, "vs+default-ps"),
            _ => (&mut ms_ps, "ms+default-ps"),
        };
        kind.n += 1;
        let started = Instant::now();
        let (convert_ms, pipeline_ms, status): (f64, Option<f64>, String);
        if e.stage == 5 {
            let desc = state(&root, None, Some(&e.dxil), None, Vec::new());
            let r = msc.compile(&e.dxil, Some(&root), &desc.msc_options());
            convert_ms = started.elapsed().as_secs_f64() * 1000.0;
            match r {
                Ok(c) => {
                    kind.convert_ok += 1;
                    kind.convert_ms.push(convert_ms);
                    match device.library_from_metallib(&c.metallib) {
                        Ok(_) => {
                            kind.library_ok += 1;
                            status = "ok".into();
                        }
                        Err(err) => {
                            kind.fail(&format!("library: {err}"));
                            status = "library-failed".into();
                        }
                    }
                }
                Err(err) => {
                    kind.fail(&format!("convert: {err}"));
                    status = "convert-failed".into();
                }
            }
            pipeline_ms = None;
        } else {
            let (vs, ms) = if e.stage == 4 {
                (Some(e.dxil.as_slice()), None)
            } else {
                (None, Some(e.dxil.as_slice()))
            };
            // Vertex inputs come from the reflection of the first conversion.
            let first = state(&root, vs, Some(&default_ps), ms, Vec::new());
            let prepared = prepare_graphics(&first);
            convert_ms = started.elapsed().as_secs_f64() * 1000.0;
            match prepared {
                Err(err) => {
                    kind.fail(&format!("convert: {err}"));
                    status = "convert-failed".into();
                    pipeline_ms = None;
                }
                Ok(prep) => {
                    kind.convert_ok += 1;
                    kind.convert_ms.push(convert_ms);
                    let names: Vec<String> = prep
                        .vs
                        .as_ref()
                        .and_then(|c| c.reflection.vertex.as_ref())
                        .map(|v| v.inputs.iter().map(|i| i.name.clone()).collect())
                        .unwrap_or_default();
                    let mut result = Err(String::new());
                    let mut ms_taken = 0.0;
                    for (try_no, format) in [
                        MTLVertexFormat::Float4,
                        MTLVertexFormat::UInt4,
                        MTLVertexFormat::Float,
                        MTLVertexFormat::UInt,
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        if try_no > 0 && names.is_empty() {
                            break;
                        }
                        let desc =
                            state(&root, vs, Some(&default_ps), ms, elements(&names, format));
                        let t = Instant::now();
                        let r = device.graphics_pipeline(&prep, &desc);
                        ms_taken = t.elapsed().as_secs_f64() * 1000.0;
                        match r {
                            Ok(_) => {
                                if try_no > 0 {
                                    kind.retried_uint += 1;
                                }
                                result = Ok(());
                                break;
                            }
                            Err(err) => result = Err(err.to_string()),
                        }
                    }
                    match result {
                        Ok(()) => {
                            kind.library_ok += 1;
                            kind.pipeline_ok += 1;
                            kind.pipeline_ms.push(ms_taken);
                            status = "ok".into();
                        }
                        Err(err) => {
                            kind.fail(&format!("pipeline: {err}"));
                            status = "pipeline-failed".into();
                        }
                    }
                    pipeline_ms = Some(ms_taken);
                }
            }
        }
        writeln!(
            out,
            "{i}\t{stage_name}\t{row}\t{}\t{status}\t{convert_ms:.1}\t{}",
            e.dxil.len(),
            pipeline_ms.map_or("-".into(), |m| format!("{m:.1}")),
        )
        .unwrap();
        out.flush().unwrap();
    }
    let wall = total.elapsed().as_secs_f64();
    println!(
        "== graphics survey of the cache (aggregate only); refused by the translator {refused}"
    );
    line(&ps_only, "pixel alone (convert + library)", wall);
    line(&vs_ps, "vertex + default pixel (pipeline)", wall);
    line(&ms_ps, "mesh + default pixel (pipeline)", wall);
}
