//! Render passes, render state and draws on a Metal 4 render encoder.
//!
//! The proto front derives the pass ([`PassSpec`]: attachments, load actions,
//! clear values) and the back runs it. This module knows no proto types: the
//! interpreter in `exec/stream.rs` maps ids to [`Texture`]s and wire enums to
//! the plain values here.
//!
//! # One pass, one or more encoders
//!
//! [`CommandList::begin_pass`] only records the pass. The encoder is created
//! by the first draw (or clear, or the end of a pass that has a clear), so a
//! pass nothing draws into costs nothing and a load action that needs no
//! work needs no encoder. The barrier tracker ([`crate::barrier`]) decides
//! when the encoder ends early: a barrier that makes the pass wait on its own
//! fragment work, or a copy or dispatch recorded in the middle of it. The
//! next draw then starts a new encoder for the same pass with every load
//! action `Load` (what was cleared is already in the tile memory's backing
//! store) and the state shadow re-applied.
//!
//! # State
//!
//! Viewports, scissors, blend factor, stencil reference, topology, index and
//! vertex buffers live in [`RenderState`], which belongs to the list, not to
//! a pass (the original API keeps them across `OMSetRenderTargets`). The draw
//! writes what changed to the encoder; a fresh encoder gets all of it.
//! Pipeline-bound state (depth-stencil object, cull, fill, winding, depth
//! bias and clip) is applied when the pipeline changes.
//!
//! # Bindings
//!
//! The one argument table is set for the vertex, fragment, object and mesh
//! stages. Its slots: 0 descriptor heap, 1 sampler heap, 2 top-level argument
//! buffer ([`bind_point`]), 4 and 5 the draw arguments and uniforms of the
//! converter's runtime (`IRRuntimeDrawParams`, the index type: written per
//! draw into the constant ring), `6 + slot` vertex buffers with their strides.

use std::collections::HashMap;
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{
    MTL4CommandBuffer, MTL4Compiler, MTL4LibraryFunctionDescriptor, MTL4PipelineDescriptor,
    MTL4RenderCommandEncoder, MTL4RenderPassDescriptor, MTL4RenderPipelineDescriptor,
    MTLClearColor, MTLColorWriteMask, MTLCompareFunction, MTLCullMode, MTLDepthStencilDescriptor,
    MTLDepthStencilState, MTLDevice, MTLIndexType, MTLLoadAction, MTLPixelFormat,
    MTLPrimitiveTopologyClass, MTLPrimitiveType, MTLRenderPipelineState, MTLRenderStages,
    MTLScissorRect, MTLSize, MTLStencilDescriptor, MTLStencilOperation, MTLStoreAction, MTLTexture,
    MTLTriangleFillMode, MTLViewport, MTLVisibilityResultMode,
};

use crate::descriptors::{RingAlloc, bind_point};
use crate::error::{Error, Result};
use crate::indirect::{GS_GROUPS_AT, GsExpand, IndirectDispatch, Kernels};
use crate::render::{GeometryPipelineInfo, GraphicsPipeline, Kind};
use crate::stages::{FRAGMENT, MESH, OBJECT, VERTEX};
use crate::texture::{TexDim, Texture, ViewDim, ViewSpec};

pub(crate) type RenderEncoder = Retained<ProtocolObject<dyn MTL4RenderCommandEncoder>>;

/// Most colour attachments.
pub const MAX_COLOR_ATTACHMENTS: usize = 8;
/// Vertex buffer slots (D3D12 has 16; the table has room for 6 + 16).
pub const MAX_VERTEX_BUFFERS: usize = 16;
/// Bytes of the draw-parameter block written per draw: `IRRuntimeDrawParams`
/// (20 bytes) and the `uint16` index type 32 bytes behind it.
const DRAW_BLOCK: u64 = 64;
const UNIFORMS_AT: u64 = 32;

/// What a pass does with an attachment's contents when it starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Load {
    Load,
    Clear,
    DontCare,
}

impl Load {
    fn metal(self) -> MTLLoadAction {
        match self {
            Load::Load => MTLLoadAction::Load,
            Load::Clear => MTLLoadAction::Clear,
            Load::DontCare => MTLLoadAction::DontCare,
        }
    }
}

/// A texture as a render target: which part of it.
#[derive(Clone)]
pub struct TargetSpec {
    pub texture: Texture,
    /// DXGI number of the view's format (0 = the texture's own).
    pub format: u32,
    pub dim: ViewDim,
    pub mip: u32,
    /// First array slice, or the W slice of a 3D texture.
    pub first_slice: u32,
    /// Array slices rendered to (layered rendering when more than one).
    pub slice_count: u32,
}

/// A colour attachment.
#[derive(Clone)]
pub struct ColorAttachment {
    pub target: TargetSpec,
    pub load: Load,
    pub clear: [f32; 4],
}

/// The depth-stencil attachment; the stencil plane is attached when the
/// texture's format has one.
#[derive(Clone)]
pub struct DepthAttachment {
    pub target: TargetSpec,
    pub depth_load: Load,
    pub stencil_load: Load,
    pub clear_depth: f32,
    pub clear_stencil: u32,
}

/// A render pass as the front derived it.
#[derive(Clone, Default)]
pub struct PassSpec {
    /// Slot `i` is attachment `i`; `None` is an unused slot.
    pub colors: Vec<Option<ColorAttachment>>,
    pub depth: Option<DepthAttachment>,
    /// The size of a pass with no attachment at all.
    pub empty_size: Option<[u32; 2]>,
}

/// What a pass ends with: which stores can be skipped.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stores {
    /// `true` = the contents of colour attachment `i` are not needed.
    pub color_dont_care: [bool; MAX_COLOR_ATTACHMENTS],
    pub depth_dont_care: bool,
    pub stencil_dont_care: bool,
}

/// Primitive topologies of draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Primitive {
    PointList,
    LineList,
    LineStrip,
    TriangleList,
    TriangleStrip,
}

impl Primitive {
    /// The `IRInputPrimitive` a geometry shader reading this primitive has.
    fn geometry_input(self) -> u32 {
        match self {
            Primitive::PointList => 1,
            Primitive::LineList | Primitive::LineStrip => 2,
            Primitive::TriangleList | Primitive::TriangleStrip => 3,
        }
    }

    /// `IRRuntimePrimitiveType`.
    fn ir_type(self) -> u8 {
        match self {
            Primitive::PointList => 0,
            Primitive::LineList => 1,
            Primitive::LineStrip => 2,
            Primitive::TriangleList => 3,
            Primitive::TriangleStrip => 4,
        }
    }

    /// Vertices one primitive reads.
    fn vertex_count(self) -> u32 {
        match self {
            Primitive::PointList => 1,
            Primitive::LineList | Primitive::LineStrip => 2,
            Primitive::TriangleList | Primitive::TriangleStrip => 3,
        }
    }

    /// Vertices a strip shares with the next primitive.
    fn strip_overlap(self) -> u32 {
        match self {
            Primitive::LineStrip => 1,
            Primitive::TriangleStrip => 2,
            _ => 0,
        }
    }

    fn is_strip(self) -> bool {
        matches!(self, Primitive::LineStrip | Primitive::TriangleStrip)
    }

    fn metal(self) -> MTLPrimitiveType {
        match self {
            Primitive::PointList => MTLPrimitiveType::Point,
            Primitive::LineList => MTLPrimitiveType::Line,
            Primitive::LineStrip => MTLPrimitiveType::LineStrip,
            Primitive::TriangleList => MTLPrimitiveType::Triangle,
            Primitive::TriangleStrip => MTLPrimitiveType::TriangleStrip,
        }
    }

    fn class(self) -> MTLPrimitiveTopologyClass {
        match self {
            Primitive::PointList => MTLPrimitiveTopologyClass::Point,
            Primitive::LineList | Primitive::LineStrip => MTLPrimitiveTopologyClass::Line,
            Primitive::TriangleList | Primitive::TriangleStrip => {
                MTLPrimitiveTopologyClass::Triangle
            }
        }
    }
}

/// A viewport in the original API's terms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

/// Arguments of a non-indexed draw.
#[derive(Clone, Copy, Debug)]
pub struct DrawArgs {
    pub vertex_count: u32,
    pub instance_count: u32,
    pub start_vertex: u32,
    pub start_instance: u32,
}

/// Arguments of an indexed draw.
#[derive(Clone, Copy, Debug)]
pub struct DrawIndexedArgs {
    pub index_count: u32,
    pub instance_count: u32,
    pub start_index: u32,
    pub base_vertex: i32,
    pub start_instance: u32,
}

#[derive(Clone, Copy)]
struct IndexBuffer {
    va: u64,
    size: u32,
    wide: bool,
}

