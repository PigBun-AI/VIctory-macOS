//! Call statistics of the front: how often the application calls each D3D12 /
//! DXGI method, and what it passes (resource dimensions and formats, view
//! kinds, barrier transitions, root parameter kinds, pipeline state, indirect
//! signatures, query types). The data for the "used" and "freq" columns of
//! the translation matrix (`docs/metal/translation-matrix.csv`).
//!
//! Switch: `VICTORY_METAL_CALLSTATS=<file>` (a path outside the repository).
//! Unset or empty: off; a call costs one atomic load. When on, the process
//! writes the file
//!
//! * every `VICTORY_METAL_CALLSTATS_INTERVAL` seconds (default 10; a game run
//!   is usually ended by a kill, which no exit hook sees),
//! * and when the DLL is unloaded (the cdylibs call [`flush`] from
//!   `DllMain(DLL_PROCESS_DETACH)`).
//!
//! `d3d12.dll` and `dxgi.dll` each carry their own copy of this crate, so each
//! writes its own file: the tag (`d3d12` or `dxgi`, [`set_tag`]) is put before
//! the extension of the path (`calls.json` -> `calls.d3d12.json`). The
//! written file is replaced atomically (temp file, rename), so a reader never
//! sees half of it.
//!
//! File format (JSON, version 1):
//!
//! ```text
//! { "version": 1, "tag": "d3d12", "pid": 123, "elapsed_s": 41.2,
//!   "calls": { "ID3D12GraphicsCommandList::DrawInstanced": 5747, ... },
//!   "dist":  { "res.dim_format_flags": { "tex2d R8G8B8A8_UNORM RT": 12, ... }, ... } }
//! ```
//!
//! Method names are `Interface::Method` with the interface that declares the
//! method (`ID3D12Device::CreateCommittedResource`, not the `Device14` the
//! application holds), generated into the shell by
//! `tools/metal/gen-front-stubs.sh`. `QueryInterface`, `AddRef` and `Release`
//! are the COM macro's and are not counted.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The counters of one process. Tests make their own; the front uses the
/// global one ([`call`], [`tally`]).
pub struct Stats {
    started: Instant,
    calls: Mutex<BTreeMap<&'static str, u64>>,
    dist: Mutex<BTreeMap<(&'static str, String), u64>>,
}

impl Default for Stats {
    fn default() -> Self {
        Stats::new()
    }
}

impl Stats {
    pub fn new() -> Stats {
        Stats {
            started: Instant::now(),
            calls: Mutex::new(BTreeMap::new()),
            dist: Mutex::new(BTreeMap::new()),
        }
    }

    /// One call of the method `name`.
    pub fn call(&self, name: &'static str) {
        *self.calls.lock().unwrap().entry(name).or_insert(0) += 1;
    }

    /// One occurrence of `key` in the distribution `cat`.
    pub fn tally(&self, cat: &'static str, key: String) {
        *self.dist.lock().unwrap().entry((cat, key)).or_insert(0) += 1;
    }

    /// The count of a method (tests, reports).
    pub fn calls_of(&self, name: &str) -> u64 {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .find(|(k, _)| **k == name)
            .map_or(0, |(_, v)| *v)
    }

    /// The count of a key in a distribution.
    pub fn count_of(&self, cat: &str, key: &str) -> u64 {
        self.dist
            .lock()
            .unwrap()
            .iter()
            .find(|((c, k), _)| *c == cat && k == key)
            .map_or(0, |(_, v)| *v)
    }

    /// The file's text (see the module docs).
    pub fn to_json(&self, tag: &str, pid: u32) -> String {
        let calls = self.calls.lock().unwrap();
        let dist = self.dist.lock().unwrap();
        let mut s = String::new();
        let _ = write!(
            s,
            "{{\n  \"version\": 1,\n  \"tag\": {},\n  \"pid\": {pid},\n  \"elapsed_s\": {:.1},\n  \"calls\": {{",
            json_str(tag),
            self.started.elapsed().as_secs_f64()
        );
        for (i, (name, n)) in calls.iter().enumerate() {
            let _ = write!(
                s,
                "{}\n    {}: {n}",
                if i == 0 { "" } else { "," },
                json_str(name)
            );
        }
        s.push_str("\n  },\n  \"dist\": {");
        let mut cats: BTreeMap<&str, Vec<(&String, u64)>> = BTreeMap::new();
        for ((cat, key), n) in dist.iter() {
            cats.entry(cat).or_default().push((key, *n));
        }
        for (i, (cat, keys)) in cats.iter().enumerate() {
            let _ = write!(
                s,
                "{}\n    {}: {{",
                if i == 0 { "" } else { "," },
                json_str(cat)
            );
            for (j, (key, n)) in keys.iter().enumerate() {
                let _ = write!(
                    s,
                    "{}\n      {}: {n}",
                    if j == 0 { "" } else { "," },
                    json_str(key)
                );
            }
            s.push_str("\n    }");
        }
        s.push_str("\n  }\n}\n");
        s
    }

    /// Writes [`to_json`](Self::to_json) to `path`, replacing it atomically.
    pub fn write(&self, path: &Path, tag: &str, pid: u32) -> std::io::Result<()> {
        let tmp = {
            let mut t = path.as_os_str().to_owned();
            t.push(".tmp");
            PathBuf::from(t)
        };
        std::fs::write(&tmp, self.to_json(tag, pid))?;
        std::fs::rename(&tmp, path)
    }
}

/// A JSON string literal (quotes, backslashes and control characters escaped).
fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// `path` with `.<tag>` before its extension (`a/calls.json` -> `a/calls.d3d12.json`;
/// no extension: `a/calls` -> `a/calls.d3d12`).
pub fn tagged_path(path: &Path, tag: &str) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
    let ext = path.extension().map(|s| s.to_string_lossy().into_owned());
    let name = match (stem, ext) {
        (Some(s), Some(e)) => format!("{s}.{tag}.{e}"),
        (Some(s), None) => format!("{s}.{tag}"),
        _ => format!("callstats.{tag}"),
    };
    path.with_file_name(name)
}

