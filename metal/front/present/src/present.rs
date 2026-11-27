//! The checks. Unsafe COM, as in the callers this imitates.
#![allow(clippy::missing_safety_doc)]

use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Interface, w};

type Res<T> = Result<T, String>;

fn e(what: &str) -> impl Fn(windows::core::Error) -> String + '_ {
    move |err| format!("{what}: {:#010x}", err.code().0 as u32)
}

/// (r, g, b): distinct, and none of them a value a broken copy would give by accident.
const COLOURS: [(u8, u8, u8); 8] = [
    (255, 0, 0),
    (0, 255, 0),
    (0, 0, 255),
    (255, 255, 0),
    (255, 0, 255),
    (0, 255, 255),
    (255, 136, 0),
    (51, 102, 204),
];

struct Opts {
    frames: u32,
    bgra: bool,
    hold: u64,
    waitable: bool,
}

fn options() -> Opts {
    let mut o = Opts {
        frames: 6,
        bgra: false,
        hold: 0,
        waitable: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--frames" => o.frames = args.next().and_then(|v| v.parse().ok()).unwrap_or(6),
            "--hold" => o.hold = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--bgra" => o.bgra = true,
            "--waitable" => o.waitable = true,
            other => eprintln!("ignoring {other}"),
        }
    }
    o
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(h, m, w, l) }
}

unsafe fn make_window(w: i32, h: i32) -> Res<HWND> {
    unsafe {
        let inst = GetModuleHandleW(None).map_err(e("GetModuleHandle"))?;
        let class = w!("VictoryMetalPresentTest");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: inst.into(),
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err("RegisterClassExW failed".into());
        }
        let mut r = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        let style = WS_OVERLAPPEDWINDOW;
        let _ = AdjustWindowRect(&mut r, style, false);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("victory-metal present test"),
            style | WS_VISIBLE,
            100,
            100,
            r.right - r.left,
            r.bottom - r.top,
            None,
            None,
            Some(inst.into()),
            None,
        )
        .map_err(e("CreateWindowExW"))?;
        pump();
        Ok(hwnd)
    }
}

fn pump() {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn client(hwnd: HWND) -> (u32, u32) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut r);
    }
    ((r.right - r.left) as u32, (r.bottom - r.top) as u32)
}

struct Gpu {
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    alloc: ID3D12CommandAllocator,
    fence: ID3D12Fence,
    event: HANDLE,
    value: u64,
}

impl Gpu {
    unsafe fn sync(&mut self) -> Res<()> {
        unsafe {
            self.value += 1;
            self.queue
                .Signal(&self.fence, self.value)
                .map_err(e("Signal"))?;
            self.fence
                .SetEventOnCompletion(self.value, self.event)
                .map_err(e("SetEventOnCompletion"))?;
            if WaitForSingleObject(self.event, 20_000) != WAIT_OBJECT_0 {
                return Err("fence wait timed out".into());
            }
            Ok(())
        }
    }

    unsafe fn buffer(
        &self,
        heap: D3D12_HEAP_TYPE,
        size: u64,
        state: D3D12_RESOURCE_STATES,
    ) -> Res<ID3D12Resource> {
        unsafe {
            let mut r: Option<ID3D12Resource> = None;
            self.device
                .CreateCommittedResource(
                    &D3D12_HEAP_PROPERTIES {
                        Type: heap,
                        ..Default::default()
                    },
                    D3D12_HEAP_FLAG_NONE,
                    &D3D12_RESOURCE_DESC {
                        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                        Width: size,
                        Height: 1,
                        DepthOrArraySize: 1,
                        MipLevels: 1,
                        SampleDesc: DXGI_SAMPLE_DESC {
                            Count: 1,
                            Quality: 0,
                        },
                        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                        ..Default::default()
                    },
                    state,
                    None,
                    &mut r,
                )
                .map_err(e("CreateCommittedResource"))?;
            r.ok_or_else(|| "no resource".to_owned())
        }
    }
}

