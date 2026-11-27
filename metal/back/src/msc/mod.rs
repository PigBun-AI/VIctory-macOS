//! Metal Shader Converter (MSC) in the backend: DXIL in, `metallib` and
//! reflection out, and the entry point that turns both into a compute
//! pipeline ([`Device::compute_pipeline_from_dxil`]).
//!
//! MSC 4.0.3 (`IR_VERSION` 4.0.3, the installed
//! `metal-shaderconverter` 4.0.3, checked 2026-11-27) is a universal
//! (x86_64 + arm64) `libmetalirconverter.dylib`, so the same code links in
//! the arm64 process and in the x86_64 (Rosetta / Wine unixlib) one. It is
//! **loaded with `dlopen` at the first use** ([`Msc::get`]): a machine
//! without MSC still builds and runs the rest of the backend, and gets
//! [`MscError::Library`] with the paths tried and what to install instead
//! of a loader abort. Search order: `VICTORY_MSC_DYLIB` (only that, when
//! set), `/usr/local/lib/libmetalirconverter.dylib` (where the pkg installs
//! it), then the bare name (dyld's search path). The library's install name
//! is `@rpath/libmetalirconverter.dylib`, so a link-time dependency would
//! need an rpath; `dlopen` by path needs none.
//!
//! Layering: this is the only place MSC types exist (rule 3). Callers pass
//! plain Rust values ([`RootSignatureDesc`], [`CompileOptions`]) and get
//! plain Rust values back ([`Converted`], [`Reflection`]).
//!
//! # Metal 4
//!
//! `IR_RUNTIME_METAL4` is a switch of the *runtime companion header*
//! (`metal_irconverter_runtime.h`): it only enables the draw helpers
//! `IRRuntime4Draw*Emulation` (geometry / tessellation emulation through an
//! `MTL4ArgumentTable`; they bind the scratch buffer at
//! `kIRArgumentBufferDrawArgumentsBindPoint` and
//! `kIRArgumentBufferUniformsBindPoint`). The converter itself has no
//! "Metal 4" option and the compute path is the same: the top-level
//! argument buffer address goes into the argument table at index
//! [`bind_point::ARGUMENT_BUFFER`](crate::descriptors::bind_point) (2), the
//! resource heap at 0, the sampler heap at 1. This crate does not use the
//! runtime header (it is C/Objective-C and inline); the encodings it defines are
//! in [`descriptors`](crate::descriptors).

mod compile;
pub mod dxbc;
pub mod dxil;
mod ffi;
mod pipeline;
mod root;

pub use compile::{
    Compat, CompileOptions, Converted, FragmentInfo, FunctionConstant, GeometryInfo,
    GraphicsCompile, InputTopology, MeshInfo, Reflection, ShaderStage, StageInElement, VertexInfo,
    VertexInput,
};
pub use pipeline::ShaderPipeline;
pub use root::{
    ArgKind, ArgLocation, OFFSET_APPEND, Param, ParamKind, Range, RangeKind, RootSignatureDesc,
    StaticSampler, TopLevelLayout, UNBOUNDED, Visibility, flags,
};

use std::ffi::{CStr, CString, c_char, c_void};
use std::fmt;
use std::sync::OnceLock;

/// The header version this crate's FFI was written from.
pub const HEADER_VERSION: (u32, u32, u32) = (4, 0, 3);

/// Where the pkg installs the library.
pub const DEFAULT_DYLIB: &str = "/usr/local/lib/libmetalirconverter.dylib";
/// Environment variable naming the library to load instead.
pub const DYLIB_ENV: &str = "VICTORY_MSC_DYLIB";

/// `IRErrorCode` of a refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ErrorCode(pub u32);

