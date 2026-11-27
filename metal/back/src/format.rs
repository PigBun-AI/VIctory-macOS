//! The one table from DXGI format numbers to Metal pixel formats.
//!
//! Textures, copies, views and clears all go through [`lookup`]; nothing else
//! in the back maps a format number to a `MTLPixelFormat` for a *texture*
//! (the texture-buffer table of typed buffer views in [`crate::typed`] answers
//! a narrower question and stays beside its descriptor code). The proto
//! carries formats as `u32` DXGI numbers (layering rule 2); this file is where
//! they get their meaning. Checked against objc2-metal 0.3.2 and the Metal
//! feature set tables of 2026-11-27, Apple M5 Max, macOS 26.
//!
//! # What a row says
//!
//! * `pixel`: the Metal format a texture of this number is made with and
//!   viewed through. `None` = Metal has none (see "Not covered").
//! * `bytes`, `block`: bytes per block and block size in texels (4x4 for BCn,
//!   1x1 else). Footprint arithmetic of copies is done in blocks.
//! * `family`: the typeless number of the group. Two formats of one family can
//!   be views of each other (`R8G8B8A8_UNORM` and `_SRGB`, `R32_FLOAT` and
//!   `R32_UINT`); a texture whose family has several Metal formats is made
//!   with `PixelFormatView` usage ([`Format::needs_view_usage`]).
//!
//! # Typeless formats
//!
//! A typeless texture has no format of its own in Metal. It is created with
//! the `pixel` of its typeless row (the first choice: `R8G8B8A8_TYPELESS` is
//! `RGBA8Unorm`, `R32_TYPELESS` is `R32Float`, or `Depth32Float` when the
//! texture is a depth target, see [`texture_pixel_format`]) and every
//! descriptor reinterprets it through [`view_pixel_format`], which makes a
//! texture view of the same size. A view in a format of another family is
//! refused.
//!
//! # Depth and stencil
//!
//! Apple GPUs have no 24-bit depth. `D24_UNORM_S8_UINT` and
//! `R24G8_TYPELESS` (with `R24_UNORM_X8_TYPELESS` and `X24_TYPELESS_G8_UINT`
//! views) are made as `Depth32Float_Stencil8`, as are the
//! `R32G8X24` formats. The difference is real and cannot be hidden:
//!
//! * depth values are 32-bit floats, not 24-bit fixed point, so a shader
//!   comparing a sampled depth against a constant sees the exact value, not
//!   the one rounded to 1/16777215;
//! * texels are 8 bytes (4 depth, 1 stencil, padding), not 4: the size
//!   `GET_RESOURCE_ALLOCATION_INFO` gives is the real one;
//! * a copy between a buffer and a depth/stencil texture moves floats for
//!   depth (`R32_FLOAT` footprint) and bytes for stencil (`R8_UINT`); a
//!   `R24_UNORM_X8_TYPELESS` footprint is refused, not converted.
//!
//! # Not covered
//!
//! `R32G32B32_*` (no 96-bit format in Metal), `B8G8R8X8_*` (no format with
//! an unused channel), `B4G4R4A4_UNORM` (channel order Metal lacks),
//! `R1_UNORM`, `R10G10B10_XR_BIAS_A2_UNORM`, video formats, `R8G8_B8G8` and
//! `G8R8_G8B8`, planar and the palette formats: a texture or view in one of
//! them is refused with `Unsupported`, and nothing is created.

use objc2_metal::{MTLPixelFormat, MTLTextureUsage};

use crate::error::{Error, Result};

/// DXGI format numbers used here (`DXGI_FORMAT`, values fixed since DXGI 1.0).
pub mod dxgi {
    macro_rules! formats {
        ($($name:ident = $n:expr,)*) => { $(pub const $name: u32 = $n;)* };
    }
    formats! {
        UNKNOWN = 0,
        R32G32B32A32_TYPELESS = 1, R32G32B32A32_FLOAT = 2, R32G32B32A32_UINT = 3, R32G32B32A32_SINT = 4,
        R32G32B32_TYPELESS = 5, R32G32B32_FLOAT = 6, R32G32B32_UINT = 7, R32G32B32_SINT = 8,
        R16G16B16A16_TYPELESS = 9, R16G16B16A16_FLOAT = 10, R16G16B16A16_UNORM = 11,
        R16G16B16A16_UINT = 12, R16G16B16A16_SNORM = 13, R16G16B16A16_SINT = 14,
        R32G32_TYPELESS = 15, R32G32_FLOAT = 16, R32G32_UINT = 17, R32G32_SINT = 18,
        R32G8X24_TYPELESS = 19, D32_FLOAT_S8X24_UINT = 20, R32_FLOAT_X8X24_TYPELESS = 21,
        X32_TYPELESS_G8X24_UINT = 22,
        R10G10B10A2_TYPELESS = 23, R10G10B10A2_UNORM = 24, R10G10B10A2_UINT = 25,
        R11G11B10_FLOAT = 26,
        R8G8B8A8_TYPELESS = 27, R8G8B8A8_UNORM = 28, R8G8B8A8_UNORM_SRGB = 29,
        R8G8B8A8_UINT = 30, R8G8B8A8_SNORM = 31, R8G8B8A8_SINT = 32,
        R16G16_TYPELESS = 33, R16G16_FLOAT = 34, R16G16_UNORM = 35, R16G16_UINT = 36,
        R16G16_SNORM = 37, R16G16_SINT = 38,
        R32_TYPELESS = 39, D32_FLOAT = 40, R32_FLOAT = 41, R32_UINT = 42, R32_SINT = 43,
        R24G8_TYPELESS = 44, D24_UNORM_S8_UINT = 45, R24_UNORM_X8_TYPELESS = 46,
        X24_TYPELESS_G8_UINT = 47,
        R8G8_TYPELESS = 48, R8G8_UNORM = 49, R8G8_UINT = 50, R8G8_SNORM = 51, R8G8_SINT = 52,
        R16_TYPELESS = 53, R16_FLOAT = 54, D16_UNORM = 55, R16_UNORM = 56, R16_UINT = 57,
        R16_SNORM = 58, R16_SINT = 59,
        R8_TYPELESS = 60, R8_UNORM = 61, R8_UINT = 62, R8_SNORM = 63, R8_SINT = 64,
        A8_UNORM = 65, R1_UNORM = 66, R9G9B9E5_SHAREDEXP = 67,
        BC1_TYPELESS = 70, BC1_UNORM = 71, BC1_UNORM_SRGB = 72,
        BC2_TYPELESS = 73, BC2_UNORM = 74, BC2_UNORM_SRGB = 75,
        BC3_TYPELESS = 76, BC3_UNORM = 77, BC3_UNORM_SRGB = 78,
        BC4_TYPELESS = 79, BC4_UNORM = 80, BC4_SNORM = 81,
        BC5_TYPELESS = 82, BC5_UNORM = 83, BC5_SNORM = 84,
        B5G6R5_UNORM = 85, B5G5R5A1_UNORM = 86,
        B8G8R8A8_UNORM = 87, B8G8R8X8_UNORM = 88, B8G8R8A8_TYPELESS = 90,
        B8G8R8A8_UNORM_SRGB = 91, B8G8R8X8_TYPELESS = 92, B8G8R8X8_UNORM_SRGB = 93,
        BC6H_TYPELESS = 94, BC6H_UF16 = 95, BC6H_SF16 = 96,
        BC7_TYPELESS = 97, BC7_UNORM = 98, BC7_UNORM_SRGB = 99,
        B4G4R4A4_UNORM = 115,
    }
}

