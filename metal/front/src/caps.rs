//! `ID3D12Device::CheckFeatureSupport`: what the device claims.
//!
//! The bulk of the answers are D3DMetal 4.0's (Wine 11, M5 Max, captured
//! 2026-11-27 in tests/golden/d3dmetal-wine11.txt: tiers, limits, the
//! OPTIONSn blocks), so that software written against it sees the same
//! device. The fields that depend on our backend are in [`Caps`] and patched
//! in: shader model, wave size, mesh tier, unified memory, tile-based.

use crate::format;
use crate::hresult::{Hr, Result};

/// `D3D_SHADER_MODEL_6_6`.
pub const SHADER_MODEL_6_6: u32 = 0x66;
/// `D3D12_MESH_SHADER_TIER_1`.
pub const MESH_SHADER_TIER_1: u32 = 10;
pub const FEATURE_LEVEL_12_0: u32 = 0xc000;
pub const FEATURE_LEVEL_12_1: u32 = 0xc100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    /// Highest shader model answered (6.6 for M1).
    pub shader_model: u32,
    pub wave_lane_min: u32,
    pub wave_lane_max: u32,
    pub mesh_shader_tier: u32,
    /// Apple GPUs: UMA and tile-based.
    pub unified_memory: bool,
    pub tile_based: bool,
    pub max_feature_level: u32,
}

impl Caps {
    /// The M1 claim: SM 6.6, wave32, mesh tier 1, unified memory, tile-based.
    pub const fn apple() -> Caps {
        Caps {
            shader_model: SHADER_MODEL_6_6,
            wave_lane_min: 32,
            wave_lane_max: 32,
            mesh_shader_tier: MESH_SHADER_TIER_1,
            unified_memory: true,
            tile_based: true,
            max_feature_level: FEATURE_LEVEL_12_1,
        }
    }
}

// Feature ids (D3D12_FEATURE).
pub const F_OPTIONS: u32 = 0;
pub const F_ARCHITECTURE: u32 = 1;
pub const F_FEATURE_LEVELS: u32 = 2;
pub const F_FORMAT_SUPPORT: u32 = 3;
pub const F_MULTISAMPLE_QUALITY_LEVELS: u32 = 4;
pub const F_FORMAT_INFO: u32 = 5;
pub const F_GPU_VIRTUAL_ADDRESS_SUPPORT: u32 = 6;
pub const F_SHADER_MODEL: u32 = 7;
pub const F_OPTIONS1: u32 = 8;
pub const F_ROOT_SIGNATURE: u32 = 12;
pub const F_ARCHITECTURE1: u32 = 16;
pub const F_OPTIONS2: u32 = 18;
pub const F_SHADER_CACHE: u32 = 19;
pub const F_COMMAND_QUEUE_PRIORITY: u32 = 20;
pub const F_OPTIONS3: u32 = 21;
pub const F_EXISTING_HEAPS: u32 = 22;
pub const F_OPTIONS4: u32 = 23;
pub const F_SERIALIZATION: u32 = 24;
pub const F_CROSS_NODE: u32 = 25;
pub const F_OPTIONS5: u32 = 27;
pub const F_DISPLAYABLE: u32 = 28;
pub const F_OPTIONS6: u32 = 30;
pub const F_OPTIONS7: u32 = 32;
pub const F_OPTIONS8: u32 = 36;
pub const F_OPTIONS9: u32 = 37;
pub const F_OPTIONS10: u32 = 39;
pub const F_OPTIONS11: u32 = 40;
pub const F_OPTIONS12: u32 = 41;
pub const F_OPTIONS13: u32 = 42;
pub const F_OPTIONS14: u32 = 43;
pub const F_OPTIONS15: u32 = 44;
pub const F_OPTIONS16: u32 = 45;
pub const F_OPTIONS17: u32 = 46;
pub const F_OPTIONS18: u32 = 47;
pub const F_OPTIONS19: u32 = 48;
pub const F_OPTIONS20: u32 = 49;
pub const F_OPTIONS21: u32 = 53;

