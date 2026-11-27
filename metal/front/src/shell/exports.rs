//! The bodies of the DLL exports. The cdylib crates (`front/d3d12`,
//! `front/dxgi`) declare the `extern "system"` symbols and call these, so
//! the exported names and the implementation stay apart.

use std::ffi::c_void;

use windows::Win32::Foundation::{E_INVALIDARG, E_NOINTERFACE, E_POINTER, S_OK};
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{ComObject, GUID, HRESULT, IUnknown, Interface, Result as WResult};

use super::device::DeviceObj;
use super::dxgi::{AdapterObj, FactoryObj};
use super::private::PrivateData;
use super::serialize::{self, RootSigDeserializerObj, Versioned, VersionedDeserializerObj};
use super::{OrWerr, return_object, werr, wres};
use crate::caps;
use crate::hresult::Hr;
use crate::rootsig;

fn code(r: WResult<()>) -> HRESULT {
    match r {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    }
}

/// `DXGI_ERROR_SDK_COMPONENT_MISSING`: what a machine without the debug layers answers.
const SDK_COMPONENT_MISSING: HRESULT = HRESULT(0x887A_002Du32 as i32);

// ---- d3d12.dll ---------------------------------------------------------------

/// # Safety
/// `adapter` is null or a COM pointer; `ppdevice` is null or writable.
pub unsafe fn d3d12_create_device(
    adapter: *mut c_void,
    min_feature_level: i32,
    riid: *const GUID,
    ppdevice: *mut *mut c_void,
) -> HRESULT {
    crate::callstats::call("d3d12.dll::D3D12CreateDevice");
    code((|| {
        if !adapter.is_null() {
            // Any COM object will do as the adapter the application got from us.
            // SAFETY: a COM pointer the caller owns; borrowed, not released.
            let unk = unsafe { IUnknown::from_raw_borrowed(&adapter) }.or_werr(Hr::E_POINTER)?;
            if unk.cast_object_ref::<AdapterObj>().is_err() {
                // An adapter from another implementation: refuse like DXGI would.
                return Err(werr(Hr::E_INVALIDARG));
            }
        }
        let dev = DeviceObj::create()?;
        if min_feature_level as u32 > dev.dev.sys.caps.max_feature_level {
            return Err(werr(Hr::DXGI_ERROR_UNSUPPORTED));
        }
        let _ = caps::FEATURE_LEVEL_12_0;
        return_object(&dev.to_interface::<IUnknown>(), riid, ppdevice)
    })())
}

/// # Safety
/// `desc` is a valid description; the out pointers are null or writable.
pub unsafe fn d3d12_serialize_root_signature(
    desc: *const D3D12_ROOT_SIGNATURE_DESC,
    version: D3D_ROOT_SIGNATURE_VERSION,
    blob: *mut Option<ID3DBlob>,
    error: *mut Option<ID3DBlob>,
) -> HRESULT {
    crate::callstats::call("d3d12.dll::D3D12SerializeRootSignature");
    if !error.is_null() {
        // SAFETY: writable out pointer.
        unsafe { *error = None };
    }
    code((|| {
        // SAFETY: caller's pointers.
        let d = unsafe { desc.as_ref() }.ok_or(windows::core::Error::from(E_INVALIDARG))?;
        if blob.is_null() {
            return Err(windows::core::Error::from(E_POINTER));
        }
        // SAFETY: forwarded contract.
        let b = unsafe { serialize::serialize_1_0(d, version) }?;
        // SAFETY: `blob` checked.
        unsafe { *blob = Some(b) };
        Ok(())
    })())
}