use MTLPixelFormat as P;
use dxgi as d;

/// What a format number is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// A colour format with its own interpretation.
    Color,
    /// A typeless number: only views give it a meaning.
    Typeless,
    /// A depth format (`D32_FLOAT`, `D16_UNORM`).
    Depth,
    /// A depth-stencil format (`D24_UNORM_S8_UINT`, `D32_FLOAT_S8X24_UINT`).
    DepthStencil,
    /// A view of the depth plane of a depth-stencil texture
    /// (`R32_FLOAT_X8X24_TYPELESS`, `R24_UNORM_X8_TYPELESS`).
    DepthPlane,
    /// A view of the stencil plane (`X32_TYPELESS_G8X24_UINT`,
    /// `X24_TYPELESS_G8_UINT`).
    StencilPlane,
}

/// One row of the table.
#[derive(Clone, Copy, Debug)]
pub struct Format {
    /// The DXGI number.
    pub dxgi: u32,
    /// The DXGI name without the prefix.
    pub name: &'static str,
    /// The typeless number of the group the format belongs to.
    pub family: u32,
    pub class: Class,
    /// The Metal format; `None` when Metal has no equivalent.
    pub pixel: Option<MTLPixelFormat>,
    /// Bytes per block (per texel when the block is 1x1).
    pub bytes: u8,
    /// Block width and height in texels.
    pub block: (u8, u8),
}

const fn row(
    dxgi: u32,
    name: &'static str,
    family: u32,
    class: Class,
    pixel: Option<MTLPixelFormat>,
    bytes: u8,
    block: (u8, u8),
) -> Format {
    Format {
        dxgi,
        name,
        family,
        class,
        pixel,
        bytes,
        block,
    }
}

const fn c(dxgi: u32, name: &'static str, family: u32, pixel: MTLPixelFormat, bytes: u8) -> Format {
    row(dxgi, name, family, Class::Color, Some(pixel), bytes, (1, 1))
}

const fn t(dxgi: u32, name: &'static str, pixel: MTLPixelFormat, bytes: u8) -> Format {
    row(
        dxgi,
        name,
        dxgi,
        Class::Typeless,
        Some(pixel),
        bytes,
        (1, 1),
    )
}

const fn bc(
    dxgi: u32,
    name: &'static str,
    family: u32,
    class: Class,
    pixel: MTLPixelFormat,
    bytes: u8,
) -> Format {
    row(dxgi, name, family, class, Some(pixel), bytes, (4, 4))
}

const fn none(dxgi: u32, name: &'static str, bytes: u8) -> Format {
    row(dxgi, name, dxgi, Class::Color, None, bytes, (1, 1))
}

