//! Pipeline state descriptions: the application's (two shapes of one thing)
//! to proto's [`GraphicsPsoDesc`]. Pure Rust, no `windows` crate: the shell
//! reads the COM structures into a [`PipelineState`], this module does
//! everything else.
//!
//! Two inputs, one result:
//!
//! * `CreateGraphicsPipelineState`: the shell copies the fields of the
//!   description struct into a [`PipelineState`];
//! * `ID3D12Device2::CreatePipelineState`: [`parse_stream`] reads the
//!   subobject stream. Each subobject is a 4-byte type, then its data;
//!   the type starts on an 8-byte boundary (`alignas(void*)`), the data on
//!   its own alignment (8 for the structures that hold a pointer, 4 for the
//!   rest), and the next subobject on the next 8-byte boundary after the
//!   data. Layouts are the 64-bit ones (the game is x86_64).
//!
//! then [`to_proto`] converts the numbering (every D3D12 enum is mapped to
//! proto's own values here, in one place, with tests) and refuses what the
//! back has no pipeline for, with a text for the log.
//!
//! What a missing subobject means: the defaults of `D3D12_DEFAULT` helpers
//! (blending off with all channels written, solid, back-face culling, depth
//! test less with writes, no stencil, every sample, one sample per pixel).
//! The legacy description has every field explicitly.
//!
//! Not carried: stream output and tessellation stages (refused
//! with `E_NOTIMPL` when present; a geometry shader is carried), view instancing, `RASTERIZER1/2` and
//! `DEPTH_STENCIL2` (their float depth bias / separate stencil masks have
//! no use in the game's pipelines yet), cached PSO blobs and flags (ignored:
//! the back has its own caching, the debug flag changes nothing).

use victory_metal_proto::pso::{
    APPEND_ALIGNED, BlendFactor, BlendOp, CullMode, FillMode, GraphicsPsoDesc, InputClass,
    InputElement, LogicOp, MAX_INPUT_ELEMENTS, SEMANTIC_NAME_BYTES, StageCode, StencilFace,
    StencilOp, StripCut, TopologyType,
};
use victory_metal_proto::types::CompareFunc;

use crate::hresult::Hr;

/// A refused description: the HRESULT to return and why (for the log).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PsoError {
    pub hr: Hr,
    pub why: String,
}

fn invalid(why: impl Into<String>) -> PsoError {
    PsoError {
        hr: Hr::E_INVALIDARG,
        why: why.into(),
    }
}

fn notimpl(why: impl Into<String>) -> PsoError {
    PsoError {
        hr: Hr::E_NOTIMPL,
        why: why.into(),
    }
}

/// `D3D12_SHADER_BYTECODE`: address and length (both 0 = no stage).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bytecode {
    pub addr: u64,
    pub len: u64,
}

/// `D3D12_RENDER_TARGET_BLEND_DESC`, enum values as D3D12 numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RtBlend {
    pub enable: bool,
    pub logic_enable: bool,
    pub src: u32,
    pub dst: u32,
    pub op: u32,
    pub src_alpha: u32,
    pub dst_alpha: u32,
    pub op_alpha: u32,
    pub logic_op: u32,
    pub write_mask: u8,
}

impl RtBlend {
    /// `D3D12_DEFAULT`: blending off, source passes, all channels written.
    pub const DEFAULT: RtBlend = RtBlend {
        enable: false,
        logic_enable: false,
        src: 2,
        dst: 1,
        op: 1,
        src_alpha: 2,
        dst_alpha: 1,
        op_alpha: 1,
        logic_op: 4,
        write_mask: 0xF,
    };
}

/// `D3D12_BLEND_DESC`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Blend {
    pub alpha_to_coverage: bool,
    pub independent: bool,
    pub rt: [RtBlend; 8],
}

/// `D3D12_RASTERIZER_DESC`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Raster {
    pub fill: u32,
    pub cull: u32,
    pub front_ccw: bool,
    pub depth_bias: i32,
    pub depth_bias_clamp: f32,
    pub slope_scaled_bias: f32,
    pub depth_clip: bool,
    pub multisample: bool,
    pub antialiased_line: bool,
    pub forced_sample_count: u32,
    pub conservative: u32,
}

/// `D3D12_DEPTH_STENCILOP_DESC`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StencilOps {
    pub fail: u32,
    pub depth_fail: u32,
    pub pass: u32,
    pub func: u32,
}

/// `D3D12_DEPTH_STENCIL_DESC` (+ the depth bounds flag of `..1`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DepthStencil {
    pub depth_enable: bool,
    /// 0 = zero, 1 = all.
    pub depth_write_mask: u32,
    pub depth_func: u32,
    pub stencil_enable: bool,
    pub stencil_read_mask: u8,
    pub stencil_write_mask: u8,
    pub front: StencilOps,
    pub back: StencilOps,
    pub depth_bounds: bool,
}

/// `D3D12_INPUT_LAYOUT_DESC`: where the elements are (resolved by
/// [`read_input_layout`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputLayoutRef {
    pub elements: u64,
    pub count: u32,
}