/// What a cdylib's `DllMain` does: on attach (`reason` 1) it names the file
/// by the DLL's tag, on detach (`reason` 0) it writes the counts.
pub fn dll_main(tag: &'static str, reason: u32) {
    match reason {
        1 => set_tag(tag),
        0 => flush(),
        _ => {}
    }
}

// ---- the process-wide counters --------------------------------------------

struct Global {
    stats: Stats,
    path: PathBuf,
}

static GLOBAL: OnceLock<Option<Global>> = OnceLock::new();
static TAG: OnceLock<&'static str> = OnceLock::new();
static FLUSHER: AtomicBool = AtomicBool::new(false);

/// Names this DLL's file (`d3d12`, `dxgi`). Call before the first count; the
/// first call wins. Without it the tag is `front`.
pub fn set_tag(tag: &'static str) {
    let _ = TAG.set(tag);
}

fn tag() -> &'static str {
    TAG.get().copied().unwrap_or("front")
}

fn global() -> Option<&'static Global> {
    GLOBAL
        .get_or_init(|| {
            let path = std::env::var_os("VICTORY_METAL_CALLSTATS").filter(|v| !v.is_empty())?;
            Some(Global {
                stats: Stats::new(),
                path: tagged_path(Path::new(&path), tag()),
            })
        })
        .as_ref()
}

/// Whether the switch is on.
pub fn enabled() -> bool {
    global().is_some()
}

/// One call of `name` (`Interface::Method`).
pub fn call(name: &'static str) {
    if let Some(g) = global() {
        g.stats.call(name);
        start_flusher();
    }
}

/// One occurrence of the key `key()` makes in the distribution `cat`; the
/// closure only runs when the switch is on.
pub fn tally(cat: &'static str, key: impl FnOnce() -> String) {
    if let Some(g) = global() {
        g.stats.tally(cat, key());
        start_flusher();
    }
}

/// Writes the file now (the DLL's `DllMain` calls it on process detach).
pub fn flush() {
    if let Some(g) = global() {
        let _ = g.stats.write(&g.path, tag(), std::process::id());
    }
}

/// The periodic writer: one thread, started by the first count.
fn start_flusher() {
    if FLUSHER.swap(true, Ordering::Relaxed) {
        return;
    }
    let secs = std::env::var("VICTORY_METAL_CALLSTATS_INTERVAL")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(10);
    let _ = std::thread::Builder::new()
        .name("callstats".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(secs));
                flush();
            }
        });
}

// ---- names for the keys -----------------------------------------------------

const DXGI_NAMES: [&str; 100] = [
    "UNKNOWN",
    "R32G32B32A32_TYPELESS",
    "R32G32B32A32_FLOAT",
    "R32G32B32A32_UINT",
    "R32G32B32A32_SINT",
    "R32G32B32_TYPELESS",
    "R32G32B32_FLOAT",
    "R32G32B32_UINT",
    "R32G32B32_SINT",
    "R16G16B16A16_TYPELESS",
    "R16G16B16A16_FLOAT",
    "R16G16B16A16_UNORM",
    "R16G16B16A16_UINT",
    "R16G16B16A16_SNORM",
    "R16G16B16A16_SINT",
    "R32G32_TYPELESS",
    "R32G32_FLOAT",
    "R32G32_UINT",
    "R32G32_SINT",
    "R32G8X24_TYPELESS",
    "D32_FLOAT_S8X24_UINT",
    "R32_FLOAT_X8X24_TYPELESS",
    "X32_TYPELESS_G8X24_UINT",
    "R10G10B10A2_TYPELESS",
    "R10G10B10A2_UNORM",
    "R10G10B10A2_UINT",
    "R11G11B10_FLOAT",
    "R8G8B8A8_TYPELESS",
    "R8G8B8A8_UNORM",
    "R8G8B8A8_UNORM_SRGB",
    "R8G8B8A8_UINT",
    "R8G8B8A8_SNORM",
    "R8G8B8A8_SINT",
    "R16G16_TYPELESS",
    "R16G16_FLOAT",
    "R16G16_UNORM",
    "R16G16_UINT",
    "R16G16_SNORM",
    "R16G16_SINT",
    "R32_TYPELESS",
    "D32_FLOAT",
    "R32_FLOAT",
    "R32_UINT",
    "R32_SINT",
    "R24G8_TYPELESS",
    "D24_UNORM_S8_UINT",
    "R24_UNORM_X8_TYPELESS",
    "X24_TYPELESS_G8_UINT",
    "R8G8_TYPELESS",
    "R8G8_UNORM",
    "R8G8_UINT",
    "R8G8_SNORM",
    "R8G8_SINT",
    "R16_TYPELESS",
    "R16_FLOAT",
    "D16_UNORM",
    "R16_UNORM",
    "R16_UINT",
    "R16_SNORM",
    "R16_SINT",
    "R8_TYPELESS",
    "R8_UNORM",
    "R8_UINT",
    "R8_SNORM",
    "R8_SINT",
    "A8_UNORM",
    "R1_UNORM",
    "R9G9B9E5_SHAREDEXP",
    "R8G8_B8G8_UNORM",
    "G8R8_G8B8_UNORM",
    "BC1_TYPELESS",
    "BC1_UNORM",
    "BC1_UNORM_SRGB",
    "BC2_TYPELESS",
    "BC2_UNORM",
    "BC2_UNORM_SRGB",
    "BC3_TYPELESS",
    "BC3_UNORM",
    "BC3_UNORM_SRGB",
    "BC4_TYPELESS",
    "BC4_UNORM",
    "BC4_SNORM",
    "BC5_TYPELESS",
    "BC5_UNORM",
    "BC5_SNORM",
    "B5G6R5_UNORM",
    "B5G5R5A1_UNORM",
    "B8G8R8A8_UNORM",
    "B8G8R8X8_UNORM",
    "R10G10B10_XR_BIAS_A2_UNORM",
    "B8G8R8A8_TYPELESS",
    "B8G8R8A8_UNORM_SRGB",
    "B8G8R8X8_TYPELESS",
    "B8G8R8X8_UNORM_SRGB",
    "BC6H_TYPELESS",
    "BC6H_UF16",
    "BC6H_SF16",
    "BC7_TYPELESS",
    "BC7_UNORM",
    "BC7_UNORM_SRGB",
];