#[derive(Clone, Copy)]
struct VertexBuffer {
    va: u64,
    stride: u32,
    size: u32,
}

const DIRTY_VIEWPORT: u8 = 1;
const DIRTY_SCISSOR: u8 = 2;
const DIRTY_BLEND: u8 = 4;
const DIRTY_STENCIL: u8 = 8;
const DIRTY_ALL: u8 = 15;

/// The render state a stream sets, kept for the next draw.
pub(crate) struct RenderState {
    viewports: Vec<MTLViewport>,
    scissors: Vec<[i32; 4]>,
    blend: [f32; 4],
    stencil_ref: u32,
    primitive: Option<Primitive>,
    index: Option<IndexBuffer>,
    vertex: [Option<VertexBuffer>; MAX_VERTEX_BUFFERS],
    dirty: u8,
    /// The pipeline the open encoder has (identity), if any.
    pipeline: Option<*const ()>,
}

impl Default for RenderState {
    fn default() -> Self {
        RenderState {
            viewports: Vec::new(),
            scissors: Vec::new(),
            // `OMSetBlendFactor` has never been called: (1, 1, 1, 1).
            blend: [1.0; 4],
            stencil_ref: 0,
            primitive: None,
            index: None,
            vertex: [None; MAX_VERTEX_BUFFERS],
            dirty: 0,
            pipeline: None,
        }
    }
}

/// A pass between `begin_pass` and `end_pass`.
pub(crate) struct Pass {
    spec: PassSpec,
    colors: Vec<Option<Retained<ProtocolObject<dyn MTLTexture>>>>,
    depth: Option<Retained<ProtocolObject<dyn MTLTexture>>>,
    has_stencil: bool,
    /// Size of the smallest attachment.
    size: [u32; 2],
    /// Layers rendered to (1 = not layered).
    layers: u32,
    /// An encoder has been made for it: later ones continue with `Load`.
    opened: bool,
    /// Formats the pipelines of its draws must have.
    pub(crate) color_formats: [MTLPixelFormat; MAX_COLOR_ATTACHMENTS],
    pub(crate) depth_format: MTLPixelFormat,
    pub(crate) samples: u32,
}

impl Pass {
    /// The pass as a line of the GPU time table (`VICTORY_METAL_GPUPROF`): size, attachments
    /// (Metal pixel format numbers), draws.
    pub(crate) fn describe(&self, draws: u32) -> String {
        let colors: Vec<String> = self
            .color_formats
            .iter()
            .filter(|f| **f != MTLPixelFormat::Invalid)
            .map(|f| f.0.to_string())
            .collect();
        let loads: Vec<String> = self
            .spec
            .colors
            .iter()
            .flatten()
            .map(|c| format!("{:?}", c.load))
            .collect();
        format!(
            "pass {}x{} colors=[{}] depth={} s{} load=[{}] draws={draws}",
            self.size[0],
            self.size[1],
            colors.join(","),
            self.depth_format.0,
            self.samples,
            loads.join(","),
        )
    }

    fn any_clear(&self) -> bool {
        self.spec
            .colors
            .iter()
            .flatten()
            .any(|c| c.load == Load::Clear)
            || self
                .spec
                .depth
                .as_ref()
                .is_some_and(|d| d.depth_load == Load::Clear || d.stencil_load == Load::Clear)
    }
}

fn view_dim_is_3d(d: ViewDim) -> bool {
    d == ViewDim::D3
}

/// The Metal texture of one attachment and its size at the mip.
type SizedView = (Retained<ProtocolObject<dyn MTLTexture>>, [u32; 2]);

fn target_view(t: &TargetSpec) -> Result<SizedView> {
    let desc = t.texture.desc();
    if t.mip >= desc.mips {
        return Err(Error::BadArgument(format!(
            "render target mip {} of {}",
            t.mip, desc.mips
        )));
    }
    if desc.dim == TexDim::D3 && !view_dim_is_3d(t.dim) {
        return Err(Error::Unsupported(
            "a 2D view of a 3D texture as a render target".into(),
        ));
    }
    let single = matches!(
        t.dim,
        ViewDim::D1 | ViewDim::D2 | ViewDim::D2Ms | ViewDim::D3
    );
    let spec = ViewSpec {
        format: t.format,
        dim: t.dim,
        mip_first: t.mip,
        mip_count: 1,
        slice_first: if view_dim_is_3d(t.dim) {
            0
        } else {
            t.first_slice
        },
        slice_count: if single || view_dim_is_3d(t.dim) {
            if view_dim_is_3d(t.dim) { 0 } else { 1 }
        } else {
            t.slice_count.max(1)
        },
        plane: 0,
        swizzle: crate::texture::SWIZZLE_IDENTITY,
    };
    let (view, _) = t.texture.view_texture(&spec)?;
    let size = desc.mip_size(t.mip);
    Ok((view, [size[0], size[1]]))
}

fn has_stencil(p: MTLPixelFormat) -> bool {
    matches!(
        p,
        MTLPixelFormat::Depth32Float_Stencil8
            | MTLPixelFormat::Depth24Unorm_Stencil8
            | MTLPixelFormat::Stencil8
            | MTLPixelFormat::X32_Stencil8
            | MTLPixelFormat::X24_Stencil8
    )
}

fn has_depth(p: MTLPixelFormat) -> bool {
    matches!(
        p,
        MTLPixelFormat::Depth32Float
            | MTLPixelFormat::Depth16Unorm
            | MTLPixelFormat::Depth32Float_Stencil8
            | MTLPixelFormat::Depth24Unorm_Stencil8
    )
}

impl crate::record::CommandList {
    /// Starts a render pass. Nothing is encoded until a draw or a clear
    /// needs the encoder.
    pub fn begin_pass(&mut self, spec: PassSpec) -> Result<()> {
        self.live()?;
        if self.pass.is_some() {
            return Err(Error::BadArgument(
                "a render pass is already open (end it first)".into(),
            ));
        }
        if spec.colors.len() > MAX_COLOR_ATTACHMENTS {
            return Err(Error::BadArgument("more than 8 colour attachments".into()));
        }
        let mut colors = Vec::new();
        let mut size = [u32::MAX; 2];
        let mut layers = 1;
        let mut samples = 0u32;
        let mut color_formats = [MTLPixelFormat::Invalid; MAX_COLOR_ATTACHMENTS];
        let mut note = |t: &TargetSpec, view_size: [u32; 2]| -> Result<()> {
            let s = t.texture.desc().samples;
            if samples != 0 && samples != s {
                return Err(Error::BadArgument(
                    "attachments with different sample counts".into(),
                ));
            }
            samples = s;
            size = [size[0].min(view_size[0]), size[1].min(view_size[1])];
            if t.slice_count > 1 && !view_dim_is_3d(t.dim) {
                layers = layers.max(t.slice_count);
            }
            Ok(())
        };
        for (i, c) in spec.colors.iter().enumerate() {
            match c {
                Some(c) => {
                    let (v, sz) = target_view(&c.target)?;
                    if has_depth(v.pixelFormat()) || has_stencil(v.pixelFormat()) {
                        return Err(Error::BadArgument(
                            "a depth format as a colour attachment".into(),
                        ));
                    }
                    note(&c.target, sz)?;
                    color_formats[i] = v.pixelFormat();
                    colors.push(Some(v));
                }
                None => colors.push(None),
            }
        }
        let mut depth = None;
        let mut stencil = false;
        let mut depth_format = MTLPixelFormat::Invalid;
        if let Some(d) = &spec.depth {
            let (v, sz) = target_view(&d.target)?;
            if !has_depth(v.pixelFormat()) && !has_stencil(v.pixelFormat()) {
                return Err(Error::BadArgument(
                    "a colour format as the depth-stencil attachment".into(),
                ));
            }
            note(&d.target, sz)?;
            stencil = has_stencil(v.pixelFormat());
            depth_format = v.pixelFormat();
            depth = Some(v);
        }
        if size[0] == u32::MAX {
            size = spec.empty_size.ok_or_else(|| {
                Error::BadArgument("a render pass without attachments or a size".into())
            })?;
        }
        self.pass = Some(Pass {
            spec,
            colors,
            depth,
            has_stencil: stencil,
            size,
            layers,
            opened: false,
            color_formats,
            depth_format,
            samples: samples.max(1),
        });
        Ok(())
    }

