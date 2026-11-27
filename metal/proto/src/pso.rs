//! Graphics pipeline description: our own POD form, not D3D12's and not
//! Metal's (added in protocol version 2, `CREATE_GRAPHICS_PIPELINE`).
//!
//! One fixed-size record, [`GraphicsPsoDesc`], embedded in the argument block
//! of the sync call. It carries, per stage, the address and length of the
//! DXIL container (valid for the call only, like the compute call's), the
//! vertex input layout, the primitive topology *type*, the render target
//! and depth formats, the sample count, blending (per render target),
//! rasterizer and depth-stencil state.
//!
//! Which pipeline it is follows from which stages have code:
//!
//! * `vs` set, `ms` and `amp` empty: a vertex pipeline (`vs` + optional `ps`);
//! * `ms` set, `vs` empty: a mesh pipeline (optional `amp` before it, optional `ps`);
//! * anything else (nothing, `vs` and `ms` together, `amp` without `ms`): invalid.
//!
//! Only the *pipeline* state is here. State that D3D12 sets on the command
//! list (blend factor, stencil reference, viewports, scissors, the topology
//! in use, vertex strides) is for the render commands of the next milestone
//! (`docs/metal/proto-render-draft.md`).
//!
//! Enum values are ours (zero-based, zero = the neutral choice); the front
//! converts. Every enum is open (`is_known()`); [`GraphicsPsoDesc::check`]
//! refuses unknown values, counts over the limits and unterminated names.

use crate::status::Error;
use crate::wire::{Wire, array_wire, open_enum, wire_struct};

/// Most input elements a pipeline has (D3D12's own limit is 32).
pub const MAX_INPUT_ELEMENTS: usize = 32;
/// Bytes of an input element's semantic name, NUL included.
pub const SEMANTIC_NAME_BYTES: usize = 32;
/// Most render targets.
pub const MAX_RENDER_TARGETS: usize = 8;
/// `InputElement::offset` meaning "right after the previous element of the
/// same slot, aligned to its format's component size".
pub const APPEND_ALIGNED: u32 = u32::MAX;

const OPCODE_NONE: u16 = 0;

open_enum! {
    /// The kind of primitive a pipeline draws (not the list/strip choice,
    /// which is set when drawing). `UNDEFINED` is what a mesh pipeline may carry.
    pub struct TopologyType(u8) {
        UNDEFINED = 0,
        POINT = 1,
        LINE = 2,
        TRIANGLE = 3,
    }
}

open_enum! {
    /// The index value that restarts a strip, when restarting is on.
    pub struct StripCut(u8) {
        DISABLED = 0,
        INDEX_0XFFFF = 1,
        INDEX_0XFFFFFFFF = 2,
    }
}

open_enum! {
    pub struct InputClass(u8) {
        PER_VERTEX = 0,
        PER_INSTANCE = 1,
    }
}

open_enum! {
    pub struct BlendFactor(u8) {
        ZERO = 0,
        ONE = 1,
        SRC_COLOR = 2,
        INV_SRC_COLOR = 3,
        SRC_ALPHA = 4,
        INV_SRC_ALPHA = 5,
        DST_ALPHA = 6,
        INV_DST_ALPHA = 7,
        DST_COLOR = 8,
        INV_DST_COLOR = 9,
        SRC_ALPHA_SAT = 10,
        /// The constant set by the blend-factor command.
        CONSTANT_COLOR = 11,
        INV_CONSTANT_COLOR = 12,
        /// Second fragment output (dual-source blending).
        SRC1_COLOR = 13,
        INV_SRC1_COLOR = 14,
        SRC1_ALPHA = 15,
        INV_SRC1_ALPHA = 16,
        /// Alpha of the constant only.
        CONSTANT_ALPHA = 17,
        INV_CONSTANT_ALPHA = 18,
    }
}

