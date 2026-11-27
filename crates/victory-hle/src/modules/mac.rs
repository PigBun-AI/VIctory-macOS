//! Mac patch H1 (docs/mac/PATCHES.md): `SuspendThread` for the title's own
//! threads only, and only under Wine on Rosetta (the module is not registered
//! anywhere else). Ported from VIctory-macOS (#50): under Wine on Rosetta a
//! thread of ours suspended while Wine held its virtual-memory lock left the
//! lock held for good, and every thread that allocated or faulted waited on
//! it (a shader pipeline worker suspended inside a server call made from
//! `NtQueryVolumeInformationFile`). Threads that start outside the title's
//! images (VIctory's, Wine's, D3DMetal's) are reported as suspended without
//! being stopped; the title cannot tell, as it never made them.

use std::ffi::c_void;

use super::files::host;
use crate::{HleModule, hle_export};

/// The thread's start routine (`ThreadQuerySetWin32StartAddress`), 0 if unknown.
fn start_address(thread: *mut c_void) -> usize {
    let mut start = 0usize;
    host!("ntdll.dll", "NtQueryInformationThread": extern "system" fn(*mut c_void, u32, *mut c_void, u32, *mut u32) -> i32)(
        thread, 9, (&raw mut start).cast(), size_of::<usize>() as u32, std::ptr::null_mut(),
    );
    start
}

pub extern "system" fn SuspendThread(thread: *mut c_void) -> u32 {
    let start = start_address(thread);
    if !victory_loader::mac::in_title_image(start) {
        static SPARED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SPARED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n.is_power_of_two() {
            tracing::warn!(n, start = format_args!("{start:#x}"), "the title asked to suspend a thread it did not make; left running");
        }
        return 0;
    }
    host!("kernel32.dll", "SuspendThread": extern "system" fn(*mut c_void) -> u32)(thread)
}

/// Registered only under Rosetta (`modules::all`).
pub(crate) fn module() -> HleModule {
    HleModule {
        name: "mac",
        dlls: &["kernel32.dll", "kernelbase.dll", "api-ms-win-core-processthreads-*"],
        exports: vec![hle_export!(SuspendThread)],
    }
}
