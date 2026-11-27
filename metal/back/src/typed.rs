//! Typed buffer views: a buffer read or written through a format.
//!
//! A shader converted by Metal Shader Converter reaches a typed buffer
//! (`Buffer<T>`, `RWBuffer<T>`) through a **texture buffer**: a
//! `MTLTextureTypeTextureBuffer` texture that shares the storage of the
//! `MTLBuffer`. The descriptor-table entry of such a view holds the texture's
//! `gpuResourceID` in `textureViewID`, the buffer address in `gpuVA`, and in
//! `metadata` the length in bytes, the *offset in elements* (below) and the
//! typed flag (header `metal_irconverter_runtime.h`, `IRBufferView`,
//! `IRDescriptorTableSetBufferView`; checked 2026-11-27 against MSC 4.0.3).
//!
//! # The format table
//!
//! [`typed_format`] maps a DXIL/DXGI format number (the proto carries it as
//! a `u32`; layering rule 2 puts the table here) to the `MTLPixelFormat` of
//! the texture buffer and the element size. Formats Metal has no pixel format
//! for are not in the table and the view is refused (`Unsupported`): the
//! three-channel 32-bit formats (`R32G32B32_FLOAT/UINT/SINT`: 6, 7, 8) and
//! all typeless, depth and block-compressed formats.
//!
//! # Alignment and offsets
//!
//! A texture buffer must start at a multiple of the device's alignment for
//! its pixel format (`minimumLinearTextureAlignmentForPixelFormat:` and
//! `minimumTextureBufferAlignmentForPixelFormat:`; [`texture_alignment`]
//! takes the larger; 16 bytes for every format of the table on the Apple M5
//! Max, 2026-11-27). Metal does not refuse a smaller offset when no
//! validation layer runs, but the GPU then reads from the aligned address
//! below it (measured: a view at byte 12 read element 0 of the buffer).
//!
//! So a view must start at an aligned byte offset: `FirstElement * element
//! size` a multiple of the alignment, else it is refused with
//! [`Error::Unsupported`](crate::Error::Unsupported) and nothing is written
//! (the status is `UNSUPPORTED`, the text is in `Executor::last_error`). The
//! descriptor has a field for "elements to skip in front of the texture"
//! (`textureViewOffsetInElements`), but MSC 4.0.3 does not apply it to typed
//! reads and writes (measured, typed flag set and clear: indexing still starts
//! at the texture's first element), so a padded texture would give wrong data
//! and there is no fallback that keeps the same memory. D3D12 asks nothing of
//! the kind of a typed view, so a game that slices one buffer into typed
//! views at element offsets not a multiple of 16 bytes will hit this; when one
//! does, the answer is a copy of the range (decided then, not now).
//!
//! The texture's width is `NumElements`; the maximum Metal accepts is large
//! (measured: 2^28 elements of `R32Uint`), more than D3D12's 2^27 limit.
//!
//! # Lifetime
//!
//! The texture objects of a buffer are kept in the buffer ([`Buffer::texture_view`]
//! caches them by format, offset, length and access) and retired with it, in
//! the same residency batch and after the same GPU work: a descriptor in a
//! heap never owns anything, as in the original API, and it is valid as long
//! as the resource is.

use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLDevice, MTLPixelFormat, MTLTextureUsage};

/// How a typed buffer is read and written: the Metal pixel format of the
/// texture buffer, the size of one element, whether the format has 32-bit
/// integer atomics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypedFormat {
    /// DXGI format number this was made for.
    pub dxgi: u32,
    /// The pixel format of the texture buffer.
    pub pixel: MTLPixelFormat,
    /// Bytes per element.
    pub bytes: u32,
    /// `R32_UINT` / `R32_SINT`: the texture needs `ShaderAtomic` when written
    /// (`InterlockedAdd` and friends on a typed UAV).
    pub atomic: bool,
}