open_enum! {
    pub struct BlendOp(u8) {
        ADD = 0,
        SUBTRACT = 1,
        REV_SUBTRACT = 2,
        MIN = 3,
        MAX = 4,
    }
}

open_enum! {
    /// Logical operations of a render target (only `NOOP`, or blending off, is
    /// accepted by the Metal back: it has no logic ops).
    pub struct LogicOp(u8) {
        CLEAR = 0,
        SET = 1,
        COPY = 2,
        COPY_INVERTED = 3,
        NOOP = 4,
        INVERT = 5,
        AND = 6,
        NAND = 7,
        OR = 8,
        NOR = 9,
        XOR = 10,
        EQUIV = 11,
        AND_REVERSE = 12,
        AND_INVERTED = 13,
        OR_REVERSE = 14,
        OR_INVERTED = 15,
    }
}

open_enum! {
    pub struct FillMode(u8) {
        SOLID = 0,
        WIREFRAME = 1,
    }
}

open_enum! {
    pub struct CullMode(u8) {
        NONE = 0,
        FRONT = 1,
        BACK = 2,
    }
}

open_enum! {
    pub struct StencilOp(u8) {
        KEEP = 0,
        ZERO = 1,
        REPLACE = 2,
        /// Clamps at the maximum.
        INCR_SAT = 3,
        /// Clamps at zero.
        DECR_SAT = 4,
        INVERT = 5,
        /// Wraps around.
        INCR_WRAP = 6,
        DECR_WRAP = 7,
    }
}

wire_struct! {
    /// Address and length of one stage's DXIL container, valid for the call
    /// only. `addr == 0 && len == 0` = the stage is absent.
    pub struct StageCode {
        addr: u64,
        len: u64,
    }
}

impl StageCode {
    pub const NONE: StageCode = StageCode { addr: 0, len: 0 };

    pub const fn is_present(&self) -> bool {
        self.addr != 0 || self.len != 0
    }

    /// A stage over a byte slice that outlives the call.
    pub fn of(code: &[u8]) -> StageCode {
        StageCode {
            addr: code.as_ptr() as u64,
            len: code.len() as u64,
        }
    }
}

wire_struct! {
    /// One vertex input element (what the shader reads as `semantic` +
    /// `semantic_index`, from vertex buffer `slot`).
    pub struct InputElement {
        /// ASCII name, NUL padded; at least one NUL.
        semantic: [u8; 32],
        semantic_index: u32,
        /// Format id (DXGI number).
        format: u32,
        /// Vertex buffer slot.
        slot: u32,
        /// Byte offset in the vertex, or [`APPEND_ALIGNED`].
        offset: u32,
        /// Per-instance elements: instances per step (0 and 1 = every instance).
        step_rate: u32,
        class: InputClass,
        _pad: [u8; 3],
    }
}

impl InputElement {
    /// An element named `name` (at most 31 bytes; longer names are cut).
    pub fn new(name: &str, semantic_index: u32, format: u32, slot: u32, offset: u32) -> Self {
        let mut e = <Self as Wire>::ZERO;
        let n = name.len().min(SEMANTIC_NAME_BYTES - 1);
        e.semantic[..n].copy_from_slice(&name.as_bytes()[..n]);
        e.semantic_index = semantic_index;
        e.format = format;
        e.slot = slot;
        e.offset = offset;
        e
    }

    /// The semantic name (up to the first NUL).
    pub fn name(&self) -> &str {
        let end = self
            .semantic
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.semantic.len());
        core::str::from_utf8(&self.semantic[..end]).unwrap_or("")
    }
}

wire_struct! {
    /// Blending of one render target.
    pub struct RtBlend {
        /// Nonzero = blending on.
        enable: u8,
        /// Nonzero = logic op on (refused by the Metal back unless `logic_op` is `NOOP`).
        logic_enable: u8,
        /// Bit 0 red, 1 green, 2 blue, 3 alpha.
        write_mask: u8,
        logic_op: LogicOp,
        src: BlendFactor,
        dst: BlendFactor,
        op: BlendOp,
        src_alpha: BlendFactor,
        dst_alpha: BlendFactor,
        op_alpha: BlendOp,
        _pad: [u8; 2],
    }
}

