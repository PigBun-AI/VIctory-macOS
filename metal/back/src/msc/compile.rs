//! DXIL to `metallib`, with reflection.

use std::ffi::CString;
use std::ptr;

use super::root::{ArgKind, ArgLocation, CRootSignature, TopLevelLayout};
use super::{ErrorCode, Msc, MscError, RootSignatureDesc, bounded_c_str, dxil, ffi};

/// The cap on an error payload we read (the library's own are far shorter).
const PAYLOAD_MAX: usize = 64 * 1024;

/// `IRCompatibilityFlags`: behaviours MSC can add to match D3D12.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Compat(pub u32);

impl Compat {
    pub const NONE: Compat = Compat(0);
    /// Bounds-check buffer and texture accesses (D3D12 returns 0 / drops writes).
    pub const BOUNDS_CHECK: Compat = Compat(ffi::compat::BOUNDS_CHECK);
    pub const VERTEX_POSITION_INF_TO_NAN: Compat = Compat(ffi::compat::VERTEX_POSITION_INF_TO_NAN);
    pub const TEXTURE_MIN_LOD_CLAMP: Compat = Compat(ffi::compat::TEXTURE_MIN_LOD_CLAMP);
    pub const SAMPLER_LOD_BIAS: Compat = Compat(ffi::compat::SAMPLER_LOD_BIAS);
    pub const POSITION_INVARIANCE: Compat = Compat(ffi::compat::POSITION_INVARIANCE);
    pub const SAMPLE_NAN_TO_ZERO: Compat = Compat(ffi::compat::SAMPLE_NAN_TO_ZERO);
    pub const TEX_WRITE_ROUNDING_RTZ: Compat = Compat(ffi::compat::TEX_WRITE_ROUNDING_RTZ);
    pub const SUPPRESS_2D_COMPUTE_DERIVATIVE_ERRORS: Compat =
        Compat(ffi::compat::SUPPRESS_2D_COMPUTE_DERIVATIVE_ERRORS);
    pub const FORCE_TEXTURE_ARRAY: Compat = Compat(ffi::compat::FORCE_TEXTURE_ARRAY);
    pub const DISABLE_NAN_INF_OPTIMIZATION: Compat =
        Compat(ffi::compat::DISABLE_NAN_INF_OPTIMIZATION);
    pub const ENSURE_TG_MEMORY_COHERENCY: Compat = Compat(ffi::compat::ENSURE_TG_MEMORY_COHERENCY);
}

impl std::ops::BitOr for Compat {
    type Output = Compat;
    fn bitor(self, o: Compat) -> Compat {
        Compat(self.0 | o.0)
    }
}

/// How to convert.
#[derive(Clone, Debug, Default)]
pub struct CompileOptions {
    /// Entry point name; `None` lets MSC take the shader's own (`main`).
    pub entry: Option<String>,
    pub compat: Compat,
    /// `IRGPUFamily` to target at least (`None`: MSC's default).
    pub min_gpu_family: Option<u32>,
    /// Turn MSC's own validation off (`IRCompilerSetValidationFlags(None)`):
    /// faster, and an invalid input is then undefined.
    pub no_validation: bool,
    /// What a vertex, fragment, mesh or amplification shader needs told about
    /// the pipeline it joins (`None` for compute: nothing is set).
    pub graphics: Option<GraphicsCompile>,
}

/// The primitive class a vertex shader is converted for
/// (`IRCompilerSetInputTopology`; points need it to get `[[point_size]]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum InputTopology {
    #[default]
    Undefined,
    Point,
    Line,
    Triangle,
}

/// Pipeline facts MSC bakes into the converted code. The same converted
/// shader is only valid for the pipelines that agree with these, so they are
/// part of what a shader cache keys on.
///
/// Vertex input uses Metal vertex fetch (`IRStageInGenerationMode` =
/// `UseMetalVertexFetch`, set for every graphics conversion): the pipeline's
/// `MTLVertexDescriptor` feeds the shader, see `render.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphicsCompile {
    pub input_topology: InputTopology,
    /// Bit `i` set: render target `i` has an integer format (the converter
    /// inserts the bitcast of the shader's output).
    pub int_rt_mask: u8,
    /// Fragment output sample mask (all samples: `u32::MAX`).
    pub sample_mask: u32,
    /// The pipeline blends with a second fragment output (`SRC1_*` factors).
    pub dual_source: bool,
    /// The pipeline has a depth target the fragment shader may write
    /// (`SV_Depth`): the converter emits the depth output.
    pub depth_feedback: bool,
}

