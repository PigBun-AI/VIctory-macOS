//! `CREATE_GRAPHICS_PIPELINE`: the proto description to Metal's terms.
//!
//! The one place the protocol's pipeline enums meet Metal's: every table is
//! here, each a total function with a test that walks every value of the
//! enum (`tests`). The core ([`crate::render`]) only sees Metal values.
//!
//! Formats: DXGI numbers travel through proto untouched. Render target and
//! depth-stencil formats go through [`crate::format`], the one DXGI ->
//! `MTLPixelFormat` table; `vertex_format` maps vertex input formats to
//! `MTLVertexFormat`, which that table does not cover.

use std::sync::Arc;

use objc2_metal::{
    MTLBlendFactor, MTLBlendOperation, MTLColorWriteMask, MTLCompareFunction, MTLCullMode,
    MTLDepthClipMode, MTLPixelFormat, MTLPrimitiveTopologyClass, MTLStencilOperation,
    MTLTriangleFillMode, MTLVertexFormat, MTLWinding,
};
use victory_metal_proto::dropstats::{Label, PipeType};
use victory_metal_proto::pso::{
    APPEND_ALIGNED, BlendFactor, BlendOp, CullMode, DepthStencilDesc, FillMode, GraphicsPsoDesc,
    InputClass, LogicOp, PsoKind, StageCode, StencilFace as PStencilFace, StencilOp, TopologyType,
};
use victory_metal_proto::status::Status;
use victory_metal_proto::sync::CreateGraphicsPipelineArgs;
use victory_metal_proto::types::CompareFunc;

use super::dxil::status_of_msc;
use super::sync::{exhausted, range_is_mapped};
use super::{Executor, Pso, PsoBody};
use crate::format::{self, Class};
use crate::msc::RootSignatureDesc;
use crate::render::{
    ColorTarget, DepthStencil, GraphicsDesc, Raster, StencilFace, TargetBlend, VertexElement,
};

/// Largest DXIL container of one stage.
const MAX_DXIL_BYTES: u64 = 64 << 20;

// ---- the tables ---------------------------------------------------------------

pub(crate) fn blend_factor(f: BlendFactor) -> Option<MTLBlendFactor> {
    Some(match f {
        BlendFactor::ZERO => MTLBlendFactor::Zero,
        BlendFactor::ONE => MTLBlendFactor::One,
        BlendFactor::SRC_COLOR => MTLBlendFactor::SourceColor,
        BlendFactor::INV_SRC_COLOR => MTLBlendFactor::OneMinusSourceColor,
        BlendFactor::SRC_ALPHA => MTLBlendFactor::SourceAlpha,
        BlendFactor::INV_SRC_ALPHA => MTLBlendFactor::OneMinusSourceAlpha,
        BlendFactor::DST_ALPHA => MTLBlendFactor::DestinationAlpha,
        BlendFactor::INV_DST_ALPHA => MTLBlendFactor::OneMinusDestinationAlpha,
        BlendFactor::DST_COLOR => MTLBlendFactor::DestinationColor,
        BlendFactor::INV_DST_COLOR => MTLBlendFactor::OneMinusDestinationColor,
        BlendFactor::SRC_ALPHA_SAT => MTLBlendFactor::SourceAlphaSaturated,
        BlendFactor::CONSTANT_COLOR => MTLBlendFactor::BlendColor,
        BlendFactor::INV_CONSTANT_COLOR => MTLBlendFactor::OneMinusBlendColor,
        BlendFactor::SRC1_COLOR => MTLBlendFactor::Source1Color,
        BlendFactor::INV_SRC1_COLOR => MTLBlendFactor::OneMinusSource1Color,
        BlendFactor::SRC1_ALPHA => MTLBlendFactor::Source1Alpha,
        BlendFactor::INV_SRC1_ALPHA => MTLBlendFactor::OneMinusSource1Alpha,
        BlendFactor::CONSTANT_ALPHA => MTLBlendFactor::BlendAlpha,
        BlendFactor::INV_CONSTANT_ALPHA => MTLBlendFactor::OneMinusBlendAlpha,
        _ => return None,
    })
}

pub(crate) fn blend_op(o: BlendOp) -> Option<MTLBlendOperation> {
    Some(match o {
        BlendOp::ADD => MTLBlendOperation::Add,
        BlendOp::SUBTRACT => MTLBlendOperation::Subtract,
        BlendOp::REV_SUBTRACT => MTLBlendOperation::ReverseSubtract,
        BlendOp::MIN => MTLBlendOperation::Min,
        BlendOp::MAX => MTLBlendOperation::Max,
        _ => return None,
    })
}