fn transition(
    r: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                // SAFETY: a borrowed pointer; the barrier never releases it.
                pResource: unsafe { std::mem::transmute_copy(r) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

fn texture_location(r: &ID3D12Resource) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: a borrowed pointer, never released through the location.
        pResource: unsafe { std::mem::transmute_copy(r) },
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    }
}

fn buffer_location(
    r: &ID3D12Resource,
    format: DXGI_FORMAT,
    w: u32,
    h: u32,
    pitch: u32,
) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: as above.
        pResource: unsafe { std::mem::transmute_copy(r) },
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                Offset: 0,
                Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                    Format: format,
                    Width: w,
                    Height: h,
                    Depth: 1,
                    RowPitch: pitch,
                },
            },
        },
    }
}

/// Fills the back buffer with `bytes` (4 per texel), reads it back, compares.
unsafe fn paint(
    gpu: &mut Gpu,
    buf: &ID3D12Resource,
    format: DXGI_FORMAT,
    w: u32,
    h: u32,
    texel: [u8; 4],
) -> Res<()> {
    unsafe {
        let pitch = (w * 4).div_ceil(256) * 256;
        let size = u64::from(pitch) * u64::from(h);
        let up = gpu.buffer(
            D3D12_HEAP_TYPE_UPLOAD,
            size,
            D3D12_RESOURCE_STATE_GENERIC_READ,
        )?;
        let rb = gpu.buffer(
            D3D12_HEAP_TYPE_READBACK,
            size,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        let mut p: *mut c_void = std::ptr::null_mut();
        up.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut p))
            .map_err(e("Map upload"))?;
        let row = std::slice::from_raw_parts_mut(p.cast::<u8>(), size as usize);
        for y in 0..h as usize {
            for x in 0..w as usize {
                let o = y * pitch as usize + x * 4;
                row[o..o + 4].copy_from_slice(&texel);
            }
        }
        up.Unmap(0, None);
        gpu.alloc.Reset().map_err(e("allocator Reset"))?;
        let list: ID3D12GraphicsCommandList = gpu
            .device
            .CreateCommandList(
                0,
                D3D12_COMMAND_LIST_TYPE_DIRECT,
                &gpu.alloc,
                None::<&ID3D12PipelineState>,
            )
            .map_err(e("CreateCommandList"))?;
        list.ResourceBarrier(&[transition(
            buf,
            D3D12_RESOURCE_STATE_PRESENT,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )]);
        list.CopyTextureRegion(
            &texture_location(buf),
            0,
            0,
            0,
            &buffer_location(&up, format, w, h, pitch),
            None,
        );
        list.ResourceBarrier(&[transition(
            buf,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        )]);
        list.CopyTextureRegion(
            &buffer_location(&rb, format, w, h, pitch),
            0,
            0,
            0,
            &texture_location(buf),
            None,
        );
        list.ResourceBarrier(&[transition(
            buf,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
            D3D12_RESOURCE_STATE_PRESENT,
        )]);
        list.Close().map_err(e("Close"))?;
        let l: ID3D12CommandList = list.cast().map_err(e("cast list"))?;
        gpu.queue.ExecuteCommandLists(&[Some(l)]);
        gpu.sync()?;
        rb.Map(0, None, Some(&mut p)).map_err(e("Map readback"))?;
        let got = std::slice::from_raw_parts(p.cast::<u8>(), size as usize);
        let mut bad = 0usize;
        for y in 0..h as usize {
            for x in 0..w as usize {
                let o = y * pitch as usize + x * 4;
                if got[o..o + 4] != texel {
                    bad += 1;
                }
            }
        }
        rb.Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }));
        if bad != 0 {
            return Err(format!("{bad} texels of {w}x{h} differ from {texel:02x?}"));
        }
        Ok(())
    }
}

/// The colour of frame `n` as (bytes in the buffer's format, 0xAARRGGBB).
fn colour(n: u32, bgra: bool) -> ([u8; 4], u32) {
    let (r, g, b) = COLOURS[n as usize % COLOURS.len()];
    let bytes = if bgra { [b, g, r, 255] } else { [r, g, b, 255] };
    (
        bytes,
        0xFF00_0000 | u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b),
    )
}