impl Default for GraphicsCompile {
    fn default() -> Self {
        GraphicsCompile {
            input_topology: InputTopology::Undefined,
            int_rt_mask: 0,
            sample_mask: u32::MAX,
            dual_source: false,
            depth_feedback: false,
        }
    }
}

/// The stage of the converted shader (`IRShaderStage`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ShaderStage {
    Vertex,
    Fragment,
    Hull,
    Domain,
    Mesh,
    Amplification,
    Geometry,
    Compute,
    Other(u32),
}

impl ShaderStage {
    fn from_ir(v: u32) -> ShaderStage {
        use ffi::stage as s;
        match v {
            s::VERTEX => ShaderStage::Vertex,
            s::FRAGMENT => ShaderStage::Fragment,
            s::HULL => ShaderStage::Hull,
            s::DOMAIN => ShaderStage::Domain,
            s::MESH => ShaderStage::Mesh,
            s::AMPLIFICATION => ShaderStage::Amplification,
            s::GEOMETRY => ShaderStage::Geometry,
            s::COMPUTE => ShaderStage::Compute,
            x => ShaderStage::Other(x),
        }
    }
}

/// A function constant the metallib asks for at pipeline creation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionConstant {
    pub name: String,
    /// `IRFunctionConstantType` value.
    pub ty: u32,
}

/// Mesh / amplification shader facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshInfo {
    pub num_threads: [u32; 3],
    pub max_payload_bytes: u32,
    /// Mesh stage only.
    pub max_vertices: u32,
    /// Mesh stage only.
    pub max_primitives: u32,
}

/// One vertex shader input: the name MSC reflects and the attribute index
/// the `MTLVertexDescriptor` must use for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VertexInput {
    pub name: String,
    /// Index into the vertex descriptor's `attributes`.
    pub attribute_index: u32,
}

/// Vertex shader facts (`IRVSInfo_1_0`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VertexInfo {
    pub inputs: Vec<VertexInput>,
    /// Index of the `SV_VertexID` input, `-1` for none.
    pub vertex_id_index: i32,
    /// Index of the `SV_InstanceID` input, `-1` for none.
    pub instance_id_index: i32,
    /// The shader reads draw parameters (base vertex, base instance) that the
    /// draw must supply.
    pub needs_draw_params: bool,
    pub output_bytes: u32,
}

/// Fragment shader facts (`IRFSInfo_1_0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FragmentInfo {
    pub num_render_targets: i32,
    pub rt_index_int: u8,
    pub discards: bool,
}

/// What a converted shader needs from the host, from MSC's reflection and
/// the DXIL container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reflection {
    /// The function name inside the metallib.
    pub entry: String,
    pub stage: ShaderStage,
    /// Threadgroup size (compute; the mesh and amplification shaders'
    /// `num_threads` are in [`Reflection::mesh`]).
    pub threadgroup_size: [u32; 3],
    pub mesh: Option<MeshInfo>,
    /// Vertex stage only.
    pub vertex: Option<VertexInfo>,
    /// Fragment stage only.
    pub fragment: Option<FragmentInfo>,
    /// The top-level argument buffer: one entry per root parameter with its
    /// byte offset and size (from the root signature when one was given,
    /// otherwise from the shader's own).
    pub layout: TopLevelLayout,
    /// The shader indexes `ResourceDescriptorHeap[]` (SFI0 bit
    /// `RESOURCE_DESCRIPTOR_HEAP_INDEXING`): the resource heap must be bound.
    pub uses_resource_heap: bool,
    /// The shader indexes `SamplerDescriptorHeap[]`.
    pub uses_sampler_heap: bool,
    pub function_constants: Vec<FunctionConstant>,
    /// MSC's `ResourceUsages` / feature word, kept for diagnostics.
    pub dxil_features: Option<u64>,
}

