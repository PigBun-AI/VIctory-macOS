//! mac: the HLSL dual-path whole-program oracle (docs/mac/xdxc-oracle.md).
//!
//! One HLSL source, two routes to the Mac GPU, outputs compared:
//!
//! * **A (reference, HLSL semantics):** HLSL -> our helper DXC (`VICTORY_HELPER_DXC`, D3DMetal's `libdxcompiler` under Rosetta)
//!   -> DXIL (SM 6.6, resources picked from the descriptor heap) -> Metal Shader Converter -> Metal, on the executor
//!   (`victory-shader-exec`, [`super::host`]).
//! * **B (under test):** the same HLSL -> the console's shader compiler on the local reference SDK (run under Wine, NewBE-only,
//!   one descriptor table, one root-constant block) -> a console container (GFX10 code in the `DXIL` part's `XDAT`) -> [`translate`]
//!   -> DXIL -> MSC -> Metal, on the same executor with the same bindings.
//!
//! Path A is the truth of the HLSL; path B is the translator, whole program, instead of one instruction at a time against a
//! CPU model. Both read the same buffers in the same heap order and the same root constants; the UAVs they leave are compared.
//!
//! Without the local reference SDK, the helper DXC, or the executor, [`dual_path`] returns [`Verdict::Skipped`] and the tests
//! print `SKIPPED` and pass (so `cargo test` stays green on any Mac). Nothing the SDK makes is kept in the repository: the
//! compiler output goes to a private cache outside every checkout (`VICTORY_XDXC_CACHE`), the HLSL here is ours, and a comment
//! that needs a semantic of the SDK says so in its own words (依据本机 GDKX 参照).
//!
//! Environment: `VICTORY_GDKX_DIR` (the unpacked SDK; default beside the main checkouts), `VICTORY_XDXC_CACHE` (default: next to
//! that directory), `VICTORY_HELPER_DXC` / `VICTORY_DXC_HELPER` + `DXC_LIB` (path A's compiler; defaults from the Wine build under
//! the GTA6 root), `VICTORY_SHADER_EXEC` (the executor, as for every hardware test), `VICTORY_XDXC_RECOMPILE=1` (ignore the cache).
//! `tools/mac/xdxc-oracle.sh` sets all of it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use super::host::api::{D3D12_SHADER_VISIBILITY_ALL, DXGI_FORMAT};
use super::host::{Dispatch, Host, Root, Stride, View};
use super::{HostShader, HwStage, UserData, lower, translate};
use crate::native::{ResourceType, RootType};

/// One test at a time: the executor is one process, the translator's host-wave setting is per thread but the
/// compile cache and the wine prefix are shared.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------------------------------------------------
// The inputs and the verdict
// ---------------------------------------------------------------------------------------------------------------------

/// A view in the heap. Heap order is list order; the HLSL says where each one is (`DUAL_*` macros: heap slot, register).
#[derive(Clone, Debug)]
pub enum Res {
    /// A raw SRV over these dwords (`ByteAddressBuffer`).
    Srv(Vec<u32>),
    /// A constant buffer of these dwords.
    Cbv(Vec<u32>),
    /// A raw UAV of this many dwords, zeroed (`RWByteAddressBuffer`).
    Uav(u32),
}

/// What both paths are given.
#[derive(Clone, Debug)]
pub struct Inputs {
    /// The root constants (`cbuffer` at `b0`), padded to four dwords.
    pub constants: Vec<u32>,
    /// The heap's views, SRVs and CBVs first, UAVs last (the order of the table's ranges in the HLSL's root signature).
    pub resources: Vec<Res>,
    pub groups: [u32; 3],
}

/// How the UAVs of the two paths are compared. Anything but `Exact` says why in `reason` (it is printed with the verdict).
#[derive(Clone, Copy)]
pub struct Compare {
    /// Floats may differ by this many units in the last place (0: bit equal), a NaN equals any NaN.
    pub ulps: u32,
    /// Rewrites each UAV (index, dwords) before comparing: the order the lanes of a wave or the waves of a group won the
    /// atomic is not part of the program's result, so a test sorts what the order spoiled.
    pub canon: Option<fn(usize, &[u32]) -> Vec<u32>>,
    pub reason: &'static str,
}

impl Compare {
    pub const EXACT: Compare = Compare { ulps: 0, canon: None, reason: "" };
}

/// Translator settings for path B (`VICTORY_*` switches of the live whitelist), e.g. `("VICTORY_WAVE_WIDE", "1")`.
pub type Settings = Vec<(String, String)>;

/// The first differences between two paths' UAVs.
#[derive(Debug, Clone)]
pub struct Diff {
    pub uav: usize,
    pub words_differing: usize,
    pub words_total: usize,
    /// (dword index, a, b) of the first few.
    pub first: Vec<(usize, u32, u32)>,
}

