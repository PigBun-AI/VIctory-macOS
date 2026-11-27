//! Compute pipelines through `MTL4Compiler`.
//!
//! Two entrances to the same path: MSL source (tests, and the probe
//! kernels) and a `metallib` byte string (what Metal Shader Converter
//! produces; the MSC step itself is not part of M1's first half). Both end
//! in an `MTL4ComputePipelineDescriptor` compiled by the device's
//! `MTL4Compiler`.
//!
//! A pipeline state is an `MTLAllocation`; it goes into the `Resource`
//! residency set like a buffer and is retired the same deferred way.

use std::sync::Arc;

use dispatch2::DispatchData;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{
    MTL4Compiler, MTL4ComputePipelineDescriptor, MTL4LibraryDescriptor,
    MTL4LibraryFunctionDescriptor, MTLAllocation, MTLComputePipelineState, MTLDevice, MTLLibrary,
};

use crate::device::{Device, DeviceInner};
use crate::error::{Error, Result};
use crate::residency::{Retired, SetKind};

/// A compiled library (several kernels).
#[derive(Clone)]
pub struct Library {
    raw: Retained<ProtocolObject<dyn MTLLibrary>>,
}

// MTLLibrary is thread-safe.
unsafe impl Send for Library {}
unsafe impl Sync for Library {}

impl Library {
    pub(crate) fn raw(&self) -> &ProtocolObject<dyn MTLLibrary> {
        &self.raw
    }
}

struct PipelineInner {
    raw: Retained<ProtocolObject<dyn MTLComputePipelineState>>,
    device: Arc<DeviceInner>,
    entry: String,
}

unsafe impl Send for PipelineInner {}
unsafe impl Sync for PipelineInner {}

impl Drop for PipelineInner {
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

/// A compute pipeline. Cloning shares it; the last drop retires it.
#[derive(Clone)]
pub struct ComputePipeline {
    inner: Arc<PipelineInner>,
}

impl ComputePipeline {
    /// The kernel's name.
    pub fn entry(&self) -> &str {
        &self.inner.entry
    }

    /// Width of one SIMD group (`threadExecutionWidth`).
    pub fn thread_execution_width(&self) -> u32 {
        self.inner.raw.threadExecutionWidth() as u32
    }

    /// `maxTotalThreadsPerThreadgroup`.
    pub fn max_threads_per_threadgroup(&self) -> u32 {
        self.inner.raw.maxTotalThreadsPerThreadgroup() as u32
    }

    pub(crate) fn raw(&self) -> &ProtocolObject<dyn MTLComputePipelineState> {
        &self.inner.raw
    }

