//! Graphics and mesh pipelines: DXIL stages through Metal Shader Converter to
//! `MTL4Compiler` render pipelines.
//!
//! This module is the core side of `CREATE_GRAPHICS_PIPELINE` (the proto
//! interpreter in `exec/gfx.rs` converts the wire description into
//! [`GraphicsDesc`] and owns the tables from the protocol's enums to Metal's).
//! It knows no proto and no D3D12 names; the values in a [`GraphicsDesc`] are
//! already Metal's.
//!
//! # Two steps, both thread-safe
//!
//! * [`prepare_graphics`]: every stage goes through MSC (a fresh `IRCompiler`
//!   per stage, nothing shared), the reflections are merged and checked. It
//!   needs no device and no state, so any number of worker threads may run
//!   it. The result, [`PreparedGraphics`], holds the `metallib`s and is what a
//!   shader cache would store (its key is the DXIL of the stages plus the
//!   [`GraphicsDesc`] facts that reach MSC: see [`GraphicsDesc::msc_options`]).
//! * [`Device::graphics_pipeline`]: libraries, vertex descriptor and render
//!   pipeline descriptor, `MTL4Compiler`. `MTL4Compiler`'s methods may be
//!   called from several threads (the header marks the protocol `Send +
//!   Sync`; `tests/gfx_pipeline.rs` compiles from four threads at once).
//!
//! [`Device::graphics_pipeline_from_dxil`] runs both. Room is left on purpose
//! for what comes next: the compile call is the only place that talks to the
//! compiler, so the asynchronous variant (a completion handler) and the
//! `MTL4Archive` lookup (`MTL4CompilerTaskOptions::lookupArchives`) are one
//! change there; nothing is cached yet.
//!
//! # Vertex input: Metal vertex fetch
//!
//! MSC is told `UseMetalVertexFetch` (its default): the converted vertex
//! shader reads `[[stage_in]]` attributes and the pipeline's
//! `MTLVertexDescriptor` feeds them. The alternative, a separate stage-in
//! function (`IRMetalLibSynthesizeStageInFunction`) linked as a visible
//! function, was not chosen because it
//!
//! * needs one more function per vertex layout, a linking descriptor in the
//!   pipeline descriptor and a second compile (the converter manual calls
//!   vertex fetch the fast path with little setup);
//! * is only worth it for conversions Metal's vertex formats cannot do, which
//!   the game's layouts do not need (see the format table in `exec/gfx.rs`:
//!   a format Metal cannot fetch is a clear error, and the separate function
//!   remains the fallback to add for it);
//! * D3D12's per-buffer strides, which arrive when buffers are bound, map to
//!   Metal 4's dynamic strides: every buffer layout here has
//!   `MTLBufferLayoutStrideDynamic`, so the argument table must be made with
//!   `supportAttributeStrides` and each vertex buffer bound with
//!   `setAddress:attributeStride:atIndex:` (docs/metal/proto-render-draft.md).
//!
//! The converter reflects each input as `<semantic lower case><index>`
//! (`position0`, `texcoord1`) with the attribute index to use; vertex buffer
//! slot `n` is Metal buffer index [`VERTEX_BUFFER_BASE`]` + n`.

use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSArray, NSString};
use objc2_metal::{
    MTL4AlphaToCoverageState, MTL4BlendState, MTL4Compiler, MTL4FunctionDescriptor,
    MTL4LibraryFunctionDescriptor, MTL4MeshRenderPipelineDescriptor, MTL4PipelineDescriptor,
    MTL4RenderPipelineDescriptor, MTL4SpecializedFunctionDescriptor, MTL4StaticLinkingDescriptor,
    MTLAllocation, MTLBlendFactor, MTLBlendOperation, MTLColorWriteMask, MTLCompareFunction,
    MTLCullMode, MTLDataType, MTLDepthClipMode, MTLDepthStencilDescriptor, MTLDepthStencilState,
    MTLDevice, MTLFunctionConstantValues, MTLLibrary, MTLPixelFormat, MTLPrimitiveTopologyClass,
    MTLRenderPipelineState, MTLSize, MTLStencilDescriptor, MTLStencilOperation,
    MTLTriangleFillMode, MTLVertexBufferLayoutDescriptor, MTLVertexDescriptor, MTLVertexFormat,
    MTLVertexStepFunction, MTLWinding,
};

use crate::device::{Device, DeviceInner};
use crate::error::Error;
use crate::msc::{
    Compat, CompileOptions, Converted, GraphicsCompile, InputTopology, Msc, MscError, Reflection,
    RootSignatureDesc, ShaderStage, StageInElement, TopLevelLayout,
};
use crate::pipeline::Library;
use crate::residency::{Retired, SetKind};

/// Metal attribute index of the converter's attribute 0
/// (`kIRStageInAttributeStartIndex`, header 4.0.3): the `attributeIndex` the
/// reflection reports is relative to it (checked against Metal's own
/// message, `position0(11)`, in `tests/gfx_pipeline.rs`).
pub const STAGE_IN_ATTRIBUTE_BASE: u32 = 11;
/// Metal buffer index of vertex buffer slot 0 (`kIRVertexBufferBindPoint`,
/// header 4.0.3). Slots 0..=24 fit under Metal's 31 buffer indices.
pub const VERTEX_BUFFER_BASE: u32 = 6;
/// Most vertex buffer slots (31 argument table indices minus the base).
pub const MAX_VERTEX_SLOTS: u32 = 25;

/// Limits of Metal mesh pipelines the converter's output must respect.
pub mod limits {
    /// Bytes of object-to-mesh payload.
    pub const MAX_PAYLOAD_BYTES: u32 = 16 * 1024;
    /// Threads in one object or mesh threadgroup.
    pub const MAX_THREADGROUP_THREADS: u32 = 1024;
    /// Vertices a mesh threadgroup may output.
    pub const MAX_MESH_VERTICES: u32 = 256;
    /// Primitives a mesh threadgroup may output.
    pub const MAX_MESH_PRIMITIVES: u32 = 512;
}