    /// Ends the pass: store actions, then the encoder ends. A pass whose
    /// loads are all `Load` and that drew nothing encodes nothing.
    pub fn end_pass(&mut self, stores: &Stores) -> Result<()> {
        self.live()?;
        let Some(pass) = self.pass.as_ref() else {
            return Err(Error::BadArgument("no render pass is open".into()));
        };
        // A clear with no draw after it still has to happen.
        if self.render.is_none() && !pass.opened && pass.any_clear() {
            self.prepare_render(&[FRAGMENT])?;
        }
        if let Some(enc) = self.render.clone() {
            let pass = self.pass.as_ref().expect("checked above");
            for (i, dont_care) in stores.color_dont_care.iter().enumerate() {
                if *dont_care && pass.colors.get(i).is_some_and(|c| c.is_some()) {
                    // SAFETY: the attachment index is below 8 and in use.
                    unsafe { enc.setColorStoreAction_atIndex(MTLStoreAction::DontCare, i) };
                }
            }
            if pass.depth.is_some() {
                if stores.depth_dont_care {
                    enc.setDepthStoreAction(MTLStoreAction::DontCare);
                }
                if pass.has_stencil && stores.stencil_dont_care {
                    enc.setStencilStoreAction(MTLStoreAction::DontCare);
                }
            }
            self.split_encoder()?;
        }
        self.pass = None;
        self.stats.passes += 1;
        Ok(())
    }

    /// Whether a render pass is open.
    pub fn in_pass(&self) -> bool {
        self.pass.is_some()
    }

    /// Creates the render encoder of the open pass (the tracker asked for it).
    pub(crate) fn open_render(&mut self) -> Result<()> {
        let vis_buf = if crate::prof::vis_enabled() {
            crate::prof::vis_buffer(&self.queue().device().clone())
        } else {
            None
        };
        let pass = self
            .pass
            .as_mut()
            .ok_or_else(|| Error::BadArgument("a draw outside a render pass".into()))?;
        let continuing = pass.opened;
        let load = |l: Load| {
            if continuing {
                MTLLoadAction::Load
            } else {
                l.metal()
            }
        };
        let desc = MTL4RenderPassDescriptor::new();
        for (i, v) in pass.colors.iter().enumerate() {
            let (Some(v), Some(Some(c))) = (v, pass.spec.colors.get(i)) else {
                continue;
            };
            // SAFETY: at most 8 attachments (checked at `begin_pass`).
            let a = unsafe { desc.colorAttachments().objectAtIndexedSubscript(i) };
            a.setTexture(Some(v));
            a.setLevel(0);
            a.setSlice(0);
            if view_dim_is_3d(c.target.dim) {
                a.setDepthPlane(c.target.first_slice as usize);
            }
            a.setLoadAction(load(c.load));
            a.setStoreAction(MTLStoreAction::Store);
            a.setClearColor(MTLClearColor {
                red: f64::from(c.clear[0]),
                green: f64::from(c.clear[1]),
                blue: f64::from(c.clear[2]),
                alpha: f64::from(c.clear[3]),
            });
        }
        if let (Some(v), Some(d)) = (&pass.depth, &pass.spec.depth) {
            if has_depth(v.pixelFormat()) {
                let a = desc.depthAttachment();
                a.setTexture(Some(v));
                a.setLevel(0);
                a.setSlice(0);
                a.setLoadAction(load(d.depth_load));
                a.setStoreAction(MTLStoreAction::Store);
                a.setClearDepth(f64::from(d.clear_depth));
            }
            if pass.has_stencil {
                let a = desc.stencilAttachment();
                a.setTexture(Some(v));
                a.setLevel(0);
                a.setSlice(0);
                a.setLoadAction(load(d.stencil_load));
                a.setStoreAction(MTLStoreAction::Store);
                a.setClearStencil(d.clear_stencil);
            }
        }
        if pass.layers > 1 {
            desc.setRenderTargetArrayLength(pass.layers as usize);
        }
        if pass.spec.empty_size.is_some() {
            desc.setRenderTargetWidth(pass.size[0] as usize);
            desc.setRenderTargetHeight(pass.size[1] as usize);
            // SAFETY: a plain count, 1 is always valid.
            unsafe { desc.setDefaultRasterSampleCount(1) };
        }
        if let Some(b) = vis_buf {
            // SAFETY: a shared buffer of 4096 bytes, kept for the process.
            unsafe { desc.setVisibilityResultBuffer(Some(b.raw())) };
        }
        pass.opened = true;
        let encoder = self
            .cb()
            .renderCommandEncoderWithDescriptor(&desc)
            .ok_or_else(|| Error::Metal("renderCommandEncoder returned nil".into()))?;
        encoder.setArgumentTable_atStages(
            self.table.raw(),
            MTLRenderStages::Vertex
                | MTLRenderStages::Fragment
                | MTLRenderStages::Object
                | MTLRenderStages::Mesh,
        );
        self.render = Some(encoder);
        self.rstate.dirty = DIRTY_ALL;
        self.rstate.pipeline = None;
        self.stats.render_encoders += 1;
        Ok(())
    }

    /// Makes the render encoder current for commands of these stages.
    pub(crate) fn prepare_render(&mut self, stages: &[u16]) -> Result<RenderEncoder> {
        for &stage in stages {
            let actions = self
                .tracker
                .command(stage)
                .ok_or_else(|| Error::BadArgument(format!("no encoder for stage {stage:#x}")))?;
            self.perform(actions)?;
        }
        self.render
            .clone()
            .ok_or_else(|| Error::Metal("no render encoder".into()))
    }

    pub fn set_viewports(&mut self, v: &[ViewportRect]) -> Result<()> {
        self.live()?;
        if v.len() > 16 {
            return Err(Error::BadArgument("more than 16 viewports".into()));
        }
        self.rstate.viewports = v
            .iter()
            .map(|v| MTLViewport {
                originX: f64::from(v.x),
                originY: f64::from(v.y),
                width: f64::from(v.width),
                height: f64::from(v.height),
                znear: f64::from(v.min_depth),
                zfar: f64::from(v.max_depth),
            })
            .collect();
        self.rstate.dirty |= DIRTY_VIEWPORT;
        Ok(())
    }

    /// Scissor rectangles as edges `[left, top, right, bottom]`.
    pub fn set_scissors(&mut self, r: &[[i32; 4]]) -> Result<()> {
        self.live()?;
        if r.len() > 16 {
            return Err(Error::BadArgument("more than 16 scissor rectangles".into()));
        }
        self.rstate.scissors = r.to_vec();
        self.rstate.dirty |= DIRTY_SCISSOR;
        Ok(())
    }

    pub fn set_blend_factor(&mut self, color: [f32; 4]) -> Result<()> {
        self.live()?;
        self.rstate.blend = color;
        self.rstate.dirty |= DIRTY_BLEND;
        Ok(())
    }

    pub fn set_stencil_ref(&mut self, value: u32) -> Result<()> {
        self.live()?;
        self.rstate.stencil_ref = value;
        self.rstate.dirty |= DIRTY_STENCIL;
        Ok(())
    }

    pub fn set_primitive(&mut self, p: Primitive) -> Result<()> {
        self.live()?;
        self.rstate.primitive = Some(p);
        Ok(())
    }

    /// The index buffer: GPU address, size in bytes, 32-bit indices or 16-bit.
    pub fn set_index_buffer(&mut self, va: u64, size: u32, wide: bool) -> Result<()> {
        self.live()?;
        self.rstate.index = Some(IndexBuffer { va, size, wide });
        Ok(())
    }

    /// Vertex buffers `start..start + views.len()` as `(gpu address, size, stride)`.
    pub fn set_vertex_buffers(&mut self, start: u32, views: &[(u64, u32, u32)]) -> Result<()> {
        self.live()?;
        if start as usize + views.len() > MAX_VERTEX_BUFFERS {
            return Err(Error::BadArgument("vertex buffer slot past 15".into()));
        }
        for (i, &(va, size, stride)) in views.iter().enumerate() {
            self.rstate.vertex[start as usize + i] =
                (va != 0).then_some(VertexBuffer { va, stride, size });
        }
        Ok(())
    }

