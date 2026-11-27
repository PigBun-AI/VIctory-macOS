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

pub const CPU_PAGE_UNKNOWN: u32 = 0;
pub const CPU_PAGE_NOT_AVAILABLE: u32 = 1;
pub const CPU_PAGE_WRITE_COMBINE: u32 = 2;
pub const CPU_PAGE_WRITE_BACK: u32 = 3;
pub const POOL_UNKNOWN: u32 = 0;
pub const POOL_L0: u32 = 1;
pub const POOL_L1: u32 = 2;

/// `D3D12_HEAP_PROPERTIES` (the node masks are 0 or 1 and carry nothing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeapProps {
    pub ty: u32,
    pub cpu_page: u32,
    pub pool: u32,
}

impl HeapProps {
    pub const fn of_type(ty: u32) -> Self {
        HeapProps {
            ty,
            cpu_page: CPU_PAGE_UNKNOWN,
            pool: POOL_UNKNOWN,
        }
    }
}

/// What memory a heap (or a committed resource) is made of, in the terms the
/// back has.
///
/// The machine has one memory (unified): the original API's pools L0
/// (system memory) and L1 (video memory) are the same bytes. A custom heap
/// is therefore mapped by its CPU access alone, and `L1` is treated as `L0`:
///
/// | custom heap | memory |
/// |---|---|
/// | `NOT_AVAILABLE` (any pool) | `Gpu`: private storage, no CPU pointer |
/// | `WRITE_COMBINE` / `WRITE_BACK`, pool `L0` or `L1` | `Shared`: CPU-visible, cached (write-combining is not kept; `WRITE_COMBINE` reads are just fast) |
///
/// An `UPLOAD` heap is `Upload` (write-combined) and `READBACK` is `Readback`
/// (cached). A texture cannot live in CPU-visible memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Memory {
    Gpu,
    Upload,
    Readback,
    Shared,
}

impl Memory {
    pub fn cpu_visible(self) -> bool {
        self != Memory::Gpu
    }
}

impl HeapProps {
    /// The memory these properties ask for, or `E_INVALIDARG` for a custom
    /// heap the original API itself refuses (properties left unknown).
    pub fn memory(&self) -> Result<Memory> {
        match self.ty {
            HEAP_DEFAULT => Ok(Memory::Gpu),
            HEAP_UPLOAD => Ok(Memory::Upload),
            HEAP_READBACK => Ok(Memory::Readback),
            HEAP_CUSTOM => {
                if !(1..=2).contains(&self.pool) {
                    return Err(Hr::E_INVALIDARG);
                }
                match self.cpu_page {
                    CPU_PAGE_NOT_AVAILABLE => Ok(Memory::Gpu),
                    CPU_PAGE_WRITE_COMBINE | CPU_PAGE_WRITE_BACK => Ok(Memory::Shared),
                    _ => Err(Hr::E_INVALIDARG),
                }
            }
            _ => Err(Hr::E_INVALIDARG),
        }
    }
}