/// One element of the input layout, resolved: `offset` is a byte offset (no
/// "append" marker), the format is a Metal vertex format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VertexElement {
    /// Semantic name (any case; compared lower case).
    pub semantic: String,
    pub semantic_index: u32,
    pub format: MTLVertexFormat,
    /// The DXGI format the element had (a geometry pipeline's stage-in
    /// function is made from it).
    pub dxgi_format: u32,
    /// Vertex buffer slot.
    pub slot: u32,
    pub offset: u32,
    /// Per-instance data, advancing every `step_rate` instances (>= 1).
    pub per_instance: bool,
    pub step_rate: u32,
}

/// Blending of one render target, Metal's values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetBlend {
    pub enabled: bool,
    pub src_rgb: MTLBlendFactor,
    pub dst_rgb: MTLBlendFactor,
    pub op_rgb: MTLBlendOperation,
    pub src_alpha: MTLBlendFactor,
    pub dst_alpha: MTLBlendFactor,
    pub op_alpha: MTLBlendOperation,
    pub write_mask: MTLColorWriteMask,
}

impl TargetBlend {
    /// Blending off, every channel written.
    pub fn opaque() -> Self {
        TargetBlend {
            enabled: false,
            src_rgb: MTLBlendFactor::One,
            dst_rgb: MTLBlendFactor::Zero,
            op_rgb: MTLBlendOperation::Add,
            src_alpha: MTLBlendFactor::One,
            dst_alpha: MTLBlendFactor::Zero,
            op_alpha: MTLBlendOperation::Add,
            write_mask: MTLColorWriteMask::All,
        }
    }

    /// Whether a factor reads the second fragment output.
    pub fn uses_second_source(&self) -> bool {
        self.enabled
            && [self.src_rgb, self.dst_rgb, self.src_alpha, self.dst_alpha]
                .iter()
                .any(|f| {
                    matches!(
                        *f,
                        MTLBlendFactor::Source1Color
                            | MTLBlendFactor::OneMinusSource1Color
                            | MTLBlendFactor::Source1Alpha
                            | MTLBlendFactor::OneMinusSource1Alpha
                    )
                })
    }
}

/// A render target of the pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorTarget {
    /// `Invalid` = no attachment in this slot.
    pub format: MTLPixelFormat,
    /// Integer format: the converter bitcasts the shader's output.
    pub integer: bool,
    pub blend: TargetBlend,
}

/// One face of the stencil test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StencilFace {
    pub fail: MTLStencilOperation,
    pub depth_fail: MTLStencilOperation,
    pub pass: MTLStencilOperation,
    pub compare: MTLCompareFunction,
}

/// Depth and stencil state. Depth testing off is `Always` with writes off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DepthStencil {
    pub depth_compare: MTLCompareFunction,
    pub depth_write: bool,
    pub stencil: bool,
    pub front: StencilFace,
    pub back: StencilFace,
    pub read_mask: u32,
    pub write_mask: u32,
}

/// Rasterizer state, applied on the render encoder when the pipeline is bound.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Raster {
    pub fill: MTLTriangleFillMode,
    pub cull: MTLCullMode,
    pub front_winding: MTLWinding,
    pub depth_clip: MTLDepthClipMode,
    pub depth_bias: f32,
    pub slope_scale: f32,
    pub bias_clamp: f32,
}

/// What a graphics or mesh pipeline is made of, in Metal's terms.
pub struct GraphicsDesc<'a> {
    /// DXIL containers of the stages. A vertex pipeline has `vs` (and maybe
    /// `gs`); a mesh pipeline has `ms` (and maybe `amp`); `ps` is optional
    /// in all.
    pub vs: Option<&'a [u8]>,
    pub ps: Option<&'a [u8]>,
    pub amp: Option<&'a [u8]>,
    pub ms: Option<&'a [u8]>,
    pub gs: Option<&'a [u8]>,
    /// The root signature every stage is converted against.
    pub root: &'a RootSignatureDesc,
    /// Input layout (vertex pipelines).
    pub inputs: Vec<VertexElement>,
    /// Render targets, in slot order (up to 8).
    pub targets: Vec<ColorTarget>,
    pub topology: MTLPrimitiveTopologyClass,
    pub sample_count: u32,
    pub sample_mask: u32,
    pub alpha_to_coverage: bool,
    /// The pipeline has a depth-stencil target.
    pub has_depth_target: bool,
    pub depth_stencil: DepthStencil,
    pub raster: Raster,
    pub compat: Compat,
}

impl GraphicsDesc<'_> {
    /// The conversion settings of this pipeline: what MSC bakes in, so also
    /// what a converted-shader cache must key on besides the DXIL.
    pub fn msc_options(&self) -> CompileOptions {
        let mut int_rt_mask = 0u8;
        for (i, t) in self.targets.iter().enumerate().take(8) {
            if t.integer {
                int_rt_mask |= 1 << i;
            }
        }
        CompileOptions {
            compat: self.compat,
            graphics: Some(GraphicsCompile {
                input_topology: match self.topology {
                    MTLPrimitiveTopologyClass::Point => InputTopology::Point,
                    MTLPrimitiveTopologyClass::Line => InputTopology::Line,
                    MTLPrimitiveTopologyClass::Triangle => InputTopology::Triangle,
                    _ => InputTopology::Undefined,
                },
                int_rt_mask,
                sample_mask: self.sample_mask,
                dual_source: self.targets.iter().any(TargetBlendUse::second_source),
                depth_feedback: self.has_depth_target,
                geometry_emulation: false,
            }),
            ..CompileOptions::default()
        }
    }

    /// The options of the vertex or geometry stage of a geometry pipeline:
    /// converted for the emulation, the vertex stage with a separate
    /// stage-in function for this input layout.
    pub fn msc_options_geometry(&self, vertex_stage: bool) -> CompileOptions {
        let mut o = self.msc_options();
        if let Some(g) = o.graphics.as_mut() {
            g.geometry_emulation = true;
        }
        if vertex_stage {
            o.stage_in = Some(
                self.inputs
                    .iter()
                    .map(|e| StageInElement {
                        semantic: e.semantic.clone(),
                        semantic_index: e.semantic_index,
                        format: e.dxgi_format,
                        slot: e.slot,
                        offset: e.offset,
                        per_instance: e.per_instance,
                        step_rate: e.step_rate,
                    })
                    .collect(),
            );
        }
        o
    }
}