#[derive(Debug, Clone)]
pub enum Verdict {
    /// No SDK, no helper DXC or no executor: the test prints this and passes.
    Skipped(String),
    /// Both paths ran and the UAVs agree (under the comparison's tolerance).
    Same { words: usize },
    /// Both paths ran and the UAVs differ.
    Different(Diff),
    /// A path did not get as far as an output (`stage` says where).
    Failed { stage: &'static str, why: String },
}

impl Verdict {
    pub fn is_skipped(&self) -> bool {
        matches!(self, Verdict::Skipped(_))
    }
    pub fn is_same(&self) -> bool {
        matches!(self, Verdict::Same { .. })
    }
    pub fn summary(&self) -> String {
        match self {
            Verdict::Skipped(w) => format!("SKIPPED: {w}"),
            Verdict::Same { words } => format!("SAME ({words} dwords)"),
            Verdict::Different(d) => {
                let mut s = format!("DIFFERENT: uav {} differs in {} of {} dwords; first", d.uav, d.words_differing, d.words_total);
                for (i, a, b) in &d.first {
                    let _ = write!(s, " [{i}] a={a:#010x} b={b:#010x}");
                }
                s
            }
            Verdict::Failed { stage, why } => format!("FAILED at {stage}: {why}"),
        }
    }
}

/// The two UAV sets and what [`dual_path`] made of them (for tests that also check path A against a CPU model).
#[derive(Debug, Clone)]
pub struct Outputs {
    pub a: Vec<Vec<u32>>,
    pub b: Vec<Vec<u32>>,
}

// ---------------------------------------------------------------------------------------------------------------------
// Locating the tools
// ---------------------------------------------------------------------------------------------------------------------

fn gta6_root() -> PathBuf {
    if let Some(r) = std::env::var_os("GTA6_ROOT") {
        return PathBuf::from(r);
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Documents").join("GTA6Project")
}

struct Tools {
    /// The unpacked local SDK.
    sdk: PathBuf,
    cache: PathBuf,
    /// Path A's front end and its library.
    helper: PathBuf,
    dxc_lib: PathBuf,
}

fn tools() -> Result<Tools, String> {
    let root = gta6_root();
    let sdk = std::env::var_os("VICTORY_GDKX_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("private").join(concat!("gdkx", "-extracted")));
    if !sdk.join("wenv.sh").is_file() || !xdxc_exe(&sdk).is_file() {
        return Err("no local reference SDK (VICTORY_GDKX_DIR)".into());
    }
    let cache = std::env::var_os("VICTORY_XDXC_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| sdk.parent().map(|p| p.join(concat!("gdkx", "-oracle-cache"))).unwrap_or_else(|| root.join("private")));
    std::fs::create_dir_all(&cache).map_err(|e| format!("cache {}: {e}", cache.display()))?;
    let helper = std::env::var_os("VICTORY_HELPER_DXC")
        .or_else(|| std::env::var_os("VICTORY_DXC_HELPER"))
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("VIctory-macOS/target/dxc-d3dmetal"));
    if !helper.is_file() {
        return Err(format!("no helper DXC at {} (VICTORY_HELPER_DXC)", helper.display()));
    }
    let dxc_lib = std::env::var_os("DXC_LIB")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("VIctory-macOS/target/wine-cx/lib/external/D3DMetal.framework/Versions/A/Resources/libdxcompiler.dylib"));
    if !dxc_lib.is_file() {
        return Err(format!("no libdxcompiler at {} (DXC_LIB)", dxc_lib.display()));
    }
    Ok(Tools { sdk, cache, helper, dxc_lib })
}

fn xdxc_exe(sdk: &Path) -> PathBuf {
    sdk.join("261000/xbox/bin/gen9/xdxc.exe")
}

// ---------------------------------------------------------------------------------------------------------------------
// HLSL: one source, two bindings
// ---------------------------------------------------------------------------------------------------------------------

/// Prepended to every test's HLSL. `DUAL_A` is 1 for path A (resources come from `ResourceDescriptorHeap[slot]`, the root
/// signature is the host's) and 0 for path B (resources are register-bound through the HLSL's own root signature, one
/// descriptor table whose ranges are in heap order). A test declares each resource once with its heap slot and register:
///
/// ```text
/// DUAL_SRV(src, 0, 0)     // ByteAddressBuffer src at heap slot 0 = t0
/// DUAL_UAV(dst, 1, 0)     // RWByteAddressBuffer dst at heap slot 1 = u0
/// DUAL_ROOT(RS) [numthreads(64,1,1)] void main(...) { DUAL_BIND_SRV(src, 0, 0) DUAL_BIND_UAV(dst, 1, 0) ... }
/// ```
const PRELUDE: &str = "\
#if DUAL_A
#define DUAL_ROOT(rs)
#define DUAL_SRV(n, heap, reg)
#define DUAL_UAV(n, heap, reg)
#define DUAL_BIND_SRV(n, heap, reg) ByteAddressBuffer n = ResourceDescriptorHeap[heap];
#define DUAL_BIND_UAV(n, heap, reg) RWByteAddressBuffer n = ResourceDescriptorHeap[heap];
#else
#define DUAL_ROOT(rs) [RootSignature(rs)]
#define DUAL_SRV(n, heap, reg) ByteAddressBuffer n : register(t##reg);
#define DUAL_UAV(n, heap, reg) RWByteAddressBuffer n : register(u##reg);
#define DUAL_BIND_SRV(n, heap, reg)
#define DUAL_BIND_UAV(n, heap, reg)
#endif
";

fn source_for(hlsl: &str, a: bool) -> String {
    format!("#define DUAL_A {}\n{PRELUDE}#line 1\n{hlsl}", u32::from(a))
}

fn key_of(parts: &[&[u8]]) -> String {
    let mut h = blake3::Hasher::new();
    for p in parts {
        h.update(&(p.len() as u64).to_le_bytes());
        h.update(p);
    }
    h.finalize().to_hex().as_str()[..24].to_owned()
}

fn run(cmd: &mut Command, what: &str) -> Result<String, String> {
    let out = cmd.stdin(Stdio::null()).output().map_err(|e| format!("{what}: cannot start: {e}"))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    if out.status.success() { Ok(text) } else { Err(format!("{what}: {}: {}", out.status, tail(&text))) }
}

fn tail(s: &str) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(12)..].join(" | ")
}

fn recompile() -> bool {
    std::env::var("VICTORY_XDXC_RECOMPILE").is_ok_and(|v| v == "1")
}

/// Path B's compiler: the console's shader compiler of the local SDK under Wine, for the compute profile, with the NewBE code only
/// (`-noacoprecompile`): the title's containers carry their native program in the `DXIL` part (NewBE) and no `ACCC` part, so that is
/// the form to compare (docs/mac/xdxc-oracle.md). Returns the container's bytes.
fn compile_console(t: &Tools, hlsl: &str) -> Result<Vec<u8>, String> {
    let src = source_for(hlsl, false);
    let exe = xdxc_exe(&t.sdk);
    let size = std::fs::metadata(&exe).map(|m| m.len()).unwrap_or(0).to_le_bytes();
    let key = key_of(&[src.as_bytes(), b"cs_6_6 -noacoprecompile", &size]);
    let cso = t.cache.join(format!("{key}.cso"));
    if !recompile() {
        if let Ok(b) = std::fs::read(&cso) {
            return Ok(b);
        }
    }
    let name = format!("{key}.hlsl");
    std::fs::write(t.cache.join(&name), &src).map_err(|e| e.to_string())?;
    // The compiler wants the source as a path it can open from its own working directory.
    let script = ". \"$0\"; exec \"$WINE\" \"$1\" -T cs_6_6 -E main -noacoprecompile -Fo \"$2\" \"$3\"";
    run(
        Command::new("bash")
            .current_dir(&t.cache)
            .arg("-c")
            .arg(script)
            .arg(t.sdk.join("wenv.sh"))
            .arg(&exe)
            .arg(format!("{key}.cso"))
            .arg(&name),
        "the console compiler (xdxc)",
    )?;
    std::fs::read(&cso).map_err(|e| format!("the console compiler made no output: {e}"))
}