/// One resolved `D3D12_INPUT_ELEMENT_DESC`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputDesc {
    pub semantic: String,
    pub semantic_index: u32,
    pub format: u32,
    pub slot: u32,
    pub offset: u32,
    /// 0 per vertex, 1 per instance.
    pub class: u32,
    pub step_rate: u32,
}

/// Everything a pipeline description says, D3D12-numbered, nothing resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct PipelineState {
    /// The `ID3D12RootSignature*` (0 = none given).
    pub root_sig: u64,
    pub vs: Bytecode,
    pub ps: Bytecode,
    pub ds: Bytecode,
    pub hs: Bytecode,
    pub gs: Bytecode,
    pub amp: Bytecode,
    pub ms: Bytecode,
    /// `NumEntries` of the stream output (0 = none).
    pub stream_output_entries: u32,
    pub blend: Blend,
    pub sample_mask: u32,
    pub raster: Raster,
    pub depth: DepthStencil,
    pub input_layout: InputLayoutRef,
    pub strip_cut: u32,
    pub topology: u32,
    pub rt_formats: [u32; 8],
    pub rt_count: u32,
    pub ds_format: u32,
    pub sample_count: u32,
    pub sample_quality: u32,
    pub node_mask: u32,
    pub flags: u32,
    pub view_instance_count: u32,
}

impl Default for PipelineState {
    fn default() -> Self {
        let keep = StencilOps {
            fail: 1,
            depth_fail: 1,
            pass: 1,
            func: 8,
        };
        PipelineState {
            root_sig: 0,
            vs: Bytecode::default(),
            ps: Bytecode::default(),
            ds: Bytecode::default(),
            hs: Bytecode::default(),
            gs: Bytecode::default(),
            amp: Bytecode::default(),
            ms: Bytecode::default(),
            stream_output_entries: 0,
            blend: Blend {
                alpha_to_coverage: false,
                independent: false,
                rt: [RtBlend::DEFAULT; 8],
            },
            sample_mask: u32::MAX,
            raster: Raster {
                fill: 3,
                cull: 3,
                front_ccw: false,
                depth_bias: 0,
                depth_bias_clamp: 0.0,
                slope_scaled_bias: 0.0,
                depth_clip: true,
                multisample: false,
                antialiased_line: false,
                forced_sample_count: 0,
                conservative: 0,
            },
            depth: DepthStencil {
                depth_enable: true,
                depth_write_mask: 1,
                depth_func: 2,
                stencil_enable: false,
                stencil_read_mask: 0xFF,
                stencil_write_mask: 0xFF,
                front: keep,
                back: keep,
                depth_bounds: false,
            },
            input_layout: InputLayoutRef::default(),
            strip_cut: 0,
            topology: 3,
            rt_formats: [0; 8],
            rt_count: 0,
            ds_format: 0,
            sample_count: 1,
            sample_quality: 0,
            node_mask: 0,
            flags: 0,
            view_instance_count: 0,
        }
    }
}

// ---- the stream -----------------------------------------------------------------

/// `D3D12_PIPELINE_STATE_SUBOBJECT_TYPE`.
pub mod subobject {
    pub const ROOT_SIGNATURE: u32 = 0;
    pub const VS: u32 = 1;
    pub const PS: u32 = 2;
    pub const DS: u32 = 3;
    pub const HS: u32 = 4;
    pub const GS: u32 = 5;
    pub const CS: u32 = 6;
    pub const STREAM_OUTPUT: u32 = 7;
    pub const BLEND: u32 = 8;
    pub const SAMPLE_MASK: u32 = 9;
    pub const RASTERIZER: u32 = 10;
    pub const DEPTH_STENCIL: u32 = 11;
    pub const INPUT_LAYOUT: u32 = 12;
    pub const IB_STRIP_CUT_VALUE: u32 = 13;
    pub const PRIMITIVE_TOPOLOGY: u32 = 14;
    pub const RENDER_TARGET_FORMATS: u32 = 15;
    pub const DEPTH_STENCIL_FORMAT: u32 = 16;
    pub const SAMPLE_DESC: u32 = 17;
    pub const NODE_MASK: u32 = 18;
    pub const CACHED_PSO: u32 = 19;
    pub const FLAGS: u32 = 20;
    pub const DEPTH_STENCIL1: u32 = 21;
    pub const VIEW_INSTANCING: u32 = 22;
    pub const AS: u32 = 24;
    pub const MS: u32 = 25;
    pub const DEPTH_STENCIL2: u32 = 26;
    pub const RASTERIZER1: u32 = 27;
    pub const RASTERIZER2: u32 = 28;
}

