//! The barrier stage vocabulary and its one mapping to Metal.
//!
//! The masks are **our own** `u16`/`u8` bits (agreed across the layers in
//! the M1 brief); `proto` carries them, the front end derives them from
//! the original API's resource states, and this file is the only place that knows how
//! they map to `MTLStages` and `MTL4VisibilityOptions`.

use objc2_metal::{MTL4VisibilityOptions, MTLStages};

use crate::error::{Error, Result};

/// Vertex shader work of a render pass.
pub const VERTEX: u16 = 1 << 0;
/// Fragment shader work of a render pass.
pub const FRAGMENT: u16 = 1 << 1;
/// Tile shading work of a render pass.
pub const TILE: u16 = 1 << 2;
/// Object (amplification) shader work.
pub const OBJECT: u16 = 1 << 3;
/// Mesh shader work.
pub const MESH: u16 = 1 << 4;
/// Compute dispatches.
pub const DISPATCH: u16 = 1 << 5;
/// Copies and fills (the work of the copy queue).
pub const BLIT: u16 = 1 << 6;
/// Sparse / placement mapping updates.
pub const RESOURCE_STATE: u16 = 1 << 7;
/// Every stage we have a bit for.
pub const ALL: u16 = 0x00FF;

/// Visibility: flush caches to the device coherence point.
pub const VIS_DEVICE: u8 = 1 << 0;
/// Visibility: make aliased virtual addresses consistent.
pub const VIS_RESOURCE_ALIAS: u8 = 1 << 1;
/// Every visibility bit we have.
pub const VIS_ALL: u8 = 0x03;

/// The stages a Metal 4 compute encoder can encode.
pub const COMPUTE_ENCODER_STAGES: u16 = DISPATCH | BLIT;
/// The stages a Metal 4 render encoder can encode.
pub const RENDER_ENCODER_STAGES: u16 = VERTEX | FRAGMENT | TILE | OBJECT | MESH;

/// The table: one row per bit. Everything else in this crate goes through
/// [`to_mtl_stages`], so there is exactly one place to get this wrong.
pub const STAGE_TABLE: [(u16, MTLStages); 8] = [
    (VERTEX, MTLStages::Vertex),
    (FRAGMENT, MTLStages::Fragment),
    (TILE, MTLStages::Tile),
    (OBJECT, MTLStages::Object),
    (MESH, MTLStages::Mesh),
    (DISPATCH, MTLStages::Dispatch),
    (BLIT, MTLStages::Blit),
    (RESOURCE_STATE, MTLStages::ResourceState),
];

/// `mask` as `MTLStages`. Bits outside [`ALL`] are an error: a front end
/// that sends them is out of date with `proto`, and silently dropping a
/// stage would drop a barrier.
pub fn to_mtl_stages(mask: u16) -> Result<MTLStages> {
    if mask & !ALL != 0 {
        return Err(Error::BadArgument(format!(
            "stage mask {mask:#06x} has unknown bits"
        )));
    }
    let mut out = MTLStages(0);
    for (bit, stage) in STAGE_TABLE {
        if mask & bit != 0 {
            out |= stage;
        }
    }
    Ok(out)
}

/// `vis` as `MTL4VisibilityOptions`.
pub fn to_mtl_visibility(vis: u8) -> Result<MTL4VisibilityOptions> {
    if vis & !VIS_ALL != 0 {
        return Err(Error::BadArgument(format!(
            "visibility {vis:#04x} has unknown bits"
        )));
    }
    let mut out = MTL4VisibilityOptions::None;
    if vis & VIS_DEVICE != 0 {
        out |= MTL4VisibilityOptions::Device;
    }
    if vis & VIS_RESOURCE_ALIAS != 0 {
        out |= MTL4VisibilityOptions::ResourceAlias;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bit_maps_to_its_own_metal_stage() {
        let expect = [
            (VERTEX, 1u64 << 0),
            (FRAGMENT, 1 << 1),
            (TILE, 1 << 2),
            (OBJECT, 1 << 3),
            (MESH, 1 << 4),
            (DISPATCH, 1 << 27),
            (BLIT, 1 << 28),
            (RESOURCE_STATE, 1 << 26),
        ];
        for (bit, metal) in expect {
            assert_eq!(to_mtl_stages(bit).unwrap().0 as u64, metal, "bit {bit:#x}");
        }
        assert_eq!(STAGE_TABLE.len(), 8);
    }

    #[test]
    fn table_rows_are_distinct_single_bits_covering_all() {
        let mut seen = 0u16;
        for (bit, stage) in STAGE_TABLE {
            assert_eq!(bit.count_ones(), 1);
            assert_eq!(seen & bit, 0, "duplicate row");
            seen |= bit;
            assert_eq!((stage.0 as u64).count_ones(), 1);
        }
        assert_eq!(seen, ALL);
    }

    #[test]
    fn masks_combine_bitwise() {
        let m = to_mtl_stages(DISPATCH | BLIT).unwrap();
        assert_eq!(m, MTLStages::Dispatch | MTLStages::Blit);
        assert_eq!(to_mtl_stages(0).unwrap(), MTLStages(0));
        let all = to_mtl_stages(ALL).unwrap();
        for (_, stage) in STAGE_TABLE {
            assert!(all.contains(stage));
        }
        // `ALL` is the eight bits, not MTLStageAll (which would also name
        // acceleration-structure and ML stages we never use).
        assert_ne!(all, MTLStages::All);
    }

    #[test]
    fn unknown_bits_are_rejected() {
        assert!(to_mtl_stages(0x0100).is_err());
        assert!(to_mtl_stages(0x8001).is_err());
        assert!(to_mtl_visibility(0x04).is_err());
    }

    #[test]
    fn visibility_bits_map() {
        assert_eq!(to_mtl_visibility(0).unwrap(), MTL4VisibilityOptions::None);
        assert_eq!(
            to_mtl_visibility(VIS_DEVICE).unwrap(),
            MTL4VisibilityOptions::Device
        );
        assert_eq!(
            to_mtl_visibility(VIS_RESOURCE_ALIAS).unwrap(),
            MTL4VisibilityOptions::ResourceAlias
        );
        assert_eq!(
            to_mtl_visibility(VIS_ALL).unwrap(),
            MTL4VisibilityOptions::Device | MTL4VisibilityOptions::ResourceAlias
        );
    }

    #[test]
    fn encoder_stage_sets_do_not_overlap_and_stay_inside_all() {
        assert_eq!(COMPUTE_ENCODER_STAGES & RENDER_ENCODER_STAGES, 0);
        assert_eq!((COMPUTE_ENCODER_STAGES | RENDER_ENCODER_STAGES) & !ALL, 0);
    }
}
