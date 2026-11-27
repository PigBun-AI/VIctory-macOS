//! M0 bring-up subset of the Metal side.
//!
//! The kernels and the wrapping/bandwidth methodology follow
//! `unified-memory-probe` from nowak-kowalski's fork (issue #221, GPL-3.0,
//! the same licence as this repository).
//!
//! All state is one process-wide [`Gpu`] behind a mutex: the Wine unixlib
//! entry points are plain C functions with no place to keep a context.

use std::ffi::{CString, c_void};
use std::ptr::NonNull;
use std::sync::Mutex;
use std::time::Instant;

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::*;
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};

type Buffer = Retained<ProtocolObject<dyn MTLBuffer>>;
type Pipeline = Retained<ProtocolObject<dyn MTLComputePipelineState>>;
type Queue = Retained<ProtocolObject<dyn MTLCommandQueue>>;
type Encoder = ProtocolObject<dyn MTLComputeCommandEncoder>;
type Residency = Retained<ProtocolObject<dyn MTLResidencySet>>;

const SHADERS: &str = r#"
#include <metal_stdlib>
using namespace metal;

// Sums n words into one atomic: the GPU reads what the CPU wrote.
kernel void sum_u32(device const uint* src [[buffer(0)]],
                    device atomic_uint* out [[buffer(1)]],
                    constant uint& n [[buffer(2)]],
                    uint i [[thread_position_in_grid]]) {
    uint v = i < n ? src[i] : 0u;
    v = simd_sum(v);
    if (simd_is_first()) atomic_fetch_add_explicit(out, v, memory_order_relaxed);
}

// The GPU writes what the CPU will read.
kernel void xor_inplace(device uint* a [[buffer(0)]],
                        constant uint& key [[buffer(1)]],
                        constant uint& n [[buffer(2)]],
                        uint i [[thread_position_in_grid]]) {
    if (i < n) a[i] ^= key ^ i;
}

kernel void read_bw(device const uint4* src [[buffer(0)]],
                    device uint* out [[buffer(1)]],
                    constant uint& n [[buffer(2)]],
                    uint i [[thread_position_in_grid]],
                    uint threads [[threads_per_grid]]) {
    uint4 acc = 0;
    for (uint j = i; j < n; j += threads) acc ^= src[j];
    if (all(acc == uint4(0xdeadbeefu))) out[0] = 1u; // keeps the loads
}
"#;

/// Which queue an empty command buffer goes through.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// `MTLCommandQueue`: commit, `waitUntilCompleted`.
    Classic,
    /// `MTL4CommandQueue`: commit, signal a shared event, wait on it.
    Metal4,
}

/// What `init` found out about the machine and the Metal 4 queue.
#[derive(Clone, Debug)]
pub struct Info {
    pub name: String,
    pub unified_memory: bool,
    pub max_buffer_len: u64,
    pub working_set: u64,
    /// Running as x86-64 code under Rosetta.
    pub translated: bool,
    pub page_size: u32,
    /// `supportsFamily(MTLGPUFamilyMetal4)`.
    pub metal4_family: bool,
}

struct Mtl4 {
    queue: Retained<ProtocolObject<dyn MTL4CommandQueue>>,
    allocator: Retained<ProtocolObject<dyn MTL4CommandAllocator>>,
    cb: Retained<ProtocolObject<dyn MTL4CommandBuffer>>,
    event: Retained<ProtocolObject<dyn MTLSharedEvent>>,
    value: u64,
}

struct Gpu {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    queue: Queue,
    sum: Pipeline,
    xor: Pipeline,
    read_bw: Pipeline,
    sink: Buffer,
    buffers: Vec<Option<Buffer>>,
    residency: Vec<(u32, Residency)>,
    mtl4: Option<Mtl4>,
}

// The Metal objects are thread-safe; the mutex serialises our own use.
unsafe impl Send for Gpu {}

static GPU: Mutex<Option<Gpu>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Gpu) -> R) -> Option<R> {
    let mut guard = GPU.lock().ok()?;
    autoreleasepool(|_| guard.as_mut().map(f))
}

