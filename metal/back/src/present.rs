//! Swap chains: showing a back buffer in a window's `CAMetalLayer`.
//!
//! The back buffers are ordinary textures the caller makes (they are what the
//! application renders into); this module owns the layer side. A present is:
//!
//! 1. wait (on the CPU) until fewer than `max_latency` presents are in
//!    flight on the queue;
//! 2. `layer.nextDrawable()`;
//! 3. record one list with a compute kernel that copies the back buffer into
//!    the drawable's texture (the format conversion is the texture write's:
//!    an RGBA8 back buffer into a BGRA8 drawable needs no swizzle; sizes that
//!    differ are scaled nearest-neighbour);
//! 4. `queue.waitForDrawable(drawable)`, commit the list, `queue.signalDrawable(drawable)`
//!    ([`Queue::submit_drawable`]), then `drawable.present()`
//!    ([`Queue::present_drawable`]: the submit thread makes it right after
//!    `signalDrawable`).
//!
//! The layer's `residencySet` (it holds the drawables' memory) is attached to
//! every queue that presents to it, once. The layer is set to
//! `framebufferOnly = NO` because the kernel writes to the drawable.
//!
//! What it does not do: more than one refresh per present (`sync_interval`
//! above 1 is treated as 1), tearing control beyond `displaySyncEnabled`, HDR
//! metadata or colour management (the layer's colour space is left as the
//! window system set it).

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::Arc;

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSSize;
use objc2_metal::*;
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer, kCAGravityResizeAspect};

use crate::device::Device;
use crate::error::{Error, Result};
use crate::format::dxgi as d;
use crate::pipeline::ComputePipeline;
use crate::queue::Queue;
use crate::texture::{Texture, ViewDim, ViewSpec};
use crate::typed::resource_id_of;

const SRC_SLOT: u32 = 29;
const DST_SLOT: u32 = 28;
const PARAM_SLOT: u32 = 30;
/// Drawables the layer may have out at once (Metal's allowed range is 2 or 3).
const DRAWABLES: usize = 3;
/// Frames in flight when the caller does not say (the task: 2-3).
pub const DEFAULT_LATENCY: u32 = 3;
/// Largest `max_latency`.
pub const MAX_LATENCY: u32 = 8;
/// How long a present waits for an older one before it gives up and goes on.
const LATENCY_WAIT_MS: u64 = 2_000;

const KERNEL: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
struct SrcEntry { ulong va; texture2d<float, access::read> tex; ulong md; };\n\
struct DstEntry { ulong va; texture2d<float, access::write> tex; ulong md; };\n\
struct P { uint sw; uint sh; uint dw; uint dh; };\n\
kernel void present_copy(device const SrcEntry* s [[buffer(29)]], device const DstEntry* d [[buffer(28)]], \
constant P& p [[buffer(30)]], uint2 t [[thread_position_in_grid]]) {\n\
    if (t.x >= p.dw || t.y >= p.dh) return;\n\
    uint2 c = uint2(min(p.sw - 1, uint((ulong(t.x) * p.sw) / p.dw)), min(p.sh - 1, uint((ulong(t.y) * p.sh) / p.dh)));\n\
    texture2d<float, access::read> src = s->tex;\n\
    texture2d<float, access::write> dst = d->tex;\n\
    dst.write(src.read(c), t);\n}\n";

/// The Metal pixel format of the layer for a back-buffer format, or why not.
pub fn surface_format(dxgi: u32) -> Result<MTLPixelFormat> {
    match dxgi {
        d::R8G8B8A8_UNORM | d::B8G8R8A8_UNORM | d::R10G10B10A2_UNORM => {
            Ok(MTLPixelFormat::BGRA8Unorm)
        }
        d::R16G16B16A16_FLOAT => Ok(MTLPixelFormat::RGBA16Float),
        other => Err(Error::Unsupported(format!(
            "swap chain format {other} (flip-model swap chains take R8G8B8A8_UNORM, B8G8R8A8_UNORM, R10G10B10A2_UNORM, R16G16B16A16_FLOAT)"
        ))),
    }
}

/// What a swap chain can tell (see `SWAPCHAIN_INFO`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SwapchainInfo {
    pub width: u32,
    pub height: u32,
    /// The layer's `MTLPixelFormat` number.
    pub surface_format: u32,
    pub probe_valid: bool,
    pub probe_texel: u64,
    pub probe_x: u32,
    pub probe_y: u32,
    /// Presents handed to the GPU.
    pub presents: u64,
    /// Presents the GPU has finished.
    pub completed: u64,
    /// Presents dropped because the layer had no drawable in time.
    pub dropped: u64,
}

