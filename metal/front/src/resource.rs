//! Resource descriptions and the argument rules of resource creation.
//! Field values are the D3D12 enum numbers.

use crate::format;
use crate::hresult::{Hr, Result};

pub const DIM_BUFFER: u32 = 1;
pub const DIM_TEXTURE1D: u32 = 2;
pub const DIM_TEXTURE2D: u32 = 3;
pub const DIM_TEXTURE3D: u32 = 4;

pub const LAYOUT_UNKNOWN: u32 = 0;
pub const LAYOUT_ROW_MAJOR: u32 = 1;

pub const FLAG_ALLOW_RENDER_TARGET: u32 = 0x1;
pub const FLAG_ALLOW_DEPTH_STENCIL: u32 = 0x2;
pub const FLAG_ALLOW_UNORDERED_ACCESS: u32 = 0x4;
pub const FLAG_DENY_SHADER_RESOURCE: u32 = 0x8;

pub const HEAP_DEFAULT: u32 = 1;
pub const HEAP_UPLOAD: u32 = 2;
pub const HEAP_READBACK: u32 = 3;
pub const HEAP_CUSTOM: u32 = 4;

/// `D3D12_RESOURCE_DESC`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceDesc {
    pub dimension: u32,
    pub alignment: u64,
    pub width: u64,
    pub height: u32,
    pub depth_or_array: u16,
    pub mips: u16,
    pub format: u32,
    pub sample_count: u32,
    pub sample_quality: u32,
    pub layout: u32,
    pub flags: u32,
}

impl ResourceDesc {
    pub fn buffer(size: u64, flags: u32) -> Self {
        ResourceDesc {
            dimension: DIM_BUFFER,
            alignment: 0,
            width: size,
            height: 1,
            depth_or_array: 1,
            mips: 1,
            format: format::UNKNOWN,
            sample_count: 1,
            sample_quality: 0,
            layout: LAYOUT_ROW_MAJOR,
            flags,
        }
    }

    pub fn is_buffer(&self) -> bool {
        self.dimension == DIM_BUFFER
    }

    /// Array size (1 for 3D textures and buffers).
    pub fn array_size(&self) -> u32 {
        if self.dimension == DIM_TEXTURE3D || self.is_buffer() { 1 } else { self.depth_or_array as u32 }
    }

    pub fn depth(&self) -> u32 {
        if self.dimension == DIM_TEXTURE3D { self.depth_or_array as u32 } else { 1 }
    }

    pub fn planes(&self) -> u32 {
        format::info(self.format).map_or(1, |i| i.planes as u32)
    }

    /// `D3D12GetFormatPlaneCount` x mips x array size; 1 for a buffer.
    pub fn subresource_count(&self) -> u32 {
        if self.is_buffer() { 1 } else { self.mips as u32 * self.array_size() * self.planes() }
    }
}

/// Default placement alignment of a resource.
pub const DEFAULT_ALIGNMENT: u64 = 64 * 1024;
pub const MSAA_ALIGNMENT: u64 = 4 * 1024 * 1024;

// Resource states used by the creation rules.
pub const STATE_COMMON: u32 = 0;
pub const STATE_COPY_DEST: u32 = 0x400;
pub const STATE_GENERIC_READ: u32 = 0xAC3;

