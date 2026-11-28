//! Mesh shader as a compute pass, the runtime side (docs/metal/mesh-compute.md,
//! section 8, "Interface v1").
//!
//! A mesh pipeline Metal cannot take (its output is over 32 KB after the
//! repack, Apple's GPU compiler crashes on it, or the pair is on the denylist)
//! is made as an **emulation** instead of being refused: a compute pipeline
//! (the shader's compute form, which writes every output into the scratch)
//! and a vertex-and-pixel pipeline (the vertex shader fetches the scratch;
//! the pixel shader is the original one). Both come from the mesh shader's own
//! container, in an extra `VMCS` part (see [`Bundle`]); the translator makes
//! them, this back only reads them.
//!
//! What lives here: the container part, the extended root signature, the
//! pipelines ([`Emulated`]), the encoding of one emulated dispatch
//! ([`CommandList::emu_direct`]) and of an indirect one
//! ([`CommandList::emu_indirect`], with the kernel that sizes the scratch
//! on the GPU), and the counters. Where in a stream the work goes (before the
//! render encoder, split before a draw that reads what an earlier one wrote)
//! is the interpreter's, `exec/emu.rs`.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::device::Device;
use crate::error::{Error, Result};
use crate::indirect::{IndirectDispatch, Kernels};
use crate::memory::{Buffer, BufferKind};
use crate::mesh_compute::Layout;
use crate::msc::{
    self, CompileOptions, MscError, Param, ParamKind, RootSignatureDesc, TopLevelLayout, Visibility,
};
use crate::pipeline::ComputePipeline;
use crate::record::CommandList;
use crate::render::{GraphicsDesc, GraphicsPipeline};
use crate::stages::{DISPATCH, FRAGMENT, MESH, OBJECT, VERTEX, VIS_DEVICE};
use objc2_metal::MTLPrimitiveTopologyClass;

// ---- the switch and the counters ------------------------------------------

/// Whether `VICTORY_METAL_MESH_COMPUTE` allows the emulation (default: yes;
/// `0`, `false`, `off`, `no` switch it off).
pub fn enabled_from(value: Option<&str>) -> bool {
    !matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "false" | "off" | "no")
    )
}

/// [`enabled_from`] of the process environment, read once.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        enabled_from(
            std::env::var("VICTORY_METAL_MESH_COMPUTE")
                .ok()
                .as_deref(),
        )
    })
}

/// What the emulation did since the process started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Pipelines made as emulations.
    pub pipelines: u64,
    /// Draws whose kernel ran before the render encoder (the pre-scan).
    pub hoisted: u64,
    /// Draws whose kernel was encoded right before them, splitting the pass
    /// (the pass-split rule: an earlier draw of the pass wrote what they read).
    pub split: u64,
    /// Draws whose kernel was encoded in place because the pass had a
    /// command the pre-scan does not look past (counted apart from `split`).
    pub in_place: u64,
    /// Draws the rule could not decide (a heap-indexed access on one side).
    pub unproven: u64,
    /// Indirect dispatches emulated (one count per `ExecuteIndirect`).
    pub indirect: u64,
    /// Draws dropped, for lack of scratch or a refused dispatch.
    pub dropped: u64,
}

static PIPELINES: AtomicU64 = AtomicU64::new(0);
static HOISTED: AtomicU64 = AtomicU64::new(0);
static SPLIT: AtomicU64 = AtomicU64::new(0);
static IN_PLACE: AtomicU64 = AtomicU64::new(0);
static UNPROVEN: AtomicU64 = AtomicU64::new(0);
static INDIRECT: AtomicU64 = AtomicU64::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// The counters now.
pub fn counters() -> Counters {
    Counters {
        pipelines: PIPELINES.load(Ordering::Relaxed),
        hoisted: HOISTED.load(Ordering::Relaxed),
        split: SPLIT.load(Ordering::Relaxed),
        in_place: IN_PLACE.load(Ordering::Relaxed),
        unproven: UNPROVEN.load(Ordering::Relaxed),
        indirect: INDIRECT.load(Ordering::Relaxed),
        dropped: DROPPED.load(Ordering::Relaxed),
    }
}

pub(crate) fn count_hoisted() {
    HOISTED.fetch_add(1, Ordering::Relaxed);
}
pub(crate) fn count_split() {
    SPLIT.fetch_add(1, Ordering::Relaxed);
}
pub(crate) fn count_in_place() {
    IN_PLACE.fetch_add(1, Ordering::Relaxed);
}
pub(crate) fn count_unproven() {
    UNPROVEN.fetch_add(1, Ordering::Relaxed);
}
pub(crate) fn count_dropped() {
    DROPPED.fetch_add(1, Ordering::Relaxed);
}