trait TargetBlendUse {
    fn second_source(&self) -> bool;
}
impl TargetBlendUse for ColorTarget {
    fn second_source(&self) -> bool {
        self.blend.uses_second_source()
    }
}

/// Which pipeline a description makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Vertex,
    Mesh,
    /// A vertex pipeline with a geometry shader: emulated with an object
    /// stage (the vertex shader) and a mesh stage (the geometry shader).
    Geometry,
}

/// What a draw of a geometry pipeline needs (the converter's
/// `IRRuntimeGeometryPipelineConfig` and the stage's reflection).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeometryPipelineInfo {
    /// Bytes of one vertex shader output (`gsVertexSizeInBytes`).
    pub vertex_bytes: u32,
    /// `gsMaxInputPrimitivesPerMeshThreadgroup`.
    pub max_input_primitives: u32,
    /// `IRInputPrimitive` of the geometry shader.
    pub input_primitive: u32,
    pub payload_bytes: u32,
}

/// The merged facts of a converted mesh pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshPipelineInfo {
    /// Object (amplification) threadgroup size, when there is an object stage.
    pub object_threads: Option<[u32; 3]>,
    pub mesh_threads: [u32; 3],
    /// Bytes of payload the object stage hands to the mesh stage.
    pub payload_bytes: u32,
    pub max_vertices: u32,
    pub max_primitives: u32,
}

/// The conversions of all stages, checked against each other.
pub struct PreparedGraphics {
    pub kind: Kind,
    pub vs: Option<Converted>,
    pub ps: Option<Converted>,
    pub amp: Option<Converted>,
    pub ms: Option<Converted>,
    pub gs: Option<Converted>,
    /// The top-level argument buffer layout every stage agrees on.
    pub layout: TopLevelLayout,
    pub mesh: Option<MeshPipelineInfo>,
    pub geometry: Option<GeometryPipelineInfo>,
}

impl PreparedGraphics {
    /// Bytes of all `metallib`s (diagnostics).
    pub fn metallib_bytes(&self) -> usize {
        [&self.vs, &self.ps, &self.amp, &self.ms, &self.gs]
            .into_iter()
            .flatten()
            .map(|c| c.metallib.len())
            .sum()
    }
}

fn convert(
    msc: &Msc,
    stage_name: &str,
    want: ShaderStage,
    dxil: &[u8],
    desc: &GraphicsDesc<'_>,
) -> Result<Converted, MscError> {
    let options =
        if desc.gs.is_some() && matches!(want, ShaderStage::Vertex | ShaderStage::Geometry) {
            desc.msc_options_geometry(want == ShaderStage::Vertex)
        } else {
            desc.msc_options()
        };
    let c = msc.compile(dxil, Some(desc.root), &options)?;
    if c.reflection.stage != want {
        return Err(MscError::BadInput(format!(
            "the {stage_name} stage's DXIL is a {:?} shader",
            c.reflection.stage
        )));
    }
    Ok(c)
}

fn same_layout(a: &TopLevelLayout, b: &TopLevelLayout) -> bool {
    a.size == b.size
        && a.static_sampler_table == b.static_sampler_table
        && a.entries.len() == b.entries.len()
        && a.entries
            .iter()
            .zip(&b.entries)
            .all(|(x, y)| x.offset == y.offset && x.size == y.size)
}

/// Converts and checks every stage of `desc` (no device needed; safe to call
/// from any thread, concurrently).
pub fn prepare_graphics(desc: &GraphicsDesc<'_>) -> Result<PreparedGraphics, MscError> {
    let kind = match (
        desc.vs.is_some(),
        desc.amp.is_some(),
        desc.ms.is_some(),
        desc.gs.is_some(),
    ) {
        (true, false, false, false) => Kind::Vertex,
        (true, false, false, true) => Kind::Geometry,
        (false, _, true, false) => Kind::Mesh,
        _ => {
            return Err(MscError::BadInput(
                "a pipeline has a vertex stage (with an optional geometry stage), or a mesh stage with an optional amplification stage".into(),
            ));
        }
    };
    if desc.targets.len() > 8 {
        return Err(MscError::BadInput("more than 8 render targets".into()));
    }
    let msc = Msc::get()?;
    let stage = |name: &str, want, code: Option<&[u8]>| -> Result<Option<Converted>, MscError> {
        code.map(|d| convert(&msc, name, want, d, desc)).transpose()
    };
    let vs = stage("vertex", ShaderStage::Vertex, desc.vs)?;
    let amp = stage("amplification", ShaderStage::Amplification, desc.amp)?;
    let ms = stage("mesh", ShaderStage::Mesh, desc.ms)?;
    let gs = stage("geometry", ShaderStage::Geometry, desc.gs)?;
    let ps = stage("pixel", ShaderStage::Fragment, desc.ps)?;

    // One root signature, one argument buffer: the stages must have been
    // laid out the same way, or the one buffer the encoder binds cannot
    // serve them all.
    let all = [&vs, &amp, &ms, &gs, &ps];
    let first = all
        .iter()
        .find_map(|c| c.as_ref())
        .ok_or_else(|| MscError::BadInput("no stage".into()))?;
    for (name, c) in ["vertex", "amplification", "mesh", "geometry", "pixel"]
        .iter()
        .zip(all)
    {
        if let Some(c) = c
            && !same_layout(&c.reflection.layout, &first.reflection.layout)
        {
            return Err(MscError::Mismatch(format!(
                "the {name} stage's argument buffer layout {:?} differs from the first stage's {:?}",
                c.reflection.layout, first.reflection.layout
            )));
        }
    }
    let layout = first.reflection.layout.clone();

    // A pixel shader that writes more targets than the pipeline has
    // attachments (D3D12: the extra outputs are dropped) is made as it is:
    // Metal drops writes to colour slots without an attachment.

    let mesh = match &ms {
        None => None,
        Some(ms) => Some(mesh_info(
            ms.reflection.clone(),
            amp.as_ref().map(|a| &a.reflection),
        )?),
    };
    let geometry = match (&vs, &gs) {
        (Some(vs), Some(gs)) => {
            let info = gs
                .reflection
                .geometry
                .ok_or_else(|| MscError::NoOutput("no geometry reflection".into()))?;
            let vertex_bytes = vs
                .reflection
                .vertex
                .as_ref()
                .map(|v| v.output_bytes)
                .ok_or_else(|| MscError::NoOutput("no vertex reflection".into()))?;
            if vs.stage_in.is_none() {
                return Err(MscError::NoOutput("no stage-in function".into()));
            }
            Some(GeometryPipelineInfo {
                vertex_bytes,
                max_input_primitives: info.max_input_primitives_per_mesh_threadgroup,
                input_primitive: info.input_primitive,
                payload_bytes: info.max_payload_bytes,
            })
        }
        _ => None,
    };
    Ok(PreparedGraphics {
        kind,
        vs,
        ps,
        amp,
        ms,
        gs,
        layout,
        mesh,
        geometry,
    })
}

