//! The device: creation with the hard requirements, capabilities, and the
//! shared state every other object hangs off.

use std::ffi::CString;
use std::sync::{Arc, Mutex, Weak};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTL4Compiler, MTLArgumentBuffersTier, MTLCreateSystemDefaultDevice, MTLDevice, MTLGPUFamily,
};

use crate::error::{Error, Result};
use crate::memory::AddressMap;
use crate::pages::PageRegistry;
use crate::queue::Timeline;
use crate::residency::Residency;

/// What the device can do, as far as the API tells us. Queried once.
#[derive(Clone, Debug)]
pub struct Caps {
    /// `MTLDevice.name`.
    pub name: String,
    /// `MTLDevice.registryID`.
    pub registry_id: u64,
    /// Running as x86-64 code under Rosetta.
    pub translated: bool,
    /// Unified memory (all Apple silicon).
    pub unified_memory: bool,
    /// `supportsFamily(MTLGPUFamilyMetal4)`; creation fails when false.
    pub metal4: bool,
    /// `argumentBuffersSupport == Tier2`; creation fails when false.
    pub argument_buffers_tier2: bool,
    /// Mesh and object shaders: `Apple7` or `Mac2` family (derived from the
    /// family tables; not exercised in M1).
    pub mesh_shaders: bool,
    /// `threadExecutionWidth` of a probe kernel: the SIMD width.
    pub simd_width: u32,
    /// `simd_width == 32` (MSC requires wave size 32).
    pub wave32: bool,
    /// 64-bit atomics in buffers: a probe kernel using `atomic_ulong`
    /// min/max compiles on this device (MSL 3.1 needs Apple 8 or newer).
    pub atomics64: bool,
    /// `maxBufferLength`.
    pub max_buffer_len: u64,
    /// `maxThreadgroupMemoryLength`.
    pub max_threadgroup_memory: u64,
    /// `maxThreadsPerThreadgroup` (x, y, z).
    pub max_threads_per_threadgroup: [u64; 3],
    /// `maxArgumentBufferSamplerCount`.
    pub max_argument_buffer_samplers: u64,
    /// `recommendedMaxWorkingSetSize`.
    pub recommended_working_set: u64,
    /// Page size of this process (`hw.pagesize`): 4 KiB under Rosetta, 16 KiB
    /// natively. `wrap` and the page registry use it.
    pub page_size: u64,
    /// Residency sets one queue accepts (header `MTL4CommandQueue.h`).
    pub max_residency_sets_per_queue: u32,
    /// `supportsRaytracing` (unused by the backend).
    pub raytracing: bool,
}

pub(crate) struct DeviceInner {
    pub(crate) raw: Retained<ProtocolObject<dyn MTLDevice>>,
    pub(crate) caps: Caps,
    pub(crate) pages: Arc<Mutex<PageRegistry>>,
    pub(crate) residency: Residency,
    pub(crate) addresses: Mutex<AddressMap>,
    pub(crate) timelines: Mutex<Vec<Weak<Timeline>>>,
    pub(crate) compiler: Retained<ProtocolObject<dyn MTL4Compiler>>,
    /// Pipelines that clear part of an attachment inside a pass.
    pub(crate) clear_pipes: Mutex<crate::draw::ClearPipes>,
    /// Mirrors of misaligned typed views ([`crate::mirror`]).
    pub(crate) mirrors: crate::mirror::Registry,
}

/// The device and everything shared between the objects made from it.
/// Cloning shares it.
#[derive(Clone)]
pub struct Device {
    pub(crate) inner: Arc<DeviceInner>,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device")
            .field("name", &self.inner.caps.name)
            .finish()
    }
}

fn sysctl_int(name: &str) -> Option<i64> {
    let name = CString::new(name).ok()?;
    let mut value: i64 = 0;
    let mut len = size_of::<i64>();
    // SAFETY: `value` and `len` are valid for the call.
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&raw mut value).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (rc == 0).then_some(value)
}

/// The page size of this process.
pub fn process_page_size() -> usize {
    match sysctl_int("hw.pagesize") {
        Some(n) if n > 0 && (n as u64).is_power_of_two() => n as usize,
        // SAFETY: sysconf has no preconditions.
        _ => unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize },
    }
}