/// DXGI format numbers the table knows.
pub mod dxgi {
    pub const R32G32B32A32_FLOAT: u32 = 2;
    pub const R32G32B32A32_UINT: u32 = 3;
    pub const R32G32B32A32_SINT: u32 = 4;
    pub const R16G16B16A16_FLOAT: u32 = 10;
    pub const R16G16B16A16_UNORM: u32 = 11;
    pub const R16G16B16A16_UINT: u32 = 12;
    pub const R16G16B16A16_SNORM: u32 = 13;
    pub const R16G16B16A16_SINT: u32 = 14;
    pub const R32G32_FLOAT: u32 = 16;
    pub const R32G32_UINT: u32 = 17;
    pub const R32G32_SINT: u32 = 18;
    pub const R10G10B10A2_UNORM: u32 = 24;
    pub const R10G10B10A2_UINT: u32 = 25;
    pub const R11G11B10_FLOAT: u32 = 26;
    pub const R8G8B8A8_UNORM: u32 = 28;
    pub const R8G8B8A8_UNORM_SRGB: u32 = 29;
    pub const R8G8B8A8_UINT: u32 = 30;
    pub const R8G8B8A8_SNORM: u32 = 31;
    pub const R8G8B8A8_SINT: u32 = 32;
    pub const R16G16_FLOAT: u32 = 34;
    pub const R16G16_UNORM: u32 = 35;
    pub const R16G16_UINT: u32 = 36;
    pub const R16G16_SNORM: u32 = 37;
    pub const R16G16_SINT: u32 = 38;
    pub const R32_FLOAT: u32 = 41;
    pub const R32_UINT: u32 = 42;
    pub const R32_SINT: u32 = 43;
    pub const R8G8_UNORM: u32 = 49;
    pub const R8G8_UINT: u32 = 50;
    pub const R8G8_SNORM: u32 = 51;
    pub const R8G8_SINT: u32 = 52;
    pub const R16_FLOAT: u32 = 54;
    pub const R16_UNORM: u32 = 56;
    pub const R16_UINT: u32 = 57;
    pub const R16_SNORM: u32 = 58;
    pub const R16_SINT: u32 = 59;
    pub const R8_UNORM: u32 = 61;
    pub const R8_UINT: u32 = 62;
    pub const R8_SNORM: u32 = 63;
    pub const R8_SINT: u32 = 64;
    pub const A8_UNORM: u32 = 65;
    pub const R9G9B9E5_SHAREDEXP: u32 = 67;
    pub const B8G8R8A8_UNORM: u32 = 87;
    pub const B8G8R8A8_UNORM_SRGB: u32 = 91;
}

/// Every format of the table, in DXGI order (for tests and statistics).
pub const TYPED_FORMATS: &[u32] = &[
    dxgi::R32G32B32A32_FLOAT,
    dxgi::R32G32B32A32_UINT,
    dxgi::R32G32B32A32_SINT,
    dxgi::R16G16B16A16_FLOAT,
    dxgi::R16G16B16A16_UNORM,
    dxgi::R16G16B16A16_UINT,
    dxgi::R16G16B16A16_SNORM,
    dxgi::R16G16B16A16_SINT,
    dxgi::R32G32_FLOAT,
    dxgi::R32G32_UINT,
    dxgi::R32G32_SINT,
    dxgi::R10G10B10A2_UNORM,
    dxgi::R10G10B10A2_UINT,
    dxgi::R11G11B10_FLOAT,
    dxgi::R8G8B8A8_UNORM,
    dxgi::R8G8B8A8_UNORM_SRGB,
    dxgi::R8G8B8A8_UINT,
    dxgi::R8G8B8A8_SNORM,
    dxgi::R8G8B8A8_SINT,
    dxgi::R16G16_FLOAT,
    dxgi::R16G16_UNORM,
    dxgi::R16G16_UINT,
    dxgi::R16G16_SNORM,
    dxgi::R16G16_SINT,
    dxgi::R32_FLOAT,
    dxgi::R32_UINT,
    dxgi::R32_SINT,
    dxgi::R8G8_UNORM,
    dxgi::R8G8_UINT,
    dxgi::R8G8_SNORM,
    dxgi::R8G8_SINT,
    dxgi::R16_FLOAT,
    dxgi::R16_UNORM,
    dxgi::R16_UINT,
    dxgi::R16_SNORM,
    dxgi::R16_SINT,
    dxgi::R8_UNORM,
    dxgi::R8_UINT,
    dxgi::R8_SNORM,
    dxgi::R8_SINT,
    dxgi::A8_UNORM,
    dxgi::R9G9B9E5_SHAREDEXP,
    dxgi::B8G8R8A8_UNORM,
    dxgi::B8G8R8A8_UNORM_SRGB,
];