/// `VICTORY_METAL_MESH_COMPUTE_SPLIT=unproven`: the pass-split rule also splits
/// on what it cannot decide (heap-indexed accesses), for A/B runs.
pub fn split_on_unproven() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("VICTORY_METAL_MESH_COMPUTE_SPLIT").is_ok_and(|v| v == "unproven")
    })
}

// ---- the scratch ring -------------------------------------------------------

/// Bytes of one scratch-ring segment.
pub const SCRATCH_SEGMENT: u64 = 16 << 20;

/// The scratch ring of a new queue: segment size and count.
/// `VICTORY_METAL_MESH_COMPUTE_RING_MB` is the total (default 96 MB, six
/// segments): one frame's problem meshes need about 50 MB by the census
/// (research/mesh-compute-check/REPORT.md) and a ring that is too small makes
/// a draw wait for the GPU or, in one submission, drop.
pub fn default_scratch_config() -> (u64, u32) {
    let mb: u64 = std::env::var("VICTORY_METAL_MESH_COMPUTE_RING_MB")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(96);
    let segments = (mb * (1 << 20)).div_ceil(SCRATCH_SEGMENT).max(2) as u32;
    (SCRATCH_SEGMENT, segments)
}

/// Groups the arena of one indirect dispatch holds, at most: the census's
/// largest front pass is 201 groups, one draw 66.
pub const INDIRECT_ARENA_GROUPS: u64 = 256;

// ---- the container part ----------------------------------------------------

/// FourCC of the container part that carries the emulation's products.
pub const VMCS: [u8; 4] = *b"VMCS";

/// The three products of the lowering, as the `VMCS` part holds them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bundle<'a> {
    /// The `vmcl 1` text.
    pub layout: &'a str,
    /// The compute form: a DXIL container.
    pub compute: &'a [u8],
    /// The vertex-fetch shader: a DXIL container.
    pub vertex: &'a [u8],
}

fn word(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

impl<'a> Bundle<'a> {
    /// Reads the `VMCS` part of a mesh shader's container. `Ok(None)`: it has
    /// none (the shader was not lowered); `Err`: it has one this back does not
    /// understand (another version, damaged).
    pub fn find(container: &'a [u8]) -> std::result::Result<Option<Bundle<'a>>, String> {
        let Some(p) = msc::dxil::part(container, &VMCS) else {
            return Ok(None);
        };
        let bad = |m: &str| Err(format!("the VMCS part: {m}"));
        if word(p, 0) != Some(1) {
            return bad("version is not 1");
        }
        if word(p, 4) != Some(0) {
            return bad("flags are not 0");
        }
        let range = |at: usize, what: &str| -> std::result::Result<&'a [u8], String> {
            let off = word(p, at).ok_or("cut short")? as usize;
            let len = word(p, at + 4).ok_or("cut short")? as usize;
            if !off.is_multiple_of(4) {
                return Err(format!("the VMCS part: {what} is not 4-aligned"));
            }
            off.checked_add(len)
                .and_then(|end| p.get(off..end))
                .ok_or_else(|| format!("the VMCS part: {what} is outside the part"))
        };
        let layout = range(8, "the layout")?;
        let compute = range(16, "the compute form")?;
        let vertex = range(24, "the fetch shader")?;
        let layout = std::str::from_utf8(layout)
            .map_err(|_| "the VMCS part: the layout is not UTF-8".to_string())?;
        Ok(Some(Bundle {
            layout,
            compute,
            vertex,
        }))
    }

    /// `container` with a `VMCS` part holding `self` (what the translator
    /// does; here for the tests and tools). The container's other parts are
    /// kept as they are, the digest is not recomputed (the converter does not
    /// check it).
    pub fn attach(&self, container: &[u8]) -> std::result::Result<Vec<u8>, String> {
        if container.get(..4) != Some(b"DXBC") {
            return Err("not a DXBC container".into());
        }
        let count = word(container, 28).ok_or("cut short")? as usize;
        let mut parts: Vec<([u8; 4], &[u8])> = Vec::new();
        for i in 0..count {
            let at = word(container, 32 + 4 * i).ok_or("cut short")? as usize;
            let four: [u8; 4] = container
                .get(at..at + 4)
                .and_then(|f| f.try_into().ok())
                .ok_or("a part cut short")?;
            let size = word(container, at + 4).ok_or("cut short")? as usize;
            let data = container.get(at + 8..at + 8 + size).ok_or("a part cut short")?;
            if four != VMCS {
                parts.push((four, data));
            }
        }
        let mut payload = Vec::new();
        let pad = |v: &mut Vec<u8>| {
            while !v.len().is_multiple_of(4) {
                v.push(0);
            }
        };
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.resize(32, 0);
        let put = |payload: &mut Vec<u8>, at: usize, data: &[u8]| {
            pad(payload);
            let off = payload.len() as u32;
            payload.extend_from_slice(data);
            payload[at..at + 4].copy_from_slice(&off.to_le_bytes());
            payload[at + 4..at + 8].copy_from_slice(&(data.len() as u32).to_le_bytes());
        };
        put(&mut payload, 8, self.layout.as_bytes());
        put(&mut payload, 16, self.compute);
        put(&mut payload, 24, self.vertex);
        pad(&mut payload);
        parts.push((VMCS, &payload));
        let n = parts.len();
        let mut out = Vec::new();
        out.extend_from_slice(&container[..28]);
        out.extend_from_slice(&(n as u32).to_le_bytes());
        let mut at = 32 + 4 * n;
        let mut offsets = Vec::new();
        for (_, d) in &parts {
            offsets.push(at as u32);
            at += 8 + d.len().next_multiple_of(4);
        }
        for o in &offsets {
            out.extend_from_slice(&o.to_le_bytes());
        }
        for (f, d) in &parts {
            out.extend_from_slice(f);
            out.extend_from_slice(&(d.len() as u32).to_le_bytes());
            out.extend_from_slice(d);
            out.resize(out.len().next_multiple_of(4), 0);
        }
        let total = out.len() as u32;
        out[24..28].copy_from_slice(&total.to_le_bytes());
        Ok(out)
    }
}

// ---- the extended root signature and the pipelines -------------------------

/// `root` plus the two parameters the emulation appends (section 8.2): the
/// scratch (a root UAV for the compute form, a root SRV for the fetch shader)
/// and the dispatch dimensions (a root CBV). `all_visible` makes every
/// parameter visible to every stage (the compute form reads what the mesh
/// shader read, whatever visibility said "mesh").
pub fn extend_root(root: &RootSignatureDesc, layout: &Layout, uav: bool, all_visible: bool) -> RootSignatureDesc {
    let mut r = root.clone();
    if all_visible {
        for p in &mut r.params {
            p.visibility = Visibility::All;
        }
    }
    r.params.push(Param {
        kind: if uav {
            ParamKind::Uav {
                register: layout.register,
                space: layout.space,
            }
        } else {
            ParamKind::Srv {
                register: layout.register,
                space: layout.space,
            }
        },
        visibility: Visibility::All,
    });
    r.params.push(Param {
        kind: ParamKind::Cbv {
            register: 0,
            space: layout.space,
        },
        visibility: Visibility::All,
    });
    r
}

/// The argument buffer of the application's root signature, as far as the
/// extension needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppLayout {
    /// Byte offset of each root parameter.
    pub offsets: Vec<u32>,
    /// Bytes of the whole buffer.
    pub size: u32,
    /// Where the pointer to the static samplers' table is, if there is one.
    pub static_table: Option<u32>,
}

