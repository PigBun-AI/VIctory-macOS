//! The small D3D12 objects: queue, allocator, fence, resource, descriptor
//! heap, root signature, pipeline state.

use std::ffi::c_void;
use std::sync::Arc;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::System::Threading::SetEvent;
use windows::core::{ComObject, Interface, Ref, Result as WResult, implement};

use super::device::{DeviceObj, d3d_desc};
use super::list::ListObj;
use super::private::PrivateData;
use super::{werr, wres};
use crate::device as dv;
use crate::hresult::Hr;

#[implement(ID3D12CommandQueue)]
pub struct QueueObj {
    pub private: PrivateData,
    pub device_obj: ComObject<DeviceObj>,
    pub queue: dv::Queue,
    pub desc: D3D12_COMMAND_QUEUE_DESC,
}

impl QueueObj_Impl {
    pub(super) fn x_ExecuteCommandLists(
        &self,
        numcommandlists: u32,
        ppcommandlists: *const Option<ID3D12CommandList>,
    ) {
        if ppcommandlists.is_null() {
            return;
        }
        let mut lists = Vec::with_capacity(numcommandlists as usize);
        for i in 0..numcommandlists as usize {
            // SAFETY: the caller's array of `numcommandlists` interface pointers.
            let Some(l) = (unsafe { &*ppcommandlists.add(i) }) else {
                return;
            };
            let Ok(l) = l.cast_object_ref::<ListObj>() else {
                return;
            };
            // A list that is not closed, or that failed while recording, is not executed.
            let Some(items) = l.closed_items() else {
                return;
            };
            lists.push(items);
        }
        let _ = self.queue.execute(&lists);
    }

    pub(super) fn x_Signal(&self, pfence: Ref<ID3D12Fence>, value: u64) -> WResult<()> {
        let f = pfence.as_ref().ok_or_else(|| werr(Hr::E_INVALIDARG))?;
        wres(
            self.queue
                .signal(&f.cast_object_ref::<FenceObj>()?.fence, value),
        )
    }

    pub(super) fn x_Wait(&self, pfence: Ref<ID3D12Fence>, value: u64) -> WResult<()> {
        let f = pfence.as_ref().ok_or_else(|| werr(Hr::E_INVALIDARG))?;
        wres(
            self.queue
                .wait(&f.cast_object_ref::<FenceObj>()?.fence, value),
        )
    }

    pub(super) fn x_GetDesc(&self) -> D3D12_COMMAND_QUEUE_DESC {
        self.desc
    }

    /// Timestamps are M2; ticks are nanoseconds.
    pub(super) fn x_GetTimestampFrequency(&self) -> WResult<u64> {
        Ok(1_000_000_000)
    }
}

#[implement(ID3D12CommandAllocator)]
pub struct AllocatorObj {
    pub private: PrivateData,
    pub device_obj: ComObject<DeviceObj>,
    pub ty: D3D12_COMMAND_LIST_TYPE,
}

impl AllocatorObj_Impl {
    /// The recorded lists keep their own commands, so there is nothing to recycle.
    pub(super) fn x_Reset(&self) -> WResult<()> {
        Ok(())
    }
}

#[implement(ID3D12Fence1)]
pub struct FenceObj {
    pub private: PrivateData,
    pub device_obj: ComObject<DeviceObj>,
    pub fence: Arc<dv::Fence>,
}

impl FenceObj_Impl {
    pub(super) fn x_GetCompletedValue(&self) -> u64 {
        self.fence.completed()
    }

    pub(super) fn x_Signal(&self, value: u64) -> WResult<()> {
        wres(self.fence.signal(value))
    }

    /// An event handle is signalled when the value is reached (at once if it
    /// already was); with no handle the call blocks until then.
    pub(super) fn x_SetEventOnCompletion(&self, value: u64, hevent: HANDLE) -> WResult<()> {
        if self.fence.completed() >= value {
            if !hevent.is_invalid() {
                // SAFETY: the caller's event handle.
                unsafe { SetEvent(hevent)? };
            }
            return Ok(());
        }
        if hevent.is_invalid() {
            wres(self.fence.wait(value, u64::MAX))?;
            return Ok(());
        }
        let fence = self.fence.clone();
        let handle = hevent.0 as isize;
        std::thread::spawn(move || {
            if fence.wait(value, u64::MAX).unwrap_or(false) {
                // SAFETY: the event handle stays the application's until it is signalled.
                let _ = unsafe { SetEvent(HANDLE(handle as *mut c_void)) };
            }
        });
        Ok(())
    }
}

#[implement(ID3D12Resource2)]
pub struct ResourceObj {
    pub private: PrivateData,
    pub device_obj: ComObject<DeviceObj>,
    pub heap_props: D3D12_HEAP_PROPERTIES,
    pub heap_flags: D3D12_HEAP_FLAGS,
    pub resource: dv::Resource,
}