/// Every format number the back knows, in number order.
pub static FORMATS: &[Format] = &[
    t(
        d::R32G32B32A32_TYPELESS,
        "R32G32B32A32_TYPELESS",
        P::RGBA32Float,
        16,
    ),
    c(
        d::R32G32B32A32_FLOAT,
        "R32G32B32A32_FLOAT",
        d::R32G32B32A32_TYPELESS,
        P::RGBA32Float,
        16,
    ),
    c(
        d::R32G32B32A32_UINT,
        "R32G32B32A32_UINT",
        d::R32G32B32A32_TYPELESS,
        P::RGBA32Uint,
        16,
    ),
    c(
        d::R32G32B32A32_SINT,
        "R32G32B32A32_SINT",
        d::R32G32B32A32_TYPELESS,
        P::RGBA32Sint,
        16,
    ),
    none(d::R32G32B32_TYPELESS, "R32G32B32_TYPELESS", 12),
    none(d::R32G32B32_FLOAT, "R32G32B32_FLOAT", 12),
    none(d::R32G32B32_UINT, "R32G32B32_UINT", 12),
    none(d::R32G32B32_SINT, "R32G32B32_SINT", 12),
    t(
        d::R16G16B16A16_TYPELESS,
        "R16G16B16A16_TYPELESS",
        P::RGBA16Float,
        8,
    ),
    c(
        d::R16G16B16A16_FLOAT,
        "R16G16B16A16_FLOAT",
        d::R16G16B16A16_TYPELESS,
        P::RGBA16Float,
        8,
    ),
    c(
        d::R16G16B16A16_UNORM,
        "R16G16B16A16_UNORM",
        d::R16G16B16A16_TYPELESS,
        P::RGBA16Unorm,
        8,
    ),
    c(
        d::R16G16B16A16_UINT,
        "R16G16B16A16_UINT",
        d::R16G16B16A16_TYPELESS,
        P::RGBA16Uint,
        8,
    ),
    c(
        d::R16G16B16A16_SNORM,
        "R16G16B16A16_SNORM",
        d::R16G16B16A16_TYPELESS,
        P::RGBA16Snorm,
        8,
    ),
    c(
        d::R16G16B16A16_SINT,
        "R16G16B16A16_SINT",
        d::R16G16B16A16_TYPELESS,
        P::RGBA16Sint,
        8,
    ),
    t(d::R32G32_TYPELESS, "R32G32_TYPELESS", P::RG32Float, 8),
    c(
        d::R32G32_FLOAT,
        "R32G32_FLOAT",
        d::R32G32_TYPELESS,
        P::RG32Float,
        8,
    ),
    c(
        d::R32G32_UINT,
        "R32G32_UINT",
        d::R32G32_TYPELESS,
        P::RG32Uint,
        8,
    ),
    c(
        d::R32G32_SINT,
        "R32G32_SINT",
        d::R32G32_TYPELESS,
        P::RG32Sint,
        8,
    ),
    row(
        d::R32G8X24_TYPELESS,
        "R32G8X24_TYPELESS",
        d::R32G8X24_TYPELESS,
        Class::Typeless,
        Some(P::Depth32Float_Stencil8),
        8,
        (1, 1),
    ),
    row(
        d::D32_FLOAT_S8X24_UINT,
        "D32_FLOAT_S8X24_UINT",
        d::R32G8X24_TYPELESS,
        Class::DepthStencil,
        Some(P::Depth32Float_Stencil8),
        8,
        (1, 1),
    ),
    row(
        d::R32_FLOAT_X8X24_TYPELESS,
        "R32_FLOAT_X8X24_TYPELESS",
        d::R32G8X24_TYPELESS,
        Class::DepthPlane,
        Some(P::Depth32Float_Stencil8),
        8,
        (1, 1),
    ),
    row(
        d::X32_TYPELESS_G8X24_UINT,
        "X32_TYPELESS_G8X24_UINT",
        d::R32G8X24_TYPELESS,
        Class::StencilPlane,
        Some(P::X32_Stencil8),
        8,
        (1, 1),
    ),
    t(
        d::R10G10B10A2_TYPELESS,
        "R10G10B10A2_TYPELESS",
        P::RGB10A2Unorm,
        4,
    ),
    c(
        d::R10G10B10A2_UNORM,
        "R10G10B10A2_UNORM",
        d::R10G10B10A2_TYPELESS,
        P::RGB10A2Unorm,
        4,
    ),
    c(
        d::R10G10B10A2_UINT,
        "R10G10B10A2_UINT",
        d::R10G10B10A2_TYPELESS,
        P::RGB10A2Uint,
        4,
    ),
    c(
        d::R11G11B10_FLOAT,
        "R11G11B10_FLOAT",
        d::R11G11B10_FLOAT,
        P::RG11B10Float,
        4,
    ),
    t(d::R8G8B8A8_TYPELESS, "R8G8B8A8_TYPELESS", P::RGBA8Unorm, 4),
    c(
        d::R8G8B8A8_UNORM,
        "R8G8B8A8_UNORM",
        d::R8G8B8A8_TYPELESS,
        P::RGBA8Unorm,
        4,
    ),
    c(
        d::R8G8B8A8_UNORM_SRGB,
        "R8G8B8A8_UNORM_SRGB",
        d::R8G8B8A8_TYPELESS,
        P::RGBA8Unorm_sRGB,
        4,
    ),
    c(
        d::R8G8B8A8_UINT,
        "R8G8B8A8_UINT",
        d::R8G8B8A8_TYPELESS,
        P::RGBA8Uint,
        4,
    ),
    c(
        d::R8G8B8A8_SNORM,
        "R8G8B8A8_SNORM",
        d::R8G8B8A8_TYPELESS,
        P::RGBA8Snorm,
        4,
    ),
    c(
        d::R8G8B8A8_SINT,
        "R8G8B8A8_SINT",
        d::R8G8B8A8_TYPELESS,
        P::RGBA8Sint,
        4,
    ),
    t(d::R16G16_TYPELESS, "R16G16_TYPELESS", P::RG16Float, 4),
    c(
        d::R16G16_FLOAT,
        "R16G16_FLOAT",
        d::R16G16_TYPELESS,
        P::RG16Float,
        4,
    ),
    c(
        d::R16G16_UNORM,
        "R16G16_UNORM",
        d::R16G16_TYPELESS,
        P::RG16Unorm,
        4,
    ),
    c(
        d::R16G16_UINT,
        "R16G16_UINT",
        d::R16G16_TYPELESS,
        P::RG16Uint,
        4,
    ),
    c(
        d::R16G16_SNORM,
        "R16G16_SNORM",
        d::R16G16_TYPELESS,
        P::RG16Snorm,
        4,
    ),
    c(
        d::R16G16_SINT,
        "R16G16_SINT",
        d::R16G16_TYPELESS,
        P::RG16Sint,
        4,
    ),
    t(d::R32_TYPELESS, "R32_TYPELESS", P::R32Float, 4),
    row(
        d::D32_FLOAT,
        "D32_FLOAT",
        d::R32_TYPELESS,
        Class::Depth,
        Some(P::Depth32Float),
        4,
        (1, 1),
    ),
    c(d::R32_FLOAT, "R32_FLOAT", d::R32_TYPELESS, P::R32Float, 4),
    c(d::R32_UINT, "R32_UINT", d::R32_TYPELESS, P::R32Uint, 4),
    c(d::R32_SINT, "R32_SINT", d::R32_TYPELESS, P::R32Sint, 4),
    // D24S8: no 24-bit depth on Apple GPUs, see the module comment.
    row(
        d::R24G8_TYPELESS,
        "R24G8_TYPELESS",
        d::R24G8_TYPELESS,
        Class::Typeless,
        Some(P::Depth32Float_Stencil8),
        8,
        (1, 1),
    ),
    row(
        d::D24_UNORM_S8_UINT,
        "D24_UNORM_S8_UINT",
        d::R24G8_TYPELESS,
        Class::DepthStencil,
        Some(P::Depth32Float_Stencil8),
        8,
        (1, 1),
    ),
    row(
        d::R24_UNORM_X8_TYPELESS,
        "R24_UNORM_X8_TYPELESS",
        d::R24G8_TYPELESS,
        Class::DepthPlane,
        Some(P::Depth32Float_Stencil8),
        8,
        (1, 1),
    ),
    row(
        d::X24_TYPELESS_G8_UINT,
        "X24_TYPELESS_G8_UINT",
        d::R24G8_TYPELESS,
        Class::StencilPlane,
        Some(P::X32_Stencil8),
        8,
        (1, 1),
    ),
    t(d::R8G8_TYPELESS, "R8G8_TYPELESS", P::RG8Unorm, 2),
    c(
        d::R8G8_UNORM,
        "R8G8_UNORM",
        d::R8G8_TYPELESS,
        P::RG8Unorm,
        2,
    ),
    c(d::R8G8_UINT, "R8G8_UINT", d::R8G8_TYPELESS, P::RG8Uint, 2),
    c(
        d::R8G8_SNORM,
        "R8G8_SNORM",
        d::R8G8_TYPELESS,
        P::RG8Snorm,
        2,
    ),
    c(d::R8G8_SINT, "R8G8_SINT", d::R8G8_TYPELESS, P::RG8Sint, 2),
    t(d::R16_TYPELESS, "R16_TYPELESS", P::R16Float, 2),
    c(d::R16_FLOAT, "R16_FLOAT", d::R16_TYPELESS, P::R16Float, 2),
    row(
        d::D16_UNORM,
        "D16_UNORM",
        d::R16_TYPELESS,
        Class::Depth,
        Some(P::Depth16Unorm),
        2,
        (1, 1),
    ),
    c(d::R16_UNORM, "R16_UNORM", d::R16_TYPELESS, P::R16Unorm, 2),
    c(d::R16_UINT, "R16_UINT", d::R16_TYPELESS, P::R16Uint, 2),
    c(d::R16_SNORM, "R16_SNORM", d::R16_TYPELESS, P::R16Snorm, 2),
    c(d::R16_SINT, "R16_SINT", d::R16_TYPELESS, P::R16Sint, 2),
    t(d::R8_TYPELESS, "R8_TYPELESS", P::R8Unorm, 1),
    c(d::R8_UNORM, "R8_UNORM", d::R8_TYPELESS, P::R8Unorm, 1),
    c(d::R8_UINT, "R8_UINT", d::R8_TYPELESS, P::R8Uint, 1),
    c(d::R8_SNORM, "R8_SNORM", d::R8_TYPELESS, P::R8Snorm, 1),
    c(d::R8_SINT, "R8_SINT", d::R8_TYPELESS, P::R8Sint, 1),
    c(d::A8_UNORM, "A8_UNORM", d::A8_UNORM, P::A8Unorm, 1),
    none(d::R1_UNORM, "R1_UNORM", 1),
    c(
        d::R9G9B9E5_SHAREDEXP,
        "R9G9B9E5_SHAREDEXP",
        d::R9G9B9E5_SHAREDEXP,
        P::RGB9E5Float,
        4,
    ),
    bc(
        d::BC1_TYPELESS,
        "BC1_TYPELESS",
        d::BC1_TYPELESS,
        Class::Typeless,
        P::BC1_RGBA,
        8,
    ),
    bc(
        d::BC1_UNORM,
        "BC1_UNORM",
        d::BC1_TYPELESS,
        Class::Color,
        P::BC1_RGBA,
        8,
    ),
    bc(
        d::BC1_UNORM_SRGB,
        "BC1_UNORM_SRGB",
        d::BC1_TYPELESS,
        Class::Color,
        P::BC1_RGBA_sRGB,
        8,
    ),
    bc(
        d::BC2_TYPELESS,
        "BC2_TYPELESS",
        d::BC2_TYPELESS,
        Class::Typeless,
        P::BC2_RGBA,
        16,
    ),
    bc(
        d::BC2_UNORM,
        "BC2_UNORM",
        d::BC2_TYPELESS,
        Class::Color,
        P::BC2_RGBA,
        16,
    ),
    bc(
        d::BC2_UNORM_SRGB,
        "BC2_UNORM_SRGB",
        d::BC2_TYPELESS,
        Class::Color,
        P::BC2_RGBA_sRGB,
        16,
    ),
    bc(
        d::BC3_TYPELESS,
        "BC3_TYPELESS",
        d::BC3_TYPELESS,
        Class::Typeless,
        P::BC3_RGBA,
        16,
    ),
    bc(
        d::BC3_UNORM,
        "BC3_UNORM",
        d::BC3_TYPELESS,
        Class::Color,
        P::BC3_RGBA,
        16,
    ),
    bc(
        d::BC3_UNORM_SRGB,
        "BC3_UNORM_SRGB",
        d::BC3_TYPELESS,
        Class::Color,
        P::BC3_RGBA_sRGB,
        16,
    ),
    bc(
        d::BC4_TYPELESS,
        "BC4_TYPELESS",
        d::BC4_TYPELESS,
        Class::Typeless,
        P::BC4_RUnorm,
        8,
    ),
    bc(
        d::BC4_UNORM,
        "BC4_UNORM",
        d::BC4_TYPELESS,
        Class::Color,
        P::BC4_RUnorm,
        8,
    ),
    bc(
        d::BC4_SNORM,
        "BC4_SNORM",
        d::BC4_TYPELESS,
        Class::Color,
        P::BC4_RSnorm,
        8,
    ),
    bc(
        d::BC5_TYPELESS,
        "BC5_TYPELESS",
        d::BC5_TYPELESS,
        Class::Typeless,
        P::BC5_RGUnorm,
        16,
    ),
    bc(
        d::BC5_UNORM,
        "BC5_UNORM",
        d::BC5_TYPELESS,
        Class::Color,
        P::BC5_RGUnorm,
        16,
    ),
    bc(
        d::BC5_SNORM,
        "BC5_SNORM",
        d::BC5_TYPELESS,
        Class::Color,
        P::BC5_RGSnorm,
        16,
    ),
    c(
        d::B5G6R5_UNORM,
        "B5G6R5_UNORM",
        d::B5G6R5_UNORM,
        P::B5G6R5Unorm,
        2,
    ),
    c(
        d::B5G5R5A1_UNORM,
        "B5G5R5A1_UNORM",
        d::B5G5R5A1_UNORM,
        P::BGR5A1Unorm,
        2,
    ),
    c(
        d::B8G8R8A8_UNORM,
        "B8G8R8A8_UNORM",
        d::B8G8R8A8_TYPELESS,
        P::BGRA8Unorm,
        4,
    ),
    none(d::B8G8R8X8_UNORM, "B8G8R8X8_UNORM", 4),
    t(d::B8G8R8A8_TYPELESS, "B8G8R8A8_TYPELESS", P::BGRA8Unorm, 4),
    c(
        d::B8G8R8A8_UNORM_SRGB,
        "B8G8R8A8_UNORM_SRGB",
        d::B8G8R8A8_TYPELESS,
        P::BGRA8Unorm_sRGB,
        4,
    ),
    none(d::B8G8R8X8_TYPELESS, "B8G8R8X8_TYPELESS", 4),
    none(d::B8G8R8X8_UNORM_SRGB, "B8G8R8X8_UNORM_SRGB", 4),
    bc(
        d::BC6H_TYPELESS,
        "BC6H_TYPELESS",
        d::BC6H_TYPELESS,
        Class::Typeless,
        P::BC6H_RGBUfloat,
        16,
    ),
    bc(
        d::BC6H_UF16,
        "BC6H_UF16",
        d::BC6H_TYPELESS,
        Class::Color,
        P::BC6H_RGBUfloat,
        16,
    ),
    bc(
        d::BC6H_SF16,
        "BC6H_SF16",
        d::BC6H_TYPELESS,
        Class::Color,
        P::BC6H_RGBFloat,
        16,
    ),
    bc(
        d::BC7_TYPELESS,
        "BC7_TYPELESS",
        d::BC7_TYPELESS,
        Class::Typeless,
        P::BC7_RGBAUnorm,
        16,
    ),
    bc(
        d::BC7_UNORM,
        "BC7_UNORM",
        d::BC7_TYPELESS,
        Class::Color,
        P::BC7_RGBAUnorm,
        16,
    ),
    bc(
        d::BC7_UNORM_SRGB,
        "BC7_UNORM_SRGB",
        d::BC7_TYPELESS,
        Class::Color,
        P::BC7_RGBAUnorm_sRGB,
        16,
    ),
    none(d::B4G4R4A4_UNORM, "B4G4R4A4_UNORM", 2),
];