/// Merges the mesh and amplification reflections and checks the Metal limits,
/// with a message that names the number and the limit.
fn mesh_info(ms: Reflection, amp: Option<&Reflection>) -> Result<MeshPipelineInfo, MscError> {
    use limits::*;
    let m = ms
        .mesh
        .ok_or_else(|| MscError::NoOutput("no mesh reflection".into()))?;
    let a = match amp {
        Some(r) => Some(
            r.mesh
                .ok_or_else(|| MscError::NoOutput("no amplification reflection".into()))?,
        ),
        None => None,
    };
    let over = |what: &str, got: u32, max: u32| {
        MscError::Limit(format!(
            "mesh pipeline: {what} is {got}, Metal allows at most {max}"
        ))
    };
    let threads = |t: [u32; 3]| t[0].saturating_mul(t[1]).saturating_mul(t[2]);
    if threads(m.num_threads) > MAX_THREADGROUP_THREADS {
        return Err(over(
            "mesh threads per group",
            threads(m.num_threads),
            MAX_THREADGROUP_THREADS,
        ));
    }
    if let Some(a) = a
        && threads(a.num_threads) > MAX_THREADGROUP_THREADS
    {
        return Err(over(
            "amplification threads per group",
            threads(a.num_threads),
            MAX_THREADGROUP_THREADS,
        ));
    }
    if m.max_vertices > MAX_MESH_VERTICES {
        return Err(over(
            "mesh output vertices",
            m.max_vertices,
            MAX_MESH_VERTICES,
        ));
    }
    if m.max_primitives > MAX_MESH_PRIMITIVES {
        return Err(over(
            "mesh output primitives",
            m.max_primitives,
            MAX_MESH_PRIMITIVES,
        ));
    }
    // The payload size is declared by both stages (the object stage writes it,
    // the mesh stage reads it): take the larger, they should agree.
    let payload = m
        .max_payload_bytes
        .max(a.map_or(0, |a| a.max_payload_bytes));
    if payload > MAX_PAYLOAD_BYTES {
        return Err(over("payload", payload, MAX_PAYLOAD_BYTES));
    }
    Ok(MeshPipelineInfo {
        object_threads: a.map(|a| a.num_threads),
        mesh_threads: m.num_threads,
        payload_bytes: payload,
        max_vertices: m.max_vertices,
        max_primitives: m.max_primitives,
    })
}

struct GraphicsInner {
    raw: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    depth_stencil: Retained<ProtocolObject<dyn MTLDepthStencilState>>,
    device: Arc<DeviceInner>,
    kind: Kind,
    raster: Raster,
    topology: MTLPrimitiveTopologyClass,
    samples: u32,
    layout: TopLevelLayout,
    mesh: Option<MeshPipelineInfo>,
    geometry: Option<GeometryPipelineInfo>,
    vertex_slots: Vec<u32>,
    needs_draw_params: bool,
    reflections: Vec<(ShaderStage, Reflection)>,
}

unsafe impl Send for GraphicsInner {}
unsafe impl Sync for GraphicsInner {}

impl Drop for GraphicsInner {
    fn drop(&mut self) {
        let object: Retained<ProtocolObject<dyn MTLAllocation>> =
            ProtocolObject::from_retained(self.raw.clone());
        self.device.residency.retire(Retired {
            object,
            kind: SetKind::Resource,
            waits: self.device.in_flight_waits(),
            keep: None,
        });
    }
}

/// A vertex or mesh render pipeline with the state that goes with it on the
/// encoder (depth-stencil state object, rasterizer state). Cloning shares it;
/// the last drop retires it like any resource.
#[derive(Clone)]
pub struct GraphicsPipeline {
    inner: Arc<GraphicsInner>,
}

impl GraphicsPipeline {
    pub fn kind(&self) -> Kind {
        self.inner.kind
    }

    pub fn raster(&self) -> Raster {
        self.inner.raster
    }

    /// The primitive class the pipeline was made for (a draw's topology must
    /// be of this class).
    pub fn topology_class(&self) -> MTLPrimitiveTopologyClass {
        self.inner.topology
    }

    /// Samples per pixel it renders with.
    pub fn sample_count(&self) -> u32 {
        self.inner.samples
    }

    /// The argument buffer layout (the same for every stage).
    pub fn layout(&self) -> &TopLevelLayout {
        &self.inner.layout
    }

    pub fn mesh(&self) -> Option<MeshPipelineInfo> {
        self.inner.mesh
    }

    /// The facts a draw of a geometry pipeline needs.
    pub fn geometry(&self) -> Option<GeometryPipelineInfo> {
        self.inner.geometry
    }

    /// Vertex buffer slots the input layout uses, ascending.
    pub fn vertex_slots(&self) -> &[u32] {
        &self.inner.vertex_slots
    }

    /// The vertex shader reads base vertex / base instance from draw parameters.
    pub fn needs_draw_params(&self) -> bool {
        self.inner.needs_draw_params
    }

