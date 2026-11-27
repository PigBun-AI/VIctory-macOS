//! Memory the way the title gets it: `VirtualAlloc` and `MapViewOfFile3`
//! under Wine; the closest thing (`mmap`) when run natively as a baseline.

use std::ptr::NonNull;

/// Metal wants no-copy memory aligned to its 16 KiB page.
pub const METAL_PAGE: usize = 16 * 1024;

/// A mapped region, unmapped on drop.
pub struct Region {
    pub ptr: NonNull<u8>,
    pub len: usize,
    release: Option<Box<dyn FnOnce()>>,
}

impl Region {
    pub fn words(&mut self) -> &mut [u32] {
        // SAFETY: `len` bytes of live, writable, 4-byte aligned memory.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr().cast(), self.len / 4) }
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release();
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, UnmapViewOfFile,
        VirtualAlloc, VirtualFree,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    pub const NAME: &str = "VirtualAlloc";
    pub const VIEW_NAME: &str = "MapViewOfFile3";

    pub fn alloc_private(len: usize) -> Option<Region> {
        // SAFETY: a fresh anonymous allocation.
        let base = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                len,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
        };
        let ptr = NonNull::new(base.cast::<u8>())?;
        let addr = ptr.as_ptr() as usize;
        Some(Region {
            ptr,
            len,
            // SAFETY: frees exactly what was allocated.
            release: Some(Box::new(move || unsafe {
                VirtualFree(addr as *mut c_void, 0, MEM_RELEASE);
            })),
        })
    }

    type MapViewOfFile3 = unsafe extern "system" fn(
        file_mapping: *mut c_void,
        process: *mut c_void,
        base: *mut c_void,
        offset: u64,
        view_size: usize,
        allocation_type: u32,
        page_protection: u32,
        extended: *mut c_void,
        extended_count: u32,
    ) -> *mut c_void;

    /// A pagefile-backed section mapped with `MapViewOfFile3`.
    pub fn alloc_view(len: usize) -> Result<Region, String> {
        let kernelbase: Vec<u16> = "kernelbase.dll\0".encode_utf16().collect();
        // SAFETY: Win32 calls with valid arguments; the function pointer type
        // is the documented `MapViewOfFile3` signature.
        unsafe {
            let module = GetModuleHandleW(kernelbase.as_ptr());
            let export = GetProcAddress(module, c"MapViewOfFile3".as_ptr().cast())
                .ok_or("kernelbase.dll has no MapViewOfFile3")?;
            let map: MapViewOfFile3 = std::mem::transmute(export);
            let section = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE,
                (len as u64 >> 32) as u32,
                len as u32,
                std::ptr::null(),
            );
            if section.is_null() {
                return Err("CreateFileMappingW failed".to_owned());
            }
            let base = map(
                section,
                GetCurrentProcess(),
                std::ptr::null_mut(),
                0,
                0,
                0,
                PAGE_READWRITE,
                std::ptr::null_mut(),
                0,
            );
            // The view keeps the section alive.
            CloseHandle(section);
            let ptr = NonNull::new(base.cast::<u8>()).ok_or("MapViewOfFile3 returned NULL")?;
            let addr = ptr.as_ptr() as usize;
            Ok(Region {
                ptr,
                len,
                release: Some(Box::new(move || {
                    UnmapViewOfFile(
                        windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS {
                            Value: addr as *mut c_void,
                        },
                    );
                })),
            })
        }
    }
}

#[cfg(target_vendor = "apple")]
mod imp {
    use super::*;

    pub const NAME: &str = "mmap(MAP_PRIVATE|MAP_ANON)";
    pub const VIEW_NAME: &str = "mmap(MAP_SHARED|MAP_ANON)";

    fn map(len: usize, flags: libc::c_int) -> Option<Region> {
        let total = len + METAL_PAGE;
        // SAFETY: a fresh anonymous mapping, unmapped as a whole on drop.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                total,
                libc::PROT_READ | libc::PROT_WRITE,
                flags | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return None;
        }
        let aligned = (base as usize).next_multiple_of(METAL_PAGE);
        let base = base as usize;
        Some(Region {
            ptr: NonNull::new(aligned as *mut u8)?,
            len,
            // SAFETY: unmaps what `mmap` returned.
            release: Some(Box::new(move || unsafe {
                libc::munmap(base as *mut libc::c_void, total);
            })),
        })
    }

    pub fn alloc_private(len: usize) -> Option<Region> {
        map(len, libc::MAP_PRIVATE)
    }

    pub fn alloc_view(len: usize) -> Result<Region, String> {
        map(len, libc::MAP_SHARED).ok_or_else(|| "mmap failed".to_owned())
    }
}

pub use imp::{NAME as PRIVATE_NAME, VIEW_NAME, alloc_private, alloc_view};