impl ErrorCode {
    /// The header's name without the `IRErrorCode` prefix.
    pub fn name(self) -> &'static str {
        use ffi::error_code as c;
        match self.0 {
            c::NO_ERROR => "NoError",
            c::SHADER_REQUIRES_ROOT_SIGNATURE => "ShaderRequiresRootSignature",
            c::UNRECOGNIZED_ROOT_SIGNATURE_DESCRIPTOR => "UnrecognizedRootSignatureDescriptor",
            c::UNRECOGNIZED_PARAMETER_TYPE_IN_ROOT_SIGNATURE => {
                "UnrecognizedParameterTypeInRootSignature"
            }
            c::RESOURCE_NOT_REFERENCED_BY_ROOT_SIGNATURE => "ResourceNotReferencedByRootSignature",
            c::SHADER_INCOMPATIBLE_WITH_DUAL_SOURCE_BLENDING => {
                "ShaderIncompatibleWithDualSourceBlending"
            }
            c::UNSUPPORTED_WAVE_SIZE => "UnsupportedWaveSize",
            c::UNSUPPORTED_INSTRUCTION => "UnsupportedInstruction",
            c::COMPILATION_ERROR => "CompilationError",
            c::FAILED_TO_SYNTHESIZE_STAGE_IN_FUNCTION => "FailedToSynthesizeStageInFunction",
            c::FAILED_TO_SYNTHESIZE_STREAM_OUT_FUNCTION => "FailedToSynthesizeStreamOutFunction",
            c::FAILED_TO_SYNTHESIZE_INDIRECT_INTERSECTION_FUNCTION => {
                "FailedToSynthesizeIndirectIntersectionFunction"
            }
            c::UNABLE_TO_VERIFY_MODULE => "UnableToVerifyModule",
            c::UNABLE_TO_LINK_MODULE => "UnableToLinkModule",
            c::UNRECOGNIZED_DXIL_HEADER => "UnrecognizedDXILHeader",
            c::INVALID_RAYTRACING_ATTRIBUTE => "InvalidRaytracingAttribute",
            c::NULL_HULL_SHADER_INPUT_OUTPUT_MISMATCH => "NullHullShaderInputOutputMismatch",
            c::INVALID_RAYTRACING_USER_ATTRIBUTE_SIZE => "InvalidRaytracingUserAttributeSize",
            c::INCORRECT_HITGROUP_TYPE => "IncorrectHitgroupType",
            c::FP64_USAGE => "FP64Usage",
            c::UNKNOWN => "Unknown",
            _ => "UnlistedCode",
        }
    }
}

/// What can go wrong between DXIL and a pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MscError {
    /// `libmetalirconverter.dylib` could not be loaded (not installed, the
    /// wrong architecture, or a symbol this crate needs is missing).
    Library { tried: Vec<String>, detail: String },
    /// A root signature MSC refused.
    RootSignature { code: ErrorCode, message: String },
    /// MSC refused the shader.
    Rejected { code: ErrorCode, message: String },
    /// MSC returned nothing and no error.
    NoOutput(String),
    /// A caller value is unusable (empty DXIL, a name with a NUL ...).
    BadInput(String),
    /// MSC converted it but Metal did not take the result.
    Metal(crate::Error),
    /// The converted stages of one pipeline do not fit together (argument
    /// buffer layouts differ, the pixel shader writes more targets than the
    /// pipeline has).
    Mismatch(String),
    /// A size Metal limits (mesh output, payload, threads): the text names
    /// the number and the limit.
    Limit(String),
    /// State or a shader feature this back does not make.
    Unsupported(String),
}

impl fmt::Display for MscError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MscError::Library { tried, detail } => write!(
                f,
                "Metal Shader Converter library not available ({detail}); tried: {}. \
                 Install Metal Shader Converter 4.0.3 (Apple developer download; \
                 it is a universal x86_64 + arm64 pkg that installs \
                 /usr/local/lib/libmetalirconverter.dylib) or set {DYLIB_ENV}",
                tried.join(", ")
            ),
            MscError::RootSignature { code, message } => {
                write!(
                    f,
                    "MSC refused the root signature: {} ({message})",
                    code.name()
                )
            }
            MscError::Rejected { code, message } => {
                write!(f, "MSC refused the shader: {} ({message})", code.name())
            }
            MscError::NoOutput(what) => write!(f, "MSC produced no output: {what}"),
            MscError::BadInput(what) => write!(f, "bad MSC input: {what}"),
            MscError::Metal(e) => write!(f, "{e}"),
            MscError::Mismatch(what) => write!(f, "the stages do not fit together: {what}"),
            MscError::Limit(what) => write!(f, "over a Metal limit: {what}"),
            MscError::Unsupported(what) => write!(f, "not supported: {what}"),
        }
    }
}

impl std::error::Error for MscError {}

impl From<MscError> for crate::Error {
    fn from(e: MscError) -> Self {
        match e {
            MscError::Metal(e) => e,
            MscError::Library { .. } => crate::Error::Unsupported(e.to_string()),
            MscError::BadInput(_) => crate::Error::BadArgument(e.to_string()),
            MscError::Limit(_) | MscError::Unsupported(_) => {
                crate::Error::Unsupported(e.to_string())
            }
            other => crate::Error::Metal(other.to_string()),
        }
    }
}