/// The row of a format number, or `None` for a number outside the table.
pub fn lookup(dxgi: u32) -> Option<&'static Format> {
    FORMATS
        .binary_search_by_key(&dxgi, |f| f.dxgi)
        .ok()
        .map(|i| &FORMATS[i])
}

/// The row of a format number Metal can make, else `Unsupported` (naming the
/// format when the table knows it).
pub fn require(dxgi: u32) -> Result<&'static Format> {
    match lookup(dxgi) {
        Some(f) if f.pixel.is_some() => Ok(f),
        Some(f) => Err(Error::Unsupported(format!(
            "DXGI format {} ({dxgi}) has no Metal pixel format",
            f.name
        ))),
        None => Err(Error::Unsupported(format!("unknown DXGI format {dxgi}"))),
    }
}

impl Format {
    /// Block-compressed (4x4 blocks).
    pub fn is_compressed(&self) -> bool {
        self.block != (1, 1)
    }

    /// Has a depth or stencil aspect.
    pub fn has_depth_stencil(&self) -> bool {
        matches!(
            self.class,
            Class::Depth | Class::DepthStencil | Class::DepthPlane | Class::StencilPlane
        ) || matches!(self.dxgi, d::R32G8X24_TYPELESS | d::R24G8_TYPELESS)
    }