pub(crate) fn compare(c: CompareFunc) -> Option<MTLCompareFunction> {
    Some(match c {
        CompareFunc::NEVER => MTLCompareFunction::Never,
        CompareFunc::LESS => MTLCompareFunction::Less,
        CompareFunc::EQUAL => MTLCompareFunction::Equal,
        CompareFunc::LESS_EQUAL => MTLCompareFunction::LessEqual,
        CompareFunc::GREATER => MTLCompareFunction::Greater,
        CompareFunc::NOT_EQUAL => MTLCompareFunction::NotEqual,
        CompareFunc::GREATER_EQUAL => MTLCompareFunction::GreaterEqual,
        CompareFunc::ALWAYS => MTLCompareFunction::Always,
        _ => return None,
    })
}

pub(crate) fn stencil_op(o: StencilOp) -> Option<MTLStencilOperation> {
    Some(match o {
        StencilOp::KEEP => MTLStencilOperation::Keep,
        StencilOp::ZERO => MTLStencilOperation::Zero,
        StencilOp::REPLACE => MTLStencilOperation::Replace,
        StencilOp::INCR_SAT => MTLStencilOperation::IncrementClamp,
        StencilOp::DECR_SAT => MTLStencilOperation::DecrementClamp,
        StencilOp::INVERT => MTLStencilOperation::Invert,
        StencilOp::INCR_WRAP => MTLStencilOperation::IncrementWrap,
        StencilOp::DECR_WRAP => MTLStencilOperation::DecrementWrap,
        _ => return None,
    })
}

pub(crate) fn cull_mode(c: CullMode) -> Option<MTLCullMode> {
    Some(match c {
        CullMode::NONE => MTLCullMode::None,
        CullMode::FRONT => MTLCullMode::Front,
        CullMode::BACK => MTLCullMode::Back,
        _ => return None,
    })
}

pub(crate) fn fill_mode(f: FillMode) -> Option<MTLTriangleFillMode> {
    Some(match f {
        FillMode::SOLID => MTLTriangleFillMode::Fill,
        FillMode::WIREFRAME => MTLTriangleFillMode::Lines,
        _ => return None,
    })
}

pub(crate) fn topology_class(t: TopologyType) -> Option<MTLPrimitiveTopologyClass> {
    Some(match t {
        TopologyType::UNDEFINED => MTLPrimitiveTopologyClass::Unspecified,
        TopologyType::POINT => MTLPrimitiveTopologyClass::Point,
        TopologyType::LINE => MTLPrimitiveTopologyClass::Line,
        TopologyType::TRIANGLE => MTLPrimitiveTopologyClass::Triangle,
        _ => return None,
    })
}

/// The format numbers the tables below read, from the one table of them.
use crate::format::dxgi as dx;

/// Vertex input format -> Metal vertex format and the element's size in
/// bytes. Formats Metal cannot fetch as vertex input (packed 10-bit integers) are `None`.
pub(crate) fn vertex_format(dxgi: u32) -> Option<(MTLVertexFormat, u32)> {
    use MTLVertexFormat as V;
    Some(match dxgi {
        dx::R32G32B32A32_FLOAT => (V::Float4, 16),
        dx::R32G32B32A32_UINT => (V::UInt4, 16),
        dx::R32G32B32A32_SINT => (V::Int4, 16),
        dx::R32G32B32_FLOAT => (V::Float3, 12),
        dx::R32G32B32_UINT => (V::UInt3, 12),
        dx::R32G32B32_SINT => (V::Int3, 12),
        dx::R16G16B16A16_FLOAT => (V::Half4, 8),
        dx::R16G16B16A16_UNORM => (V::UShort4Normalized, 8),
        dx::R16G16B16A16_UINT => (V::UShort4, 8),
        dx::R16G16B16A16_SNORM => (V::Short4Normalized, 8),
        dx::R16G16B16A16_SINT => (V::Short4, 8),
        dx::R32G32_FLOAT => (V::Float2, 8),
        dx::R32G32_UINT => (V::UInt2, 8),
        dx::R32G32_SINT => (V::Int2, 8),
        dx::R10G10B10A2_UNORM => (V::UInt1010102Normalized, 4),
        dx::R11G11B10_FLOAT => (V::FloatRG11B10, 4),
        dx::R8G8B8A8_UNORM => (V::UChar4Normalized, 4),
        dx::R8G8B8A8_UINT => (V::UChar4, 4),
        dx::R8G8B8A8_SNORM => (V::Char4Normalized, 4),
        dx::R8G8B8A8_SINT => (V::Char4, 4),
        dx::R16G16_FLOAT => (V::Half2, 4),
        dx::R16G16_UNORM => (V::UShort2Normalized, 4),
        dx::R16G16_UINT => (V::UShort2, 4),
        dx::R16G16_SNORM => (V::Short2Normalized, 4),
        dx::R16G16_SINT => (V::Short2, 4),
        dx::R32_FLOAT => (V::Float, 4),
        dx::R32_UINT => (V::UInt, 4),
        dx::R32_SINT => (V::Int, 4),
        dx::R8G8_UNORM => (V::UChar2Normalized, 2),
        dx::R8G8_UINT => (V::UChar2, 2),
        dx::R8G8_SNORM => (V::Char2Normalized, 2),
        dx::R8G8_SINT => (V::Char2, 2),
        dx::R16_FLOAT => (V::Half, 2),
        dx::R16_UNORM => (V::UShortNormalized, 2),
        dx::R16_UINT => (V::UShort, 2),
        dx::R16_SNORM => (V::ShortNormalized, 2),
        dx::R16_SINT => (V::Short, 2),
        dx::R8_UNORM => (V::UCharNormalized, 1),
        dx::R8_UINT => (V::UChar, 1),
        dx::R8_SNORM => (V::CharNormalized, 1),
        dx::R8_SINT => (V::Char, 1),
        dx::B8G8R8A8_UNORM => (V::UChar4Normalized_BGRA, 4),
        _ => return None,
    })
}

