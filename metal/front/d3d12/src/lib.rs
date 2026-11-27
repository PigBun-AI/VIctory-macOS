//! d3d12.dll of the Metal backend: only the exported symbols. Every body
//! lives in `victory_metal_front::shell::exports`.
#![cfg(windows)]
#![allow(non_snake_case, clippy::missing_safety_doc)]

use std::ffi::c_void;

use victory_metal_front::shell::exports as e;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{GUID, HRESULT};

#[unsafe(no_mangle)]
pub unsafe extern "system" fn D3D12CreateDevice(
    padapter: *mut c_void,
    minimumfeaturelevel: i32,
    riid: *const GUID,
    ppdevice: *mut *mut c_void,
) -> HRESULT {
    unsafe { e::d3d12_create_device(padapter, minimumfeaturelevel, riid, ppdevice) }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn D3D12SerializeRootSignature(
    desc: *const D3D12_ROOT_SIGNATURE_DESC,
    version: D3D_ROOT_SIGNATURE_VERSION,
    blob: *mut Option<ID3DBlob>,
    error: *mut Option<ID3DBlob>,
) -> HRESULT {
    unsafe { e::d3d12_serialize_root_signature(desc, version, blob, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn D3D12SerializeVersionedRootSignature(
    desc: *const D3D12_VERSIONED_ROOT_SIGNATURE_DESC,
    blob: *mut Option<ID3DBlob>,
    error: *mut Option<ID3DBlob>,
) -> HRESULT {
    unsafe { e::d3d12_serialize_versioned_root_signature(desc, blob, error) }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn D3D12CreateRootSignatureDeserializer(
    src: *const c_void,
    size: usize,
    riid: *const GUID,
    pp: *mut *mut c_void,
) -> HRESULT {
    unsafe { e::d3d12_create_root_signature_deserializer(src, size, riid, pp) }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn D3D12CreateVersionedRootSignatureDeserializer(
    src: *const c_void,
    size: usize,
    riid: *const GUID,
    pp: *mut *mut c_void,
) -> HRESULT {
    unsafe { e::d3d12_create_versioned_root_signature_deserializer(src, size, riid, pp) }
}

/// No debug layers: `DXGI_ERROR_SDK_COMPONENT_MISSING`, which applications treat as "not installed".
#[unsafe(no_mangle)]
pub unsafe extern "system" fn D3D12GetDebugInterface(
    _riid: *const GUID,
    _pp: *mut *mut c_void,
) -> HRESULT {
    e::d3d12_get_debug_interface()
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn D3D12EnableExperimentalFeatures(
    n: u32,
    _iids: *const GUID,
    _configs: *mut c_void,
    _sizes: *mut u32,
) -> HRESULT {
    e::d3d12_enable_experimental_features(n)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn D3D12GetInterface(
    _clsid: *const GUID,
    _riid: *const GUID,
    _pp: *mut *mut c_void,
) -> HRESULT {
    e::d3d12_get_interface()
}

/// Names the call-statistics file by this DLL and writes it when the DLL is
/// unloaded (`VICTORY_METAL_CALLSTATS`, `victory_metal_front::callstats`).
#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_instance: *mut c_void, reason: u32, _reserved: *mut c_void) -> i32 {
    victory_metal_front::callstats::dll_main("d3d12", reason);
    1
}