/// The loaded converter library. Cheap to clone; every `IRCompiler` it makes
/// lives only inside one [`compile`](Msc::compile) call, so a value can be
/// shared by all threads (the header says an `IRCompiler` is not reentrant,
/// not that the library is).
#[derive(Clone, Copy)]
pub struct Msc {
    pub(crate) api: ffi::Api,
    path: &'static str,
}

// A table of function pointers into a library that is never unloaded.
unsafe impl Send for Msc {}
unsafe impl Sync for Msc {}

impl fmt::Debug for Msc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Msc({})", self.path)
    }
}

static LOADED: OnceLock<Msc> = OnceLock::new();

impl Msc {
    /// The converter, loading the library on the first call (a failure is
    /// not cached: install MSC and call again).
    pub fn get() -> Result<Msc, MscError> {
        if let Some(m) = LOADED.get() {
            return Ok(*m);
        }
        let m = Msc::load()?;
        Ok(*LOADED.get_or_init(|| m))
    }

    /// The paths [`Msc::load`] tries, in order.
    pub fn candidates() -> Vec<String> {
        match std::env::var(DYLIB_ENV) {
            Ok(p) if !p.is_empty() => vec![p],
            _ => vec![
                DEFAULT_DYLIB.to_owned(),
                "libmetalirconverter.dylib".to_owned(),
            ],
        }
    }

    /// Loads the library (see the module docs for where from).
    pub fn load() -> Result<Msc, MscError> {
        Msc::load_from(&Msc::candidates())
    }

    /// Loads the first library of `paths` that opens.
    pub fn load_from(paths: &[String]) -> Result<Msc, MscError> {
        let tried = paths.to_vec();
        let mut detail = String::new();
        for path in &tried {
            let c = CString::new(path.as_str())
                .map_err(|_| MscError::BadInput(format!("{path:?} has a NUL")))?;
            // SAFETY: a NUL-terminated path; the handle is leaked on purpose.
            let handle = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
            if handle.is_null() {
                // SAFETY: dlerror returns null or a string valid until the next dl call.
                let e = unsafe { libc::dlerror() };
                let text = if e.is_null() {
                    "unknown dlopen error".to_owned()
                } else {
                    // SAFETY: as above.
                    unsafe { CStr::from_ptr(e) }.to_string_lossy().into_owned()
                };
                detail = format!("{path}: {text}");
                continue;
            }
            // SAFETY: a live handle of the library this crate targets.
            return match unsafe { ffi::Api::resolve(handle) } {
                Ok(api) => Ok(Msc {
                    api,
                    path: Box::leak(path.clone().into_boxed_str()),
                }),
                Err(missing) => Err(MscError::Library {
                    tried: tried.clone(),
                    detail: format!(
                        "{path} lacks `{missing}`: not Metal Shader Converter 4.0.3 \
                         (header version {}.{}.{})",
                        HEADER_VERSION.0, HEADER_VERSION.1, HEADER_VERSION.2
                    ),
                }),
            };
        }
        Err(MscError::Library { tried, detail })
    }

    /// The path the library was loaded from.
    pub fn path(&self) -> &str {
        self.path
    }

    /// The top-level argument buffer layout of a root signature
    /// (`IRRootSignatureGetResourceLocations`): one entry per root parameter,
    /// in order, with MSC's alignment padding.
    pub fn layout(&self, desc: &RootSignatureDesc) -> Result<TopLevelLayout, MscError> {
        let c = root::CRootSignature::new(desc);
        let rs = compile::RootSig::create(self, &c)?;
        Ok(rs.layout(desc.params.len(), desc.static_samplers.len()))
    }
}

/// Reads an NUL-terminated string of at most `max` bytes.
pub(crate) fn bounded_c_str(p: *const c_void, max: usize) -> String {
    if p.is_null() {
        return String::new();
    }
    let p = p.cast::<c_char>();
    let mut bytes = Vec::new();
    for i in 0..max {
        // SAFETY: the library's error payload is a NUL-terminated string
        // (the header's example prints it with `%s`); reading stops at the
        // NUL or `max`.
        let b = unsafe { *p.add(i) } as u8;
        if b == 0 {
            break;
        }
        bytes.push(b);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}