/// The extended argument buffer: the application's, then the two new
/// parameters, then (when the signature has static samplers) the table pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtLayout {
    /// Bytes of the extended buffer, rounded up to 16.
    pub size: u32,
    /// Bytes the application's buffer contributes: all but the static table
    /// pointer, which the extension moves.
    pub copy: u32,
    /// Where the scratch address and the dimensions address go.
    pub scratch_at: u32,
    pub dims_at: u32,
    /// Where the static-table pointer goes in the extended buffer.
    pub static_at: Option<u32>,
}

/// A mesh pipeline emulated by a compute pipeline and a vertex pipeline.
pub struct Emulated {
    pub layout: Layout,
    pub compute: ComputePipeline,
    /// Threads per threadgroup of the compute form.
    pub threads: [u32; 3],
    pub fetch: GraphicsPipeline,
    pub ext: ExtLayout,
}

fn refuse<T>(m: impl Into<String>) -> std::result::Result<T, MscError> {
    Err(MscError::Unsupported(format!(
        "mesh pipeline emulation: {}",
        m.into()
    )))
}

fn ext_layout(
    app: &AppLayout,
    cs: &TopLevelLayout,
    gfx: &TopLevelLayout,
) -> std::result::Result<ExtLayout, MscError> {
    let n = app.offsets.len();
    if cs.entries.len() < n + 2 || gfx.entries.len() < n + 2 {
        return refuse("the extended signature has too few arguments");
    }
    for i in 0..n + 2 {
        if cs.entries[i].offset != gfx.entries[i].offset {
            return refuse("the compute and the vertex stages disagree on the argument buffer");
        }
    }
    if cs.size != gfx.size {
        return refuse("the compute and the vertex stages disagree on the argument buffer size");
    }
    for (i, &o) in app.offsets.iter().enumerate() {
        if cs.entries[i].offset != o {
            return refuse("the extension moved an argument of the application's signature");
        }
    }
    let static_of = |l: &TopLevelLayout| {
        l.static_sampler_table
            .and_then(|i| l.entries.get(i))
            .map(|e| e.offset)
    };
    if static_of(cs) != static_of(gfx) {
        return refuse("the stages disagree on the static sampler table");
    }
    Ok(ExtLayout {
        size: cs.size.next_multiple_of(16),
        copy: app.static_table.unwrap_or(app.size),
        scratch_at: cs.entries[n].offset,
        dims_at: cs.entries[n + 1].offset,
        static_at: static_of(cs),
    })
}

