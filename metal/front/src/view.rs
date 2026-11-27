//! Texture views and render-target records, in the terms the front keeps.
//!
//! A shader view (SRV, UAV) of a texture is written to the back as a
//! `ViewDesc`. A render-target or depth-stencil view has no descriptor
//! memory on the back: the front records `(resource, mip, slices, format)`
//! per descriptor slot and the commands that use it carry those numbers.
//! UAV views are recorded as well, because `ClearUnorderedAccessView*` names
//! a descriptor, not a resource range.

use victory_metal_proto::ResId;
use victory_metal_proto::types::{TexViewDim, ViewDesc, ViewKind, swizzle};

use crate::resource::{DIM_TEXTURE1D, DIM_TEXTURE2D, DIM_TEXTURE3D, ResourceDesc};

/// A texture SRV or UAV, reduced from the application's description.
/// Counts of 0 mean "to the end" (D3D12's `-1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureView {
    pub uav: bool,
    /// `None` for a null view.
    pub resource: Option<ResId>,
    /// The view's own format (never UNKNOWN: the caller resolves it).
    pub format: u32,
    pub dim: TexViewDim,
    pub plane: u8,
    /// `[R, G, B, A]` sources, `swizzle::*`.
    pub swizzle: [u8; 4],
    pub mip_first: u32,
    pub mip_count: u32,
    pub slice_first: u32,
    pub slice_count: u32,
    pub min_lod: f32,
}

impl TextureView {
    pub fn to_proto(&self) -> ViewDesc {
        let Some(res) = self.resource else {
            return ViewDesc::null();
        };
        let kind = if self.uav {
            ViewKind::TEXTURE_UAV
        } else {
            ViewKind::TEXTURE_SRV
        };
        let mut v = ViewDesc::texture(kind, res, self.format, self.dim);
        v.plane = self.plane;
        v.swizzle = self.swizzle;
        v.mip_first = self.mip_first;
        v.mip_count = self.mip_count;
        v.slice_first = self.slice_first;
        v.slice_count = self.slice_count;
        v.min_lod = self.min_lod;
        v
    }
}

/// `D3D12_SHADER_COMPONENT_MAPPING` (the `Shader4ComponentMapping` field):
/// 3 bits per component; 0..=3 pick a source component, 4 forces 0, 5 forces
/// 1. The numbers are the same as proto's `swizzle`.
pub fn swizzle_from_mapping(mapping: u32) -> [u8; 4] {
    let part = |i: u32| ((mapping >> (3 * i)) & 7).min(swizzle::ONE as u32) as u8;
    [part(0), part(1), part(2), part(3)]
}

/// A render-target or depth-stencil view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetView {
    pub res: ResId,
    /// The view's format (the resource's own when the application said UNKNOWN).
    pub format: u32,
    pub dim: TexViewDim,
    pub depth_stencil: bool,
    pub mip: u32,
    pub first_slice: u32,
    /// 0 = to the end (`-1` in D3D12).
    pub slice_count: u32,
    pub plane: u8,
    /// `DSV` flags: bit 0 depth read-only, bit 1 stencil read-only.
    pub read_only: u32,
}

/// What the front remembers about a descriptor slot besides what the back has.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Record {
    /// A texture UAV (for `ClearUnorderedAccessView*`).
    Uav(TextureView),
    Target(TargetView),
}

/// The view of a whole resource that a null view description means.
pub fn default_dim(desc: &ResourceDesc) -> TexViewDim {
    let array = desc.depth_or_array > 1 && desc.dimension != DIM_TEXTURE3D;
    let ms = desc.sample_count > 1;
    match (desc.dimension, array, ms) {
        (DIM_TEXTURE1D, false, _) => TexViewDim::TEX1D,
        (DIM_TEXTURE1D, true, _) => TexViewDim::TEX1D_ARRAY,
        (DIM_TEXTURE2D, false, false) => TexViewDim::TEX2D,
        (DIM_TEXTURE2D, true, false) => TexViewDim::TEX2D_ARRAY,
        (DIM_TEXTURE2D, false, true) => TexViewDim::TEX2D_MS,
        (DIM_TEXTURE2D, true, true) => TexViewDim::TEX2D_MS_ARRAY,
        (DIM_TEXTURE3D, _, _) => TexViewDim::TEX3D,
        _ => TexViewDim::NONE,
    }
}

/// The SRV/UAV of a whole texture, for a call without a description.
pub fn default_view(desc: &ResourceDesc, res: ResId, uav: bool) -> TextureView {
    TextureView {
        uav,
        resource: Some(res),
        format: desc.format,
        dim: default_dim(desc),
        plane: 0,
        swizzle: swizzle::IDENTITY,
        mip_first: 0,
        mip_count: if uav { 1 } else { 0 },
        slice_first: 0,
        slice_count: 0,
        min_lod: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_mapping_decodes() {
        // D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING = 0x1688.
        assert_eq!(swizzle_from_mapping(0x1688), swizzle::IDENTITY);
        // (A, A, A, ONE-ish): 3,3,3,5
        let m = 3 | (3 << 3) | (3 << 6) | (5 << 9);
        assert_eq!(swizzle_from_mapping(m), [3, 3, 3, 5]);
        assert_eq!(
            swizzle_from_mapping(4 | (1 << 3) | (4 << 6) | (4 << 9))[0],
            4
        );
    }

    #[test]
    fn default_views_follow_the_resource() {
        let mut d = ResourceDesc::buffer(1, 0);
        d.dimension = DIM_TEXTURE2D;
        d.depth_or_array = 1;
        assert_eq!(default_dim(&d), TexViewDim::TEX2D);
        d.depth_or_array = 6;
        assert_eq!(default_dim(&d), TexViewDim::TEX2D_ARRAY);
        d.sample_count = 4;
        assert_eq!(default_dim(&d), TexViewDim::TEX2D_MS_ARRAY);
        d.dimension = DIM_TEXTURE3D;
        assert_eq!(default_dim(&d), TexViewDim::TEX3D);
    }
}