// ---- the description ----------------------------------------------------------

fn stencil_face(f: &PStencilFace) -> Option<StencilFace> {
    Some(StencilFace {
        fail: stencil_op(f.fail)?,
        depth_fail: stencil_op(f.depth_fail)?,
        pass: stencil_op(f.pass)?,
        compare: compare(f.func)?,
    })
}

/// Depth testing off is `Always` with writes off (the original API ignores
/// the write mask then); stencil off keeps its faces for a later enable.
pub(crate) fn depth_stencil(d: &DepthStencilDesc) -> Option<DepthStencil> {
    let (depth_compare, depth_write) = if d.depth_enable != 0 {
        (compare(d.depth_func)?, d.depth_write != 0)
    } else {
        (MTLCompareFunction::Always, false)
    };
    Some(DepthStencil {
        depth_compare,
        depth_write,
        stencil: d.stencil_enable != 0,
        front: stencil_face(&d.front)?,
        back: stencil_face(&d.back)?,
        read_mask: u32::from(d.stencil_read_mask),
        write_mask: u32::from(d.stencil_write_mask),
    })
}

fn color_write_mask(bits: u8) -> MTLColorWriteMask {
    let mut m = MTLColorWriteMask::None;
    if bits & 1 != 0 {
        m |= MTLColorWriteMask::Red;
    }
    if bits & 2 != 0 {
        m |= MTLColorWriteMask::Green;
    }
    if bits & 4 != 0 {
        m |= MTLColorWriteMask::Blue;
    }
    if bits & 8 != 0 {
        m |= MTLColorWriteMask::Alpha;
    }
    m
}

/// Why a description cannot be made, for the status and the text.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// A value the description should not hold (unknown enum): `INVALID_ARG`.
    Invalid(String),
    /// State or a format this back does not make: `UNSUPPORTED`.
    Unsupported(String),
}

impl Refusal {
    fn status(&self) -> Status {
        match self {
            Refusal::Invalid(_) => Status::INVALID_ARG,
            Refusal::Unsupported(_) => Status::UNSUPPORTED,
        }
    }
    fn text(&self) -> &str {
        match self {
            Refusal::Invalid(s) | Refusal::Unsupported(s) => s,
        }
    }
}

fn invalid(what: &str) -> Refusal {
    Refusal::Invalid(format!("bad {what}"))
}

/// The Metal format of render target `dxgi` (from the format table) and whether
/// it is an integer format (blending is ignored for those). `0` is an unused
/// slot.
fn render_target_format(dxgi: u32) -> Result<(MTLPixelFormat, bool), Refusal> {
    if dxgi == format::dxgi::UNKNOWN {
        return Ok((MTLPixelFormat::Invalid, false));
    }
    let not_ok = || {
        Refusal::Unsupported(format!(
            "render target format {dxgi}, not one Metal can render to"
        ))
    };
    let f = format::lookup(dxgi).ok_or_else(not_ok)?;
    let pixel = f
        .pixel
        .filter(|_| f.class == Class::Color)
        .ok_or_else(not_ok)?;
    format::texture_usage(dxgi, victory_metal_proto::types::tex_usage::RENDER_TARGET)
        .map_err(|_| not_ok())?;
    Ok((
        pixel,
        f.name.ends_with("_UINT") || f.name.ends_with("_SINT"),
    ))
}