impl Emulated {
    /// Builds the emulation of the mesh pipeline `mesh` (whose stages are the
    /// mesh shader's container and the pixel shader) from the mesh shader's
    /// `VMCS` part. `Unsupported` when there is no part, it does not
    /// match the pipeline, or either pipeline is refused.
    pub fn build(
        device: &Device,
        mesh: &GraphicsDesc<'_>,
        app: &AppLayout,
    ) -> std::result::Result<Emulated, MscError> {
        let Some(ms) = mesh.ms else {
            return refuse("not a mesh pipeline");
        };
        if mesh.amp.is_some() {
            return refuse("an amplification stage");
        }
        let bundle = match Bundle::find(ms) {
            Ok(Some(b)) => b,
            Ok(None) => return refuse("the mesh shader has no VMCS part (not lowered)"),
            Err(e) => return refuse(e),
        };
        let layout = Layout::parse(bundle.layout).map_err(|e| MscError::Unsupported(e.to_string()))?;
        let options = CompileOptions {
            compat: mesh.compat,
            ..CompileOptions::default()
        };
        let cs_root = extend_root(mesh.root, &layout, true, true);
        let made = device.compute_pipeline_from_dxil(bundle.compute, Some(&cs_root), &options)?;
        let threads = made.reflection.threadgroup_size;
        if threads != layout.threads {
            return refuse(format!(
                "the compute form has {threads:?} threads, the layout says {:?}",
                layout.threads
            ));
        }
        let gfx_root = extend_root(mesh.root, &layout, false, false);
        let fetch_desc = GraphicsDesc {
            vs: Some(bundle.vertex),
            ps: mesh.ps,
            amp: None,
            ms: None,
            gs: None,
            root: &gfx_root,
            inputs: Vec::new(),
            targets: mesh.targets.clone(),
            topology: MTLPrimitiveTopologyClass::Triangle,
            sample_count: mesh.sample_count,
            sample_mask: mesh.sample_mask,
            alpha_to_coverage: mesh.alpha_to_coverage,
            has_depth_target: mesh.has_depth_target,
            depth_stencil: mesh.depth_stencil,
            raster: mesh.raster,
            compat: mesh.compat,
        };
        let fetch = device.graphics_pipeline_from_dxil(&fetch_desc)?;
        let ext = ext_layout(app, &made.reflection.layout, fetch.layout())?;
        PIPELINES.fetch_add(1, Ordering::Relaxed);
        Ok(Emulated {
            layout,
            compute: made.pipeline,
            threads,
            fetch,
            ext,
        })
    }

    /// Vertices of the fetch draw of one group.
    pub fn draw_vertices(&self) -> u32 {
        self.layout.draw_vertices()
    }
}

// ---- encoding ---------------------------------------------------------------

/// What a direct emulated dispatch left for its draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Direct {
    /// The extended argument buffer both the dispatch and the draw read.
    pub ext_va: u64,
    /// Groups (= instances of the fetch draw).
    pub groups: u32,
}

/// One record of an emulated indirect dispatch: what its draw reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fetch {
    pub ext_va: u64,
    /// The 64-byte draw block (see [`CommandList::draw_fetch_indirect`]).
    pub draw_va: u64,
}

/// The extended argument buffer for one dispatch.
fn fill_ext(
    ext: &ExtLayout,
    shadow: &[u8],
    static_va: Option<u64>,
    scratch_va: u64,
    dims_va: u64,
) -> Vec<u8> {
    let mut b = vec![0u8; ext.size as usize];
    let n = (ext.copy as usize).min(shadow.len()).min(b.len());
    b[..n].copy_from_slice(&shadow[..n]);
    b[ext.scratch_at as usize..ext.scratch_at as usize + 8]
        .copy_from_slice(&scratch_va.to_le_bytes());
    b[ext.dims_at as usize..ext.dims_at as usize + 8].copy_from_slice(&dims_va.to_le_bytes());
    if let (Some(at), Some(va)) = (ext.static_at, static_va) {
        b[at as usize..at as usize + 8].copy_from_slice(&va.to_le_bytes());
    }
    b
}