/// The DXGI format's name, or its number for one the table does not have.
pub fn dxgi_name(format: u32) -> String {
    match DXGI_NAMES.get(format as usize) {
        Some(n) => (*n).to_string(),
        None if format == 115 => "B4G4R4A4_UNORM".to_string(),
        None => format!("DXGI_{format}"),
    }
}

/// The resource state's bits by name (`0` = `COMMON`, which is also `PRESENT`).
pub fn state_name(state: u32) -> String {
    const BITS: [(u32, &str); 16] = [
        (0x1, "VB_CB"),
        (0x2, "IB"),
        (0x4, "RT"),
        (0x8, "UAV"),
        (0x10, "DEPTH_WRITE"),
        (0x20, "DEPTH_READ"),
        (0x40, "NON_PS_SRV"),
        (0x80, "PS_SRV"),
        (0x100, "STREAM_OUT"),
        (0x200, "INDIRECT"),
        (0x400, "COPY_DEST"),
        (0x800, "COPY_SOURCE"),
        (0x1000, "RESOLVE_DEST"),
        (0x2000, "RESOLVE_SOURCE"),
        (0x40_0000, "RT_AS"),
        (0x100_0000, "SHADING_RATE"),
    ];
    if state == 0 {
        return "COMMON".to_string();
    }
    if state == 0xAC3 {
        return "GENERIC_READ".to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    let mut rest = state;
    for (bit, name) in BITS {
        if state & bit != 0 {
            parts.push(name.to_string());
            rest &= !bit;
        }
    }
    if rest != 0 {
        parts.push(format!("0x{rest:x}"));
    }
    parts.join("|")
}

/// `D3D12_RESOURCE_DIMENSION` by name.
pub fn dim_name(dim: u32) -> &'static str {
    match dim {
        1 => "buffer",
        2 => "tex1d",
        3 => "tex2d",
        4 => "tex3d",
        _ => "dim?",
    }
}

/// `D3D12_RESOURCE_FLAGS` (the four that matter) as `RT DS UAV NOSRV`.
pub fn resource_flags(flags: u32) -> String {
    let mut p = Vec::new();
    for (bit, n) in [(1, "RT"), (2, "DS"), (4, "UAV"), (8, "NOSRV")] {
        if flags & bit != 0 {
            p.push(n);
        }
    }
    if p.is_empty() {
        "-".to_string()
    } else {
        p.join("|")
    }
}

/// `D3D12_HEAP_TYPE` by name.
pub fn heap_type_name(ty: u32) -> &'static str {
    match ty {
        1 => "default",
        2 => "upload",
        3 => "readback",
        4 => "custom",
        _ => "heap?",
    }
}

/// A count in a few buckets (`0`, `1`, `2-3`, `4-7`, ... `>=4096`), for sizes
/// that would otherwise make one key per value.
pub fn bucket(n: u64) -> String {
    match n {
        0 => "0".to_string(),
        1 => "1".to_string(),
        n if n >= 4096 => ">=4096".to_string(),
        n => {
            let lo = 1u64 << (63 - n.leading_zeros());
            format!("{lo}-{}", lo * 2 - 1)
        }
    }
}

/// `D3D12_COMPARISON_FUNC` by name.
pub fn compare_name(f: u32) -> String {
    const N: [&str; 9] = [
        "?", "NEVER", "LESS", "EQUAL", "LESS_EQUAL", "GREATER", "NOT_EQUAL", "GREATER_EQUAL",
        "ALWAYS",
    ];
    N.get(f as usize).map_or_else(|| format!("cmp{f}"), |s| (*s).to_string())
}

fn blend_factor_name(f: u32) -> String {
    let n = match f {
        1 => "ZERO",
        2 => "ONE",
        3 => "SRC_COLOR",
        4 => "INV_SRC_COLOR",
        5 => "SRC_ALPHA",
        6 => "INV_SRC_ALPHA",
        7 => "DST_ALPHA",
        8 => "INV_DST_ALPHA",
        9 => "DST_COLOR",
        10 => "INV_DST_COLOR",
        11 => "SRC_ALPHA_SAT",
        14 => "BLEND_FACTOR",
        15 => "INV_BLEND_FACTOR",
        16 => "SRC1_COLOR",
        17 => "INV_SRC1_COLOR",
        18 => "SRC1_ALPHA",
        19 => "INV_SRC1_ALPHA",
        _ => return format!("bf{f}"),
    };
    n.to_string()
}

fn blend_op_name(o: u32) -> String {
    match o {
        1 => "ADD".to_string(),
        2 => "SUB".to_string(),
        3 => "REV_SUB".to_string(),
        4 => "MIN".to_string(),
        5 => "MAX".to_string(),
        _ => format!("op{o}"),
    }
}

/// `D3D_PRIMITIVE_TOPOLOGY` by name (the ones that matter).
pub fn topology_name(t: i32) -> String {
    match t {
        1 => "POINTLIST".to_string(),
        2 => "LINELIST".to_string(),
        3 => "LINESTRIP".to_string(),
        4 => "TRIANGLELIST".to_string(),
        5 => "TRIANGLESTRIP".to_string(),
        6 => "TRIANGLEFAN".to_string(),
        10..=13 => format!("ADJACENCY_{t}"),
        33..=64 => format!("PATCHLIST_{}", t - 32),
        _ => format!("topology{t}"),
    }
}

// ---- tallies of what the application passes ---------------------------------

use crate::pso::PipelineState;
use crate::resource::ResourceDesc;
use crate::rootsig::{ParamKind, RootSignatureDesc};