fn sysctl_int(name: &str) -> Option<i64> {
    let name = CString::new(name).ok()?;
    let mut value: i64 = 0;
    let mut len = size_of::<i64>();
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

/// Creates the device, queue and kernels. Idempotent: a second call just
/// reports again. The Metal 4 objects wait for [`init_metal4`].
pub fn init() -> Result<Info, String> {
    let mut guard = GPU.lock().map_err(|e| e.to_string())?;
    autoreleasepool(|_| {
        if guard.is_none() {
            *guard = Some(Gpu::new()?);
        }
        Ok(guard.as_ref().map(Gpu::info).expect("just made"))
    })
}

impl Gpu {
    fn new() -> Result<Gpu, String> {
        let device = MTLCreateSystemDefaultDevice().ok_or("no Metal device")?;
        let queue = device.newCommandQueue().ok_or("no command queue")?;
        let library = device
            .newLibraryWithSource_options_error(&NSString::from_str(SHADERS), None)
            .map_err(|e| format!("shaders: {}", e.localizedDescription()))?;
        let pipeline = |name: &str| -> Result<Pipeline, String> {
            let function = library
                .newFunctionWithName(&NSString::from_str(name))
                .ok_or(format!("no function {name}"))?;
            device
                .newComputePipelineStateWithFunction_error(&function)
                .map_err(|e| format!("{name}: {}", e.localizedDescription()))
        };
        let sink = device
            .newBufferWithLength_options(4, MTLResourceOptions::StorageModeShared)
            .ok_or("no sink buffer")?;
        Ok(Gpu {
            sum: pipeline("sum_u32")?,
            xor: pipeline("xor_inplace")?,
            read_bw: pipeline("read_bw")?,
            sink,
            queue,
            mtl4: None,
            buffers: Vec::new(),
            residency: Vec::new(),
            device,
        })
    }

    fn make_mtl4(&self) -> Result<Mtl4, String> {
        let queue = self
            .device
            .newMTL4CommandQueue()
            .ok_or("newMTL4CommandQueue returned nil")?;
        let allocator = self
            .device
            .newCommandAllocator()
            .ok_or("newCommandAllocator returned nil")?;
        let cb = self
            .device
            .newCommandBuffer()
            .ok_or("newCommandBuffer (MTL4) returned nil")?;
        let event = self
            .device
            .newSharedEvent()
            .ok_or("newSharedEvent returned nil")?;
        Ok(Mtl4 {
            queue,
            allocator,
            cb,
            event,
            value: 0,
        })
    }

    fn info(&self) -> Info {
        let metal4_family = self.device.supportsFamily(MTLGPUFamily::Metal4);
        Info {
            name: self.device.name().to_string(),
            unified_memory: self.device.hasUnifiedMemory(),
            max_buffer_len: self.device.maxBufferLength() as u64,
            working_set: self.device.recommendedMaxWorkingSetSize(),
            translated: sysctl_int("sysctl.proc_translated") == Some(1),
            page_size: sysctl_int("hw.pagesize").unwrap_or(0) as u32,
            metal4_family,
        }
    }

    fn run(&self, encode: impl FnOnce(&Encoder)) -> Option<(f64, u64)> {
        let start = Instant::now();
        let cb = self.queue.commandBuffer()?;
        let encoder = cb.computeCommandEncoderWithDispatchType(MTLDispatchType::Serial)?;
        encode(&encoder);
        encoder.endEncoding();
        cb.commit();
        cb.waitUntilCompleted();
        let wall = start.elapsed().as_nanos() as u64;
        (cb.status() == MTLCommandBufferStatus::Completed)
            .then(|| ((cb.GPUEndTime() - cb.GPUStartTime()) * 1e9, wall))
    }

    fn empty_classic(&self) -> bool {
        let Some(cb) = self.queue.commandBuffer() else {
            return false;
        };
        cb.commit();
        cb.waitUntilCompleted();
        cb.status() == MTLCommandBufferStatus::Completed
    }

    fn empty_mtl4(&mut self) -> bool {
        let Some(m) = self.mtl4.as_mut() else {
            return false;
        };
        m.allocator.reset();
        m.cb.beginCommandBufferWithAllocator(&m.allocator);
        m.cb.endCommandBuffer();
        let mut list = [NonNull::from(&*m.cb)];
        // SAFETY: one valid, ended command buffer.
        unsafe { m.queue.commit_count(NonNull::from(&mut list).cast(), 1) };
        m.value += 1;
        let event: &ProtocolObject<dyn MTLEvent> = ProtocolObject::from_ref(&*m.event);
        m.queue.signalEvent_value(event, m.value);
        m.event.waitUntilSignaledValue_timeoutMS(m.value, 2000)
    }

    fn buffer(&self, handle: u32) -> Option<&Buffer> {
        self.buffers.get(handle as usize)?.as_ref()
    }

    fn store(&mut self, buffer: Buffer) -> u32 {
        if let Some(slot) = self.buffers.iter().position(Option::is_none) {
            self.buffers[slot] = Some(buffer);
            return slot as u32;
        }
        self.buffers.push(Some(buffer));
        (self.buffers.len() - 1) as u32
    }
}

fn dispatch(encoder: &Encoder, pipeline: &Pipeline, threads: usize) {
    encoder.setComputePipelineState(pipeline);
    let group = pipeline.maxTotalThreadsPerThreadgroup().min(256);
    encoder.dispatchThreads_threadsPerThreadgroup(
        MTLSize {
            width: threads,
            height: 1,
            depth: 1,
        },
        MTLSize {
            width: group,
            height: 1,
            depth: 1,
        },
    );
}

fn set_buffer(encoder: &Encoder, buffer: &Buffer, index: usize) {
    // SAFETY: the buffer outlives the command buffer, which we wait for.
    unsafe { encoder.setBuffer_offset_atIndex(Some(buffer), 0, index) };
}

fn set_u32(encoder: &Encoder, value: u32, index: usize) {
    let ptr = NonNull::from(&value).cast::<c_void>();
    // SAFETY: Metal copies the 4 bytes before returning.
    unsafe { encoder.setBytes_length_atIndex(ptr, 4, index) };
}

/// Makes the Metal 4 queue and friends if they are not there yet; returns
/// the reason when they cannot be made.
pub fn init_metal4() -> Result<(), String> {
    with(|g| {
        if g.mtl4.is_some() {
            return Ok(());
        }
        if !g.device.supportsFamily(MTLGPUFamily::Metal4) {
            return Err("device does not support MTLGPUFamilyMetal4".to_owned());
        }
        g.mtl4 = Some(g.make_mtl4()?);
        Ok(())
    })
    .unwrap_or_else(|| Err("no device (call init first)".to_owned()))
}

/// Submits `iterations` empty command buffers one after the other, each
/// committed and waited for; returns the elapsed nanoseconds in total.
pub fn empty_cb(kind: Kind, iterations: u32) -> Option<u64> {
    with(|g| {
        let start = Instant::now();
        for _ in 0..iterations {
            let ok = match kind {
                Kind::Classic => g.empty_classic(),
                Kind::Metal4 => g.empty_mtl4(),
            };
            if !ok {
                return None;
            }
        }
        Some(start.elapsed().as_nanos() as u64)
    })?
}

/// A Metal-owned shared buffer (the control for the bandwidth test).
pub fn alloc(len: usize) -> Option<u32> {
    with(|g| {
        let buffer = g
            .device
            .newBufferWithLength_options(len, MTLResourceOptions::StorageModeShared)?;
        Some(g.store(buffer))
    })?
}

/// `newBufferWithBytesNoCopy` over caller memory.
///
/// # Safety
/// `ptr..ptr + len` must stay mapped until [`release`] is called for the
/// returned handle, and `ptr` and `len` must be multiples of the page size
/// Metal wants (16 KiB on Apple Silicon).
pub unsafe fn wrap(ptr: *mut u8, len: usize) -> Option<u32> {
    let ptr = NonNull::new(ptr.cast::<c_void>())?;
    with(|g| {
        // SAFETY: forwarded from the caller.
        let buffer = unsafe {
            g.device
                .newBufferWithBytesNoCopy_length_options_deallocator(
                    ptr,
                    len,
                    MTLResourceOptions::StorageModeShared,
                    None,
                )
        }?;
        Some(g.store(buffer))
    })?
}

pub fn release(handle: u32) -> bool {
    with(|g| {
        g.residency.retain(|(h, _)| *h != handle);
        g.buffers
            .get_mut(handle as usize)
            .and_then(Option::take)
            .is_some()
    })
    .unwrap_or(false)
}

/// The GPU sums `words` words of the buffer (wrapping `u32`).
pub fn sum(handle: u32, words: u32) -> Option<u32> {
    with(|g| {
        let buffer = g.buffer(handle)?.clone();
        // SAFETY: the sink is 4 bytes of shared memory we own.
        unsafe { g.sink.contents().cast::<u32>().write(0) };
        g.run(|e| {
            set_buffer(e, &buffer, 0);
            set_buffer(e, &g.sink, 1);
            set_u32(e, words, 2);
            dispatch(e, &g.sum, words as usize);
        })?;
        // SAFETY: as above; the command buffer is complete.
        Some(unsafe { g.sink.contents().cast::<u32>().read() })
    })?
}

/// The GPU does `a[i] ^= key ^ i` over `words` words.
pub fn xor(handle: u32, words: u32, key: u32) -> bool {
    with(|g| {
        let Some(buffer) = g.buffer(handle).cloned() else {
            return false;
        };
        g.run(|e| {
            set_buffer(e, &buffer, 0);
            set_u32(e, key, 1);
            set_u32(e, words, 2);
            dispatch(e, &g.xor, words as usize);
        })
        .is_some()
    })
    .unwrap_or(false)
}

/// One read-bandwidth dispatch over `len` bytes; returns (GPU ns, wall ns).
pub fn read_bw(handle: u32, len: usize) -> Option<(u64, u64)> {
    with(|g| {
        let buffer = g.buffer(handle)?.clone();
        let n = (len / 16) as u32;
        let (gpu, wall) = g.run(|e| {
            set_buffer(e, &buffer, 0);
            set_buffer(e, &g.sink, 1);
            set_u32(e, n, 2);
            dispatch(e, &g.read_bw, 256 * 1024);
        })?;
        Some((gpu as u64, wall))
    })?
}

/// Residency-set steps on a handle, timed in nanoseconds.
#[derive(Clone, Copy, Debug, Default)]
pub struct ResidencyTimes {
    pub create_ns: u64,
    pub add_ns: u64,
    pub commit_ns: u64,
    pub request_ns: u64,
}

/// Puts the buffer in a new residency set: create, add, commit, request.
pub fn residency_build(handle: u32) -> Option<ResidencyTimes> {
    with(|g| {
        let buffer = g.buffer(handle)?.clone();
        let t = Instant::now();
        let descriptor = MTLResidencySetDescriptor::new();
        let set = g
            .device
            .newResidencySetWithDescriptor_error(&descriptor)
            .ok()?;
        let create_ns = t.elapsed().as_nanos() as u64;
        let t = Instant::now();
        let allocation: &ProtocolObject<dyn MTLAllocation> = ProtocolObject::from_ref(&*buffer);
        set.addAllocation(allocation);
        let add_ns = t.elapsed().as_nanos() as u64;
        let t = Instant::now();
        set.commit();
        let commit_ns = t.elapsed().as_nanos() as u64;
        let t = Instant::now();
        set.requestResidency();
        let request_ns = t.elapsed().as_nanos() as u64;
        g.residency.push((handle, set));
        Some(ResidencyTimes {
            create_ns,
            add_ns,
            commit_ns,
            request_ns,
        })
    })?
}

/// What the handle's residency set reports: (contains the buffer, count).
pub fn residency_state(handle: u32) -> Option<(bool, u64)> {
    with(|g| {
        let buffer = g.buffer(handle)?.clone();
        let (_, set) = g.residency.iter().find(|(h, _)| *h == handle)?;
        let allocation: &ProtocolObject<dyn MTLAllocation> = ProtocolObject::from_ref(&*buffer);
        Some((
            set.containsAllocation(allocation),
            set.allocationCount() as u64,
        ))
    })?
}

/// Attaches the handle's set to the queue of `kind`, submits the work that
/// queue supports (a GPU sum for the classic queue, an empty command buffer
/// for Metal 4), detaches it again. Returns the GPU sum for the classic
/// queue and 0 for Metal 4; `None` when anything failed.
pub fn residency_use(handle: u32, kind: Kind, words: u32) -> Option<u32> {
    with(|g| {
        let (_, set) = g.residency.iter().find(|(h, _)| *h == handle)?;
        let set = set.clone();
        match kind {
            Kind::Classic => {
                g.queue.addResidencySet(&set);
                let out = {
                    let buffer = g.buffer(handle)?.clone();
                    // SAFETY: the sink is 4 bytes of shared memory we own.
                    unsafe { g.sink.contents().cast::<u32>().write(0) };
                    g.run(|e| {
                        set_buffer(e, &buffer, 0);
                        set_buffer(e, &g.sink, 1);
                        set_u32(e, words, 2);
                        dispatch(e, &g.sum, words as usize);
                    })
                    // SAFETY: as above.
                    .map(|_| unsafe { g.sink.contents().cast::<u32>().read() })
                };
                g.queue.removeResidencySet(&set);
                out
            }
            Kind::Metal4 => {
                g.mtl4.as_ref()?.queue.addResidencySet(&set);
                let ok = g.empty_mtl4();
                g.mtl4.as_ref()?.queue.removeResidencySet(&set);
                ok.then_some(0)
            }
        }
    })?
}

/// What a clear-and-present on a `CAMetalLayer` saw.
#[derive(Clone, Copy, Debug, Default)]
pub struct Presented {
    pub drawable_width: u32,
    pub drawable_height: u32,
    pub pixel_format: u32,
    /// Top-left pixel read back, as stored (BGRA8 -> `0xAARRGGBB`).
    pub pixel: u32,
    pub readback: bool,
}

/// Clears the layer's next drawable to `rgba` and presents it.
///
/// # Safety
/// `layer` must be a live `CAMetalLayer`.
pub unsafe fn present_clear(layer: *mut c_void, rgba: [f64; 4]) -> Result<Presented, String> {
    autoreleasepool(|_| {
        // SAFETY: forwarded from the caller.
        let layer: &CAMetalLayer =
            unsafe { (layer as *const CAMetalLayer).as_ref() }.ok_or("null layer")?;
        let device = layer.device().ok_or("layer has no device")?;
        let queue = device.newCommandQueue().ok_or("no command queue")?;
        // Readable, so the clear can be read back before it is shown.
        layer.setFramebufferOnly(false);
        let drawable = layer.nextDrawable().ok_or("nextDrawable returned nil")?;
        let texture = drawable.texture();
        let descriptor = MTLRenderPassDescriptor::renderPassDescriptor();
        let attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
        attachment.setTexture(Some(&texture));
        attachment.setLoadAction(MTLLoadAction::Clear);
        attachment.setStoreAction(MTLStoreAction::Store);
        attachment.setClearColor(MTLClearColor {
            red: rgba[0],
            green: rgba[1],
            blue: rgba[2],
            alpha: rgba[3],
        });
        let cb = queue.commandBuffer().ok_or("no command buffer")?;
        let encoder = cb
            .renderCommandEncoderWithDescriptor(&descriptor)
            .ok_or("no encoder")?;
        encoder.endEncoding();
        cb.commit();
        cb.waitUntilCompleted();
        if cb.status() != MTLCommandBufferStatus::Completed {
            return Err(format!("clear pass ended with status {}", cb.status().0));
        }
        let mut out = Presented {
            drawable_width: texture.width() as u32,
            drawable_height: texture.height() as u32,
            pixel_format: texture.pixelFormat().0 as u32,
            ..Presented::default()
        };
        let mut pixel = [0u8; 4];
        let region = MTLRegion {
            origin: MTLOrigin { x: 0, y: 0, z: 0 },
            size: MTLSize {
                width: 1,
                height: 1,
                depth: 1,
            },
        };
        // SAFETY: four bytes for one BGRA8 pixel.
        unsafe {
            texture.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                NonNull::from(&mut pixel).cast(),
                4,
                region,
                0,
            );
        }
        out.pixel = u32::from_le_bytes(pixel);
        out.readback = true;
        ProtocolObject::<dyn MTLDrawable>::from_ref(&*drawable).present();
        Ok(out)
    })
}