impl CommandList {
    /// `groups` threadgroups of the compute form of `em`, with the
    /// application's root arguments `shadow` (the bytes of its argument
    /// buffer) and the scratch of the ring: encoded into the current compute
    /// encoder. The draw is the caller's, after a barrier from the dispatch
    /// stage to the vertex stage ([`CommandList::emu_publish`]).
    pub fn emu_direct(
        &mut self,
        em: &Emulated,
        shadow: &[u8],
        static_va: Option<u64>,
        groups: [u32; 3],
    ) -> Result<Direct> {
        let n = u64::from(groups[0]) * u64::from(groups[1]) * u64::from(groups[2]);
        if n == 0 {
            return Err(Error::BadArgument("an emulated dispatch of no groups".into()));
        }
        let bytes = n
            .checked_mul(u64::from(em.layout.group_stride))
            .filter(|&b| b <= u64::from(i32::MAX as u32))
            .ok_or_else(|| {
                Error::Unsupported(format!(
                    "{n} groups of {} bytes of mesh scratch",
                    em.layout.group_stride
                ))
            })?;
        let scratch = self.alloc_scratch(bytes, 256)?;
        // The extended argument buffer, then the dimensions behind it.
        let size = u64::from(em.ext.size);
        let ring = self.alloc_constants(size + 16, 16)?;
        let dims = ring.gpu_va + size;
        ring.write_bytes(0, &fill_ext(&em.ext, shadow, static_va, scratch.gpu_va, dims))?;
        ring.write_u32s(size, &[groups[0], groups[1], groups[2], 0])?;
        self.set_root_arguments(&ring)?;
        self.dispatch(&em.compute, groups, em.threads)?;
        Ok(Direct {
            ext_va: ring.gpu_va,
            groups: n as u32,
        })
    }

    /// The writes of the emulated dispatches encoded so far are made visible
    /// to the stages that draw from them.
    pub fn emu_publish(&mut self) -> Result<()> {
        self.barrier(DISPATCH, VERTEX | FRAGMENT | OBJECT | MESH, VIS_DEVICE)
    }

    /// The draw of a direct dispatch.
    pub fn emu_draw(&mut self, em: &Emulated, d: &Direct) -> Result<()> {
        self.set_root_arguments_va(d.ext_va)?;
        self.draw_fetch(&em.fetch, em.draw_vertices(), d.groups)
    }

    /// The draw of one record of an indirect dispatch.
    pub fn emu_draw_record(&mut self, em: &Emulated, r: &Fetch) -> Result<()> {
        self.set_root_arguments_va(r.ext_va)?;
        self.draw_fetch_indirect(&em.fetch, r.draw_va)
    }