/// Where a tally goes: the global counters, or a test's [`Stats`].
type Sink<'a> = &'a dyn Fn(&'static str, String);

/// Runs `f` against the global counters when the switch is on.
fn with_global(f: impl FnOnce(Sink<'_>)) {
    if let Some(g) = global() {
        f(&|cat, key| g.stats.tally(cat, key));
        start_flusher();
    }
}

/// A resource at creation: dimension, format, flags, heap, samples, mips.
pub fn resource_created(heap_type: u32, d: &ResourceDesc, initial_state: u32, placed: bool) {
    with_global(|t| resource_created_into(t, heap_type, d, initial_state, placed));
}

/// [`resource_created`] into a sink.
pub fn resource_created_into(
    t: Sink<'_>,
    heap_type: u32,
    d: &ResourceDesc,
    initial_state: u32,
    placed: bool,
) {
    t(
        "res.dim_format_flags",
        format!(
            "{} {} {}",
            dim_name(d.dimension),
            dxgi_name(d.format),
            resource_flags(d.flags)
        ),
    );
    t(
        "res.heap",
        format!(
            "{} {}",
            heap_type_name(heap_type),
            if placed { "placed" } else { "committed" }
        ),
    );
    t("res.initial_state", state_name(initial_state));
    if d.dimension != 1 {
        t(
            "res.mips",
            format!("{} {}", dim_name(d.dimension), bucket(u64::from(d.mips))),
        );
        t("res.samples", d.sample_count.to_string());
        t(
            "res.array_or_depth",
            format!(
                "{} {}",
                dim_name(d.dimension),
                bucket(u64::from(d.depth_or_array))
            ),
        );
    } else {
        // KiB, bucketed.
        t("res.buffer_kib", bucket(d.width.min(1 << 40) >> 10));
    }
}

/// A root signature: the kinds of its parameters and ranges.
pub fn root_signature(d: &RootSignatureDesc) {
    with_global(|t| root_signature_into(t, d));
}

/// [`root_signature`] into a sink.
pub fn root_signature_into(t: Sink<'_>, d: &RootSignatureDesc) {
    let (mut constants, mut cbv, mut srv, mut uav, mut tables) = (0, 0, 0, 0, 0);
    for p in &d.params {
        match &p.kind {
            ParamKind::Constants { num, .. } => {
                constants += 1;
                t("rootsig.constants_dwords", bucket(u64::from(*num)));
                t("rootsig.param", "constants".to_string());
            }
            ParamKind::Descriptor { ty, .. } => {
                // `D3D12_ROOT_PARAMETER_TYPE`: 2 CBV, 3 SRV, 4 UAV.
                let n = match ty {
                    2 => {
                        cbv += 1;
                        "root_cbv"
                    }
                    3 => {
                        srv += 1;
                        "root_srv"
                    }
                    _ => {
                        uav += 1;
                        "root_uav"
                    }
                };
                t("rootsig.param", n.to_string());
            }
            ParamKind::Table(ranges) => {
                tables += 1;
                t("rootsig.param", "table".to_string());
                for r in ranges {
                    // `D3D12_DESCRIPTOR_RANGE_TYPE`: 0 SRV, 1 UAV, 2 CBV, 3 SAMPLER.
                    let n = match r.ty {
                        0 => "srv",
                        1 => "uav",
                        2 => "cbv",
                        _ => "sampler",
                    };
                    t(
                        "rootsig.range",
                        format!(
                            "{n} {}",
                            if r.num == u32::MAX {
                                "unbounded"
                            } else {
                                "bounded"
                            }
                        ),
                    );
                }
            }
        }
    }
    t(
        "rootsig.shape",
        format!(
            "constants={constants} cbv={cbv} srv={srv} uav={uav} tables={tables} static_samplers={}",
            d.samplers.len()
        ),
    );
    t("rootsig.flags", format!("0x{:x}", d.flags));
}

/// A graphics (vertex, mesh or geometry) pipeline state as the application
/// described it: stages, blend, depth/stencil, rasterizer, targets.
pub fn pipeline_state(s: &PipelineState) {
    with_global(|t| pipeline_state_into(t, s));
}

/// [`pipeline_state`] into a sink.
pub fn pipeline_state_into(t: Sink<'_>, s: &PipelineState) {
    let mut stages = Vec::new();
    for (len, n) in [
        (s.amp.len, "amp"),
        (s.ms.len, "ms"),
        (s.vs.len, "vs"),
        (s.hs.len, "hs"),
        (s.ds.len, "ds"),
        (s.gs.len, "gs"),
        (s.ps.len, "ps"),
    ] {
        if len != 0 {
            stages.push(n);
        }
    }
    t("pso.stages", stages.join("+"));
    let rts = (s.rt_count as usize).min(8);
    // Without independent blend every target uses the first description.
    let blend = |i: usize| s.blend.rt[if s.blend.independent { i } else { 0 }];
    let enabled: Vec<usize> = (0..rts).filter(|&i| blend(i).enable).collect();
    t(
        "pso.blend_targets",
        format!(
            "enabled={}/{} independent={} a2c={}",
            enabled.len(),
            rts,
            u8::from(s.blend.independent),
            u8::from(s.blend.alpha_to_coverage)
        ),
    );
    for &i in &enabled {
        let b = blend(i);
        t(
            "pso.blend_factors",
            format!(
                "rgb {} {} {} | a {} {} {} | mask {:x}",
                blend_factor_name(b.src),
                blend_factor_name(b.dst),
                blend_op_name(b.op),
                blend_factor_name(b.src_alpha),
                blend_factor_name(b.dst_alpha),
                blend_op_name(b.op_alpha),
                b.write_mask
            ),
        );
    }
    if (0..rts).any(|i| blend(i).logic_enable) {
        t("pso.logic_op", "enabled".to_string());
    }
    for i in 0..rts {
        t("pso.rt_format", dxgi_name(s.rt_formats[i]));
        if blend(i).write_mask != 0xF {
            t("pso.write_mask", format!("{:x}", blend(i).write_mask));
        }
    }
    t("pso.rt_count", rts.to_string());
    t("pso.ds_format", dxgi_name(s.ds_format));
    t(
        "pso.depth",
        format!(
            "depth={} write={} func={} stencil={} bounds={}",
            u8::from(s.depth.depth_enable),
            s.depth.depth_write_mask,
            compare_name(s.depth.depth_func),
            u8::from(s.depth.stencil_enable),
            u8::from(s.depth.depth_bounds)
        ),
    );
    t(
        "pso.raster",
        format!(
            "fill={} cull={} ccw={} clip={} bias={} conservative={} msaa={} forced_samples={}",
            s.raster.fill,
            s.raster.cull,
            u8::from(s.raster.front_ccw),
            u8::from(s.raster.depth_clip),
            u8::from(s.raster.depth_bias != 0 || s.raster.slope_scaled_bias != 0.0),
            s.raster.conservative,
            u8::from(s.raster.multisample),
            s.raster.forced_sample_count
        ),
    );
    t("pso.topology_type", s.topology.to_string());
    t("pso.sample_count", s.sample_count.to_string());
    t("pso.strip_cut", s.strip_cut.to_string());
    if s.stream_output_entries != 0 {
        t("pso.stream_output", "entries>0".to_string());
    }
    t("pso.input_elements", bucket(u64::from(s.input_layout.count)));
    if s.flags != 0 {
        t("pso.flags", format!("0x{:x}", s.flags));
    }
}

// ---- views, samplers, barriers, signatures, queries, copies -----------------

use crate::convert::SamplerParams;
use crate::device::{BufferView, ResourceBarrier};
use crate::indirect::CommandSignature;
use crate::view::{TargetView, TextureView};
use victory_metal_proto::indirect::IndirectArgKind;
use victory_metal_proto::types::TexViewDim;

fn view_dim_name(d: TexViewDim) -> &'static str {
    match d {
        TexViewDim::TEX1D => "1d",
        TexViewDim::TEX1D_ARRAY => "1d_array",
        TexViewDim::TEX2D => "2d",
        TexViewDim::TEX2D_ARRAY => "2d_array",
        TexViewDim::TEX2D_MS => "2d_ms",
        TexViewDim::TEX2D_MS_ARRAY => "2d_ms_array",
        TexViewDim::TEX3D => "3d",
        TexViewDim::CUBE => "cube",
        TexViewDim::CUBE_ARRAY => "cube_array",
        _ => "dim?",
    }
}

fn arg_kind_name(k: IndirectArgKind) -> &'static str {
    match k {
        IndirectArgKind::DRAW => "draw",
        IndirectArgKind::DRAW_INDEXED => "draw_indexed",
        IndirectArgKind::DISPATCH => "dispatch",
        IndirectArgKind::VERTEX_BUFFER_VIEW => "vertex_buffer",
        IndirectArgKind::INDEX_BUFFER_VIEW => "index_buffer",
        IndirectArgKind::CONSTANT => "constants",
        IndirectArgKind::CONSTANT_BUFFER_VIEW => "root_cbv",
        IndirectArgKind::SHADER_RESOURCE_VIEW => "root_srv",
        IndirectArgKind::UNORDERED_ACCESS_VIEW => "root_uav",
        IndirectArgKind::DISPATCH_RAYS => "dispatch_rays",
        IndirectArgKind::DISPATCH_MESH => "dispatch_mesh",
        _ => "arg?",
    }
}