/// The result of one conversion.
#[derive(Clone, Debug)]
pub struct Converted {
    /// A `metallib` image.
    pub metallib: Vec<u8>,
    pub reflection: Reflection,
}

// ---- RAII wrappers ----------------------------------------------------------

/// Reads and releases an `IRError` taken out of an out-parameter.
fn take_error(msc: &Msc, p: *mut ffi::IRError) -> (ErrorCode, String) {
    if p.is_null() {
        return (ErrorCode(ffi::error_code::UNKNOWN), String::new());
    }
    // SAFETY: a live error from the library, released once, after the
    // payload is copied out.
    unsafe {
        let code = (msc.api.IRErrorGetCode)(p);
        let message = bounded_c_str((msc.api.IRErrorGetPayload)(p), PAYLOAD_MAX);
        (msc.api.IRErrorDestroy)(p);
        (ErrorCode(code), message)
    }
}

/// An `IRRootSignature`.
pub(crate) struct RootSig {
    api: ffi::Api,
    raw: *mut ffi::IRRootSignature,
}

impl RootSig {
    pub(crate) fn create(msc: &Msc, c: &CRootSignature) -> Result<RootSig, MscError> {
        let mut err = ptr::null_mut();
        // SAFETY: the descriptor points into `c`, alive for the call.
        let raw = unsafe { (msc.api.IRRootSignatureCreateFromDescriptor)(&c.desc, &mut err) };
        if raw.is_null() {
            let (code, message) = take_error(msc, err);
            return Err(MscError::RootSignature { code, message });
        }
        Ok(RootSig { api: msc.api, raw })
    }

    pub(crate) fn layout(&self, params: usize, static_samplers: usize) -> TopLevelLayout {
        // SAFETY: a live root signature; the buffer has `n` slots.
        let n = unsafe { (self.api.IRRootSignatureGetResourceCount)(self.raw) };
        let mut locs = vec![ffi::IRResourceLocation::default(); n];
        if n > 0 {
            // SAFETY: as above.
            unsafe { (self.api.IRRootSignatureGetResourceLocations)(self.raw, locs.as_mut_ptr()) };
        }
        TopLevelLayout::from_locations(locations(&locs), params, static_samplers)
    }
}

impl Drop for RootSig {
    fn drop(&mut self) {
        // SAFETY: released once.
        unsafe { (self.api.IRRootSignatureDestroy)(self.raw) };
    }
}

fn locations(locs: &[ffi::IRResourceLocation]) -> Vec<ArgLocation> {
    locs.iter()
        .map(|l| ArgLocation {
            kind: ArgKind::from_ir(l.resourceType),
            space: l.space,
            slot: l.slot,
            offset: l.topLevelOffset,
            size: l.sizeBytes,
        })
        .collect()
}

struct Compiler {
    api: ffi::Api,
    raw: *mut ffi::IRCompiler,
}
impl Drop for Compiler {
    fn drop(&mut self) {
        // SAFETY: released once.
        unsafe { (self.api.IRCompilerDestroy)(self.raw) };
    }
}

struct Object {
    api: ffi::Api,
    raw: *mut ffi::IRObject,
}
impl Drop for Object {
    fn drop(&mut self) {
        // SAFETY: released once.
        unsafe { (self.api.IRObjectDestroy)(self.raw) };
    }
}

struct Lib {
    api: ffi::Api,
    raw: *mut ffi::IRMetalLibBinary,
}
impl Drop for Lib {
    fn drop(&mut self) {
        // SAFETY: released once.
        unsafe { (self.api.IRMetalLibBinaryDestroy)(self.raw) };
    }
}

struct Refl {
    api: ffi::Api,
    raw: *mut ffi::IRShaderReflection,
}
impl Drop for Refl {
    fn drop(&mut self) {
        // SAFETY: released once.
        unsafe { (self.api.IRShaderReflectionDestroy)(self.raw) };
    }
}