/// Fixed-size blocks, as D3DMetal answered them (hex of the structure), except
/// two answers that say what this front does not build (docs/metal/m4-newbase.md):
///
/// * `F_OPTIONS`: `TiledResourcesTier` (word 3) is 0, not D3DMetal's 2: there are no
///   reserved resources or tile mappings (`CreateReservedResource` /
///   `UpdateTileMappings` are `E_NOTIMPL`), so a title asked "tiled resources?" must
///   take its non-tiled path. `victory-gpu`'s tiles.rs `wanted()` is then false.
/// * `F_OPTIONS12`: `EnhancedBarriersSupported` (word 1) is 0, not 1: lists are
///   `ID3D12GraphicsCommandList6` and have no `Barrier` (List7), so legacy
///   `ResourceBarrier` it is; `victory-gpu` falls back when the List7 cast fails.
const BLOCKS: &[(u32, &str)] = &[
    (
        F_OPTIONS,
        "000000000100000002000000000000000300000001000000010000000100000000000000400000000000000000000000000000000100000002000000",
    ),
    (F_GPU_VIRTUAL_ADDRESS_SUPPORT, "4000000040000000"),
    (
        F_OPTIONS1,
        "010000002000000020000000000400000100000001000000",
    ),
    (F_OPTIONS2, "0100000001000000"),
    (F_OPTIONS3, "00000000010000000d0000000200000001000000"),
    (F_EXISTING_HEAPS, "00000000"),
    (F_OPTIONS4, "010000000200000001000000"),
    (F_OPTIONS5, "00000000020000000b000000"),
    (F_OPTIONS6, "0000000000000000000000000000000001000000"),
    (F_OPTIONS7, "0a00000000000000"),
    (F_OPTIONS8, "00000000"),
    (
        F_OPTIONS9,
        "010000000100000001000000010000000000000000000000",
    ),
    (F_OPTIONS10, "0000000000000000"),
    (F_OPTIONS11, "01000000"),
    (F_OPTIONS12, "ffffffff0000000000000000"),
    (
        F_OPTIONS13,
        "010000000100000000000000000000000000000000000000",
    ),
    (F_OPTIONS14, "000000000000000000000000"),
    (F_OPTIONS15, "0000000000000000"),
    (F_OPTIONS16, "0000000001000000"),
    (F_OPTIONS17, "0000000000000000"),
    (F_OPTIONS18, "00000000"),
    (
        F_OPTIONS19,
        "000000000000000001000000000000000000000000000000000800000008000040420f0000000000",
    ),
    (F_OPTIONS20, "0000000000000000"),
    (F_OPTIONS21, "000000000a0000000100000001000000"),
    // Known, nothing supported.
    (F_SHADER_CACHE, "00000000"),
    (F_SERIALIZATION, "0000000000000000"),
    (F_CROSS_NODE, "0000000000000000"),
    (F_DISPLAYABLE, "0000000000000000"),
];

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

fn put_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn get_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// Support1 / Support2 bits of `D3D12_FEATURE_DATA_FORMAT_SUPPORT`.
pub mod fmt_support {
    pub const BUFFER: u32 = 0x1;
    pub const IA_VERTEX_BUFFER: u32 = 0x2;
    pub const IA_INDEX_BUFFER: u32 = 0x4;
    pub const TEXTURE1D: u32 = 0x10;
    pub const TEXTURE2D: u32 = 0x20;
    pub const TEXTURE3D: u32 = 0x40;
    pub const TEXTURECUBE: u32 = 0x80;
    pub const SHADER_LOAD: u32 = 0x100;
    pub const SHADER_SAMPLE: u32 = 0x200;
    pub const MIP: u32 = 0x1000;
    pub const RENDER_TARGET: u32 = 0x4000;
    pub const BLENDABLE: u32 = 0x8000;
    pub const DEPTH_STENCIL: u32 = 0x10000;
    pub const TYPED_UNORDERED_ACCESS_VIEW: u32 = 0x200_0000;
    pub const UAV_ATOMIC_ALL: u32 = 0x3f;
    pub const UAV_TYPED_LOAD: u32 = 0x40;
    pub const UAV_TYPED_STORE: u32 = 0x80;
    pub const OUTPUT_MERGER_LOGIC_OP: u32 = 0x100;
}