/// One window's presentation state.
pub struct Swapchain {
    device: Device,
    layer: Retained<CAMetalLayer>,
    kernel: Option<ComputePipeline>,
    width: u32,
    height: u32,
    pixel: MTLPixelFormat,
    max_latency: u32,
    /// Timeline values of presents not yet known to be complete, per queue.
    in_flight: VecDeque<(usize, Arc<crate::queue::Timeline>, u64)>,
    /// Queues whose residency sets already include the layer's.
    attached: Vec<usize>,
    probe: bool,
    /// `VICTORY_METAL_DUMP_PRESENT` (see [`crate::dump`]).
    dump: Option<crate::dump::DumpSpec>,
    dump_kernel: Option<ComputePipeline>,
    last_probe: Option<(u64, u32, u32)>,
    presents: u64,
    dropped: u64,
}

// The layer and the textures it hands out are thread-safe; a swap chain is
// used under the executor's state lock.
unsafe impl Send for Swapchain {}
unsafe impl Sync for Swapchain {}

impl Swapchain {
    /// Takes a reference to `layer` (a `CAMetalLayer`) and sets it up for
    /// drawables of `width` x `height` in the format of `dxgi_format`.
    ///
    /// # Safety
    /// `layer` must point to a live `CAMetalLayer`.
    pub unsafe fn new(
        device: &Device,
        layer: *mut c_void,
        width: u32,
        height: u32,
        dxgi_format: u32,
        max_latency: u32,
        probe: bool,
    ) -> Result<Swapchain> {
        // SAFETY: forwarded from the caller.
        let layer = unsafe { Retained::retain(layer.cast::<CAMetalLayer>()) }
            .ok_or_else(|| Error::BadArgument("null layer".into()))?;
        let mut sc = Swapchain {
            device: device.clone(),
            layer,
            kernel: None,
            width: 0,
            height: 0,
            pixel: MTLPixelFormat::BGRA8Unorm,
            max_latency: clamp_latency(max_latency),
            in_flight: VecDeque::new(),
            attached: Vec::new(),
            probe,
            dump: crate::dump::DumpSpec::from_env(),
            dump_kernel: None,
            last_probe: None,
            presents: 0,
            dropped: 0,
        };
        sc.layer.setDevice(Some(device.raw()));
        // The kernel writes the drawable.
        sc.layer.setFramebufferOnly(false);
        // The default gravity (`resize`) stretches the drawable over the layer without keeping its
        // aspect ratio: Wine's client area is not 16:9 (1272x686 pt for a 1280x720 request), so the
        // picture came out squeezed by about 4%. The title's output is scaled uniformly to the
        // display (research/present-size-20261127.md). The bars are the layer's background.
        // SAFETY: a constant of QuartzCore, valid for the process lifetime.
        sc.layer
            .setContentsGravity(unsafe { kCAGravityResizeAspect });
        sc.layer.setMaximumDrawableCount(DRAWABLES);
        sc.layer.setAllowsNextDrawableTimeout(true);
        sc.resize(width, height, Some(dxgi_format))?;
        sc.log_layer("created");
        Ok(sc)
    }

    /// The layer's gravity as the string CoreAnimation reports.
    pub fn contents_gravity(&self) -> String {
        self.layer.contentsGravity().to_string()
    }

    /// `VICTORY_METAL_LOG=1`: what the layer looks like (bounds in points, scale, drawable size, gravity).
    fn log_layer(&self, what: &str) {
        if !std::env::var_os("VICTORY_METAL_LOG").is_some_and(|v| v != "0") {
            return;
        }
        let b = self.layer.bounds();
        let d = self.layer.drawableSize();
        eprintln!(
            "victory-metal back: swap chain {what}: layer bounds {}x{} pt at ({}, {}), contentsScale {}, drawableSize {}x{}, contentsGravity {}, format {}",
            b.size.width,
            b.size.height,
            b.origin.x,
            b.origin.y,
            self.layer.contentsScale(),
            d.width,
            d.height,
            self.layer.contentsGravity(),
            self.pixel.0,
        );
    }

    /// New drawable size (and format, if given).
    pub fn resize(&mut self, width: u32, height: u32, dxgi_format: Option<u32>) -> Result<()> {
        if width == 0 || height == 0 || width > 16384 || height > 16384 {
            return Err(Error::BadArgument(format!(
                "swap chain size {width}x{height}"
            )));
        }
        if let Some(f) = dxgi_format {
            self.pixel = surface_format(f)?;
            self.layer.setPixelFormat(self.pixel);
        }
        self.layer.setDrawableSize(NSSize {
            width: f64::from(width),
            height: f64::from(height),
        });
        self.width = width;
        self.height = height;
        self.log_layer("resized");
        Ok(())
    }