/// `ID3D12Device::GetCustomHeapProperties`: the custom properties that
/// describe `ty` on this machine (unified memory, cache coherent: everything
/// is pool `L0`, the CPU sees `UPLOAD` and `READBACK` memory, and `DEFAULT`
/// has no CPU access). The answer agrees with `CheckFeatureSupport(ARCHITECTURE)`
/// reporting UMA and CacheCoherentUMA.
pub fn custom_heap_properties(ty: u32, unified: bool) -> Result<HeapProps> {
    let (cpu_page, pool) = match ty {
        HEAP_DEFAULT if unified => (CPU_PAGE_NOT_AVAILABLE, POOL_L0),
        HEAP_DEFAULT => (CPU_PAGE_NOT_AVAILABLE, POOL_L1),
        HEAP_UPLOAD => (CPU_PAGE_WRITE_COMBINE, POOL_L0),
        HEAP_READBACK => (CPU_PAGE_WRITE_BACK, POOL_L0),
        _ => return Err(Hr::E_INVALIDARG),
    };
    Ok(HeapProps {
        ty: HEAP_CUSTOM,
        cpu_page,
        pool,
    })
}

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

    /// `CREATE_TEXTURE` usage bits: a texture can be read by shaders unless
    /// the description denies it.
    pub fn texture_usage(&self) -> u32 {
        use victory_metal_proto::types::tex_usage;
        let mut u = 0;
        if self.flags & FLAG_DENY_SHADER_RESOURCE == 0 {
            u |= tex_usage::SRV;
        }
        if self.flags & FLAG_ALLOW_UNORDERED_ACCESS != 0 {
            u |= tex_usage::UAV;
        }
        if self.flags & FLAG_ALLOW_RENDER_TARGET != 0 {
            u |= tex_usage::RENDER_TARGET;
        }
        if self.flags & FLAG_ALLOW_DEPTH_STENCIL != 0 {
            u |= tex_usage::DEPTH_STENCIL;
        }
        u
    }

    /// Array size (1 for 3D textures and buffers).
    pub fn array_size(&self) -> u32 {
        if self.dimension == DIM_TEXTURE3D || self.is_buffer() {
            1
        } else {
            self.depth_or_array as u32
        }
    }

    pub fn depth(&self) -> u32 {
        if self.dimension == DIM_TEXTURE3D {
            self.depth_or_array as u32
        } else {
            1
        }
    }

    pub fn planes(&self) -> u32 {
        format::info(self.format).map_or(1, |i| i.planes as u32)
    }

    /// `D3D12GetFormatPlaneCount` x mips x array size; 1 for a buffer.
    pub fn subresource_count(&self) -> u32 {
        if self.is_buffer() {
            1
        } else {
            self.mips as u32 * self.array_size() * self.planes()
        }
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
pub fn validate_committed(
    heap_type: u32,
    desc: &ResourceDesc,
    initial_state: u32,
) -> Result<ResourceDesc> {
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
                d.alignment = if d.sample_count > 1 {
                    MSAA_ALIGNMENT
                } else {
                    DEFAULT_ALIGNMENT
                };
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
            if d.flags
                & (FLAG_ALLOW_RENDER_TARGET
                    | FLAG_ALLOW_DEPTH_STENCIL
                    | FLAG_ALLOW_UNORDERED_ACCESS)
                != 0 =>
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
        assert_eq!(
            validate_committed(HEAP_UPLOAD, &b, STATE_COPY_DEST),
            Err(Hr::E_INVALIDARG)
        );
        assert!(validate_committed(HEAP_READBACK, &b, STATE_COPY_DEST).is_ok());
        assert_eq!(
            validate_committed(HEAP_READBACK, &b, STATE_GENERIC_READ),
            Err(Hr::E_INVALIDARG)
        );
        assert!(
            validate_committed(
                HEAP_DEFAULT,
                &ResourceDesc::buffer(4096, FLAG_ALLOW_UNORDERED_ACCESS),
                0
            )
            .is_ok()
        );
        assert_eq!(
            validate_committed(HEAP_DEFAULT, &ResourceDesc::buffer(0, 0), 0),
            Err(Hr::E_INVALIDARG)
        );
        assert_eq!(
            validate_committed(
                HEAP_DEFAULT,
                &ResourceDesc::buffer(16, FLAG_ALLOW_RENDER_TARGET),
                0
            ),
            Err(Hr::E_INVALIDARG)
        );
    }

    #[test]
    fn custom_heaps_map_to_memory_by_cpu_access() {
        let c = |cpu_page, pool| HeapProps {
            ty: HEAP_CUSTOM,
            cpu_page,
            pool,
        };
        assert_eq!(c(CPU_PAGE_NOT_AVAILABLE, POOL_L1).memory(), Ok(Memory::Gpu));
        assert_eq!(c(CPU_PAGE_NOT_AVAILABLE, POOL_L0).memory(), Ok(Memory::Gpu));
        for page in [CPU_PAGE_WRITE_BACK, CPU_PAGE_WRITE_COMBINE] {
            for pool in [POOL_L0, POOL_L1] {
                assert_eq!(c(page, pool).memory(), Ok(Memory::Shared));
            }
        }
        // Unknown properties of a custom heap are refused, as the API does.
        assert_eq!(c(CPU_PAGE_UNKNOWN, POOL_L0).memory(), Err(Hr::E_INVALIDARG));
        assert_eq!(
            c(CPU_PAGE_WRITE_BACK, POOL_UNKNOWN).memory(),
            Err(Hr::E_INVALIDARG)
        );
        assert_eq!(c(9, POOL_L0).memory(), Err(Hr::E_INVALIDARG));
        assert_eq!(HeapProps::of_type(HEAP_UPLOAD).memory(), Ok(Memory::Upload));
        assert_eq!(
            HeapProps::of_type(HEAP_READBACK).memory(),
            Ok(Memory::Readback)
        );
        assert_eq!(HeapProps::of_type(HEAP_DEFAULT).memory(), Ok(Memory::Gpu));
        assert_eq!(HeapProps::of_type(7).memory(), Err(Hr::E_INVALIDARG));
    }

    #[test]
    fn custom_heap_properties_agree_with_the_unified_architecture() {
        let up = custom_heap_properties(HEAP_UPLOAD, true).unwrap();
        assert_eq!(up.memory(), Ok(Memory::Shared));
        assert_eq!((up.cpu_page, up.pool), (CPU_PAGE_WRITE_COMBINE, POOL_L0));
        let rb = custom_heap_properties(HEAP_READBACK, true).unwrap();
        assert_eq!((rb.cpu_page, rb.pool), (CPU_PAGE_WRITE_BACK, POOL_L0));
        let def = custom_heap_properties(HEAP_DEFAULT, true).unwrap();
        assert_eq!(def.memory(), Ok(Memory::Gpu));
        assert_eq!(
            custom_heap_properties(HEAP_CUSTOM, true),
            Err(Hr::E_INVALIDARG)
        );
    }

    #[test]
    fn texture_usage_follows_the_flags() {
        use victory_metal_proto::types::tex_usage::*;
        let mut t = ResourceDesc::buffer(1, 0);
        assert_eq!(t.texture_usage(), SRV);
        t.flags = FLAG_ALLOW_UNORDERED_ACCESS | FLAG_ALLOW_RENDER_TARGET;
        assert_eq!(t.texture_usage(), SRV | UAV | RENDER_TARGET);
        t.flags = FLAG_ALLOW_DEPTH_STENCIL | FLAG_DENY_SHADER_RESOURCE;
        assert_eq!(t.texture_usage(), DEPTH_STENCIL);
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