impl RtBlend {
    /// Blending off, all channels written: the default.
    pub const fn opaque() -> Self {
        let mut b = <Self as Wire>::ZERO;
        b.write_mask = 0xF;
        b.logic_op = LogicOp::NOOP;
        b.src = BlendFactor::ONE;
        b.src_alpha = BlendFactor::ONE;
        b
    }
}

wire_struct! {
    pub struct BlendDesc {
        /// Nonzero = alpha to coverage.
        alpha_to_coverage: u8,
        /// Zero: `rt[0]` applies to every target.
        independent: u8,
        _pad: [u8; 2],
        rt: [RtBlend; 8],
    }
}

wire_struct! {
    pub struct RasterDesc {
        fill: FillMode,
        cull: CullMode,
        /// Nonzero: counter-clockwise triangles are front-facing.
        front_ccw: u8,
        /// Nonzero: depth clipping on (off: depth is clamped).
        depth_clip: u8,
        /// Nonzero: conservative rasterization (the Metal back refuses it).
        conservative: u8,
        /// Nonzero: multisample rasterization (`MultisampleEnable`); informational.
        multisample: u8,
        _pad: [u8; 2],
        depth_bias: i32,
        depth_bias_clamp: f32,
        slope_scaled_bias: f32,
    }
}

wire_struct! {
    pub struct StencilFace {
        fail: StencilOp,
        depth_fail: StencilOp,
        pass: StencilOp,
        func: crate::types::CompareFunc,
    }
}

wire_struct! {
    pub struct DepthStencilDesc {
        depth_enable: u8,
        depth_write: u8,
        depth_func: crate::types::CompareFunc,
        stencil_enable: u8,
        stencil_read_mask: u8,
        stencil_write_mask: u8,
        /// Nonzero: depth bounds test (Metal has none: the back ignores it and says so).
        depth_bounds: u8,
        _pad: u8,
        front: StencilFace,
        back: StencilFace,
    }
}

array_wire!(InputElement, RtBlend);

wire_struct! {
    /// Everything a graphics pipeline is made of.
    pub struct GraphicsPsoDesc {
        vs: StageCode,
        ps: StageCode,
        /// Amplification (object) stage.
        amp: StageCode,
        ms: StageCode,
        /// Geometry stage (with `vs`): the pipeline is a vertex pipeline whose
        /// primitives the geometry shader rewrites.
        gs: StageCode,
        /// Valid entries of `inputs`.
        input_count: u32,
        /// Reserved, 0.
        flags: u32,
        inputs: [InputElement; 32],
        blend: BlendDesc,
        raster: RasterDesc,
        depth: DepthStencilDesc,
        /// Format ids of the render targets; entries from `rt_count` on are 0.
        rt_formats: [u32; 8],
        rt_count: u32,
        /// Format id of the depth-stencil target, 0 = none.
        ds_format: u32,
        sample_count: u32,
        sample_mask: u32,
        topology: TopologyType,
        strip_cut: StripCut,
        _pad: [u8; 2],
        _pad2: u32,
    }
}

/// Which pipeline a description makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PsoKind {
    /// `vs` (+ `ps`): vertex, rasterizer.
    Vertex,
    /// `ms` (+ `amp`, `ps`): mesh.
    Mesh,
    /// `vs` + `gs` (+ `ps`): vertex pipeline with a geometry shader.
    Geometry,
}

