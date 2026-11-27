//! Can the unixlib get winemac.drv's Metal layer for a window of ours,
//! and can Metal draw into it? (M0 open point 3.)

use std::time::{Duration, Instant};

use victory_metal_bridge::abi::*;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, SW_SHOW,
    ShowWindow, TranslateMessage, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

use crate::suite::Report;

fn pump(duration: Duration) {
    let end = Instant::now() + duration;
    while Instant::now() < end {
        // SAFETY: a plain message pump on the thread that owns the window.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Opens a window, asks the unixlib for its layer, clears and presents it.
/// `hold` keeps the window up afterwards (for a screenshot).
pub fn run<L: RawLink>(link: &L, hold: Duration) -> Report {
    let mut r = Report::default();
    r.section("Metal layer from winemac.drv");
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    let title: Vec<u16> = "VIctory Metal M0 layer probe\0".encode_utf16().collect();
    // SAFETY: a plain top-level window of a system class.
    let hwnd: HWND = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            100,
            100,
            640,
            400,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    r.check("window created", !hwnd.is_null(), format!("hwnd {hwnd:p}"));
    if hwnd.is_null() {
        return r;
    }
    // SAFETY: the window above.
    unsafe { ShowWindow(hwnd, SW_SHOW) };
    pump(Duration::from_millis(500));

    let mut layer = LayerArgs {
        hwnd: hwnd as u64,
        layer: 0,
        view: 0,
        device: 0,
        stage: 0,
        _pad: 0,
    };
    let status = link.call(OP_LAYER_ACQUIRE, (&raw mut layer).cast());
    r.info(
        "acquire stage reached (0 no table, 1 table, 2 win data, 3 device, 4 view, 5 layer)",
        layer.stage.to_string(),
    );
    r.check(
        "unixlib obtained a Metal layer for the HWND",
        status == STATUS_SUCCESS && layer.layer != 0,
        format!(
            "status {status:#x}, layer {:#x}, view {:#x}",
            layer.layer, layer.view
        ),
    );
    if status == STATUS_SUCCESS && layer.layer != 0 {
        let mut present = PresentArgs {
            layer: layer.layer,
            rgba: [0.0, 0.5, 1.0, 1.0],
            width: 0,
            height: 0,
            pixel_format: 0,
            pixel: 0,
            readback: 0,
            _pad: 0,
            error: [0; 160],
        };
        let status = link.call(OP_PRESENT_CLEAR, (&raw mut present).cast());
        r.check(
            "Metal cleared and presented a drawable of that layer",
            status == STATUS_SUCCESS,
            if status == STATUS_SUCCESS {
                format!(
                    "{}x{} pixel format {}, top-left pixel read back {:#010x} (readback {})",
                    present.width,
                    present.height,
                    present.pixel_format,
                    present.pixel,
                    present.readback
                )
            } else {
                format!("status {status:#x}: {}", get_text(&present.error))
            },
        );
        // clear (0, 0.5, 1, 1) in BGRA8 is B=ff G=80 R=00 A=ff.
        if status == STATUS_SUCCESS && present.readback == 1 {
            let expect = 0xff00_80ff_u32;
            r.check(
                "read-back pixel is the clear colour",
                (present.pixel & 0xff00_ffff) == (expect & 0xff00_ffff)
                    && ((present.pixel >> 8) & 0xff).abs_diff(0x80) <= 1,
                format!("{:#010x} expected about {expect:#010x}", present.pixel),
            );
        }
        pump(hold);
        let status = link.call(OP_LAYER_RELEASE, (&raw mut layer).cast());
        r.info("layer released", format!("status {status:#x}"));
    }
    // SAFETY: the window above, destroyed on its own thread.
    unsafe { DestroyWindow(hwnd) };
    r
}