/// A bounds-checked little-endian cursor over the stream.
struct Cur<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Cur<'_> {
    fn align(&mut self, to: usize) {
        self.pos = self.pos.next_multiple_of(to);
    }

    fn take(&mut self, n: usize) -> Result<&[u8], PsoError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.b.len())
            .ok_or_else(|| invalid("the pipeline state stream ends inside a subobject"))?;
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u32(&mut self) -> Result<u32, PsoError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, PsoError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn f32(&mut self) -> Result<f32, PsoError> {
        Ok(f32::from_bits(self.u32()?))
    }

    fn bool(&mut self) -> Result<bool, PsoError> {
        Ok(self.u32()? != 0)
    }

    fn u8_in_u32_slot(&mut self) -> Result<u8, PsoError> {
        Ok(self.take(1)?[0])
    }

    fn bytecode(&mut self) -> Result<Bytecode, PsoError> {
        Ok(Bytecode {
            addr: self.u64()?,
            len: self.u64()?,
        })
    }
}

fn read_blend(c: &mut Cur) -> Result<Blend, PsoError> {
    let alpha_to_coverage = c.bool()?;
    let independent = c.bool()?;
    let mut rt = [RtBlend::DEFAULT; 8];
    for r in &mut rt {
        let (enable, logic_enable) = (c.bool()?, c.bool()?);
        let (src, dst, op) = (c.u32()?, c.u32()?, c.u32()?);
        let (src_alpha, dst_alpha, op_alpha) = (c.u32()?, c.u32()?, c.u32()?);
        let logic_op = c.u32()?;
        let write_mask = c.u8_in_u32_slot()?;
        c.take(3)?; // the structure's tail padding
        *r = RtBlend {
            enable,
            logic_enable,
            src,
            dst,
            op,
            src_alpha,
            dst_alpha,
            op_alpha,
            logic_op,
            write_mask,
        };
    }
    Ok(Blend {
        alpha_to_coverage,
        independent,
        rt,
    })
}

fn read_raster(c: &mut Cur) -> Result<Raster, PsoError> {
    Ok(Raster {
        fill: c.u32()?,
        cull: c.u32()?,
        front_ccw: c.bool()?,
        depth_bias: c.u32()? as i32,
        depth_bias_clamp: c.f32()?,
        slope_scaled_bias: c.f32()?,
        depth_clip: c.bool()?,
        multisample: c.bool()?,
        antialiased_line: c.bool()?,
        forced_sample_count: c.u32()?,
        conservative: c.u32()?,
    })
}

fn read_stencil_ops(c: &mut Cur) -> Result<StencilOps, PsoError> {
    Ok(StencilOps {
        fail: c.u32()?,
        depth_fail: c.u32()?,
        pass: c.u32()?,
        func: c.u32()?,
    })
}

fn read_depth_stencil(c: &mut Cur, with_bounds: bool) -> Result<DepthStencil, PsoError> {
    let depth_enable = c.bool()?;
    let depth_write_mask = c.u32()?;
    let depth_func = c.u32()?;
    let stencil_enable = c.bool()?;
    let stencil_read_mask = c.u8_in_u32_slot()?;
    let stencil_write_mask = c.u8_in_u32_slot()?;
    c.take(2)?;
    let front = read_stencil_ops(c)?;
    let back = read_stencil_ops(c)?;
    let depth_bounds = if with_bounds { c.bool()? } else { false };
    Ok(DepthStencil {
        depth_enable,
        depth_write_mask,
        depth_func,
        stencil_enable,
        stencil_read_mask,
        stencil_write_mask,
        front,
        back,
        depth_bounds,
    })
}