    /// An indirect mesh dispatch (`spec`: the records, as the list's
    /// `ExecuteIndirect` has them) as compute work: per record, a kernel
    /// sizes its slice of one scratch arena from the group counts the GPU
    /// holds (a record that does not fit the arena gets no groups, and the
    /// GPU counts it: [`EmuKernels::dropped_records`]), writes the record's
    /// extended argument buffer and its draw block, then the compute form runs
    /// as an indirect dispatch. Returns what the draws read, one per record.
    pub(crate) fn emu_indirect(
        &mut self,
        kernels: &Kernels,
        ek: &EmuKernels,
        em: &Emulated,
        spec: &IndirectDispatch<'_>,
        static_va: Option<u64>,
    ) -> Result<Vec<Fetch>> {
        self.live()?;
        let spec = IndirectDispatch {
            always_expand: true,
            draws: false,
            gs: None,
            action_words: 3,
            consumer: DISPATCH,
            ..*spec
        };
        let gstride = u64::from(em.layout.group_stride);
        let segment = self.queue().scratch_segment_bytes();
        let arena_bytes = (INDIRECT_ARENA_GROUPS * gstride)
            .min(segment)
            .min(u64::from(u32::MAX))
            / 256
            * 256;
        if arena_bytes < gstride {
            return Err(Error::Unsupported(format!(
                "a mesh group of {gstride} bytes does not fit a scratch-ring segment"
            )));
        }
        let arena = self.alloc_scratch(arena_bytes, 256)?;
        let cursor = self.alloc_constants(16, 16)?;
        cursor.write_u32s(0, &[0, 0, 0, 0])?;
        let chunk = ek.chunk()?;
        let stats_va = ek.stats_va()?;
        let ext = em.ext;
        let ext_words = (ext.size / 4).next_multiple_of(4);
        let ext_stride = u64::from(ext_words) * 4 + 16;
        let app_words = (ext.copy as usize).min(spec.base.len()) as u32 / 4;
        struct Chunk {
            out_va: u64,
            draw_va: u64,
        }
        let state = RefCell::new(Chunk { out_va: 0, draw_va: 0 });
        let mut fetches = Vec::new();
        let dispatch_threads = em.threads;
        let compute = em.compute.clone();
        self.execute_indirect_chunks(
            kernels,
            &spec,
            |l, v| {
                // Header (the kernel's parameters), a copy of the list's
                // argument buffer, then the extended buffers and draw blocks.
                let base_bytes = (spec.base.len() as u64).next_multiple_of(16);
                let head = 256u64;
                let out_at = head + base_bytes;
                let draw_at = out_at + v.k * ext_stride;
                let ring = l.alloc_constants(draw_at + v.k * 64, 16)?;
                ring.write_bytes(head, spec.base)?;
                let va = |at: u64| ring.gpu_va + at;
                let mut p = Vec::<u8>::with_capacity(head as usize);
                for q in [
                    v.disps_va,
                    v.tlabs_va.unwrap_or(0),
                    va(head),
                    va(out_at),
                    va(draw_at),
                    arena.gpu_va,
                    cursor.gpu_va,
                    stats_va,
                    static_va.unwrap_or(0),
                ] {
                    p.extend_from_slice(&q.to_le_bytes());
                }
                for w in [
                    v.k as u32,
                    (v.out_bytes / 4) as u32,
                    (v.tlab_bytes / 4) as u32,
                    app_words,
                    ext_words,
                    ext_words + 4,
                    ext.scratch_at / 4,
                    ext.dims_at / 4,
                    ext.static_at.map_or(u32::MAX, |a| a / 4),
                    gstride as u32,
                    arena_bytes as u32,
                    em.draw_vertices(),
                ] {
                    p.extend_from_slice(&w.to_le_bytes());
                }
                p.resize(head as usize, 0);
                ring.write_bytes(0, &p)?;
                l.set_address(30, ring.gpu_va)?;
                l.dispatch(&chunk, [1, 1, 1], [256, 1, 1])?;
                // The records' dispatch arguments (and the argument buffers)
                // are read by the dispatches that follow.
                l.barrier(DISPATCH, DISPATCH, VIS_DEVICE)?;
                let mut s = state.borrow_mut();
                s.out_va = va(out_at);
                s.draw_va = va(draw_at);
                Ok(())
            },
            |l, r| {
                let mut s = state.borrow_mut();
                let (ext_va, draw_va) = (s.out_va, s.draw_va);
                s.out_va += ext_stride;
                s.draw_va += 64;
                l.set_root_arguments_va(ext_va)?;
                l.encode_dispatch_indirect(&compute, r.action_va, dispatch_threads)?;
                fetches.push(Fetch { ext_va, draw_va });
                Ok(())
            },
        )?;
        INDIRECT.fetch_add(1, Ordering::Relaxed);
        Ok(fetches)
    }
}

// ---- the kernel that sizes an indirect dispatch -------------------------------

const KERNEL: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
struct P { ulong disp_va; ulong tlab_va; ulong base_va; ulong out_va; ulong draw_va; ulong arena_va; ulong cursor_va; ulong stats_va; ulong static_va;\n\
  uint k; uint disp_words; uint tlab_words; uint app_words; uint ext_words; uint ext_stride_words;\n\
  uint scratch_word; uint dims_word; uint static_word; uint gstride; uint arena_bytes; uint vertices; };\n\
