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
pub(crate) mod ext;
mod library;
mod list;
mod objects;
mod private;
mod serialize;
mod swapchain;
#[rustfmt::skip]
mod stubs;

pub use private::PrivateData;

/// Every method the front has no implementation for calls this (the generated
/// stubs do). With `VICTORY_METAL_NOTIMPL_LOG=<file>` set, one line
/// `Interface::Method` is appended to that file per call: the data for "which
/// unimplemented methods does this program call, how often"
/// (`tools/metal/notimpl-stats.sh`). Without the variable it costs one
/// environment lookup, cached.
pub(crate) fn note_notimpl(what: &str) {
    use std::io::Write;
    use std::sync::OnceLock;
    static LOG: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    let Some(path) = LOG.get_or_init(|| {
        std::env::var_os("VICTORY_METAL_NOTIMPL_LOG")
            .filter(|v| !v.is_empty())
            .map(Into::into)
    }) else {
        return;
    };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{what}");
    }
}

/// `VICTORY_METAL_LENIENT=1`: see `ListObj::unsupported_call`.
pub(crate) fn lenient() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_METAL_LENIENT").is_some_and(|v| v != "0"))
}

pub(crate) fn log_errors() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_METAL_LOG").is_some_and(|v| v != "0"))
}

/// Core `Hr` → COM error.
#[track_caller]
pub(crate) fn werr(h: Hr) -> Error {
    // `VICTORY_METAL_LOG=1`: every error the front hands back, with the caller.
    if log_errors() {
        eprintln!(
            "victory-metal front: error {:#010x} returned from {}",
            h.0 as u32,
            std::panic::Location::caller()
        );
    }
    Error::from(HRESULT(h.0))
}

/// Core result → COM result.
#[track_caller]
pub(crate) fn wres<T>(r: Result<T>) -> WResult<T> {
    match r {
        Ok(v) => Ok(v),
        Err(h) => Err(werr(h)),
    }
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