/// The texture-buffer format of a DXGI format number, or `None` when Metal
/// has no equivalent (see the module comment).
pub fn typed_format(dxgi: u32) -> Option<TypedFormat> {
    use MTLPixelFormat as P;
    let (pixel, bytes) = match dxgi {
        dxgi::R32G32B32A32_FLOAT => (P::RGBA32Float, 16),
        dxgi::R32G32B32A32_UINT => (P::RGBA32Uint, 16),
        dxgi::R32G32B32A32_SINT => (P::RGBA32Sint, 16),
        dxgi::R16G16B16A16_FLOAT => (P::RGBA16Float, 8),
        dxgi::R16G16B16A16_UNORM => (P::RGBA16Unorm, 8),
        dxgi::R16G16B16A16_UINT => (P::RGBA16Uint, 8),
        dxgi::R16G16B16A16_SNORM => (P::RGBA16Snorm, 8),
        dxgi::R16G16B16A16_SINT => (P::RGBA16Sint, 8),
        dxgi::R32G32_FLOAT => (P::RG32Float, 8),
        dxgi::R32G32_UINT => (P::RG32Uint, 8),
        dxgi::R32G32_SINT => (P::RG32Sint, 8),
        dxgi::R10G10B10A2_UNORM => (P::RGB10A2Unorm, 4),
        dxgi::R10G10B10A2_UINT => (P::RGB10A2Uint, 4),
        dxgi::R11G11B10_FLOAT => (P::RG11B10Float, 4),
        dxgi::R8G8B8A8_UNORM => (P::RGBA8Unorm, 4),
        dxgi::R8G8B8A8_UNORM_SRGB => (P::RGBA8Unorm_sRGB, 4),
        dxgi::R8G8B8A8_UINT => (P::RGBA8Uint, 4),
        dxgi::R8G8B8A8_SNORM => (P::RGBA8Snorm, 4),
        dxgi::R8G8B8A8_SINT => (P::RGBA8Sint, 4),
        dxgi::R16G16_FLOAT => (P::RG16Float, 4),
        dxgi::R16G16_UNORM => (P::RG16Unorm, 4),
        dxgi::R16G16_UINT => (P::RG16Uint, 4),
        dxgi::R16G16_SNORM => (P::RG16Snorm, 4),
        dxgi::R16G16_SINT => (P::RG16Sint, 4),
        dxgi::R32_FLOAT => (P::R32Float, 4),
        dxgi::R32_UINT => (P::R32Uint, 4),
        dxgi::R32_SINT => (P::R32Sint, 4),
        dxgi::R8G8_UNORM => (P::RG8Unorm, 2),
        dxgi::R8G8_UINT => (P::RG8Uint, 2),
        dxgi::R8G8_SNORM => (P::RG8Snorm, 2),
        dxgi::R8G8_SINT => (P::RG8Sint, 2),
        dxgi::R16_FLOAT => (P::R16Float, 2),
        dxgi::R16_UNORM => (P::R16Unorm, 2),
        dxgi::R16_UINT => (P::R16Uint, 2),
        dxgi::R16_SNORM => (P::R16Snorm, 2),
        dxgi::R16_SINT => (P::R16Sint, 2),
        dxgi::R8_UNORM => (P::R8Unorm, 1),
        dxgi::R8_UINT => (P::R8Uint, 1),
        dxgi::R8_SNORM => (P::R8Snorm, 1),
        dxgi::R8_SINT => (P::R8Sint, 1),
        dxgi::A8_UNORM => (P::A8Unorm, 1),
        dxgi::R9G9B9E5_SHAREDEXP => (P::RGB9E5Float, 4),
        dxgi::B8G8R8A8_UNORM => (P::BGRA8Unorm, 4),
        dxgi::B8G8R8A8_UNORM_SRGB => (P::BGRA8Unorm_sRGB, 4),
        _ => return None,
    };
    Some(TypedFormat {
        dxgi,
        pixel,
        bytes,
        atomic: matches!(dxgi, dxgi::R32_UINT | dxgi::R32_SINT),
    })
}

/// How a texture view may be used: what the descriptor kind asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ViewAccess {
    /// SRV: `ShaderRead`.
    Read,
    /// UAV: `ShaderRead | ShaderWrite` (and `ShaderAtomic` for 32-bit integer
    /// formats).
    ReadWrite,
}

impl ViewAccess {
    pub(crate) fn usage(self, format: &TypedFormat) -> MTLTextureUsage {
        match self {
            ViewAccess::Read => MTLTextureUsage::ShaderRead,
            ViewAccess::ReadWrite => {
                let mut u = MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite;
                if format.atomic {
                    u |= MTLTextureUsage::ShaderAtomic;
                }
                u
            }
        }
    }
}

/// The 64-bit value of a `MTLResourceID` (`struct { uint64_t _impl; }` in
/// Metal's header; objc2 keeps the field private).
pub fn resource_id_of(id: objc2_metal::MTLResourceID) -> u64 {
    const _: () = assert!(std::mem::size_of::<objc2_metal::MTLResourceID>() == 8);
    // SAFETY: same size (asserted), a plain integer inside.
    unsafe { std::mem::transmute::<objc2_metal::MTLResourceID, u64>(id) }
}

