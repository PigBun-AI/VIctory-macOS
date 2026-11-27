//! `ID3D12PipelineLibrary1`: the object over [`crate::library`]. It keeps
//! names and fingerprints; a load that matches makes the pipeline through
//! the device's own `Create*PipelineState` (see the core's notes).

use std::ffi::c_void;
use std::sync::Mutex;

use victory_metal_proto::pso::{GraphicsPsoDesc, StageCode};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{
    ComObject, GUID, IUnknown, Interface, PCWSTR, Ref, Result as WResult, implement,
};

use super::device::{DeviceObj, refuse, state_of_desc};
use super::objects::{PsoObj, RootSigObj};
use super::private::PrivateData;
use super::{return_object, werr};
use crate::device::RootSignature;
use crate::hresult::Hr;
use crate::library::{Fingerprint, Library};
use crate::pso;

#[implement(ID3D12PipelineLibrary1)]
pub struct LibraryObj {
    pub private: PrivateData,
    pub device_obj: ComObject<DeviceObj>,
    library: Mutex<Library>,
}

/// What a compute pipeline is made from: the root signature as the
/// application gave it and the shader.
pub(super) fn compute_fingerprint(rs: &RootSignature, dxil: &[u8]) -> u64 {
    Fingerprint::new()
        .bytes(b"compute")
        .bytes(&rs.blob)
        .bytes(dxil)
        .finish()
}

/// What a graphics pipeline is made from: the root signature, the proto
/// description (with the stages' addresses taken out) and the stages' bytes.
pub(super) fn graphics_fingerprint(rs: &RootSignature, desc: &GraphicsPsoDesc) -> u64 {
    let mut plain = *desc;
    let mut f = Fingerprint::new().bytes(b"graphics").bytes(&rs.blob);
    for stage in [&mut plain.vs, &mut plain.ps, &mut plain.amp, &mut plain.ms] {
        let (addr, len) = (stage.addr, stage.len as usize);
        *stage = StageCode::NONE;
        let bytes: &[u8] = if addr == 0 || len == 0 {
            &[]
        } else {
            // SAFETY: the application's shader bytecode, valid for the call
            // that gave it (`len` readable bytes at `addr`).
            unsafe { std::slice::from_raw_parts(addr as *const u8, len) }
        };
        f = f.bytes(bytes);
        stage.len = len as u64;
    }
    f.bytes(format!("{plain:?}").as_bytes()).finish()
}

impl LibraryObj {
    /// `ID3D12Device1::CreatePipelineLibrary`.
    pub fn create(
        device_obj: ComObject<DeviceObj>,
        blob: *const c_void,
        len: usize,
        riid: *const GUID,
        out: *mut *mut c_void,
    ) -> WResult<()> {
        let bytes: &[u8] = if blob.is_null() || len == 0 {
            &[]
        } else {
            // SAFETY: `len` readable bytes at `blob`, the caller's initial data.
            unsafe { std::slice::from_raw_parts(blob.cast::<u8>(), len) }
        };
        if blob.is_null() && len != 0 {
            return Err(werr(Hr::E_INVALIDARG));
        }
        let library = Library::from_blob(bytes).map_err(werr)?;
        let lib = ComObject::new(LibraryObj {
            private: PrivateData::default(),
            device_obj,
            library: Mutex::new(library),
        });
        return_object(&lib.to_interface::<IUnknown>(), riid, out)
    }
}

fn name_of(name: &PCWSTR) -> WResult<String> {
    if name.is_null() {
        return Err(werr(Hr::E_INVALIDARG));
    }
    // SAFETY: a NUL-terminated name, the caller's.
    Ok(String::from_utf16_lossy(unsafe { name.as_wide() }))
}

impl LibraryObj_Impl {
    fn library(&self) -> std::sync::MutexGuard<'_, Library> {
        self.library.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn device(&self) -> ID3D12Device14 {
        self.device_obj.to_interface()
    }

    pub(super) fn x_StorePipeline(
        &self,
        name: &PCWSTR,
        ppipeline: Ref<ID3D12PipelineState>,
    ) -> WResult<()> {
        let name = name_of(name)?;
        let p = ppipeline.ok().map_err(|_| werr(Hr::E_INVALIDARG))?;
        let fingerprint = p.cast_object_ref::<PsoObj>()?.fingerprint;
        self.library().store(&name, fingerprint).map_err(werr)
    }

