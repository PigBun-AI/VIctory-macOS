//! `IDXGISwapChain4` over the core's [`SwapChain`](crate::swapchain::SwapChain).
//!
//! The shell does ABI and objects: `GetBuffer` hands out the back buffers as
//! `ID3D12Resource` objects (the same object every time), the window size is
//! read here when the application leaves it to the window, and the
//! frame-latency waitable object is an event that is always signalled (the back
//! blocks `Present` itself when too many frames are in flight).

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, HWND, RECT,
};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::{CreateEventW, GetCurrentProcess};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::core::{
    BOOL, ComObject, GUID, HRESULT, IUnknown, Interface, Result as WResult, implement,
};

use super::dxgi::FactoryObj;
use super::objects::{QueueObj, ResourceObj};
use super::private::PrivateData;
use super::{return_object, werr, wres};
use crate::hresult::Hr;
use crate::resource;
use crate::swapchain::{FLAG_FRAME_LATENCY_WAITABLE_OBJECT, SwapChain, SwapChainDesc};

#[implement(IDXGISwapChain4)]
pub struct SwapChainObj {
    pub private: PrivateData,
    pub factory: ComObject<FactoryObj>,
    pub queue: ComObject<QueueObj>,
    pub hwnd: HWND,
    pub chain: Arc<SwapChain>,
    /// What `GetDesc1` reports besides the core's description.
    pub scaling: DXGI_SCALING,
    pub alpha_mode: DXGI_ALPHA_MODE,
    /// The back buffers handed out so far.
    buffers: Mutex<Vec<Option<ComObject<ResourceObj>>>>,
    /// The waitable object (signalled for good), when the flag asked for one.
    event: Option<isize>,
}

// The event handle is only duplicated and closed.
unsafe impl Send for SwapChainObj {}
unsafe impl Sync for SwapChainObj {}

impl Drop for SwapChainObj {
    fn drop(&mut self) {
        if let Some(h) = self.event {
            // SAFETY: the handle this object made.
            let _ = unsafe { CloseHandle(HANDLE(h as *mut c_void)) };
        }
    }
}

/// The size of a window's client area, or `None` if it has none.
fn client_size(hwnd: HWND) -> Option<(u32, u32)> {
    let mut r = RECT::default();
    // SAFETY: a window handle and a RECT.
    unsafe { GetClientRect(hwnd, &mut r) }.ok()?;
    let (w, h) = (
        (r.right - r.left).max(0) as u32,
        (r.bottom - r.top).max(0) as u32,
    );
    (w != 0 && h != 0).then_some((w, h))
}

impl SwapChainObj {
    /// A swap chain on `queue` for `hwnd`. A width or height of 0 takes the
    /// window's client size.
    pub fn create(
        factory: ComObject<FactoryObj>,
        queue: ComObject<QueueObj>,
        hwnd: HWND,
        mut desc: SwapChainDesc,
        scaling: DXGI_SCALING,
        alpha_mode: DXGI_ALPHA_MODE,
    ) -> WResult<ComObject<SwapChainObj>> {
        if hwnd.0.is_null() {
            return Err(werr(Hr::E_INVALIDARG));
        }
        if desc.width == 0 || desc.height == 0 {
            let (w, h) = client_size(hwnd).ok_or_else(|| werr(Hr::DXGI_ERROR_INVALID_CALL))?;
            if desc.width == 0 {
                desc.width = w;
            }
            if desc.height == 0 {
                desc.height = h;
            }
        }
        let chain = wres(SwapChain::create(
            queue.queue.device(),
            queue.queue.id,
            hwnd.0 as usize as u64,
            desc,
        ))?;
        let event = (desc.flags & FLAG_FRAME_LATENCY_WAITABLE_OBJECT != 0).then(|| {
            // SAFETY: a manual-reset event, signalled.
            unsafe { CreateEventW(None, true, true, None) }
                .map(|h| h.0 as isize)
                .unwrap_or(0)
        });
        Ok(ComObject::new(SwapChainObj {
            private: PrivateData::default(),
            factory,
            queue,
            hwnd,
            chain,
            scaling,
            alpha_mode,
            buffers: Mutex::new(Vec::new()),
            event: event.filter(|h| *h != 0),
        }))
    }