/// The alignment of a texture buffer of this pixel format: the larger of
/// `minimumLinearTextureAlignmentForPixelFormat:` and
/// `minimumTextureBufferAlignmentForPixelFormat:`.
pub fn texture_alignment(device: &ProtocolObject<dyn MTLDevice>, pixel: MTLPixelFormat) -> u64 {
    let linear = device.minimumLinearTextureAlignmentForPixelFormat(pixel) as u64;
    let buffer = device.minimumTextureBufferAlignmentForPixelFormat(pixel) as u64;
    linear.max(buffer).max(1).next_power_of_two()
}

impl crate::device::Device {
    /// The byte alignment a typed view of `format` needs to start at without
    /// padding (see [`texture_alignment`]).
    pub fn typed_view_alignment(&self, format: &TypedFormat) -> u64 {
        texture_alignment(self.raw(), format.pixel)
    }
}

/// Checks that a view of `elements` elements of `bytes_per_element` bytes at
/// byte `first_byte` can be a texture buffer given the device's `alignment`
/// for its format: not empty, not overflowing, aligned
/// ([`Error::Unsupported`](crate::Error::Unsupported) when it is not).
pub fn check_view(
    first_byte: u64,
    elements: u64,
    bytes_per_element: u64,
    alignment: u64,
) -> crate::Result<()> {
    use crate::Error;
    if elements == 0
        || bytes_per_element == 0
        || !alignment.is_power_of_two()
        || elements.checked_mul(bytes_per_element).is_none()
    {
        return Err(Error::BadArgument(format!(
            "typed view of {elements} elements of {bytes_per_element} bytes, alignment {alignment}"
        )));
    }
    if !first_byte.is_multiple_of(alignment) {
        return Err(Error::Unsupported(format!(
            "a typed view starting at byte {first_byte} is not a multiple of the {alignment}-byte \
             alignment of a Metal texture buffer, and the shader converter cannot offset typed reads"
        )));
    }
    Ok(())
}

/// A texture buffer over a buffer, as a descriptor needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferTextureView {
    /// `MTLTexture.gpuResourceID` (the descriptor's `textureViewID`).
    pub resource_id: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_has_sizes_that_match_the_names() {
        for &f in TYPED_FORMATS {
            let t = typed_format(f).unwrap_or_else(|| panic!("format {f}"));
            assert_eq!(t.dxgi, f);
            assert!(matches!(t.bytes, 1 | 2 | 4 | 8 | 16), "{f}: {}", t.bytes);
        }
        let t = |f| typed_format(f).unwrap();
        assert_eq!(t(dxgi::R32G32B32A32_UINT).bytes, 16);
        assert_eq!(t(dxgi::R16G16B16A16_UINT).bytes, 8);
        assert_eq!(t(dxgi::R16_UINT).bytes, 2);
        assert_eq!(t(dxgi::R32_FLOAT).bytes, 4);
        assert!(t(dxgi::R32_UINT).atomic && t(dxgi::R32_SINT).atomic);
        assert!(!t(dxgi::R32_FLOAT).atomic && !t(dxgi::R16_UINT).atomic);
    }

    #[test]
    fn no_format_is_listed_twice_and_unknown_ones_are_refused() {
        let mut seen = std::collections::HashSet::new();
        assert!(TYPED_FORMATS.iter().all(|f| seen.insert(*f)));
        // Three 32-bit channels: Metal has no such pixel format.
        for f in [6, 7, 8] {
            assert!(typed_format(f).is_none(), "{f}");
        }
        // Typeless, UNKNOWN, depth, block-compressed.
        for f in [0, 1, 5, 15, 27, 39, 40, 53, 60, 71, 80] {
            assert!(typed_format(f).is_none(), "{f}");
        }
    }

    #[test]
    fn an_aligned_view_is_accepted() {
        assert!(check_view(256, 10, 4, 16).is_ok());
        assert!(check_view(0, 1, 16, 16).is_ok());
    }

    #[test]
    fn a_misaligned_view_is_unsupported() {
        use crate::Error;
        assert!(matches!(
            check_view(12, 5, 4, 16),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            check_view(1, 5, 1, 16),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn an_empty_or_overflowing_view_is_a_bad_argument() {
        use crate::Error;
        assert!(matches!(
            check_view(0, 0, 4, 16),
            Err(Error::BadArgument(_))
        ));
        assert!(matches!(
            check_view(0, 4, 4, 12),
            Err(Error::BadArgument(_))
        ));
        assert!(matches!(
            check_view(0, u64::MAX, 4, 16),
            Err(Error::BadArgument(_))
        ));
    }
}