    /// Everything a draw needs before the call: the encoder, the pipeline
    /// and its state, the shadowed state that changed.
    fn begin_draw(&mut self, p: &GraphicsPipeline, stages: &[u16]) -> Result<RenderEncoder> {
        self.live()?;
        let pass = self
            .pass
            .as_ref()
            .ok_or_else(|| Error::BadArgument("a draw outside a render pass".into()))?;
        if pass.samples != p.sample_count() {
            return Err(Error::BadArgument(format!(
                "a pipeline of {} samples in a pass of {}",
                p.sample_count(),
                pass.samples
            )));
        }
        let enc = self.prepare_render(stages)?;
        let key = p.identity();
        if self.rstate.pipeline != Some(key) {
            enc.setRenderPipelineState(p.raw());
            enc.setDepthStencilState(Some(p.depth_stencil_state()));
            let r = p.raster();
            enc.setCullMode(r.cull);
            enc.setTriangleFillMode(r.fill);
            enc.setFrontFacingWinding(r.front_winding);
            enc.setDepthClipMode(r.depth_clip);
            enc.setDepthBias_slopeScale_clamp(r.depth_bias, r.slope_scale, r.bias_clamp);
            self.rstate.pipeline = Some(key);
            if !self.keep_gfx.iter().any(|k| k.identity() == key) {
                self.keep_gfx.push(p.clone());
            }
        }
        let size = self.pass.as_ref().map(|p| p.size).unwrap_or([0, 0]);
        let st = &mut self.rstate;
        if st.dirty & DIRTY_VIEWPORT != 0 && !st.viewports.is_empty() {
            // SAFETY: at most 16 viewports, the pointer is to `count` of them.
            unsafe {
                enc.setViewports_count(
                    std::ptr::NonNull::from(&mut st.viewports[0]),
                    st.viewports.len(),
                )
            };
        }
        if st.dirty & DIRTY_SCISSOR != 0 && !st.scissors.is_empty() {
            let mut rects: Vec<MTLScissorRect> = st
                .scissors
                .iter()
                .map(|r| {
                    let x = r[0].clamp(0, size[0] as i32) as usize;
                    let y = r[1].clamp(0, size[1] as i32) as usize;
                    let right = r[2].clamp(0, size[0] as i32) as usize;
                    let bottom = r[3].clamp(0, size[1] as i32) as usize;
                    MTLScissorRect {
                        x,
                        y,
                        width: right.saturating_sub(x),
                        height: bottom.saturating_sub(y),
                    }
                })
                .collect();
            // SAFETY: at most 16 rectangles, the pointer is to `count` of them.
            unsafe {
                enc.setScissorRects_count(std::ptr::NonNull::from(&mut rects[0]), rects.len())
            };
        }
        if st.dirty & DIRTY_BLEND != 0 {
            let b = st.blend;
            enc.setBlendColorRed_green_blue_alpha(b[0], b[1], b[2], b[3]);
        }
        if st.dirty & DIRTY_STENCIL != 0 {
            enc.setStencilReferenceValue(st.stencil_ref);
        }
        st.dirty = 0;
        Ok(enc)
    }

    /// The vertex buffers the pipeline reads, into the argument table.
    fn bind_vertex_buffers(&self, p: &GraphicsPipeline) -> Result<()> {
        for &slot in p.vertex_slots() {
            let vb = self
                .rstate
                .vertex
                .get(slot as usize)
                .copied()
                .flatten()
                .ok_or_else(|| {
                    Error::BadArgument(format!(
                        "the pipeline reads vertex buffer {slot}, none is set"
                    ))
                })?;
            self.table.set_address_stride(
                (bind_point::VERTEX_BUFFER + u64::from(slot)) as u32,
                vb.va,
                vb.stride,
            )?;
        }
        Ok(())
    }

    /// The converter's draw arguments (`IRRuntimeDrawParams`) and the index
    /// type, in the ring, for the vertex shader.
    fn write_draw_params(&mut self, words: [u32; 5], index_type: u16) -> Result<()> {
        let block: RingAlloc = self.alloc_constants(DRAW_BLOCK, 16)?;
        let mut bytes = [0u8; 20];
        for (i, w) in words.iter().enumerate() {
            bytes[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
        }
        block.write_bytes(0, &bytes)?;
        block.write_bytes(UNIFORMS_AT, &index_type.to_le_bytes())?;
        self.table
            .set_address(bind_point::DRAW_ARGUMENTS as u32, block.gpu_va)?;
        self.table
            .set_address(bind_point::UNIFORMS as u32, block.gpu_va + UNIFORMS_AT)
    }

    fn vertex_pipeline_checks(&self, p: &GraphicsPipeline) -> Result<Primitive> {
        if p.kind() == Kind::Mesh {
            return Err(Error::BadArgument("a draw with a mesh pipeline".into()));
        }
        let prim = self
            .rstate
            .primitive
            .ok_or_else(|| Error::BadArgument("a draw without a primitive topology".into()))?;
        if p.kind() == Kind::Geometry {
            // The geometry shader declares the primitive it reads.
            let want = p.geometry().map_or(0, |g| g.input_primitive);
            if want != prim.geometry_input() {
                return Err(Error::BadArgument(format!(
                    "topology {prim:?} with a geometry shader that reads primitive kind {want}"
                )));
            }
            return Ok(prim);
        }
        if prim.class() != p.topology_class() {
            return Err(Error::BadArgument(format!(
                "topology {prim:?} with a pipeline made for {:?} primitives",
                p.topology_class()
            )));
        }
        Ok(prim)
    }

    /// `DrawInstanced`.
    pub fn draw(&mut self, p: &GraphicsPipeline, a: DrawArgs) -> Result<()> {
        let prim = self.vertex_pipeline_checks(p)?;
        if a.vertex_count == 0 || a.instance_count == 0 {
            return Ok(());
        }
        if p.kind() == Kind::Geometry {
            return self.draw_geometry(
                p,
                prim,
                None,
                [
                    a.vertex_count,
                    a.instance_count,
                    a.start_vertex,
                    a.start_instance,
                    0,
                ],
            );
        }
        self.bind_vertex_buffers(p)?;
        // `kIRNonIndexedDraw` is 0 in the converter's runtime.
        self.write_draw_params(
            [
                a.vertex_count,
                a.instance_count,
                a.start_vertex,
                a.start_instance,
                0,
            ],
            0,
        )?;
        let enc = self.begin_draw(p, &[VERTEX, FRAGMENT])?;
        // SAFETY: arguments are plain counts; Metal checks the state.
        unsafe {
            enc.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                prim.metal(),
                a.start_vertex as usize,
                a.vertex_count as usize,
                a.instance_count as usize,
                a.start_instance as usize,
            );
        }
        self.stats.draws += 1;
        Ok(())
    }