/// A texture shader view (SRV or UAV): kind, dimension, format.
pub fn texture_view(v: &TextureView) {
    with_global(|t| texture_view_into(t, v));
}

/// [`texture_view`] into a sink.
pub fn texture_view_into(t: Sink<'_>, v: &TextureView) {
    let kind = if v.uav { "uav" } else { "srv" };
    if v.resource.is_none() {
        return t("view.null", kind.to_string());
    }
    t(
        "view.texture",
        format!("{kind} {} {}", view_dim_name(v.dim), dxgi_name(v.format)),
    );
    if v.plane != 0 {
        t("view.texture_plane", format!("{kind} plane={}", v.plane));
    }
}

/// A render-target or depth-stencil view: kind, dimension, format.
pub fn target_view(v: &TargetView) {
    with_global(|t| target_view_into(t, v));
}

/// [`target_view`] into a sink.
pub fn target_view_into(t: Sink<'_>, v: &TargetView) {
    t(
        "view.target",
        format!(
            "{} {} {}",
            if v.depth_stencil { "dsv" } else { "rtv" },
            view_dim_name(v.dim),
            dxgi_name(v.format)
        ),
    );
}

/// A buffer shader view: SRV or UAV, raw, structured or typed (+ format).
pub fn buffer_view(v: &BufferView) {
    with_global(|t| buffer_view_into(t, v));
}

/// [`buffer_view`] into a sink.
pub fn buffer_view_into(t: Sink<'_>, v: &BufferView) {
    let kind = if v.uav { "uav" } else { "srv" };
    if v.resource.is_none() {
        return t("view.null", format!("buffer {kind}"));
    }
    let shape = if v.raw {
        "raw".to_string()
    } else if v.structure_stride != 0 {
        format!("structured stride={}", v.structure_stride)
    } else {
        format!("typed {}", dxgi_name(v.format))
    };
    t(
        "view.buffer",
        format!("{kind} {shape}{}", if v.counter.is_some() { " counter" } else { "" }),
    );
}

/// A constant buffer view (size bucket in bytes).
pub fn cbv(size: u32) {
    with_global(|t| t("view.cbv_bytes", bucket(u64::from(size) / 16)));
}

/// A sampler: filter, address modes, comparison.
pub fn sampler(s: &SamplerParams) {
    with_global(|t| sampler_into(t, s));
}

/// [`sampler`] into a sink.
pub fn sampler_into(t: Sink<'_>, s: &SamplerParams) {
    t(
        "sampler",
        format!(
            "filter=0x{:x} addr={},{},{} aniso={} cmp={}",
            s.filter,
            s.address_u,
            s.address_v,
            s.address_w,
            s.max_anisotropy,
            if s.filter & 0x80 != 0 {
                compare_name(s.comparison)
            } else {
                "-".to_string()
            }
        ),
    );
}