struct Report {
    pass: u32,
    fail: u32,
}

impl Report {
    fn check(&mut self, name: &str, r: Res<()>) {
        match r {
            Ok(()) => {
                self.pass += 1;
                println!("PRESENT PASS {name}");
            }
            Err(why) => {
                self.fail += 1;
                println!("PRESENT FAIL {name}: {why}");
            }
        }
    }
}

unsafe fn frames(
    gpu: &mut Gpu,
    sc: &IDXGISwapChain3,
    from: u32,
    count: u32,
    format: DXGI_FORMAT,
    bgra: bool,
) -> Res<()> {
    unsafe {
        for n in from..from + count {
            pump();
            let d = sc.GetDesc1().map_err(e("GetDesc1"))?;
            let index = sc.GetCurrentBackBufferIndex();
            let buf: ID3D12Resource = sc.GetBuffer(index).map_err(e("GetBuffer"))?;
            let (bytes, argb) = colour(n, bgra);
            paint(gpu, &buf, format, d.Width, d.Height, bytes)?;
            println!(
                "EXPECT frame {} texel {:#010x} size {}x{}",
                n + 1,
                argb,
                d.Width,
                d.Height
            );
            let hr = sc.Present(1, DXGI_PRESENT(0));
            if hr.is_err() {
                return Err(format!("Present: {:#010x}", hr.0 as u32));
            }
        }
        Ok(())
    }
}