    fn desc1(&self) -> DXGI_SWAP_CHAIN_DESC1 {
        let d = self.chain.desc();
        DXGI_SWAP_CHAIN_DESC1 {
            Width: d.width,
            Height: d.height,
            Format: DXGI_FORMAT(d.format as i32),
            Stereo: BOOL::from(false),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE(d.buffer_usage),
            BufferCount: d.buffer_count,
            Scaling: self.scaling,
            SwapEffect: DXGI_SWAP_EFFECT(d.swap_effect as i32),
            AlphaMode: self.alpha_mode,
            Flags: d.flags,
        }
    }
}

impl SwapChainObj_Impl {
    pub(super) fn x_GetParent(&self, riid: *const GUID, ppparent: *mut *mut c_void) -> WResult<()> {
        if ppparent.is_null() {
            return Err(werr(Hr::E_POINTER));
        }
        // SAFETY: the caller's out pointer.
        unsafe {
            self.factory
                .to_interface::<IDXGIFactory7>()
                .query(riid, ppparent)
        }
        .ok()
    }

    /// The device of the swap chain is the D3D12 device of its queue.
    pub(super) fn x_GetDevice(&self, riid: *const GUID, ppdevice: *mut *mut c_void) -> WResult<()> {
        super::query_device(&self.queue.device_obj, riid, ppdevice)
    }

    pub(super) fn x_Present(&self, syncinterval: u32, flags: DXGI_PRESENT) -> HRESULT {
        match self.chain.present(syncinterval, flags.0) {
            Ok(()) => HRESULT(0),
            Err(h) => werr(h).into(),
        }
    }

    pub(super) fn x_Present1(
        &self,
        syncinterval: u32,
        presentflags: DXGI_PRESENT,
        _ppresentparameters: *const DXGI_PRESENT_PARAMETERS,
    ) -> HRESULT {
        self.x_Present(syncinterval, presentflags)
    }