/// The barriers of one `ResourceBarrier` call: kinds, transition pairs, batch size.
pub fn barriers(bs: &[ResourceBarrier]) {
    with_global(|t| barriers_into(t, bs));
}

/// [`barriers`] into a sink.
pub fn barriers_into(t: Sink<'_>, bs: &[ResourceBarrier]) {
    t("barrier.batch", bucket(bs.len() as u64));
    for b in bs {
        match *b {
            ResourceBarrier::Transition {
                before,
                after,
                flags,
                subresource,
                ..
            } => {
                t(
                    "barrier.transition",
                    format!("{} -> {}", state_name(before), state_name(after)),
                );
                if flags != 0 {
                    t(
                        "barrier.split",
                        if flags & 1 != 0 { "begin_only" } else { "end_only" }.to_string(),
                    );
                }
                t(
                    "barrier.subresource",
                    if subresource == 0xffff_ffff { "all" } else { "one" }.to_string(),
                );
            }
            ResourceBarrier::Uav { res } => {
                t(
                    "barrier.uav",
                    if res.is_some() { "resource" } else { "all" }.to_string(),
                );
            }
            ResourceBarrier::Aliasing { .. } => t("barrier.aliasing", "aliasing".to_string()),
        }
    }
}

/// A command signature at creation: its argument list in order.
pub fn command_signature(sig: &CommandSignature) {
    with_global(|t| command_signature_into(t, sig));
}

/// [`command_signature`] into a sink.
pub fn command_signature_into(t: Sink<'_>, sig: &CommandSignature) {
    let list: Vec<&str> = sig.args.iter().map(|a| arg_kind_name(a.kind)).collect();
    t("indirect.signature", list.join(" + "));
    t("indirect.stride", bucket(u64::from(sig.stride)));
}

/// One `ExecuteIndirect`: the action, the cap and whether a count buffer is given.
pub fn execute_indirect(sig: &CommandSignature, max_count: u32, has_count_buffer: bool) {
    with_global(|t| execute_indirect_into(t, sig, max_count, has_count_buffer));
}

/// [`execute_indirect`] into a sink.
pub fn execute_indirect_into(
    t: Sink<'_>,
    sig: &CommandSignature,
    max_count: u32,
    has_count_buffer: bool,
) {
    let action = sig.args.last().map_or("none", |a| arg_kind_name(a.kind));
    t(
        "indirect.execute",
        format!(
            "{action} max={} count_buffer={}",
            bucket(u64::from(max_count)),
            u8::from(has_count_buffer)
        ),
    );
}

/// A query heap at creation (`D3D12_QUERY_HEAP_TYPE`) or a query that ends.
pub fn query_heap(ty: u32, count: u32) {
    with_global(|t| {
        t("query.heap", format!("type={ty} count={}", bucket(u64::from(count))));
    });
}

/// `EndQuery` / `BeginQuery` of a `D3D12_QUERY_TYPE`.
pub fn query_op(op: &'static str, ty: i32) {
    with_global(|t| {
        let n = match ty {
            0 => "occlusion",
            1 => "binary_occlusion",
            2 => "timestamp",
            3 => "pipeline_statistics",
            4..=7 => "so_statistics",
            _ => "type?",
        };
        t("query.op", format!("{op} {n}"));
    });
}

/// A swap chain at creation.
pub fn swapchain(width: u32, height: u32, format: u32, buffers: u32, effect: u32, flags: u32) {
    with_global(|t| {
        t(
            "swapchain.create",
            format!(
                "{width}x{height} {} buffers={buffers} effect={effect} flags=0x{flags:x}",
                dxgi_name(format)
            ),
        );
    });
}

/// A heap (`CreateHeap`): type and size bucket in MiB.
pub fn heap_created(ty: u32, size: u64, flags: u32) {
    with_global(|t| {
        t(
            "heap.create",
            format!("{} {}MiB flags=0x{flags:x}", heap_type_name(ty), bucket(size >> 20)),
        );
    });
}

/// A descriptor heap: `D3D12_DESCRIPTOR_HEAP_TYPE`, visibility, size bucket.
pub fn descriptor_heap(ty: u32, count: u32, shader_visible: bool) {
    with_global(|t| {
        let n = match ty {
            0 => "cbv_srv_uav",
            1 => "sampler",
            2 => "rtv",
            3 => "dsv",
            _ => "type?",
        };
        t(
            "descriptor_heap",
            format!(
                "{n} {} count={}",
                if shader_visible { "visible" } else { "staging" },
                bucket(u64::from(count))
            ),
        );
    });
}

/// `IASetPrimitiveTopology`.
pub fn topology(d3d: i32) {
    with_global(|t| t("draw.topology", topology_name(d3d)));
}

/// `OMSetRenderTargets`: bound targets and whether depth is bound.
pub fn render_targets(rt_bound: usize, depth: bool) {
    with_global(|t| t("om.targets", format!("rt={rt_bound} ds={}", u8::from(depth))));
}