    /// Reflection of each stage (diagnostics, tests).
    pub fn reflections(&self) -> &[(ShaderStage, Reflection)] {
        &self.inner.reflections
    }

    /// The Metal pipeline state (for the render encoder).
    pub fn raw(&self) -> &ProtocolObject<dyn MTLRenderPipelineState> {
        &self.inner.raw
    }

    /// The depth-stencil state object to set with the pipeline.
    pub fn depth_stencil_state(&self) -> &ProtocolObject<dyn MTLDepthStencilState> {
        &self.inner.depth_stencil
    }

    /// Identity of the underlying pipeline (for redundant-bind elision).
    pub fn identity(&self) -> *const () {
        Arc::as_ptr(&self.inner).cast()
    }
}

fn function_descriptor(
    library: &ProtocolObject<dyn MTLLibrary>,
    entry: &str,
) -> Result<Retained<MTL4LibraryFunctionDescriptor>, Error> {
    // The compiler aborts the process when the function is missing: ask first.
    let wanted = NSString::from_str(entry);
    if !library
        .functionNames()
        .iter()
        .any(|n| n.isEqualToString(&wanted))
    {
        return Err(Error::BadArgument(format!(
            "the library has no function named `{entry}`"
        )));
    }
    let f = MTL4LibraryFunctionDescriptor::new();
    f.setName(Some(&wanted));
    f.setLibrary(Some(library));
    Ok(f)
}

/// A function constant value.
enum Constant {
    Bool(bool),
    Int(i32),
}

fn constants(values: &[(&str, Constant)]) -> Retained<MTLFunctionConstantValues> {
    let c = MTLFunctionConstantValues::new();
    for (name, v) in values {
        let name = NSString::from_str(name);
        // SAFETY: the value is read for the call (Metal copies it).
        unsafe {
            match v {
                Constant::Bool(b) => {
                    let b = *b;
                    c.setConstantValue_type_withName(
                        std::ptr::NonNull::from(&b).cast(),
                        MTLDataType::Bool,
                        &name,
                    );
                }
                Constant::Int(i) => {
                    let i = *i;
                    c.setConstantValue_type_withName(
                        std::ptr::NonNull::from(&i).cast(),
                        MTLDataType::Int,
                        &name,
                    );
                }
            }
        }
    }
    c
}

/// A library function with function constants.
fn specialized(
    library: &ProtocolObject<dyn MTLLibrary>,
    entry: &str,
    values: &MTLFunctionConstantValues,
) -> Result<Retained<MTL4SpecializedFunctionDescriptor>, Error> {
    let f = function_descriptor(library, entry)?;
    let base: Retained<MTL4FunctionDescriptor> = Retained::into_super(f);
    let d = MTL4SpecializedFunctionDescriptor::new();
    d.setFunctionDescriptor(Some(&base));
    d.setConstantValues(Some(values));
    Ok(d)
}

/// The vertex descriptor for the shader's inputs from the layout elements.
/// A shader input the layout does not supply is an error: Metal would read
/// whatever the attribute defaults to.
fn vertex_descriptor(
    vs: &Reflection,
    inputs: &[VertexElement],
) -> Result<(Retained<MTLVertexDescriptor>, Vec<u32>), MscError> {
    let info = vs
        .vertex
        .as_ref()
        .ok_or_else(|| MscError::NoOutput("no vertex reflection".into()))?;
    let descriptor = MTLVertexDescriptor::new();
    let mut slots: Vec<u32> = Vec::new();
    // SAFETY: attribute indices (base + index) are checked below to be < 31 and buffer
    // indices are < 31 by `MAX_VERTEX_SLOTS`: inside Metal's arrays.
    unsafe {
        for input in &info.inputs {
            if STAGE_IN_ATTRIBUTE_BASE + input.attribute_index >= 31 {
                return Err(MscError::Unsupported(format!(
                    "vertex attribute index {}",
                    input.attribute_index
                )));
            }
            let element = inputs.iter().find(|e| {
                format!("{}{}", e.semantic.to_ascii_lowercase(), e.semantic_index) == input.name
            });
            let Some(e) = element else {
                return Err(MscError::Unsupported(format!(
                    "the vertex shader reads `{}`, which the input layout does not have",
                    input.name
                )));
            };
            if e.slot >= MAX_VERTEX_SLOTS {
                return Err(MscError::Unsupported(format!(
                    "vertex buffer slot {} (at most {} slots)",
                    e.slot, MAX_VERTEX_SLOTS
                )));
            }
            let buffer_index = (VERTEX_BUFFER_BASE + e.slot) as usize;
            let attribute = descriptor.attributes().objectAtIndexedSubscript(
                (STAGE_IN_ATTRIBUTE_BASE + input.attribute_index) as usize,
            );
            attribute.setFormat(e.format);
            attribute.setOffset(e.offset as usize);
            attribute.setBufferIndex(buffer_index);
            if !slots.contains(&e.slot) {
                slots.push(e.slot);
                let layout: Retained<MTLVertexBufferLayoutDescriptor> =
                    descriptor.layouts().objectAtIndexedSubscript(buffer_index);
                // The stride comes with the vertex buffer binding.
                layout.setStride(objc2_metal::MTLBufferLayoutStrideDynamic);
                if e.per_instance {
                    layout.setStepFunction(MTLVertexStepFunction::PerInstance);
                    layout.setStepRate(e.step_rate.max(1) as usize);
                } else {
                    layout.setStepFunction(MTLVertexStepFunction::PerVertex);
                    layout.setStepRate(1);
                }
            } else {
                // Two elements of one buffer must agree on the step.
                let layout = descriptor.layouts().objectAtIndexedSubscript(buffer_index);
                let want = if e.per_instance {
                    MTLVertexStepFunction::PerInstance
                } else {
                    MTLVertexStepFunction::PerVertex
                };
                if layout.stepFunction() != want {
                    return Err(MscError::Unsupported(format!(
                        "vertex buffer slot {} mixes per-vertex and per-instance elements",
                        e.slot
                    )));
                }
            }
        }
    }
    slots.sort_unstable();
    Ok((descriptor, slots))
}

