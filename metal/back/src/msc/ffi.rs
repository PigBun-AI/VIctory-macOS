//! The subset of `libmetalirconverter` this backend calls, loaded with
//! `dlopen` at run time.
//!
//! Source of truth: the installed headers `metal_irconverter/metal_irconverter.h`
//! (`IR_VERSION` 4.0.3, installed and read on
//! 2026-11-27). Every struct below is the C layout (sizes and offsets were
//! printed by `clang` from those headers and are asserted at the bottom).
//!
//! Why not the `saxaboom` crate (git `main` `efd8085` is the first with
//! 4.0.3; crates.io stops at `0.2.0+irconverter-2.0`, checked 2026-11-27):
//! its wrapper `IRError::code()` panics on the error codes `NullHullShaderInputOutputMismatch`,
//! `InvalidRaytracingUserAttributeSize`, `IncorrectHitgroupType` and `FP64Usage`
//! (its `match` has no arm for them, and a game shader can produce
//! `FP64Usage`), and `IRCompilerCreate`/`IRObjectCreateFromDXIL` failures
//! `expect`. This backend must turn every refusal into an error value, so it
//! keeps its own thin layer: about forty functions, `dlopen`, no extra crate.

// A binding table: not every entry is used yet.
#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    clippy::upper_case_acronyms
)]

use std::ffi::{CStr, c_char, c_void};

// ---- opaque handles ---------------------------------------------------

#[repr(C)]
pub struct IRCompiler {
    _p: [u8; 0],
}
#[repr(C)]
pub struct IRObject {
    _p: [u8; 0],
}
#[repr(C)]
pub struct IRRootSignature {
    _p: [u8; 0],
}
#[repr(C)]
pub struct IRMetalLibBinary {
    _p: [u8; 0],
}
#[repr(C)]
pub struct IRShaderReflection {
    _p: [u8; 0],
}
#[repr(C)]
pub struct IRError {
    _p: [u8; 0],
}

// ---- enums (all 32-bit in C; kept as u32 so an unknown value is no UB) ---

/// `IRShaderStage`.
pub mod stage {
    pub const INVALID: u32 = 0;
    pub const VERTEX: u32 = 1;
    pub const FRAGMENT: u32 = 2;
    pub const HULL: u32 = 3;
    pub const DOMAIN: u32 = 4;
    pub const MESH: u32 = 5;
    pub const AMPLIFICATION: u32 = 6;
    pub const GEOMETRY: u32 = 7;
    pub const COMPUTE: u32 = 8;
    pub const CLOSEST_HIT: u32 = 9;
    pub const STREAM_OUT: u32 = 15;
    pub const STAGE_IN: u32 = 16;
}

/// `IRRootParameterType`.
pub mod param_type {
    pub const TABLE: u32 = 0;
    pub const CONSTANTS: u32 = 1;
    pub const CBV: u32 = 2;
    pub const SRV: u32 = 3;
    pub const UAV: u32 = 4;
}

/// `IRResourceType` (as `IRResourceLocation::resourceType`).
pub mod resource_type {
    pub const TABLE: u32 = 0;
    pub const CONSTANT: u32 = 1;
    pub const CBV: u32 = 2;
    pub const SRV: u32 = 3;
    pub const UAV: u32 = 4;
    pub const SAMPLER: u32 = 5;
    pub const INVALID: u32 = 6;
}

/// `IRErrorCode`.
pub mod error_code {
    pub const NO_ERROR: u32 = 0;
    pub const SHADER_REQUIRES_ROOT_SIGNATURE: u32 = 1;
    pub const UNRECOGNIZED_ROOT_SIGNATURE_DESCRIPTOR: u32 = 2;
    pub const UNRECOGNIZED_PARAMETER_TYPE_IN_ROOT_SIGNATURE: u32 = 3;
    pub const RESOURCE_NOT_REFERENCED_BY_ROOT_SIGNATURE: u32 = 4;
    pub const SHADER_INCOMPATIBLE_WITH_DUAL_SOURCE_BLENDING: u32 = 5;
    pub const UNSUPPORTED_WAVE_SIZE: u32 = 6;
    pub const UNSUPPORTED_INSTRUCTION: u32 = 7;
    pub const COMPILATION_ERROR: u32 = 8;
    pub const FAILED_TO_SYNTHESIZE_STAGE_IN_FUNCTION: u32 = 9;
    pub const FAILED_TO_SYNTHESIZE_STREAM_OUT_FUNCTION: u32 = 10;
    pub const FAILED_TO_SYNTHESIZE_INDIRECT_INTERSECTION_FUNCTION: u32 = 11;
    pub const UNABLE_TO_VERIFY_MODULE: u32 = 12;
    pub const UNABLE_TO_LINK_MODULE: u32 = 13;
    pub const UNRECOGNIZED_DXIL_HEADER: u32 = 14;
    pub const INVALID_RAYTRACING_ATTRIBUTE: u32 = 15;
    pub const NULL_HULL_SHADER_INPUT_OUTPUT_MISMATCH: u32 = 16;
    pub const INVALID_RAYTRACING_USER_ATTRIBUTE_SIZE: u32 = 17;
    pub const INCORRECT_HITGROUP_TYPE: u32 = 18;
    pub const FP64_USAGE: u32 = 19;
    pub const UNKNOWN: u32 = 0x7FFF_FFFF;
}