    /// A non-indexed triangle-list draw of the mesh emulation's vertex-fetch
    /// pipeline (`p` reads no vertex buffer; the topology the stream set, a
    /// mesh pipeline has none, does not apply).
    pub(crate) fn draw_fetch(
        &mut self,
        p: &GraphicsPipeline,
        vertex_count: u32,
        instance_count: u32,
    ) -> Result<()> {
        if p.kind() != Kind::Vertex {
            return Err(Error::BadArgument(
                "a fetch draw with a pipeline that is not a vertex pipeline".into(),
            ));
        }
        if vertex_count == 0 || instance_count == 0 {
            return Ok(());
        }
        self.write_draw_params([vertex_count, instance_count, 0, 0, 0], 0)?;
        let enc = self.begin_draw(p, &[VERTEX, FRAGMENT])?;
        // SAFETY: arguments are plain counts; Metal checks the state.
        unsafe {
            enc.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                MTLPrimitiveType::Triangle,
                0,
                vertex_count as usize,
                instance_count as usize,
                0,
            );
        }
        self.stats.draws += 1;
        Ok(())
    }

    /// [`draw_fetch`](Self::draw_fetch) with the counts a kernel wrote: a
    /// 64-byte block at `args_va` (16-byte aligned) whose first four words
    /// are Metal's indirect arguments (vertex count, instance count, start
    /// vertex, start instance) and which the converter's runtime reads as its
    /// draw parameters (the index type, at +32, is zero).
    pub(crate) fn draw_fetch_indirect(&mut self, p: &GraphicsPipeline, args_va: u64) -> Result<()> {
        if p.kind() != Kind::Vertex {
            return Err(Error::BadArgument(
                "a fetch draw with a pipeline that is not a vertex pipeline".into(),
            ));
        }
        self.table
            .set_address(bind_point::DRAW_ARGUMENTS as u32, args_va)?;
        self.table
            .set_address(bind_point::UNIFORMS as u32, args_va + UNIFORMS_AT)?;
        let enc = self.begin_draw(p, &[VERTEX, FRAGMENT])?;
        enc.drawPrimitives_indirectBuffer(MTLPrimitiveType::Triangle, args_va);
        self.stats.draws += 1;
        Ok(())
    }

    /// `DrawIndexedInstanced`.
    pub fn draw_indexed(&mut self, p: &GraphicsPipeline, a: DrawIndexedArgs) -> Result<()> {
        let prim = self.vertex_pipeline_checks(p)?;
        let ib = self
            .rstate
            .index
            .ok_or_else(|| Error::BadArgument("an indexed draw without an index buffer".into()))?;
        if a.index_count == 0 || a.instance_count == 0 {
            return Ok(());
        }
        let width = if ib.wide { 4u64 } else { 2 };
        let start = u64::from(a.start_index) * width;
        if start + u64::from(a.index_count) * width > u64::from(ib.size) {
            return Err(Error::BadArgument(format!(
                "indices {}+{} outside an index buffer of {} bytes",
                a.start_index, a.index_count, ib.size
            )));
        }
        if p.kind() == Kind::Geometry {
            return self.draw_geometry(
                p,
                prim,
                // The emulation's object stage ignores the start index:
                // it is folded into the index buffer address.
                Some((
                    ib.va + u64::from(a.start_index) * if ib.wide { 4 } else { 2 },
                    ib.wide,
                )),
                [
                    a.index_count,
                    a.instance_count,
                    0,
                    a.base_vertex as u32,
                    a.start_instance,
                ],
            );
        }
        self.bind_vertex_buffers(p)?;
        // The runtime's `IRMetalIndexToIRIndex`: Metal's index type + 1.
        let (metal_index, ir_index) = if ib.wide {
            (MTLIndexType::UInt32, 2u16)
        } else {
            (MTLIndexType::UInt16, 1u16)
        };
        self.write_draw_params(
            [
                a.index_count,
                a.instance_count,
                a.start_index,
                a.base_vertex as u32,
                a.start_instance,
            ],
            ir_index,
        )?;
        let enc = self.begin_draw(p, &[VERTEX, FRAGMENT])?;
        // SAFETY: the address is inside a live buffer (checked by the
        // interpreter against the address map), aligned to the index size.
        unsafe {
            enc.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferLength_instanceCount_baseVertex_baseInstance(
                prim.metal(),
                a.index_count as usize,
                metal_index,
                ib.va + start,
                (u64::from(ib.size) - start) as usize,
                a.instance_count as usize,
                a.base_vertex as isize,
                a.start_instance as usize,
            );
        }
        self.stats.draws += 1;
        Ok(())
    }

    /// A draw of a geometry pipeline, the way the converter's runtime does it
    /// (`IRRuntime4Draw[Indexed]PrimitivesGeometryEmulation`): a mesh draw
    /// whose object threadgroups each take a run of vertices (the vertex
    /// shader), whose mesh threadgroups each take some of the primitives
    /// those make (the geometry shader). `words` are the draw arguments as
    /// `IRRuntimeDrawParams` has them (count, instances, start, base vertex,
    /// start instance); `index` is the index buffer address and width of an
    /// indexed draw. The vertex buffers go in one `IRRuntimeVertexBuffers`
    /// block.
    fn draw_geometry(
        &mut self,
        p: &GraphicsPipeline,
        prim: Primitive,
        index: Option<(u64, bool)>,
        words: [u32; 5],
    ) -> Result<()> {
        let count = words[0];
        let plan = geometry_plan(p, prim, index.map(|(_, wide)| wide), false)?;
        if count < plan.verts {
            // Fewer vertices than one primitive.
            return Ok(());
        }
        let object_groups = (count - plan.non_provoking).div_ceil(plan.stride);
        let mut info = plan.info;
        info[10..12].copy_from_slice(&(words[1] as u16).to_le_bytes());
        info[16..24].copy_from_slice(&index.map_or(0, |(va, _)| va).to_le_bytes());

        self.bind_geometry_vertex_buffers()?;

        let block: RingAlloc = self.alloc_constants(DRAW_BLOCK, 16)?;
        let mut bytes = [0u8; 20];
        for (i, w) in words.iter().enumerate() {
            bytes[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
        }
        block.write_bytes(0, &bytes)?;
        block.write_bytes(UNIFORMS_AT, &info)?;
        self.table
            .set_address(bind_point::DRAW_ARGUMENTS as u32, block.gpu_va)?;
        self.table
            .set_address(bind_point::UNIFORMS as u32, block.gpu_va + UNIFORMS_AT)?;

        let enc = self.begin_draw(p, &[OBJECT, MESH, FRAGMENT])?;
        let size = |w: u32, h: u32| MTLSize {
            width: w as usize,
            height: h as usize,
            depth: 1,
        };
        enc.drawMeshThreadgroups_threadsPerObjectThreadgroup_threadsPerMeshThreadgroup(
            size(object_groups, words[1]),
            size(plan.object_threads, 1),
            size(plan.mesh_threads, 1),
        );
        self.stats.draws += 1;
        Ok(())
    }

    /// The vertex buffers of a geometry pipeline as one
    /// `IRRuntimeVertexBuffers` block (31 of { address, length, stride }), at
    /// the vertex buffer bind point.
    /// `VICTORY_METAL_GS_VISIBILITY`: copies the first words of the bound vertex and index
    /// buffers where the profiler prints them, and notes the state's viewports and scissors.
    fn dump_gs_inputs(&mut self, tlab_va: Option<u64>) -> Result<()> {
        let dev = self.queue().device().clone();
        let (Some(k), Some(buf)) = (
            crate::prof::dump_kernel(&dev),
            crate::prof::dump_buffer(&dev),
        ) else {
            return Ok(());
        };
        let mut meta = String::new();
        for (i, v) in self.rstate.viewports.iter().enumerate() {
            meta.push_str(&format!(
                "\n  viewport {i}: x {} y {} w {} h {} z {}..{}",
                v.originX, v.originY, v.width, v.height, v.znear, v.zfar
            ));
        }
        for (i, s) in self.rstate.scissors.iter().enumerate() {
            meta.push_str(&format!("\n  scissor {i}: {s:?}"));
        }
        let mut regions: Vec<(u64, u32)> = Vec::new();
        for (slot, vb) in self.rstate.vertex.iter().enumerate().take(4) {
            match vb {
                Some(vb) => {
                    meta.push_str(&format!(
                        "\n  vertex slot {slot}: va {:#x} size {} stride {}",
                        vb.va, vb.size, vb.stride
                    ));
                    regions.push((vb.va, vb.size.min(64) / 4));
                }
                None => regions.push((0, 0)),
            }
        }
        while regions.len() < 4 {
            regions.push((0, 0));
        }
        match self.rstate.index {
            Some(ib) => {
                meta.push_str(&format!(
                    "\n  index buffer: va {:#x} size {} wide {}",
                    ib.va, ib.size, ib.wide
                ));
                regions.push((ib.va, ib.size.min(64) / 4));
            }
            None => regions.push((0, 0)),
        }
        *crate::prof::DUMP_META
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = meta;
        self.barrier(
            crate::stages::DISPATCH,
            crate::stages::DISPATCH,
            crate::stages::VIS_DEVICE,
        )?;
        for (k_i, (va, words)) in regions.into_iter().enumerate() {
            if words == 0 {
                continue;
            }
            let params = self.alloc_constants(32, 16)?;
            let dst = buf.gpu_va() + (k_i as u64) * 64;
            params.write_u32s(
                0,
                &[
                    va as u32,
                    (va >> 32) as u32,
                    dst as u32,
                    (dst >> 32) as u32,
                    words,
                    0,
                    0,
                    0,
                ],
            )?;
            self.set_address(30, params.gpu_va)?;
            self.dispatch(k, [1, 1, 1], [1, 1, 1])?;
        }
        let args_va = tlab_va.unwrap_or_else(|| {
            crate::prof::LAST_ARGS_VA.load(std::sync::atomic::Ordering::Relaxed)
        });
        meta_push(&format!(
            "\n  root arguments va {args_va:#x} (record's: {})",
            tlab_va.is_some()
        ));
        if let (Some(t), Some(k2)) = (
            Some(args_va).filter(|v| *v != 0),
            crate::prof::dump2_kernel(&dev),
        ) {
            let heap = crate::prof::LAST_HEAP_VA.load(std::sync::atomic::Ordering::Relaxed);
            let params = self.alloc_constants(32, 16)?;
            let dst = buf.gpu_va() + 5 * 64;
            params.write_u32s(
                0,
                &[
                    t as u32,
                    (t >> 32) as u32,
                    heap as u32,
                    (heap >> 32) as u32,
                    dst as u32,
                    (dst >> 32) as u32,
                    0,
                    0,
                ],
            )?;
            self.set_address(30, params.gpu_va)?;
            self.dispatch(k2, [1, 1, 1], [1, 1, 1])?;
        }
        self.keep_buffers.push(buf.clone());
        Ok(())
    }

    fn bind_geometry_vertex_buffers(&mut self) -> Result<()> {
        let vb_block: RingAlloc = self.alloc_constants(31 * 16, 16)?;
        let mut vbs = [0u8; 31 * 16];
        for (slot, vb) in self.rstate.vertex.iter().enumerate().take(31) {
            if let Some(vb) = vb {
                vbs[slot * 16..slot * 16 + 8].copy_from_slice(&vb.va.to_le_bytes());
                vbs[slot * 16 + 8..slot * 16 + 12].copy_from_slice(&vb.size.to_le_bytes());
                vbs[slot * 16 + 12..slot * 16 + 16].copy_from_slice(&vb.stride.to_le_bytes());
            }
        }
        vb_block.write_bytes(0, &vbs)?;
        self.table
            .set_address(bind_point::VERTEX_BUFFER as u32, vb_block.gpu_va)
    }

    /// `DispatchMesh`: `groups` are object threadgroups when the pipeline
    /// has an amplification stage, mesh threadgroups otherwise.
    pub fn dispatch_mesh(&mut self, p: &GraphicsPipeline, groups: [u32; 3]) -> Result<()> {
        if p.kind() != Kind::Mesh {
            return Err(Error::BadArgument(
                "DispatchMesh with a vertex pipeline".into(),
            ));
        }
        let info = p
            .mesh()
            .ok_or_else(|| Error::BadArgument("a mesh pipeline without mesh facts".into()))?;
        if groups.contains(&0) {
            return Ok(());
        }
        let stages: &[u16] = if info.object_threads.is_some() {
            &[OBJECT, MESH, FRAGMENT]
        } else {
            &[MESH, FRAGMENT]
        };
        let enc = self.begin_draw(p, stages)?;
        let size = |t: [u32; 3]| MTLSize {
            width: t[0] as usize,
            height: t[1] as usize,
            depth: t[2] as usize,
        };
        enc.drawMeshThreadgroups_threadsPerObjectThreadgroup_threadsPerMeshThreadgroup(
            size(groups),
            size(info.object_threads.unwrap_or([1, 1, 1])),
            size(info.mesh_threads),
        );
        self.stats.mesh_draws += 1;
        Ok(())
    }

    /// The converter runtime's index type of the bound index buffer
    /// (`IRMetalIndexToIRIndex`: Metal's index type + 1), 0 without one.
    pub(crate) fn index_ir_type(&self) -> u32 {
        match self.rstate.index {
            Some(ib) if ib.wide => 2,
            Some(_) => 1,
            None => 0,
        }
    }

    /// `ExecuteIndirect` of draws through a geometry shader.
    ///
    /// The emulation is a mesh draw whose threadgroup counts and
    /// `IRRuntimeDrawInfo` depend on the record's vertex and instance counts,
    /// which the GPU reads. The expansion kernel works them out per record
    /// ([`GsExpand`]) and each record is one
    /// `drawMeshThreadgroupsWithIndirectBuffer`. The converter runtime's header
    /// (4.0.3) has no indirect form of the emulation draw, nor does its manual.
    ///
    /// Memory: nothing but the expansion, 64 bytes per record from the
    /// constant ring in chunks of at most 256 records (so the ring bound holds
    /// for any `MaxCommandCount`, itself at most 65 536), and the vertex buffer
    /// block, 496 bytes; a record's counts past the count buffer, or an
    /// indexed record's past the end of the index buffer, are clamped by the
    /// kernel. The emulation's payload lives in the stage, not in a buffer.
    /// Everything the draw reads (the argument and count buffers, the
    /// bound vertex and index buffers) is in the residency set like any other
    /// resource of the list.
    fn execute_indirect_geometry(
        &mut self,
        kernels: &Kernels,
        p: &GraphicsPipeline,
        kind: IndirectDraw,
        spec: &IndirectDispatch<'_>,
    ) -> Result<()> {
        // Diagnostic (docs/metal/m5-gs.md section 1): with
        // `VICTORY_METAL_SKIP_GS_INDIRECT=1` these draws are not drawn, to tell
        // what they do to a picture from what the rest of a frame does.
        if std::env::var_os("VICTORY_METAL_SKIP_GS_INDIRECT").is_some_and(|v| v != "0") {
            crate::dropstats::skip("indirect GS draw skipped (VICTORY_METAL_SKIP_GS_INDIRECT)");
            return Ok(());
        }
        let prim = self.vertex_pipeline_checks(p)?;
        let ib = self.rstate.index;
        let indexed = kind == IndirectDraw::DrawIndexed;
        let ib = match (indexed, ib) {
            (true, None) => {
                return Err(Error::BadArgument(
                    "an indexed draw without an index buffer".into(),
                ));
            }
            (true, ib) => ib,
            (false, _) => None,
        };
        let plan = geometry_plan(p, prim, ib.map(|ib| ib.wide), true)?;
        let width = ib.map_or(0, |ib| if ib.wide { 4u32 } else { 2 });
        let gs = GsExpand {
            indexed,
            ib_va: ib.map_or(0, |ib| ib.va),
            ib_count: ib.map_or(0, |ib| ib.size / width),
            ib_width: width,
            stride: plan.stride,
            verts: plan.verts,
            non_provoking: plan.non_provoking,
            info: plan.info,
        };
        let spec = IndirectDispatch {
            gs: Some(gs),
            draws: true,
            ..*spec
        };
        self.bind_geometry_vertex_buffers()?;
        let size = |w: u32| MTLSize {
            width: w as usize,
            height: 1,
            depth: 1,
        };
        self.execute_indirect_with(kernels, &spec, |l, r| {
            if let Some(t) = r.tlab_va {
                l.set_root_arguments_va(t)?;
            }
            l.table
                .set_address(bind_point::DRAW_ARGUMENTS as u32, r.action_va)?;
            l.table
                .set_address(bind_point::UNIFORMS as u32, r.action_va + UNIFORMS_AT)?;
            l.cap_gs_groups(r.action_va)?;
            l.snap_gs_record(r.action_va)?;
            if crate::prof::GS_VIS.with(|c| c.get()) {
                l.dump_gs_inputs(r.tlab_va)?;
            }
            let enc = l.begin_draw(p, &[OBJECT, MESH, FRAGMENT])?;
            let vis = crate::prof::GS_VIS.with(|c| c.get());
            if vis {
                // SAFETY: slot 0 of the visibility buffer the pass was made with.
                unsafe { enc.setVisibilityResultMode_offset(MTLVisibilityResultMode::Counting, 0) };
            }
            enc.drawMeshThreadgroupsWithIndirectBuffer_threadsPerObjectThreadgroup_threadsPerMeshThreadgroup(
                r.action_va + GS_GROUPS_AT,
                size(plan.object_threads),
                size(plan.mesh_threads),
            );
            if vis {
                // SAFETY: as above.
                unsafe { enc.setVisibilityResultMode_offset(MTLVisibilityResultMode::Disabled, 0) };
            }
            l.stats.draws += 1;
            Ok(())
        })
    }

    /// `ExecuteIndirect` of draws or mesh dispatches (`kind`), the records
    /// described by `spec` (see [`crate::indirect`]).
    ///
    /// A mesh dispatch with no count buffer and no root arguments reads the
    /// application's records where they are. Everything else goes through
    /// the expansion kernel: it clamps the records to the count, copies the
    /// root arguments each record changes, and for a vertex draw writes the
    /// converter's draw parameters (`IRRuntimeDrawParams` and the index
    /// type), which a shader reads and an indirect draw has no way to give.
    /// Then one indirect draw per record. Vertex and index buffer changes in
    /// a record are not done (the argument table's vertex buffer addresses
    /// are written by the CPU, and Metal's indirect indexed draw takes the
    /// index buffer when it is encoded): the interpreter refuses those.
    pub fn execute_indirect_draws(
        &mut self,
        kernels: &Kernels,
        p: &GraphicsPipeline,
        kind: IndirectDraw,
        spec: &IndirectDispatch<'_>,
    ) -> Result<()> {
        self.live()?;
        let mesh = p.kind() == Kind::Mesh;
        match kind {
            IndirectDraw::Mesh if !mesh => {
                return Err(Error::BadArgument(
                    "an indirect mesh dispatch with a vertex pipeline".into(),
                ));
            }
            IndirectDraw::Draw | IndirectDraw::DrawIndexed if p.kind() == Kind::Geometry => {
                return self.execute_indirect_geometry(kernels, p, kind, spec);
            }
            _ => {}
        }
        let size = |t: [u32; 3]| MTLSize {
            width: t[0] as usize,
            height: t[1] as usize,
            depth: t[2] as usize,
        };
        match kind {
            IndirectDraw::Mesh => {
                let info = p.mesh().ok_or_else(|| {
                    Error::BadArgument("a mesh pipeline without mesh facts".into())
                })?;
                let stages: &[u16] = if info.object_threads.is_some() {
                    &[OBJECT, MESH, FRAGMENT]
                } else {
                    &[MESH, FRAGMENT]
                };
                let (object, mesh_threads) =
                    (info.object_threads.unwrap_or([1, 1, 1]), info.mesh_threads);
                self.execute_indirect_with(kernels, spec, |l, r| {
                    if let Some(t) = r.tlab_va {
                        l.set_root_arguments_va(t)?;
                    }
                    let enc = l.begin_draw(p, stages)?;
                    enc.drawMeshThreadgroupsWithIndirectBuffer_threadsPerObjectThreadgroup_threadsPerMeshThreadgroup(
                        r.action_va,
                        size(object),
                        size(mesh_threads),
                    );
                    l.stats.mesh_draws += 1;
                    Ok(())
                })
            }
            IndirectDraw::Draw | IndirectDraw::DrawIndexed => {
                let prim = self.vertex_pipeline_checks(p)?;
                let ib = self.rstate.index;
                if kind == IndirectDraw::DrawIndexed && ib.is_none() {
                    return Err(Error::BadArgument(
                        "an indexed draw without an index buffer".into(),
                    ));
                }
                self.bind_vertex_buffers(p)?;
                self.execute_indirect_with(kernels, spec, |l, r| {
                    if let Some(t) = r.tlab_va {
                        l.set_root_arguments_va(t)?;
                    }
                    l.table
                        .set_address(bind_point::DRAW_ARGUMENTS as u32, r.action_va)?;
                    l.table
                        .set_address(bind_point::UNIFORMS as u32, r.action_va + UNIFORMS_AT)?;
                    let enc = l.begin_draw(p, &[VERTEX, FRAGMENT])?;
                    match ib {
                        Some(ib) if kind == IndirectDraw::DrawIndexed => {
                            let ty = if ib.wide {
                                MTLIndexType::UInt32
                            } else {
                                MTLIndexType::UInt16
                            };
                            // SAFETY: the address is a live buffer's; Metal
                            // reads the records the expansion wrote.
                            unsafe {
                                enc.drawIndexedPrimitives_indexType_indexBuffer_indexBufferLength_indirectBuffer(
                                    prim.metal(),
                                    ty,
                                    ib.va,
                                    ib.size as usize,
                                    r.action_va,
                                );
                            }
                        }
                        _ => enc.drawPrimitives_indirectBuffer(prim.metal(), r.action_va),
                    }
                    l.stats.draws += 1;
                    Ok(())
                })
            }
        }
    }
}