fn color_attachments(
    descriptor: &objc2_metal::MTL4RenderPipelineColorAttachmentDescriptorArray,
    targets: &[ColorTarget],
) {
    for (i, t) in targets.iter().enumerate().take(8) {
        // SAFETY: at most 8 targets, Metal has 8 color attachments.
        let a = unsafe { descriptor.objectAtIndexedSubscript(i) };
        a.setPixelFormat(t.format);
        if t.format == MTLPixelFormat::Invalid {
            continue;
        }
        a.setWriteMask(t.blend.write_mask);
        if t.blend.enabled && !t.integer {
            a.setBlendingState(MTL4BlendState::Enabled);
            a.setSourceRGBBlendFactor(t.blend.src_rgb);
            a.setDestinationRGBBlendFactor(t.blend.dst_rgb);
            a.setRgbBlendOperation(t.blend.op_rgb);
            a.setSourceAlphaBlendFactor(t.blend.src_alpha);
            a.setDestinationAlphaBlendFactor(t.blend.dst_alpha);
            a.setAlphaBlendOperation(t.blend.op_alpha);
        } else {
            a.setBlendingState(MTL4BlendState::Disabled);
        }
    }
}

fn stencil_descriptor(f: &StencilFace, d: &DepthStencil) -> Retained<MTLStencilDescriptor> {
    let s = MTLStencilDescriptor::new();
    s.setStencilFailureOperation(f.fail);
    s.setDepthFailureOperation(f.depth_fail);
    s.setDepthStencilPassOperation(f.pass);
    s.setStencilCompareFunction(f.compare);
    s.setReadMask(d.read_mask);
    s.setWriteMask(d.write_mask);
    s
}