/// `IRRootSignatureVersion`.
pub const ROOT_SIGNATURE_VERSION_1_1: u32 = 2;
/// `IRReflectionVersion_1_0`.
pub const REFLECTION_VERSION_1_0: u32 = 1;
/// `IRBytecodeOwnershipCopy`.
pub const BYTECODE_COPY: u32 = 1;

/// `IRCompatibilityFlags`.
pub mod compat {
    pub const BOUNDS_CHECK: u32 = 1 << 0;
    pub const VERTEX_POSITION_INF_TO_NAN: u32 = 1 << 1;
    pub const TEXTURE_MIN_LOD_CLAMP: u32 = 1 << 2;
    pub const SAMPLER_LOD_BIAS: u32 = 1 << 3;
    pub const POSITION_INVARIANCE: u32 = 1 << 4;
    pub const SAMPLE_NAN_TO_ZERO: u32 = 1 << 5;
    pub const TEX_WRITE_ROUNDING_RTZ: u32 = 1 << 6;
    pub const SUPPRESS_2D_COMPUTE_DERIVATIVE_ERRORS: u32 = 1 << 7;
    pub const FORCE_TEXTURE_ARRAY: u32 = 1 << 8;
    pub const DISABLE_NAN_INF_OPTIMIZATION: u32 = 1 << 9;
    pub const ENSURE_TG_MEMORY_COHERENCY: u32 = 1 << 10;
}

/// `IRCompilerValidationFlags`.
pub mod validation {
    pub const NONE: u32 = 0;
    pub const RAW_ROOT_RESOURCES: u32 = 1;
    pub const ALL_RESOURCES_BOUND: u32 = 1 << 1;
    pub const DXIL: u32 = 1 << 2;
}

/// `IRGPUFamily`.
pub mod gpu_family {
    pub const APPLE6: u32 = 1006;
    pub const APPLE7: u32 = 1007;
    pub const APPLE8: u32 = 1008;
    pub const APPLE9: u32 = 1009;
    pub const APPLE10: u32 = 1010;
    pub const METAL3: u32 = 5001;
}

/// `IRInputTopology`.
pub mod input_topology {
    pub const UNDEFINED: u32 = 0;
    pub const POINT: u32 = 1;
    pub const LINE: u32 = 2;
    pub const TRIANGLE: u32 = 3;
}

/// `IRStageInCodeGenerationMode`.
pub const STAGE_IN_METAL_VERTEX_FETCH: u32 = 0;

/// `IRDualSourceBlendingConfiguration` / `IRDepthFeedbackConfiguration`
/// (the two enums have the same three values).
pub mod feature_config {
    pub const DECIDE_AT_RUNTIME: u32 = 0;
    pub const FORCE_ENABLED: u32 = 1;
    pub const FORCE_DISABLED: u32 = 2;
}

/// `IROperatingSystem_macOS`.
pub const OS_MACOS: u32 = 0;

