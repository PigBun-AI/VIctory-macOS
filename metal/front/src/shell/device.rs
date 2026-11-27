//! `ID3D12Device14`: decode arguments, call the core, wrap the result.

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{
    ComObject, GUID, IUnknown, IUnknownImpl, Interface, Ref, Result as WResult, implement,
};

use super::list::ListObj;
use super::objects::*;
use super::private::PrivateData;
use super::{return_object, werr, wres};
use crate::caps;
use crate::convert::SamplerParams;
use crate::descriptor::HeapDesc;
use crate::device as dv;
use crate::footprint;
use crate::hresult::Hr;
use crate::resource::{self, ResourceDesc};

#[implement(ID3D12Device14)]
pub struct DeviceObj {
    pub private: PrivateData,
    pub dev: Arc<dv::Device>,
}

impl DeviceObj {
    pub fn create() -> WResult<ComObject<DeviceObj>> {
        let sys = wres(super::system())?;
        Ok(ComObject::new(DeviceObj {
            private: PrivateData::default(),
            dev: dv::Device::new(sys),
        }))
    }
}

pub(crate) fn core_desc(d: &D3D12_RESOURCE_DESC) -> ResourceDesc {
    ResourceDesc {
        dimension: d.Dimension.0 as u32,
        alignment: d.Alignment,
        width: d.Width,
        height: d.Height,
        depth_or_array: d.DepthOrArraySize,
        mips: d.MipLevels,
        format: d.Format.0 as u32,
        sample_count: d.SampleDesc.Count,
        sample_quality: d.SampleDesc.Quality,
        layout: d.Layout.0 as u32,
        flags: d.Flags.0 as u32,
    }
}

pub(crate) fn d3d_desc(d: &ResourceDesc) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION(d.dimension as i32),
        Alignment: d.alignment,
        Width: d.width,
        Height: d.height,
        DepthOrArraySize: d.depth_or_array,
        MipLevels: d.mips,
        Format: DXGI_FORMAT(d.format as i32),
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: d.sample_count,
            Quality: d.sample_quality,
        },
        Layout: D3D12_TEXTURE_LAYOUT(d.layout as i32),
        Flags: D3D12_RESOURCE_FLAGS(d.flags as i32),
    }
}

fn require<'a, T>(p: *const T) -> WResult<&'a T> {
    // SAFETY: a pointer argument of a D3D12 call; null is refused, validity is the caller's contract.
    unsafe { p.as_ref() }.ok_or_else(|| werr(Hr::E_INVALIDARG))
}

impl DeviceObj_Impl {
    pub(super) fn x_GetNodeCount(&self) -> u32 {
        1
    }

    pub(super) fn x_GetAdapterLuid(&self) -> LUID {
        let a = &self.dev.sys.adapter;
        LUID {
            LowPart: a.luid_low,
            HighPart: a.luid_high,
        }
    }

    pub(super) fn x_GetDeviceRemovedReason(&self) -> WResult<()> {
        match self.dev.removed_reason() {
            Hr::S_OK => Ok(()),
            h => Err(werr(h)),
        }
    }

    pub(super) fn x_CreateCommandQueue(
        &self,
        pdesc: *const D3D12_COMMAND_QUEUE_DESC,
        riid: *const GUID,
        ppcommandqueue: *mut *mut c_void,
    ) -> WResult<()> {
        let d = require(pdesc)?;
        let kind = match d.Type {
            D3D12_COMMAND_LIST_TYPE_DIRECT => 0,
            D3D12_COMMAND_LIST_TYPE_COMPUTE => 1,
            D3D12_COMMAND_LIST_TYPE_COPY => 2,
            _ => return Err(werr(Hr::E_INVALIDARG)),
        };
        let queue = wres(self.dev.create_queue(kind, d.Priority as u32))?;
        let q = ComObject::new(QueueObj {
            private: PrivateData::default(),
            device_obj: self.to_object(),
            queue,
            desc: *d,
        });
        return_object(&q.to_interface::<IUnknown>(), riid, ppcommandqueue)
    }

