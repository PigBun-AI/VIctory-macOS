//! `VICTORY_METAL_DUMP_PRESENT`: the drawable the display gets and the source back buffer, written for
//! the same present. At equal sizes they are byte-equal; at different sizes the drawable is the nearest
//! scaling of the source. One test per file: it sets the environment variable before any swap chain.
#![cfg(target_vendor = "apple")]

mod exec_support;

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use exec_support::*;
use objc2::rc::Retained;
use objc2_quartz_core::CAMetalLayer;
use victory_metal_back::dump::read_png_rgba8;
use victory_metal_proto::cmd::*;
use victory_metal_proto::ids::*;
use victory_metal_proto::link::{SurfaceProvider, WindowRef};
use victory_metal_proto::status::Status;
use victory_metal_proto::sync::*;
use victory_metal_proto::types::*;

#[derive(Default)]
struct Layers(Mutex<Vec<Retained<CAMetalLayer>>>);
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
        self.0.lock().unwrap().push(layer);
        p
    }
    fn release_layer(&self, _: WindowRef) {}
}

fn present_clear(fx: &Fx, q: QueueId, sc: &CreateSwapchainArgs, i: usize, rgba: [f32; 4]) {
    let res = ResId(sc.buffers[i]);
    let mut c = <ClearUavTexture as victory_metal_proto::wire::Wire>::ZERO;
    c.res = res;
    c.dimension = TexViewDim::TEX2D;
    c.float_values = 1;
    c.values = rgba.map(f32::to_bits);
    assert_eq!(
        fx.submit(&[begin(1, q), Command::ClearUavTexture(c), end(1)]),
        Status::OK
    );
    let mut p = PresentArgs::new();
    p.queue = q;
    p.swapchain = sc.swapchain;
    p.buffer = res;
    p.sync_interval = 0;
    assert_eq!(fx.status(p), Status::OK);
}

#[test]
fn the_drawable_equals_the_source_at_equal_size_and_scales_otherwise() {
    let dir = std::env::temp_dir().join(format!("victory-dump-present-{}", std::process::id()));
    // SAFETY: the only test of this binary, nothing else reads the environment concurrently.
    unsafe {
        std::env::set_var(
            "VICTORY_METAL_DUMP_PRESENT",
            format!("1,2@{}", dir.display()),
        )
    };
    let fx = Fx::new();
    let q = fx.queue();
    fx.executor()
        .set_surface_provider(Arc::new(Layers::default()));

    // present 1: 64x36 buffer, 64x36 drawable.
    let mut a = CreateSwapchainArgs::new();
    a.window = 0x1234;
    (a.width, a.height) = (64, 36);
    a.format = format::R8G8B8A8_UNORM;
    a.buffer_count = 3;
    a.usage = tex_usage::UAV;
    let sc: CreateSwapchainArgs = fx.ok(a);
    present_clear(&fx, q, &sc, 0, [1.0, 0.5, 0.25, 1.0]);
    let read = |n: u64, what: &str| {
        let bytes = std::fs::read(dir.join(format!("present-{n}-{what}.png"))).unwrap();
        read_png_rgba8(&bytes).unwrap()
    };
    let (sw, sh, src) = read(1, "source");
    let (dw, dh, dst) = read(1, "drawable");
    assert_eq!((sw, sh, dw, dh), (64, 36, 64, 36));
    assert_eq!(src, dst, "same size: byte-equal");
    assert_eq!(
        &src[0..4],
        &[255, 128, 64, 255],
        "RGBA order, 0.5 -> 128, 0.25 -> 64"
    );

    // present 2: the swap chain is made 128x72 but the caller still presents its old 64x36 buffer.
    let mut r = ResizeSwapchainArgs::new();
    r.swapchain = sc.swapchain;
    (r.width, r.height) = (128, 72);
    fx.ok(r);
    present_clear(&fx, q, &sc, 1, [0.0, 1.0, 0.0, 1.0]);
    let (sw, sh, src) = read(2, "source");
    let (dw, dh, dst) = read(2, "drawable");
    assert_eq!((sw, sh, dw, dh), (64, 36, 128, 72));
    for y in 0..dh as usize {
        for x in 0..dw as usize {
            let s = ((y / 2) * sw as usize + x / 2) * 4;
            let d = (y * dw as usize + x) * 4;
            assert_eq!(&dst[d..d + 4], &src[s..s + 4], "({x},{y})");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