impl Device {
    /// Opens the system default device. Fails (does not degrade) when the
    /// GPU lacks the Metal 4 family or Argument Buffers Tier 2.
    pub fn new() -> Result<Device> {
        let raw = MTLCreateSystemDefaultDevice().ok_or(Error::NoDevice)?;
        if !raw.supportsFamily(MTLGPUFamily::Metal4) {
            return Err(Error::Unsupported(format!(
                "{} does not support MTLGPUFamilyMetal4",
                raw.name()
            )));
        }
        if raw.argumentBuffersSupport() != MTLArgumentBuffersTier::Tier2 {
            return Err(Error::Unsupported(format!(
                "{} does not support Argument Buffers Tier 2",
                raw.name()
            )));
        }
        let compiler = raw
            .newCompilerWithDescriptor_error(&crate::archive::compiler_descriptor(&raw))
            .map_err(|e| Error::Metal(format!("MTL4Compiler: {}", e.localizedDescription())))?;
        let page_size = process_page_size();
        let residency = Residency::new(&raw)?;
        // The two caps the API has no query for are probed by compiling tiny
        // kernels, the same way the backend compiles everything.
        let probe = crate::pipeline::probe_caps(&raw, &compiler);
        let caps = Caps {
            name: raw.name().to_string(),
            registry_id: raw.registryID(),
            translated: sysctl_int("sysctl.proc_translated") == Some(1),
            unified_memory: raw.hasUnifiedMemory(),
            metal4: true,
            argument_buffers_tier2: true,
            mesh_shaders: raw.supportsFamily(MTLGPUFamily::Apple7)
                || raw.supportsFamily(MTLGPUFamily::Mac2),
            simd_width: probe.simd_width,
            wave32: probe.simd_width == 32,
            atomics64: probe.atomics64,
            max_buffer_len: raw.maxBufferLength() as u64,
            max_threadgroup_memory: raw.maxThreadgroupMemoryLength() as u64,
            max_threads_per_threadgroup: {
                let s = raw.maxThreadsPerThreadgroup();
                [s.width as u64, s.height as u64, s.depth as u64]
            },
            max_argument_buffer_samplers: raw.maxArgumentBufferSamplerCount() as u64,
            recommended_working_set: raw.recommendedMaxWorkingSetSize(),
            page_size: page_size as u64,
            max_residency_sets_per_queue: crate::queue::MAX_RESIDENCY_SETS_PER_QUEUE,
            raytracing: raw.supportsRaytracing(),
        };
        Ok(Device {
            inner: Arc::new(DeviceInner {
                raw,
                caps,
                pages: Arc::new(Mutex::new(PageRegistry::new(page_size))),
                residency,
                addresses: Mutex::new(AddressMap::default()),
                timelines: Mutex::new(Vec::new()),
                compiler,
                clear_pipes: Mutex::new(Default::default()),
                mirrors: Default::default(),
            }),
        })
    }

    /// The capabilities.
    pub fn caps(&self) -> &Caps {
        &self.inner.caps
    }

    /// The residency sets.
    pub fn residency(&self) -> &Residency {
        &self.inner.residency
    }

    /// Commits pending residency additions, then frees retired resources the
    /// queues are done with. Every submission does this; call it yourself
    /// when idle. Returns how many retired resources it freed.
    pub fn maintain(&self) -> usize {
        self.inner.residency.commit();
        self.inner.residency.reclaim()
    }

    pub(crate) fn raw(&self) -> &ProtocolObject<dyn MTLDevice> {
        &self.inner.raw
    }

    pub(crate) fn register_timeline(&self, timeline: &Arc<Timeline>) {
        let mut list = self
            .inner
            .timelines
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        list.retain(|w| w.strong_count() > 0);
        list.push(Arc::downgrade(timeline));
    }
}

impl DeviceInner {
    /// For every queue: the value its timeline must reach before something
    /// destroyed now can be removed. Timelines with nothing in flight are
    /// left out.
    pub(crate) fn in_flight_waits(&self) -> Vec<(Arc<Timeline>, u64)> {
        let list = self.timelines.lock().unwrap_or_else(|e| e.into_inner());
        list.iter()
            .filter_map(Weak::upgrade)
            .filter_map(|t| {
                let submitted = t.submitted();
                (t.completed() < submitted).then_some((t, submitted))
            })
            .collect()
    }
}