impl Device {
    /// Makes the pipeline from converted stages (the second step; see the
    /// module docs). Safe to call from several threads.
    pub fn graphics_pipeline(
        &self,
        prepared: &PreparedGraphics,
        desc: &GraphicsDesc<'_>,
    ) -> Result<GraphicsPipeline, MscError> {
        if desc.sample_count == 0 || desc.sample_count > 16 {
            return Err(MscError::BadInput(format!(
                "sample count {}",
                desc.sample_count
            )));
        }
        let library = |c: &Option<Converted>| -> Result<Option<(Library, String)>, MscError> {
            c.as_ref()
                .map(|c| {
                    self.library_from_metallib(&c.metallib)
                        .map(|l| (l, c.reflection.entry.clone()))
                        .map_err(MscError::Metal)
                })
                .transpose()
        };
        let function = |l: &Option<(Library, String)>| -> Result<_, MscError> {
            l.as_ref()
                .map(|(l, entry)| function_descriptor(l.raw(), entry).map_err(MscError::Metal))
                .transpose()
        };
        let ps = library(&prepared.ps)?;
        let ps_fn = function(&ps)?;
        let compiler: &ProtocolObject<dyn MTL4Compiler> = &self.inner.compiler;
        let mut vertex_slots = Vec::new();
        let mut needs_draw_params = false;
        let mut reflections = Vec::new();
        let compiled: Result<Retained<ProtocolObject<dyn MTLRenderPipelineState>>, MscError> =
            match prepared.kind {
                Kind::Vertex => {
                    let vs_c = prepared
                        .vs
                        .as_ref()
                        .expect("a vertex pipeline has a vertex stage");
                    let vs = library(&prepared.vs)?;
                    let vs_fn = function(&vs)?.expect("present");
                    let (vd, slots) = vertex_descriptor(&vs_c.reflection, &desc.inputs)?;
                    vertex_slots = slots;
                    needs_draw_params = vs_c
                        .reflection
                        .vertex
                        .as_ref()
                        .is_some_and(|v| v.needs_draw_params);
                    let d = MTL4RenderPipelineDescriptor::new();
                    d.setVertexFunctionDescriptor(Some(&vs_fn));
                    if let Some(f) = &ps_fn {
                        d.setFragmentFunctionDescriptor(Some(f));
                    }
                    d.setVertexDescriptor(Some(&vd));
                    // SAFETY: a sample count Metal checks at compile.
                    unsafe { d.setRasterSampleCount(desc.sample_count as usize) };
                    d.setInputPrimitiveTopology(desc.topology);
                    d.setAlphaToCoverageState(if desc.alpha_to_coverage {
                        MTL4AlphaToCoverageState::Enabled
                    } else {
                        MTL4AlphaToCoverageState::Disabled
                    });
                    color_attachments(&d.colorAttachments(), &desc.targets);
                    compile_render(compiler, &d, "vertex pipeline")
                }
                Kind::Geometry => {
                    let g = prepared
                        .geometry
                        .expect("a geometry pipeline has geometry facts");
                    let vs_c = prepared.vs.as_ref().expect("present");
                    let gs_c = prepared.gs.as_ref().expect("present");
                    let vs = library(&prepared.vs)?.expect("present");
                    let gs = library(&prepared.gs)?.expect("present");
                    let stage_in_lib = self
                        .library_from_metallib(vs_c.stage_in.as_ref().expect("present"))
                        .map_err(MscError::Metal)?;
                    // The converter's runtime helper builds this pipeline
                    // (`IRRuntimeNewGeometryEmulationPipeline`): the vertex
                    // shader is the object function, the geometry shader the
                    // mesh function, the stage-in function linked to the
                    // object stage.
                    let tess_off = constants(&[("tessellationEnabled", Constant::Bool(false))]);
                    let object_name =
                        format!("{}.dxil_irconverter_object_shader", vs_c.reflection.entry);
                    let object_fn = specialized(vs.0.raw(), &object_name, &tess_off)
                        .map_err(MscError::Metal)?;
                    let mesh_consts = constants(&[
                        ("tessellationEnabled", Constant::Bool(false)),
                        (
                            "vertex_shader_output_size_fc",
                            Constant::Int(g.vertex_bytes as i32),
                        ),
                    ]);
                    let mesh_fn = specialized(gs.0.raw(), &gs_c.reflection.entry, &mesh_consts)
                        .map_err(MscError::Metal)?;
                    let stage_in_name = stage_in_lib
                        .raw()
                        .functionNames()
                        .iter()
                        .next()
                        .map(|n| n.to_string())
                        .ok_or_else(|| MscError::NoOutput("an empty stage-in library".into()))?;
                    let stage_in_fn = function_descriptor(stage_in_lib.raw(), &stage_in_name)
                        .map_err(MscError::Metal)?;
                    let d = MTL4MeshRenderPipelineDescriptor::new();
                    d.setObjectFunctionDescriptor(Some(&object_fn));
                    d.setMeshFunctionDescriptor(Some(&mesh_fn));
                    if let Some(f) = &ps_fn {
                        d.setFragmentFunctionDescriptor(Some(f));
                    }
                    let linking = MTL4StaticLinkingDescriptor::new();
                    let linked: Retained<MTL4FunctionDescriptor> =
                        Retained::into_super(stage_in_fn);
                    linking.setFunctionDescriptors(Some(&NSArray::from_retained_slice(&[linked])));
                    d.setObjectStaticLinkingDescriptor(Some(&linking));
                    d.setMaxTotalThreadsPerMeshThreadgroup(g.max_input_primitives.max(1) as usize);
                    d.setMaxTotalThreadsPerObjectThreadgroup(256);
                    d.setPayloadMemoryLength(g.payload_bytes.max(16384) as usize);
                    // SAFETY: a sample count Metal checks at compile.
                    unsafe { d.setRasterSampleCount(desc.sample_count as usize) };
                    d.setAlphaToCoverageState(if desc.alpha_to_coverage {
                        MTL4AlphaToCoverageState::Enabled
                    } else {
                        MTL4AlphaToCoverageState::Disabled
                    });
                    color_attachments(&d.colorAttachments(), &desc.targets);
                    compile_mesh(compiler, &d)
                }
                Kind::Mesh => {
                    let info = prepared.mesh.expect("a mesh pipeline has mesh facts");
                    let ms = library(&prepared.ms)?;
                    let ms_fn = function(&ms)?.expect("present");
                    let amp = library(&prepared.amp)?;
                    let amp_fn = function(&amp)?;
                    let d = MTL4MeshRenderPipelineDescriptor::new();
                    d.setMeshFunctionDescriptor(Some(&ms_fn));
                    if let Some(f) = &amp_fn {
                        d.setObjectFunctionDescriptor(Some(f));
                    }
                    if let Some(f) = &ps_fn {
                        d.setFragmentFunctionDescriptor(Some(f));
                    }
                    let size = |t: [u32; 3]| MTLSize {
                        width: t[0] as usize,
                        height: t[1] as usize,
                        depth: t[2] as usize,
                    };
                    d.setRequiredThreadsPerMeshThreadgroup(size(info.mesh_threads));
                    d.setMaxTotalThreadsPerMeshThreadgroup(
                        info.mesh_threads.iter().product::<u32>() as usize,
                    );
                    if let Some(t) = info.object_threads {
                        d.setRequiredThreadsPerObjectThreadgroup(size(t));
                        d.setMaxTotalThreadsPerObjectThreadgroup(t.iter().product::<u32>() as usize);
                        d.setPayloadMemoryLength(info.payload_bytes as usize);
                    }
                    // SAFETY: a sample count Metal checks at compile.
                    unsafe { d.setRasterSampleCount(desc.sample_count as usize) };
                    d.setAlphaToCoverageState(if desc.alpha_to_coverage {
                        MTL4AlphaToCoverageState::Enabled
                    } else {
                        MTL4AlphaToCoverageState::Disabled
                    });
                    color_attachments(&d.colorAttachments(), &desc.targets);
                    compile_mesh(compiler, &d)
                }
            };
        let raw = compiled?;
        for (stage, c) in [
            (ShaderStage::Vertex, &prepared.vs),
            (ShaderStage::Amplification, &prepared.amp),
            (ShaderStage::Mesh, &prepared.ms),
            (ShaderStage::Geometry, &prepared.gs),
            (ShaderStage::Fragment, &prepared.ps),
        ] {
            if let Some(c) = c {
                reflections.push((stage, c.reflection.clone()));
            }
        }

        let ds = MTLDepthStencilDescriptor::new();
        ds.setDepthCompareFunction(desc.depth_stencil.depth_compare);
        ds.setDepthWriteEnabled(desc.depth_stencil.depth_write);
        if desc.depth_stencil.stencil {
            ds.setFrontFaceStencil(Some(&stencil_descriptor(
                &desc.depth_stencil.front,
                &desc.depth_stencil,
            )));
            ds.setBackFaceStencil(Some(&stencil_descriptor(
                &desc.depth_stencil.back,
                &desc.depth_stencil,
            )));
        }
        let depth_stencil = self
            .inner
            .raw
            .newDepthStencilStateWithDescriptor(&ds)
            .ok_or_else(|| MscError::Metal(Error::Metal("depth-stencil state".into())))?;

        let allocation: &ProtocolObject<dyn MTLAllocation> = ProtocolObject::from_ref(&*raw);
        self.inner.residency.add(SetKind::Resource, allocation);
        Ok(GraphicsPipeline {
            inner: Arc::new(GraphicsInner {
                raw,
                depth_stencil,
                device: self.inner.clone(),
                kind: prepared.kind,
                raster: desc.raster,
                topology: desc.topology,
                samples: desc.sample_count,
                layout: prepared.layout.clone(),
                mesh: prepared.mesh,
                geometry: prepared.geometry,
                vertex_slots,
                needs_draw_params,
                reflections,
            }),
        })
    }

    /// DXIL stages to a pipeline: [`prepare_graphics`] then
    /// [`Device::graphics_pipeline`]. This is the entry for worker threads.
    pub fn graphics_pipeline_from_dxil(
        &self,
        desc: &GraphicsDesc<'_>,
    ) -> Result<GraphicsPipeline, MscError> {
        let prepared = prepare_graphics(desc)?;
        self.graphics_pipeline(&prepared, desc)
    }
}

