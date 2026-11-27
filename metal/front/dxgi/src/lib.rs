//! dxgi.dll of the Metal backend: only the exported symbols. Every body
//! lives in `victory_metal_front::shell::exports`.
#![cfg(windows)]
#![allow(non_snake_case, clippy::missing_safety_doc)]

use std::ffi::c_void;

use victory_metal_front::shell::exports as e;
use windows::core::{GUID, HRESULT};

#[unsafe(no_mangle)]
pub unsafe extern "system" fn CreateDXGIFactory(
    riid: *const GUID,
    pp: *mut *mut c_void,
) -> HRESULT {
    unsafe { e::dxgi_create_factory(riid, pp) }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn CreateDXGIFactory1(
    riid: *const GUID,
    pp: *mut *mut c_void,
) -> HRESULT {
    unsafe { e::dxgi_create_factory(riid, pp) }
}

/// The debug flag is accepted and ignored.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn CreateDXGIFactory2(
    _flags: u32,
    riid: *const GUID,
    pp: *mut *mut c_void,
) -> HRESULT {
    unsafe { e::dxgi_create_factory(riid, pp) }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DXGIGetDebugInterface1(
    _flags: u32,
    _riid: *const GUID,
    _pp: *mut *mut c_void,
) -> HRESULT {
    e::dxgi_get_debug_interface()
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DXGIDeclareAdapterRemovalSupport() -> HRESULT {
    HRESULT(0)
}

/// Names the call-statistics file by this DLL and writes it when the DLL is
/// unloaded (`VICTORY_METAL_CALLSTATS`, `victory_metal_front::callstats`).
#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_instance: *mut c_void, reason: u32, _reserved: *mut c_void) -> i32 {
    victory_metal_front::callstats::dll_main("dxgi", reason);
    1
}