/// The input layout with append offsets resolved and Metal formats.
fn vertex_elements(d: &GraphicsPsoDesc) -> Result<Vec<VertexElement>, Refusal> {
    let mut out = Vec::new();
    // End of the previous element of each slot (for the append marker).
    let mut ends = [0u32; 32];
    for e in d.input_elements() {
        let (format, size) = vertex_format(e.format).ok_or_else(|| {
            Refusal::Unsupported(format!(
                "input element {}{} has format {}, which Metal cannot fetch",
                e.name(),
                e.semantic_index,
                e.format
            ))
        })?;
        let slot = e.slot as usize;
        if slot >= ends.len() {
            return Err(Refusal::Unsupported(format!(
                "vertex buffer slot {}",
                e.slot
            )));
        }
        let offset = if e.offset == APPEND_ALIGNED {
            ends[slot].next_multiple_of(size.min(4))
        } else {
            e.offset
        };
        ends[slot] = offset.saturating_add(size);
        out.push(VertexElement {
            semantic: e.name().to_owned(),
            semantic_index: e.semantic_index,
            format,
            dxgi_format: e.format,
            slot: e.slot,
            offset,
            per_instance: e.class == InputClass::PER_INSTANCE,
            step_rate: e.step_rate,
        });
    }
    Ok(out)
}

/// The core's description from the wire's. `code` holds the stages' DXIL (in
/// the order vs, ps, amp, ms, gs). Everything proto cannot say is checked here.
pub(crate) fn describe<'a>(
    d: &GraphicsPsoDesc,
    root: &'a RootSignatureDesc,
    code: [Option<&'a [u8]>; 5],
    compat: crate::msc::Compat,
) -> Result<GraphicsDesc<'a>, Refusal> {
    let [vs, ps, amp, ms, gs] = code;
    let raster = &d.raster;
    // Conservative rasterization: Metal has none. The pipeline is made
    // without it (a known difference, docs/metal/m4-render.md): the games
    // ask for it on a few passes where the plain rule only loses edge pixels.
    let blend = &d.blend;
    let mut targets = Vec::new();
    for i in 0..d.rt_count as usize {
        let rt = &blend.rt[if blend.independent != 0 { i } else { 0 }];
        let (format, integer) = render_target_format(d.rt_formats[i])?;
        if rt.logic_enable != 0 && rt.logic_op != LogicOp::NOOP {
            return Err(Refusal::Unsupported(
                "logic operations on render targets (Metal has none)".into(),
            ));
        }
        targets.push(ColorTarget {
            format,
            integer,
            blend: TargetBlend {
                // Blending into an integer target is ignored, as the console does.
                enabled: rt.enable != 0 && !integer,
                src_rgb: blend_factor(rt.src).ok_or_else(|| invalid("blend factor"))?,
                dst_rgb: blend_factor(rt.dst).ok_or_else(|| invalid("blend factor"))?,
                op_rgb: blend_op(rt.op).ok_or_else(|| invalid("blend op"))?,
                src_alpha: blend_factor(rt.src_alpha).ok_or_else(|| invalid("blend factor"))?,
                dst_alpha: blend_factor(rt.dst_alpha).ok_or_else(|| invalid("blend factor"))?,
                op_alpha: blend_op(rt.op_alpha).ok_or_else(|| invalid("blend op"))?,
                write_mask: color_write_mask(rt.write_mask),
            },
        });
    }
    if d.ds_format != 0 {
        let ok = format::lookup(d.ds_format).is_some_and(|f| {
            matches!(f.class, Class::Depth | Class::DepthStencil) && f.pixel.is_some()
        });
        if !ok {
            return Err(Refusal::Unsupported(format!(
                "depth-stencil format {}",
                d.ds_format
            )));
        }
    }
    Ok(GraphicsDesc {
        vs,
        ps,
        amp,
        ms,
        gs,
        root,
        inputs: vertex_elements(d)?,
        targets,
        topology: topology_class(d.topology).ok_or_else(|| invalid("topology"))?,
        sample_count: d.sample_count,
        sample_mask: d.sample_mask,
        alpha_to_coverage: blend.alpha_to_coverage != 0,
        has_depth_target: d.ds_format != 0,
        depth_stencil: depth_stencil(&d.depth).ok_or_else(|| invalid("depth-stencil state"))?,
        raster: Raster {
            fill: fill_mode(raster.fill).ok_or_else(|| invalid("fill mode"))?,
            cull: cull_mode(raster.cull).ok_or_else(|| invalid("cull mode"))?,
            front_winding: if raster.front_ccw != 0 {
                MTLWinding::CounterClockwise
            } else {
                MTLWinding::Clockwise
            },
            depth_clip: if raster.depth_clip != 0 {
                MTLDepthClipMode::Clip
            } else {
                MTLDepthClipMode::Clamp
            },
            depth_bias: raster.depth_bias as f32,
            slope_scale: raster.slope_scaled_bias,
            bias_clamp: raster.depth_bias_clamp,
        },
        compat,
    })
}

/// The bytes of a stage: checked for size and mapping.
fn stage_bytes(s: &StageCode) -> Result<Option<&'static [u8]>, Status> {
    if !s.is_present() {
        return Ok(None);
    }
    if s.len > MAX_DXIL_BYTES {
        return Err(Status::INVALID_ARG);
    }
    let page = 16384usize;
    let start = s.addr as usize & !(page - 1);
    let span = s.addr as usize + s.len as usize - start;
    if !range_is_mapped(start, span) {
        return Err(Status::INVALID_ARG);
    }
    // SAFETY: the `Link` contract: the bytes are the caller's, valid for the
    // call; the slice never outlives `create_graphics_pipeline`.
    Ok(Some(unsafe {
        std::slice::from_raw_parts(s.addr as *const u8, s.len as usize)
    }))
}