impl ResourceObj_Impl {
    pub(super) fn x_Map(
        &self,
        subresource: u32,
        _preadrange: *const D3D12_RANGE,
        ppdata: *mut *mut c_void,
    ) -> WResult<()> {
        let addr = wres(self.resource.map(subresource))?;
        if !ppdata.is_null() {
            // SAFETY: the caller's out pointer.
            unsafe { *ppdata = addr as usize as *mut c_void };
        }
        Ok(())
    }

    pub(super) fn x_Unmap(&self, _subresource: u32, _pwrittenrange: *const D3D12_RANGE) {}

    pub(super) fn x_GetDesc(&self) -> D3D12_RESOURCE_DESC {
        d3d_desc(&self.resource.desc)
    }

    pub(super) fn x_GetDesc1(&self) -> D3D12_RESOURCE_DESC1 {
        let d = d3d_desc(&self.resource.desc);
        D3D12_RESOURCE_DESC1 {
            Dimension: d.Dimension,
            Alignment: d.Alignment,
            Width: d.Width,
            Height: d.Height,
            DepthOrArraySize: d.DepthOrArraySize,
            MipLevels: d.MipLevels,
            Format: d.Format,
            SampleDesc: d.SampleDesc,
            Layout: d.Layout,
            Flags: d.Flags,
            SamplerFeedbackMipRegion: D3D12_MIP_REGION::default(),
        }
    }

    /// Textures have no GPU virtual address.
    pub(super) fn x_GetGPUVirtualAddress(&self) -> u64 {
        self.resource.gpu_va
    }

    pub(super) fn x_GetHeapProperties(
        &self,
        pheapproperties: *mut D3D12_HEAP_PROPERTIES,
        pheapflags: *mut D3D12_HEAP_FLAGS,
    ) -> WResult<()> {
        // SAFETY: optional out pointers.
        unsafe {
            if !pheapproperties.is_null() {
                *pheapproperties = self.heap_props;
            }
            if !pheapflags.is_null() {
                *pheapflags = self.heap_flags;
            }
        }
        Ok(())
    }
}

/// `ID3D12Heap`: memory resources are placed in.
#[implement(ID3D12Heap)]
pub struct MemHeapObj {
    pub private: PrivateData,
    pub device_obj: ComObject<DeviceObj>,
    pub heap: Arc<dv::Heap>,
    /// The properties as the application gave them.
    pub props: D3D12_HEAP_PROPERTIES,
    pub flags: D3D12_HEAP_FLAGS,
}

impl MemHeapObj_Impl {
    pub(super) fn x_GetDesc(&self) -> D3D12_HEAP_DESC {
        D3D12_HEAP_DESC {
            SizeInBytes: self.heap.size,
            Properties: self.props,
            Alignment: self.heap.alignment,
            Flags: self.flags,
        }
    }
}

#[implement(ID3D12DescriptorHeap)]
pub struct HeapObj {
    pub private: PrivateData,
    pub device_obj: ComObject<DeviceObj>,
    pub heap: dv::DescriptorHeap,
}

impl HeapObj_Impl {
    pub(super) fn x_GetDesc(&self) -> D3D12_DESCRIPTOR_HEAP_DESC {
        let d = &self.heap.desc;
        D3D12_DESCRIPTOR_HEAP_DESC {
            Type: D3D12_DESCRIPTOR_HEAP_TYPE(d.ty as i32),
            NumDescriptors: d.count,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAGS(d.flags as i32),
            NodeMask: d.node_mask,
        }
    }

    pub(super) fn x_GetCPUDescriptorHandleForHeapStart(&self) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: self.heap.cpu_start() as usize,
        }
    }

    pub(super) fn x_GetGPUDescriptorHandleForHeapStart(&self) -> D3D12_GPU_DESCRIPTOR_HANDLE {
        D3D12_GPU_DESCRIPTOR_HANDLE {
            ptr: self.heap.gpu_start(),
        }
    }
}

#[implement(ID3D12RootSignature)]
pub struct RootSigObj {
    pub private: PrivateData,
    pub device_obj: ComObject<DeviceObj>,
    pub root_sig: dv::RootSignature,
}

#[implement(ID3D12PipelineState)]
pub struct PsoObj {
    pub private: PrivateData,
    pub device_obj: ComObject<DeviceObj>,
    pub pipeline: dv::Pipeline,
    /// What the pipeline was made from, for a pipeline library
    /// ([`crate::library::Fingerprint`]).
    pub fingerprint: u64,
}

/// `ID3D12CommandSignature`: the checked signature and the root signature it
/// was made with (kept alive: the back knows it by id).
#[implement(ID3D12CommandSignature)]
pub struct CmdSigObj {
    pub private: PrivateData,
    pub device_obj: ComObject<DeviceObj>,
    pub signature: crate::indirect::CommandSignature,
    pub _root_sig: Option<ComObject<RootSigObj>>,
}

impl CmdSigObj_Impl {}
impl PsoObj_Impl {}
impl RootSigObj_Impl {}