/// Support1 and Support2 for a format. UAV typed load/store follows D3D12's
/// rules: R32 UINT/SINT/FLOAT always (atomics on the integer ones), the 4- and
/// 1-component formats D3D12 lists as optional ones reported supported (Apple
/// GPUs read and write them all).
pub fn format_support(fmt: u32) -> (u32, u32) {
    use fmt_support::*;
    if fmt == format::UNKNOWN {
        return (BUFFER, 0);
    }
    let Some(info) = format::info(fmt) else {
        return (0, 0);
    };
    if format::is_typeless(fmt) {
        return (TEXTURE1D | TEXTURE2D | TEXTURE3D | TEXTURECUBE | MIP, 0);
    }
    let mut s1 =
        TEXTURE1D | TEXTURE2D | TEXTURE3D | TEXTURECUBE | SHADER_LOAD | SHADER_SAMPLE | MIP;
    let mut s2 = 0;
    if format::is_depth(fmt) {
        return (s1 | DEPTH_STENCIL, 0);
    }
    if format::is_block_compressed(fmt) || info.planes > 1 || info.block_w > 1 {
        return (s1, 0);
    }
    s1 |= RENDER_TARGET | BLENDABLE | BUFFER | IA_VERTEX_BUFFER;
    if matches!(fmt, 42 | 57) {
        s1 |= IA_INDEX_BUFFER;
    }
    match fmt {
        // R32_UINT / R32_SINT: typed UAV and all atomics.
        42 | 43 => {
            s1 |= TYPED_UNORDERED_ACCESS_VIEW;
            s2 |= UAV_ATOMIC_ALL | UAV_TYPED_LOAD | UAV_TYPED_STORE | OUTPUT_MERGER_LOGIC_OP;
        }
        // R32_FLOAT: load/store.
        41 => {
            s1 |= TYPED_UNORDERED_ACCESS_VIEW;
            s2 |= UAV_TYPED_LOAD | UAV_TYPED_STORE;
        }
        // RGBA32 F/U/S, RG32 F/U/S, RGBA16 F/UNORM/U/SNORM/S, RGB10A2
        // UNORM/UINT, R11G11B10, RGBA8 UNORM/U/SNORM/S, RG16 F/UNORM/U/SNORM/S,
        // RG8 UNORM/U/SNORM/S, R16 F/UNORM/U/SNORM/S, R8 UNORM/U/SNORM/S,
        // B8G8R8A8 UNORM. (Apple GPUs write them all; the castable views of
        // one texel size on a shared texture need the whole row of a size.)
        2..=4
        | 16..=18
        | 10..=14
        | 24..=26
        | 28
        | 30..=32
        | 34..=38
        | 49..=52
        | 54
        | 56..=59
        | 61..=64
        | 87 => {
            s1 |= TYPED_UNORDERED_ACCESS_VIEW;
            s2 |= UAV_TYPED_LOAD | UAV_TYPED_STORE | OUTPUT_MERGER_LOGIC_OP;
        }
        _ => {}
    }
    (s1, s2)
}

