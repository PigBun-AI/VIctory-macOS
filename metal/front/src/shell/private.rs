//! `SetPrivateData` & co. (D3D12 and DXGI flavours of the same store).

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;

use windows::Win32::Foundation::{E_INVALIDARG, E_POINTER};
use windows::core::{Error, GUID, IUnknown, Interface, PCWSTR, Ref, Result as WResult};

use crate::hresult::Hr;

use super::werr;

enum Entry {
    Bytes(Vec<u8>),
    Interface(IUnknown),
}

/// Per-object private data. D3D12 objects also keep their debug name here
/// (`WKPDID_D3DDebugObjectNameW`).
#[derive(Default)]
pub struct PrivateData {
    map: Mutex<HashMap<u128, Entry>>,
}

/// `WKPDID_D3DDebugObjectNameW`.
const NAME_GUID: GUID = GUID::from_u128(0x4cca5fd8_921f_42c8_8566_70caf2a9b741);

fn key(g: *const GUID) -> WResult<u128> {
    if g.is_null() {
        return Err(Error::from(E_POINTER));
    }
    // SAFETY: non-null GUID pointer from the caller.
    Ok(unsafe { *g }.to_u128())
}

impl PrivateData {
    pub fn set(&self, guid: *const GUID, size: u32, data: *const c_void) -> WResult<()> {
        let k = key(guid)?;
        let mut m = self.map.lock().unwrap();
        if size == 0 && data.is_null() {
            m.remove(&k);
            return Ok(());
        }
        if data.is_null() {
            return Err(Error::from(E_INVALIDARG));
        }
        // SAFETY: the caller gives `size` readable bytes at `data`.
        let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size as usize) }.to_vec();
        m.insert(k, Entry::Bytes(bytes));
        Ok(())
    }

    pub fn set_interface(&self, guid: *const GUID, obj: Ref<IUnknown>) -> WResult<()> {
        let k = key(guid)?;
        let mut m = self.map.lock().unwrap();
        match obj.as_ref() {
            Some(o) => m.insert(k, Entry::Interface(o.clone())),
            None => m.remove(&k),
        };
        Ok(())
    }

    pub fn get(&self, guid: *const GUID, size: *mut u32, data: *mut c_void) -> WResult<()> {
        let k = key(guid)?;
        if size.is_null() {
            return Err(Error::from(E_POINTER));
        }
        let m = self.map.lock().unwrap();
        // SAFETY: non-null `size` from the caller.
        let cap = unsafe { *size } as usize;
        match m.get(&k) {
            None => {
                // SAFETY: as above.
                unsafe { *size = 0 };
                Err(werr(Hr::DXGI_ERROR_NOT_FOUND))
            }
            Some(e) => {
                let need = match e {
                    Entry::Bytes(b) => b.len(),
                    Entry::Interface(_) => std::mem::size_of::<usize>(),
                };
                // SAFETY: as above.
                unsafe { *size = need as u32 };
                if data.is_null() {
                    return Ok(());
                }
                if cap < need {
                    return Err(werr(Hr(0x887A_0003u32 as i32))); // DXGI_ERROR_MORE_DATA
                }
                match e {
                    // SAFETY: `cap >= need` bytes writable at `data`.
                    Entry::Bytes(b) => unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), data.cast::<u8>(), b.len()) },
                    Entry::Interface(i) => unsafe { data.cast::<*mut c_void>().write(i.clone().into_raw()) },
                }
                Ok(())
            }
        }
    }

    /// The debug name, if one was set.
    pub fn name(&self) -> Option<String> {
        match self.map.lock().unwrap().get(&NAME_GUID.to_u128())? {
            Entry::Bytes(b) => {
                let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                Some(String::from_utf16_lossy(&w))
            }
            _ => None,
        }
    }

    // The generated impls call these two families by name.
    pub fn d3d12_GetPrivateData(&self, guid: *const GUID, size: *mut u32, data: *mut c_void) -> WResult<()> {
        self.get(guid, size, data)
    }
    pub fn d3d12_SetPrivateData(&self, guid: *const GUID, size: u32, data: *const c_void) -> WResult<()> {
        self.set(guid, size, data)
    }
    pub fn d3d12_SetPrivateDataInterface(&self, guid: *const GUID, obj: Ref<IUnknown>) -> WResult<()> {
        self.set_interface(guid, obj)
    }
    pub fn d3d12_SetName(&self, name: &PCWSTR) -> WResult<()> {
        if name.is_null() {
            return self.set(&NAME_GUID, 0, std::ptr::null());
        }
        // SAFETY: a NUL-terminated wide string from the caller.
        let units = unsafe { name.as_wide() };
        let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
        self.set(&NAME_GUID, bytes.len() as u32, bytes.as_ptr().cast())
    }
    pub fn dxgi_GetPrivateData(&self, guid: *const GUID, size: *mut u32, data: *mut c_void) -> WResult<()> {
        self.get(guid, size, data)
    }
    pub fn dxgi_SetPrivateData(&self, guid: *const GUID, size: u32, data: *const c_void) -> WResult<()> {
        self.set(guid, size, data)
    }
    pub fn dxgi_SetPrivateDataInterface(&self, guid: *const GUID, obj: Ref<IUnknown>) -> WResult<()> {
        self.set_interface(guid, obj)
    }
}