kernel void emu_chunk(constant P& p [[buffer(30)]], uint i [[thread_position_in_threadgroup]]) {\n\
  threadgroup uint grp[256];\n\
  threadgroup uint offs[256];\n\
  bool live = i < p.k;\n\
  device uint* d = ((device uint*)p.disp_va) + (ulong)i * (ulong)p.disp_words;\n\
  uint x = live ? d[0] : 0u; uint y = live ? d[1] : 0u; uint z = live ? d[2] : 0u;\n\
  ulong n = (ulong)x * (ulong)y * (ulong)z;\n\
  if (n > 0x7fffffffUL) { n = 0; }\n\
  grp[i] = (uint)n; offs[i] = 0;\n\
  threadgroup_barrier(mem_flags::mem_threadgroup);\n\
  if (i == 0) {\n\
    device atomic_uint* stats = (device atomic_uint*)p.stats_va;\n\
    device uint* cursor = (device uint*)p.cursor_va;\n\
    ulong cur = *cursor;\n\
    for (uint j = 0; j < p.k; j++) {\n\
      offs[j] = (uint)cur;\n\
      if (grp[j] == 0) { continue; }\n\
      ulong take = (((ulong)grp[j] * (ulong)p.gstride) + 255UL) & ~255UL;\n\
      if (cur + take <= (ulong)p.arena_bytes) { cur += take; atomic_fetch_add_explicit(stats + 1, grp[j], memory_order_relaxed); }\n\
      else { grp[j] = 0; atomic_fetch_add_explicit(stats + 0, 1u, memory_order_relaxed); }\n\
    }\n\
    *cursor = (uint)cur;\n\
  }\n\
  threadgroup_barrier(mem_flags::mem_threadgroup);\n\
  if (!live) { return; }\n\
  uint g = grp[i];\n\
  if (g == 0) { d[0] = 0; d[1] = 0; d[2] = 0; x = 0; y = 0; z = 0; }\n\
  device uint* e = ((device uint*)p.out_va) + (ulong)i * (ulong)p.ext_stride_words;\n\
  device const uint* src = (p.tlab_va != 0) ? (((device const uint*)p.tlab_va) + (ulong)i * (ulong)p.tlab_words) : (device const uint*)p.base_va;\n\
  for (uint w = 0; w < p.app_words; w++) { e[w] = src[w]; }\n\
  for (uint w = p.app_words; w < p.ext_words; w++) { e[w] = 0; }\n\
  ulong sc = p.arena_va + (ulong)offs[i];\n\
  e[p.scratch_word] = (uint)(sc & 0xffffffffUL); e[p.scratch_word + 1] = (uint)(sc >> 32);\n\
  ulong ext_va = p.out_va + (ulong)i * (ulong)p.ext_stride_words * 4UL;\n\
  ulong dims = ext_va + (ulong)p.ext_words * 4UL;\n\
  e[p.dims_word] = (uint)(dims & 0xffffffffUL); e[p.dims_word + 1] = (uint)(dims >> 32);\n\
  if (p.static_word != 0xffffffffu) { e[p.static_word] = (uint)(p.static_va & 0xffffffffUL); e[p.static_word + 1] = (uint)(p.static_va >> 32); }\n\
  e[p.ext_words] = x; e[p.ext_words + 1] = y; e[p.ext_words + 2] = z; e[p.ext_words + 3] = 0;\n\
  device uint* dr = ((device uint*)p.draw_va) + (ulong)i * 16UL;\n\
  for (uint w = 0; w < 16; w++) { dr[w] = 0; }\n\
  dr[0] = p.vertices; dr[1] = g;\n\
}\n";

/// The kernel of [`CommandList::emu_indirect`] and the buffer the GPU counts
/// into (made on first use).
pub struct EmuKernels {
    device: Device,
    chunk: Mutex<Option<ComputePipeline>>,
    stats: Mutex<Option<Buffer>>,
}

impl Device {
    /// An empty holder of the emulation's kernel.
    pub fn emu_kernels(&self) -> EmuKernels {
        EmuKernels {
            device: self.clone(),
            chunk: Mutex::new(None),
            stats: Mutex::new(None),
        }
    }
}

impl EmuKernels {
    fn chunk(&self) -> Result<ComputePipeline> {
        let mut g = self.chunk.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = &*g {
            return Ok(p.clone());
        }
        let p = self.device.compute_pipeline_from_msl(KERNEL, "emu_chunk")?;
        *g = Some(p.clone());
        Ok(p)
    }

    fn stats_buffer(&self) -> Result<Buffer> {
        let mut g = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(b) = &*g {
            return Ok(b.clone());
        }
        let b = self.device.create_buffer(BufferKind::Shared, 64)?;
        b.write(0, &[0u8; 64])?;
        *g = Some(b.clone());
        Ok(b)
    }

    fn stats_va(&self) -> Result<u64> {
        Ok(self.stats_buffer()?.gpu_va())
    }

    fn word(&self, i: u64) -> u64 {
        let Ok(b) = self.stats_buffer() else { return 0 };
        let mut w = [0u8; 4];
        if b.read(4 * i, &mut w).is_err() {
            return 0;
        }
        u64::from(u32::from_le_bytes(w))
    }

    /// Records of indirect dispatches the GPU found no room for in the arena
    /// (their draws were dropped), since the device was made. Read with the
    /// work finished.
    pub fn dropped_records(&self) -> u64 {
        self.word(0)
    }