// ---- structs -------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IRDescriptorRange1 {
    pub RangeType: u32,
    pub NumDescriptors: u32,
    pub BaseShaderRegister: u32,
    pub RegisterSpace: u32,
    pub Flags: u32,
    pub OffsetInDescriptorsFromTableStart: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IRRootDescriptorTable1 {
    pub NumDescriptorRanges: u32,
    pub pDescriptorRanges: *const IRDescriptorRange1,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IRRootConstants {
    pub ShaderRegister: u32,
    pub RegisterSpace: u32,
    pub Num32BitValues: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IRRootDescriptor1 {
    pub ShaderRegister: u32,
    pub RegisterSpace: u32,
    pub Flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union IRRootParameter1Data {
    pub DescriptorTable: IRRootDescriptorTable1,
    pub Constants: IRRootConstants,
    pub Descriptor: IRRootDescriptor1,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IRRootParameter1 {
    pub ParameterType: u32,
    pub data: IRRootParameter1Data,
    pub ShaderVisibility: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IRStaticSamplerDescriptor {
    pub Filter: u32,
    pub AddressU: u32,
    pub AddressV: u32,
    pub AddressW: u32,
    pub MipLODBias: f32,
    pub MaxAnisotropy: u32,
    pub ComparisonFunc: u32,
    pub BorderColor: u32,
    pub MinLOD: f32,
    pub MaxLOD: f32,
    pub ShaderRegister: u32,
    pub RegisterSpace: u32,
    pub ShaderVisibility: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IRRootSignatureDescriptor1 {
    pub NumParameters: u32,
    pub pParameters: *const IRRootParameter1,
    pub NumStaticSamplers: u32,
    pub pStaticSamplers: *const IRStaticSamplerDescriptor,
    pub Flags: u32,
}

/// `IRVersionedRootSignatureDescriptor` with the 1.1 arm of its union (the
/// 1.0 arm has the same size and alignment: 40 bytes, 8).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IRVersionedRootSignatureDescriptor {
    pub version: u32,
    pub desc_1_1: IRRootSignatureDescriptor1,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IRResourceLocation {
    pub resourceType: u32,
    pub space: u32,
    pub slot: u32,
    pub topLevelOffset: u32,
    pub sizeBytes: u64,
    pub resourceName: *const c_char,
}

impl Default for IRResourceLocation {
    fn default() -> Self {
        IRResourceLocation {
            resourceType: resource_type::INVALID,
            space: 0,
            slot: 0,
            topLevelOffset: 0,
            sizeBytes: 0,
            resourceName: std::ptr::null(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct IRVersionedCSInfo {
    pub version: u32,
    pub tg_size: [u32; 3],
}

/// `IRVersionedMSInfo` (`IRMSInfo_1_0` inlined).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct IRVersionedMSInfo {
    pub version: u32,
    pub max_vertex_output_count: u32,
    pub max_primitive_output_count: u32,
    pub primitive_topology: u32,
    pub max_payload_size_in_bytes: u32,
    pub num_threads: [u32; 3],
}

/// `IRVersionedASInfo` (`IRASInfo_1_0` inlined).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct IRVersionedASInfo {
    pub version: u32,
    pub num_threads: [u32; 3],
    pub max_payload_size_in_bytes: u32,
}

/// `IRVertexInputInfo_1_0`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IRVertexInputInfo {
    pub name: *const c_char,
    pub attributeIndex: u8,
}

/// `IRVersionedVSInfo` (`IRVSInfo_1_0` inlined).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IRVersionedVSInfo {
    pub version: u32,
    pub instance_id_index: i32,
    pub vertex_id_index: i32,
    pub vertex_output_size_in_bytes: u32,
    pub needs_draw_params: bool,
    pub vertex_inputs: *mut IRVertexInputInfo,
    pub num_vertex_inputs: usize,
}

impl Default for IRVersionedVSInfo {
    fn default() -> Self {
        IRVersionedVSInfo {
            version: 0,
            instance_id_index: -1,
            vertex_id_index: -1,
            vertex_output_size_in_bytes: 0,
            needs_draw_params: false,
            vertex_inputs: std::ptr::null_mut(),
            num_vertex_inputs: 0,
        }
    }
}

/// `IRVersionedFSInfo` (`IRFSInfo_1_0` inlined).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct IRVersionedFSInfo {
    pub version: u32,
    pub num_render_targets: i32,
    pub rt_index_int: u8,
    pub discards: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IRFunctionConstant {
    pub name: *const c_char,
    pub type_: u32,
}

// ---- layout checks against the C headers ------------------------------------

const _: () = {
    use std::mem::{align_of, offset_of, size_of};
    assert!(size_of::<IRDescriptorRange1>() == 24);
    assert!(size_of::<IRRootDescriptorTable1>() == 16);
    assert!(size_of::<IRRootConstants>() == 12);
    assert!(size_of::<IRRootDescriptor1>() == 12);
    assert!(size_of::<IRRootParameter1>() == 32);
    assert!(offset_of!(IRRootParameter1, data) == 8);
    assert!(offset_of!(IRRootParameter1, ShaderVisibility) == 24);
    assert!(size_of::<IRStaticSamplerDescriptor>() == 52);
    assert!(offset_of!(IRStaticSamplerDescriptor, ShaderVisibility) == 48);
    assert!(size_of::<IRRootSignatureDescriptor1>() == 40);
    assert!(offset_of!(IRRootSignatureDescriptor1, pParameters) == 8);
    assert!(offset_of!(IRRootSignatureDescriptor1, pStaticSamplers) == 24);
    assert!(offset_of!(IRRootSignatureDescriptor1, Flags) == 32);
    assert!(size_of::<IRVersionedRootSignatureDescriptor>() == 48);
    assert!(align_of::<IRVersionedRootSignatureDescriptor>() == 8);
    assert!(offset_of!(IRVersionedRootSignatureDescriptor, desc_1_1) == 8);
    assert!(size_of::<IRResourceLocation>() == 32);
    assert!(offset_of!(IRResourceLocation, topLevelOffset) == 12);
    assert!(offset_of!(IRResourceLocation, sizeBytes) == 16);
    assert!(offset_of!(IRResourceLocation, resourceName) == 24);
    assert!(size_of::<IRVersionedCSInfo>() == 16);
    assert!(size_of::<IRVersionedMSInfo>() == 32);
    assert!(offset_of!(IRVersionedMSInfo, num_threads) == 20);
    assert!(size_of::<IRVersionedASInfo>() == 20);
    assert!(size_of::<IRFunctionConstant>() == 16);
    assert!(size_of::<IRVertexInputInfo>() == 16);
    assert!(size_of::<IRVersionedVSInfo>() == 40);
    assert!(offset_of!(IRVersionedVSInfo, vertex_inputs) == 24);
    assert!(offset_of!(IRVersionedVSInfo, num_vertex_inputs) == 32);
    assert!(size_of::<IRVersionedFSInfo>() == 12);
    assert!(offset_of!(IRVersionedFSInfo, rt_index_int) == 8);
};

// ---- the function table -----------------------------------------------------

macro_rules! api {
    ($( fn $name:ident($($arg:ty),*) $(-> $ret:ty)?; )*) => {
        /// The loaded library's entry points. Plain function pointers: the
        /// table is `Send + Sync`; each `IRCompiler` is used by one thread at a time.
        #[derive(Clone, Copy)]
        pub struct Api {
            $( pub $name: unsafe extern "C" fn($($arg),*) $(-> $ret)?, )*
        }

        impl Api {
            /// Resolves every function; names the first missing one.
            ///
            /// # Safety
            /// `handle` must be a live `dlopen` handle of a
            /// `libmetalirconverter` whose functions have the signatures
            /// of the header this file was written from.
            pub unsafe fn resolve(handle: *mut c_void) -> Result<Api, String> {
                Ok(Api {
                    $( $name: {
                        // SAFETY: a NUL-terminated name; `handle` per the contract.
                        let sym = unsafe {
                            libc::dlsym(handle, concat!(stringify!($name), "\0").as_ptr().cast())
                        };
                        if sym.is_null() {
                            return Err(stringify!($name).to_owned());
                        }
                        // SAFETY: the symbol has the C signature in this table.
                        unsafe { std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($arg),*) $(-> $ret)?>(sym) }
                    }, )*
                })
            }
        }
    };
}

api! {
    // errors
    fn IRErrorGetCode(*const IRError) -> u32;
    fn IRErrorGetPayload(*const IRError) -> *const c_void;
    fn IRErrorDestroy(*mut IRError);
    // root signatures
    fn IRRootSignatureCreateFromDescriptor(*const IRVersionedRootSignatureDescriptor, *mut *mut IRError) -> *mut IRRootSignature;
    fn IRRootSignatureDestroy(*mut IRRootSignature);
    fn IRRootSignatureGetResourceCount(*const IRRootSignature) -> usize;
    fn IRRootSignatureGetResourceLocations(*const IRRootSignature, *mut IRResourceLocation);
    // compiler
    fn IRCompilerCreate() -> *mut IRCompiler;
    fn IRCompilerDestroy(*mut IRCompiler);
    fn IRCompilerSetGlobalRootSignature(*mut IRCompiler, *const IRRootSignature);
    fn IRCompilerSetValidationFlags(*mut IRCompiler, u32);
    fn IRCompilerSetCompatibilityFlags(*mut IRCompiler, u32);
    fn IRCompilerSetEntryPointName(*mut IRCompiler, *const c_char);
    fn IRCompilerSetMinimumGPUFamily(*mut IRCompiler, u32);
    fn IRCompilerSetMinimumDeploymentTarget(*mut IRCompiler, u32, *const c_char);
    fn IRCompilerSetInputTopology(*mut IRCompiler, u32);
    fn IRCompilerSetStageInGenerationMode(*mut IRCompiler, u32);
    fn IRCompilerSetDualSourceBlendingConfiguration(*mut IRCompiler, u32);
    fn IRCompilerSetDepthFeedbackConfiguration(*mut IRCompiler, u32);
    fn IRCompilerSetIntRTMask(*mut IRCompiler, u8);
    fn IRCompilerSetSampleMask(*mut IRCompiler, u32);
    fn IRCompilerIgnoreRootSignature(*mut IRCompiler, bool);
    fn IRCompilerIgnoreDebugInformation(*mut IRCompiler, bool);
    fn IRCompilerAllocCompileAndLink(*mut IRCompiler, *const c_char, *const IRObject, *mut *mut IRError) -> *mut IRObject;
    // objects
    fn IRObjectCreateFromDXIL(*const u8, usize, u32) -> *mut IRObject;
    fn IRObjectDestroy(*mut IRObject);
    fn IRObjectGetMetalIRShaderStage(*const IRObject) -> u32;
    fn IRObjectGetMetalLibBinary(*const IRObject, u32, *mut IRMetalLibBinary) -> bool;
    fn IRObjectGetReflection(*const IRObject, u32, *mut IRShaderReflection) -> bool;
    // metallib
    fn IRMetalLibBinaryCreate() -> *mut IRMetalLibBinary;
    fn IRMetalLibBinaryDestroy(*mut IRMetalLibBinary);
    fn IRMetalLibGetBytecodeSize(*const IRMetalLibBinary) -> usize;
    fn IRMetalLibGetBytecode(*const IRMetalLibBinary, *mut u8) -> usize;
    // reflection
    fn IRShaderReflectionCreate() -> *mut IRShaderReflection;
    fn IRShaderReflectionDestroy(*mut IRShaderReflection);
    fn IRShaderReflectionGetEntryPointFunctionName(*const IRShaderReflection) -> *const c_char;
    fn IRShaderReflectionNeedsFunctionConstants(*const IRShaderReflection) -> bool;
    fn IRShaderReflectionGetFunctionConstantCount(*const IRShaderReflection) -> usize;
    fn IRShaderReflectionCopyFunctionConstants(*const IRShaderReflection, *mut IRFunctionConstant);
    fn IRShaderReflectionReleaseFunctionConstants(*mut IRFunctionConstant, usize);
    fn IRShaderReflectionCopyComputeInfo(*const IRShaderReflection, u32, *mut IRVersionedCSInfo) -> bool;
    fn IRShaderReflectionReleaseComputeInfo(*mut IRVersionedCSInfo) -> bool;
    fn IRShaderReflectionCopyMeshInfo(*const IRShaderReflection, u32, *mut IRVersionedMSInfo) -> bool;
    fn IRShaderReflectionReleaseMeshInfo(*mut IRVersionedMSInfo) -> bool;
    fn IRShaderReflectionCopyAmplificationInfo(*const IRShaderReflection, u32, *mut IRVersionedASInfo) -> bool;
    fn IRShaderReflectionReleaseAmplificationInfo(*mut IRVersionedASInfo) -> bool;
    fn IRShaderReflectionCopyVertexInfo(*const IRShaderReflection, u32, *mut IRVersionedVSInfo) -> bool;
    fn IRShaderReflectionReleaseVertexInfo(*mut IRVersionedVSInfo) -> bool;
    fn IRShaderReflectionCopyFragmentInfo(*const IRShaderReflection, u32, *mut IRVersionedFSInfo) -> bool;
    fn IRShaderReflectionReleaseFragmentInfo(*mut IRVersionedFSInfo) -> bool;
    fn IRShaderReflectionGetResourceCount(*const IRShaderReflection) -> usize;
    fn IRShaderReflectionGetResourceLocations(*const IRShaderReflection, *mut IRResourceLocation);
    fn IRShaderReflectionCopyJSONString(*const IRShaderReflection) -> *const c_char;
    fn IRShaderReflectionReleaseString(*const c_char);
}

/// `CStr` of a C string the library owns; lossy UTF-8; empty for null.
///
/// # Safety
/// `p` is null or a NUL-terminated string that outlives the call.
pub unsafe fn c_str(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        // SAFETY: per the contract.
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}