    /// Frames that may be in flight (1 to [`MAX_LATENCY`]; 0 = default).
    pub fn set_max_latency(&mut self, n: u32) {
        self.max_latency = clamp_latency(n);
    }

    /// What is known about the swap chain.
    pub fn info(&self) -> SwapchainInfo {
        let (texel, x, y) = self.last_probe.unwrap_or((0, 0, 0));
        SwapchainInfo {
            width: self.width,
            height: self.height,
            surface_format: self.pixel.0 as u32,
            probe_valid: self.last_probe.is_some(),
            probe_texel: texel,
            probe_x: x,
            probe_y: y,
            presents: self.presents,
            completed: self.presents
                - self
                    .in_flight
                    .iter()
                    .filter(|(_, tl, v)| tl.completed() < *v)
                    .count() as u64,
            dropped: self.dropped,
        }
    }

    /// Shows `src` after the work already committed to `queue`.
    pub fn present(
        &mut self,
        queue: &Queue,
        src: &Texture,
        sync_interval: u32,
        _tearing: bool,
    ) -> Result<()> {
        if !src.belongs_to(&self.device) {
            return Err(Error::BadArgument("back buffer of another device".into()));
        }
        let sd = *src.desc();
        if sd.samples != 1 || sd.dim != crate::texture::TexDim::D2 || sd.array_len() != 1 {
            return Err(Error::BadArgument(format!(
                "back buffer must be a plain 2D texture: {sd:?}"
            )));
        }
        self.wait_for_room();
        autoreleasepool(|_| self.present_inner(queue, src, sync_interval))
    }

    /// Blocks while `max_latency` presents are still in flight.
    fn wait_for_room(&mut self) {
        self.in_flight.retain(|(_, tl, v)| tl.completed() < *v);
        while self.in_flight.len() >= self.max_latency as usize {
            let (_, tl, v) = self.in_flight.front().expect("not empty").clone();
            if !tl.wait(v, LATENCY_WAIT_MS) {
                // The GPU is not finishing: go on rather than hang the game.
                self.in_flight.pop_front();
                continue;
            }
            self.in_flight.pop_front();
        }
    }

    fn pipeline(&mut self) -> Result<ComputePipeline> {
        if let Some(k) = &self.kernel {
            return Ok(k.clone());
        }
        let k = self
            .device
            .compute_pipeline_from_msl(KERNEL, "present_copy")?;
        self.kernel = Some(k.clone());
        Ok(k)
    }