/// Answer a feature query in place. `data` is the caller's structure (its
/// length is the `FeatureSupportDataSize`); queries that take input read it
/// first. Unknown features and wrong sizes are `E_INVALIDARG`, as D3DMetal
/// and D3D12 do. `FEATURE_LEVELS` carries a pointer and is answered by
/// [`max_feature_level`] in the shell.
pub fn check_feature_support(caps: &Caps, feature: u32, data: &mut [u8]) -> Result<()> {
    if let Some((_, hex)) = BLOCKS.iter().find(|(id, _)| *id == feature) {
        let mut block = unhex(hex);
        if data.len() != block.len() {
            return Err(Hr::E_INVALIDARG);
        }
        match feature {
            F_OPTIONS1 => {
                put_u32(&mut block, 4, caps.wave_lane_min);
                put_u32(&mut block, 8, caps.wave_lane_max);
            }
            F_OPTIONS7 => put_u32(&mut block, 0, caps.mesh_shader_tier),
            _ => {}
        }
        data.copy_from_slice(&block);
        return Ok(());
    }
    match feature {
        F_ARCHITECTURE | F_ARCHITECTURE1 => {
            let want = if feature == F_ARCHITECTURE { 16 } else { 20 };
            if data.len() != want || get_u32(data, 0) != 0 {
                return Err(Hr::E_INVALIDARG);
            }
            data.fill(0);
            put_u32(data, 4, caps.tile_based as u32);
            put_u32(data, 8, caps.unified_memory as u32);
            put_u32(data, 12, caps.unified_memory as u32);
            Ok(())
        }
        F_SHADER_MODEL => {
            if data.len() != 4 {
                return Err(Hr::E_INVALIDARG);
            }
            let asked = get_u32(data, 0);
            if asked < 0x51 {
                return Err(Hr::E_INVALIDARG);
            }
            put_u32(data, 0, asked.min(caps.shader_model));
            Ok(())
        }
        F_ROOT_SIGNATURE => {
            if data.len() != 4 || !matches!(get_u32(data, 0), 1 | 2) {
                return Err(Hr::E_INVALIDARG);
            }
            put_u32(data, 0, 2);
            Ok(())
        }
        F_FORMAT_SUPPORT => {
            if data.len() != 12 {
                return Err(Hr::E_INVALIDARG);
            }
            let (s1, s2) = format_support(get_u32(data, 0));
            put_u32(data, 4, s1);
            put_u32(data, 8, s2);
            Ok(())
        }
        F_FORMAT_INFO => {
            if data.len() != 8 {
                return Err(Hr::E_INVALIDARG);
            }
            let planes = format::info(get_u32(data, 0))
                .ok_or(Hr::E_INVALIDARG)?
                .planes;
            data[4] = planes;
            Ok(())
        }
        F_MULTISAMPLE_QUALITY_LEVELS => {
            if data.len() != 16 {
                return Err(Hr::E_INVALIDARG);
            }
            let count = get_u32(data, 4);
            if !count.is_power_of_two() || count > 16 {
                return Err(Hr::E_INVALIDARG);
            }
            let ok = format::info(get_u32(data, 0)).is_some() && matches!(count, 1 | 2 | 4 | 8);
            put_u32(data, 12, ok as u32);
            Ok(())
        }
        F_COMMAND_QUEUE_PRIORITY => {
            if data.len() != 12 || get_u32(data, 0) > 3 {
                return Err(Hr::E_INVALIDARG);
            }
            let normal = get_u32(data, 4) == 0;
            put_u32(data, 8, normal as u32);
            Ok(())
        }
        _ => Err(Hr::E_INVALIDARG),
    }
}

