//! `IDXGIFactory7` and `IDXGIAdapter4`: one adapter, the Metal GPU.

use std::ffi::c_void;
use std::sync::Arc;

use windows::Win32::Foundation::{HWND, LUID};
use windows::Win32::Graphics::Dxgi::*;
use windows::core::{BOOL, ComObject, GUID, IUnknownImpl, Interface, Result as WResult, implement};

use super::objects::QueueObj;
use super::private::PrivateData;
use super::swapchain::SwapChainObj;
use super::{OrWerr, return_object, werr, wres};
use crate::device::System;
use crate::hresult::Hr;
use crate::swapchain::SwapChainDesc;

#[implement(IDXGIFactory7)]
pub struct FactoryObj {
    pub private: PrivateData,
    pub sys: Arc<System>,
}

#[implement(IDXGIAdapter4)]
pub struct AdapterObj {
    pub private: PrivateData,
    pub sys: Arc<System>,
    pub factory: ComObject<FactoryObj>,
}

impl FactoryObj {
    pub fn create() -> WResult<ComObject<FactoryObj>> {
        let sys = wres(super::system())?;
        Ok(ComObject::new(FactoryObj {
            private: PrivateData::default(),
            sys,
        }))
    }
}

impl FactoryObj_Impl {
    fn adapter(&self) -> ComObject<AdapterObj> {
        ComObject::new(AdapterObj {
            private: PrivateData::default(),
            sys: self.sys.clone(),
            factory: self.to_object(),
        })
    }

    pub(super) fn x_EnumAdapters(&self, adapter: u32) -> WResult<IDXGIAdapter> {
        if adapter != 0 {
            return Err(werr(Hr::DXGI_ERROR_NOT_FOUND));
        }
        self.adapter().to_interface::<IDXGIAdapter4>().cast()
    }

    pub(super) fn x_EnumAdapters1(&self, adapter: u32) -> WResult<IDXGIAdapter1> {
        if adapter != 0 {
            return Err(werr(Hr::DXGI_ERROR_NOT_FOUND));
        }
        self.adapter().to_interface::<IDXGIAdapter4>().cast()
    }

    /// The queue a swap chain presents on: the `pDevice` argument of
    /// `CreateSwapChain*` is the direct `ID3D12CommandQueue` for D3D12.
    fn swap_queue(
        &self,
        pdevice: &windows::core::Ref<windows::core::IUnknown>,
    ) -> WResult<ComObject<QueueObj>> {
        let unk = pdevice.as_ref().or_werr(Hr::E_INVALIDARG)?;
        unk.cast_object::<QueueObj>()
            .map_err(|_| werr(Hr::DXGI_ERROR_INVALID_CALL))
    }

    pub(super) fn x_CreateSwapChainForHwnd(
        &self,
        pdevice: windows::core::Ref<windows::core::IUnknown>,
        hwnd: HWND,
        pdesc: *const DXGI_SWAP_CHAIN_DESC1,
        _pfullscreendesc: *const DXGI_SWAP_CHAIN_FULLSCREEN_DESC,
        _prestricttooutput: windows::core::Ref<IDXGIOutput>,
    ) -> WResult<IDXGISwapChain1> {
        // SAFETY: the caller's description.
        let d = unsafe { pdesc.as_ref() }.or_werr(Hr::E_POINTER)?;
        let queue = self.swap_queue(&pdevice)?;
        let desc = SwapChainDesc {
            width: d.Width,
            height: d.Height,
            format: d.Format.0 as u32,
            buffer_count: d.BufferCount,
            buffer_usage: d.BufferUsage.0,
            sample_count: d.SampleDesc.Count,
            swap_effect: d.SwapEffect.0 as u32,
            flags: d.Flags,
        };
        if d.Stereo.as_bool() {
            return Err(werr(Hr::DXGI_ERROR_UNSUPPORTED));
        }
        let sc = SwapChainObj::create(self.to_object(), queue, hwnd, desc, d.Scaling, d.AlphaMode)?;
        sc.to_interface::<IDXGISwapChain4>().cast()
    }