/// How a geometry-shader draw is cut into threadgroups, the same for a direct
/// draw and for each record of an indirect one: the converter runtime's
/// `IRRuntimeCalculateDrawInfoForGSEmulation` and the threadgroup counts of
/// `IRRuntimeCalculateObjectTgCountForTessellationAndGeometryEmulation`, with
/// the vertex count left open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GeometryPlan {
    /// Vertices one primitive reads.
    pub verts: u32,
    /// Vertices one object threadgroup takes.
    pub stride: u32,
    /// Vertices at the end of a strip that start no primitive.
    pub non_provoking: u32,
    /// Threads of an object and of a mesh threadgroup.
    pub object_threads: u32,
    pub mesh_threads: u32,
    /// `IRRuntimeDrawInfo` with the instance count and the index buffer
    /// address zero (the draw fills them).
    pub info: [u8; 24],
}

/// `VICTORY_METAL_LOG=1`: each distinct kind of geometry-shader draw, once
/// (what the title does: topology, indexed or not, direct or indirect, the
/// vertex size and the primitives per mesh threadgroup).
fn note_geometry(how: &str, prim: Primitive, indexed: bool, g: &GeometryPipelineInfo) {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    if !std::env::var_os("VICTORY_METAL_LOG").is_some_and(|v| v != "0") {
        return;
    }
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let line = format!(
        "{how} {prim:?} {} vertex {} bytes, {} primitives per mesh threadgroup",
        if indexed { "indexed" } else { "non-indexed" },
        g.vertex_bytes,
        g.max_input_primitives
    );
    let mut seen = SEEN
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if seen.insert(line.clone()) {
        eprintln!("victory-metal back: geometry draw: {line}");
    }
}