/// Path A's compiler: the helper DXC, shader model 6.6.
fn compile_reference(t: &Tools, hlsl: &str) -> Result<Vec<u8>, String> {
    let src = source_for(hlsl, true);
    let key = key_of(&[src.as_bytes(), b"cs_6_6 -O3 helper"]);
    let out = t.cache.join(format!("{key}.a.dxil"));
    if !recompile() {
        if let Ok(b) = std::fs::read(&out) {
            return Ok(b);
        }
    }
    let name = t.cache.join(format!("{key}.a.hlsl"));
    std::fs::write(&name, &src).map_err(|e| e.to_string())?;
    run(
        Command::new(&t.helper).env("DXC_LIB", &t.dxc_lib).args(["-T", "cs_6_6", "-E", "main", "-Fo"]).arg(&out).arg(&name),
        "the helper DXC",
    )?;
    std::fs::read(&out).map_err(|e| format!("the helper DXC made no output: {e}"))
}

// ---------------------------------------------------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------------------------------------------------

fn views_of(inputs: &Inputs) -> Vec<View<'_>> {
    inputs
        .resources
        .iter()
        .map(|r| match r {
            Res::Srv(d) => View::Buffer { data: d, stride: Stride::Raw },
            Res::Cbv(d) => View::Cbv(d),
            Res::Uav(n) => View::RawUav(*n),
        })
        .collect()
}

fn constants_padded(inputs: &Inputs, at_least: usize) -> Vec<u32> {
    let mut c = inputs.constants.clone();
    c.resize(c.len().max(at_least).next_multiple_of(4).max(4), 0);
    c
}

/// Path A: DXIL from the helper DXC on a host root signature that is the root constants and nothing else (both heaps indexed
/// directly).
fn run_reference(host: &Host, dxil: &[u8], inputs: &Inputs) -> Result<Vec<Vec<u32>>, String> {
    let consts = constants_padded(inputs, 4);
    let stub = HostShader {
        stage: HwStage::Cs,
        dxil: Vec::new(),
        user_data: vec![UserData { index: consts.len() as u32 - 1, slot: 0, root: RootType::Constants, dword: 0 }],
        samplers: Vec::new(),
        static_samplers: Vec::new(),
        root_cbvs: Vec::new(),
        root_srvs: Vec::new(),
        root_uavs: Vec::new(),
        root_indexed_views: Vec::new(),
        vertex_reads: Vec::new(),
    };
    let root = host.root_signature_for(&[(&stub, D3D12_SHADER_VISIBILITY_ALL)])?;
    let pso = host.compute_pipeline_with(&root, dxil)?;
    let views = views_of(inputs);
    host.dispatch(&root, &pso, &Dispatch { roots: vec![(0, Root::Constants(consts))], views: &views, samplers: &[], groups: inputs.groups })
}

/// The container translated for a host of 32-lane waves under `settings`.
fn translated(container: &[u8], settings: &Settings) -> Result<HostShader, String> {
    let snap = super::live::snapshot_of(settings)?;
    let was = lower::TEST_HOST_WAVE.replace(Some(32));
    let r = super::live::scoped(&snap, || translate(container));
    lower::TEST_HOST_WAVE.set(was);
    r.map_err(|e| format!("{e:?}"))
}

/// What the console container's native program says it reads, checked against the heap order the inputs promise.
fn check_layout(container: &[u8], inputs: &Inputs) -> Result<(), String> {
    let c = crate::Container::parse(container).map_err(|e| format!("{e:?}"))?;
    let part = c.part(crate::dxbc::FourCC::DXIL).ok_or("no DXIL part")?;
    for n in crate::native_programs(part.data).map_err(|e| format!("{e:?}"))? {
        for u in &n.used {
            if u.root != RootType::Table {
                continue;
            }
            if u.table_offset_dwords % 8 != 0 {
                return Err(format!("a descriptor at table dword {}: not a whole 32-byte heap slot", u.table_offset_dwords));
            }
            let slot = (u.table_offset_dwords / 8) as usize;
            let want = match inputs.resources.get(slot) {
                Some(Res::Srv(_)) => ResourceType::Srv,
                Some(Res::Uav(_)) => ResourceType::Uav,
                Some(Res::Cbv(_)) => ResourceType::Cbv,
                None => return Err(format!("the program reads table slot {slot}, the inputs have {} views", inputs.resources.len())),
            };
            if u.kind != want {
                return Err(format!("table slot {slot}: the program reads a {:?}, the inputs put a {want:?} there", u.kind));
            }
        }
    }
    Ok(())
}

/// Path B: the translated console program on the same bindings. `shader` is its translation.
fn run_console(host: &Host, shader: &HostShader, inputs: &Inputs) -> Result<Vec<Vec<u32>>, String> {
    let need = shader.user_data_dwords() as usize;
    let consts = constants_padded(inputs, 0);
    let mut ud = vec![0u32; need.max(1)];
    for u in &shader.user_data {
        ud[u.index as usize] = match u.root {
            RootType::Constants => *consts.get(u.dword as usize).ok_or("the program reads a root constant the inputs do not have")?,
            // One table at the start of the heap.
            RootType::Table => 0,
            other => return Err(format!("unsupported: user data of kind {other:?}")),
        };
    }
    let mut roots = vec![(0u32, Root::Constants(ud))];
    let mut rv: Vec<(u8, u8)> = shader
        .root_cbvs
        .iter()
        .map(|&r| (r, 0))
        .chain(shader.root_srvs.iter().map(|&r| (r, 1)))
        .chain(shader.root_uavs.iter().map(|&r| (r, 2)))
        .collect();
    rv.sort_unstable();
    rv.dedup();
    for (i, &(slot, kind)) in rv.iter().enumerate() {
        let param = 1 + i as u32;
        if kind == 1 && (slot == super::SLOT_TABLE_STRIDES || slot == super::SLOT_STRIDES) {
            // the bytes per element of each heap slot: raw views 1, constant buffers 0
            let strides: Vec<u32> = inputs
                .resources
                .iter()
                .map(|r| match r {
                    Res::Cbv(_) => 0,
                    _ => 1,
                })
                .collect();
            roots.push((param, Root::View { srv: true, data: strides }));
        } else {
            return Err(format!("unsupported: root view of kind {kind} at slot {slot}"));
        }
    }
    let root = host.root_signature_for(&[(shader, D3D12_SHADER_VISIBILITY_ALL)])?;
    let pso = host.compute_pipeline_with(&root, &shader.dxil)?;
    let views = views_of(inputs);
    host.dispatch(&root, &pso, &Dispatch { roots, views: &views, samplers: &[], groups: inputs.groups })
}

