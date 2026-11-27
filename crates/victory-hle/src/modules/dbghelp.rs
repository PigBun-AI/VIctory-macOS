//! Mac patch H2 (docs/mac/PATCHES.md): `dbghelp.dll` stubs.
//!
//! The title's crash handler loads `dbghelp.dll` by name and looks up one
//! function, `MiniDumpWriteDump` (no static import; its other strings name no
//! `Sym*` call). Under Wine, `MiniDumpWriteDump` runs `SymInitializeW` /
//! `SymLoadModuleExW` over `victory.exe`'s Rust DWARF and Wine's parser reads
//! garbage (`dwarf2_parse_subroutine_type`), a second crash that hides the
//! first exception. A minidump is of no use to us and the title still writes
//! its own `crash.log`, so the library is not passed through: these stubs
//! fail, touch none of the caller's pointers, and set `ERROR_NOT_SUPPORTED`
//! where the real call would have a last error.
//!
//! Shared layer, not Mac-only: Windows' dbghelp does not crash here, but there
//! is no platform switch in `HostPolicy` and the dump is worth nothing on PC
//! either. (`victory-loader`'s `crash.rs` and the timeline report do not go
//! through this module: the former uses `RtlCaptureStackBackTrace`, the
//! latter loads the system's dbghelp itself.)
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::c_void;

use crate::{HleModule, hle_export};

/// `ERROR_NOT_SUPPORTED`.
const NOT_SUPPORTED: u32 = 50;

fn fail() -> i32 {
    #[cfg(windows)]
    // SAFETY: sets this thread's last error.
    unsafe {
        windows_sys::Win32::Foundation::SetLastError(NOT_SUPPORTED)
    };
    0
}

/// Returns FALSE; the handles, path, exception and callback pointers are not read.
pub extern "system" fn MiniDumpWriteDump(
    _process: *mut c_void,
    _pid: u32,
    _file: *mut c_void,
    _kind: u32,
    _exception: *const c_void,
    _user: *const c_void,
    _callback: *const c_void,
) -> i32 {
    tracing::warn!("minidump skipped (dbghelp stubbed)");
    fail()
}

pub extern "system" fn SymInitialize(_: *mut c_void, _: *const u8, _: i32) -> i32 {
    fail()
}

pub extern "system" fn SymInitializeW(_: *mut c_void, _: *const u16, _: i32) -> i32 {
    fail()
}

pub extern "system" fn SymCleanup(_: *mut c_void) -> i32 {
    fail()
}

/// The option mask stays as it was: 0.
pub extern "system" fn SymSetOptions(_: u32) -> u32 {
    0
}

pub extern "system" fn SymGetOptions() -> u32 {
    0
}

pub extern "system" fn SymFromAddr(_: *mut c_void, _: u64, _: *mut u64, _: *mut c_void) -> i32 {
    fail()
}

pub extern "system" fn SymFromAddrW(_: *mut c_void, _: u64, _: *mut u64, _: *mut c_void) -> i32 {
    fail()
}

pub extern "system" fn SymGetLineFromAddr64(
    _: *mut c_void,
    _: u64,
    _: *mut u32,
    _: *mut c_void,
) -> i32 {
    fail()
}

pub(crate) fn module() -> HleModule {
    HleModule {
        name: "dbghelp",
        dlls: &["dbghelp.dll"],
        exports: vec![
            hle_export!(MiniDumpWriteDump),
            hle_export!(SymInitialize),
            hle_export!(SymInitializeW),
            hle_export!(SymCleanup),
            hle_export!(SymSetOptions),
            hle_export!(SymGetOptions),
            hle_export!(SymFromAddr),
            hle_export!(SymFromAddrW),
            hle_export!(SymGetLineFromAddr64),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HostPolicy, registry::lookup};

    #[test]
    fn dbghelp_is_not_passed_to_the_host() {
        assert!(!HostPolicy::default().allows("dbghelp.dll"));
        assert!(!HostPolicy::default().allows("DbgHelp.DLL"));
    }

    #[test]
    fn minidump_fails_and_touches_no_pointer() {
        // Pointers that would fault if read or written.
        let bad = 0x10usize as *const c_void;
        let r = MiniDumpWriteDump(bad as _, 1, bad as _, 0, bad, bad, bad);
        assert_eq!(r, 0);
        assert_eq!(SymInitializeW(bad as _, bad.cast(), 1), 0);
        assert_eq!(SymInitialize(bad as _, bad.cast(), 1), 0);
        assert_eq!(SymCleanup(bad as _), 0);
        assert_eq!(SymFromAddr(bad as _, 1, bad as _, bad as _), 0);
    }

    #[test]
    fn the_title_finds_minidump_by_name() {
        let a = lookup("dbghelp.dll", "MiniDumpWriteDump").expect("exported");
        assert_eq!(a, MiniDumpWriteDump as *const () as usize);
    }
}