/// The cut for pipeline `p` reading `prim`; `index` is `Some(wide)` for an
/// indexed draw.
pub(crate) fn geometry_plan(
    p: &GraphicsPipeline,
    prim: Primitive,
    index: Option<bool>,
    how_indirect: bool,
) -> Result<GeometryPlan> {
    let g = p
        .geometry()
        .ok_or_else(|| Error::BadArgument("a geometry pipeline without geometry facts".into()))?;
    let verts = prim.vertex_count();
    let overlap = prim.strip_overlap();
    note_geometry(
        if how_indirect { "indirect" } else { "direct" },
        prim,
        index.is_some(),
        &g,
    );
    // The runtime's `IRRuntimeCalculateDrawInfoForGSEmulation`.
    const PAYLOAD_TOTAL: u32 = 16384;
    const PAYLOAD_METADATA: u32 = 32;
    // A strip's object threadgroup also holds the `overlap` vertices that start
    // the next group's primitives: they take payload space and threads too. The
    // runtime's own sizes (`256 / verts` primitives) would need 257 threads for
    // a triangle strip, one over the 256 the pipeline allows, and the draw
    // would run nothing.
    let by_payload = ((((PAYLOAD_TOTAL - PAYLOAD_METADATA) / g.vertex_bytes.max(1))
        .saturating_sub(overlap))
        / verts)
        * verts;
    let by_amplification = 1024 * g.max_input_primitives;
    let prims_per_object = (by_payload / verts)
        .min(by_amplification)
        .min((256 - overlap) / verts);
    if prims_per_object == 0 {
        return Err(Error::BadArgument(format!(
            "a geometry pipeline whose vertex is {} bytes does not fit the payload",
            g.vertex_bytes
        )));
    }
    let stride = prims_per_object * verts;
    let mesh_threads = g.max_input_primitives;
    let ir_index: u16 = match index {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    };
    let mut info = [0u8; 24];
    info[0..2].copy_from_slice(&ir_index.to_le_bytes());
    info[2] = prim.ir_type();
    info[3] = verts as u8;
    info[4..6].copy_from_slice(&(mesh_threads as u16).to_le_bytes());
    info[6..8].copy_from_slice(&(stride as u16).to_le_bytes());
    info[8..10].copy_from_slice(&(mesh_threads as u16).to_le_bytes());
    info[12..14].copy_from_slice(&(prims_per_object as u16).to_le_bytes());
    info[14..16].copy_from_slice(&(verts as u16).to_le_bytes());
    Ok(GeometryPlan {
        verts,
        stride,
        non_provoking: if prim.is_strip() { overlap } else { 0 },
        object_threads: stride + overlap,
        mesh_threads,
        info,
    })
}

/// What the action of an indirect record is, for [`CommandList::execute_indirect_draws`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndirectDraw {
    Draw,
    DrawIndexed,
    Mesh,
}

// ---------------------------------------------------------------------------
// Clearing part of an attachment inside a pass (`CLEAR_ATTACHMENT`).
//
// A load action clears a whole attachment when a pass starts; the original
// API's `ClearRenderTargetView` with rectangles, or a clear after the target
// was drawn to, needs a clear in the middle. Metal has no such call, so a
// triangle covering the rectangle is drawn with a small fixed pipeline: colour
// goes to one attachment (the others have an empty write mask), depth is the
// vertex's z with the test `Always`, stencil is `Replace` with the reference
// value. The pipelines come from MSL (the one use of MSL outside the tests),
// are made once per attachment format set and live in the device.

/// Arguments of [`CommandList::clear_attachment`].
#[derive(Clone, Copy)]
pub struct ClearArgs<'a> {
    /// Colour attachment index, or 8 for the depth-stencil attachment.
    pub target: u32,
    pub depth: bool,
    pub stencil: bool,
    /// Edges `[left, top, right, bottom]`; none = the whole attachment.
    pub rects: &'a [[i32; 4]],
    pub color: [f32; 4],
    pub clear_depth: f32,
    pub clear_stencil: u32,
}

const CLEAR_TABLE_SLOT: u32 = 3;
const CLEAR_COLOR: u8 = 0;
const CLEAR_DEPTH: u8 = 1;
const CLEAR_STENCIL: u8 = 2;
const CLEAR_LAYERED: u8 = 4;

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct ClearKey {
    colors: [u64; MAX_COLOR_ATTACHMENTS],
    depth: u64,
    samples: u32,
    /// Colour attachment index, or 8 for depth-stencil.
    target: u8,
    /// `CLEAR_*` bits.
    what: u8,
}

pub(crate) struct ClearPipe {
    pso: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    ds: Retained<ProtocolObject<dyn MTLDepthStencilState>>,
}