fn ulps(a: u32, b: u32) -> u64 {
    let key = |x: u32| -> i64 {
        let v = x as i32;
        if v < 0 { i64::from(i32::MIN) - i64::from(v) } else { i64::from(v) }
    };
    key(a).abs_diff(key(b))
}

fn compare(a: &[Vec<u32>], b: &[Vec<u32>], how: &Compare) -> Verdict {
    if a.len() != b.len() {
        return Verdict::Failed { stage: "compare", why: format!("path A left {} UAVs, path B {}", a.len(), b.len()) };
    }
    let mut total = 0;
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        let (x, y) = match how.canon {
            Some(f) => (f(i, x), f(i, y)),
            None => (x.clone(), y.clone()),
        };
        total += x.len();
        let mut differing = 0;
        let mut first = Vec::new();
        for (k, (p, q)) in x.iter().zip(&y).enumerate() {
            let same = p == q
                || (how.ulps > 0 && ulps(*p, *q) <= u64::from(how.ulps))
                || (how.ulps > 0 && f32::from_bits(*p).is_nan() && f32::from_bits(*q).is_nan());
            if !same {
                differing += 1;
                if first.len() < 4 {
                    first.push((k, *p, *q));
                }
            }
        }
        if x.len() != y.len() {
            differing += x.len().abs_diff(y.len());
        }
        if differing > 0 {
            return Verdict::Different(Diff { uav: i, words_differing: differing, words_total: x.len(), first });
        }
    }
    Verdict::Same { words: total }
}

fn skip(why: impl Into<String>) -> Verdict {
    let why = why.into();
    eprintln!("SKIPPED: {why}");
    Verdict::Skipped(why)
}

/// Runs `hlsl` down both paths with `inputs` and compares the UAVs. `settings` apply to path B's translation
/// (`VICTORY_WAVE_WIDE`, `VICTORY_F16_NATIVE`, ...). Returns the verdict and, when both ran, the outputs.
pub fn dual_path_with(hlsl: &str, inputs: &Inputs, settings: &Settings, how: &Compare) -> (Verdict, Option<Outputs>) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let t = match tools() {
        Ok(t) => t,
        Err(e) => return (skip(e), None),
    };
    let host = match Host::new(false) {
        Ok(h) => h,
        Err(e) => return (skip(format!("no executor: {e}")), None),
    };
    let fail = |stage: &'static str, why: String| (Verdict::Failed { stage, why }, None);
    let a_dxil = match compile_reference(&t, hlsl) {
        Ok(d) => d,
        Err(e) => return fail("path A: compile (helper DXC)", e),
    };
    let container = match compile_console(&t, hlsl) {
        Ok(d) => d,
        Err(e) => return fail("path B: compile (console compiler)", e),
    };
    let a = match run_reference(&host, &a_dxil, inputs) {
        Ok(o) => o,
        Err(e) => return fail("path A: run", e),
    };
    if let Err(e) = check_layout(&container, inputs) {
        return fail("path B: layout", e);
    }
    let shader = match translated(&container, settings) {
        Ok(s) => s,
        Err(e) => return fail("path B: translate", e),
    };
    let b = match run_console(&host, &shader, inputs) {
        Ok(o) => o,
        Err(e) => return fail("path B: run", e),
    };
    (compare(&a, &b, how), Some(Outputs { a, b }))
}

/// [`dual_path_with`] with the translator's defaults and an exact comparison.
pub fn dual_path(hlsl: &str, inputs: &Inputs) -> Verdict {
    dual_path_with(hlsl, inputs, &Vec::new(), &Compare::EXACT).0
}

/// The same HLSL with path B translated three ways: `VICTORY_WAVE_WIDE` = 0 and 1 (the two forms a console wave64 program may
/// take on a host of 32-lane waves) and the default; path A runs once. Each verdict is path A against that form.
pub struct WaveVerdicts {
    pub wide0: Verdict,
    pub wide1: Verdict,
    pub outputs: Option<(Outputs, Outputs)>,
}

pub fn dual_path_wave_wide(hlsl: &str, inputs: &Inputs, how: &Compare) -> WaveVerdicts {
    let s = |v: &str| vec![("VICTORY_WAVE_WIDE".to_string(), v.to_string())];
    let (wide0, o0) = dual_path_with(hlsl, inputs, &s("0"), how);
    let (wide1, o1) = dual_path_with(hlsl, inputs, &s("1"), how);
    WaveVerdicts { wide0, wide1, outputs: o0.zip(o1) }
}

/// A host of 32-lane waves only: a program whose truth does not depend on the wave size runs everywhere the executor does. Used
/// by tests that want the device's own report of lanes.
pub fn device_lanes() -> Option<(u32, u32)> {
    Host::new(false).ok().map(|h| (h.caps().wave_lane_min, h.caps().wave_lane_max))
}

#[allow(dead_code)]
const _TYPES: Option<DXGI_FORMAT> = None;

// ---------------------------------------------------------------------------------------------------------------------
// Input helpers
// ---------------------------------------------------------------------------------------------------------------------

/// xorshift64*: the inputs are fixed by the seed, not by the machine.
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }
}

pub fn words(n: usize, seed: u64, f: impl Fn(u32) -> u32) -> Vec<u32> {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15 ^ seed.wrapping_mul(0xD6E8_FEB8_6659_FD93) | 1);
    (0..n).map(|_| f(r.next())).collect()
}

