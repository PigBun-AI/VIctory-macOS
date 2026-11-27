//! Swap chains through the wire: `CREATE_SWAPCHAIN`, `PRESENT`, `RESIZE_SWAPCHAIN`,
//! `SWAPCHAIN_INFO`, `DESTROY_SWAPCHAIN`. The surface is a layer that is not in
//! any window (a layer vends drawables without one); the swap chain is made
//! with `PROBE`, so after each present the back has read one texel of the
//! drawable it wrote and `SWAPCHAIN_INFO` reports it.
#![cfg(target_vendor = "apple")]

mod exec_support;

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use exec_support::*;
use objc2::rc::Retained;
use objc2_quartz_core::CAMetalLayer;
use victory_metal_proto::cmd::*;
use victory_metal_proto::ids::*;
use victory_metal_proto::link::{SurfaceProvider, WindowRef};
use victory_metal_proto::status::Status;
use victory_metal_proto::sync::*;
use victory_metal_proto::types::*;

/// Hands out one detached layer per window and counts the releases.
#[derive(Default)]
struct Layers {
    layers: Mutex<Vec<(u64, Retained<CAMetalLayer>)>>,
    released: Mutex<Vec<u64>>,
}
// A layer is thread-safe.
unsafe impl Send for Layers {}
unsafe impl Sync for Layers {}

impl SurfaceProvider for Layers {
    fn layer_for(&self, window: WindowRef) -> *mut c_void {
        if window.0 == 0 {
            return std::ptr::null_mut();
        }
        let layer = CAMetalLayer::new();
        let p = Retained::as_ptr(&layer) as *mut c_void;
        self.layers.lock().unwrap().push((window.0, layer));
        p
    }
    fn release_layer(&self, window: WindowRef) {
        self.released.lock().unwrap().push(window.0);
    }
}

struct World {
    fx: Fx,
    q: QueueId,
    layers: Arc<Layers>,
}

fn world() -> World {
    let fx = Fx::new();
    let q = fx.queue();
    let layers = Arc::new(Layers::default());
    fx.executor().set_surface_provider(layers.clone());
    World { fx, q, layers }
}

fn create(format: u32, w: u32, h: u32, count: u32) -> CreateSwapchainArgs {
    let mut a = CreateSwapchainArgs::new();
    a.window = 0x1234;
    a.width = w;
    a.height = h;
    a.format = format;
    a.buffer_count = count;
    a.usage = tex_usage::UAV;
    a.flags = swapchain_flags::PROBE;
    a
}

impl World {
    /// Clears back buffer `res` to `rgba` (floats) and presents it.
    fn show(&self, sc: SurfaceId, res: ResId, rgba: [f32; 4]) -> Status {
        let mut c = <ClearUavTexture as victory_metal_proto::wire::Wire>::ZERO;
        c.res = res;
        c.dimension = TexViewDim::TEX2D;
        c.float_values = 1;
        c.values = rgba.map(f32::to_bits);
        assert_eq!(
            self.fx
                .submit(&[begin(1, self.q), Command::ClearUavTexture(c), end(1)]),
            Status::OK
        );
        let mut p = PresentArgs::new();
        p.queue = self.q;
        p.swapchain = sc;
        p.buffer = res;
        p.sync_interval = 0;
        self.fx.status(p)
    }

    fn info(&self, sc: SurfaceId) -> SwapchainInfoArgs {
        let mut i = SwapchainInfoArgs::new();
        i.swapchain = sc;
        self.fx.ok(i)
    }
}

fn bgra(rgba: [u8; 4]) -> u32 {
    u32::from_le_bytes([rgba[2], rgba[1], rgba[0], rgba[3]])
}