/// The rules `CreateCommittedResource` enforces on its arguments (checked
/// against D3DMetal on Wine: the upload/readback state rules, zero size,
/// descriptor sanity). Returns the description `GetDesc` reports.
pub fn validate_committed(heap_type: u32, desc: &ResourceDesc, initial_state: u32) -> Result<ResourceDesc> {
    let mut d = *desc;
    match heap_type {
        HEAP_DEFAULT | HEAP_UPLOAD | HEAP_READBACK | HEAP_CUSTOM => {}
        _ => return Err(Hr::E_INVALIDARG),
    }
    if d.width == 0 || d.sample_count == 0 {
        return Err(Hr::E_INVALIDARG);
    }
    match d.dimension {
        DIM_BUFFER => {
            if d.height != 1
                || d.depth_or_array != 1
                || d.mips != 1
                || d.format != format::UNKNOWN
                || d.sample_count != 1
                || d.sample_quality != 0
                || d.layout != LAYOUT_ROW_MAJOR
                || d.flags & (FLAG_ALLOW_RENDER_TARGET | FLAG_ALLOW_DEPTH_STENCIL) != 0
                || (d.alignment != 0 && d.alignment != DEFAULT_ALIGNMENT)
            {
                return Err(Hr::E_INVALIDARG);
            }
            d.alignment = DEFAULT_ALIGNMENT;
        }
        DIM_TEXTURE1D | DIM_TEXTURE2D | DIM_TEXTURE3D => {
            if d.height == 0 || d.depth_or_array == 0 || format::info(d.format).is_none() {
                return Err(Hr::E_INVALIDARG);
            }
            if d.dimension == DIM_TEXTURE1D && d.height != 1 {
                return Err(Hr::E_INVALIDARG);
            }
            if d.width > u32::MAX as u64 {
                return Err(Hr::E_INVALIDARG);
            }
            if d.mips == 0 {
                // 0 means "all mips".
                let longest = match d.dimension {
                    DIM_TEXTURE1D => d.width as u32,
                    DIM_TEXTURE2D => (d.width as u32).max(d.height),
                    _ => (d.width as u32).max(d.height).max(d.depth_or_array as u32),
                };
                d.mips = (32 - longest.leading_zeros()) as u16;
            }
            if d.alignment == 0 {
                d.alignment = if d.sample_count > 1 { MSAA_ALIGNMENT } else { DEFAULT_ALIGNMENT };
            }
        }
        _ => return Err(Hr::E_INVALIDARG),
    }
    // Upload heaps start in GENERIC_READ, readback heaps in COPY_DEST, and
    // neither can hold render targets, depth buffers or UAVs.
    match heap_type {
        HEAP_UPLOAD if initial_state != STATE_GENERIC_READ => return Err(Hr::E_INVALIDARG),
        HEAP_READBACK if initial_state != STATE_COPY_DEST => return Err(Hr::E_INVALIDARG),
        HEAP_UPLOAD | HEAP_READBACK
            if d.flags & (FLAG_ALLOW_RENDER_TARGET | FLAG_ALLOW_DEPTH_STENCIL | FLAG_ALLOW_UNORDERED_ACCESS) != 0 =>
        {
            return Err(Hr::E_INVALIDARG);
        }
        HEAP_UPLOAD | HEAP_READBACK if !d.is_buffer() => return Err(Hr::E_INVALIDARG),
        _ => {}
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_follow_the_heap_rules() {
        let b = ResourceDesc::buffer(4096, 0);
        let ok = validate_committed(HEAP_UPLOAD, &b, STATE_GENERIC_READ).unwrap();
        assert_eq!(ok.alignment, 65536);
        assert_eq!(validate_committed(HEAP_UPLOAD, &b, STATE_COPY_DEST), Err(Hr::E_INVALIDARG));
        assert!(validate_committed(HEAP_READBACK, &b, STATE_COPY_DEST).is_ok());
        assert_eq!(validate_committed(HEAP_READBACK, &b, STATE_GENERIC_READ), Err(Hr::E_INVALIDARG));
        assert!(validate_committed(HEAP_DEFAULT, &ResourceDesc::buffer(4096, FLAG_ALLOW_UNORDERED_ACCESS), 0).is_ok());
        assert_eq!(validate_committed(HEAP_DEFAULT, &ResourceDesc::buffer(0, 0), 0), Err(Hr::E_INVALIDARG));
        assert_eq!(
            validate_committed(HEAP_DEFAULT, &ResourceDesc::buffer(16, FLAG_ALLOW_RENDER_TARGET), 0),
            Err(Hr::E_INVALIDARG)
        );
    }

    #[test]
    fn zero_mips_means_a_full_chain() {
        let t = ResourceDesc {
            dimension: DIM_TEXTURE2D,
            alignment: 0,
            width: 100,
            height: 37,
            depth_or_array: 1,
            mips: 0,
            format: 28,
            sample_count: 1,
            sample_quality: 0,
            layout: LAYOUT_UNKNOWN,
            flags: 0,
        };
        assert_eq!(validate_committed(HEAP_DEFAULT, &t, 0).unwrap().mips, 7);
    }
}
