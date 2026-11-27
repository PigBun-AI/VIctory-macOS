//! HRESULT values the front returns. Plain `i32` newtype: no `windows` crate.

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hr(pub i32);

impl std::fmt::Debug for Hr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Hr({:#010x})", self.0 as u32)
    }
}

impl Hr {
    pub const S_OK: Hr = Hr(0);
    pub const E_NOTIMPL: Hr = Hr(0x8000_4001u32 as i32);
    pub const E_NOINTERFACE: Hr = Hr(0x8000_4002u32 as i32);
    pub const E_POINTER: Hr = Hr(0x8000_4003u32 as i32);
    pub const E_FAIL: Hr = Hr(0x8000_4005u32 as i32);
    pub const E_OUTOFMEMORY: Hr = Hr(0x8007_000Eu32 as i32);
    pub const E_INVALIDARG: Hr = Hr(0x8007_0057u32 as i32);
    pub const DXGI_ERROR_INVALID_CALL: Hr = Hr(0x887A_0001u32 as i32);
    pub const DXGI_ERROR_NOT_FOUND: Hr = Hr(0x887A_0002u32 as i32);
    pub const DXGI_ERROR_UNSUPPORTED: Hr = Hr(0x887A_0004u32 as i32);
    pub const DXGI_ERROR_DEVICE_HUNG: Hr = Hr(0x887A_0006u32 as i32);
    pub const DXGI_ERROR_DEVICE_REMOVED: Hr = Hr(0x887A_0005u32 as i32);

    pub fn is_ok(self) -> bool {
        self.0 >= 0
    }
}

pub type Result<T> = std::result::Result<T, Hr>;