impl GraphicsPsoDesc {
    /// The `Default` blank with the usual defaults filled in: blending off,
    /// solid fill, back-face culling, clockwise front faces, depth clip on,
    /// depth test `LESS` with writes, no stencil, one sample, every sample
    /// in the mask, triangles. No code, no inputs, no targets.
    pub fn new() -> Self {
        let mut d = <Self as Wire>::ZERO;
        d.blend.rt = [RtBlend::opaque(); 8];
        d.raster.cull = CullMode::BACK;
        d.raster.depth_clip = 1;
        d.depth.depth_enable = 1;
        d.depth.depth_write = 1;
        d.depth.depth_func = crate::types::CompareFunc::LESS;
        d.depth.stencil_read_mask = 0xFF;
        d.depth.stencil_write_mask = 0xFF;
        let keep = StencilFace {
            fail: StencilOp::KEEP,
            depth_fail: StencilOp::KEEP,
            pass: StencilOp::KEEP,
            func: crate::types::CompareFunc::ALWAYS,
        };
        d.depth.front = keep;
        d.depth.back = keep;
        d.sample_count = 1;
        d.sample_mask = u32::MAX;
        d.topology = TopologyType::TRIANGLE;
        d
    }

    /// The input elements in use.
    pub fn input_elements(&self) -> &[InputElement] {
        &self.inputs[..(self.input_count as usize).min(MAX_INPUT_ELEMENTS)]
    }

    /// The pipeline kind the stages say, or `None` for a combination that
    /// is no pipeline.
    pub fn kind(&self) -> Option<PsoKind> {
        match (
            self.vs.is_present(),
            self.amp.is_present(),
            self.ms.is_present(),
            self.gs.is_present(),
        ) {
            (true, false, false, false) => Some(PsoKind::Vertex),
            (true, false, false, true) => Some(PsoKind::Geometry),
            (false, _, true, false) => Some(PsoKind::Mesh),
            _ => None,
        }
    }

