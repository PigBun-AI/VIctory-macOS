//! Whether this process is the Windows exe on Wine on a Mac, and whether
//! title writes may then fault into the trackers (ported from VIctory-macOS
//! `watch.rs`, issues #1, #14, #50, #144).
//!
//! Hooked in by: `watch::writes` (no tracking when off: callers then copy
//! the memory whole, as for memory that is not the runtime's),
//! `victory_hle`'s `XMemVirtualAlloc` (no MEM_WRITE_WATCH when off) and
//! `files::ReadFile` (waits for an overlapped read when on under Rosetta).

use std::sync::LazyLock;

/// Whether title writes may fault into a tracker: the guards of `watch.rs`
/// and the write watch of the runtime's pools. Every store that hits a
/// tracked page must be resumed after the fault. Rosetta cannot resume some:
/// it prints `unexpectedly need to EmulateForward on a synchronous
/// exception` and the process hangs (VIctory-macOS issue #1). On macOS 27
/// those are AVX masked stores that start in writable memory and end in a
/// protected page, and `vcvtps2ph` stores anywhere in one; the loader
/// rewrites both as plain stores whenever tracking runs under Rosetta
/// ([`stores_need_patching`]). Earlier Rosetta also stops on a plain 32-byte
/// `vmovups` across pages (macOS 26.6, #1), and #144 hung on an M1 with both
/// switches on, so under Rosetta tracking stays off unless
/// `VICTORY_FAULT_TRACKING=1` asks for it. `0` turns tracking off anywhere.
/// Tests keep it on: their stores do not cross pages.
pub fn fault_tracking() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| {
        decide(std::env::var("VICTORY_FAULT_TRACKING").ok().as_deref(), cfg!(test) || !rosetta())
    });
    *ON
}

/// Whether tracking runs under Rosetta, so the stores Rosetta cannot resume
/// after a fault must be rewritten at load time (`victory_loader::mac::masked`).
pub fn stores_need_patching() -> bool {
    fault_tracking() && rosetta()
}

/// `fault_tracking` from the variable's value and the default.
fn decide(var: Option<&str>, default: bool) -> bool {
    match var {
        Some("1") => true,
        Some("0") => false,
        _ => default,
    }
}

/// Whether x86-64 code here runs under Rosetta: Wine on macOS, whose x86-64
/// Wine only runs that way on the Apple silicon Macs D3DMetal needs.
#[cfg(windows)]
pub fn rosetta() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| {
        use std::ffi::{CStr, c_char};
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
        type HostVersion = unsafe extern "C" fn(*mut *const c_char, *mut *const c_char);
        // SAFETY: ntdll is always loaded and both names end with a NUL. Wine's
        // `wine_get_host_version` (cdecl, two out pointers) sets them to static
        // strings: the host's `uname` system name and release.
        unsafe {
            let Some(f) = GetProcAddress(
                GetModuleHandleA(c"ntdll.dll".as_ptr().cast()),
                c"wine_get_host_version".as_ptr().cast(),
            ) else {
                return false;
            };
            let f: HostVersion = std::mem::transmute(f);
            let (mut sysname, mut release) = (std::ptr::null(), std::ptr::null());
            f(&mut sysname, &mut release);
            !sysname.is_null() && CStr::from_ptr(sysname) == c"Darwin"
        }
    });
    *ON
}

/// Not Windows: not the Windows exe, so not under Rosetta.
#[cfg(not(windows))]
pub fn rosetta() -> bool {
    false
}

/// Whether an overlapped `ReadFile` must be waited for before its
/// destination is guarded again: a kernel write into a guarded block under
/// Wine on Rosetta stops the process for good (#50). Only with tracking on
/// under Rosetta (the Windows line does not wait on a PC).
pub fn wait_for_overlapped_reads() -> bool {
    rosetta() && fault_tracking()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_variable_decides_and_tests_track() {
        assert!(decide(Some("1"), false));
        assert!(!decide(Some("0"), true));
        assert!(decide(None, true) && !decide(Some("x"), false));
        assert!(fault_tracking(), "tests keep tracking on");
        // Whether this is Rosetta depends on where the tests run (a PC, a
        // Mac under Wine, natively on macOS): what follows from it must hold
        // on each. Off Windows it is never Rosetta.
        let under = rosetta();
        assert!(!under || cfg!(windows));
        assert_eq!(stores_need_patching(), under, "tracking is on in tests");
        assert_eq!(wait_for_overlapped_reads(), under);
    }
}