/// Parses a `D3D12_PIPELINE_STATE_STREAM_DESC` stream (see the module docs
/// for the layout). A subobject type this does not know cannot be skipped
/// (its size is unknown): `E_INVALIDARG`, as is one given twice, or a stream
/// that ends inside a subobject. A compute shader subobject is refused here:
/// compute pipelines have their own entry point.
pub fn parse_stream(bytes: &[u8]) -> Result<PipelineState, PsoError> {
    use subobject as t;
    let mut s = PipelineState::default();
    let mut c = Cur { b: bytes, pos: 0 };
    let mut seen: u64 = 0;
    loop {
        c.align(8);
        if c.pos == bytes.len() {
            return Ok(s);
        }
        let ty = c.u32()?;
        if ty >= 64 || seen >> ty & 1 == 1 {
            return Err(invalid(format!(
                "pipeline state stream: subobject type {ty} is unknown or repeated"
            )));
        }
        seen |= 1 << ty;
        // The data's alignment: 8 where the structure holds a pointer.
        let align = match ty {
            t::ROOT_SIGNATURE
            | t::VS
            | t::PS
            | t::DS
            | t::HS
            | t::GS
            | t::CS
            | t::AS
            | t::MS
            | t::STREAM_OUTPUT
            | t::INPUT_LAYOUT
            | t::CACHED_PSO
            | t::VIEW_INSTANCING => 8,
            _ => 4,
        };
        c.align(align);
        match ty {
            t::ROOT_SIGNATURE => s.root_sig = c.u64()?,
            t::VS => s.vs = c.bytecode()?,
            t::PS => s.ps = c.bytecode()?,
            t::DS => s.ds = c.bytecode()?,
            t::HS => s.hs = c.bytecode()?,
            t::GS => s.gs = c.bytecode()?,
            t::AS => s.amp = c.bytecode()?,
            t::MS => s.ms = c.bytecode()?,
            t::CS => {
                return Err(invalid(
                    "a compute shader in a graphics pipeline state stream",
                ));
            }
            t::STREAM_OUTPUT => {
                // pSODeclaration, NumEntries, pBufferStrides, NumStrides, RasterizedStream
                c.u64()?;
                s.stream_output_entries = c.u32()?;
                c.take(4)?;
                c.u64()?;
                c.u32()?;
                c.u32()?;
            }
            t::BLEND => s.blend = read_blend(&mut c)?,
            t::SAMPLE_MASK => s.sample_mask = c.u32()?,
            t::RASTERIZER => s.raster = read_raster(&mut c)?,
            t::DEPTH_STENCIL => s.depth = read_depth_stencil(&mut c, false)?,
            t::DEPTH_STENCIL1 => s.depth = read_depth_stencil(&mut c, true)?,
            t::INPUT_LAYOUT => {
                s.input_layout = InputLayoutRef {
                    elements: c.u64()?,
                    count: c.u32()?,
                };
                c.take(4)?;
            }
            t::IB_STRIP_CUT_VALUE => s.strip_cut = c.u32()?,
            t::PRIMITIVE_TOPOLOGY => s.topology = c.u32()?,
            t::RENDER_TARGET_FORMATS => {
                for f in &mut s.rt_formats {
                    *f = c.u32()?;
                }
                s.rt_count = c.u32()?;
            }
            t::DEPTH_STENCIL_FORMAT => s.ds_format = c.u32()?,
            t::SAMPLE_DESC => {
                s.sample_count = c.u32()?;
                s.sample_quality = c.u32()?;
            }
            t::NODE_MASK => s.node_mask = c.u32()?,
            t::CACHED_PSO => {
                c.u64()?;
                c.u64()?;
            }
            t::FLAGS => s.flags = c.u32()?,
            t::VIEW_INSTANCING => {
                s.view_instance_count = c.u32()?;
                c.take(4)?;
                c.u64()?;
                c.u32()?;
                c.take(4)?;
            }
            t::DEPTH_STENCIL2 | t::RASTERIZER1 | t::RASTERIZER2 => {
                return Err(notimpl(format!(
                    "pipeline state stream: subobject type {ty} (RASTERIZER1/2, DEPTH_STENCIL2) is not carried"
                )));
            }
            _ => {
                return Err(invalid(format!(
                    "pipeline state stream: subobject type {ty} is unknown"
                )));
            }
        }
    }
}

// ---- the input layout -------------------------------------------------------------

/// `sizeof(D3D12_INPUT_ELEMENT_DESC)` on 64-bit.
const ELEMENT_SIZE: usize = 32;

/// Reads the input elements and their semantic names.
///
/// # Safety
///
/// `layout.elements` points at `layout.count` readable
/// `D3D12_INPUT_ELEMENT_DESC`s whose `SemanticName`s are NUL-terminated
/// strings: what the application promises for the call.
pub unsafe fn read_input_layout(layout: InputLayoutRef) -> Result<Vec<InputDesc>, PsoError> {
    if layout.count == 0 {
        return Ok(Vec::new());
    }
    if layout.elements == 0 || layout.count as usize > MAX_INPUT_ELEMENTS {
        return Err(invalid(format!(
            "input layout of {} elements at {:#x} (at most {MAX_INPUT_ELEMENTS})",
            layout.count, layout.elements
        )));
    }
    // SAFETY: the caller's promise.
    let raw = unsafe {
        std::slice::from_raw_parts(
            layout.elements as *const u8,
            layout.count as usize * ELEMENT_SIZE,
        )
    };
    let mut out = Vec::new();
    for i in 0..layout.count as usize {
        let e = &raw[i * ELEMENT_SIZE..(i + 1) * ELEMENT_SIZE];
        let u32_at = |o: usize| u32::from_le_bytes(e[o..o + 4].try_into().unwrap());
        let name_ptr = u64::from_le_bytes(e[0..8].try_into().unwrap());
        if name_ptr == 0 {
            return Err(invalid("an input element without a semantic name"));
        }
        // SAFETY: a NUL-terminated string, as promised; read at most one more
        // byte than a name can have so a missing terminator cannot run on.
        let name = unsafe {
            let p = name_ptr as *const u8;
            let mut n = 0;
            while n < SEMANTIC_NAME_BYTES && *p.add(n) != 0 {
                n += 1;
            }
            if n == SEMANTIC_NAME_BYTES {
                return Err(invalid("a semantic name of 32 bytes or more"));
            }
            std::slice::from_raw_parts(p, n)
        };
        let name = std::str::from_utf8(name)
            .ok()
            .filter(|n| n.is_ascii())
            .ok_or_else(|| invalid("a semantic name that is not ASCII"))?;
        out.push(InputDesc {
            semantic: name.to_owned(),
            semantic_index: u32_at(8),
            format: u32_at(12),
            slot: u32_at(16),
            offset: u32_at(20),
            class: u32_at(24),
            step_rate: u32_at(28),
        });
    }
    Ok(out)
}