unsafe fn scenario(report: &mut Report, o: &Opts) -> Res<()> {
    unsafe {
        let hwnd = make_window(640, 360)?;
        let (cw, ch) = client(hwnd);
        println!("INFO window client {cw}x{ch}");
        let factory: IDXGIFactory4 =
            CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).map_err(e("CreateDXGIFactory2"))?;
        let adapter = factory.EnumAdapters1(0).map_err(e("EnumAdapters1"))?;
        let mut device: Option<ID3D12Device> = None;
        D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_12_0, &mut device)
            .map_err(e("D3D12CreateDevice"))?;
        let device = device.ok_or("no device")?;
        let queue: ID3D12CommandQueue = device
            .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                ..Default::default()
            })
            .map_err(e("CreateCommandQueue"))?;
        let alloc = device
            .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
            .map_err(e("CreateCommandAllocator"))?;
        let fence = device
            .CreateFence(0, D3D12_FENCE_FLAG_NONE)
            .map_err(e("CreateFence"))?;
        let event = CreateEventW(None, false, false, None).map_err(e("CreateEventW"))?;
        let mut gpu = Gpu {
            device,
            queue,
            alloc,
            fence,
            event,
            value: 0,
        };

        let format = if o.bgra {
            DXGI_FORMAT_B8G8R8A8_UNORM
        } else {
            DXGI_FORMAT_R8G8B8A8_UNORM
        };
        let flags = if o.waitable {
            DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32
        } else {
            0
        };
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: 0,
            Height: 0,
            Format: format,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: flags,
        };
        let sc1 = factory
            .CreateSwapChainForHwnd(&gpu.queue, hwnd, &desc, None, None)
            .map_err(e("CreateSwapChainForHwnd"))?;
        let sc: IDXGISwapChain3 = sc1.cast().map_err(e("cast IDXGISwapChain3"))?;
        report.check("create", Ok(()));

        let d = sc.GetDesc1().map_err(e("GetDesc1"))?;
        report.check(
            "desc from window size",
            if (d.Width, d.Height) == (cw, ch) && d.Format == format && d.BufferCount == 2 {
                Ok(())
            } else {
                Err(format!(
                    "{}x{} format {} count {}",
                    d.Width, d.Height, d.Format.0, d.BufferCount
                ))
            },
        );
        let hw = sc.GetHwnd().map_err(e("GetHwnd"));
        report.check(
            "GetHwnd",
            hw.and_then(|h| {
                if h == hwnd {
                    Ok(())
                } else {
                    Err("another window".into())
                }
            }),
        );
        report.check(
            "same buffer object each time",
            (|| {
                let a: ID3D12Resource = sc.GetBuffer(1).map_err(e("GetBuffer"))?;
                let b: ID3D12Resource = sc.GetBuffer(1).map_err(e("GetBuffer"))?;
                if a.as_raw() == b.as_raw() {
                    Ok(())
                } else {
                    Err("two objects".into())
                }
            })(),
        );
        report.check(
            "buffer index out of range",
            match sc.GetBuffer::<ID3D12Resource>(2) {
                Err(_) => Ok(()),
                Ok(_) => Err("GetBuffer(2) of 2 buffers worked".into()),
            },
        );
        if o.waitable {
            let h = sc
                .cast::<IDXGISwapChain2>()
                .map_err(e("cast IDXGISwapChain2"))?;
            let w = h.GetFrameLatencyWaitableObject();
            let r = if w.is_invalid() {
                Err("no waitable object".to_owned())
            } else if WaitForSingleObject(w, 1000) != WAIT_OBJECT_0 {
                Err("waitable object not signalled".to_owned())
            } else {
                h.SetMaximumFrameLatency(2)
                    .map_err(e("SetMaximumFrameLatency"))?;
                if h.GetMaximumFrameLatency()
                    .map_err(e("GetMaximumFrameLatency"))?
                    == 2
                {
                    Ok(())
                } else {
                    Err("latency not kept".to_owned())
                }
            };
            report.check("frame latency waitable object", r);
        }

        let t0 = Instant::now();
        report.check("frames", frames(&mut gpu, &sc, 0, o.frames, format, o.bgra));
        println!("INFO {} frames in {:?}", o.frames, t0.elapsed());
        report.check(
            "index advances",
            if sc.GetCurrentBackBufferIndex() == o.frames % 2 {
                Ok(())
            } else {
                Err(format!("index {}", sc.GetCurrentBackBufferIndex()))
            },
        );

        // The window grows, the buffers follow.
        SetWindowPos(
            hwnd,
            None,
            100,
            100,
            0,
            0,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
        .ok();
        let mut r = RECT {
            left: 0,
            top: 0,
            right: 800,
            bottom: 450,
        };
        let _ = AdjustWindowRect(&mut r, WS_OVERLAPPEDWINDOW, false);
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
        .map_err(e("SetWindowPos"))?;
        pump();
        let (nw, nh) = client(hwnd);
        println!("INFO window client now {nw}x{nh}");
        report.check(
            "ResizeBuffers",
            sc.ResizeBuffers(
                0,
                0,
                0,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(flags as i32),
            )
            .map_err(e("ResizeBuffers")),
        );
        let d = sc.GetDesc1().map_err(e("GetDesc1"))?;
        report.check(
            "desc after resize",
            if (d.Width, d.Height) == (nw, nh) && d.Format == format {
                Ok(())
            } else {
                Err(format!("{}x{} format {}", d.Width, d.Height, d.Format.0))
            },
        );
        report.check(
            "frames after resize",
            frames(&mut gpu, &sc, o.frames, o.frames, format, o.bgra),
        );

        if o.hold > 0 {
            // One steady colour for a screenshot (orange).
            let end = Instant::now() + Duration::from_secs(o.hold);
            println!(
                "INFO holding colour {:#010x} for {} s",
                colour(6, o.bgra).1,
                o.hold
            );
            while Instant::now() < end {
                report.check_quiet(frames(&mut gpu, &sc, 6, 1, format, o.bgra));
            }
        }
        DestroyWindow(hwnd).ok();
        Ok(())
    }
}

impl Report {
    fn check_quiet(&mut self, r: Res<()>) {
        if let Err(why) = r {
            self.fail += 1;
            println!("PRESENT FAIL hold: {why}");
        }
    }
}

pub fn run() -> i32 {
    let o = options();
    let mut report = Report { pass: 0, fail: 0 };
    // SAFETY: COM calls as an application makes them.
    if let Err(why) = unsafe { scenario(&mut report, &o) } {
        report.fail += 1;
        println!("PRESENT FAIL scenario: {why}");
    }
    println!("PRESENT SUMMARY pass={} fail={}", report.pass, report.fail);
    i32::from(report.fail != 0)
}