impl Executor {
    /// `CREATE_GRAPHICS_PIPELINE`: the stages' DXIL through MSC against the
    /// root signature, then an `MTL4Compiler` render or mesh pipeline. The
    /// conversion and compile (milliseconds to a second) run outside the
    /// state lock, so several threads may create pipelines at once.
    pub(super) fn create_graphics_pipeline(
        &self,
        a: &mut CreateGraphicsPipelineArgs,
    ) -> Result<(), Status> {
        let started = std::time::Instant::now();
        let stats = crate::dropstats::get();
        if stats.is_some() {
            crate::dropstats::take_reason();
        }
        let mut label = Label::NONE;
        let r = self.create_graphics_pipeline_inner(a, &mut label);
        let kind = match label.ty {
            PipeType::Ms => "graphics-ms",
            PipeType::Gs => "graphics-gs",
            _ => "graphics-vs",
        };
        // A pipeline made slowly is logged (`VICTORY_METAL_COMPILE_LOG_MS`, 100 by default).
        crate::prof::compiled(
            kind,
            label.hash,
            started.elapsed(),
            std::time::Duration::from_micros(crate::archive::peek_compile_us()),
        );
        if r.is_ok() {
            crate::prof::register_pipeline(a.pso.0, format!("{kind} {:016x}", label.hash));
        }
        let Some(stats) = stats else {
            crate::archive::take_compile_us();
            return r;
        };
        match &r {
            Ok(()) => {
                stats.register(a.pso.0, label);
                stats.pso_created(kind, label.hash, Ok(a.pso.0));
            }
            Err(s) => {
                let why = crate::dropstats::take_reason().unwrap_or_else(|| format!("{s:?}"));
                stats.pso_created(kind, label.hash, Err(&why));
            }
        }
        r
    }