    pub(super) fn x_LoadGraphicsPipeline(
        &self,
        name: &PCWSTR,
        pdesc: *const D3D12_GRAPHICS_PIPELINE_STATE_DESC,
        riid: *const GUID,
        ppipelinestate: *mut *mut c_void,
    ) -> WResult<()> {
        let name = name_of(name)?;
        // SAFETY: the caller's description.
        let d = unsafe { pdesc.as_ref() }.ok_or_else(|| werr(Hr::E_INVALIDARG))?;
        let rs = d
            .pRootSignature
            .as_ref()
            .ok_or_else(|| werr(Hr::E_INVALIDARG))?;
        let state = state_of_desc(d);
        self.library()
            .load(&name, graphics_of_state(rs, &state)?)
            .map_err(werr)?;
        // SAFETY: the same description, valid for the call.
        let pso: ID3D12PipelineState = unsafe { self.device().CreateGraphicsPipelineState(d) }?;
        return_object(&pso.cast::<IUnknown>()?, riid, ppipelinestate)
    }

    pub(super) fn x_LoadComputePipeline(
        &self,
        name: &PCWSTR,
        pdesc: *const D3D12_COMPUTE_PIPELINE_STATE_DESC,
        riid: *const GUID,
        ppipelinestate: *mut *mut c_void,
    ) -> WResult<()> {
        let name = name_of(name)?;
        // SAFETY: the caller's description.
        let d = unsafe { pdesc.as_ref() }.ok_or_else(|| werr(Hr::E_INVALIDARG))?;
        let rs = d
            .pRootSignature
            .as_ref()
            .ok_or_else(|| werr(Hr::E_INVALIDARG))?;
        let rs = rs.cast_object_ref::<RootSigObj>()?;
        if d.CS.pShaderBytecode.is_null() || d.CS.BytecodeLength == 0 {
            return Err(werr(Hr::E_INVALIDARG));
        }
        // SAFETY: `BytecodeLength` readable bytes, the caller's shader.
        let dxil = unsafe {
            std::slice::from_raw_parts(d.CS.pShaderBytecode.cast::<u8>(), d.CS.BytecodeLength)
        };
        self.library()
            .load(&name, compute_fingerprint(&rs.root_sig, dxil))
            .map_err(werr)?;
        // SAFETY: as above.
        let pso: ID3D12PipelineState = unsafe { self.device().CreateComputePipelineState(d) }?;
        return_object(&pso.cast::<IUnknown>()?, riid, ppipelinestate)
    }

    pub(super) fn x_LoadPipeline(
        &self,
        name: &PCWSTR,
        pdesc: *const D3D12_PIPELINE_STATE_STREAM_DESC,
        riid: *const GUID,
        ppipelinestate: *mut *mut c_void,
    ) -> WResult<()> {
        let name = name_of(name)?;
        // SAFETY: the caller's description.
        let d = unsafe { pdesc.as_ref() }.ok_or_else(|| werr(Hr::E_INVALIDARG))?;
        if d.pPipelineStateSubobjectStream.is_null() || d.SizeInBytes == 0 {
            return Err(werr(Hr::E_INVALIDARG));
        }
        // SAFETY: `SizeInBytes` readable bytes, the caller's stream.
        let bytes = unsafe {
            std::slice::from_raw_parts(d.pPipelineStateSubobjectStream.cast::<u8>(), d.SizeInBytes)
        };
        let state = pso::parse_stream(bytes).map_err(refuse)?;
        let raw = state.root_sig as *mut c_void;
        // SAFETY: the root signature the application put in the stream,
        // borrowed for the call.
        let rs = unsafe { ID3D12RootSignature::from_raw_borrowed(&raw) }
            .ok_or_else(|| werr(Hr::E_INVALIDARG))?;
        self.library()
            .load(&name, graphics_of_state(rs, &state)?)
            .map_err(werr)?;
        // SAFETY: as above.
        let pso: ID3D12PipelineState = unsafe { self.device().CreatePipelineState(d) }?;
        return_object(&pso.cast::<IUnknown>()?, riid, ppipelinestate)
    }

    pub(super) fn x_GetSerializedSize(&self) -> usize {
        self.library().serialized_size()
    }

    pub(super) fn x_Serialize(&self, pdata: *mut c_void, datasizeinbytes: usize) -> WResult<()> {
        if pdata.is_null() {
            return Err(werr(Hr::E_INVALIDARG));
        }
        // SAFETY: `datasizeinbytes` writable bytes at `pdata`, the caller's buffer.
        let out = unsafe { std::slice::from_raw_parts_mut(pdata.cast::<u8>(), datasizeinbytes) };
        self.library().serialize_into(out).map_err(werr)
    }
}

/// The fingerprint a graphics pipeline made from `state` has.
fn graphics_of_state(rs: &ID3D12RootSignature, state: &pso::PipelineState) -> WResult<u64> {
    let rs = rs.cast_object_ref::<RootSigObj>()?;
    // SAFETY: the application's input layout is valid for the call.
    let inputs = unsafe { pso::read_input_layout(state.input_layout) }.map_err(refuse)?;
    let desc = pso::to_proto(state, &inputs).map_err(refuse)?;
    Ok(graphics_fingerprint(&rs.root_sig, &desc))
}