/// The compiler call of vertex pipelines: the one place to make asynchronous
/// or to look an archive up.
fn compile_render(
    compiler: &ProtocolObject<dyn MTL4Compiler>,
    d: &MTL4RenderPipelineDescriptor,
    what: &str,
) -> Result<Retained<ProtocolObject<dyn MTLRenderPipelineState>>, MscError> {
    let base: &MTL4PipelineDescriptor = d;
    compiler
        .newRenderPipelineStateWithDescriptor_compilerTaskOptions_error(
            base,
            crate::archive::task_options().as_deref(),
        )
        .map_err(|e| metal_refusal(what, &e.localizedDescription().to_string()))
}

/// The compiler call of mesh pipelines (see [`compile_render`]).
fn compile_mesh(
    compiler: &ProtocolObject<dyn MTL4Compiler>,
    d: &MTL4MeshRenderPipelineDescriptor,
) -> Result<Retained<ProtocolObject<dyn MTLRenderPipelineState>>, MscError> {
    let base: &MTL4PipelineDescriptor = d;
    compiler
        .newRenderPipelineStateWithDescriptor_compilerTaskOptions_error(
            base,
            crate::archive::task_options().as_deref(),
        )
        .map_err(|e| metal_refusal("mesh pipeline", &e.localizedDescription().to_string()))
}

/// Metal's refusal of a pipeline, told apart when it is a size limit (the
/// text names the limit and the amount, e.g. mesh output memory) from the
/// rest.
fn metal_refusal(what: &str, text: &str) -> MscError {
    let lower = text.to_ascii_lowercase();
    if ["exceed", "too large", "limit", "maximum"]
        .iter()
        .any(|k| lower.contains(k))
    {
        MscError::Limit(format!("{what} over a Metal limit: {text}"))
    } else {
        MscError::Metal(Error::Metal(format!("{what}: {text}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_limit_texts_are_told_apart() {
        assert!(matches!(
            metal_refusal(
                "mesh pipeline",
                "mesh output size 34816 exceeds the limit 28672"
            ),
            MscError::Limit(_)
        ));
        assert!(matches!(
            metal_refusal("vertex pipeline", "invalid vertex descriptor"),
            MscError::Metal(_)
        ));
    }

    fn reflection(threads: [u32; 3], payload: u32, v: u32, p: u32) -> Reflection {
        Reflection {
            entry: "main".into(),
            stage: ShaderStage::Mesh,
            threadgroup_size: [0; 3],
            mesh: Some(crate::msc::MeshInfo {
                num_threads: threads,
                max_payload_bytes: payload,
                max_vertices: v,
                max_primitives: p,
            }),
            vertex: None,
            fragment: None,
            geometry: None,
            layout: TopLevelLayout {
                entries: vec![],
                static_sampler_table: None,
                size: 0,
            },
            uses_resource_heap: false,
            uses_sampler_heap: false,
            function_constants: vec![],
            dxil_features: None,
        }
    }

    #[test]
    fn mesh_limits_name_the_number_and_the_limit() {
        let ok = mesh_info(reflection([32, 1, 1], 0, 64, 126), None).unwrap();
        assert_eq!(ok.mesh_threads, [32, 1, 1]);
        assert_eq!(ok.object_threads, None);
        for (r, what) in [
            (
                reflection([2048, 1, 1], 0, 64, 64),
                "mesh threads per group is 2048",
            ),
            (
                reflection([32, 1, 1], 0, 300, 64),
                "mesh output vertices is 300",
            ),
            (
                reflection([32, 1, 1], 0, 64, 600),
                "mesh output primitives is 600",
            ),
            (reflection([32, 1, 1], 20000, 64, 64), "payload is 20000"),
        ] {
            let e = mesh_info(r, None).unwrap_err();
            assert!(matches!(e, MscError::Limit(_)), "{e:?}");
            assert!(e.to_string().contains(what), "{e}");
        }
        let amp = reflection([64, 1, 1], 4096, 0, 0);
        let m = mesh_info(reflection([32, 1, 1], 4096, 64, 64), Some(&amp)).unwrap();
        assert_eq!(m.object_threads, Some([64, 1, 1]));
        assert_eq!(m.payload_bytes, 4096);
    }

    #[test]
    fn msc_options_follow_the_pipeline() {
        let mut blend = TargetBlend::opaque();
        blend.enabled = true;
        blend.src_rgb = MTLBlendFactor::Source1Color;
        let int_target = ColorTarget {
            format: MTLPixelFormat::R32Uint,
            integer: true,
            blend: TargetBlend::opaque(),
        };
        let float_target = ColorTarget {
            format: MTLPixelFormat::RGBA8Unorm,
            integer: false,
            blend,
        };
        let root = RootSignatureDesc::default();
        let desc = GraphicsDesc {
            vs: None,
            ps: None,
            amp: None,
            ms: None,
            gs: None,
            root: &root,
            inputs: vec![],
            targets: vec![float_target, int_target],
            topology: MTLPrimitiveTopologyClass::Point,
            sample_count: 1,
            sample_mask: 0xF,
            alpha_to_coverage: false,
            has_depth_target: true,
            depth_stencil: DepthStencil {
                depth_compare: MTLCompareFunction::Always,
                depth_write: false,
                stencil: false,
                front: StencilFace {
                    fail: MTLStencilOperation::Keep,
                    depth_fail: MTLStencilOperation::Keep,
                    pass: MTLStencilOperation::Keep,
                    compare: MTLCompareFunction::Always,
                },
                back: StencilFace {
                    fail: MTLStencilOperation::Keep,
                    depth_fail: MTLStencilOperation::Keep,
                    pass: MTLStencilOperation::Keep,
                    compare: MTLCompareFunction::Always,
                },
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
        };
        let g = desc.msc_options().graphics.unwrap();
        assert_eq!(g.input_topology, InputTopology::Point);
        assert_eq!(g.int_rt_mask, 0b10);
        assert_eq!(g.sample_mask, 0xF);
        assert!(g.dual_source);
        assert!(g.depth_feedback);
    }
}
