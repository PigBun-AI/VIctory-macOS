//! PE half: reaching the unixlib that sits next to a Wine-marked dll.
//!
//! This is what `winecrt0`'s `__wine_init_unix_call` does, written out so
//! that no C and no winegcc are needed: ask ntdll for the unixlib handle of
//! the dll's own image (`NtQueryVirtualMemory`, class `MemoryWineUnixFuncs`),
//! take `__wine_unix_call_dispatcher` from ntdll's exports, and call it with
//! the handle, a code and an argument pointer.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use victory_metal_proto::{Link, Status, SyncOp};

use crate::abi::{CallArgs, OP_PROTO_CALL, OP_PROTO_SUBMIT, RawLink, SubmitArgs, nt_to_status};

/// `MemoryWineUnixFuncs` in `winternl.h`.
const MEMORY_WINE_UNIX_FUNCS: u32 = 1000;

type Dispatcher = unsafe extern "system" fn(u64, u32, *mut c_void) -> i32;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryVirtualMemory(
        process: *mut c_void,
        base: *const c_void,
        class: u32,
        buffer: *mut c_void,
        len: usize,
        returned: *mut usize,
    ) -> i32;
}

/// A loaded Wine-marked dll and the handle of its unixlib.
pub struct WineUnixlib {
    handle: u64,
    dispatcher: Dispatcher,
    pub module: HMODULE,
}

fn wide(text: &str) -> Vec<u16> {
    std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(Some(0))
        .collect()
}

impl WineUnixlib {
    /// Loads `dll` (found through `WINEDLLPATH` when it is Wine-marked) and
    /// connects to its unixlib. The error carries the failing step.
    pub fn load(dll: &str) -> Result<WineUnixlib, String> {
        let name = wide(dll);
        // SAFETY: plain Win32 calls with NUL-terminated names.
        unsafe {
            let module = LoadLibraryW(name.as_ptr());
            if module.is_null() {
                return Err(format!("LoadLibraryW({dll}) failed"));
            }
            let mut handle = 0u64;
            let status = NtQueryVirtualMemory(
                GetCurrentProcess(),
                module,
                MEMORY_WINE_UNIX_FUNCS,
                (&raw mut handle).cast(),
                size_of::<u64>(),
                std::ptr::null_mut(),
            );
            if status != 0 {
                return Err(format!(
                    "NtQueryVirtualMemory(MemoryWineUnixFuncs) = {:#010x}: {dll} has no unixlib",
                    status as u32
                ));
            }
            let ntdll = GetModuleHandleW(wide("ntdll.dll").as_ptr());
            let export = GetProcAddress(ntdll, c"__wine_unix_call_dispatcher".as_ptr().cast());
            let Some(export) = export else {
                return Err("ntdll has no __wine_unix_call_dispatcher: not Wine".to_owned());
            };
            // The export is a variable holding the function pointer.
            let dispatcher = *(export as *const Dispatcher);
            Ok(WineUnixlib {
                handle,
                dispatcher,
                module,
            })
        }
    }
}

// SAFETY: the module handle is process-global and never freed here, the
// dispatcher is a plain function, and the unixlib entries are re-entrant.
unsafe impl Send for WineUnixlib {}
unsafe impl Sync for WineUnixlib {}

/// The proto over the unixlib: a block or an argument block goes by address
/// (the PE and the unixlib share the process), the status comes back as an
/// `NTSTATUS` that `nt_to_status` reads.
impl Link for WineUnixlib {
    fn submit(&self, block: &[u8]) -> Status {
        let mut a = SubmitArgs {
            block: block.as_ptr() as u64,
            len: block.len() as u64,
        };
        nt_to_status(RawLink::call(self, OP_PROTO_SUBMIT, (&raw mut a).cast()))
    }

    fn call(&self, op: SyncOp, args: *mut u8, len: usize) -> Status {
        let mut a = CallArgs {
            op: op.0,
            _pad: 0,
            args: args as u64,
            len: len as u64,
        };
        nt_to_status(RawLink::call(self, OP_PROTO_CALL, (&raw mut a).cast()))
    }
}

impl RawLink for WineUnixlib {
    // `RawLink::call` is the safe face of an FFI boundary: the pointer is only
    // handed on, never read here, and the trait cannot be `unsafe` per call.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    #[inline(always)]
    fn call(&self, op: u32, args: *mut c_void) -> i32 {
        // SAFETY: the caller passes the argument block `op` documents.
        unsafe { (self.dispatcher)(self.handle, op, args) }
    }
}

/// The export that makes the cdylib a loadable Wine anchor. Nothing calls it.
#[unsafe(no_mangle)]
pub extern "system" fn victory_metal_bridge_anchor() -> u32 {
    1
}