    pub(super) fn x_CreateSwapChain(
        &self,
        pdevice: windows::core::Ref<windows::core::IUnknown>,
        pdesc: *const DXGI_SWAP_CHAIN_DESC,
        ppswapchain: windows::core::OutRef<IDXGISwapChain>,
    ) -> windows::core::HRESULT {
        let make = || -> WResult<IDXGISwapChain> {
            // SAFETY: the caller's description.
            let d = unsafe { pdesc.as_ref() }.or_werr(Hr::E_POINTER)?;
            let queue = self.swap_queue(&pdevice)?;
            let desc = SwapChainDesc {
                width: d.BufferDesc.Width,
                height: d.BufferDesc.Height,
                format: d.BufferDesc.Format.0 as u32,
                buffer_count: d.BufferCount,
                buffer_usage: d.BufferUsage.0,
                sample_count: d.SampleDesc.Count,
                swap_effect: d.SwapEffect.0 as u32,
                flags: d.Flags,
            };
            let sc = SwapChainObj::create(
                self.to_object(),
                queue,
                d.OutputWindow,
                desc,
                DXGI_SCALING_STRETCH,
                windows::Win32::Graphics::Dxgi::Common::DXGI_ALPHA_MODE_IGNORE,
            )?;
            sc.to_interface::<IDXGISwapChain4>().cast()
        };
        match make() {
            Ok(sc) => match ppswapchain.write(Some(sc)) {
                Ok(()) => windows::core::HRESULT(0),
                Err(e) => e.into(),
            },
            Err(e) => e.into(),
        }
    }

    pub(super) fn x_IsCurrent(&self) -> BOOL {
        BOOL::from(true)
    }

    pub(super) fn x_MakeWindowAssociation(
        &self,
        _windowhandle: HWND,
        _flags: DXGI_MWA_FLAGS,
    ) -> WResult<()> {
        Ok(())
    }

    pub(super) fn x_GetWindowAssociation(&self) -> WResult<HWND> {
        Ok(HWND::default())
    }

    pub(super) fn x_EnumAdapterByLuid(
        &self,
        adapterluid: &LUID,
        riid: *const GUID,
        ppvadapter: *mut *mut c_void,
    ) -> WResult<()> {
        let a = &self.sys.adapter;
        if adapterluid.LowPart != a.luid_low || adapterluid.HighPart != a.luid_high {
            return Err(werr(Hr::DXGI_ERROR_NOT_FOUND));
        }
        return_object(&self.adapter().to_interface(), riid, ppvadapter)
    }

    /// No software adapter: callers fall back or skip (D3DMetal answers the same).
    pub(super) fn x_EnumWarpAdapter(
        &self,
        _riid: *const GUID,
        _ppvadapter: *mut *mut c_void,
    ) -> WResult<()> {
        Err(werr(Hr::DXGI_ERROR_UNSUPPORTED))
    }

    pub(super) fn x_EnumAdapterByGpuPreference(
        &self,
        adapter: u32,
        _gpupreference: DXGI_GPU_PREFERENCE,
        riid: *const GUID,
        ppvadapter: *mut *mut c_void,
    ) -> WResult<()> {
        if adapter != 0 {
            return Err(werr(Hr::DXGI_ERROR_NOT_FOUND));
        }
        return_object(&self.adapter().to_interface(), riid, ppvadapter)
    }
}

impl AdapterObj_Impl {
    pub(super) fn x_GetParent(&self, riid: *const GUID, ppparent: *mut *mut c_void) -> WResult<()> {
        if ppparent.is_null() {
            return Err(werr(Hr::E_POINTER));
        }
        // SAFETY: `ppparent` is the caller's out pointer.
        unsafe {
            self.factory
                .to_interface::<IDXGIFactory7>()
                .query(riid, ppparent)
        }
        .ok()
    }

    pub(super) fn x_EnumOutputs(&self, _output: u32) -> WResult<IDXGIOutput> {
        // Display enumeration is M3; "no outputs" is a valid answer.
        Err(werr(Hr::DXGI_ERROR_NOT_FOUND))
    }

    fn desc_name(&self) -> [u16; 128] {
        self.sys.adapter.description_utf16()
    }