    pub(super) fn x_CreateCommandAllocator(
        &self,
        r#type: D3D12_COMMAND_LIST_TYPE,
        riid: *const GUID,
        ppcommandallocator: *mut *mut c_void,
    ) -> WResult<()> {
        if !matches!(
            r#type,
            D3D12_COMMAND_LIST_TYPE_DIRECT
                | D3D12_COMMAND_LIST_TYPE_BUNDLE
                | D3D12_COMMAND_LIST_TYPE_COMPUTE
                | D3D12_COMMAND_LIST_TYPE_COPY
        ) {
            return Err(werr(Hr::E_INVALIDARG));
        }
        let a = ComObject::new(AllocatorObj {
            private: PrivateData::default(),
            device_obj: self.to_object(),
            ty: r#type,
        });
        return_object(&a.to_interface::<IUnknown>(), riid, ppcommandallocator)
    }

    pub(super) fn x_CreateCommandList(
        &self,
        _nodemask: u32,
        r#type: D3D12_COMMAND_LIST_TYPE,
        pcommandallocator: Ref<ID3D12CommandAllocator>,
        pinitialstate: Ref<ID3D12PipelineState>,
        riid: *const GUID,
        ppcommandlist: *mut *mut c_void,
    ) -> WResult<()> {
        let kind = match r#type {
            D3D12_COMMAND_LIST_TYPE_DIRECT => 0,
            D3D12_COMMAND_LIST_TYPE_COMPUTE => 1,
            D3D12_COMMAND_LIST_TYPE_COPY => 2,
            _ => return Err(werr(Hr::E_INVALIDARG)),
        };
        let alloc = pcommandallocator
            .as_ref()
            .ok_or_else(|| werr(Hr::E_INVALIDARG))?;
        if alloc.cast_object_ref::<AllocatorObj>()?.ty != r#type {
            return Err(werr(Hr::E_INVALIDARG));
        }
        let mut list = dv::CommandList::new(&self.dev, kind);
        if let Some(p) = pinitialstate.as_ref() {
            list.set_pipeline(&p.cast_object_ref::<PsoObj>()?.pipeline);
        }
        let l = ComObject::new(ListObj {
            private: PrivateData::default(),
            device_obj: self.to_object(),
            ty: r#type,
            list: Mutex::new(list),
        });
        return_object(&l.to_interface::<IUnknown>(), riid, ppcommandlist)
    }

    pub(super) fn x_CreateComputePipelineState(
        &self,
        pdesc: *const D3D12_COMPUTE_PIPELINE_STATE_DESC,
        riid: *const GUID,
        pppipelinestate: *mut *mut c_void,
    ) -> WResult<()> {
        let d = require(pdesc)?;
        let rs = d
            .pRootSignature
            .as_ref()
            .ok_or_else(|| werr(Hr::E_INVALIDARG))?;
        let rs = rs.cast_object_ref::<RootSigObj>()?;
        if d.CS.pShaderBytecode.is_null() || d.CS.BytecodeLength == 0 {
            return Err(werr(Hr::E_INVALIDARG));
        }
        // SAFETY: the application's shader bytecode: `BytecodeLength` readable bytes.
        let dxil = unsafe {
            std::slice::from_raw_parts(d.CS.pShaderBytecode.cast::<u8>(), d.CS.BytecodeLength)
        };
        let pipeline = wres(self.dev.create_compute_pipeline(&rs.root_sig, dxil))?;
        let p = ComObject::new(PsoObj {
            private: PrivateData::default(),
            device_obj: self.to_object(),
            pipeline,
        });
        return_object(&p.to_interface::<IUnknown>(), riid, pppipelinestate)
    }

    pub(super) fn x_CheckFeatureSupport(
        &self,
        feature: D3D12_FEATURE,
        data: *mut c_void,
        size: u32,
    ) -> WResult<()> {
        if data.is_null() {
            return Err(werr(Hr::E_INVALIDARG));
        }
        let c = &self.dev.sys.caps;
        if feature.0 as u32 == caps::F_FEATURE_LEVELS {
            if size as usize != std::mem::size_of::<D3D12_FEATURE_DATA_FEATURE_LEVELS>() {
                return Err(werr(Hr::E_INVALIDARG));
            }
            // SAFETY: size checked; the caller's structure.
            let fl = unsafe { &mut *data.cast::<D3D12_FEATURE_DATA_FEATURE_LEVELS>() };
            if fl.NumFeatureLevels == 0 || fl.pFeatureLevelsRequested.is_null() {
                return Err(werr(Hr::E_INVALIDARG));
            }
            // SAFETY: `NumFeatureLevels` entries at the given pointer.
            let req = unsafe {
                std::slice::from_raw_parts(fl.pFeatureLevelsRequested, fl.NumFeatureLevels as usize)
            };
            let req: Vec<u32> = req.iter().map(|l| l.0 as u32).collect();
            fl.MaxSupportedFeatureLevel =
                D3D_FEATURE_LEVEL(wres(caps::max_feature_level(c, &req))? as i32);
            return Ok(());
        }
        // SAFETY: `size` bytes at `data`, the caller's structure.
        let bytes = unsafe { std::slice::from_raw_parts_mut(data.cast::<u8>(), size as usize) };
        wres(caps::check_feature_support(c, feature.0 as u32, bytes))
    }

    pub(super) fn x_CreateDescriptorHeap(
        &self,
        pdescriptorheapdesc: *const D3D12_DESCRIPTOR_HEAP_DESC,
        riid: *const GUID,
        ppvheap: *mut *mut c_void,
    ) -> WResult<()> {
        let d = require(pdescriptorheapdesc)?;
        let hd = HeapDesc {
            ty: d.Type.0 as u32,
            count: d.NumDescriptors,
            flags: d.Flags.0 as u32,
            node_mask: d.NodeMask,
        };
        let heap = wres(self.dev.create_descriptor_heap(&hd))?;
        let h = ComObject::new(HeapObj {
            private: PrivateData::default(),
            device_obj: self.to_object(),
            heap,
        });
        return_object(&h.to_interface::<IUnknown>(), riid, ppvheap)
    }

    pub(super) fn x_GetDescriptorHandleIncrementSize(
        &self,
        descriptorheaptype: D3D12_DESCRIPTOR_HEAP_TYPE,
    ) -> u32 {
        match descriptorheaptype.0 {
            0..=3 => self.dev.descriptor_increment(descriptorheaptype.0 as u32),
            _ => 0,
        }
    }

    pub(super) fn x_CreateRootSignature(
        &self,
        _nodemask: u32,
        pblob: *const c_void,
        bloblength: usize,
        riid: *const GUID,
        ppvrootsignature: *mut *mut c_void,
    ) -> WResult<()> {
        if pblob.is_null() || bloblength == 0 {
            return Err(werr(Hr::E_INVALIDARG));
        }
        // SAFETY: `bloblength` readable bytes at `pblob`.
        let blob = unsafe { std::slice::from_raw_parts(pblob.cast::<u8>(), bloblength) };
        let root_sig = wres(self.dev.create_root_signature(blob))?;
        let r = ComObject::new(RootSigObj {
            private: PrivateData::default(),
            device_obj: self.to_object(),
            root_sig,
        });
        return_object(&r.to_interface::<IUnknown>(), riid, ppvrootsignature)
    }

    pub(super) fn x_CreateConstantBufferView(
        &self,
        pdesc: *const D3D12_CONSTANT_BUFFER_VIEW_DESC,
        dest: &D3D12_CPU_DESCRIPTOR_HANDLE,
    ) {
        // SAFETY: optional pointer argument.
        let (va, size) = match unsafe { pdesc.as_ref() } {
            Some(d) => (d.BufferLocation, d.SizeInBytes),
            None => (0, 0),
        };
        let _ = self.dev.create_cbv(dest.ptr as u64, va, size);
    }

    pub(super) fn x_CreateShaderResourceView(
        &self,
        presource: Ref<ID3D12Resource>,
        pdesc: *const D3D12_SHADER_RESOURCE_VIEW_DESC,
        dest: &D3D12_CPU_DESCRIPTOR_HANDLE,
    ) {
        let res = presource
            .as_ref()
            .and_then(|r| r.cast_object_ref::<ResourceObj>().ok())
            .map(|r| r.buffer.res);
        // SAFETY: optional pointer argument.
        let Some(d) = (unsafe { pdesc.as_ref() }) else {
            return;
        };
        if d.ViewDimension != D3D12_SRV_DIMENSION_BUFFER {
            // textures: M3
            super::note_notimpl(&format!(
                "ID3D12Device::CreateShaderResourceView(view dimension {})",
                d.ViewDimension.0
            ));
            return;
        }
        // SAFETY: `ViewDimension` says which union member is live.
        let b = unsafe { d.Anonymous.Buffer };
        let r = self.dev.create_buffer_view(
            dest.ptr as u64,
            &dv::BufferView {
                uav: false,
                resource: res,
                first_element: b.FirstElement,
                num_elements: b.NumElements,
                structure_stride: b.StructureByteStride,
                format: d.Format.0 as u32,
                raw: b.Flags.0 & D3D12_BUFFER_SRV_FLAG_RAW.0 != 0,
                counter: None,
            },
        );
        view_result("CreateShaderResourceView", d.Format.0, r);
    }

    pub(super) fn x_CreateUnorderedAccessView(
        &self,
        presource: Ref<ID3D12Resource>,
        pcounterresource: Ref<ID3D12Resource>,
        pdesc: *const D3D12_UNORDERED_ACCESS_VIEW_DESC,
        dest: &D3D12_CPU_DESCRIPTOR_HANDLE,
    ) {
        let res = presource
            .as_ref()
            .and_then(|r| r.cast_object_ref::<ResourceObj>().ok())
            .map(|r| r.buffer.res);
        let counter = pcounterresource
            .as_ref()
            .and_then(|r| r.cast_object_ref::<ResourceObj>().ok())
            .map(|r| r.buffer.res);
        // SAFETY: optional pointer argument.
        let Some(d) = (unsafe { pdesc.as_ref() }) else {
            return;
        };
        if d.ViewDimension != D3D12_UAV_DIMENSION_BUFFER {
            super::note_notimpl(&format!(
                "ID3D12Device::CreateUnorderedAccessView(view dimension {})",
                d.ViewDimension.0
            ));
            return;
        }
        // SAFETY: `ViewDimension` says which union member is live.
        let b = unsafe { d.Anonymous.Buffer };
        let r = self.dev.create_buffer_view(
            dest.ptr as u64,
            &dv::BufferView {
                uav: true,
                resource: res,
                first_element: b.FirstElement,
                num_elements: b.NumElements,
                structure_stride: b.StructureByteStride,
                format: d.Format.0 as u32,
                raw: b.Flags.0 & D3D12_BUFFER_UAV_FLAG_RAW.0 != 0,
                counter: counter.map(|c| (c, b.CounterOffsetInBytes)),
            },
        );
        view_result("CreateUnorderedAccessView", d.Format.0, r);
    }

    pub(super) fn x_CreateRenderTargetView(
        &self,
        _presource: Ref<ID3D12Resource>,
        _pdesc: *const D3D12_RENDER_TARGET_VIEW_DESC,
        _dest: &D3D12_CPU_DESCRIPTOR_HANDLE,
    ) {
        // Render targets are M3; the heap exists, nothing to write yet.
    }

    pub(super) fn x_CreateDepthStencilView(
        &self,
        _presource: Ref<ID3D12Resource>,
        _pdesc: *const D3D12_DEPTH_STENCIL_VIEW_DESC,
        _dest: &D3D12_CPU_DESCRIPTOR_HANDLE,
    ) {
    }

    pub(super) fn x_CreateSampler(
        &self,
        pdesc: *const D3D12_SAMPLER_DESC,
        dest: &D3D12_CPU_DESCRIPTOR_HANDLE,
    ) {
        let Ok(d) = require(pdesc) else { return };
        let _ = self.dev.create_sampler(
            dest.ptr as u64,
            &SamplerParams {
                filter: d.Filter.0 as u32,
                address_u: d.AddressU.0 as u32,
                address_v: d.AddressV.0 as u32,
                address_w: d.AddressW.0 as u32,
                mip_lod_bias: d.MipLODBias,
                max_anisotropy: d.MaxAnisotropy,
                comparison: d.ComparisonFunc.0 as u32,
                border_color: d.BorderColor,
                min_lod: d.MinLOD,
                max_lod: d.MaxLOD,
            },
        );
    }

    pub(super) fn x_CopyDescriptorsSimple(
        &self,
        numdescriptors: u32,
        dst: &D3D12_CPU_DESCRIPTOR_HANDLE,
        src: &D3D12_CPU_DESCRIPTOR_HANDLE,
        _heaptype: D3D12_DESCRIPTOR_HEAP_TYPE,
    ) {
        let _ = self
            .dev
            .copy_descriptors(dst.ptr as u64, src.ptr as u64, numdescriptors);
    }

    pub(super) fn x_CopyDescriptors(
        &self,
        ndst: u32,
        dst_starts: *const D3D12_CPU_DESCRIPTOR_HANDLE,
        dst_sizes: *const u32,
        nsrc: u32,
        src_starts: *const D3D12_CPU_DESCRIPTOR_HANDLE,
        src_sizes: *const u32,
        heaptype: D3D12_DESCRIPTOR_HEAP_TYPE,
    ) {
        let inc = self.x_GetDescriptorHandleIncrementSize(heaptype) as u64;
        // SAFETY: the caller's arrays of `n` ranges; null sizes mean "all ones".
        let ranges = |starts: *const D3D12_CPU_DESCRIPTOR_HANDLE,
                      sizes: *const u32,
                      n: u32|
         -> Vec<(u64, u32)> {
            (0..n as usize)
                .map(|i| unsafe {
                    (
                        (*starts.add(i)).ptr as u64,
                        if sizes.is_null() { 1 } else { *sizes.add(i) },
                    )
                })
                .collect()
        };
        if inc == 0 || dst_starts.is_null() || src_starts.is_null() {
            return;
        }
        let (d, s) = (
            ranges(dst_starts, dst_sizes, ndst),
            ranges(src_starts, src_sizes, nsrc),
        );
        let (mut di, mut si) = (0usize, 0usize);
        let (mut doff, mut soff) = (0u32, 0u32);
        while di < d.len() && si < s.len() {
            let n = (d[di].1 - doff).min(s[si].1 - soff);
            if n > 0 {
                let _ = self.dev.copy_descriptors(
                    d[di].0 + doff as u64 * inc,
                    s[si].0 + soff as u64 * inc,
                    n,
                );
            }
            doff += n;
            soff += n;
            if doff == d[di].1 {
                di += 1;
                doff = 0;
            }
            if soff == s[si].1 {
                si += 1;
                soff = 0;
            }
        }
    }

    pub(super) fn x_GetResourceAllocationInfo(
        &self,
        _visiblemask: u32,
        numresourcedescs: u32,
        presourcedescs: *const D3D12_RESOURCE_DESC,
    ) -> D3D12_RESOURCE_ALLOCATION_INFO {
        let mut size = 0u64;
        for i in 0..numresourcedescs as usize {
            // SAFETY: the caller's array of descriptions.
            let d = core_desc(unsafe { &*presourcedescs.add(i) });
            if !d.is_buffer() {
                return D3D12_RESOURCE_ALLOCATION_INFO {
                    SizeInBytes: u64::MAX,
                    Alignment: 0,
                }; // textures: M3
            }
            size += d.width.div_ceil(resource::DEFAULT_ALIGNMENT) * resource::DEFAULT_ALIGNMENT;
        }
        D3D12_RESOURCE_ALLOCATION_INFO {
            SizeInBytes: size,
            Alignment: resource::DEFAULT_ALIGNMENT,
        }
    }

    pub(super) fn x_CreateCommittedResource(
        &self,
        pheapproperties: *const D3D12_HEAP_PROPERTIES,
        _heapflags: D3D12_HEAP_FLAGS,
        pdesc: *const D3D12_RESOURCE_DESC,
        initialresourcestate: D3D12_RESOURCE_STATES,
        _poptimizedclearvalue: *const D3D12_CLEAR_VALUE,
        riidresource: *const GUID,
        ppvresource: *mut *mut c_void,
    ) -> WResult<()> {
        let hp = require(pheapproperties)?;
        let d = require(pdesc)?;
        let buffer = match self.dev.create_committed_buffer(
            hp.Type.0 as u32,
            &core_desc(d),
            initialresourcestate.0 as u32,
        ) {
            Ok(b) => b,
            Err(h) => {
                if h == Hr::E_NOTIMPL {
                    // Textures and custom heaps: not built yet (see `note_notimpl`).
                    super::note_notimpl(&format!(
                        "ID3D12Device::CreateCommittedResource(dimension {}, heap type {})",
                        d.Dimension.0, hp.Type.0
                    ));
                }
                return Err(windows::core::Error::from(windows::core::HRESULT(h.0)));
            }
        };
        let r = ComObject::new(ResourceObj {
            private: PrivateData::default(),
            device_obj: self.to_object(),
            heap_props: *hp,
            buffer,
        });
        return_object(&r.to_interface::<IUnknown>(), riidresource, ppvresource)
    }

    pub(super) fn x_CreateFence(
        &self,
        initialvalue: u64,
        _flags: D3D12_FENCE_FLAGS,
        riid: *const GUID,
        ppfence: *mut *mut c_void,
    ) -> WResult<()> {
        let fence = Arc::new(wres(self.dev.create_fence(initialvalue))?);
        let f = ComObject::new(FenceObj {
            private: PrivateData::default(),
            device_obj: self.to_object(),
            fence,
        });
        return_object(&f.to_interface::<IUnknown>(), riid, ppfence)
    }

    pub(super) fn x_MakeResident(
        &self,
        _n: u32,
        _objects: *const Option<ID3D12Pageable>,
    ) -> WResult<()> {
        Ok(())
    }

    pub(super) fn x_Evict(&self, _n: u32, _objects: *const Option<ID3D12Pageable>) -> WResult<()> {
        Ok(())
    }

    pub(super) fn x_SetStablePowerState(&self, _enable: windows::core::BOOL) -> WResult<()> {
        Ok(())
    }

    pub(super) fn x_GetCopyableFootprints(
        &self,
        presourcedesc: *const D3D12_RESOURCE_DESC,
        firstsubresource: u32,
        numsubresources: u32,
        baseoffset: u64,
        playouts: *mut D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
        pnumrows: *mut u32,
        prowsizeinbytes: *mut u64,
        ptotalbytes: *mut u64,
    ) {
        let n = numsubresources as usize;
        let Ok(d) = require(presourcedesc) else {
            return;
        };
        let result = footprint::copyable_footprints(
            &core_desc(d),
            firstsubresource,
            numsubresources,
            baseoffset,
        );
        // SAFETY: each out pointer is optional and, when given, has room for `n` entries.
        unsafe {
            match result {
                Ok((layouts, total)) => {
                    for (i, l) in layouts.iter().enumerate() {
                        if !playouts.is_null() {
                            *playouts.add(i) = D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                                Offset: l.footprint.offset,
                                Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                                    Format: DXGI_FORMAT(l.footprint.format as i32),
                                    Width: l.footprint.width,
                                    Height: l.footprint.height,
                                    Depth: l.footprint.depth,
                                    RowPitch: l.footprint.row_pitch,
                                },
                            };
                        }
                        if !pnumrows.is_null() {
                            *pnumrows.add(i) = l.num_rows;
                        }
                        if !prowsizeinbytes.is_null() {
                            *prowsizeinbytes.add(i) = l.row_size;
                        }
                    }
                    if !ptotalbytes.is_null() {
                        *ptotalbytes = total;
                    }
                }
                Err(_) => {
                    // D3D12 fills every output with all-ones on bad input.
                    for i in 0..n {
                        if !playouts.is_null() {
                            std::ptr::write_bytes(playouts.add(i), 0xff, 1);
                        }
                        if !pnumrows.is_null() {
                            *pnumrows.add(i) = u32::MAX;
                        }
                        if !prowsizeinbytes.is_null() {
                            *prowsizeinbytes.add(i) = u64::MAX;
                        }
                    }
                    if !ptotalbytes.is_null() {
                        *ptotalbytes = u64::MAX;
                    }
                }
            }
        }
    }
}

/// The view methods return nothing: say (with `VICTORY_METAL_LOG=1`, and in
/// the not-implemented statistics) when the view was not made.
fn view_result(what: &str, format: i32, r: crate::hresult::Result<()>) {
    if let Err(h) = r {
        if h == Hr::E_NOTIMPL {
            super::note_notimpl(&format!(
                "ID3D12Device::{what}(buffer view, format {format})"
            ));
        }
        if super::log_errors() {
            eprintln!("victory-metal front: {what} failed: {:#010x}", h.0 as u32);
        }
    }
}