    #[allow(clippy::too_many_arguments)]
    fn write_dump(
        &self,
        spec: &crate::dump::DumpSpec,
        src: &crate::memory::Buffer,
        target: &ProtocolObject<dyn MTLTexture>,
        sw: u32,
        sh: u32,
        dw: u32,
        dh: u32,
    ) {
        let n = self.presents + 1;
        let _ = std::fs::create_dir_all(&spec.dir);
        let Some(p) = src.cpu_ptr() else { return };
        // SAFETY: a readback buffer of sw*sh*4 bytes the finished list wrote.
        let src_px = unsafe { std::slice::from_raw_parts(p, sw as usize * sh as usize * 4) };
        let mut px = vec![0u8; dw as usize * dh as usize * 4];
        let region = MTLRegion {
            origin: MTLOrigin { x: 0, y: 0, z: 0 },
            size: MTLSize {
                width: dw as usize,
                height: dh as usize,
                depth: 1,
            },
        };
        // SAFETY: room for dw*dh texels of 4 bytes (BGRA8; other layer formats are not dumped below).
        unsafe {
            target.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                std::ptr::NonNull::new(px.as_mut_ptr()).unwrap().cast(),
                dw as usize * 4,
                region,
                0,
            );
        }
        let drawable_ok = self.pixel == MTLPixelFormat::BGRA8Unorm;
        for t in px.as_chunks_mut::<4>().0 {
            t.swap(0, 2);
        }
        let s = crate::dump::write_png_rgba8(&spec.path(n, "source"), sw, sh, src_px);
        let d = if drawable_ok {
            crate::dump::write_png_rgba8(&spec.path(n, "drawable"), dw, dh, &px)
        } else {
            Ok(())
        };
        if std::env::var_os("VICTORY_METAL_LOG").is_some_and(|v| v != "0") {
            eprintln!(
                "victory-metal back: dumped present {n}: source {sw}x{sh}, drawable {dw}x{dh} into {} ({s:?}, {d:?})",
                spec.dir.display()
            );
        }
    }

    fn present_inner(&mut self, queue: &Queue, src: &Texture, sync_interval: u32) -> Result<()> {
        let kernel = self.pipeline()?;
        let key = Arc::as_ptr(&queue.inner) as usize;
        if !self.attached.contains(&key) {
            queue.add_residency_set(&self.layer.residencySet())?;
            self.attached.push(key);
        }
        self.layer.setDisplaySyncEnabled(sync_interval != 0);
        let Some(drawable) = self.layer.nextDrawable() else {
            crate::dropstats::silent("present", "no_drawable");
            self.dropped += 1;
            return Ok(());
        };
        let target = drawable.texture();
        let (dw, dh) = (target.width() as u32, target.height() as u32);
        let sd = *src.desc();
        let src_id = src.view(&ViewSpec::whole(ViewDim::D2))?;
        let dst_id = resource_id_of(target.gpuResourceID());

        let mut list = queue.begin_list()?;
        let ring = list.alloc_constants(96, 16)?;
        let mut entry = [0u8; 24];
        entry[8..16].copy_from_slice(&src_id.to_le_bytes());
        ring.write_bytes(0, &entry)?;
        entry[8..16].copy_from_slice(&dst_id.to_le_bytes());
        ring.write_bytes(32, &entry)?;
        ring.write_u32s(64, &[sd.width, sd.height, dw, dh])?;
        list.keep_texture_alive(src);
        list.set_address(SRC_SLOT, ring.gpu_va)?;
        list.set_address(DST_SLOT, ring.gpu_va + 32)?;
        list.set_address(PARAM_SLOT, ring.gpu_va + 64)?;
        list.dispatch(&kernel, [dw.div_ceil(8), dh.div_ceil(8), 1], [8, 8, 1])?;
        // Diagnostics: the source as RGBA8 into a readback buffer, in the same list (the kernel only
        // reads the source; the drawable is read below, after the list).
        let dump = self
            .dump
            .as_ref()
            .filter(|d| d.wants(self.presents + 1))
            .cloned();
        let mut dump_buf = None;
        if dump.is_some() {
            let k = match &self.dump_kernel {
                Some(k) => k.clone(),
                None => {
                    let k = self
                        .device
                        .compute_pipeline_from_msl(crate::dump::DUMP_KERNEL, "present_dump")?;
                    self.dump_kernel = Some(k.clone());
                    k
                }
            };
            let buf = self.device.create_buffer(
                crate::memory::BufferKind::Readback,
                u64::from(sd.width) * u64::from(sd.height) * 4,
            )?;
            list.set_address(DST_SLOT, buf.gpu_va())?;
            list.dispatch(
                &k,
                [sd.width.div_ceil(8), sd.height.div_ceil(8), 1],
                [8, 8, 1],
            )?;
            dump_buf = Some(buf);
        }
        list.end()?;
        let as_drawable: &ProtocolObject<dyn MTLDrawable> = ProtocolObject::from_ref(&*drawable);
        let value = queue.submit_drawable(vec![list], as_drawable)?;
        if let (Some(spec), Some(buf)) = (dump, dump_buf) {
            // After the list, before the display gets the drawable.
            queue.wait_for(value, 10_000)?;
            self.write_dump(&spec, &buf, &target, sd.width, sd.height, dw, dh);
        }
        if self.probe {
            // Read before the present hands the drawable to the display.
            queue.wait_for(value, 5_000)?;
            self.last_probe = Some(read_texel(&target, dw / 2, dh / 2));
        }
        queue.present_drawable(as_drawable);
        self.presents += 1;
        self.in_flight
            .push_back((key, queue.timeline().clone(), value));
        Ok(())
    }
}

fn clamp_latency(n: u32) -> u32 {
    if n == 0 {
        DEFAULT_LATENCY
    } else {
        n.min(MAX_LATENCY)
    }
}

/// The first 8 bytes of the texel at `(x, y)`, with the position.
fn read_texel(t: &ProtocolObject<dyn MTLTexture>, x: u32, y: u32) -> (u64, u32, u32) {
    let mut bytes = [0u8; 8];
    let region = MTLRegion {
        origin: MTLOrigin {
            x: x as usize,
            y: y as usize,
            z: 0,
        },
        size: MTLSize {
            width: 1,
            height: 1,
            depth: 1,
        },
    };
    // SAFETY: room for one texel of the formats a layer has (up to 8 bytes).
    unsafe {
        t.getBytes_bytesPerRow_fromRegion_mipmapLevel(
            std::ptr::NonNull::from(&mut bytes).cast(),
            8,
            region,
            0,
        );
    }
    (u64::from_le_bytes(bytes), x, y)
}

impl crate::record::CommandList {
    /// Keeps `t` alive until the list has run.
    fn keep_texture_alive(&mut self, t: &Texture) {
        self.copies.keep_texture(t);
    }
}