// Metal pipeline and depth-stencil state objects are immutable and thread safe.
unsafe impl Send for ClearPipe {}
unsafe impl Sync for ClearPipe {}

#[derive(Default)]
pub(crate) struct ClearPipes(HashMap<ClearKey, Arc<ClearPipe>>);

fn clear_source(target: u8, layered: bool) -> String {
    let layer_out = if layered {
        " uint layer [[render_target_array_index]];"
    } else {
        ""
    };
    let layer_set = if layered { " o.layer = iid;" } else { "" };
    let mut s = format!(
        "#include <metal_stdlib>\nusing namespace metal;\n\
         struct P {{ float4 color; float depth; }};\n\
         struct VO {{ float4 pos [[position]];{layer_out} }};\n\
         vertex VO clear_vs(uint vid [[vertex_id]], uint iid [[instance_id]], constant P& p [[buffer({CLEAR_TABLE_SLOT})]]) {{\n\
           float2 xy = float2(float((vid << 1) & 2), float(vid & 2));\n\
           VO o; o.pos = float4(xy * 2.0 - 1.0, p.depth, 1.0);{layer_set} return o; }}\n"
    );
    if target < 8 {
        s.push_str(&format!(
            "struct FO {{ float4 c [[color({target})]]; }};\n\
             fragment FO clear_fs(VO i [[stage_in]], constant P& p [[buffer({CLEAR_TABLE_SLOT})]]) {{ FO o; o.c = p.color; return o; }}\n"
        ));
    }
    s
}

impl crate::record::CommandList {
    fn clear_pipe(&self, key: &ClearKey, pass: &Pass) -> Result<Arc<ClearPipe>> {
        let device = self.queue().device();
        let mut cache = device
            .inner
            .clear_pipes
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(p) = cache.0.get(key) {
            return Ok(p.clone());
        }
        let layered = key.what & CLEAR_LAYERED != 0;
        let library = device.library_from_msl(&clear_source(key.target, layered))?;
        let function = |name: &str| {
            let f = MTL4LibraryFunctionDescriptor::new();
            f.setName(Some(&NSString::from_str(name)));
            f.setLibrary(Some(library.raw()));
            f
        };
        let d = MTL4RenderPipelineDescriptor::new();
        d.setVertexFunctionDescriptor(Some(&function("clear_vs")));
        if key.target < 8 {
            d.setFragmentFunctionDescriptor(Some(&function("clear_fs")));
        }
        // SAFETY: the pass's own sample count.
        unsafe { d.setRasterSampleCount(key.samples as usize) };
        for (i, f) in pass.color_formats.iter().enumerate() {
            if *f == MTLPixelFormat::Invalid {
                continue;
            }
            // SAFETY: index below 8.
            let a = unsafe { d.colorAttachments().objectAtIndexedSubscript(i) };
            a.setPixelFormat(*f);
            a.setWriteMask(if i as u8 == key.target {
                MTLColorWriteMask::All
            } else {
                MTLColorWriteMask::None
            });
        }
        let base: &MTL4PipelineDescriptor = &d;
        let pso = device
            .inner
            .compiler
            .newRenderPipelineStateWithDescriptor_compilerTaskOptions_error(base, None)
            .map_err(|e| Error::Metal(format!("clear pipeline: {}", e.localizedDescription())))?;
        let ds = MTLDepthStencilDescriptor::new();
        ds.setDepthCompareFunction(MTLCompareFunction::Always);
        ds.setDepthWriteEnabled(key.what & CLEAR_DEPTH != 0);
        if key.what & CLEAR_STENCIL != 0 {
            let s = MTLStencilDescriptor::new();
            s.setStencilCompareFunction(MTLCompareFunction::Always);
            s.setStencilFailureOperation(MTLStencilOperation::Replace);
            s.setDepthFailureOperation(MTLStencilOperation::Replace);
            s.setDepthStencilPassOperation(MTLStencilOperation::Replace);
            s.setReadMask(0xFF);
            s.setWriteMask(0xFF);
            ds.setFrontFaceStencil(Some(&s));
            ds.setBackFaceStencil(Some(&s));
        }
        let ds = device
            .inner
            .raw
            .newDepthStencilStateWithDescriptor(&ds)
            .ok_or_else(|| Error::Metal("clear depth-stencil state".into()))?;
        let p = Arc::new(ClearPipe { pso, ds });
        cache.0.insert(key.clone(), p.clone());
        Ok(p)
    }

    /// Clears `rects` (edges `[left, top, right, bottom]`; none = all) of
    /// colour attachment `target`, or of the depth and/or stencil plane
    /// when `target` is 8.
    pub fn clear_attachment(&mut self, a: &ClearArgs) -> Result<()> {
        let ClearArgs {
            target,
            depth,
            stencil,
            rects,
            color,
            clear_depth,
            clear_stencil,
        } = *a;
        self.live()?;
        let pass = self
            .pass
            .as_ref()
            .ok_or_else(|| Error::BadArgument("a clear outside a render pass".into()))?;
        let (kind, tgt) = if target < 8 {
            match pass.spec.colors.get(target as usize) {
                Some(Some(_)) => {}
                _ => return Err(Error::BadArgument(format!("no colour attachment {target}"))),
            }
            (CLEAR_COLOR, target as u8)
        } else if target == 8 {
            if pass.depth.is_none() {
                return Err(Error::BadArgument("no depth-stencil attachment".into()));
            }
            let mut w = 0;
            if depth && has_depth(pass.depth_format) {
                w |= CLEAR_DEPTH;
            }
            if stencil && pass.has_stencil {
                w |= CLEAR_STENCIL;
            }
            (w, 8)
        } else {
            return Err(Error::BadArgument(format!("clear target {target}")));
        };
        if tgt == 8 && kind == 0 {
            return Ok(());
        }
        let layered = pass.layers > 1;
        let mut key = ClearKey {
            colors: [0; MAX_COLOR_ATTACHMENTS],
            depth: if tgt == 8 {
                pass.depth_format.0 as u64
            } else {
                0
            },
            samples: pass.samples,
            target: tgt,
            what: kind | if layered { CLEAR_LAYERED } else { 0 },
        };
        for (i, f) in pass.color_formats.iter().enumerate() {
            key.colors[i] = f.0 as u64;
        }
        let pipe = self.clear_pipe(&key, pass)?;
        let size = pass.size;
        let layers = pass.layers;
        let enc = self.prepare_render(&[VERTEX, FRAGMENT])?;
        let block = self.alloc_constants(32, 16)?;
        let mut bytes = [0u8; 20];
        for (i, c) in color.iter().enumerate() {
            bytes[4 * i..4 * i + 4].copy_from_slice(&c.to_le_bytes());
        }
        bytes[16..20].copy_from_slice(&clear_depth.to_le_bytes());
        block.write_bytes(0, &bytes)?;
        self.table.set_address(CLEAR_TABLE_SLOT, block.gpu_va)?;

        enc.setRenderPipelineState(&pipe.pso);
        enc.setDepthStencilState(Some(&pipe.ds));
        enc.setCullMode(MTLCullMode::None);
        enc.setTriangleFillMode(MTLTriangleFillMode::Fill);
        enc.setDepthBias_slopeScale_clamp(0.0, 0.0, 0.0);
        enc.setStencilReferenceValue(clear_stencil);
        let mut view = MTLViewport {
            originX: 0.0,
            originY: 0.0,
            width: f64::from(size[0]),
            height: f64::from(size[1]),
            znear: 0.0,
            zfar: 1.0,
        };
        // SAFETY: one viewport.
        unsafe { enc.setViewports_count(std::ptr::NonNull::from(&mut view), 1) };
        let whole = [[0, 0, size[0] as i32, size[1] as i32]];
        for r in if rects.is_empty() { &whole[..] } else { rects } {
            let x = r[0].clamp(0, size[0] as i32);
            let y = r[1].clamp(0, size[1] as i32);
            let right = r[2].clamp(0, size[0] as i32);
            let bottom = r[3].clamp(0, size[1] as i32);
            if right <= x || bottom <= y {
                continue;
            }
            let mut sc = MTLScissorRect {
                x: x as usize,
                y: y as usize,
                width: (right - x) as usize,
                height: (bottom - y) as usize,
            };
            // SAFETY: one rectangle.
            unsafe {
                enc.setScissorRects_count(std::ptr::NonNull::from(&mut sc), 1);
                enc.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                    MTLPrimitiveType::Triangle,
                    0,
                    3,
                    layers as usize,
                    0,
                );
            }
        }
        // The state the next draw expects is the shadow's, not this.
        self.rstate.dirty = DIRTY_ALL;
        self.rstate.pipeline = None;
        self.stats.attachment_clears += 1;
        Ok(())
    }
}