    /// Checks everything the decoder can without knowing the formats:
    /// known enum values, counts, names, entries past the counts zero, a
    /// stage combination that is a pipeline, code with an address and a length.
    pub fn check(&self) -> Result<(), Error> {
        let bad = |field| Error::BadValue {
            opcode: OPCODE_NONE,
            field,
        };
        for s in [&self.vs, &self.ps, &self.amp, &self.ms, &self.gs] {
            if s.is_present() && (s.addr == 0 || s.len == 0) {
                return Err(bad("stage code"));
            }
        }
        if self.kind().is_none() {
            return Err(bad("stages"));
        }
        if self.flags != 0 {
            return Err(bad("flags"));
        }
        if self.input_count as usize > MAX_INPUT_ELEMENTS {
            return Err(bad("input_count"));
        }
        for (i, e) in self.inputs.iter().enumerate() {
            if i >= self.input_count as usize {
                if *e != <InputElement as Wire>::ZERO {
                    return Err(bad("input past the count"));
                }
                continue;
            }
            if !e.class.is_known() {
                return Err(bad("input class"));
            }
            if !e.semantic.contains(&0)
                || !e.semantic.iter().all(|b| b.is_ascii())
                || e.semantic[0] == 0
            {
                return Err(bad("semantic"));
            }
        }
        if self.rt_count as usize > MAX_RENDER_TARGETS {
            return Err(bad("rt_count"));
        }
        if self.rt_formats[self.rt_count as usize..]
            .iter()
            .any(|&f| f != 0)
        {
            return Err(bad("rt format past the count"));
        }
        if !self.topology.is_known() || !self.strip_cut.is_known() {
            return Err(bad("topology"));
        }
        if self.sample_count == 0 || self.sample_count > 16 {
            return Err(bad("sample_count"));
        }
        let b = &self.blend;
        for r in &b.rt {
            let known = r.logic_op.is_known()
                && r.src.is_known()
                && r.dst.is_known()
                && r.op.is_known()
                && r.src_alpha.is_known()
                && r.dst_alpha.is_known()
                && r.op_alpha.is_known();
            if !known || r.write_mask & !0xF != 0 {
                return Err(bad("blend"));
            }
        }
        let rs = &self.raster;
        if !rs.fill.is_known() || !rs.cull.is_known() {
            return Err(bad("rasterizer"));
        }
        let d = &self.depth;
        let face = |f: &StencilFace| {
            f.fail.is_known() && f.depth_fail.is_known() && f.pass.is_known() && f.func.is_known()
        };
        if !d.depth_func.is_known() || !face(&d.front) || !face(&d.back) {
            return Err(bad("depth stencil"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_is_a_multiple_of_eight_and_fixed() {
        assert_eq!(<GraphicsPsoDesc as Wire>::SIZE % 8, 0);
        assert_eq!(<InputElement as Wire>::SIZE, 56);
        assert_eq!(<RtBlend as Wire>::SIZE, 12);
        assert_eq!(<BlendDesc as Wire>::SIZE, 100);
        assert_eq!(<RasterDesc as Wire>::SIZE, 20);
        assert_eq!(<DepthStencilDesc as Wire>::SIZE, 16);
        assert_eq!(<GraphicsPsoDesc as Wire>::SIZE, 2072);
    }

    fn with_code() -> GraphicsPsoDesc {
        let mut d = GraphicsPsoDesc::new();
        d.vs = StageCode { addr: 8, len: 8 };
        d
    }

    #[test]
    fn kinds_follow_the_stages() {
        let mut d = with_code();
        assert_eq!(d.kind(), Some(PsoKind::Vertex));
        d.ms = StageCode { addr: 8, len: 8 };
        assert_eq!(d.kind(), None, "vs with ms is no pipeline");
        d.vs = StageCode::NONE;
        assert_eq!(d.kind(), Some(PsoKind::Mesh));
        d.amp = StageCode { addr: 8, len: 8 };
        assert_eq!(d.kind(), Some(PsoKind::Mesh));
        d.ms = StageCode::NONE;
        assert_eq!(d.kind(), None, "amp alone is no pipeline");
        assert_eq!(GraphicsPsoDesc::new().kind(), None);
        let mut g = with_code();
        g.gs = StageCode { addr: 8, len: 8 };
        assert_eq!(g.kind(), Some(PsoKind::Geometry));
        g.ms = StageCode { addr: 8, len: 8 };
        assert_eq!(g.kind(), None, "gs with ms is no pipeline");
        g.ms = StageCode::NONE;
        g.vs = StageCode::NONE;
        assert_eq!(g.kind(), None, "gs without vs is no pipeline");
    }

    #[test]
    fn check_refuses_what_it_can_see() {
        assert_eq!(with_code().check(), Ok(()));
        let mut d = with_code();
        d.input_count = 1;
        d.inputs[0] = InputElement::new("POSITION", 0, 6, 0, 0);
        assert_eq!(d.check(), Ok(()));
        assert_eq!(d.input_elements()[0].name(), "POSITION");
        let mut e = d;
        e.inputs[0].semantic = [b'A'; 32];
        assert!(e.check().is_err(), "no NUL");
        let mut e = d;
        e.inputs[1] = InputElement::new("X", 0, 6, 0, 0);
        assert!(e.check().is_err(), "an input past the count");
        let mut e = d;
        e.input_count = 33;
        assert!(e.check().is_err());
        let mut e = d;
        e.rt_count = 9;
        assert!(e.check().is_err());
        let mut e = d;
        e.rt_formats[3] = 28;
        assert!(e.check().is_err(), "format past the count");
        e.rt_count = 4;
        assert_eq!(e.check(), Ok(()));
        let mut e = d;
        e.blend.rt[2].src = BlendFactor(99);
        assert!(e.check().is_err());
        let mut e = d;
        e.raster.cull = CullMode(9);
        assert!(e.check().is_err());
        let mut e = d;
        e.depth.front.pass = StencilOp(8);
        assert!(e.check().is_err());
        let mut e = d;
        e.sample_count = 0;
        assert!(e.check().is_err());
        let mut e = d;
        e.vs.len = 0;
        assert!(e.check().is_err(), "an address without a length");
    }
}