/// Sorted copy (a compare canon for outputs whose order is the atomics' race).
pub fn sorted(_uav: usize, w: &[u32]) -> Vec<u32> {
    let mut v = w.to_vec();
    v.sort_unstable();
    v
}

/// Reports a verdict and, for tests, panics only on what a test cannot accept (a path that did not run).
pub fn report(name: &str, v: &Verdict) {
    eprintln!("xdxc-oracle {name}: {}", v.summary());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prelude_selects_a_binding_style() {
        let a = source_for("float4 x;", true);
        let b = source_for("float4 x;", false);
        assert!(a.starts_with("#define DUAL_A 1\n"));
        assert!(b.starts_with("#define DUAL_A 0\n"));
        assert!(a.ends_with("float4 x;") && b.ends_with("float4 x;"));
    }

    #[test]
    fn the_compare_tolerates_what_it_says_it_tolerates() {
        let a = vec![vec![1.0f32.to_bits(), f32::NAN.to_bits(), 5]];
        let b = vec![vec![f32::from_bits(1.0f32.to_bits() + 1).to_bits(), 0x7FC0_0001, 5]];
        assert!(!compare(&a, &b, &Compare::EXACT).is_same());
        let loose = Compare { ulps: 1, canon: None, reason: "test" };
        assert!(compare(&a, &b, &loose).is_same());
        let sort = Compare { ulps: 0, canon: Some(sorted), reason: "test" };
        assert!(compare(&[vec![3, 1, 2]], &[vec![1, 2, 3]], &sort).is_same());
    }

    fn scale_add_inputs() -> Inputs {
        Inputs {
            constants: vec![7, 0xA5A5_0000],
            resources: vec![Res::Srv(words(256, 1, |x| x)), Res::Uav(256), Res::Uav(256)],
            groups: [4, 1, 1],
        }
    }

    /// The harness itself: the simplest program through both paths. The CPU model checks path A, so a harness fault is not
    /// mistaken for a translator one.
    #[test]
    fn a_scale_and_add_is_the_same_on_both_paths() {
        let hlsl = include_str!("../../tests/xdxc/scale_add.hlsl");
        let inputs = scale_add_inputs();
        let (v, out) = dual_path_with(hlsl, &inputs, &Vec::new(), &Compare::EXACT);
        report("scale_add", &v);
        if v.is_skipped() {
            return;
        }
        let out = out.unwrap_or_else(|| panic!("{}", v.summary()));
        let Res::Srv(src) = &inputs.resources[0] else { unreachable!() };
        let want0: Vec<u32> = src.iter().map(|x| x.wrapping_mul(3).wrapping_add(7)).collect();
        let want1: Vec<u32> = src.iter().map(|x| x ^ 0xA5A5_0000).collect();
        assert_eq!(out.a, vec![want0, want1], "path A against the CPU model");
        assert!(v.is_same(), "{}", v.summary());
    }

    // ---- helpers shared by the cases ----

    fn with_variant(hlsl: &str, v: u32) -> String {
        format!("#define VARIANT {v}\n{hlsl}")
    }

    /// Path A against a CPU model of the HLSL (after the case's canon): the truth the other path is measured by must itself be right.
    fn a_matches_cpu(name: &str, out: &Outputs, cpu: &[Vec<u32>], how: &Compare) {
        let c = compare(&out.a, cpu, how);
        assert!(c.is_same(), "{name}: path A is not what the HLSL says (CPU model): {}", c.summary());
    }

    fn wave_label(w: &WaveVerdicts) -> String {
        format!("A vs B(VICTORY_WAVE_WIDE=0): {} ; A vs B(VICTORY_WAVE_WIDE=1): {}", w.wide0.summary(), w.wide1.summary())
    }

    // ---- wave prefix sum + atomic allocation (the three containers whose wide and non-wide forms differ) ----

    const WAVE_ALLOC: &str = include_str!("../../tests/xdxc/wave_alloc.hlsl");

    fn alloc_inputs() -> (Inputs, Vec<u32>) {
        let src = words(256, 11, |x| x);
        let inputs = Inputs { constants: vec![0; 4], resources: vec![Res::Srv(src.clone()), Res::Uav(2048), Res::Uav(1)], groups: [4, 1, 1] };
        (inputs, src)
    }

    /// recs sorted (the slot order is the atomic's race), the counter as it is.
    fn alloc_canon(uav: usize, w: &[u32]) -> Vec<u32> {
        if uav == 0 { sorted(uav, w) } else { w.to_vec() }
    }

    fn alloc_cpu(variant: u32, src: &[u32]) -> Vec<Vec<u32>> {
        let mut recs = vec![0u32; 2048];
        let mut n = 0;
        for (t, v) in src.iter().enumerate() {
            let c = match variant {
                0 | 1 => (v & 7) as usize,
                _ => (v & 1) as usize,
            };
            for _ in 0..c {
                recs[n] = t as u32 + 1;
                n += 1;
            }
        }
        vec![recs, vec![n as u32]]
    }

    fn wave_alloc(variant: u32) {
        let (inputs, src) = alloc_inputs();
        let how = Compare {
            ulps: 0,
            canon: Some(alloc_canon),
            reason: "which wave wins the counter first (and so which slots a lane gets) is the atomic's race; the multiset of records and the total are the result",
        };
        let hlsl = with_variant(WAVE_ALLOC, variant);
        let w = dual_path_wave_wide(&hlsl, &inputs, &how);
        report(&format!("wave_alloc{variant}"), &w.wide0);
        report(&format!("wave_alloc{variant}"), &w.wide1);
        if w.wide0.is_skipped() {
            return;
        }
        eprintln!("xdxc-oracle wave_alloc{variant}: {}", wave_label(&w));
        if let Some((o0, _)) = &w.outputs {
            let cpu: Vec<Vec<u32>> = alloc_cpu(variant, &src);
            a_matches_cpu(&format!("wave_alloc{variant}"), o0, &cpu, &how);
        }
        record(&format!("wave_alloc{variant}"), &w);
    }

    /// Expected outcomes per case, once seen: (wide=0 same, wide=1 same). A change in either direction fails the test, so a
    /// translator fix or break shows.
    fn record(name: &str, w: &WaveVerdicts) {
        let want = expected_wave(name);
        let got = (w.wide0.is_same(), w.wide1.is_same());
        assert_eq!(got, want, "{name}: (wide=0 same, wide=1 same) changed: {}", wave_label(w));
    }

    fn expected_wave(name: &str) -> (bool, bool) {
        match name {
            _ => (true, true),
        }
    }

    #[test]
    fn wave_prefix_sum_and_atomic_allocation() {
        wave_alloc(0);
    }

    #[test]
    fn wave_alloc_with_a_divergent_exec() {
        wave_alloc(1);
    }

    #[test]
    fn wave_stream_compaction_by_ballot() {
        wave_alloc(2);
    }

    // ---- S3: divergent loops and group barriers ----

    const S3: &str = include_str!("../../tests/xdxc/s3_loop_barrier.hlsl");

    fn s3_cpu(variant: u32, src: &[u32], x: u32) -> Vec<u32> {
        let mut out = vec![0u32; src.len()];
        for g in 0..src.len() / 64 {
            let v = &src[g * 64..g * 64 + 64];
            if variant == 0 {
                let acc: Vec<u32> = (0..64u32)
                    .map(|gi| {
                        let mut acc = gi.wrapping_add(x);
                        for i in 0..(v[gi as usize] & 15) {
                            acc = acc.wrapping_mul(3).wrapping_add(i);
                            if acc & 1 != 0 {
                                acc ^= v[gi as usize];
                            }
                        }
                        acc
                    })
                    .collect();
                let nb: Vec<u32> = (0..64).map(|gi| acc[(gi + 1) & 63] ^ acc[(gi + 37) & 63]).collect();
                let l: Vec<u32> = (0..64).map(|gi| nb[gi].wrapping_add(acc[gi])).collect();
                for gi in 0..64 {
                    out[g * 64 + gi] = l[63 - gi] ^ acc[gi];
                }
            } else {
                let mut acc: Vec<u32> = v.to_vec();
                let mut lds = vec![0u32; 64];
                for r in 0..4u32 {
                    for gi in 0..64 {
                        if (v[gi] >> r) & 1 != 0 {
                            lds[gi] = lds[gi].wrapping_add(acc[gi]).wrapping_add(r);
                        }
                    }
                    let next: Vec<u32> = (0..64).map(|gi| acc[gi].wrapping_add(lds[(gi + r as usize + 1) & 63])).collect();
                    acc = next;
                }
                for gi in 0..64 {
                    out[g * 64 + gi] = acc[gi] ^ lds[gi];
                }
            }
        }
        out
    }

    fn s3(variant: u32) {
        let src = words(256, 21, |x| x);
        let inputs = Inputs { constants: vec![5], resources: vec![Res::Srv(src.clone()), Res::Uav(256)], groups: [4, 1, 1] };
        let hlsl = with_variant(S3, variant);
        let w = dual_path_wave_wide(&hlsl, &inputs, &Compare::EXACT);
        report(&format!("s3_loop_barrier{variant}"), &w.wide0);
        report(&format!("s3_loop_barrier{variant}"), &w.wide1);
        if w.wide0.is_skipped() {
            return;
        }
        if let Some((o0, _)) = &w.outputs {
            a_matches_cpu(&format!("s3_loop_barrier{variant}"), o0, &[s3_cpu(variant, &src, 5)], &Compare::EXACT);
        }
        record(&format!("s3_loop_barrier{variant}"), &w);
    }

    #[test]
    fn s3_a_divergent_loop_then_barriers() {
        s3(0);
    }

    #[test]
    fn s3_a_barrier_in_a_loop_with_a_divergent_body() {
        s3(1);
    }

    // ---- S7: f16 conversions ----

    const F16: &str = include_str!("../../tests/xdxc/f16_convert.hlsl");

    fn f16_nan_canon(uav: usize, w: &[u32]) -> Vec<u32> {
        w.iter()
            .map(|&x| match uav {
                0 if x & 0x7C00 == 0x7C00 && x & 0x3FF != 0 => (x & 0x8000) | 0x7E00,
                1 if x & 0x7F80_0000 == 0x7F80_0000 && x & 0x007F_FFFF != 0 => (x & 0x8000_0000) | 0x7FC0_0000,
                _ => x,
            })
            .collect()
    }

    fn f16_to_f32_cpu(h: u32) -> u32 {
        let (s, e, m) = ((h >> 15) & 1, (h >> 10) & 31, h & 0x3FF);
        let bits = match (e, m) {
            (0, 0) => s << 31,
            (0, m) => {
                let mut e = 127 - 15 + 1;
                let mut m = m;
                while m & 0x400 == 0 {
                    m <<= 1;
                    e -= 1;
                }
                s << 31 | (e as u32) << 23 | (m & 0x3FF) << 13
            }
            (31, 0) => s << 31 | 0x7F80_0000,
            (31, m) => s << 31 | 0x7F80_0000 | m << 13,
            (e, m) => s << 31 | (e + 112) << 23 | m << 13,
        };
        bits
    }

    /// f32 -> f16 bits, rounding to nearest even (`round_to_zero` truncates instead).
    fn f32_to_f16_cpu(x: u32, round_to_zero: bool) -> u32 {
        let (s, e, m) = ((x >> 16) & 0x8000, ((x >> 23) & 0xFF) as i32, x & 0x7F_FFFF);
        if e == 255 {
            return s | 0x7C00 | if m != 0 { 0x200 | (m >> 13) } else { 0 };
        }
        let e = e - 127 + 15;
        let (mant, shift, exp) = if e >= 1 { (m | 0x80_0000, 13, e as u32) } else { (m | 0x80_0000, (14 - e) as u32, 0) };
        if shift > 24 {
            return s;
        }
        let q = mant >> shift;
        let rem = mant & ((1 << shift) - 1);
        let half = 1u32 << (shift - 1);
        let up = !round_to_zero && (rem > half || (rem == half && q & 1 == 1));
        let v = if exp == 0 { q } else { ((exp << 10) | (q & 0x3FF)) + 0 };
        let v = if exp == 0 { v + u32::from(up) } else { v + u32::from(up) };
        if v >= 0x7C00 {
            return s | if round_to_zero { 0x7BFF } else { 0x7C00 };
        }
        s | v
    }

    fn f16_inputs() -> Inputs {
        let n = 65536usize;
        let mut r = Rng(0xF16F_16F1_6F16_F16F);
        let in0: Vec<u32> = (0..n)
            .map(|i| match i % 4 {
                0 => r.next(),
                // exactly half way between two halves, ties to even or not
                1 => {
                    let e = 100 + r.next() % 50;
                    (r.next() & 0x8000_0000) | e << 23 | (r.next() & 0x3FF) << 13 | 0x1000
                }
                // inside the normal range of a half
                2 => f32::to_bits(((r.next() % 2_000_000) as f32 - 1_000_000.0) / 977.0),
                // denormal halves, overflow, infinities, zero, NaNs
                _ => match r.next() % 8 {
                    0 => 0x3300_0000 | r.next() & 0x00FF_FFFF,
                    1 => 0x4780_0000 | r.next() & 0x007F_FFFF,
                    2 => 0x7F80_0000,
                    3 => 0xFF80_0000,
                    4 => 0x8000_0000,
                    5 => 0x7FC0_0000 | r.next() & 0x3F_FFFF,
                    6 => 0x477F_E000 + (r.next() & 0x3FFF),
                    _ => r.next() & 0x0000_7FFF_u32 | 0x3800_0000,
                },
            })
            .collect();
        let in1: Vec<u32> = (0..n).map(|i| (r.next() << 16) | i as u32).collect();
        Inputs { constants: vec![0; 4], resources: vec![Res::Srv(in0), Res::Srv(in1), Res::Uav(n as u32), Res::Uav(n as u32)], groups: [1024, 1, 1] }
    }

    #[test]
    fn s7_f16_conversions() {
        let inputs = f16_inputs();
        let how = Compare { ulps: 0, canon: Some(f16_nan_canon), reason: "the payload of a NaN is not compared (a NaN is a NaN)" };
        let (v, out) = dual_path_with(F16, &inputs, &Vec::new(), &how);
        report("f16_convert", &v);
        if v.is_skipped() {
            return;
        }
        let out = out.unwrap_or_else(|| panic!("{}", v.summary()));
        let (Res::Srv(in0), Res::Srv(in1)) = (&inputs.resources[0], &inputs.resources[1]) else { unreachable!() };
        // path A: widening is exact; narrowing is checked against both roundings and the counts are reported
        let wide: Vec<u32> = in1.iter().map(|h| f16_to_f32_cpu(h & 0xFFFF)).collect();
        let c = compare(&[out.a[1].clone()], &[wide], &how);
        assert!(c.is_same(), "f16 -> f32 on path A is not exact: {}", c.summary());
        let (mut rne, mut rtz) = (0, 0);
        for (x, h) in in0.iter().zip(&out.a[0]) {
            let h = f16_nan_canon(0, &[*h])[0];
            rne += usize::from(f16_nan_canon(0, &[f32_to_f16_cpu(*x, false)])[0] != h);
            rtz += usize::from(f16_nan_canon(0, &[f32_to_f16_cpu(*x, true)])[0] != h);
        }
        eprintln!("xdxc-oracle f16_convert: path A f32->f16 differs from round-to-nearest-even in {rne} of 65536, from round-to-zero in {rtz}");
        assert!(v.is_same(), "{}", v.summary());
    }

    // ---- v_div_fixup: the quotient of special values ----

    const DIV: &str = include_str!("../../tests/xdxc/div_nan.hlsl");

    fn special_floats() -> Vec<u32> {
        vec![
            0x0000_0000, 0x8000_0000, 0x3F80_0000, 0xBF80_0000, 0x4040_0000, 0x3DCC_CCCD, 0x7F80_0000, 0xFF80_0000, 0x7FC0_0000, 0xFFC0_0000,
            0x7FC0_1234, 0x7FA0_0000, 0x0000_0001, 0x807F_FFFF, 0x0080_0000, 0x7F7F_FFFF, 0xFF7F_FFFF, 0x7E80_0000, 0x0180_0000, 0x1E3C_E508,
            0x6015_8CFB, 0x3F7F_FFFF, 0x3F80_0001, 0x0040_0000, 0x4B00_0000, 0xC2F6_E979, 0x4E6E_6B28, 0x0000_FFFF, 0x7F00_0000, 0x00FF_FFFF,
            0x3EAA_AAAB, 0x4120_0000,
        ]
    }

    fn nan_canon_f32(_uav: usize, w: &[u32]) -> Vec<u32> {
        w.iter().map(|&x| if x & 0x7F80_0000 == 0x7F80_0000 && x & 0x007F_FFFF != 0 { (x & 0x8000_0000) | 0x7FC0_0000 } else { x }).collect()
    }

    #[test]
    fn v_div_fixup_special_quotients() {
        let t = special_floats();
        assert_eq!(t.len(), 32);
        let inputs = Inputs { constants: vec![0; 4], resources: vec![Res::Srv(t.clone()), Res::Uav(1024)], groups: [16, 1, 1] };
        let how = Compare { ulps: 0, canon: Some(nan_canon_f32), reason: "the payload of a NaN is not compared; its sign is" };
        let (v, out) = dual_path_with(DIV, &inputs, &Vec::new(), &how);
        report("div_nan", &v);
        if v.is_skipped() {
            return;
        }
        let out = out.unwrap_or_else(|| panic!("{}", v.summary()));
        let cpu: Vec<u32> = (0..1024).map(|k| (f32::from_bits(t[k / 32]) / f32::from_bits(t[k % 32])).to_bits()).collect();
        let c = compare(&out.a, &[cpu], &how);
        eprintln!("xdxc-oracle div_nan: path A against Rust IEEE division: {}", c.summary());
        assert!(v.is_same(), "{}", v.summary());
    }

    // ---- out-of-range buffer accesses ----

    const OOB: &str = include_str!("../../tests/xdxc/oob_buffer.hlsl");

    #[test]
    fn out_of_range_buffer_accesses() {
        let src = words(16, 31, |x| x | 1);
        let inputs = Inputs {
            constants: vec![1 << 20],
            resources: vec![Res::Srv(src.clone()), Res::Uav(40), Res::Uav(512), Res::Uav(8)],
            groups: [1, 1, 1],
        };
        let (v, out) = dual_path_with(OOB, &inputs, &Vec::new(), &Compare::EXACT);
        report("oob_buffer", &v);
        if v.is_skipped() {
            return;
        }
        let out = out.unwrap_or_else(|| panic!("{}", v.summary()));
        let at = |i: usize| src.get(i).copied().unwrap_or(0);
        let mut dst = vec![0u32; 40];
        let mut got = vec![0u32; 512];
        for t in 0..64usize {
            if t < 40 {
                dst[t] = t as u32 + 1;
            }
            let (a, b0, b1) = (at(t), at(t), at(t + 1));
            let _ = b0;
            let c = [at(t), at(t + 1), at(t + 2), at(t + 3)];
            let k = t * 8;
            got[k] = a.wrapping_add(b1.wrapping_mul(3));
            got[k + 1] = c[0] ^ c[1] ^ c[2] ^ c[3];
            got[k + 2] = 0;
            got[k + 3] = 0;
            got[k + 4] = c[3];
            got[k + 5] = c[0];
            got[k + 6] = c[2];
            got[k + 7] = a;
        }
        // Load2/Load4 straddling the end: each dword that is out of range reads 0 (HLSL's out-of-bounds rule)
        a_matches_cpu("oob_buffer", &out, &[dst, got, vec![1, 2, 3, 4, 2, 3, 4, 5]], &Compare::EXACT);
        assert!(v.is_same(), "{}", v.summary());
    }

    // ---- LDS atomics ----

    const LDS: &str = include_str!("../../tests/xdxc/lds_atomics.hlsl");

    fn lds_canon(uav: usize, w: &[u32]) -> Vec<u32> {
        match uav {
            0 => w.chunks(8).flat_map(|c| c.iter().enumerate().map(|(i, &x)| if i == 7 { 0 } else { x })).collect(),
            _ => w
                .chunks(192)
                .flat_map(|g| {
                    let mut a = g[..64].to_vec();
                    a.sort_unstable();
                    let mut b = g[64..129].to_vec();
                    b.sort_unstable();
                    a.into_iter().chain(b).chain(g[129..].iter().copied())
                })
                .collect(),
        }
    }

    #[test]
    fn lds_atomics_agree_and_return_a_permutation() {
        let src = words(256, 41, |x| x);
        let inputs = Inputs { constants: vec![0; 4], resources: vec![Res::Srv(src.clone()), Res::Uav(32), Res::Uav(768)], groups: [4, 1, 1] };
        let how = Compare {
            ulps: 0,
            canon: Some(lds_canon),
            reason: "the order lanes win a groupshared atomic is a race: the exchanged word's last value and the returns are sorted",
        };
        let (v, out) = dual_path_with(LDS, &inputs, &Vec::new(), &how);
        report("lds_atomics", &v);
        if v.is_skipped() {
            return;
        }
        let out = out.unwrap_or_else(|| panic!("{}", v.summary()));
        let mut fin = vec![0u32; 32];
        let mut olds = vec![0u32; 768];
        for g in 0..4 {
            let v = &src[g * 64..g * 64 + 64];
            let f = &mut fin[g * 8..g * 8 + 8];
            f[0] = 64;
            f[1] = v.iter().copied().fold(0xFFFF_FFFF, u32::min);
            f[2] = v.iter().copied().fold(0, u32::max);
            f[3] = v.iter().fold(0, |a, x| a | 1 << (x & 31));
            f[4] = v.iter().fold(0x1234_5678, |a, x| a ^ x);
            f[5] = v.iter().enumerate().filter(|(i, _)| i & 1 == 1).fold(0xFFFF_FFFF, |a, (_, x)| a & (x | 0x0F0F_0F0F));
            f[6] = v.iter().enumerate().filter(|(i, _)| *i >= 32).fold(7, |a, (_, x)| a.wrapping_add(x & 0xFF));
            f[7] = 0;
            let o = &mut olds[g * 192..g * 192 + 192];
            for i in 0..64 {
                o[i] = i as u32;
                o[64 + i] = i as u32 + 1;
            }
            o[64] = 100;
            o[128] = 64; // some lane's id + 1 is last; with the 63 returned ids and 100, {100, 1..=64} is the multiset
            let _ = &o[0];
        }
        // the exchange multiset is {100, 1..=64}: the 64 returns plus the final value
        for g in 0..4 {
            let o = &mut olds[g * 192 + 64..g * 192 + 129];
            for (i, w) in o.iter_mut().enumerate() {
                *w = if i == 0 { 100 } else { i as u32 };
            }
        }
        a_matches_cpu("lds_atomics", &out, &[fin, olds], &how);
        assert!(v.is_same(), "{}", v.summary());
    }

    // ---- quad operations ----

    const QUAD: &str = include_str!("../../tests/xdxc/quad_ops.hlsl");

    #[test]
    fn quad_operations_read_the_neighbours_of_a_two_by_two() {
        let src = words(16 * 16, 51, |x| x);
        let inputs = Inputs { constants: vec![0; 4], resources: vec![Res::Srv(src.clone()), Res::Uav(16 * 16 * 6)], groups: [2, 2, 1] };
        let (v, out) = dual_path_with(QUAD, &inputs, &Vec::new(), &Compare::EXACT);
        report("quad_ops", &v);
        if v.is_skipped() {
            return;
        }
        let out = out.unwrap_or_else(|| panic!("{}", v.summary()));
        let mut want = vec![0u32; 16 * 16 * 6];
        for y in 0..16usize {
            for x in 0..16usize {
                let f = |xx: usize, yy: usize| src[yy * 16 + xx];
                let (qx, qy) = (x & !1, y & !1);
                let o = (y * 16 + x) * 6;
                want[o] = f(x ^ 1, y);
                want[o + 1] = f(x, y ^ 1);
                want[o + 2] = f(x ^ 1, y ^ 1);
                want[o + 3] = f(qx, qy + 1); // lane 2 of the quad: x even, y odd
                want[o + 4] = if (x & 7) & 1 == 1 { f(qx + 1, qy) } else { 0 };
                want[o + 5] = f(x, y);
            }
        }
        a_matches_cpu("quad_ops", &out, &[want], &Compare::EXACT);
        assert!(v.is_same(), "{}", v.summary());
    }
}