    pub(super) fn x_GetDesc(&self) -> WResult<DXGI_ADAPTER_DESC> {
        let a = &self.sys.adapter;
        Ok(DXGI_ADAPTER_DESC {
            Description: self.desc_name(),
            VendorId: a.vendor_id,
            DeviceId: a.device_id,
            SubSysId: a.sub_sys_id,
            Revision: a.revision,
            DedicatedVideoMemory: a.dedicated_video_memory as usize,
            DedicatedSystemMemory: a.dedicated_system_memory as usize,
            SharedSystemMemory: a.shared_system_memory as usize,
            AdapterLuid: LUID {
                LowPart: a.luid_low,
                HighPart: a.luid_high,
            },
        })
    }

    pub(super) fn x_GetDesc1(&self) -> WResult<DXGI_ADAPTER_DESC1> {
        let a = &self.sys.adapter;
        Ok(DXGI_ADAPTER_DESC1 {
            Description: self.desc_name(),
            VendorId: a.vendor_id,
            DeviceId: a.device_id,
            SubSysId: a.sub_sys_id,
            Revision: a.revision,
            DedicatedVideoMemory: a.dedicated_video_memory as usize,
            DedicatedSystemMemory: a.dedicated_system_memory as usize,
            SharedSystemMemory: a.shared_system_memory as usize,
            AdapterLuid: LUID {
                LowPart: a.luid_low,
                HighPart: a.luid_high,
            },
            Flags: if a.software {
                DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32
            } else {
                0
            },
        })
    }

    pub(super) fn x_GetDesc3(&self) -> WResult<DXGI_ADAPTER_DESC3> {
        let a = &self.sys.adapter;
        Ok(DXGI_ADAPTER_DESC3 {
            Description: self.desc_name(),
            VendorId: a.vendor_id,
            DeviceId: a.device_id,
            SubSysId: a.sub_sys_id,
            Revision: a.revision,
            DedicatedVideoMemory: a.dedicated_video_memory as usize,
            DedicatedSystemMemory: a.dedicated_system_memory as usize,
            SharedSystemMemory: a.shared_system_memory as usize,
            AdapterLuid: LUID {
                LowPart: a.luid_low,
                HighPart: a.luid_high,
            },
            Flags: DXGI_ADAPTER_FLAG3_NONE,
            GraphicsPreemptionGranularity: DXGI_GRAPHICS_PREEMPTION_DMA_BUFFER_BOUNDARY,
            ComputePreemptionGranularity: DXGI_COMPUTE_PREEMPTION_DMA_BUFFER_BOUNDARY,
        })
    }

    /// Unified memory: one local segment; the non-local one is empty.
    pub(super) fn x_QueryVideoMemoryInfo(
        &self,
        nodeindex: u32,
        memorysegmentgroup: DXGI_MEMORY_SEGMENT_GROUP,
        pvideomemoryinfo: *mut DXGI_QUERY_VIDEO_MEMORY_INFO,
    ) -> WResult<()> {
        if nodeindex != 0 || pvideomemoryinfo.is_null() {
            return Err(werr(Hr::E_INVALIDARG));
        }
        let local = match memorysegmentgroup {
            DXGI_MEMORY_SEGMENT_GROUP_LOCAL => true,
            DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL => false,
            _ => return Err(werr(Hr::E_INVALIDARG)),
        };
        let ws = if local {
            self.sys.limits.working_set
        } else {
            0
        };
        // SAFETY: non-null out pointer from the caller.
        unsafe {
            *pvideomemoryinfo = DXGI_QUERY_VIDEO_MEMORY_INFO {
                Budget: ws,
                CurrentUsage: 0,
                AvailableForReservation: ws / 2,
                CurrentReservation: 0,
            }
        };
        Ok(())
    }

    pub(super) fn x_SetVideoMemoryReservation(
        &self,
        nodeindex: u32,
        _memorysegmentgroup: DXGI_MEMORY_SEGMENT_GROUP,
        _reservation: u64,
    ) -> WResult<()> {
        if nodeindex != 0 {
            Err(werr(Hr::E_INVALIDARG))
        } else {
            Ok(())
        }
    }
}
