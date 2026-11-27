//! `ID3D12GraphicsCommandList6`, compute subset: each call lands in the
//! core's `CommandList`, which keeps proto commands until `ExecuteCommandLists`.
//! Calls outside M1 are remembered and fail the list at `Close`.

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use victory_metal_proto::ResId;
use victory_metal_proto::types::RootParamKind;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{ComObject, Interface, Ref, Result as WResult, implement};

use super::device::DeviceObj;
use super::objects::*;
use super::private::PrivateData;
use super::{werr, wres};
use crate::device::{CommandList, CopyLoc, Item, ListState, ResourceBarrier};
use crate::hresult::Hr;

#[implement(ID3D12GraphicsCommandList6)]
pub struct ListObj {
    pub private: PrivateData,
    pub device_obj: ComObject<DeviceObj>,
    pub ty: D3D12_COMMAND_LIST_TYPE,
    pub list: Mutex<CommandList>,
}

fn location_resource(l: &D3D12_TEXTURE_COPY_LOCATION) -> Option<ComObject<ResourceObj>> {
    l.pResource.as_ref()?.cast_object().ok()
}

fn copy_loc<'a>(l: &D3D12_TEXTURE_COPY_LOCATION, res: &'a crate::device::Resource) -> CopyLoc<'a> {
    // SAFETY: `Type` says which union member is live.
    unsafe {
        if l.Type == D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT {
            let p = l.Anonymous.PlacedFootprint;
            CopyLoc::Footprint {
                res,
                offset: p.Offset,
                format: p.Footprint.Format.0 as u32,
                width: p.Footprint.Width,
                height: p.Footprint.Height,
                depth: p.Footprint.Depth,
                row_pitch: p.Footprint.RowPitch,
            }
        } else {
            CopyLoc::Subresource {
                res,
                index: l.Anonymous.SubresourceIndex,
            }
        }
    }
}

fn resource<'a>(r: &'a Ref<ID3D12Resource>) -> Option<&'a ResourceObj_Impl> {
    r.as_ref()?.cast_object_ref::<ResourceObj>().ok()
}

impl ListObj_Impl {
    pub fn unsupported_call(&self) {
        self.list.lock().unwrap().unsupported();
    }

    #[allow(clippy::too_many_arguments)]
    fn clear_uav(
        &self,
        gpu: &D3D12_GPU_DESCRIPTOR_HANDLE,
        cpu: &D3D12_CPU_DESCRIPTOR_HANDLE,
        res: &Ref<ID3D12Resource>,
        values: [u32; 4],
        float_values: bool,
        numrects: u32,
        prects: *const RECT,
    ) {
        let mut l = self.list.lock().unwrap();
        let Some(r) = resource(res) else {
            return l.unsupported();
        };
        // Buffer UAV clears are not built (the proto has a placeholder only).
        let Some(view) = self.device_obj.dev.texture_uav(cpu.ptr as u64, gpu.ptr) else {
            return l.unsupported();
        };
        let rects: Vec<[i32; 4]> = (0..if prects.is_null() {
            0
        } else {
            numrects as usize
        })
            // SAFETY: the caller's array of `numrects` rectangles.
            .map(|i| unsafe { *prects.add(i) })
            .map(|r| [r.left, r.top, r.right, r.bottom])
            .collect();
        l.clear_uav_texture(&r.resource, &view, values, float_values, &rects);
    }

    /// The recorded commands of a closed, healthy list.
    pub fn closed_items(&self) -> Option<Arc<[Item]>> {
        let mut l = self.list.lock().unwrap();
        if l.state != ListState::Closed {
            return None;
        }
        l.close().ok()
    }

    pub(super) fn x_GetType(&self) -> D3D12_COMMAND_LIST_TYPE {
        self.ty
    }

    /// A second `Close` is fine (D3DMetal agrees).
    pub(super) fn x_Close(&self) -> WResult<()> {
        wres(self.list.lock().unwrap().close().map(|_| ()))
    }

    pub(super) fn x_Reset(
        &self,
        pallocator: Ref<ID3D12CommandAllocator>,
        pinitialstate: Ref<ID3D12PipelineState>,
    ) -> WResult<()> {
        if pallocator.as_ref().is_none() {
            return Err(werr(Hr::E_INVALIDARG));
        }
        let mut l = self.list.lock().unwrap();
        l.reset();
        if let Some(p) = pinitialstate.as_ref() {
            l.set_pipeline(&p.cast_object_ref::<PsoObj>()?.pipeline);
        }
        Ok(())
    }

    pub(super) fn x_ClearState(&self, pinitialstate: Ref<ID3D12PipelineState>) {
        if let Some(Ok(p)) = pinitialstate
            .as_ref()
            .map(|p| p.cast_object_ref::<PsoObj>())
        {
            self.list.lock().unwrap().set_pipeline(&p.pipeline);
        }
    }

