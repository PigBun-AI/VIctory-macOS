//! The common state a graphics or mesh shader is compiled with when no title
//! state is known (offline censuses, warm-up archives, tests): opaque
//! blending, no depth test, no culling, 1 sample. Plain-Rust inputs, so the
//! callers need no Metal types.

use objc2_metal::{
    MTLBlendFactor, MTLBlendOperation, MTLCompareFunction, MTLCullMode, MTLDepthClipMode,
    MTLPixelFormat, MTLPrimitiveTopologyClass, MTLStencilOperation, MTLTriangleFillMode,
    MTLVertexFormat, MTLWinding,
};

use crate::msc::{Compat, RootSignatureDesc};
use crate::render::{
    ColorTarget, DepthStencil, GraphicsDesc, Raster, StencilFace, TargetBlend, VertexElement,
};

/// What a shader input or output is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Component {
    Float,
    Uint,
    Sint,
}

/// One vertex input, fed from slot 0 as `width` components of `component`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefaultInput {
    /// Semantic name without its index.
    pub semantic: String,
    pub index: u32,
    pub component: Component,
    /// 1 to 4.
    pub width: u32,
}

/// Which of the two common states to build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// Opaque 8-bit (or 32-bit integer) targets, no depth target.
    Plain,
    /// 16-bit float targets with alpha blending, a depth target with a
    /// depth test, back-face culling, 4x multisampling.
    Blended,
}

fn vertex_format(c: Component, width: u32) -> MTLVertexFormat {
    let w = width.clamp(1, 4) as usize;
    match c {
        Component::Float => [
            MTLVertexFormat::Float,
            MTLVertexFormat::Float2,
            MTLVertexFormat::Float3,
            MTLVertexFormat::Float4,
        ][w - 1],
        Component::Uint => [
            MTLVertexFormat::UInt,
            MTLVertexFormat::UInt2,
            MTLVertexFormat::UInt3,
            MTLVertexFormat::UInt4,
        ][w - 1],
        Component::Sint => [
            MTLVertexFormat::Int,
            MTLVertexFormat::Int2,
            MTLVertexFormat::Int3,
            MTLVertexFormat::Int4,
        ][w - 1],
    }
}

/// `DXGI_FORMAT_R32*_FLOAT/UINT/SINT` of `width` components.
fn dxgi_format(c: Component, width: u32) -> u32 {
    let w = width.clamp(1, 4) as usize;
    match c {
        Component::Float => [41, 16, 6, 2][w - 1],
        Component::Uint => [42, 17, 7, 3][w - 1],
        Component::Sint => [43, 18, 8, 4][w - 1],
    }
}

/// The DXIL containers of a pipeline's stages.
#[derive(Clone, Copy, Default)]
pub struct Stages<'a> {
    pub vs: Option<&'a [u8]>,
    pub ps: Option<&'a [u8]>,
    pub ms: Option<&'a [u8]>,
    /// A geometry stage (with `vs`).
    pub gs: Option<&'a [u8]>,
}

/// A pipeline description with the given stages, inputs (each at the next
/// 16 bytes of slot 0) and one render target per entry of `targets`.
pub fn default_graphics<'a>(
    root: &'a RootSignatureDesc,
    stages: Stages<'a>,
    inputs: &[DefaultInput],
    targets: &[Component],
    depth_output: bool,
    variant: Variant,
) -> GraphicsDesc<'a> {
    let Stages { vs, ps, ms, gs } = stages;
    let blended = variant == Variant::Blended;
    let keep = StencilFace {
        fail: MTLStencilOperation::Keep,
        depth_fail: MTLStencilOperation::Keep,
        pass: MTLStencilOperation::Keep,
        compare: MTLCompareFunction::Always,
    };
    let target = |c: Component| {
        let integer = c != Component::Float;
        let format = match (c, blended) {
            (Component::Float, false) => MTLPixelFormat::RGBA8Unorm,
            (Component::Float, true) => MTLPixelFormat::RGBA16Float,
            (Component::Uint, _) => MTLPixelFormat::RGBA32Uint,
            (Component::Sint, _) => MTLPixelFormat::RGBA32Sint,
        };
        let mut blend = TargetBlend::opaque();
        if blended && !integer {
            blend.enabled = true;
            blend.src_rgb = MTLBlendFactor::SourceAlpha;
            blend.dst_rgb = MTLBlendFactor::OneMinusSourceAlpha;
            blend.op_rgb = MTLBlendOperation::Add;
            blend.src_alpha = MTLBlendFactor::One;
            blend.dst_alpha = MTLBlendFactor::OneMinusSourceAlpha;
        }
        ColorTarget {
            format,
            integer,
            blend,
        }
    };
    GraphicsDesc {
        vs,
        ps,
        amp: None,
        ms,
        gs,
        root,
        inputs: inputs
            .iter()
            .enumerate()
            .map(|(i, e)| VertexElement {
                semantic: e.semantic.clone(),
                semantic_index: e.index,
                format: vertex_format(e.component, e.width),
                dxgi_format: dxgi_format(e.component, e.width),
                slot: 0,
                offset: 16 * i as u32,
                per_instance: false,
                step_rate: 1,
            })
            .collect(),
        targets: targets.iter().map(|&c| target(c)).collect(),
        topology: if ms.is_some() {
            MTLPrimitiveTopologyClass::Unspecified
        } else {
            MTLPrimitiveTopologyClass::Triangle
        },
        sample_count: if blended { 4 } else { 1 },
        sample_mask: u32::MAX,
        alpha_to_coverage: false,
        has_depth_target: depth_output || blended,
        depth_stencil: DepthStencil {
            depth_compare: if blended {
                MTLCompareFunction::LessEqual
            } else {
                MTLCompareFunction::Always
            },
            depth_write: blended,
            stencil: false,
            front: keep,
            back: keep,
            read_mask: 0xFF,
            write_mask: 0xFF,
        },
        raster: Raster {
            fill: MTLTriangleFillMode::Fill,
            cull: if blended {
                MTLCullMode::Back
            } else {
                MTLCullMode::None
            },
            front_winding: MTLWinding::Clockwise,
            depth_clip: MTLDepthClipMode::Clip,
            depth_bias: 0.0,
            slope_scale: 0.0,
            bias_clamp: 0.0,
        },
        compat: Compat::NONE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_state_has_one_target_per_component_and_no_depth() {
        let root = RootSignatureDesc::default();
        let d = default_graphics(
            &root,
            Stages::default(),
            &[DefaultInput {
                semantic: "position".into(),
                index: 0,
                component: Component::Float,
                width: 3,
            }],
            &[Component::Float, Component::Uint],
            false,
            Variant::Plain,
        );
        assert_eq!(d.targets.len(), 2);
        assert!(!d.targets[0].integer && d.targets[1].integer);
        assert!(!d.has_depth_target);
        assert_eq!(d.sample_count, 1);
        assert_eq!(d.inputs[0].format, MTLVertexFormat::Float3);
        assert_eq!(d.inputs[0].offset, 0);
    }

    #[test]
    fn the_blended_variant_differs_in_every_pipeline_state() {
        let root = RootSignatureDesc::default();
        let d = default_graphics(
            &root,
            Stages::default(),
            &[],
            &[Component::Float],
            false,
            Variant::Blended,
        );
        assert!(d.targets[0].blend.enabled);
        assert_eq!(d.targets[0].format, MTLPixelFormat::RGBA16Float);
        assert!(d.has_depth_target);
        assert_eq!(d.sample_count, 4);
    }
}