    /// Planes of a texture of this format (depth and stencil are two).
    pub fn planes(&self) -> u32 {
        match self.dxgi {
            d::R32G8X24_TYPELESS
            | d::D32_FLOAT_S8X24_UINT
            | d::R24G8_TYPELESS
            | d::D24_UNORM_S8_UINT => 2,
            _ => 1,
        }
    }

    /// Bytes of one row of `width` texels, counted in blocks.
    pub fn row_bytes(&self, width: u32) -> u64 {
        u64::from(width.div_ceil(u32::from(self.block.0))) * u64::from(self.bytes)
    }

    /// Rows of blocks in `height` texels.
    pub fn block_rows(&self, height: u32) -> u32 {
        height.div_ceil(u32::from(self.block.1))
    }

    /// The family holds an sRGB member: a texture of any of its formats may
    /// be viewed as the other one (`RGBA8Unorm` <-> `RGBA8Unorm_sRGB`).
    fn family_has_srgb(&self) -> bool {
        FORMATS
            .iter()
            .any(|f| f.family == self.family && f.name.ends_with("_SRGB"))
    }

    /// Whether a texture created with this number must allow views in other
    /// pixel formats (`MTLTextureUsagePixelFormatView`): typeless ones always;
    /// typed ones only when the family has an sRGB pair (the one cast the
    /// original API allows on a typed texture). Depth and stencil textures are
    /// viewed by plane, which needs no flag.
    pub fn needs_view_usage(&self) -> bool {
        if self.has_depth_stencil() {
            return false;
        }
        self.class == Class::Typeless || self.family_has_srgb()
    }