    pub(super) fn x_Dispatch(&self, x: u32, y: u32, z: u32) {
        self.list.lock().unwrap().dispatch(x, y, z);
    }

    pub(super) fn x_CopyBufferRegion(
        &self,
        dst: Ref<ID3D12Resource>,
        dstoffset: u64,
        src: Ref<ID3D12Resource>,
        srcoffset: u64,
        numbytes: u64,
    ) {
        let mut l = self.list.lock().unwrap();
        match (resource(&dst), resource(&src)) {
            (Some(d), Some(s)) if d.resource.desc.is_buffer() && s.resource.desc.is_buffer() => {
                l.copy_buffer_region(&d.resource, dstoffset, &s.resource, srcoffset, numbytes)
            }
            _ => l.unsupported(),
        }
    }

    pub(super) fn x_CopyResource(&self, dst: Ref<ID3D12Resource>, src: Ref<ID3D12Resource>) {
        let mut l = self.list.lock().unwrap();
        match (resource(&dst), resource(&src)) {
            (Some(d), Some(s)) => l.copy_resource(&d.resource, &s.resource),
            _ => l.unsupported(),
        }
    }

    pub(super) fn x_CopyTextureRegion(
        &self,
        pdst: *const D3D12_TEXTURE_COPY_LOCATION,
        dstx: u32,
        dsty: u32,
        dstz: u32,
        psrc: *const D3D12_TEXTURE_COPY_LOCATION,
        psrcbox: *const D3D12_BOX,
    ) {
        let mut l = self.list.lock().unwrap();
        // SAFETY: the caller's pointers; null is a failed list, as in D3D12's validation.
        let (Some(dst), Some(src)) = (unsafe { pdst.as_ref() }, unsafe { psrc.as_ref() }) else {
            return l.unsupported();
        };
        let (Some(dr), Some(sr)) = (location_resource(dst), location_resource(src)) else {
            return l.unsupported();
        };
        let src_box = unsafe { psrcbox.as_ref() }
            .map(|b| [b.left, b.top, b.front, b.right, b.bottom, b.back]);
        let (dl, sl) = (copy_loc(dst, &dr.resource), copy_loc(src, &sr.resource));
        l.copy_texture_region(&dl, [dstx, dsty, dstz], &sl, src_box);
    }

    pub(super) fn x_ClearUnorderedAccessViewUint(
        &self,
        viewgpuhandleincurrentheap: &D3D12_GPU_DESCRIPTOR_HANDLE,
        viewcpuhandle: &D3D12_CPU_DESCRIPTOR_HANDLE,
        presource: Ref<ID3D12Resource>,
        values: *const u32,
        numrects: u32,
        prects: *const RECT,
    ) {
        if values.is_null() {
            return self.list.lock().unwrap().unsupported();
        }
        // SAFETY: four values, the D3D12 contract.
        let v = unsafe { [*values, *values.add(1), *values.add(2), *values.add(3)] };
        self.clear_uav(
            viewgpuhandleincurrentheap,
            viewcpuhandle,
            &presource,
            v,
            false,
            numrects,
            prects,
        );
    }

    pub(super) fn x_ClearUnorderedAccessViewFloat(
        &self,
        viewgpuhandleincurrentheap: &D3D12_GPU_DESCRIPTOR_HANDLE,
        viewcpuhandle: &D3D12_CPU_DESCRIPTOR_HANDLE,
        presource: Ref<ID3D12Resource>,
        values: *const f32,
        numrects: u32,
        prects: *const RECT,
    ) {
        if values.is_null() {
            return self.list.lock().unwrap().unsupported();
        }
        // SAFETY: four values, the D3D12 contract.
        let v =
            unsafe { [*values, *values.add(1), *values.add(2), *values.add(3)].map(f32::to_bits) };
        self.clear_uav(
            viewgpuhandleincurrentheap,
            viewcpuhandle,
            &presource,
            v,
            true,
            numrects,
            prects,
        );
    }

