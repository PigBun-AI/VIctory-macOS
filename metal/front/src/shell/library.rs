//! `ID3D12PipelineLibrary1`: the object over [`crate::library`], the library
//! that holds nothing.

use std::ffi::c_void;

use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{ComObject, GUID, IUnknown, Ref, Result as WResult, implement};

use super::device::DeviceObj;
use super::private::PrivateData;
use super::{return_object, werr};
use crate::hresult::Hr;
use crate::library;

#[implement(ID3D12PipelineLibrary1)]
pub struct LibraryObj {
    pub private: PrivateData,
    pub device_obj: ComObject<DeviceObj>,
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
        library::check_blob(bytes).map_err(werr)?;
        let lib = ComObject::new(LibraryObj {
            private: PrivateData::default(),
            device_obj,
        });
        return_object(&lib.to_interface::<IUnknown>(), riid, out)
    }
}

impl LibraryObj_Impl {
    /// Accepted and dropped: the library never holds a pipeline.
    pub(super) fn x_StorePipeline(
        &self,
        _name: &windows::core::PCWSTR,
        _ppipeline: Ref<ID3D12PipelineState>,
    ) -> WResult<()> {
        Ok(())
    }

    /// Never in the library.
    pub(super) fn x_LoadGraphicsPipeline(
        &self,
        _name: &windows::core::PCWSTR,
        _pdesc: *const D3D12_GRAPHICS_PIPELINE_STATE_DESC,
        _riid: *const GUID,
        _ppipelinestate: *mut *mut c_void,
    ) -> WResult<()> {
        Err(werr(Hr::E_INVALIDARG))
    }

    pub(super) fn x_LoadComputePipeline(
        &self,
        _name: &windows::core::PCWSTR,
        _pdesc: *const D3D12_COMPUTE_PIPELINE_STATE_DESC,
        _riid: *const GUID,
        _ppipelinestate: *mut *mut c_void,
    ) -> WResult<()> {
        Err(werr(Hr::E_INVALIDARG))
    }

    pub(super) fn x_LoadPipeline(
        &self,
        _name: &windows::core::PCWSTR,
        _pdesc: *const D3D12_PIPELINE_STATE_STREAM_DESC,
        _riid: *const GUID,
        _ppipelinestate: *mut *mut c_void,
    ) -> WResult<()> {
        Err(werr(Hr::E_INVALIDARG))
    }

    pub(super) fn x_GetSerializedSize(&self) -> usize {
        library::SERIALIZED_SIZE
    }

    pub(super) fn x_Serialize(&self, pdata: *mut c_void, datasizeinbytes: usize) -> WResult<()> {
        if pdata.is_null() {
            return Err(werr(Hr::E_INVALIDARG));
        }
        // SAFETY: `datasizeinbytes` writable bytes at `pdata`, the caller's buffer.
        let out = unsafe { std::slice::from_raw_parts_mut(pdata.cast::<u8>(), datasizeinbytes) };
        library::serialize_into(out).map_err(werr)
    }
}