    /// Groups the GPU gave scratch to in indirect dispatches.
    pub fn indirect_groups(&self) -> u64 {
        self.word(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_defaults_on() {
        assert!(enabled_from(None));
        assert!(enabled_from(Some("1")));
        assert!(enabled_from(Some("")));
        for off in ["0", "false", "OFF", " no "] {
            assert!(!enabled_from(Some(off)), "{off}");
        }
    }

    #[test]
    fn the_default_ring_is_six_segments() {
        let (seg, n) = default_scratch_config();
        assert_eq!(seg, SCRATCH_SEGMENT);
        assert!(n >= 2);
    }

    fn container(parts: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
        let n = parts.len();
        let mut out = b"DXBC".to_vec();
        out.extend_from_slice(&[7u8; 16]);
        out.extend_from_slice(&[1, 0, 0, 0]);
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(n as u32).to_le_bytes());
        let mut at = 32 + 4 * n;
        for (_, d) in parts {
            out.extend_from_slice(&(at as u32).to_le_bytes());
            at += 8 + d.len().next_multiple_of(4);
        }
        for (f, d) in parts {
            out.extend_from_slice(f);
            out.extend_from_slice(&(d.len() as u32).to_le_bytes());
            out.extend_from_slice(d);
            out.resize(out.len().next_multiple_of(4), 0);
        }
        let total = out.len() as u32;
        out[24..28].copy_from_slice(&total.to_le_bytes());
        out
    }

    #[test]
    fn a_bundle_goes_into_a_container_and_comes_out() {
        let base = container(&[(*b"SFI0", vec![1; 8]), (*b"DXIL", vec![2; 13])]);
        assert_eq!(Bundle::find(&base), Ok(None));
        let b = Bundle {
            layout: "vmcl 1\nthreads 1 1 1\n",
            compute: b"CS-container!",
            vertex: b"VS",
        };
        let with = b.attach(&base).unwrap();
        // The other parts are still where the readers look for them.
        assert_eq!(msc::dxil::part(&with, b"DXIL"), Some(&[2u8; 13][..]));
        assert_eq!(msc::dxil::part(&with, b"SFI0"), Some(&[1u8; 8][..]));
        assert_eq!(Bundle::find(&with), Ok(Some(b.clone())));
        // Attaching again replaces the part instead of adding another.
        let again = Bundle {
            vertex: b"VS-two",
            ..b.clone()
        }
        .attach(&with)
        .unwrap();
        assert_eq!(Bundle::find(&again).unwrap().unwrap().vertex, b"VS-two");
        assert_eq!(
            u32::from_le_bytes(again[28..32].try_into().unwrap()),
            3,
            "three parts"
        );
    }

    #[test]
    fn a_damaged_part_is_an_error_not_a_guess() {
        let base = container(&[(*b"DXIL", vec![2; 8])]);
        let b = Bundle {
            layout: "x",
            compute: b"c",
            vertex: b"v",
        };
        let mut with = b.attach(&base).unwrap();
        let p = with[..].windows(4).position(|w| w == VMCS).unwrap();
        with[p + 8] = 2;
        assert!(Bundle::find(&with).unwrap_err().contains("version"));
        with[p + 8] = 1;
        // a range past the part (the layout's length)
        with[p + 8 + 12] = 0xff;
        assert!(Bundle::find(&with).unwrap_err().contains("outside"));
    }

    #[test]
    fn the_extended_root_appends_two_parameters() {
        let layout = Layout::parse(
            "vmcl 1\nthreads 1 1 1\nmaxv 3\nmaxp 1\nvstride 16\npstride 16\nvhead 16\nparea 64\ngstride 80\nspace 4000000000\nregister 0\nv SV_Position 0 3 float 1 4 0 4\n",
        )
        .unwrap();
        let root = RootSignatureDesc {
            flags: 1,
            params: vec![Param {
                kind: ParamKind::Cbv {
                    register: 0,
                    space: 0,
                },
                visibility: Visibility::Mesh,
            }],
            static_samplers: vec![],
        };
        let cs = extend_root(&root, &layout, true, true);
        assert_eq!(cs.params.len(), 3);
        assert_eq!(cs.params[0].visibility, Visibility::All);
        assert!(matches!(cs.params[1].kind, ParamKind::Uav { register: 0, space: 4000000000 }));
        assert!(matches!(cs.params[2].kind, ParamKind::Cbv { register: 0, space: 4000000000 }));
        let g = extend_root(&root, &layout, false, false);
        assert_eq!(g.params[0].visibility, Visibility::Mesh);
        assert!(matches!(g.params[1].kind, ParamKind::Srv { .. }));
        assert_eq!(g.flags, 1);
    }

    #[test]
    fn the_extended_buffer_keeps_the_application_part_and_moves_the_static_table() {
        let ext = ExtLayout {
            size: 48,
            copy: 16,
            scratch_at: 16,
            dims_at: 24,
            static_at: Some(32),
        };
        let shadow: Vec<u8> = (0..24u8).collect();
        let b = fill_ext(&ext, &shadow, Some(0xAA), 0x1111, 0x2222);
        assert_eq!(&b[..16], &shadow[..16]);
        assert_eq!(u64::from_le_bytes(b[16..24].try_into().unwrap()), 0x1111);
        assert_eq!(u64::from_le_bytes(b[24..32].try_into().unwrap()), 0x2222);
        assert_eq!(u64::from_le_bytes(b[32..40].try_into().unwrap()), 0xAA);
        assert_eq!(&b[40..], &[0u8; 8]);
    }
}