// ---- to proto -----------------------------------------------------------------------

/// `D3D12_BLEND` (1..=21, 12 and 13 unused).
pub fn blend_factor(f: u32) -> Result<BlendFactor, PsoError> {
    Ok(match f {
        1 => BlendFactor::ZERO,
        2 => BlendFactor::ONE,
        3 => BlendFactor::SRC_COLOR,
        4 => BlendFactor::INV_SRC_COLOR,
        5 => BlendFactor::SRC_ALPHA,
        6 => BlendFactor::INV_SRC_ALPHA,
        7 => BlendFactor::DST_ALPHA,
        8 => BlendFactor::INV_DST_ALPHA,
        9 => BlendFactor::DST_COLOR,
        10 => BlendFactor::INV_DST_COLOR,
        11 => BlendFactor::SRC_ALPHA_SAT,
        14 => BlendFactor::CONSTANT_COLOR,
        15 => BlendFactor::INV_CONSTANT_COLOR,
        16 => BlendFactor::SRC1_COLOR,
        17 => BlendFactor::INV_SRC1_COLOR,
        18 => BlendFactor::SRC1_ALPHA,
        19 => BlendFactor::INV_SRC1_ALPHA,
        20 => BlendFactor::CONSTANT_ALPHA,
        21 => BlendFactor::INV_CONSTANT_ALPHA,
        _ => return Err(invalid(format!("blend factor {f}"))),
    })
}

/// `D3D12_BLEND_OP` (1-based).
pub fn blend_op(o: u32) -> Result<BlendOp, PsoError> {
    match o {
        1..=5 => Ok(BlendOp(o as u8 - 1)),
        _ => Err(invalid(format!("blend op {o}"))),
    }
}

/// `D3D12_LOGIC_OP` (same order as proto's).
pub fn logic_op(o: u32) -> Result<LogicOp, PsoError> {
    match o {
        0..=15 => Ok(LogicOp(o as u8)),
        _ => Err(invalid(format!("logic op {o}"))),
    }
}

/// `D3D12_STENCIL_OP` (1-based; `INCR`/`DECR` wrap).
pub fn stencil_op(o: u32) -> Result<StencilOp, PsoError> {
    Ok(match o {
        1 => StencilOp::KEEP,
        2 => StencilOp::ZERO,
        3 => StencilOp::REPLACE,
        4 => StencilOp::INCR_SAT,
        5 => StencilOp::DECR_SAT,
        6 => StencilOp::INVERT,
        7 => StencilOp::INCR_WRAP,
        8 => StencilOp::DECR_WRAP,
        _ => return Err(invalid(format!("stencil op {o}"))),
    })
}

/// `D3D12_COMPARISON_FUNC` (1-based).
fn compare(f: u32) -> Result<CompareFunc, PsoError> {
    crate::convert::compare_func(f).map_err(|_| invalid(format!("comparison function {f}")))
}

/// `D3D12_FILL_MODE`: 2 wireframe, 3 solid.
pub fn fill_mode(f: u32) -> Result<FillMode, PsoError> {
    match f {
        2 => Ok(FillMode::WIREFRAME),
        3 => Ok(FillMode::SOLID),
        _ => Err(invalid(format!("fill mode {f}"))),
    }
}

/// `D3D12_CULL_MODE` (1-based).
pub fn cull_mode(c: u32) -> Result<CullMode, PsoError> {
    match c {
        1 => Ok(CullMode::NONE),
        2 => Ok(CullMode::FRONT),
        3 => Ok(CullMode::BACK),
        _ => Err(invalid(format!("cull mode {c}"))),
    }
}

/// `D3D12_PRIMITIVE_TOPOLOGY_TYPE`; patches (tessellation) are not carried.
pub fn topology_type(t: u32) -> Result<TopologyType, PsoError> {
    match t {
        0 => Ok(TopologyType::UNDEFINED),
        1 => Ok(TopologyType::POINT),
        2 => Ok(TopologyType::LINE),
        3 => Ok(TopologyType::TRIANGLE),
        4 => Err(notimpl("patch primitive topology (tessellation)")),
        _ => Err(invalid(format!("primitive topology type {t}"))),
    }
}

/// `D3D12_INDEX_BUFFER_STRIP_CUT_VALUE`.
pub fn strip_cut(v: u32) -> Result<StripCut, PsoError> {
    match v {
        0..=2 => Ok(StripCut(v as u8)),
        _ => Err(invalid(format!("strip cut value {v}"))),
    }
}

fn stage(code: Bytecode, name: &str) -> Result<StageCode, PsoError> {
    match (code.addr, code.len) {
        (0, 0) => Ok(StageCode::NONE),
        (0, _) | (_, 0) => Err(invalid(format!(
            "the {name} shader has an address or a length but not both"
        ))),
        (addr, len) => Ok(StageCode { addr, len }),
    }
}