    /// 32-bit integer atomics on a texture of this format.
    pub fn atomic(&self) -> bool {
        matches!(self.dxgi, d::R32_UINT | d::R32_SINT)
    }
}

/// `tex_usage` bits (proto `types::tex_usage`), repeated as numbers so that the
/// back's tests need not name the proto module.
const USAGE_SRV: u32 = 1 << 0;
const USAGE_UAV: u32 = 1 << 1;
const USAGE_RT: u32 = 1 << 2;
const USAGE_DS: u32 = 1 << 3;

/// The Metal format a texture of `dxgi` is created with. A typeless
/// `R32_TYPELESS` / `R16_TYPELESS` becomes the depth format when `usage` has
/// the depth-stencil bit (the only way Metal can render depth to it).
pub fn texture_pixel_format(dxgi: u32, usage: u32) -> Result<MTLPixelFormat> {
    let f = require(dxgi)?;
    let depth = usage & USAGE_DS != 0;
    Ok(match (f.dxgi, depth) {
        (d::R32_TYPELESS, true) => P::Depth32Float,
        (d::R16_TYPELESS, true) => P::Depth16Unorm,
        _ => f.pixel.expect("require checked"),
    })
}

/// The Metal usage flags of a texture of `dxgi` used as `usage` says.
/// Refuses what Metal cannot do: shader writes to a depth, stencil, sRGB or
/// block-compressed texture, a render target with a compressed format.
pub fn texture_usage(dxgi: u32, usage: u32) -> Result<MTLTextureUsage> {
    let f = require(dxgi)?;
    let mut u = MTLTextureUsage::empty();
    // Copies work whatever the flags are; a texture nobody reads or writes
    // from a shader is still given `ShaderRead` so that views can be made.
    if usage & USAGE_SRV != 0 || usage & (USAGE_UAV | USAGE_RT | USAGE_DS) == 0 {
        u |= MTLTextureUsage::ShaderRead;
    }
    if usage & USAGE_UAV != 0 {
        if f.is_compressed() || f.has_depth_stencil() {
            return Err(Error::Unsupported(format!(
                "UAV access to {} (shader writes to compressed or depth-stencil textures)",
                f.name
            )));
        }
        u |= MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite;
        if f.atomic() {
            u |= MTLTextureUsage::ShaderAtomic;
        }
    }
    if usage & (USAGE_RT | USAGE_DS) != 0 {
        if f.is_compressed() {
            return Err(Error::Unsupported(format!(
                "{} cannot be a render target",
                f.name
            )));
        }
        u |= MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead;
    }
    if f.needs_view_usage() {
        u |= MTLTextureUsage::PixelFormatView;
    }
    Ok(u)
}

/// The Metal format of a view in `view` of a texture made as `texture`
/// (both DXGI numbers; `view` 0 = the texture's own). The family must agree.
/// For depth-stencil textures the view picks a plane: depth views read as
/// the texture's depth format, stencil views as `X32_Stencil8`. `usage` is
/// what the texture was created with (it decides `R32_TYPELESS` depth).
pub fn view_pixel_format(texture: u32, view: u32, usage: u32) -> Result<MTLPixelFormat> {
    let base = texture_pixel_format(texture, usage)?;
    if view == dxgi::UNKNOWN || view == texture {
        return stencil_or(base, view);
    }
    let t = require(texture)?;
    let v = require(view)?;
    if t.family != v.family {
        return Err(Error::Unsupported(format!(
            "a view in {} of a {} texture (different format families)",
            v.name, t.name
        )));
    }
    if t.has_depth_stencil() || matches!(base, P::Depth32Float | P::Depth16Unorm) {
        return Ok(match v.class {
            Class::StencilPlane => P::X32_Stencil8,
            _ => base,
        });
    }
    Ok(v.pixel.expect("require checked"))
}