impl Msc {
    /// Converts one DXIL container to a `metallib`.
    ///
    /// `root` is the global root signature (the shader's own, embedded one is
    /// used when it is `None`; a shader without either is refused with
    /// `ShaderRequiresRootSignature`). A fresh `IRCompiler` is made per call:
    /// the library says one is not reentrant, so calls from several threads
    /// are safe.
    pub fn compile(
        &self,
        dxil_bytes: &[u8],
        root: Option<&RootSignatureDesc>,
        options: &CompileOptions,
    ) -> Result<Converted, MscError> {
        if dxil_bytes.is_empty() {
            return Err(MscError::BadInput("empty DXIL".into()));
        }
        let info = dxil::inspect(dxil_bytes);
        let api = self.api;
        let c_root = root.map(CRootSignature::new);
        // Declared before the compiler: dropped after it, since the compiler
        // must not outlive the root signature it points at.
        let rs = match &c_root {
            Some(c) => Some(RootSig::create(self, c)?),
            None => None,
        };
        // SAFETY: plain constructor.
        let raw = unsafe { (api.IRCompilerCreate)() };
        if raw.is_null() {
            return Err(MscError::NoOutput("IRCompilerCreate returned null".into()));
        }
        let compiler = Compiler { api, raw };
        // SAFETY: a live compiler and root signature.
        unsafe {
            if let Some(rs) = &rs {
                (api.IRCompilerSetGlobalRootSignature)(compiler.raw, rs.raw);
            }
            if options.no_validation {
                (api.IRCompilerSetValidationFlags)(compiler.raw, ffi::validation::NONE);
            }
            (api.IRCompilerSetCompatibilityFlags)(compiler.raw, options.compat.0);
            if let Some(f) = options.min_gpu_family {
                (api.IRCompilerSetMinimumGPUFamily)(compiler.raw, f);
            }
            if let Some(g) = &options.graphics {
                use ffi::{feature_config as fc, input_topology as it};
                (api.IRCompilerSetStageInGenerationMode)(
                    compiler.raw,
                    ffi::STAGE_IN_METAL_VERTEX_FETCH,
                );
                (api.IRCompilerSetInputTopology)(
                    compiler.raw,
                    match g.input_topology {
                        InputTopology::Undefined => it::UNDEFINED,
                        InputTopology::Point => it::POINT,
                        InputTopology::Line => it::LINE,
                        InputTopology::Triangle => it::TRIANGLE,
                    },
                );
                (api.IRCompilerSetIntRTMask)(compiler.raw, g.int_rt_mask);
                (api.IRCompilerSetSampleMask)(compiler.raw, g.sample_mask);
                let on = |b: bool| {
                    if b {
                        fc::FORCE_ENABLED
                    } else {
                        fc::FORCE_DISABLED
                    }
                };
                (api.IRCompilerSetDualSourceBlendingConfiguration)(compiler.raw, on(g.dual_source));
                (api.IRCompilerSetDepthFeedbackConfiguration)(compiler.raw, on(g.depth_feedback));
            }
        }
        let entry = match &options.entry {
            Some(e) => Some(
                CString::new(e.as_str())
                    .map_err(|_| MscError::BadInput("entry name has a NUL".into()))?,
            ),
            None => None,
        };
        if let Some(e) = &entry {
            // SAFETY: the name is copied by the library.
            unsafe { (api.IRCompilerSetEntryPointName)(compiler.raw, e.as_ptr()) };
        }
        // SAFETY: copies the bytes (`Copy` ownership).
        let input = unsafe {
            (api.IRObjectCreateFromDXIL)(dxil_bytes.as_ptr(), dxil_bytes.len(), ffi::BYTECODE_COPY)
        };
        if input.is_null() {
            return Err(MscError::NoOutput(
                "IRObjectCreateFromDXIL returned null".into(),
            ));
        }
        let input = Object { api, raw: input };
        let mut err = ptr::null_mut();
        // SAFETY: live compiler and input; null entry point name = the shader's own.
        let out = unsafe {
            (api.IRCompilerAllocCompileAndLink)(compiler.raw, ptr::null(), input.raw, &mut err)
        };
        if out.is_null() {
            let (code, message) = take_error(self, err);
            return Err(if code.0 == ffi::error_code::NO_ERROR {
                MscError::NoOutput("compile returned null without an error".into())
            } else {
                MscError::Rejected { code, message }
            });
        }
        let out = Object { api, raw: out };
        // SAFETY: a live output object.
        let stage_raw = unsafe { (api.IRObjectGetMetalIRShaderStage)(out.raw) };
        let stage = ShaderStage::from_ir(stage_raw);

        // The metallib.
        // SAFETY: plain constructor.
        let lib = unsafe { (api.IRMetalLibBinaryCreate)() };
        if lib.is_null() {
            return Err(MscError::NoOutput(
                "IRMetalLibBinaryCreate returned null".into(),
            ));
        }
        let lib = Lib { api, raw: lib };
        // SAFETY: live objects.
        let ok = unsafe { (api.IRObjectGetMetalLibBinary)(out.raw, stage_raw, lib.raw) };
        // SAFETY: as above.
        let size = unsafe { (api.IRMetalLibGetBytecodeSize)(lib.raw) };
        if !ok || size == 0 {
            return Err(MscError::NoOutput("no metallib for the stage".into()));
        }
        let mut metallib = vec![0u8; size];
        // SAFETY: the buffer is `size` bytes.
        let written = unsafe { (api.IRMetalLibGetBytecode)(lib.raw, metallib.as_mut_ptr()) };
        if written != size {
            return Err(MscError::NoOutput(format!(
                "metallib copy wrote {written} of {size} bytes"
            )));
        }

        // Reflection.
        // SAFETY: plain constructor.
        let refl = unsafe { (api.IRShaderReflectionCreate)() };
        if refl.is_null() {
            return Err(MscError::NoOutput(
                "IRShaderReflectionCreate returned null".into(),
            ));
        }
        let refl = Refl { api, raw: refl };
        // SAFETY: live objects.
        if !unsafe { (api.IRObjectGetReflection)(out.raw, stage_raw, refl.raw) } {
            return Err(MscError::NoOutput("no reflection for the stage".into()));
        }
        // SAFETY: a live reflection.
        let name =
            unsafe { ffi::c_str((api.IRShaderReflectionGetEntryPointFunctionName)(refl.raw)) };

        let mut threadgroup_size = [0; 3];
        let mut mesh = None;
        let mut vertex = None;
        let mut fragment = None;
        // SAFETY: each Copy*Info fills a struct this function owns; Release*
        // frees what the library attached.
        unsafe {
            match stage {
                ShaderStage::Compute => {
                    let mut cs = ffi::IRVersionedCSInfo::default();
                    if (api.IRShaderReflectionCopyComputeInfo)(
                        refl.raw,
                        ffi::REFLECTION_VERSION_1_0,
                        &mut cs,
                    ) {
                        threadgroup_size = cs.tg_size;
                        (api.IRShaderReflectionReleaseComputeInfo)(&mut cs);
                    }
                }
                ShaderStage::Mesh => {
                    let mut ms = ffi::IRVersionedMSInfo::default();
                    if (api.IRShaderReflectionCopyMeshInfo)(
                        refl.raw,
                        ffi::REFLECTION_VERSION_1_0,
                        &mut ms,
                    ) {
                        mesh = Some(MeshInfo {
                            num_threads: ms.num_threads,
                            max_payload_bytes: ms.max_payload_size_in_bytes,
                            max_vertices: ms.max_vertex_output_count,
                            max_primitives: ms.max_primitive_output_count,
                        });
                        (api.IRShaderReflectionReleaseMeshInfo)(&mut ms);
                    }
                }
                ShaderStage::Amplification => {
                    let mut asi = ffi::IRVersionedASInfo::default();
                    if (api.IRShaderReflectionCopyAmplificationInfo)(
                        refl.raw,
                        ffi::REFLECTION_VERSION_1_0,
                        &mut asi,
                    ) {
                        mesh = Some(MeshInfo {
                            num_threads: asi.num_threads,
                            max_payload_bytes: asi.max_payload_size_in_bytes,
                            max_vertices: 0,
                            max_primitives: 0,
                        });
                        (api.IRShaderReflectionReleaseAmplificationInfo)(&mut asi);
                    }
                }
                ShaderStage::Vertex => {
                    let mut vs = ffi::IRVersionedVSInfo::default();
                    if (api.IRShaderReflectionCopyVertexInfo)(
                        refl.raw,
                        ffi::REFLECTION_VERSION_1_0,
                        &mut vs,
                    ) {
                        let inputs = if vs.vertex_inputs.is_null() {
                            Vec::new()
                        } else {
                            std::slice::from_raw_parts(vs.vertex_inputs, vs.num_vertex_inputs)
                                .iter()
                                .map(|i| VertexInput {
                                    name: ffi::c_str(i.name),
                                    attribute_index: u32::from(i.attributeIndex),
                                })
                                .collect()
                        };
                        vertex = Some(VertexInfo {
                            inputs,
                            vertex_id_index: vs.vertex_id_index,
                            instance_id_index: vs.instance_id_index,
                            needs_draw_params: vs.needs_draw_params,
                            output_bytes: vs.vertex_output_size_in_bytes,
                        });
                        (api.IRShaderReflectionReleaseVertexInfo)(&mut vs);
                    }
                }
                ShaderStage::Fragment => {
                    let mut fs = ffi::IRVersionedFSInfo::default();
                    if (api.IRShaderReflectionCopyFragmentInfo)(
                        refl.raw,
                        ffi::REFLECTION_VERSION_1_0,
                        &mut fs,
                    ) {
                        fragment = Some(FragmentInfo {
                            num_render_targets: fs.num_render_targets,
                            rt_index_int: fs.rt_index_int,
                            discards: fs.discards,
                        });
                        (api.IRShaderReflectionReleaseFragmentInfo)(&mut fs);
                    }
                }
                _ => {}
            }
        }

        // Function constants.
        let mut function_constants = Vec::new();
        // SAFETY: a live reflection; the array is released with the library's call.
        unsafe {
            if (api.IRShaderReflectionNeedsFunctionConstants)(refl.raw) {
                let n = (api.IRShaderReflectionGetFunctionConstantCount)(refl.raw);
                let mut v = vec![
                    ffi::IRFunctionConstant {
                        name: ptr::null(),
                        type_: 0
                    };
                    n
                ];
                if n > 0 {
                    (api.IRShaderReflectionCopyFunctionConstants)(refl.raw, v.as_mut_ptr());
                    for f in &v {
                        function_constants.push(FunctionConstant {
                            name: ffi::c_str(f.name),
                            ty: f.type_,
                        });
                    }
                    (api.IRShaderReflectionReleaseFunctionConstants)(v.as_mut_ptr(), n);
                }
            }
        }

        // The layout: the root signature's, or else what the shader reports.
        let layout = match &rs {
            Some(rs) => rs.layout(
                root.map_or(0, |r| r.params.len()),
                root.map_or(0, |r| r.static_samplers.len()),
            ),
            None => {
                // SAFETY: a live reflection.
                let n = unsafe { (api.IRShaderReflectionGetResourceCount)(refl.raw) };
                let mut locs = vec![ffi::IRResourceLocation::default(); n];
                if n > 0 {
                    // SAFETY: `n` slots.
                    unsafe {
                        (api.IRShaderReflectionGetResourceLocations)(refl.raw, locs.as_mut_ptr())
                    };
                }
                let n = locs.len();
                TopLevelLayout::from_locations(locations(&locs), n, 0)
            }
        };

        let features = info.and_then(|i| i.features);
        let reflection = Reflection {
            entry: name,
            stage,
            threadgroup_size,
            mesh,
            vertex,
            fragment,
            layout,
            uses_resource_heap: features
                .is_some_and(|f| f & dxil::feature::RESOURCE_DESCRIPTOR_HEAP_INDEXING != 0),
            uses_sampler_heap: features
                .is_some_and(|f| f & dxil::feature::SAMPLER_DESCRIPTOR_HEAP_INDEXING != 0),
            function_constants,
            dxil_features: features,
        };
        Ok(Converted {
            metallib,
            reflection,
        })
    }
}
