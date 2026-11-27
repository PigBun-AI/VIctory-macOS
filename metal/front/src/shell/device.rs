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
use super::{OrWerr, return_object, werr, wres};
use crate::caps;
use crate::convert::SamplerParams;
use crate::descriptor::HeapDesc;
use crate::device as dv;
use crate::footprint;
use crate::hresult::Hr;
use crate::pso;
use crate::resource::{self, HeapProps, ResourceDesc};
use crate::view::{self, ViewParams};

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

/// `D3D12_RESOURCE_DESC1` (the same description and the sampler-feedback
/// mip region, which is not used).
pub(crate) fn core_desc1(d: &D3D12_RESOURCE_DESC1) -> ResourceDesc {
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

#[track_caller]
fn require<'a, T>(p: *const T) -> WResult<&'a T> {
    // SAFETY: a pointer argument of a D3D12 call; null is refused, validity is the caller's contract.
    unsafe { p.as_ref() }.or_werr(Hr::E_INVALIDARG)
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
        let alloc = pcommandallocator.as_ref().or_werr(Hr::E_INVALIDARG)?;
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
        let rs = d.pRootSignature.as_ref().or_werr(Hr::E_INVALIDARG)?;
        let rs = rs.cast_object_ref::<RootSigObj>()?;
        if d.CS.pShaderBytecode.is_null() || d.CS.BytecodeLength == 0 {
            return Err(werr(Hr::E_INVALIDARG));
        }
        // SAFETY: the application's shader bytecode: `BytecodeLength` readable bytes.
        let dxil = unsafe {
            std::slice::from_raw_parts(d.CS.pShaderBytecode.cast::<u8>(), d.CS.BytecodeLength)
        };
        self.make_compute_pso(rs, dxil, riid, pppipelinestate)
    }

    fn make_compute_pso(
        &self,
        rs: &RootSigObj,
        dxil: &[u8],
        riid: *const GUID,
        pppipelinestate: *mut *mut c_void,
    ) -> WResult<()> {
        let pipeline = wres(self.dev.create_compute_pipeline(&rs.root_sig, dxil))?;
        let p = ComObject::new(PsoObj {
            private: PrivateData::default(),
            device_obj: self.to_object(),
            pipeline,
            fingerprint: super::library::compute_fingerprint(&rs.root_sig, dxil),
        });
        return_object(&p.to_interface::<IUnknown>(), riid, pppipelinestate)
    }

    pub(super) fn x_CreateGraphicsPipelineState(
        &self,
        pdesc: *const D3D12_GRAPHICS_PIPELINE_STATE_DESC,
        riid: *const GUID,
        pppipelinestate: *mut *mut c_void,
    ) -> WResult<()> {
        let d = require(pdesc)?;
        let rs = d.pRootSignature.as_ref().or_werr(Hr::E_INVALIDARG)?;
        let state = state_of_desc(d);
        self.make_graphics_pso(rs, &state, riid, pppipelinestate)
    }

    pub(super) fn x_CreatePipelineState(
        &self,
        pdesc: *const D3D12_PIPELINE_STATE_STREAM_DESC,
        riid: *const GUID,
        pppipelinestate: *mut *mut c_void,
    ) -> WResult<()> {
        let d = require(pdesc)?;
        if d.pPipelineStateSubobjectStream.is_null() || d.SizeInBytes == 0 {
            return Err(werr(Hr::E_INVALIDARG));
        }
        // SAFETY: the application's stream: `SizeInBytes` readable bytes.
        let bytes = unsafe {
            std::slice::from_raw_parts(d.pPipelineStateSubobjectStream.cast::<u8>(), d.SizeInBytes)
        };
        let state = pso::parse_stream(bytes).map_err(refuse)?;
        let raw = state.root_sig as *mut c_void;
        // SAFETY: the root signature interface pointer the application put in
        // the stream; borrowed for the call.
        let rs =
            unsafe { ID3D12RootSignature::from_raw_borrowed(&raw) }.or_werr(Hr::E_INVALIDARG)?;
        if state.cs.len != 0 {
            // A compute pipeline made through the stream (victory-gpu's helper
            // passes and the replay's card compare do): the stage alone.
            let others = [
                state.vs, state.ps, state.ds, state.hs, state.gs, state.amp, state.ms,
            ];
            if others.iter().any(|b| b.len != 0) {
                return Err(werr(Hr::E_INVALIDARG));
            }
            let rs = rs.cast_object_ref::<RootSigObj>()?;
            // SAFETY: the application's shader bytecode: `len` readable bytes.
            let dxil = unsafe {
                std::slice::from_raw_parts(
                    state.cs.addr as usize as *const u8,
                    state.cs.len as usize,
                )
            };
            return self.make_compute_pso(rs, dxil, riid, pppipelinestate);
        }
        self.make_graphics_pso(rs, &state, riid, pppipelinestate)
    }

    fn make_graphics_pso(
        &self,
        rs: &ID3D12RootSignature,
        state: &pso::PipelineState,
        riid: *const GUID,
        pppipelinestate: *mut *mut c_void,
    ) -> WResult<()> {
        let rs = rs.cast_object_ref::<RootSigObj>()?;
        crate::callstats::pipeline_state(state);
        // SAFETY: the application's input layout (elements and names) is
        // valid for the call.
        let inputs = unsafe { pso::read_input_layout(state.input_layout) }.map_err(refuse)?;
        let desc = pso::to_proto(state, &inputs).map_err(refuse)?;
        let pipeline = wres(self.dev.create_graphics_pipeline(&rs.root_sig, &desc))?;
        let p = ComObject::new(PsoObj {
            private: PrivateData::default(),
            device_obj: self.to_object(),
            pipeline,
            fingerprint: super::library::graphics_fingerprint(&rs.root_sig, &desc),
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
        let heap = self.dev.create_descriptor_heap(&hd).map_err(|e| {
            if super::log_errors() {
                eprintln!(
                    "victory-metal front: CreateDescriptorHeap refused: type {} count {} flags {:#x} node mask {:#x}",
                    hd.ty, hd.count, hd.flags, hd.node_mask
                );
            }
            werr(e)
        })?;
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

    pub(super) fn x_CreatePipelineLibrary(
        &self,
        plibraryblob: *const c_void,
        bloblength: usize,
        riid: *const GUID,
        pppipelinelibrary: *mut *mut c_void,
    ) -> WResult<()> {
        super::library::LibraryObj::create(
            self.to_object(),
            plibraryblob,
            bloblength,
            riid,
            pppipelinelibrary,
        )
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

    pub(super) fn x_CreateCommandSignature(
        &self,
        pdesc: *const D3D12_COMMAND_SIGNATURE_DESC,
        prootsignature: Ref<ID3D12RootSignature>,
        riid: *const GUID,
        ppvcommandsignature: *mut *mut c_void,
    ) -> WResult<()> {
        let d = require(pdesc)?;
        if d.NumArgumentDescs == 0 || d.pArgumentDescs.is_null() {
            return Err(werr(Hr::E_INVALIDARG));
        }
        // SAFETY: `NumArgumentDescs` descriptions at `pArgumentDescs`.
        let raw =
            unsafe { std::slice::from_raw_parts(d.pArgumentDescs, d.NumArgumentDescs as usize) };
        let descs: Vec<crate::indirect::ArgDesc> = raw
            .iter()
            .map(|a| {
                // SAFETY: `Type` says which member of the union is live.
                let (index, dest_offset, count) = unsafe {
                    match a.Type {
                        D3D12_INDIRECT_ARGUMENT_TYPE_VERTEX_BUFFER_VIEW => {
                            (a.Anonymous.VertexBuffer.Slot, 0, 0)
                        }
                        D3D12_INDIRECT_ARGUMENT_TYPE_CONSTANT => {
                            let c = a.Anonymous.Constant;
                            (
                                c.RootParameterIndex,
                                c.DestOffsetIn32BitValues,
                                c.Num32BitValuesToSet,
                            )
                        }
                        D3D12_INDIRECT_ARGUMENT_TYPE_CONSTANT_BUFFER_VIEW => {
                            (a.Anonymous.ConstantBufferView.RootParameterIndex, 0, 0)
                        }
                        D3D12_INDIRECT_ARGUMENT_TYPE_SHADER_RESOURCE_VIEW => {
                            (a.Anonymous.ShaderResourceView.RootParameterIndex, 0, 0)
                        }
                        D3D12_INDIRECT_ARGUMENT_TYPE_UNORDERED_ACCESS_VIEW => {
                            (a.Anonymous.UnorderedAccessView.RootParameterIndex, 0, 0)
                        }
                        _ => (0, 0, 0),
                    }
                };
                crate::indirect::ArgDesc {
                    ty: a.Type.0 as u32,
                    index,
                    dest_offset,
                    count,
                }
            })
            .collect();
        let root = prootsignature
            .as_ref()
            .map(|r| r.cast_object::<RootSigObj>())
            .transpose()?;
        let signature = wres(crate::indirect::CommandSignature::new(
            d.ByteStride,
            &descs,
            root.as_ref().map(|r| &r.root_sig),
        ))?;
        let s = ComObject::new(CmdSigObj {
            private: PrivateData::default(),
            device_obj: self.to_object(),
            signature,
            _root_sig: root,
        });
        return_object(&s.to_interface::<IUnknown>(), riid, ppvcommandsignature)
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
        let robj = presource
            .as_ref()
            .and_then(|r| r.cast_object_ref::<ResourceObj>().ok());
        let res = robj.as_ref().map(|r| r.resource.res);
        // SAFETY: optional pointer argument.
        let d = unsafe { pdesc.as_ref() };
        let is_texture_view = match d {
            Some(d) => d.ViewDimension != D3D12_SRV_DIMENSION_BUFFER,
            None => robj.as_ref().is_some_and(|r| r.resource.is_texture()),
        };
        if is_texture_view {
            let r = self.texture_view(dest, robj, false, |rd, id| match d {
                Some(d) => view::srv(id, rd, &srv_params(d)?),
                None => Ok(view::default_view(rd, id, false)),
            });
            if r.is_err() && super::log_errors() {
                // The view that was refused, in the terms of the API (a view
                // that fails 115 times in a run is found from this line).
                let resource = robj.as_ref().map(|r| r.resource.desc);
                eprintln!(
                    "victory-metal front: CreateShaderResourceView refused: resource {resource:?}, view {:?}",
                    d.map(|d| srv_params(d).map_err(|h| h.0))
                );
            }
            return view_result("CreateShaderResourceView", d.map_or(0, |d| d.Format.0), r);
        }
        let Some(d) = d else { return };
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

    /// Writes a texture view (or a null one when the resource is null).
    fn texture_view(
        &self,
        dest: &D3D12_CPU_DESCRIPTOR_HANDLE,
        robj: Option<&ResourceObj_Impl>,
        uav: bool,
        make: impl FnOnce(
            &ResourceDesc,
            victory_metal_proto::ResId,
        ) -> crate::hresult::Result<crate::view::TextureView>,
    ) -> crate::hresult::Result<()> {
        let v = match robj {
            Some(r) => make(&r.resource.desc, r.resource.res)?,
            None => crate::view::TextureView::null(uav),
        };
        self.dev.create_texture_view(dest.ptr as u64, &v)
    }

    pub(super) fn x_CreateUnorderedAccessView(
        &self,
        presource: Ref<ID3D12Resource>,
        pcounterresource: Ref<ID3D12Resource>,
        pdesc: *const D3D12_UNORDERED_ACCESS_VIEW_DESC,
        dest: &D3D12_CPU_DESCRIPTOR_HANDLE,
    ) {
        let robj = presource
            .as_ref()
            .and_then(|r| r.cast_object_ref::<ResourceObj>().ok());
        let res = robj.as_ref().map(|r| r.resource.res);
        let counter = pcounterresource
            .as_ref()
            .and_then(|r| r.cast_object_ref::<ResourceObj>().ok())
            .map(|r| r.resource.res);
        // SAFETY: optional pointer argument.
        let d = unsafe { pdesc.as_ref() };
        let is_texture_view = match d {
            Some(d) => d.ViewDimension != D3D12_UAV_DIMENSION_BUFFER,
            None => robj.as_ref().is_some_and(|r| r.resource.is_texture()),
        };
        if is_texture_view {
            let r = self.texture_view(dest, robj, true, |rd, id| match d {
                Some(d) => view::uav(id, rd, &uav_params(d)?),
                None => Ok(view::default_view(rd, id, true)),
            });
            return view_result("CreateUnorderedAccessView", d.map_or(0, |d| d.Format.0), r);
        }
        let Some(d) = d else { return };
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

    /// Render targets and depth buffers are records of `(resource, mip,
    /// slices, format)`: the descriptor heap has no memory on the back.
    pub(super) fn x_CreateRenderTargetView(
        &self,
        presource: Ref<ID3D12Resource>,
        pdesc: *const D3D12_RENDER_TARGET_VIEW_DESC,
        dest: &D3D12_CPU_DESCRIPTOR_HANDLE,
    ) {
        let robj = presource
            .as_ref()
            .and_then(|r| r.cast_object_ref::<ResourceObj>().ok());
        // SAFETY: optional pointer argument.
        let d = unsafe { pdesc.as_ref() };
        let tv = robj.as_ref().map(|r| {
            let (rd, id) = (&r.resource.desc, r.resource.res);
            match d {
                Some(d) => rtv_params(d).and_then(|p| view::rtv(id, rd, &p)),
                None => view::default_target(id, rd, false),
            }
        });
        let r = match tv {
            Some(Ok(v)) => self.dev.create_target_view(dest.ptr as u64, Some(v)),
            Some(Err(e)) => Err(e),
            None => self.dev.create_target_view(dest.ptr as u64, None),
        };
        view_result("CreateRenderTargetView", d.map_or(0, |d| d.Format.0), r);
    }

    pub(super) fn x_CreateDepthStencilView(
        &self,
        presource: Ref<ID3D12Resource>,
        pdesc: *const D3D12_DEPTH_STENCIL_VIEW_DESC,
        dest: &D3D12_CPU_DESCRIPTOR_HANDLE,
    ) {
        let robj = presource
            .as_ref()
            .and_then(|r| r.cast_object_ref::<ResourceObj>().ok());
        // SAFETY: optional pointer argument.
        let d = unsafe { pdesc.as_ref() };
        let tv = robj.as_ref().map(|r| {
            let (rd, id) = (&r.resource.desc, r.resource.res);
            match d {
                Some(d) => dsv_params(d).and_then(|p| view::dsv(id, rd, &p)),
                None => view::default_target(id, rd, true),
            }
        });
        let r = match tv {
            Some(Ok(v)) => self.dev.create_target_view(dest.ptr as u64, Some(v)),
            Some(Err(e)) => Err(e),
            None => self.dev.create_target_view(dest.ptr as u64, None),
        };
        view_result("CreateDepthStencilView", d.map_or(0, |d| d.Format.0), r);
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
        let descs: Vec<ResourceDesc> = (0..numresourcedescs as usize)
            // SAFETY: the caller's array of descriptions.
            .map(|i| core_desc(unsafe { &*presourcedescs.add(i) }))
            .collect();
        match self.dev.allocation_info(&descs) {
            Some((size, alignment)) => D3D12_RESOURCE_ALLOCATION_INFO {
                SizeInBytes: size,
                Alignment: alignment,
            },
            None => D3D12_RESOURCE_ALLOCATION_INFO {
                SizeInBytes: u64::MAX,
                Alignment: 0,
            },
        }
    }

    /// Unified memory: `DEFAULT` is private (`NOT_AVAILABLE`), `UPLOAD` and
    /// `READBACK` are CPU-visible, and there is one pool (`L0`).
    pub(super) fn x_GetCustomHeapProperties(
        &self,
        nodemask: u32,
        heaptype: D3D12_HEAP_TYPE,
    ) -> D3D12_HEAP_PROPERTIES {
        let mask = if nodemask == 0 { 1 } else { nodemask };
        match resource::custom_heap_properties(heaptype.0 as u32, self.dev.sys.caps.unified_memory)
        {
            Ok(p) => D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_CUSTOM,
                CPUPageProperty: D3D12_CPU_PAGE_PROPERTY(p.cpu_page as i32),
                MemoryPoolPreference: D3D12_MEMORY_POOL(p.pool as i32),
                CreationNodeMask: mask,
                VisibleNodeMask: mask,
            },
            Err(_) => D3D12_HEAP_PROPERTIES::default(),
        }
    }

    pub(super) fn x_CreateHeap(
        &self,
        pdesc: *const D3D12_HEAP_DESC,
        riid: *const GUID,
        ppvheap: *mut *mut c_void,
    ) -> WResult<()> {
        let d = require(pdesc)?;
        let heap = match self.dev.create_heap(
            core_props(&d.Properties),
            d.SizeInBytes,
            d.Alignment,
            d.Flags.0 as u32,
        ) {
            Ok(h) => Arc::new(h),
            Err(h) => {
                return Err(creation_error(
                    "ID3D12Device::CreateHeap",
                    d.Properties.Type.0,
                    h,
                ));
            }
        };
        let h = ComObject::new(MemHeapObj {
            private: PrivateData::default(),
            device_obj: self.to_object(),
            heap,
            props: d.Properties,
            flags: d.Flags,
        });
        return_object(&h.to_interface::<IUnknown>(), riid, ppvheap)
    }

    pub(super) fn x_CreatePlacedResource(
        &self,
        pheap: Ref<ID3D12Heap>,
        heapoffset: u64,
        pdesc: *const D3D12_RESOURCE_DESC,
        initialstate: D3D12_RESOURCE_STATES,
        _poptimizedclearvalue: *const D3D12_CLEAR_VALUE,
        riid: *const GUID,
        ppvresource: *mut *mut c_void,
    ) -> WResult<()> {
        let d = require(pdesc)?;
        let heap = pheap.as_ref().or_werr(Hr::E_INVALIDARG)?;
        let heap = heap.cast_object_ref::<MemHeapObj>()?;
        let resource = match self.dev.create_placed(
            &heap.heap,
            heapoffset,
            &core_desc(d),
            initialstate.0 as u32,
        ) {
            Ok(r) => r,
            Err(h) => {
                return Err(creation_error(
                    "ID3D12Device::CreatePlacedResource",
                    d.Dimension.0,
                    h,
                ));
            }
        };
        let r = ComObject::new(ResourceObj {
            private: PrivateData::default(),
            device_obj: self.to_object(),
            heap_props: heap.props,
            heap_flags: heap.flags,
            resource,
        });
        return_object(&r.to_interface::<IUnknown>(), riid, ppvresource)
    }

    pub(super) fn x_CreateCommittedResource(
        &self,
        pheapproperties: *const D3D12_HEAP_PROPERTIES,
        heapflags: D3D12_HEAP_FLAGS,
        pdesc: *const D3D12_RESOURCE_DESC,
        initialresourcestate: D3D12_RESOURCE_STATES,
        _poptimizedclearvalue: *const D3D12_CLEAR_VALUE,
        riidresource: *const GUID,
        ppvresource: *mut *mut c_void,
    ) -> WResult<()> {
        let hp = require(pheapproperties)?;
        let d = require(pdesc)?;
        let resource = match self.dev.create_committed(
            core_props(hp),
            &core_desc(d),
            initialresourcestate.0 as u32,
        ) {
            Ok(b) => b,
            Err(h) => {
                return Err(creation_error(
                    "ID3D12Device::CreateCommittedResource",
                    d.Dimension.0,
                    h,
                ));
            }
        };
        let r = ComObject::new(ResourceObj {
            private: PrivateData::default(),
            device_obj: self.to_object(),
            heap_props: *hp,
            heap_flags: heapflags,
            resource,
        });
        return_object(&r.to_interface::<IUnknown>(), riidresource, ppvresource)
    }

    /// The state a resource of `desc` is created in when the call names a
    /// layout: buffers have none (their heap type implies the state), a
    /// texture's layout stands for its state.
    fn state_of_initial_layout(
        &self,
        what: &str,
        heap_type: u32,
        is_buffer: bool,
        layout: D3D12_BARRIER_LAYOUT,
    ) -> WResult<u32> {
        if is_buffer {
            return Ok(match heap_type {
                resource::HEAP_UPLOAD => resource::STATE_GENERIC_READ,
                resource::HEAP_READBACK => resource::STATE_COPY_DEST,
                _ => resource::STATE_COMMON,
            });
        }
        crate::state::state_of_layout(layout.0 as u32)
            .ok_or_else(|| creation_error(what, layout.0, Hr::E_INVALIDARG))
    }

    pub(super) fn x_CreateCommittedResource3(
        &self,
        pheapproperties: *const D3D12_HEAP_PROPERTIES,
        heapflags: D3D12_HEAP_FLAGS,
        pdesc: *const D3D12_RESOURCE_DESC1,
        initiallayout: D3D12_BARRIER_LAYOUT,
        _poptimizedclearvalue: *const D3D12_CLEAR_VALUE,
        _pprotectedsession: Ref<ID3D12ProtectedResourceSession>,
        numcastableformats: u32,
        pcastableformats: *const DXGI_FORMAT,
        riidresource: *const GUID,
        ppvresource: *mut *mut c_void,
    ) -> WResult<()> {
        const WHAT: &str = "ID3D12Device10::CreateCommittedResource3";
        let hp = require(pheapproperties)?;
        let d1 = require(pdesc)?;
        let d = core_desc1(d1);
        let state =
            self.state_of_initial_layout(WHAT, core_props(hp).ty, d.is_buffer(), initiallayout)?;
        castable_check(WHAT, &d, numcastableformats, pcastableformats)?;
        let resource =
            match self
                .dev
                .create_committed_cast(core_props(hp), &d, state, numcastableformats != 0)
            {
                Ok(b) => b,
                Err(h) => return Err(creation_error(WHAT, d1.Dimension.0, h)),
            };
        let r = ComObject::new(ResourceObj {
            private: PrivateData::default(),
            device_obj: self.to_object(),
            heap_props: *hp,
            heap_flags: heapflags,
            resource,
        });
        return_object(&r.to_interface::<IUnknown>(), riidresource, ppvresource)
    }

    pub(super) fn x_CreatePlacedResource2(
        &self,
        pheap: Ref<ID3D12Heap>,
        heapoffset: u64,
        pdesc: *const D3D12_RESOURCE_DESC1,
        initiallayout: D3D12_BARRIER_LAYOUT,
        _poptimizedclearvalue: *const D3D12_CLEAR_VALUE,
        numcastableformats: u32,
        pcastableformats: *const DXGI_FORMAT,
        riid: *const GUID,
        ppvresource: *mut *mut c_void,
    ) -> WResult<()> {
        const WHAT: &str = "ID3D12Device10::CreatePlacedResource2";
        let d1 = require(pdesc)?;
        let heap = pheap.as_ref().or_werr(Hr::E_INVALIDARG)?;
        let heap = heap.cast_object_ref::<MemHeapObj>()?;
        let d = core_desc1(d1);
        let state =
            self.state_of_initial_layout(WHAT, heap.heap.props.ty, d.is_buffer(), initiallayout)?;
        castable_check(WHAT, &d, numcastableformats, pcastableformats)?;
        let resource = match self.dev.create_placed_cast(
            &heap.heap,
            heapoffset,
            &d,
            state,
            numcastableformats != 0,
        ) {
            Ok(r) => r,
            Err(h) => return Err(creation_error(WHAT, d1.Dimension.0, h)),
        };
        let r = ComObject::new(ResourceObj {
            private: PrivateData::default(),
            device_obj: self.to_object(),
            heap_props: heap.props,
            heap_flags: heap.flags,
            resource,
        });
        return_object(&r.to_interface::<IUnknown>(), riid, ppvresource)
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
            waiter: crate::fence_wait::Waiter::new(fence.clone()),
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

/// A list of castable formats Metal cannot honour (`castable_formats_supported`)
/// is refused with `E_INVALIDARG`; the caller makes the texture without it.
fn castable_check(
    what: &str,
    d: &ResourceDesc,
    count: u32,
    list: *const DXGI_FORMAT,
) -> WResult<()> {
    if count == 0 || list.is_null() || d.is_buffer() {
        return Ok(());
    }
    // SAFETY: the API's pair of count and array.
    let formats: Vec<u32> = unsafe { std::slice::from_raw_parts(list, count as usize) }
        .iter()
        .map(|f| f.0 as u32)
        .collect();
    if crate::resource::castable_formats_supported(d.format, &formats) {
        return Ok(());
    }
    if super::log_errors() {
        // Thousands of textures ask the same: each distinct list is said once.
        static SEEN: std::sync::Mutex<Vec<(u32, Vec<u32>)>> = std::sync::Mutex::new(Vec::new());
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        let key = (d.format, formats.clone());
        if !seen.contains(&key) {
            seen.push(key);
            eprintln!(
                "victory-metal front: {what}: castable formats {formats:?} of format {} refused (Metal views a block format only as its own family; the caller makes the texture without the list), said once per list",
                d.format
            );
        }
    }
    Err(werr(Hr::E_INVALIDARG))
}

fn core_props(p: &D3D12_HEAP_PROPERTIES) -> HeapProps {
    HeapProps {
        ty: p.Type.0 as u32,
        cpu_page: p.CPUPageProperty.0 as u32,
        pool: p.MemoryPoolPreference.0 as u32,
    }
}

/// A creation call that failed: say so for what is not built yet (with
/// `VICTORY_METAL_NOTIMPL_LOG`), and hand the HRESULT back.
fn creation_error(what: &str, detail: i32, h: Hr) -> windows::core::Error {
    if h == Hr::E_NOTIMPL {
        super::note_notimpl(&format!("{what}({detail})"));
    }
    windows::core::Error::from(windows::core::HRESULT(h.0))
}

fn srv_params(d: &D3D12_SHADER_RESOURCE_VIEW_DESC) -> crate::hresult::Result<ViewParams> {
    let mut p = ViewParams {
        dimension: d.ViewDimension.0 as u32,
        format: d.Format.0 as u32,
        mapping: d.Shader4ComponentMapping,
        ..Default::default()
    };
    // SAFETY: `ViewDimension` says which union member is live.
    unsafe {
        match d.ViewDimension {
            D3D12_SRV_DIMENSION_TEXTURE1D => {
                let t = d.Anonymous.Texture1D;
                (p.mip_first, p.mip_count, p.min_lod) =
                    (t.MostDetailedMip, t.MipLevels, t.ResourceMinLODClamp);
            }
            D3D12_SRV_DIMENSION_TEXTURE1DARRAY => {
                let t = d.Anonymous.Texture1DArray;
                (p.mip_first, p.mip_count, p.min_lod) =
                    (t.MostDetailedMip, t.MipLevels, t.ResourceMinLODClamp);
                (p.first_slice, p.slice_count) = (t.FirstArraySlice, t.ArraySize);
            }
            D3D12_SRV_DIMENSION_TEXTURE2D => {
                let t = d.Anonymous.Texture2D;
                (p.mip_first, p.mip_count, p.min_lod) =
                    (t.MostDetailedMip, t.MipLevels, t.ResourceMinLODClamp);
                p.plane = t.PlaneSlice;
            }
            D3D12_SRV_DIMENSION_TEXTURE2DARRAY => {
                let t = d.Anonymous.Texture2DArray;
                (p.mip_first, p.mip_count, p.min_lod) =
                    (t.MostDetailedMip, t.MipLevels, t.ResourceMinLODClamp);
                (p.first_slice, p.slice_count, p.plane) =
                    (t.FirstArraySlice, t.ArraySize, t.PlaneSlice);
            }
            D3D12_SRV_DIMENSION_TEXTURE2DMS => {
                p.mip_count = 1;
            }
            D3D12_SRV_DIMENSION_TEXTURE2DMSARRAY => {
                let t = d.Anonymous.Texture2DMSArray;
                (p.first_slice, p.slice_count, p.mip_count) = (t.FirstArraySlice, t.ArraySize, 1);
            }
            D3D12_SRV_DIMENSION_TEXTURE3D => {
                let t = d.Anonymous.Texture3D;
                (p.mip_first, p.mip_count, p.min_lod) =
                    (t.MostDetailedMip, t.MipLevels, t.ResourceMinLODClamp);
            }
            D3D12_SRV_DIMENSION_TEXTURECUBE => {
                let t = d.Anonymous.TextureCube;
                (p.mip_first, p.mip_count, p.min_lod) =
                    (t.MostDetailedMip, t.MipLevels, t.ResourceMinLODClamp);
            }
            D3D12_SRV_DIMENSION_TEXTURECUBEARRAY => {
                let t = d.Anonymous.TextureCubeArray;
                (p.mip_first, p.mip_count, p.min_lod) =
                    (t.MostDetailedMip, t.MipLevels, t.ResourceMinLODClamp);
                (p.first_slice, p.slice_count) = (t.First2DArrayFace, t.NumCubes);
            }
            _ => return Err(Hr::E_INVALIDARG),
        }
    }
    Ok(p)
}

fn uav_params(d: &D3D12_UNORDERED_ACCESS_VIEW_DESC) -> crate::hresult::Result<ViewParams> {
    let mut p = ViewParams {
        dimension: d.ViewDimension.0 as u32,
        format: d.Format.0 as u32,
        ..Default::default()
    };
    // SAFETY: `ViewDimension` says which union member is live.
    unsafe {
        match d.ViewDimension {
            D3D12_UAV_DIMENSION_TEXTURE1D => p.mip_first = d.Anonymous.Texture1D.MipSlice,
            D3D12_UAV_DIMENSION_TEXTURE1DARRAY => {
                let t = d.Anonymous.Texture1DArray;
                (p.mip_first, p.first_slice, p.slice_count) =
                    (t.MipSlice, t.FirstArraySlice, t.ArraySize);
            }
            D3D12_UAV_DIMENSION_TEXTURE2D => {
                let t = d.Anonymous.Texture2D;
                (p.mip_first, p.plane) = (t.MipSlice, t.PlaneSlice);
            }
            D3D12_UAV_DIMENSION_TEXTURE2DARRAY => {
                let t = d.Anonymous.Texture2DArray;
                (p.mip_first, p.first_slice, p.slice_count, p.plane) =
                    (t.MipSlice, t.FirstArraySlice, t.ArraySize, t.PlaneSlice);
            }
            D3D12_UAV_DIMENSION_TEXTURE3D => {
                let t = d.Anonymous.Texture3D;
                (p.mip_first, p.first_slice, p.slice_count) = (t.MipSlice, t.FirstWSlice, t.WSize);
            }
            _ => return Err(Hr::E_INVALIDARG),
        }
    }
    Ok(p)
}

fn rtv_params(d: &D3D12_RENDER_TARGET_VIEW_DESC) -> crate::hresult::Result<ViewParams> {
    let mut p = ViewParams {
        dimension: d.ViewDimension.0 as u32,
        format: d.Format.0 as u32,
        ..Default::default()
    };
    // SAFETY: `ViewDimension` says which union member is live.
    unsafe {
        match d.ViewDimension {
            D3D12_RTV_DIMENSION_TEXTURE1D => p.mip_first = d.Anonymous.Texture1D.MipSlice,
            D3D12_RTV_DIMENSION_TEXTURE1DARRAY => {
                let t = d.Anonymous.Texture1DArray;
                (p.mip_first, p.first_slice, p.slice_count) =
                    (t.MipSlice, t.FirstArraySlice, t.ArraySize);
            }
            D3D12_RTV_DIMENSION_TEXTURE2D => {
                let t = d.Anonymous.Texture2D;
                (p.mip_first, p.plane) = (t.MipSlice, t.PlaneSlice);
            }
            D3D12_RTV_DIMENSION_TEXTURE2DARRAY => {
                let t = d.Anonymous.Texture2DArray;
                (p.mip_first, p.first_slice, p.slice_count, p.plane) =
                    (t.MipSlice, t.FirstArraySlice, t.ArraySize, t.PlaneSlice);
            }
            D3D12_RTV_DIMENSION_TEXTURE2DMS => {}
            D3D12_RTV_DIMENSION_TEXTURE2DMSARRAY => {
                let t = d.Anonymous.Texture2DMSArray;
                (p.first_slice, p.slice_count) = (t.FirstArraySlice, t.ArraySize);
            }
            D3D12_RTV_DIMENSION_TEXTURE3D => {
                let t = d.Anonymous.Texture3D;
                (p.mip_first, p.first_slice, p.slice_count) = (t.MipSlice, t.FirstWSlice, t.WSize);
            }
            _ => return Err(Hr::E_INVALIDARG),
        }
    }
    Ok(p)
}

fn dsv_params(d: &D3D12_DEPTH_STENCIL_VIEW_DESC) -> crate::hresult::Result<ViewParams> {
    let mut p = ViewParams {
        dimension: d.ViewDimension.0 as u32,
        format: d.Format.0 as u32,
        flags: d.Flags.0 as u32,
        ..Default::default()
    };
    // SAFETY: `ViewDimension` says which union member is live.
    unsafe {
        match d.ViewDimension {
            D3D12_DSV_DIMENSION_TEXTURE1D => p.mip_first = d.Anonymous.Texture1D.MipSlice,
            D3D12_DSV_DIMENSION_TEXTURE1DARRAY => {
                let t = d.Anonymous.Texture1DArray;
                (p.mip_first, p.first_slice, p.slice_count) =
                    (t.MipSlice, t.FirstArraySlice, t.ArraySize);
            }
            D3D12_DSV_DIMENSION_TEXTURE2D => p.mip_first = d.Anonymous.Texture2D.MipSlice,
            D3D12_DSV_DIMENSION_TEXTURE2DARRAY => {
                let t = d.Anonymous.Texture2DArray;
                (p.mip_first, p.first_slice, p.slice_count) =
                    (t.MipSlice, t.FirstArraySlice, t.ArraySize);
            }
            D3D12_DSV_DIMENSION_TEXTURE2DMS => {}
            D3D12_DSV_DIMENSION_TEXTURE2DMSARRAY => {
                let t = d.Anonymous.Texture2DMSArray;
                (p.first_slice, p.slice_count) = (t.FirstArraySlice, t.ArraySize);
            }
            _ => return Err(Hr::E_INVALIDARG),
        }
    }
    Ok(p)
}

/// A refused pipeline description: noted (when `VICTORY_METAL_NOTIMPL_LOG`
/// is set, for what the back lacks) and returned as its HRESULT.
pub(super) fn refuse(e: pso::PsoError) -> windows::core::Error {
    if e.hr == Hr::E_NOTIMPL {
        super::note_notimpl(&format!("pipeline state: {}", e.why));
    }
    if super::log_errors() {
        eprintln!("victory-metal front: pipeline state refused: {}", e.why);
    }
    werr(e.hr)
}

fn bytecode_of(b: &D3D12_SHADER_BYTECODE) -> pso::Bytecode {
    pso::Bytecode {
        addr: b.pShaderBytecode as usize as u64,
        len: b.BytecodeLength as u64,
    }
}

fn stencil_ops_of(f: &D3D12_DEPTH_STENCILOP_DESC) -> pso::StencilOps {
    pso::StencilOps {
        fail: f.StencilFailOp.0 as u32,
        depth_fail: f.StencilDepthFailOp.0 as u32,
        pass: f.StencilPassOp.0 as u32,
        func: f.StencilFunc.0 as u32,
    }
}

/// The fields of `D3D12_GRAPHICS_PIPELINE_STATE_DESC`, copied into the
/// core's description (nothing is read through a pointer yet).
#[allow(clippy::field_reassign_with_default)]
pub(super) fn state_of_desc(d: &D3D12_GRAPHICS_PIPELINE_STATE_DESC) -> pso::PipelineState {
    let mut s = pso::PipelineState::default();
    s.vs = bytecode_of(&d.VS);
    s.ps = bytecode_of(&d.PS);
    s.ds = bytecode_of(&d.DS);
    s.hs = bytecode_of(&d.HS);
    s.gs = bytecode_of(&d.GS);
    s.stream_output_entries = d.StreamOutput.NumEntries;
    s.blend.alpha_to_coverage = d.BlendState.AlphaToCoverageEnable.as_bool();
    s.blend.independent = d.BlendState.IndependentBlendEnable.as_bool();
    for (out, r) in s.blend.rt.iter_mut().zip(&d.BlendState.RenderTarget) {
        *out = pso::RtBlend {
            enable: r.BlendEnable.as_bool(),
            logic_enable: r.LogicOpEnable.as_bool(),
            src: r.SrcBlend.0 as u32,
            dst: r.DestBlend.0 as u32,
            op: r.BlendOp.0 as u32,
            src_alpha: r.SrcBlendAlpha.0 as u32,
            dst_alpha: r.DestBlendAlpha.0 as u32,
            op_alpha: r.BlendOpAlpha.0 as u32,
            logic_op: r.LogicOp.0 as u32,
            write_mask: r.RenderTargetWriteMask,
        };
    }
    s.sample_mask = d.SampleMask;
    let r = &d.RasterizerState;
    s.raster = pso::Raster {
        fill: r.FillMode.0 as u32,
        cull: r.CullMode.0 as u32,
        front_ccw: r.FrontCounterClockwise.as_bool(),
        depth_bias: r.DepthBias,
        depth_bias_clamp: r.DepthBiasClamp,
        slope_scaled_bias: r.SlopeScaledDepthBias,
        depth_clip: r.DepthClipEnable.as_bool(),
        multisample: r.MultisampleEnable.as_bool(),
        antialiased_line: r.AntialiasedLineEnable.as_bool(),
        forced_sample_count: r.ForcedSampleCount,
        conservative: r.ConservativeRaster.0 as u32,
    };
    let z = &d.DepthStencilState;
    s.depth = pso::DepthStencil {
        depth_enable: z.DepthEnable.as_bool(),
        depth_write_mask: z.DepthWriteMask.0 as u32,
        depth_func: z.DepthFunc.0 as u32,
        stencil_enable: z.StencilEnable.as_bool(),
        stencil_read_mask: z.StencilReadMask,
        stencil_write_mask: z.StencilWriteMask,
        front: stencil_ops_of(&z.FrontFace),
        back: stencil_ops_of(&z.BackFace),
        depth_bounds: false,
    };
    s.input_layout = pso::InputLayoutRef {
        elements: d.InputLayout.pInputElementDescs as usize as u64,
        count: d.InputLayout.NumElements,
    };
    s.strip_cut = d.IBStripCutValue.0 as u32;
    s.topology = d.PrimitiveTopologyType.0 as u32;
    s.rt_count = d.NumRenderTargets;
    for (out, f) in s.rt_formats.iter_mut().zip(&d.RTVFormats) {
        *out = f.0 as u32;
    }
    s.ds_format = d.DSVFormat.0 as u32;
    s.sample_count = d.SampleDesc.Count;
    s.sample_quality = d.SampleDesc.Quality;
    s.node_mask = d.NodeMask;
    s.flags = d.Flags.0 as u32;
    s
}

// The layouts `crate::pso::parse_stream` and `read_input_layout` read by
// hand (64-bit): checked against the bindings at compile time.
const _: () = {
    use std::mem::size_of;
    assert!(size_of::<D3D12_SHADER_BYTECODE>() == 16);
    assert!(size_of::<D3D12_RENDER_TARGET_BLEND_DESC>() == 40);
    assert!(size_of::<D3D12_BLEND_DESC>() == 328);
    assert!(size_of::<D3D12_RASTERIZER_DESC>() == 44);
    assert!(size_of::<D3D12_DEPTH_STENCIL_DESC>() == 52);
    assert!(size_of::<D3D12_DEPTH_STENCIL_DESC1>() == 56);
    assert!(size_of::<D3D12_INPUT_ELEMENT_DESC>() == 32);
    assert!(size_of::<D3D12_INPUT_LAYOUT_DESC>() == 16);
    assert!(size_of::<D3D12_STREAM_OUTPUT_DESC>() == 32);
    assert!(size_of::<D3D12_VIEW_INSTANCING_DESC>() == 24);
    assert!(size_of::<D3D12_CACHED_PIPELINE_STATE>() == 16);
};