#[test]
fn every_frame_shows_its_own_clear_colour() {
    let w = world();
    let sc = w.fx.ok(create(format::R8G8B8A8_UNORM, 64, 48, 3));
    assert!(!sc.swapchain.is_null());
    let info = w.info(sc.swapchain);
    assert_eq!((info.width, info.height), (64, 48));
    // BGRA8Unorm is Metal's number 80.
    assert_eq!(info.surface_format, 80);
    let colours: [[u8; 4]; 5] = [
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 255, 255],
        [0, 0, 0, 255],
    ];
    for (i, c) in colours.iter().enumerate() {
        let res = ResId(sc.buffers[i % 3]);
        let f = c.map(|v| f32::from(v) / 255.0);
        assert_eq!(w.show(sc.swapchain, res, f), Status::OK, "frame {i}");
        let info = w.info(sc.swapchain);
        assert_eq!(info.probe_valid, 1);
        assert_eq!(
            (info.probe_x, info.probe_y),
            (32, 24),
            "the centre texel is read"
        );
        assert_eq!(
            info.probe_texel as u32,
            bgra(*c),
            "frame {i}: {:#010x}",
            info.probe_texel
        );
        assert_eq!(info.presents, i as u64 + 1);
    }
    assert_eq!(w.fx.status(destroy(sc.swapchain)), Status::OK);
    assert_eq!(*w.layers.released.lock().unwrap(), vec![0x1234]);
    assert_eq!(w.fx.status(destroy(sc.swapchain)), Status::NOT_FOUND);
}

fn destroy(sc: SurfaceId) -> DestroySwapchainArgs {
    let mut d = DestroySwapchainArgs::new();
    d.swapchain = sc;
    d
}

#[test]
fn formats_convert_and_sizes_scale() {
    let w = world();
    // B8G8R8A8 back buffer: the same bytes as the drawable.
    let sc = w.fx.ok(create(format::B8G8R8A8_UNORM, 32, 32, 2));
    assert_eq!(
        w.show(sc.swapchain, ResId(sc.buffers[0]), [1.0, 0.5, 0.0, 1.0]),
        Status::OK
    );
    let i = w.info(sc.swapchain);
    // 0.5 stored as 128 (round to nearest of 127.5).
    let px = i.probe_texel as u32;
    assert_eq!(px & 0xFF, 0, "blue");
    assert!((px >> 8 & 0xFF).abs_diff(128) <= 1, "green {px:#x}");
    assert_eq!(px >> 16 & 0xFF, 255, "red");
    // R16G16B16A16_FLOAT surface: halves.
    let hf = w.fx.ok(create(format::R16G16B16A16_FLOAT, 16, 16, 2));
    assert_eq!(hf.surface_format, 115);
    assert_eq!(
        w.show(hf.swapchain, ResId(hf.buffers[1]), [0.25, 0.5, 1.0, 1.0]),
        Status::OK
    );
    let t = w.info(hf.swapchain).probe_texel;
    let half = |bits: u16| f32::from_bits(half_to_f32(bits));
    assert_eq!(half(t as u16), 0.25);
    assert_eq!(half((t >> 16) as u16), 0.5);
    assert_eq!(half((t >> 32) as u16), 1.0);

    // A resize: new back buffers, the surface follows, old ones are the caller's.
    for b in &sc.buffers[..2] {
        assert_eq!(w.fx.status(destroy_texture(ResId(*b))), Status::OK);
    }
    let mut r = ResizeSwapchainArgs::new();
    r.swapchain = sc.swapchain;
    (r.width, r.height) = (80, 60);
    let r = w.fx.ok(r);
    assert_ne!(r.buffers[0], 0);
    let i = w.info(sc.swapchain);
    assert_eq!((i.width, i.height), (80, 60));
    assert_eq!(
        w.show(sc.swapchain, ResId(r.buffers[1]), [0.0, 0.0, 1.0, 1.0]),
        Status::OK
    );
    let i = w.info(sc.swapchain);
    assert_eq!((i.probe_x, i.probe_y), (40, 30));
    assert_eq!(i.probe_texel as u32, bgra([0, 0, 255, 255]));
}

fn destroy_texture(res: ResId) -> DestroyTextureArgs {
    let mut d = DestroyTextureArgs::new();
    d.res = res;
    d
}

/// Half-precision to the bits of an `f32`.
fn half_to_f32(h: u16) -> u32 {
    let sign = u32::from(h >> 15) << 31;
    let exp = u32::from(h >> 10 & 0x1F);
    let man = u32::from(h & 0x3FF);
    assert!(exp != 0 && exp != 31, "only normal halves in this test");
    sign | ((exp + 112) << 23) | (man << 13)
}