    pub(super) fn x_ResourceBarrier(
        &self,
        numbarriers: u32,
        pbarriers: *const D3D12_RESOURCE_BARRIER,
    ) {
        let mut l = self.list.lock().unwrap();
        if pbarriers.is_null() {
            return;
        }
        let id = |r: &Option<ID3D12Resource>| -> Option<ResId> {
            Some(
                r.as_ref()?
                    .cast_object_ref::<ResourceObj>()
                    .ok()?
                    .resource
                    .res,
            )
        };
        let mut out = Vec::with_capacity(numbarriers as usize);
        for i in 0..numbarriers as usize {
            // SAFETY: the caller's array; the union member read is the one `Type` names.
            let b = unsafe { &*pbarriers.add(i) };
            match b.Type {
                D3D12_RESOURCE_BARRIER_TYPE_TRANSITION => {
                    let t = unsafe { &*b.Anonymous.Transition };
                    let Some(res) = id(&t.pResource) else {
                        return l.unsupported();
                    };
                    out.push(ResourceBarrier::Transition {
                        res,
                        subresource: t.Subresource,
                        before: t.StateBefore.0 as u32,
                        after: t.StateAfter.0 as u32,
                        flags: b.Flags.0 as u32,
                    });
                }
                D3D12_RESOURCE_BARRIER_TYPE_UAV => {
                    let u = unsafe { &*b.Anonymous.UAV };
                    out.push(ResourceBarrier::Uav {
                        res: id(&u.pResource),
                    });
                }
                D3D12_RESOURCE_BARRIER_TYPE_ALIASING => {
                    let a = unsafe { &*b.Anonymous.Aliasing };
                    out.push(ResourceBarrier::Aliasing {
                        before: id(&a.pResourceBefore),
                        after: id(&a.pResourceAfter),
                    });
                }
                _ => return l.unsupported(),
            }
        }
        l.resource_barrier(&out);
    }

    pub(super) fn x_SetPipelineState(&self, p: Ref<ID3D12PipelineState>) {
        let mut l = self.list.lock().unwrap();
        match p.as_ref().map(|p| p.cast_object_ref::<PsoObj>()) {
            Some(Ok(p)) => l.set_pipeline(&p.pipeline),
            _ => l.unsupported(),
        }
    }

    pub(super) fn x_SetComputeRootSignature(&self, rs: Ref<ID3D12RootSignature>) {
        let mut l = self.list.lock().unwrap();
        match rs.as_ref().map(|r| r.cast_object_ref::<RootSigObj>()) {
            Some(Ok(r)) => l.set_compute_root_signature(&r.root_sig),
            _ => l.unsupported(),
        }
    }

    pub(super) fn x_SetDescriptorHeaps(
        &self,
        numdescriptorheaps: u32,
        ppdescriptorheaps: *const Option<ID3D12DescriptorHeap>,
    ) {
        let mut l = self.list.lock().unwrap();
        let mut heaps = Vec::new();
        for i in 0..numdescriptorheaps as usize {
            // SAFETY: the caller's array of interface pointers.
            let Some(h) = (unsafe { ppdescriptorheaps.add(i).as_ref() }).and_then(|h| h.as_ref())
            else {
                return l.unsupported();
            };
            let Ok(h) = h.cast_object_ref::<HeapObj>() else {
                return l.unsupported();
            };
            heaps.push(&h.heap);
        }
        l.set_descriptor_heaps(&heaps);
    }

    pub(super) fn x_SetComputeRootDescriptorTable(
        &self,
        idx: u32,
        base: &D3D12_GPU_DESCRIPTOR_HANDLE,
    ) {
        self.list
            .lock()
            .unwrap()
            .set_compute_root_table(idx, base.ptr);
    }

    pub(super) fn x_SetComputeRoot32BitConstant(&self, idx: u32, srcdata: u32, destoffset: u32) {
        self.list
            .lock()
            .unwrap()
            .set_compute_root_constants(idx, destoffset, &[srcdata]);
    }

    pub(super) fn x_SetComputeRoot32BitConstants(
        &self,
        idx: u32,
        n: u32,
        psrcdata: *const c_void,
        destoffset: u32,
    ) {
        let mut l = self.list.lock().unwrap();
        if psrcdata.is_null() || n == 0 {
            return;
        }
        // SAFETY: `n` 32-bit values at `psrcdata` (not necessarily aligned).
        let values: Vec<u32> = (0..n as usize)
            .map(|i| unsafe { psrcdata.cast::<u32>().add(i).read_unaligned() })
            .collect();
        l.set_compute_root_constants(idx, destoffset, &values);
    }

    pub(super) fn x_SetComputeRootConstantBufferView(&self, idx: u32, va: u64) {
        self.list
            .lock()
            .unwrap()
            .set_compute_root_descriptor(idx, RootParamKind::CBV, va);
    }

    pub(super) fn x_SetComputeRootShaderResourceView(&self, idx: u32, va: u64) {
        self.list
            .lock()
            .unwrap()
            .set_compute_root_descriptor(idx, RootParamKind::SRV, va);
    }

    pub(super) fn x_SetComputeRootUnorderedAccessView(&self, idx: u32, va: u64) {
        self.list
            .lock()
            .unwrap()
            .set_compute_root_descriptor(idx, RootParamKind::UAV, va);
    }

    // Debug markers: accepted, not recorded (queries and labels are M2).
    pub(super) fn x_SetMarker(&self, _metadata: u32, _pdata: *const c_void, _size: u32) {}
    pub(super) fn x_BeginEvent(&self, _metadata: u32, _pdata: *const c_void, _size: u32) {}
    pub(super) fn x_EndEvent(&self) {}
}