    pub(super) fn x_GetBuffer(
        &self,
        buffer: u32,
        riid: *const GUID,
        ppsurface: *mut *mut c_void,
    ) -> WResult<()> {
        let mut cache = self.buffers.lock().unwrap();
        let count = self.chain.desc().buffer_count as usize;
        if cache.len() != count {
            cache.resize_with(count, || None);
        }
        let slot = cache
            .get_mut(buffer as usize)
            .ok_or_else(|| werr(Hr::DXGI_ERROR_INVALID_CALL))?;
        if slot.is_none() {
            let resource = wres(self.chain.take_buffer(buffer))?;
            *slot = Some(ComObject::new(ResourceObj {
                private: PrivateData::default(),
                device_obj: self.queue.device_obj.clone(),
                heap_props: D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE(resource::HEAP_DEFAULT as i32),
                    ..Default::default()
                },
                heap_flags: D3D12_HEAP_FLAG_NONE,
                resource,
            }));
        }
        let obj = slot.as_ref().expect("filled above");
        return_object(&obj.to_interface::<IUnknown>(), riid, ppsurface)
    }

    pub(super) fn x_GetCurrentBackBufferIndex(&self) -> u32 {
        self.chain.current_index()
    }

    pub(super) fn x_GetDesc(&self) -> WResult<DXGI_SWAP_CHAIN_DESC> {
        let d = self.desc1();
        Ok(DXGI_SWAP_CHAIN_DESC {
            BufferDesc: DXGI_MODE_DESC {
                Width: d.Width,
                Height: d.Height,
                RefreshRate: DXGI_RATIONAL {
                    Numerator: 60,
                    Denominator: 1,
                },
                Format: d.Format,
                ScanlineOrdering: DXGI_MODE_SCANLINE_ORDER_UNSPECIFIED,
                Scaling: DXGI_MODE_SCALING_UNSPECIFIED,
            },
            SampleDesc: d.SampleDesc,
            BufferUsage: d.BufferUsage,
            BufferCount: d.BufferCount,
            OutputWindow: self.hwnd,
            Windowed: BOOL::from(true),
            SwapEffect: d.SwapEffect,
            Flags: d.Flags,
        })
    }

    pub(super) fn x_GetDesc1(&self) -> WResult<DXGI_SWAP_CHAIN_DESC1> {
        Ok(self.desc1())
    }

    pub(super) fn x_GetFullscreenDesc(&self) -> WResult<DXGI_SWAP_CHAIN_FULLSCREEN_DESC> {
        Ok(DXGI_SWAP_CHAIN_FULLSCREEN_DESC {
            RefreshRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            ScanlineOrdering: DXGI_MODE_SCANLINE_ORDER_UNSPECIFIED,
            Scaling: DXGI_MODE_SCALING_UNSPECIFIED,
            Windowed: BOOL::from(true),
        })
    }

    pub(super) fn x_GetHwnd(&self) -> WResult<HWND> {
        Ok(self.hwnd)
    }

    /// Windowed only: asking for exclusive fullscreen is accepted and ignored
    /// (the game's window stays what the window system makes of it).
    pub(super) fn x_SetFullscreenState(
        &self,
        _fullscreen: BOOL,
        _ptarget: windows::core::Ref<IDXGIOutput>,
    ) -> WResult<()> {
        Ok(())
    }

    pub(super) fn x_GetFullscreenState(
        &self,
        pfullscreen: *mut BOOL,
        pptarget: windows::core::OutRef<IDXGIOutput>,
    ) -> WResult<()> {
        if !pfullscreen.is_null() {
            // SAFETY: the caller's out pointer.
            unsafe { *pfullscreen = BOOL::from(false) };
        }
        if !pptarget.is_null() {
            pptarget.write(None)?;
        }
        Ok(())
    }

    pub(super) fn x_ResizeTarget(
        &self,
        _pnewtargetparameters: *const DXGI_MODE_DESC,
    ) -> WResult<()> {
        Ok(())
    }

    pub(super) fn x_ResizeBuffers(
        &self,
        buffercount: u32,
        width: u32,
        height: u32,
        newformat: DXGI_FORMAT,
        swapchainflags: &DXGI_SWAP_CHAIN_FLAG,
    ) -> WResult<()> {
        let (mut w, mut h) = (width, height);
        if (w == 0 || h == 0)
            && let Some((cw, ch)) = client_size(self.hwnd)
        {
            if w == 0 {
                w = cw;
            }
            if h == 0 {
                h = ch;
            }
        }
        // The application released its references to the old buffers; ours go too.
        self.buffers.lock().unwrap().clear();
        wres(self.chain.resize(
            w,
            h,
            newformat.0 as u32,
            buffercount,
            swapchainflags.0 as u32,
        ))
    }

    pub(super) fn x_GetLastPresentCount(&self) -> WResult<u32> {
        Ok(self.chain.present_count())
    }

    pub(super) fn x_SetMaximumFrameLatency(&self, maxlatency: u32) -> WResult<()> {
        wres(self.chain.set_max_latency(maxlatency))
    }

    pub(super) fn x_GetMaximumFrameLatency(&self) -> WResult<u32> {
        Ok(self.chain.max_latency())
    }

    /// A handle of its own to the signalled event (the caller closes it).
    pub(super) fn x_GetFrameLatencyWaitableObject(&self) -> HANDLE {
        let Some(h) = self.event else {
            return HANDLE::default();
        };
        let mut dup = HANDLE::default();
        // SAFETY: duplicating our own event into this process.
        let r = unsafe {
            let me = GetCurrentProcess();
            DuplicateHandle(
                me,
                HANDLE(h as *mut c_void),
                me,
                &mut dup,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if r.is_ok() { dup } else { HANDLE::default() }
    }
}