#[test]
fn refusals_have_the_right_status() {
    let w = world();
    // Not a format a flip-model swap chain has.
    let mut a = create(format::R8G8B8A8_UNORM_SRGB, 8, 8, 2);
    assert_eq!(w.fx.status(a), Status::UNSUPPORTED);
    a = create(format::R8G8B8A8_UNORM, 8, 8, 0);
    assert_eq!(w.fx.status(a), Status::INVALID_ARG);
    a = create(format::R8G8B8A8_UNORM, 8, 8, 17);
    assert_eq!(w.fx.status(a), Status::INVALID_ARG);
    a = create(format::R8G8B8A8_UNORM, 0, 8, 2);
    assert_eq!(w.fx.status(a), Status::INVALID_ARG);
    // A window the window system has no layer for.
    a = create(format::R8G8B8A8_UNORM, 8, 8, 2);
    a.window = 0;
    assert_eq!(w.fx.status(a), Status::UNSUPPORTED);
    assert!(w.fx.executor().last_error().unwrap().contains("window"));
    // Present with bad handles.
    let sc = w.fx.ok(create(format::R8G8B8A8_UNORM, 8, 8, 2));
    let mut p = PresentArgs::new();
    p.queue = w.q;
    p.swapchain = SurfaceId(99);
    p.buffer = ResId(sc.buffers[0]);
    assert_eq!(w.fx.status(p), Status::NOT_FOUND);
    p.swapchain = sc.swapchain;
    p.buffer = ResId(9999);
    assert_eq!(w.fx.status(p), Status::NOT_FOUND);
    p.buffer = ResId(sc.buffers[0]);
    p.queue = QueueId(99);
    assert_eq!(w.fx.status(p), Status::NOT_FOUND);
    p.queue = w.q;
    p.flags = 0x80;
    assert_eq!(w.fx.status(p), Status::INVALID_ARG);
}

#[test]
fn no_provider_means_unsupported() {
    let fx = Fx::new();
    assert_eq!(
        fx.status(create(format::R8G8B8A8_UNORM, 8, 8, 2)),
        Status::UNSUPPORTED
    );
}

#[test]
fn many_presents_keep_within_the_latency() {
    let w = world();
    let sc = w.fx.ok(create(format::R8G8B8A8_UNORM, 32, 32, 2));
    let mut l = SetSwapchainLatencyArgs::new();
    l.swapchain = sc.swapchain;
    l.max_latency = 2;
    w.fx.ok(l);
    // Without the probe the present does not wait for the GPU.
    let mut a = create(format::R8G8B8A8_UNORM, 32, 32, 2);
    a.flags = 0;
    a.window = 7;
    let free = w.fx.ok(a);
    for i in 0..60u32 {
        let c = [(i % 7) as f32 / 7.0, 0.2, 0.4, 1.0];
        assert_eq!(
            w.show(free.swapchain, ResId(free.buffers[(i % 2) as usize]), c),
            Status::OK
        );
    }
    let i = w.info(free.swapchain);
    assert_eq!(i.presents, 60);
    assert!(i.presents >= i.completed);
}

#[test]
fn a_back_buffer_of_another_size_is_scaled() {
    let w = world();
    let sc = w.fx.ok(create(format::R8G8B8A8_UNORM, 16, 16, 2));
    let mut r = ResizeSwapchainArgs::new();
    r.swapchain = sc.swapchain;
    (r.width, r.height) = (48, 32);
    w.fx.ok(r);
    // The old 16x16 buffer is still the caller's: it is stretched over the 48x32 surface.
    assert_eq!(
        w.show(sc.swapchain, ResId(sc.buffers[0]), [0.0, 1.0, 0.0, 1.0]),
        Status::OK
    );
    let i = w.info(sc.swapchain);
    assert_eq!((i.width, i.height, i.probe_x, i.probe_y), (48, 32, 24, 16));
    assert_eq!(i.probe_texel as u32, bgra([0, 255, 0, 255]));
}
