//! `format()` against the sizes the DXGI format table gives (C-V-003: "every format the detiler covers").
//!
//! Reference: Microsoft's `DXGI_FORMAT` enumeration (dxgiformat.h, Windows SDK 10.0.26100 numbering) and
//! "Programming Guide for DXGI / Block Compression" (BC1 and BC4 take 8 bytes per 4x4 block, BC2, BC3, BC5, BC6H
//! and BC7 16); the console numbers (115-120) are those of the Xbox GDK's `d3d12_xs.h` /`dxgiformat_xs.h`
//! (116/117 the two 10-bit float colour formats, 118-120 the 16-bit depth with 8-bit stencil whose stencil is a
//! plane of its own). Formats with no layout here (96-bit, 1-bit, packed 4:2:2, video) answer `None` on purpose.
use victory_formats::texture::format;

/// (first number, last number, name, bytes per element, texels per element side, stencil plane)
#[rustfmt::skip]
const SPEC: &[(u32, u32, &str, u32, u32, bool)] = &[
    (1, 4, "R32G32B32A32_*", 16, 1, false),
    (9, 14, "R16G16B16A16_*", 8, 1, false),
    (15, 18, "R32G32_*", 8, 1, false),
    (19, 22, "R32G8X24 / D32_FLOAT_S8X24 family", 4, 1, true),
    (23, 25, "R10G10B10A2_*", 4, 1, false),
    (26, 26, "R11G11B10_FLOAT", 4, 1, false),
    (27, 32, "R8G8B8A8_*", 4, 1, false),
    (33, 38, "R16G16_*", 4, 1, false),
    (39, 43, "R32_* / D32_FLOAT", 4, 1, false),
    (44, 47, "R24G8 / D24_UNORM_S8_UINT family", 4, 1, true),
    (48, 52, "R8G8_*", 2, 1, false),
    (53, 59, "R16_* / D16_UNORM", 2, 1, false),
    (60, 65, "R8_* / A8_UNORM", 1, 1, false),
    (67, 67, "R9G9B9E5_SHAREDEXP", 4, 1, false),
    (70, 72, "BC1_*", 8, 4, false),
    (73, 78, "BC2_*, BC3_*", 16, 4, false),
    (79, 81, "BC4_*", 8, 4, false),
    (82, 84, "BC5_*", 16, 4, false),
    (85, 86, "B5G6R5, B5G5R5A1", 2, 1, false),
    (87, 93, "B8G8R8A8 / B8G8R8X8 / R10G10B10_XR_BIAS_A2 families", 4, 1, false),
    (94, 99, "BC6H_*, BC7_*", 16, 4, false),
    (115, 115, "B4G4R4A4_UNORM", 2, 1, false),
    (116, 117, "R10G10B10_7E3_A2 / 6E4_A2 (console)", 4, 1, false),
    (118, 120, "D16_UNORM_S8_UINT family (console)", 2, 1, true),
];

#[test]
fn every_format_of_the_spec_has_its_size() {
    for &(first, last, name, bytes, block, stencil) in SPEC {
        for dxgi in first..=last {
            let f = format(dxgi).unwrap_or_else(|| panic!("{name} ({dxgi}) has no layout"));
            assert_eq!((f.bytes, f.block, f.stencil), (bytes, block, stencil), "{name} ({dxgi})");
        }
    }
}

#[test]
fn formats_with_no_layout_are_none() {
    // 0 UNKNOWN; 5-8 R32G32B32 (96 bits, 12-byte elements); 66 R1_UNORM; 68/69 packed 4:2:2 (R8G8_B8G8, G8R8_G8B8);
    // 100 and up: video (AYUV, Y410, NV12, ...) and the console-only numbers the detiler does not know
    for dxgi in [0, 5, 6, 7, 8, 66, 68, 69, 100, 103, 114, 121, 130, 189, 190, 191, 200] {
        assert!(format(dxgi).is_none(), "{dxgi}");
    }
    // every number the table covers is in `SPEC`: nothing answers that the spec does not list
    for dxgi in 0..256 {
        let listed = SPEC.iter().any(|&(a, b, ..)| (a..=b).contains(&dxgi));
        assert_eq!(format(dxgi).is_some(), listed, "format {dxgi}");
    }
}

/// A texture in a mode with XOR has a size (checked against addrlib in `addrlib_golden.rs`) but no addressing:
/// detiling and tiling say `Unsupported` and write nothing, never a wrong picture (matrix row C-V-003, the part that is a gap).
#[test]
fn xor_modes_refuse_to_detile_and_write_nothing() {
    use victory_formats::texture::{Desc, Dimension, Error, Layout, Swizzle};
    for mode in [17, 18, 21, 22, 24, 25, 26, 27] {
        let l = Layout::new(&Desc {
            dimension: Dimension::Tex2d,
            width: 64,
            height: 64,
            depth: 1,
            mips: 1,
            element_bytes: 4,
            block: 1,
            samples: 1,
            swizzle: Swizzle(mode),
        })
        .unwrap();
        assert!(!l.addressable(), "mode {mode}");
        let tiled = vec![0x11u8; l.size as usize];
        let mut linear = vec![0xeeu8; 64 * 4 * 64];
        assert_eq!(l.detile(&tiled, 0, 0, &mut linear, 256, 256 * 64), Err(Error::Unsupported), "mode {mode}");
        assert!(linear.iter().all(|&b| b == 0xee), "mode {mode}: nothing written");
        let mut tiled = tiled;
        assert_eq!(l.tile(&mut tiled, 0, 0, &vec![0x22u8; 256 * 64], 256, 256 * 64), Err(Error::Unsupported));
        assert!(tiled.iter().all(|&b| b == 0x11), "mode {mode}: nothing written");
    }
}