    fn create_graphics_pipeline_inner(
        &self,
        a: &mut CreateGraphicsPipelineArgs,
        label: &mut Label,
    ) -> Result<(), Status> {
        if a.flags != 0 || a._pad != 0 {
            return Err(Status::INVALID_ARG);
        }
        a.desc.check().map_err(|e| {
            self.note_error(format!("CREATE_GRAPHICS_PIPELINE: {e}"));
            Status::INVALID_ARG
        })?;
        let kind = a.desc.kind().ok_or(Status::INVALID_ARG)?;
        let (device, sig) = self.with_state(|st| {
            st.root_sigs
                .get(a.root_sig.0)
                .map(|s| (st.device.clone(), s.clone()))
                .ok_or(Status::NOT_FOUND)
        })??;
        if kind == PsoKind::Mesh && !device.caps().mesh_shaders {
            self.note_error("CREATE_GRAPHICS_PIPELINE: the device has no mesh shaders".into());
            return Err(Status::UNSUPPORTED);
        }
        let d = &a.desc;
        let code = [
            stage_bytes(&d.vs)?,
            stage_bytes(&d.ps)?,
            stage_bytes(&d.amp)?,
            stage_bytes(&d.ms)?,
            stage_bytes(&d.gs)?,
        ];
        {
            let ty = match (kind, code[4].is_some()) {
                (PsoKind::Mesh, _) => PipeType::Ms,
                (_, true) => PipeType::Gs,
                _ => PipeType::Vs,
            };
            let parts: Vec<&[u8]> = code.iter().map(|c| c.unwrap_or(&[])).collect();
            *label = crate::dropstats::label_of_stages(ty, &parts);
        }
        let desc = describe(d, &sig.msc_desc, code, self.options.compat).map_err(|r| {
            self.note_error(format!("CREATE_GRAPHICS_PIPELINE: {}", r.text()));
            r.status()
        })?;
        let made = match device.graphics_pipeline_from_dxil(&desc) {
            // A mesh pipeline over Metal's 32 KB: once more with its
            // outputs packed tighter (docs/metal/mesh32k.md).
            Err(e) => crate::mesh_fit::retry(&device, &desc, &e).unwrap_or(Err(e)),
            made => made,
        };
        let made = match made {
            // A mesh pipeline Metal cannot take (too big after the repack,
            // Apple's compiler crashes on it, or it is on the denylist) runs
            // as a compute pass and a vertex pass instead of being refused
            // (docs/metal/mesh-compute.md, section 8).
            Err(e)
                if kind == PsoKind::Mesh
                    && self.options.mesh_compute
                    && super::emu::is_problem_pair(&e) =>
            {
                match crate::mesh_emu::Emulated::build(&device, &desc, &super::emu::app_layout(&sig)) {
                    Ok(em) => {
                        let pso = Arc::new(Pso {
                            body: PsoBody::Emulated(Arc::new(em)),
                            root_sig: sig,
                        });
                        a.pso = victory_metal_proto::ids::PsoId(self.with_state(|st| {
                            st.emulations += 1;
                            st.psos.insert(pso).map_err(exhausted)
                        })??);
                        return Ok(());
                    }
                    Err(why) => {
                        self.note_error(format!(
                            "CREATE_GRAPHICS_PIPELINE: {e}; the emulation: {why}"
                        ));
                        return Err(status_of_msc(&e));
                    }
                }
            }
            made => made,
        };
        let pipeline = made.map_err(|e| {
            self.note_error(format!("CREATE_GRAPHICS_PIPELINE: {e}"));
            status_of_msc(&e)
        })?;
        // The stages read their arguments where MSC put them for this root
        // signature; they must be where the interpreter writes them.
        if !super::dxil::layout_agrees(&sig, pipeline.layout()) {
            self.note_error(format!(
                "CREATE_GRAPHICS_PIPELINE: the stages' argument layout {:?} is not the root signature's {:?}",
                pipeline.layout(),
                sig.layout
            ));
            return Err(Status::INTERNAL);
        }
        let pso = Arc::new(Pso {
            body: PsoBody::Graphics(pipeline),
            root_sig: sig,
        });
        a.pso = victory_metal_proto::ids::PsoId(
            self.with_state(|st| st.psos.insert(pso).map_err(exhausted))??,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use victory_metal_proto::pso::{InputElement, RtBlend};

    #[test]
    fn every_blend_factor_has_a_distinct_metal_factor() {
        let mut seen = Vec::new();
        for v in 0..=18u8 {
            let m = blend_factor(BlendFactor(v)).unwrap_or_else(|| panic!("factor {v}"));
            assert!(!seen.contains(&m.0), "factor {v} repeats a Metal factor");
            seen.push(m.0);
        }
        assert_eq!(blend_factor(BlendFactor(19)), None);
        assert_eq!(
            blend_factor(BlendFactor::SRC1_COLOR),
            Some(MTLBlendFactor::Source1Color)
        );
        assert_eq!(
            blend_factor(BlendFactor::CONSTANT_ALPHA),
            Some(MTLBlendFactor::BlendAlpha)
        );
        assert_eq!(
            blend_factor(BlendFactor::SRC_ALPHA_SAT),
            Some(MTLBlendFactor::SourceAlphaSaturated)
        );
    }

    #[test]
    fn blend_ops_compare_and_stencil_ops_map_every_value() {
        let ops: Vec<_> = (0..=4u8).map(|v| blend_op(BlendOp(v)).unwrap().0).collect();
        assert_eq!(
            ops,
            [0, 1, 2, 3, 4],
            "add, subtract, reverse subtract, min, max in Metal's order"
        );
        assert_eq!(blend_op(BlendOp(5)), None);
        let cmp: Vec<_> = (0..=7u8)
            .map(|v| compare(CompareFunc(v)).unwrap().0)
            .collect();
        assert_eq!(
            cmp,
            [0, 1, 2, 3, 4, 5, 6, 7],
            "never, less, equal, less-equal, greater, not-equal, greater-equal, always"
        );
        assert_eq!(compare(CompareFunc(8)), None);
        assert_eq!(
            stencil_op(StencilOp::INCR_SAT),
            Some(MTLStencilOperation::IncrementClamp)
        );
        assert_eq!(
            stencil_op(StencilOp::DECR_WRAP),
            Some(MTLStencilOperation::DecrementWrap)
        );
        let all: Vec<_> = (0..=7u8)
            .map(|v| stencil_op(StencilOp(v)).unwrap().0)
            .collect();
        let mut sorted = all.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 8);
        assert_eq!(stencil_op(StencilOp(8)), None);
    }

    #[test]
    fn raster_and_topology_tables() {
        assert_eq!(cull_mode(CullMode::NONE), Some(MTLCullMode::None));
        assert_eq!(cull_mode(CullMode::FRONT), Some(MTLCullMode::Front));
        assert_eq!(cull_mode(CullMode::BACK), Some(MTLCullMode::Back));
        assert_eq!(cull_mode(CullMode(3)), None);
        assert_eq!(fill_mode(FillMode::SOLID), Some(MTLTriangleFillMode::Fill));
        assert_eq!(
            fill_mode(FillMode::WIREFRAME),
            Some(MTLTriangleFillMode::Lines)
        );
        assert_eq!(fill_mode(FillMode(2)), None);
        assert_eq!(
            topology_class(TopologyType::UNDEFINED),
            Some(MTLPrimitiveTopologyClass::Unspecified)
        );
        assert_eq!(
            topology_class(TopologyType::POINT),
            Some(MTLPrimitiveTopologyClass::Point)
        );
        assert_eq!(
            topology_class(TopologyType::LINE),
            Some(MTLPrimitiveTopologyClass::Line)
        );
        assert_eq!(
            topology_class(TopologyType::TRIANGLE),
            Some(MTLPrimitiveTopologyClass::Triangle)
        );
        assert_eq!(topology_class(TopologyType(4)), None);
    }

    #[test]
    fn write_masks_map_bit_for_bit() {
        assert_eq!(color_write_mask(0), MTLColorWriteMask::None);
        assert_eq!(color_write_mask(0xF), MTLColorWriteMask::All);
        assert_eq!(color_write_mask(1), MTLColorWriteMask::Red);
        assert_eq!(color_write_mask(8), MTLColorWriteMask::Alpha);
        assert_eq!(
            color_write_mask(0b0110),
            MTLColorWriteMask::Green | MTLColorWriteMask::Blue
        );
    }

    #[test]
    fn format_tables_agree_with_each_other() {
        // Every colour format of the vertex table with a render-target twin
        // has the same component class.
        let rt = |d| render_target_format(d).ok();
        assert_eq!(rt(0), Some((MTLPixelFormat::Invalid, false)));
        assert_eq!(
            rt(dx::R8G8B8A8_UNORM),
            Some((MTLPixelFormat::RGBA8Unorm, false))
        );
        assert_eq!(rt(dx::R32_UINT), Some((MTLPixelFormat::R32Uint, true)));
        assert_eq!(rt(dx::R8_SINT), Some((MTLPixelFormat::R8Sint, true)));
        assert_eq!(rt(9999), None);
        assert_eq!(rt(dx::R8G8B8A8_TYPELESS), None);
        assert_eq!(rt(dx::BC1_UNORM), None);
        let (f, size) = vertex_format(dx::R16G16_FLOAT).unwrap();
        assert_eq!((f, size), (MTLVertexFormat::Half2, 4));
        assert_eq!(
            vertex_format(dx::B8G8R8A8_UNORM).unwrap().0,
            MTLVertexFormat::UChar4Normalized_BGRA
        );
        assert_eq!(vertex_format(dx::R10G10B10A2_UINT), None);
        // Sizes: the vertex format's size is the DXGI format's byte size.
        for (dxgi, bytes) in [
            (dx::R32G32B32A32_FLOAT, 16),
            (dx::R32G32B32_UINT, 12),
            (dx::R16G16B16A16_UNORM, 8),
            (dx::R8G8_SNORM, 2),
            (dx::R8_UINT, 1),
        ] {
            assert_eq!(vertex_format(dxgi).unwrap().1, bytes, "format {dxgi}");
        }
    }

    #[test]
    fn depth_off_is_always_without_writes() {
        let mut d = GraphicsPsoDesc::new();
        d.depth.depth_enable = 0;
        d.depth.depth_func = CompareFunc::LESS;
        let s = depth_stencil(&d.depth).unwrap();
        assert_eq!(
            (s.depth_compare, s.depth_write),
            (MTLCompareFunction::Always, false)
        );
        d.depth.depth_enable = 1;
        let s = depth_stencil(&d.depth).unwrap();
        assert_eq!(
            (s.depth_compare, s.depth_write),
            (MTLCompareFunction::Less, true)
        );
        d.depth.depth_write = 0;
        assert!(!depth_stencil(&d.depth).unwrap().depth_write);
    }

    fn base() -> GraphicsPsoDesc {
        let mut d = GraphicsPsoDesc::new();
        d.vs = StageCode { addr: 8, len: 8 };
        d
    }

    #[test]
    fn append_offsets_follow_the_previous_element_of_the_slot() {
        let mut d = base();
        d.input_count = 4;
        d.inputs[0] = InputElement::new("POSITION", 0, dx::R32G32B32_FLOAT, 0, APPEND_ALIGNED);
        d.inputs[1] = InputElement::new("COLOR", 0, dx::R8G8B8A8_UNORM, 0, APPEND_ALIGNED);
        d.inputs[2] = InputElement::new("TEXCOORD", 0, dx::R16G16_FLOAT, 0, APPEND_ALIGNED);
        d.inputs[3] = InputElement::new("OFFS", 1, dx::R32G32_FLOAT, 1, APPEND_ALIGNED);
        let v = vertex_elements(&d).unwrap();
        assert_eq!(
            v.iter().map(|e| e.offset).collect::<Vec<_>>(),
            [0, 12, 16, 0]
        );
        assert_eq!(v[3].slot, 1);
        // An explicit offset is kept and moves the append point.
        d.inputs[1].offset = 20;
        let v = vertex_elements(&d).unwrap();
        assert_eq!(v[1].offset, 20);
        assert_eq!(v[2].offset, 24);
    }

    #[test]
    fn a_format_metal_cannot_fetch_is_a_clear_refusal() {
        let mut d = base();
        d.input_count = 1;
        d.inputs[0] = InputElement::new("POSITION", 0, dx::R10G10B10A2_UINT, 0, 0);
        match vertex_elements(&d) {
            Err(Refusal::Unsupported(t)) => {
                assert!(t.contains("POSITION0") && t.contains("format 25"), "{t}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn independent_blend_picks_each_targets_own_state() {
        let root = RootSignatureDesc::default();
        let mut d = base();
        d.rt_count = 2;
        d.rt_formats[0] = dx::R8G8B8A8_UNORM;
        d.rt_formats[1] = dx::R32_UINT;
        d.blend.rt[0] = RtBlend {
            enable: 1,
            src: BlendFactor::SRC_ALPHA,
            dst: BlendFactor::INV_SRC_ALPHA,
            ..RtBlend::opaque()
        };
        d.blend.rt[1] = RtBlend {
            enable: 1,
            ..RtBlend::opaque()
        };
        // Not independent: target 0's state for both.
        let g = describe(&d, &root, [None; 5], crate::msc::Compat::NONE).unwrap();
        assert!(g.targets[0].blend.enabled);
        assert_eq!(g.targets[0].blend.src_rgb, MTLBlendFactor::SourceAlpha);
        assert!(
            !g.targets[1].blend.enabled,
            "an integer target never blends"
        );
        assert!(g.targets[1].integer);
        d.blend.independent = 1;
        d.blend.rt[0].enable = 0;
        let g = describe(&d, &root, [None; 5], crate::msc::Compat::NONE).unwrap();
        assert!(!g.targets[0].blend.enabled);
        assert_eq!(g.targets[1].format, MTLPixelFormat::R32Uint);
    }

    #[test]
    fn what_metal_lacks_is_refused_by_name() {
        let root = RootSignatureDesc::default();
        // Conservative rasterization is accepted and ignored.
        let mut d = base();
        d.raster.conservative = 1;
        assert!(describe(&d, &root, [None; 5], crate::msc::Compat::NONE).is_ok());
        let mut d = base();
        d.rt_count = 1;
        d.rt_formats[0] = dx::R8G8B8A8_UNORM;
        d.blend.rt[0].logic_enable = 1;
        d.blend.rt[0].logic_op = LogicOp::XOR;
        assert!(matches!(
            describe(&d, &root, [None; 5], crate::msc::Compat::NONE),
            Err(Refusal::Unsupported(t)) if t.contains("logic")
        ));
        let mut d = base();
        d.rt_count = 1;
        d.rt_formats[0] = 9999;
        let r = describe(&d, &root, [None; 5], crate::msc::Compat::NONE);
        assert!(matches!(&r, Err(Refusal::Unsupported(t)) if t.contains("9999")));
        assert_eq!(r.err().unwrap().status(), Status::UNSUPPORTED);
        let mut d = base();
        d.ds_format = 9999;
        assert!(describe(&d, &root, [None; 5], crate::msc::Compat::NONE).is_err());
    }

    #[test]
    fn raster_state_maps() {
        let root = RootSignatureDesc::default();
        let mut d = base();
        d.raster.fill = FillMode::WIREFRAME;
        d.raster.cull = CullMode::FRONT;
        d.raster.front_ccw = 1;
        d.raster.depth_clip = 0;
        d.raster.depth_bias = -5;
        d.raster.slope_scaled_bias = 2.0;
        d.raster.depth_bias_clamp = 0.5;
        d.topology = TopologyType::LINE;
        d.sample_count = 4;
        d.blend.alpha_to_coverage = 1;
        let g = describe(&d, &root, [None; 5], crate::msc::Compat::NONE).unwrap();
        assert_eq!(g.raster.fill, MTLTriangleFillMode::Lines);
        assert_eq!(g.raster.cull, MTLCullMode::Front);
        assert_eq!(g.raster.front_winding, MTLWinding::CounterClockwise);
        assert_eq!(g.raster.depth_clip, MTLDepthClipMode::Clamp);
        assert_eq!(
            (
                g.raster.depth_bias,
                g.raster.slope_scale,
                g.raster.bias_clamp
            ),
            (-5.0, 2.0, 0.5)
        );
        assert_eq!(g.topology, MTLPrimitiveTopologyClass::Line);
        assert_eq!(g.sample_count, 4);
        assert!(g.alpha_to_coverage);
        // The defaults: clockwise front faces, clip, back-face culling.
        let g = describe(&base(), &root, [None; 5], crate::msc::Compat::NONE).unwrap();
        assert_eq!(g.raster.front_winding, MTLWinding::Clockwise);
        assert_eq!(g.raster.depth_clip, MTLDepthClipMode::Clip);
        assert_eq!(g.raster.cull, MTLCullMode::Back);
    }
}