fn stencil_face(f: &StencilOps) -> Result<StencilFace, PsoError> {
    Ok(StencilFace {
        fail: stencil_op(f.fail)?,
        depth_fail: stencil_op(f.depth_fail)?,
        pass: stencil_op(f.pass)?,
        func: compare(f.func)?,
    })
}

/// The proto description for `state` with its resolved input layout.
///
/// Refused as `E_NOTIMPL` (the back has no such pipeline): hull or domain
/// shaders, stream output, patch topologies, view instancing. A
/// depth-bounds test is dropped (the back says so when it builds the
/// pipeline; the flag travels in the description).
pub fn to_proto(state: &PipelineState, inputs: &[InputDesc]) -> Result<GraphicsPsoDesc, PsoError> {
    for (code, name) in [(&state.hs, "hull"), (&state.ds, "domain")] {
        if code.addr != 0 || code.len != 0 {
            return Err(notimpl(format!("a {name} shader")));
        }
    }
    if state.stream_output_entries != 0 {
        return Err(notimpl("stream output"));
    }
    if state.view_instance_count != 0 {
        return Err(notimpl("view instancing"));
    }
    if state.node_mask > 1 {
        return Err(invalid(format!("node mask {:#x}", state.node_mask)));
    }
    if state.rt_count > 8 {
        return Err(invalid(format!("{} render targets", state.rt_count)));
    }
    if state.sample_count == 0 || state.sample_count > 16 {
        return Err(invalid(format!("sample count {}", state.sample_count)));
    }
    if inputs.len() > MAX_INPUT_ELEMENTS {
        return Err(invalid(format!("{} input elements", inputs.len())));
    }

    let mut d = GraphicsPsoDesc::new();
    d.vs = stage(state.vs, "vertex")?;
    d.ps = stage(state.ps, "pixel")?;
    d.amp = stage(state.amp, "amplification")?;
    d.ms = stage(state.ms, "mesh")?;
    d.gs = stage(state.gs, "geometry")?;
    if d.kind().is_none() {
        return Err(invalid(
            "a graphics pipeline has a vertex shader (with an optional geometry shader), or a mesh shader (with an optional amplification shader)",
        ));
    }

    d.input_count = inputs.len() as u32;
    for (slot, e) in d.inputs.iter_mut().zip(inputs) {
        if e.semantic.len() >= SEMANTIC_NAME_BYTES {
            return Err(invalid("a semantic name of 32 bytes or more"));
        }
        let append = e.offset == 0xFFFF_FFFF;
        *slot = InputElement::new(
            &e.semantic,
            e.semantic_index,
            e.format,
            e.slot,
            if append { APPEND_ALIGNED } else { e.offset },
        );
        slot.step_rate = e.step_rate;
        slot.class = match e.class {
            0 => InputClass::PER_VERTEX,
            1 => InputClass::PER_INSTANCE,
            c => return Err(invalid(format!("input slot class {c}"))),
        };
        if e.class == 0 {
            slot.step_rate = 0;
        }
    }

    let b = &state.blend;
    d.blend.alpha_to_coverage = u8::from(b.alpha_to_coverage);
    d.blend.independent = u8::from(b.independent);
    for (i, (out, r)) in d.blend.rt.iter_mut().zip(&b.rt).enumerate() {
        // D3D12 reads target 0 only unless blends are independent, and the
        // blend (logic) fields only when that blend (logic) is enabled: what is
        // not read is whatever the application left there (often zeros, which
        // are not valid enum values). Canonicalise it to `D3D12_DEFAULT`.
        let used = b.independent || i == 0;
        let r = if used { r } else { &RtBlend::DEFAULT };
        let d0 = &RtBlend::DEFAULT;
        out.enable = u8::from(r.enable);
        out.logic_enable = u8::from(r.logic_enable);
        out.write_mask = r.write_mask & 0xF;
        out.logic_op = logic_op(if r.logic_enable {
            r.logic_op
        } else {
            d0.logic_op
        })?;
        let q = if r.enable { r } else { d0 };
        out.src = blend_factor(q.src)?;
        out.dst = blend_factor(q.dst)?;
        out.op = blend_op(q.op)?;
        out.src_alpha = blend_factor(q.src_alpha)?;
        out.dst_alpha = blend_factor(q.dst_alpha)?;
        out.op_alpha = blend_op(q.op_alpha)?;
    }

    let r = &state.raster;
    d.raster.fill = fill_mode(r.fill)?;
    d.raster.cull = cull_mode(r.cull)?;
    d.raster.front_ccw = u8::from(r.front_ccw);
    d.raster.depth_clip = u8::from(r.depth_clip);
    d.raster.conservative = match r.conservative {
        0 => 0,
        1 => 1,
        c => return Err(invalid(format!("conservative rasterization mode {c}"))),
    };
    d.raster.multisample = u8::from(r.multisample);
    d.raster.depth_bias = r.depth_bias;
    d.raster.depth_bias_clamp = r.depth_bias_clamp;
    d.raster.slope_scaled_bias = r.slope_scaled_bias;

    let s = &state.depth;
    d.depth.depth_enable = u8::from(s.depth_enable);
    // Depth state is read only with the depth test on, stencil state only with
    // the stencil test on: otherwise canonical defaults (see the blend note).
    let dd = PipelineState::default().depth;
    let depth_on = if s.depth_enable { s } else { &dd };
    d.depth.depth_write = match depth_on.depth_write_mask {
        0 => 0,
        1 => 1,
        m => return Err(invalid(format!("depth write mask {m}"))),
    };
    d.depth.depth_func = compare(depth_on.depth_func)?;
    d.depth.stencil_enable = u8::from(s.stencil_enable);
    d.depth.stencil_read_mask = s.stencil_read_mask;
    d.depth.stencil_write_mask = s.stencil_write_mask;
    d.depth.depth_bounds = u8::from(s.depth_bounds);
    let stencil_on = if s.stencil_enable { s } else { &dd };
    d.depth.front = stencil_face(&stencil_on.front)?;
    d.depth.back = stencil_face(&stencil_on.back)?;

    d.rt_count = state.rt_count;
    d.rt_formats = state.rt_formats;
    for f in &mut d.rt_formats[state.rt_count as usize..] {
        *f = 0;
    }
    d.ds_format = state.ds_format;
    d.sample_count = state.sample_count;
    d.sample_mask = state.sample_mask;
    d.topology = topology_type(state.topology)?;
    d.strip_cut = strip_cut(state.strip_cut)?;
    d.check().map_err(|e| invalid(format!("{e}")))?;
    Ok(d)
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use victory_metal_proto::pso::PsoKind;

    #[test]
    fn enum_tables_cover_exactly_the_d3d12_values() {
        for f in 1..=21u32 {
            assert_eq!(blend_factor(f).is_ok(), f != 12 && f != 13, "factor {f}");
        }
        assert!(blend_factor(0).is_err() && blend_factor(22).is_err());
        // The names the numbers stand for (d3d12.h).
        assert_eq!(blend_factor(14).unwrap(), BlendFactor::CONSTANT_COLOR);
        assert_eq!(blend_factor(16).unwrap(), BlendFactor::SRC1_COLOR);
        assert_eq!(blend_factor(20).unwrap(), BlendFactor::CONSTANT_ALPHA);
        assert_eq!(blend_factor(11).unwrap(), BlendFactor::SRC_ALPHA_SAT);
        assert_eq!(blend_op(1).unwrap(), BlendOp::ADD);
        assert_eq!(blend_op(5).unwrap(), BlendOp::MAX);
        assert!(blend_op(0).is_err() && blend_op(6).is_err());
        assert_eq!(stencil_op(1).unwrap(), StencilOp::KEEP);
        assert_eq!(stencil_op(4).unwrap(), StencilOp::INCR_SAT);
        assert_eq!(stencil_op(7).unwrap(), StencilOp::INCR_WRAP);
        assert_eq!(stencil_op(8).unwrap(), StencilOp::DECR_WRAP);
        assert!(stencil_op(0).is_err() && stencil_op(9).is_err());
        assert_eq!(fill_mode(2).unwrap(), FillMode::WIREFRAME);
        assert_eq!(fill_mode(3).unwrap(), FillMode::SOLID);
        assert!(fill_mode(1).is_err());
        assert_eq!(cull_mode(1).unwrap(), CullMode::NONE);
        assert_eq!(cull_mode(3).unwrap(), CullMode::BACK);
        assert!(cull_mode(0).is_err() && cull_mode(4).is_err());
        assert_eq!(topology_type(3).unwrap(), TopologyType::TRIANGLE);
        assert_eq!(topology_type(4).unwrap_err().hr, Hr::E_NOTIMPL);
        assert!(topology_type(5).is_err());
        assert_eq!(strip_cut(2).unwrap(), StripCut::INDEX_0XFFFFFFFF);
        assert!(strip_cut(3).is_err());
        assert_eq!(logic_op(4).unwrap(), LogicOp::NOOP);
        assert_eq!(logic_op(10).unwrap(), LogicOp::XOR);
        assert!(logic_op(16).is_err());
    }

    #[test]
    fn the_defaults_convert_to_proto_defaults() {
        let mut s = PipelineState::default();
        s.vs = Bytecode { addr: 8, len: 8 };
        let d = to_proto(&s, &[]).unwrap();
        let mut want = GraphicsPsoDesc::new();
        want.vs = StageCode { addr: 8, len: 8 };
        assert_eq!(d, want);
    }

    #[test]
    fn state_that_is_not_enabled_is_not_read() {
        // An application that skips the fields of a disabled blend or stencil
        // state leaves zeros there; D3D12 ignores them.
        let mut s = PipelineState::default();
        s.vs = Bytecode { addr: 8, len: 8 };
        let zero = RtBlend {
            enable: false,
            logic_enable: false,
            src: 0,
            dst: 0,
            op: 0,
            src_alpha: 0,
            dst_alpha: 0,
            op_alpha: 0,
            logic_op: 0,
            write_mask: 0xF,
        };
        s.blend.rt = [zero; 8];
        s.depth.stencil_enable = false;
        let z = StencilOps {
            fail: 0,
            depth_fail: 0,
            pass: 0,
            func: 0,
        };
        s.depth.front = z;
        s.depth.back = z;
        let d = to_proto(&s, &[]).unwrap();
        let mut want = GraphicsPsoDesc::new();
        want.vs = StageCode { addr: 8, len: 8 };
        assert_eq!(d, want);
        // Depth state with the depth test off.
        s.depth.depth_enable = false;
        s.depth.depth_func = 0;
        s.depth.depth_write_mask = 0;
        assert!(to_proto(&s, &[]).is_ok());
        // Targets 1.. are not read unless blends are independent.
        let mut t = PipelineState::default();
        t.vs = Bytecode { addr: 8, len: 8 };
        t.blend.rt[3].enable = true;
        t.blend.rt[3].src = 0;
        assert!(to_proto(&t, &[]).is_ok());
        // Enabled state is still checked.
        t.blend.independent = true;
        assert!(to_proto(&t, &[]).is_err());
        let mut u = PipelineState::default();
        u.vs = Bytecode { addr: 8, len: 8 };
        u.depth.stencil_enable = true;
        u.depth.front = z;
        assert!(to_proto(&u, &[]).is_err());
        let mut v = PipelineState::default();
        v.vs = Bytecode { addr: 8, len: 8 };
        v.blend.rt[0].logic_enable = true;
        v.blend.rt[0].logic_op = 16;
        assert!(to_proto(&v, &[]).is_err());
    }

    #[test]
    fn refusals_name_what_is_missing() {
        let mut s = PipelineState::default();
        s.vs = Bytecode { addr: 8, len: 8 };
        let mut t = s.clone();
        t.gs = Bytecode { addr: 8, len: 8 };
        let d = to_proto(&t, &[]).unwrap();
        assert_eq!(d.kind(), Some(PsoKind::Geometry));
        assert_eq!(d.gs.addr, 8);
        let mut t = s.clone();
        t.hs = Bytecode { addr: 8, len: 8 };
        let e = to_proto(&t, &[]).unwrap_err();
        assert_eq!(e.hr, Hr::E_NOTIMPL);
        assert!(e.why.contains("hull"));
        let mut t = s.clone();
        t.stream_output_entries = 1;
        assert_eq!(to_proto(&t, &[]).unwrap_err().hr, Hr::E_NOTIMPL);
        let mut t = s.clone();
        t.view_instance_count = 2;
        assert_eq!(to_proto(&t, &[]).unwrap_err().hr, Hr::E_NOTIMPL);
        let mut t = s.clone();
        t.vs = Bytecode::default();
        assert_eq!(to_proto(&t, &[]).unwrap_err().hr, Hr::E_INVALIDARG);
        let mut t = s.clone();
        t.vs = Bytecode { addr: 8, len: 0 };
        assert_eq!(to_proto(&t, &[]).unwrap_err().hr, Hr::E_INVALIDARG);
        let mut t = s.clone();
        t.sample_count = 0;
        assert_eq!(to_proto(&t, &[]).unwrap_err().hr, Hr::E_INVALIDARG);
        let mut t = s.clone();
        t.rt_count = 9;
        assert_eq!(to_proto(&t, &[]).unwrap_err().hr, Hr::E_INVALIDARG);
        let mut t = s;
        t.node_mask = 2;
        assert_eq!(to_proto(&t, &[]).unwrap_err().hr, Hr::E_INVALIDARG);
    }

    #[test]
    fn input_elements_convert_with_append_and_instance_step() {
        let mut s = PipelineState::default();
        s.vs = Bytecode { addr: 8, len: 8 };
        let inputs = [
            InputDesc {
                semantic: "POSITION".into(),
                semantic_index: 0,
                format: 6,
                slot: 0,
                offset: 0xFFFF_FFFF,
                class: 0,
                step_rate: 0,
            },
            InputDesc {
                semantic: "OFFSET".into(),
                semantic_index: 1,
                format: 16,
                slot: 1,
                offset: 8,
                class: 1,
                step_rate: 1,
            },
        ];
        let d = to_proto(&s, &inputs).unwrap();
        assert_eq!(d.input_count, 2);
        let e = d.input_elements();
        assert_eq!(
            (e[0].name(), e[0].offset, e[0].class),
            ("POSITION", APPEND_ALIGNED, InputClass::PER_VERTEX)
        );
        assert_eq!(
            (e[1].name(), e[1].semantic_index, e[1].slot, e[1].offset),
            ("OFFSET", 1, 1, 8)
        );
        assert_eq!((e[1].class, e[1].step_rate), (InputClass::PER_INSTANCE, 1));
        let mut bad = inputs.clone();
        bad[0].class = 2;
        assert!(to_proto(&s, &bad).is_err());
    }
}