/// # Safety
/// As [`d3d12_serialize_root_signature`].
pub unsafe fn d3d12_serialize_versioned_root_signature(
    desc: *const D3D12_VERSIONED_ROOT_SIGNATURE_DESC,
    blob: *mut Option<ID3DBlob>,
    error: *mut Option<ID3DBlob>,
) -> HRESULT {
    crate::callstats::call("d3d12.dll::D3D12SerializeVersionedRootSignature");
    if !error.is_null() {
        // SAFETY: writable out pointer.
        unsafe { *error = None };
    }
    code((|| {
        // SAFETY: caller's pointers.
        let d = unsafe { desc.as_ref() }.ok_or(windows::core::Error::from(E_INVALIDARG))?;
        if blob.is_null() {
            return Err(windows::core::Error::from(E_POINTER));
        }
        // SAFETY: forwarded contract.
        let b = unsafe { serialize::serialize_versioned(d) }?;
        // SAFETY: `blob` checked.
        unsafe { *blob = Some(b) };
        Ok(())
    })())
}

fn parse_blob(src: *const c_void, size: usize) -> WResult<rootsig::RootSignatureDesc> {
    if src.is_null() || size == 0 {
        return Err(werr(Hr::E_INVALIDARG));
    }
    // SAFETY: `size` readable bytes at `src`.
    let bytes = unsafe { std::slice::from_raw_parts(src.cast::<u8>(), size) };
    wres(rootsig::deserialize(bytes))
}

/// # Safety
/// `src` has `size` readable bytes; `pp` is null or writable.
pub unsafe fn d3d12_create_root_signature_deserializer(
    src: *const c_void,
    size: usize,
    riid: *const GUID,
    pp: *mut *mut c_void,
) -> HRESULT {
    crate::callstats::call("d3d12.dll::D3D12CreateRootSignatureDeserializer");
    code((|| {
        let d = parse_blob(src, size)?;
        let obj = ComObject::new(RootSigDeserializerObj {
            private: PrivateData::default(),
            desc: serialize::write_1_0(&d),
        });
        return_object(&obj.to_interface::<IUnknown>(), riid, pp)
    })())
}

/// # Safety
/// As [`d3d12_create_root_signature_deserializer`].
pub unsafe fn d3d12_create_versioned_root_signature_deserializer(
    src: *const c_void,
    size: usize,
    riid: *const GUID,
    pp: *mut *mut c_void,
) -> HRESULT {
    crate::callstats::call("d3d12.dll::D3D12CreateVersionedRootSignatureDeserializer");
    code((|| {
        let d = parse_blob(src, size)?;
        let obj = ComObject::new(VersionedDeserializerObj {
            private: PrivateData::default(),
            desc: Versioned::new(&d),
        });
        return_object(&obj.to_interface::<IUnknown>(), riid, pp)
    })())
}

pub fn d3d12_get_debug_interface() -> HRESULT {
    crate::callstats::call("d3d12.dll::D3D12GetDebugInterface");
    SDK_COMPONENT_MISSING
}

pub fn d3d12_enable_experimental_features(n: u32) -> HRESULT {
    crate::callstats::call("d3d12.dll::D3D12EnableExperimentalFeatures");
    if n == 0 { S_OK } else { E_INVALIDARG }
}

pub fn d3d12_get_interface() -> HRESULT {
    crate::callstats::call("d3d12.dll::D3D12GetInterface");
    E_NOINTERFACE
}

// ---- dxgi.dll ----------------------------------------------------------------

/// `CreateDXGIFactory`, `CreateDXGIFactory1` and `CreateDXGIFactory2`.
///
/// # Safety
/// `riid` valid; `pp` writable.
pub unsafe fn dxgi_create_factory(riid: *const GUID, pp: *mut *mut c_void) -> HRESULT {
    crate::callstats::call("dxgi.dll::CreateDXGIFactory*");
    if pp.is_null() {
        return E_POINTER;
    }
    // SAFETY: writable out pointer.
    unsafe { *pp = std::ptr::null_mut() };
    code((|| {
        let f = FactoryObj::create()?;
        return_object(&f.to_interface::<IUnknown>(), riid, pp)
    })())
}

pub fn dxgi_get_debug_interface() -> HRESULT {
    crate::callstats::call("dxgi.dll::DXGIGetDebugInterface1");
    SDK_COMPONENT_MISSING
}