/// `D3D12_FEATURE_FEATURE_LEVELS`: the highest of the requested levels this
/// device does; an empty or all-too-high list is `E_INVALIDARG`.
pub fn max_feature_level(caps: &Caps, requested: &[u32]) -> Result<u32> {
    requested
        .iter()
        .copied()
        .filter(|l| *l <= caps.max_feature_level)
        .max()
        .ok_or(Hr::E_INVALIDARG)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(f: u32, input: &[u8]) -> Result<Vec<u8>> {
        let mut b = input.to_vec();
        check_feature_support(&Caps::apple(), f, &mut b)?;
        Ok(b)
    }

    /// OPTIONS3 is D3DMetal's answer byte for byte (docs/metal/m4-newbase.md, attributeAtVertex):
    /// `BarycentricsSupported` (word 4) is 1 on both, so the 103 pipelines MSC refuses are not
    /// a difference in what the host reports; victory-shader emits `attributeAtVertex` without
    /// looking at it.
    #[test]
    fn options3_is_what_d3dmetal_reports_and_says_barycentrics_are_supported() {
        let o = q(F_OPTIONS3, &[0; 20]).unwrap();
        let w = |i: usize| u32::from_le_bytes(o[4 * i..4 * i + 4].try_into().unwrap());
        assert_eq!(w(0), 0, "CopyQueueTimestampQueriesSupported");
        assert_eq!(w(1), 1, "CastingFullyTypedFormatSupported");
        assert_eq!(w(2), 0xd, "WriteBufferImmediateSupportFlags");
        assert_eq!(w(3), 2, "ViewInstancingTier");
        assert_eq!(w(4), 1, "BarycentricsSupported");
    }

    #[test]
    fn what_the_front_does_not_build_is_not_advertised() {
        // OPTIONS: TiledResourcesTier (word 3) is 0 (no reserved resources); the
        // other words are D3DMetal's (ResourceBindingTier 3, ResourceHeapTier 2).
        let o = q(F_OPTIONS, &[0; 60]).unwrap();
        let w = |i: usize| u32::from_le_bytes(o[4 * i..4 * i + 4].try_into().unwrap());
        assert_eq!(w(3), 0, "TiledResourcesTier");
        assert_eq!((w(2), w(4)), (2, 3));
        assert_eq!(w(14), 2, "ResourceHeapTier");
        // OPTIONS12: EnhancedBarriersSupported (word 1) is 0 (no ID3D12GraphicsCommandList7).
        let o12 = q(F_OPTIONS12, &[0; 12]).unwrap();
        let w12 = |i: usize| u32::from_le_bytes(o12[4 * i..4 * i + 4].try_into().unwrap());
        assert_eq!(
            w12(0),
            u32::MAX,
            "MSPrimitivesPipelineStatisticIncludesCulledPrimitives"
        );
        assert_eq!(w12(1), 0, "EnhancedBarriersSupported");
    }

    #[test]
    fn shader_model_is_6_6_and_clamped() {
        assert_eq!(
            q(F_SHADER_MODEL, &0x66u32.to_le_bytes()).unwrap(),
            0x66u32.to_le_bytes()
        );
        assert_eq!(
            q(F_SHADER_MODEL, &0x69u32.to_le_bytes()).unwrap(),
            0x66u32.to_le_bytes()
        );
        assert_eq!(
            q(F_SHADER_MODEL, &0x60u32.to_le_bytes()).unwrap(),
            0x60u32.to_le_bytes()
        );
        assert_eq!(
            q(F_SHADER_MODEL, &1u32.to_le_bytes()),
            Err(Hr::E_INVALIDARG)
        );
    }

    #[test]
    fn wave32_and_mesh() {
        let o1 = q(F_OPTIONS1, &[0; 24]).unwrap();
        assert_eq!(get_u32(&o1, 0), 1, "WaveOps");
        assert_eq!((get_u32(&o1, 4), get_u32(&o1, 8)), (32, 32));
        let o7 = q(F_OPTIONS7, &[0; 8]).unwrap();
        assert_eq!(get_u32(&o7, 0), MESH_SHADER_TIER_1);
        let mut none = Caps::apple();
        none.mesh_shader_tier = 0;
        let mut b = [0u8; 8];
        check_feature_support(&none, F_OPTIONS7, &mut b).unwrap();
        assert_eq!(get_u32(&b, 0), 0);
    }

    #[test]
    fn sizes_and_unknown_features_are_refused() {
        assert_eq!(q(F_SHADER_MODEL, &[0; 3]), Err(Hr::E_INVALIDARG));
        assert_eq!(q(F_OPTIONS1, &[0; 8]), Err(Hr::E_INVALIDARG));
        assert_eq!(q(9999, &[0; 4]), Err(Hr::E_INVALIDARG));
    }

    #[test]
    fn architecture_is_unified_and_tile_based() {
        let a = q(F_ARCHITECTURE, &[0; 16]).unwrap();
        assert_eq!((get_u32(&a, 4), get_u32(&a, 8), get_u32(&a, 12)), (1, 1, 1));
        let mut node1 = [0u8; 16];
        node1[0] = 1;
        assert_eq!(q(F_ARCHITECTURE, &node1), Err(Hr::E_INVALIDARG));
    }

    #[test]
    fn root_signature_highest_is_1_1() {
        assert_eq!(
            q(F_ROOT_SIGNATURE, &1u32.to_le_bytes()).unwrap(),
            2u32.to_le_bytes()
        );
        assert_eq!(
            q(F_ROOT_SIGNATURE, &0u32.to_le_bytes()),
            Err(Hr::E_INVALIDARG)
        );
    }

    #[test]
    fn typed_uav_formats_victory_shader_asks_about() {
        // host.rs: Support1 TYPED_UAV, Support2 UAV_TYPED_LOAD | STORE.
        for fmt in [41u32, 42, 43, 28, 2, 12] {
            let (s1, s2) = format_support(fmt);
            assert_ne!(s1 & fmt_support::TYPED_UNORDERED_ACCESS_VIEW, 0, "{fmt}");
            assert_eq!(s2 & 0xc0, 0xc0, "{fmt}");
        }
        assert_eq!(
            format_support(42).1 & fmt_support::UAV_ATOMIC_ALL,
            fmt_support::UAV_ATOMIC_ALL
        );
        assert_eq!(format_support(71).1, 0);
    }

    #[test]
    fn feature_levels() {
        let c = Caps::apple();
        assert_eq!(max_feature_level(&c, &[0xb000, 0xc000, 0xc200]), Ok(0xc000));
        assert_eq!(max_feature_level(&c, &[0xc200]), Err(Hr::E_INVALIDARG));
    }
}