    /// Identity of the underlying pipeline (for redundant-bind elision).
    pub(crate) fn identity(&self) -> *const () {
        Arc::as_ptr(&self.inner).cast()
    }
}

pub(crate) struct Probe {
    pub simd_width: u32,
    pub atomics64: bool,
}

const PROBE_SIMD: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
kernel void k(device uint* o [[buffer(0)]], uint i [[thread_position_in_grid]]) { o[i] = i; }\n";

const PROBE_ATOMICS64: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
kernel void k(device atomic_ulong* o [[buffer(0)]], uint i [[thread_position_in_grid]]) {\n\
    atomic_max_explicit(o, (ulong)i, memory_order_relaxed);\n\
    atomic_min_explicit(o + 1, (ulong)i, memory_order_relaxed);\n}\n";

fn compile(
    compiler: &ProtocolObject<dyn MTL4Compiler>,
    source: &str,
    entry: &str,
) -> Result<Retained<ProtocolObject<dyn MTLComputePipelineState>>> {
    let descriptor = MTL4LibraryDescriptor::new();
    descriptor.setSource(Some(&NSString::from_str(source)));
    let library = compiler
        .newLibraryWithDescriptor_error(&descriptor)
        .map_err(|e| Error::Metal(format!("MSL: {}", e.localizedDescription())))?;
    pipeline_state(compiler, &library, entry)
}

fn pipeline_state(
    compiler: &ProtocolObject<dyn MTL4Compiler>,
    library: &ProtocolObject<dyn MTLLibrary>,
    entry: &str,
) -> Result<Retained<ProtocolObject<dyn MTLComputePipelineState>>> {
    // The compiler aborts the process (an Objective-C assertion) when the
    // function is not in the library, so ask first.
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
    let function = MTL4LibraryFunctionDescriptor::new();
    function.setName(Some(&wanted));
    function.setLibrary(Some(library));
    let descriptor = MTL4ComputePipelineDescriptor::new();
    descriptor.setComputeFunctionDescriptor(Some(&function));
    compiler
        .newComputePipelineStateWithDescriptor_compilerTaskOptions_error(
            &descriptor,
            crate::archive::task_options().as_deref(),
        )
        .map_err(|e| Error::Metal(format!("pipeline {entry}: {}", e.localizedDescription())))
}

pub(crate) fn probe_caps(
    _device: &ProtocolObject<dyn MTLDevice>,
    compiler: &ProtocolObject<dyn MTL4Compiler>,
) -> Probe {
    let simd_width = compile(compiler, PROBE_SIMD, "k")
        .map(|p| p.threadExecutionWidth() as u32)
        .unwrap_or(0);
    let atomics64 = compile(compiler, PROBE_ATOMICS64, "k").is_ok();
    Probe {
        simd_width,
        atomics64,
    }
}

impl Device {
    /// Compiles MSL source into a library.
    pub fn library_from_msl(&self, source: &str) -> Result<Library> {
        let descriptor = MTL4LibraryDescriptor::new();
        descriptor.setSource(Some(&NSString::from_str(source)));
        let raw = self
            .inner
            .compiler
            .newLibraryWithDescriptor_error(&descriptor)
            .map_err(|e| Error::Metal(format!("MSL: {}", e.localizedDescription())))?;
        Ok(Library { raw })
    }

    /// Loads a `metallib` byte string (the output of Metal Shader Converter
    /// or `metallib`). The same entrance the converted shaders will use.
    pub fn library_from_metallib(&self, bytes: &[u8]) -> Result<Library> {
        if bytes.is_empty() {
            return Err(Error::BadArgument("empty metallib".into()));
        }
        let data = DispatchData::from_bytes(bytes);
        let raw = self
            .inner
            .raw
            .newLibraryWithData_error(&data)
            .map_err(|e| Error::Metal(format!("metallib: {}", e.localizedDescription())))?;
        Ok(Library { raw })
    }

    /// A compute pipeline for the kernel `entry` of `library`, compiled by
    /// `MTL4Compiler`. The pipeline joins the resource residency set.
    pub fn compute_pipeline(&self, library: &Library, entry: &str) -> Result<ComputePipeline> {
        let raw = pipeline_state(&self.inner.compiler, &library.raw, entry)?;
        let allocation: &ProtocolObject<dyn MTLAllocation> = ProtocolObject::from_ref(&*raw);
        self.inner.residency.add(SetKind::Resource, allocation);
        Ok(ComputePipeline {
            inner: Arc::new(PipelineInner {
                raw,
                device: self.inner.clone(),
                entry: entry.to_owned(),
            }),
        })
    }

    /// MSL source to pipeline in one step (tests).
    pub fn compute_pipeline_from_msl(&self, source: &str, entry: &str) -> Result<ComputePipeline> {
        let library = self.library_from_msl(source)?;
        self.compute_pipeline(&library, entry)
    }

    /// `metallib` bytes to pipeline in one step.
    pub fn compute_pipeline_from_metallib(
        &self,
        bytes: &[u8],
        entry: &str,
    ) -> Result<ComputePipeline> {
        let library = self.library_from_metallib(bytes)?;
        self.compute_pipeline(&library, entry)
    }
}