/// `CopyTextureRegion`: the kinds of the two locations ("sub" or "footprint").
pub fn copy_texture_region(dst_footprint: bool, src_footprint: bool, has_box: bool) {
    with_global(|t| {
        let k = |f| if f { "buffer" } else { "texture" };
        t(
            "copy.texture_region",
            format!(
                "{} <- {}{}",
                k(dst_footprint),
                k(src_footprint),
                if has_box { " box" } else { "" }
            ),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_and_tallies_count() {
        let s = Stats::new();
        s.call("ID3D12Device::CreateFence");
        s.call("ID3D12Device::CreateFence");
        s.call("IDXGISwapChain::Present");
        s.tally("res.heap", "default committed".to_string());
        s.tally("res.heap", "default committed".to_string());
        s.tally("res.heap", "upload committed".to_string());
        assert_eq!(s.calls_of("ID3D12Device::CreateFence"), 2);
        assert_eq!(s.calls_of("IDXGISwapChain::Present"), 1);
        assert_eq!(s.calls_of("nothing"), 0);
        assert_eq!(s.count_of("res.heap", "default committed"), 2);
        assert_eq!(s.count_of("res.heap", "upload committed"), 1);
        assert_eq!(s.count_of("res.heap", "x"), 0);
    }

    #[test]
    fn the_file_text_is_json_with_every_count() {
        let s = Stats::new();
        s.call("A::B");
        s.call("A::B");
        s.call("A::C");
        s.tally("cat", "k \"quoted\" \\ end".to_string());
        let t = s.to_json("d3d12", 77);
        assert!(t.contains("\"version\": 1"));
        assert!(t.contains("\"tag\": \"d3d12\""));
        assert!(t.contains("\"pid\": 77"));
        assert!(t.contains("\"A::B\": 2"));
        assert!(t.contains("\"A::C\": 1"));
        assert!(t.contains("\"cat\": {"));
        assert!(t.contains("\"k \\\"quoted\\\" \\\\ end\": 1"));
        // Balanced braces: the text is one object.
        assert_eq!(t.matches('{').count(), t.matches('}').count());
        assert!(t.trim_end().ends_with('}'));
    }

    #[test]
    fn an_empty_file_is_still_a_file() {
        let t = Stats::new().to_json("front", 1);
        assert!(t.contains("\"calls\": {\n  }"));
        assert!(t.contains("\"dist\": {\n  }"));
    }

    #[test]
    fn writing_replaces_the_file_and_leaves_no_temp() {
        let dir = std::env::temp_dir().join(format!("callstats-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("calls.json");
        let s = Stats::new();
        s.call("A::B");
        s.write(&path, "t", 5).unwrap();
        s.call("A::B");
        s.write(&path, "t", 5).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"A::B\": 2"));
        assert!(!dir.join("calls.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_tag_goes_before_the_extension() {
        assert_eq!(
            tagged_path(Path::new("/x/calls.json"), "d3d12"),
            Path::new("/x/calls.d3d12.json")
        );
        assert_eq!(
            tagged_path(Path::new("/x/calls"), "dxgi"),
            Path::new("/x/calls.dxgi")
        );
    }

    #[test]
    fn names_and_buckets() {
        assert_eq!(dxgi_name(28), "R8G8B8A8_UNORM");
        assert_eq!(dxgi_name(98), "BC7_UNORM");
        assert_eq!(dxgi_name(115), "B4G4R4A4_UNORM");
        assert_eq!(dxgi_name(1000), "DXGI_1000");
        assert_eq!(state_name(0), "COMMON");
        assert_eq!(state_name(0xAC3), "GENERIC_READ");
        assert_eq!(state_name(0x4), "RT");
        assert_eq!(state_name(0x40 | 0x80), "NON_PS_SRV|PS_SRV");
        assert_eq!(state_name(0x4000), "0x4000");
        assert_eq!(resource_flags(0), "-");
        assert_eq!(resource_flags(5), "RT|UAV");
        assert_eq!(bucket(0), "0");
        assert_eq!(bucket(1), "1");
        assert_eq!(bucket(3), "2-3");
        assert_eq!(bucket(8), "8-15");
        assert_eq!(bucket(100000), ">=4096");
        assert_eq!(topology_name(4), "TRIANGLELIST");
        assert_eq!(topology_name(35), "PATCHLIST_3");
        // The table covers 0..=99 with distinct names.
        let mut seen = std::collections::BTreeSet::new();
        for n in DXGI_NAMES {
            assert!(seen.insert(n), "{n} twice");
        }
    }

    #[test]
    fn the_global_counters_are_off_without_the_switch() {
        // The test process has no VICTORY_METAL_CALLSTATS: counting is a no-op
        // and the key closure never runs.
        if std::env::var_os("VICTORY_METAL_CALLSTATS").is_some() {
            return;
        }
        assert!(!enabled());
        call("A::B");
        tally("cat", || panic!("the key is built only when the switch is on"));
        flush();
    }

    fn sink(stats: &Stats) -> impl Fn(&'static str, String) + '_ {
        move |c, k| stats.tally(c, k)
    }

    #[test]
    fn a_resource_is_tallied_by_dimension_format_flags_and_heap() {
        let s = Stats::new();
        let mut d = ResourceDesc::buffer(64 * 1024, 4);
        resource_created_into(&sink(&s), 2, &d, 0xAC3, false);
        assert_eq!(s.count_of("res.heap", "upload committed"), 1);
        assert_eq!(s.count_of("res.initial_state", "GENERIC_READ"), 1);
        assert_eq!(s.count_of("res.dim_format_flags", "buffer UNKNOWN UAV"), 1);
        assert_eq!(s.count_of("res.buffer_kib", "64-127"), 1);
        d.dimension = 3;
        d.format = 98;
        d.mips = 11;
        d.flags = 0;
        resource_created_into(&sink(&s), 1, &d, 0, true);
        assert_eq!(s.count_of("res.dim_format_flags", "tex2d BC7_UNORM -"), 1);
        assert_eq!(s.count_of("res.heap", "default placed"), 1);
        assert_eq!(s.count_of("res.mips", "tex2d 8-15"), 1);
        assert_eq!(s.count_of("res.samples", "1"), 1);
    }

    #[test]
    fn a_root_signature_is_tallied_by_parameter_and_range_kind() {
        use crate::rootsig::{Param, Range, Version};
        let s = Stats::new();
        let d = RootSignatureDesc {
            version: Version::V1_1,
            params: vec![
                Param {
                    kind: ParamKind::Constants {
                        reg: 0,
                        space: 0,
                        num: 4,
                    },
                    visibility: 0,
                },
                Param {
                    kind: ParamKind::Descriptor {
                        ty: 3,
                        reg: 0,
                        space: 1,
                        flags: 0,
                    },
                    visibility: 0,
                },
                Param {
                    kind: ParamKind::Table(vec![
                        Range {
                            ty: 1,
                            num: u32::MAX,
                            base: 0,
                            space: 2,
                            flags: 0,
                            offset: 0,
                        },
                        Range {
                            ty: 3,
                            num: 8,
                            base: 0,
                            space: 0,
                            flags: 0,
                            offset: 0,
                        },
                    ]),
                    visibility: 0,
                },
            ],
            samplers: Vec::new(),
            flags: 0xC00,
        };
        root_signature_into(&sink(&s), &d);
        assert_eq!(s.count_of("rootsig.param", "constants"), 1);
        assert_eq!(s.count_of("rootsig.param", "root_srv"), 1);
        assert_eq!(s.count_of("rootsig.param", "table"), 1);
        assert_eq!(s.count_of("rootsig.range", "uav unbounded"), 1);
        assert_eq!(s.count_of("rootsig.range", "sampler bounded"), 1);
        assert_eq!(s.count_of("rootsig.constants_dwords", "4-7"), 1);
        assert_eq!(
            s.count_of(
                "rootsig.shape",
                "constants=1 cbv=0 srv=1 uav=0 tables=1 static_samplers=0"
            ),
            1
        );
        assert_eq!(s.count_of("rootsig.flags", "0xc00"), 1);
    }

    #[test]
    fn a_pipeline_state_is_tallied_by_stage_blend_depth_and_raster() {
        let s = Stats::new();
        let mut p = PipelineState::default();
        p.vs.len = 10;
        p.ps.len = 10;
        p.rt_count = 2;
        p.rt_formats[0] = 28;
        p.rt_formats[1] = 26;
        p.ds_format = 40;
        // Not independent: both targets use description 0.
        p.blend.rt[0].enable = true;
        p.blend.rt[0].src = 5;
        p.blend.rt[0].dst = 6;
        p.raster.conservative = 1;
        pipeline_state_into(&sink(&s), &p);
        assert_eq!(s.count_of("pso.stages", "vs+ps"), 1);
        assert_eq!(
            s.count_of("pso.blend_targets", "enabled=2/2 independent=0 a2c=0"),
            1
        );
        assert_eq!(
            s.count_of(
                "pso.blend_factors",
                "rgb SRC_ALPHA INV_SRC_ALPHA ADD | a ONE ZERO ADD | mask f"
            ),
            2
        );
        assert_eq!(s.count_of("pso.rt_format", "R8G8B8A8_UNORM"), 1);
        assert_eq!(s.count_of("pso.rt_format", "R11G11B10_FLOAT"), 1);
        assert_eq!(s.count_of("pso.ds_format", "D32_FLOAT"), 1);
        assert_eq!(s.count_of("pso.rt_count", "2"), 1);
        // Mesh with a geometry stage and conservative rasterization.
        let mut m = PipelineState::default();
        m.ms.len = 1;
        m.amp.len = 1;
        m.ps.len = 1;
        m.gs.len = 1;
        pipeline_state_into(&sink(&s), &m);
        assert_eq!(s.count_of("pso.stages", "amp+ms+gs+ps"), 1);
        let text = s.to_json("t", 1);
        assert!(text.contains("conservative=1"));
        // With the switch off the global entry points do nothing.
        pipeline_state(&p);
    }

    #[test]
    fn views_samplers_barriers_and_signatures_are_tallied() {
        use crate::indirect::CommandSignature;
        use victory_metal_proto::ResId;
        use victory_metal_proto::indirect::IndirectArg;
        let s = Stats::new();
        let mut v = TextureView::null(false);
        texture_view_into(&sink(&s), &v);
        assert_eq!(s.count_of("view.null", "srv"), 1);
        v.resource = Some(ResId(3));
        v.format = 98;
        v.dim = TexViewDim::TEX2D_ARRAY;
        texture_view_into(&sink(&s), &v);
        assert_eq!(s.count_of("view.texture", "srv 2d_array BC7_UNORM"), 1);
        let bv = BufferView {
            uav: true,
            resource: Some(ResId(4)),
            first_element: 0,
            num_elements: 16,
            structure_stride: 0,
            format: 0,
            raw: true,
            counter: None,
        };
        buffer_view_into(&sink(&s), &bv);
        assert_eq!(s.count_of("view.buffer", "uav raw"), 1);
        let bs = [
            ResourceBarrier::Transition {
                res: ResId(1),
                subresource: 0xffff_ffff,
                before: 0x400,
                after: 0x80,
                flags: 0,
            },
            ResourceBarrier::Uav { res: None },
            ResourceBarrier::Aliasing {
                before: None,
                after: None,
            },
        ];
        barriers_into(&sink(&s), &bs);
        assert_eq!(s.count_of("barrier.batch", "2-3"), 1);
        assert_eq!(s.count_of("barrier.transition", "COPY_DEST -> PS_SRV"), 1);
        assert_eq!(s.count_of("barrier.uav", "all"), 1);
        assert_eq!(s.count_of("barrier.aliasing", "aliasing"), 1);
        let sig = CommandSignature {
            stride: 32,
            args: vec![
                IndirectArg {
                    kind: IndirectArgKind::CONSTANT,
                    index: 1,
                    dest_offset: 0,
                    count: 2,
                },
                IndirectArg {
                    kind: IndirectArgKind::DISPATCH_MESH,
                    index: 0,
                    dest_offset: 0,
                    count: 0,
                },
            ],
            root_sig: victory_metal_proto::RootSigId::NULL,
            record_bytes: 20,
        };
        command_signature_into(&sink(&s), &sig);
        execute_indirect_into(&sink(&s), &sig, 1, false);
        assert_eq!(s.count_of("indirect.signature", "constants + dispatch_mesh"), 1);
        assert_eq!(
            s.count_of("indirect.execute", "dispatch_mesh max=1 count_buffer=0"),
            1
        );
        let sp = SamplerParams {
            filter: 0x15,
            address_u: 1,
            address_v: 1,
            address_w: 3,
            mip_lod_bias: 0.0,
            max_anisotropy: 1,
            comparison: 0,
            border_color: [0.0; 4],
            min_lod: 0.0,
            max_lod: 3.0e38,
        };
        sampler_into(&sink(&s), &sp);
        assert_eq!(s.count_of("sampler", "filter=0x15 addr=1,1,3 aniso=1 cmp=-"), 1);
    }
}
