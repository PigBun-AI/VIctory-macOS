//! The COM shell (Windows targets only): ABI, reference counting,
//! `QueryInterface`, argument decoding. Everything D3D12 is decided in the
//! core (`crate::device` and friends); a shell method reads its raw
//! arguments, calls the core, writes the results.
//!
//! * `gen.rs` is generated (`tools/metal/gen-front-stubs.sh`): every method
//!   of every claimed interface, calling the `x_<Method>` written by hand in
//!   the other files, or a stub (`E_NOTIMPL`, no-op).
//! * `exports.rs` has the DLL entry points (`D3D12CreateDevice`, ...).

// The shell is the C ABI: methods take raw pointers and many arguments by contract.
#![allow(
    non_snake_case,
    clippy::too_many_arguments,
    clippy::not_unsafe_ptr_arg_deref
)]

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::{E_NOINTERFACE, E_POINTER};
use windows::core::{ComObject, Error, GUID, HRESULT, IUnknown, Interface, Result as WResult};

use crate::device::System;
use crate::hresult::{Hr, Result};

mod device;
mod dxgi;
pub mod exports;
mod list;
mod objects;
mod private;
mod serialize;
#[rustfmt::skip]
mod stubs;

pub use private::PrivateData;

/// Core `Hr` → COM error.
pub(crate) fn werr(h: Hr) -> Error {
    Error::from(HRESULT(h.0))
}

/// Core result → COM result.
pub(crate) fn wres<T>(r: Result<T>) -> WResult<T> {
    r.map_err(werr)
}

/// Hand a new object to an `(riid, ppv)` pair: QueryInterface for `riid` on
/// it and write the pointer. A null `ppv` means "validate only", as D3D12
/// allows (the object is dropped).
pub(crate) fn return_object(
    obj: &IUnknown,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> WResult<()> {
    if ppv.is_null() {
        return Ok(());
    }
    if riid.is_null() {
        return Err(Error::from(E_POINTER));
    }
    // SAFETY: `ppv` is non-null and the caller's out pointer.
    unsafe { obj.query(riid, ppv) }.ok()
}

/// `ID3D12DeviceChild::GetDevice`.
pub(crate) fn query_device(
    device: &ComObject<device::DeviceObj>,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> WResult<()> {
    let unk: IUnknown = device.to_interface();
    if ppv.is_null() {
        return Err(Error::from(E_POINTER));
    }
    // SAFETY: as above.
    unsafe { unk.query(riid, ppv) }.ok()
}

/// The connection to the back, opened on first use and shared by every
/// factory and device of the process.
static SYSTEM: Mutex<Option<Arc<System>>> = Mutex::new(None);

pub(crate) fn system() -> Result<Arc<System>> {
    let mut g = SYSTEM.lock().unwrap();
    if let Some(s) = &*g {
        return Ok(s.clone());
    }
    let link = crate::connect::link().map_err(|_| Hr::DXGI_ERROR_UNSUPPORTED)?;
    let s = System::open(link).map_err(|_| Hr::DXGI_ERROR_UNSUPPORTED)?;
    *g = Some(s.clone());
    Ok(s)
}

/// An `IUnknown` we do not have: `E_NOINTERFACE` as a COM error.
#[allow(dead_code)]
pub(crate) fn no_interface() -> Error {
    Error::from(E_NOINTERFACE)
}