fn stencil_or(base: MTLPixelFormat, view: u32) -> Result<MTLPixelFormat> {
    match lookup(view) {
        Some(v) if v.class == Class::StencilPlane => Ok(P::X32_Stencil8),
        _ => Ok(base),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sorted_and_unique() {
        for w in FORMATS.windows(2) {
            assert!(w[0].dxgi < w[1].dxgi, "{} before {}", w[0].name, w[1].name);
        }
    }

    #[test]
    fn names_match_numbers() {
        // Spot checks against the DXGI header.
        for (n, name) in [
            (2, "R32G32B32A32_FLOAT"),
            (28, "R8G8B8A8_UNORM"),
            (29, "R8G8B8A8_UNORM_SRGB"),
            (45, "D24_UNORM_S8_UINT"),
            (87, "B8G8R8A8_UNORM"),
            (91, "B8G8R8A8_UNORM_SRGB"),
            (98, "BC7_UNORM"),
            (99, "BC7_UNORM_SRGB"),
        ] {
            assert_eq!(lookup(n).unwrap().name, name);
        }
    }

    #[test]
    fn the_formats_the_task_needs_are_covered() {
        // RGBA8/BGRA8/sRGB, R16/RG16/RGBA16 float/unorm/uint, R32 family,
        // R11G11B10, R10G10B10A2, BC1-7, depth and typeless.
        let want = [
            "R8G8B8A8_UNORM",
            "R8G8B8A8_UNORM_SRGB",
            "B8G8R8A8_UNORM",
            "B8G8R8A8_UNORM_SRGB",
            "R16_FLOAT",
            "R16_UNORM",
            "R16_UINT",
            "R16G16_FLOAT",
            "R16G16_UNORM",
            "R16G16_UINT",
            "R16G16B16A16_FLOAT",
            "R16G16B16A16_UNORM",
            "R16G16B16A16_UINT",
            "R32_FLOAT",
            "R32_UINT",
            "R32_SINT",
            "R32G32_FLOAT",
            "R32G32_UINT",
            "R32G32B32A32_FLOAT",
            "R32G32B32A32_UINT",
            "R11G11B10_FLOAT",
            "R10G10B10A2_UNORM",
            "R10G10B10A2_UINT",
            "BC1_UNORM",
            "BC1_UNORM_SRGB",
            "BC2_UNORM",
            "BC3_UNORM",
            "BC3_UNORM_SRGB",
            "BC4_UNORM",
            "BC4_SNORM",
            "BC5_UNORM",
            "BC5_SNORM",
            "BC6H_UF16",
            "BC6H_SF16",
            "BC7_UNORM",
            "BC7_UNORM_SRGB",
            "D32_FLOAT",
            "D16_UNORM",
            "D24_UNORM_S8_UINT",
            "D32_FLOAT_S8X24_UINT",
            "R24G8_TYPELESS",
            "R32_TYPELESS",
            "R8G8B8A8_TYPELESS",
            "R16_TYPELESS",
            "R32G8X24_TYPELESS",
        ];
        for name in want {
            let f = FORMATS
                .iter()
                .find(|f| f.name == name)
                .unwrap_or_else(|| panic!("{name}"));
            assert!(f.pixel.is_some(), "{name}");
        }
    }

    #[test]
    fn typeless_rows_head_their_family_and_depth_is_in_one() {
        for f in FORMATS {
            if f.class == Class::Typeless {
                assert_eq!(f.family, f.dxgi, "{}", f.name);
            } else if f.family != f.dxgi {
                let head = lookup(f.family).unwrap_or_else(|| panic!("{} family", f.name));
                assert_eq!(head.class, Class::Typeless, "{}", f.name);
            }
        }
        assert_eq!(lookup(d::D32_FLOAT).unwrap().family, d::R32_TYPELESS);
        assert_eq!(
            lookup(d::D24_UNORM_S8_UINT).unwrap().family,
            d::R24G8_TYPELESS
        );
    }

    #[test]
    fn sizes_and_blocks() {
        assert_eq!(lookup(d::BC1_UNORM).unwrap().row_bytes(16), 32);
        assert_eq!(
            lookup(d::BC7_UNORM).unwrap().row_bytes(5),
            32,
            "5 texels are 2 blocks"
        );
        assert_eq!(lookup(d::BC7_UNORM).unwrap().block_rows(5), 2);
        assert_eq!(lookup(d::R8G8B8A8_UNORM).unwrap().row_bytes(7), 28);
        assert_eq!(lookup(d::D24_UNORM_S8_UINT).unwrap().planes(), 2);
        assert_eq!(lookup(d::D32_FLOAT).unwrap().planes(), 1);
        for f in FORMATS {
            assert!(f.bytes > 0, "{}", f.name);
            assert_eq!(f.is_compressed(), f.name.starts_with("BC"), "{}", f.name);
        }
    }

    #[test]
    fn unmappable_formats_are_refused_by_name() {
        for n in [
            d::R32G32B32_FLOAT,
            d::B8G8R8X8_UNORM,
            d::B4G4R4A4_UNORM,
            d::R1_UNORM,
        ] {
            let e = require(n).unwrap_err();
            assert!(
                matches!(&e, Error::Unsupported(m) if m.contains("no Metal pixel format")),
                "{e}"
            );
        }
        assert!(matches!(require(1000), Err(Error::Unsupported(_))));
        assert!(matches!(require(0), Err(Error::Unsupported(_))));
    }

    #[test]
    fn typeless_textures_pick_a_base_and_depth_by_usage() {
        assert_eq!(
            texture_pixel_format(d::R8G8B8A8_TYPELESS, USAGE_SRV).unwrap(),
            P::RGBA8Unorm
        );
        assert_eq!(
            texture_pixel_format(d::R32_TYPELESS, USAGE_SRV).unwrap(),
            P::R32Float
        );
        assert_eq!(
            texture_pixel_format(d::R32_TYPELESS, USAGE_SRV | USAGE_DS).unwrap(),
            P::Depth32Float
        );
        assert_eq!(
            texture_pixel_format(d::R16_TYPELESS, USAGE_DS).unwrap(),
            P::Depth16Unorm
        );
        assert_eq!(
            texture_pixel_format(d::R24G8_TYPELESS, USAGE_DS).unwrap(),
            P::Depth32Float_Stencil8
        );
        assert_eq!(
            texture_pixel_format(d::D24_UNORM_S8_UINT, USAGE_DS).unwrap(),
            P::Depth32Float_Stencil8
        );
    }

    #[test]
    fn views_follow_the_family() {
        // sRGB reinterpretation of a typeless colour texture.
        let v = |t, w| view_pixel_format(t, w, USAGE_SRV).unwrap();
        assert_eq!(
            v(d::R8G8B8A8_TYPELESS, d::R8G8B8A8_UNORM_SRGB),
            P::RGBA8Unorm_sRGB
        );
        assert_eq!(v(d::R8G8B8A8_TYPELESS, d::R8G8B8A8_UINT), P::RGBA8Uint);
        assert_eq!(v(d::R8G8B8A8_UNORM, d::UNKNOWN), P::RGBA8Unorm);
        assert_eq!(v(d::R32_TYPELESS, d::R32_UINT), P::R32Uint);
        assert_eq!(
            v(d::B8G8R8A8_TYPELESS, d::B8G8R8A8_UNORM_SRGB),
            P::BGRA8Unorm_sRGB
        );
        assert_eq!(v(d::BC1_TYPELESS, d::BC1_UNORM_SRGB), P::BC1_RGBA_sRGB);
        // Another family: refused.
        assert!(view_pixel_format(d::R8G8B8A8_UNORM, d::R32_UINT, USAGE_SRV).is_err());
        assert!(view_pixel_format(d::R8G8B8A8_UNORM, d::B8G8R8A8_UNORM, USAGE_SRV).is_err());
        // Depth: R32_FLOAT of a depth texture reads the depth format; stencil by plane.
        let ds = USAGE_SRV | USAGE_DS;
        assert_eq!(
            view_pixel_format(d::R32_TYPELESS, d::R32_FLOAT, ds).unwrap(),
            P::Depth32Float
        );
        assert_eq!(
            view_pixel_format(d::R32_TYPELESS, d::D32_FLOAT, ds).unwrap(),
            P::Depth32Float
        );
        assert_eq!(
            view_pixel_format(d::R16_TYPELESS, d::R16_UNORM, ds).unwrap(),
            P::Depth16Unorm
        );
        assert_eq!(
            view_pixel_format(d::R24G8_TYPELESS, d::R24_UNORM_X8_TYPELESS, ds).unwrap(),
            P::Depth32Float_Stencil8
        );
        assert_eq!(
            view_pixel_format(d::R24G8_TYPELESS, d::X24_TYPELESS_G8_UINT, ds).unwrap(),
            P::X32_Stencil8
        );
        assert_eq!(
            view_pixel_format(d::R32G8X24_TYPELESS, d::X32_TYPELESS_G8X24_UINT, ds).unwrap(),
            P::X32_Stencil8
        );
        assert_eq!(
            view_pixel_format(d::D32_FLOAT_S8X24_UINT, d::UNKNOWN, ds).unwrap(),
            P::Depth32Float_Stencil8
        );
    }

    #[test]
    fn usage_flags() {
        let u = |f, usage| texture_usage(f, usage).unwrap();
        let sr = MTLTextureUsage::ShaderRead;
        assert_eq!(
            u(d::R8G8B8A8_UNORM_SRGB, USAGE_SRV),
            sr | MTLTextureUsage::PixelFormatView
        );
        assert_eq!(u(d::R8G8_UNORM, USAGE_SRV), sr);
        assert_eq!(u(d::R8G8_UNORM, 0), sr, "copy-only textures stay viewable");
        assert_eq!(
            u(d::R32_UINT, USAGE_UAV | USAGE_SRV),
            sr | MTLTextureUsage::ShaderWrite | MTLTextureUsage::ShaderAtomic
        );
        assert_eq!(
            u(d::R8G8B8A8_TYPELESS, USAGE_RT),
            MTLTextureUsage::RenderTarget | sr | MTLTextureUsage::PixelFormatView
        );
        assert_eq!(
            u(d::D32_FLOAT, USAGE_DS | USAGE_SRV),
            MTLTextureUsage::RenderTarget | sr
        );
        assert!(texture_usage(d::BC7_UNORM, USAGE_UAV).is_err());
        assert!(texture_usage(d::BC7_UNORM, USAGE_RT).is_err());
        assert!(texture_usage(d::D32_FLOAT, USAGE_UAV).is_err());
        assert!(texture_usage(d::R32G32B32_FLOAT, USAGE_SRV).is_err());
    }
}
