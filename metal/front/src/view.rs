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

use crate::format;
use crate::hresult::{Hr, Result};
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
    /// A null view (the application gave no resource).
    pub fn null(uav: bool) -> Self {
        TextureView {
            uav,
            resource: None,
            format: 0,
            dim: TexViewDim::NONE,
            plane: 0,
            swizzle: swizzle::IDENTITY,
            mip_first: 0,
            mip_count: 0,
            slice_first: 0,
            slice_count: 0,
            min_lod: 0.0,
        }
    }

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

/// What the application's SRV/UAV/RTV/DSV description says, flattened by the
/// shell: the union member it read fills the fields that member has, the
/// rest are 0 (counts that are `-1` in D3D12 arrive as `u32::MAX`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewParams {
    /// The `D3D12_*_VIEW_DIMENSION` number (see [`srv`], [`uav`], [`rtv`], [`dsv`]).
    pub dimension: u32,
    /// UNKNOWN (0) = the resource's own format.
    pub format: u32,
    /// `Shader4ComponentMapping` (SRV).
    pub mapping: u32,
    /// `MostDetailedMip` or `MipSlice`.
    pub mip_first: u32,
    /// `MipLevels` (SRV).
    pub mip_count: u32,
    /// `FirstArraySlice`, `First2DArrayFace` or `FirstWSlice`.
    pub first_slice: u32,
    /// `ArraySize`, `NumCubes` or `WSize`.
    pub slice_count: u32,
    pub plane: u32,
    pub min_lod: f32,
    /// DSV flags.
    pub flags: u32,
}

const ALL: u32 = u32::MAX;

fn all_to_zero(n: u32) -> u32 {
    if n == ALL { 0 } else { n }
}

/// Resource kinds a view dimension may look at.
fn fits(rd: &ResourceDesc, dim: TexViewDim) -> bool {
    let ms = rd.sample_count > 1;
    match dim {
        TexViewDim::TEX1D | TexViewDim::TEX1D_ARRAY => rd.dimension == DIM_TEXTURE1D,
        TexViewDim::TEX2D | TexViewDim::TEX2D_ARRAY => rd.dimension == DIM_TEXTURE2D,
        TexViewDim::TEX2D_MS | TexViewDim::TEX2D_MS_ARRAY => rd.dimension == DIM_TEXTURE2D && ms,
        TexViewDim::CUBE | TexViewDim::CUBE_ARRAY => rd.dimension == DIM_TEXTURE2D && !ms,
        TexViewDim::TEX3D => rd.dimension == DIM_TEXTURE3D,
        _ => false,
    }
}

fn resolve_format(rd: &ResourceDesc, f: u32) -> Result<u32> {
    let f = if f == format::UNKNOWN { rd.format } else { f };
    if format::info(f).is_none() {
        return Err(Hr::E_INVALIDARG);
    }
    Ok(f)
}

fn check_range(
    rd: &ResourceDesc,
    mip_first: u32,
    mip_count: u32,
    first: u32,
    count: u32,
) -> Result<()> {
    let mips = rd.mips as u32;
    let slices = if rd.dimension == DIM_TEXTURE3D {
        rd.depth().max(1)
    } else {
        rd.array_size()
    };
    if mip_first >= mips || mip_count > mips - mip_first || first > slices || count > slices - first
    {
        return Err(Hr::E_INVALIDARG);
    }
    Ok(())
}

/// A shader resource view: `D3D12_SRV_DIMENSION` 2 = 1D, 3 = 1D array, 4 = 2D,
/// 5 = 2D array, 6 = 2D MS, 7 = 2D MS array, 8 = 3D, 9 = cube, 10 = cube array.
pub fn srv(res: ResId, rd: &ResourceDesc, p: &ViewParams) -> Result<TextureView> {
    let (dim, array) = match p.dimension {
        2 => (TexViewDim::TEX1D, false),
        3 => (TexViewDim::TEX1D_ARRAY, true),
        4 => (TexViewDim::TEX2D, false),
        5 => (TexViewDim::TEX2D_ARRAY, true),
        6 => (TexViewDim::TEX2D_MS, false),
        7 => (TexViewDim::TEX2D_MS_ARRAY, true),
        8 => (TexViewDim::TEX3D, false),
        9 => (TexViewDim::CUBE, false),
        10 => (TexViewDim::CUBE_ARRAY, true),
        _ => return Err(Hr::E_INVALIDARG),
    };
    if !fits(rd, dim) || rd.is_buffer() {
        return Err(Hr::E_INVALIDARG);
    }
    let fmt = resolve_format(rd, p.format)?;
    let mips = all_to_zero(p.mip_count);
    let (first, count) = match dim {
        TexViewDim::TEX3D => (0, 0),
        TexViewDim::CUBE => (0, 6),
        TexViewDim::CUBE_ARRAY => (p.first_slice, p.slice_count.saturating_mul(6)),
        _ if array => (p.first_slice, all_to_zero(p.slice_count)),
        _ => (0, 1),
    };
    if dim == TexViewDim::CUBE_ARRAY && p.slice_count == ALL {
        return Err(Hr::E_INVALIDARG);
    }
    check_range(rd, p.mip_first, mips, first, count)?;
    if matches!(dim, TexViewDim::CUBE | TexViewDim::CUBE_ARRAY) && rd.array_size() < first + count {
        return Err(Hr::E_INVALIDARG);
    }
    Ok(TextureView {
        uav: false,
        resource: Some(res),
        format: fmt,
        dim,
        plane: p.plane as u8,
        swizzle: swizzle_from_mapping(p.mapping),
        mip_first: p.mip_first,
        mip_count: mips,
        slice_first: first,
        slice_count: count,
        min_lod: p.min_lod,
    })
}

/// An unordered access view: `D3D12_UAV_DIMENSION` 2 = 1D, 3 = 1D array, 4 = 2D,
/// 5 = 2D array, 8 = 3D.
pub fn uav(res: ResId, rd: &ResourceDesc, p: &ViewParams) -> Result<TextureView> {
    let (dim, array) = match p.dimension {
        2 => (TexViewDim::TEX1D, false),
        3 => (TexViewDim::TEX1D_ARRAY, true),
        4 => (TexViewDim::TEX2D, false),
        5 => (TexViewDim::TEX2D_ARRAY, true),
        8 => (TexViewDim::TEX3D, false),
        _ => return Err(Hr::E_INVALIDARG),
    };
    if !fits(rd, dim) || rd.is_buffer() {
        return Err(Hr::E_INVALIDARG);
    }
    let fmt = resolve_format(rd, p.format)?;
    let (first, count) = match dim {
        TexViewDim::TEX3D => (p.first_slice, all_to_zero(p.slice_count)),
        _ if array => (p.first_slice, all_to_zero(p.slice_count)),
        _ => (0, 1),
    };
    check_range(rd, p.mip_first, 1, first, count)?;
    Ok(TextureView {
        uav: true,
        resource: Some(res),
        format: fmt,
        dim,
        plane: p.plane as u8,
        swizzle: swizzle::IDENTITY,
        mip_first: p.mip_first,
        mip_count: 1,
        slice_first: first,
        slice_count: count,
        min_lod: 0.0,
    })
}

fn target(
    res: ResId,
    rd: &ResourceDesc,
    p: &ViewParams,
    dim: TexViewDim,
    array: bool,
    depth_stencil: bool,
) -> Result<TargetView> {
    if !fits(rd, dim) || rd.is_buffer() {
        return Err(Hr::E_INVALIDARG);
    }
    let fmt = resolve_format(rd, p.format)?;
    let (first, count) = if array || dim == TexViewDim::TEX3D {
        (p.first_slice, all_to_zero(p.slice_count))
    } else {
        (0, 1)
    };
    check_range(rd, p.mip_first, 1, first, count)?;
    Ok(TargetView {
        res,
        format: fmt,
        dim,
        depth_stencil,
        mip: p.mip_first,
        first_slice: first,
        slice_count: target_slices(rd, p.mip_first, first, count),
        plane: p.plane as u8,
        read_only: if depth_stencil { p.flags } else { 0 },
    })
}

/// The slices a target view renders to: "to the end" (0) is resolved here,
/// because a render pass needs the number.
fn target_slices(rd: &ResourceDesc, mip: u32, first: u32, count: u32) -> u32 {
    if count != 0 {
        return count;
    }
    let slices = if rd.dimension == DIM_TEXTURE3D {
        (rd.depth().max(1) >> mip).max(1)
    } else {
        rd.array_size()
    };
    slices.saturating_sub(first).max(1)
}

/// A render target view: `D3D12_RTV_DIMENSION` 2 = 1D, 3 = 1D array, 4 = 2D,
/// 5 = 2D array, 6 = 2D MS, 7 = 2D MS array, 8 = 3D.
pub fn rtv(res: ResId, rd: &ResourceDesc, p: &ViewParams) -> Result<TargetView> {
    let (dim, array) = match p.dimension {
        2 => (TexViewDim::TEX1D, false),
        3 => (TexViewDim::TEX1D_ARRAY, true),
        4 => (TexViewDim::TEX2D, false),
        5 => (TexViewDim::TEX2D_ARRAY, true),
        6 => (TexViewDim::TEX2D_MS, false),
        7 => (TexViewDim::TEX2D_MS_ARRAY, true),
        8 => (TexViewDim::TEX3D, false),
        _ => return Err(Hr::E_INVALIDARG),
    };
    target(res, rd, p, dim, array, false)
}

/// A depth-stencil view: `D3D12_DSV_DIMENSION` 1 = 1D, 2 = 1D array, 3 = 2D,
/// 4 = 2D array, 5 = 2D MS, 6 = 2D MS array. (A different numbering from RTV.)
pub fn dsv(res: ResId, rd: &ResourceDesc, p: &ViewParams) -> Result<TargetView> {
    let (dim, array) = match p.dimension {
        1 => (TexViewDim::TEX1D, false),
        2 => (TexViewDim::TEX1D_ARRAY, true),
        3 => (TexViewDim::TEX2D, false),
        4 => (TexViewDim::TEX2D_ARRAY, true),
        5 => (TexViewDim::TEX2D_MS, false),
        6 => (TexViewDim::TEX2D_MS_ARRAY, true),
        _ => return Err(Hr::E_INVALIDARG),
    };
    target(res, rd, p, dim, array, true)
}

/// A render-target or depth-stencil view of the whole resource (no description given).
pub fn default_target(res: ResId, rd: &ResourceDesc, depth_stencil: bool) -> Result<TargetView> {
    let dim = default_dim(rd);
    if dim == TexViewDim::NONE || rd.is_buffer() {
        return Err(Hr::E_INVALIDARG);
    }
    Ok(TargetView {
        res,
        format: resolve_format(rd, format::UNKNOWN)?,
        dim,
        depth_stencil,
        mip: 0,
        first_slice: 0,
        slice_count: target_slices(rd, 0, 0, 0),
        plane: 0,
        read_only: 0,
    })
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

    fn tex(
        dim: u32,
        w: u64,
        h: u32,
        arr: u16,
        mips: u16,
        format: u32,
        samples: u32,
    ) -> ResourceDesc {
        ResourceDesc {
            dimension: dim,
            alignment: 0,
            width: w,
            height: h,
            depth_or_array: arr,
            mips,
            format,
            sample_count: samples,
            sample_quality: 0,
            layout: 0,
            flags: 0,
        }
    }

    #[test]
    fn srv_descriptions_become_ranges() {
        let r = ResId(5);
        let rd = tex(DIM_TEXTURE2D, 64, 64, 12, 7, 27, 1);
        let base = ViewParams {
            dimension: 5,
            format: 28,
            mapping: 0x1688,
            mip_first: 1,
            mip_count: ALL,
            first_slice: 2,
            slice_count: ALL,
            ..Default::default()
        };
        let v = srv(r, &rd, &base).unwrap();
        assert_eq!(
            (v.dim, v.format, v.mip_first, v.mip_count),
            (TexViewDim::TEX2D_ARRAY, 28, 1, 0)
        );
        assert_eq!((v.slice_first, v.slice_count), (2, 0));
        // A plain 2D view of an array resource sees slice 0.
        let v = srv(
            r,
            &rd,
            &ViewParams {
                dimension: 4,
                ..base
            },
        )
        .unwrap();
        assert_eq!(
            (v.dim, v.slice_first, v.slice_count),
            (TexViewDim::TEX2D, 0, 1)
        );
        // Cube and cube array.
        let v = srv(
            r,
            &rd,
            &ViewParams {
                dimension: 9,
                ..base
            },
        )
        .unwrap();
        assert_eq!(
            (v.dim, v.slice_first, v.slice_count),
            (TexViewDim::CUBE, 0, 6)
        );
        let v = srv(
            r,
            &rd,
            &ViewParams {
                dimension: 10,
                first_slice: 6,
                slice_count: 1,
                ..base
            },
        )
        .unwrap();
        assert_eq!(
            (v.dim, v.slice_first, v.slice_count),
            (TexViewDim::CUBE_ARRAY, 6, 6)
        );
        // Unknown format = the resource's, which is typeless here: allowed to the
        // front, the back decides; format 0 on an unknown resource is not.
        let v = srv(r, &rd, &ViewParams { format: 0, ..base }).unwrap();
        assert_eq!(v.format, 27);
        // Refusals: wrong dimension for the resource, past the end, buffer.
        assert!(
            srv(
                r,
                &rd,
                &ViewParams {
                    dimension: 8,
                    ..base
                }
            )
            .is_err()
        );
        assert!(
            srv(
                r,
                &rd,
                &ViewParams {
                    dimension: 2,
                    ..base
                }
            )
            .is_err()
        );
        assert!(
            srv(
                r,
                &rd,
                &ViewParams {
                    mip_first: 7,
                    ..base
                }
            )
            .is_err()
        );
        assert!(
            srv(
                r,
                &rd,
                &ViewParams {
                    first_slice: 13,
                    ..base
                }
            )
            .is_err()
        );
        assert!(
            srv(
                r,
                &rd,
                &ViewParams {
                    first_slice: 10,
                    slice_count: 3,
                    ..base
                }
            )
            .is_err()
        );
        assert!(
            srv(
                r,
                &rd,
                &ViewParams {
                    dimension: 6,
                    ..base
                }
            )
            .is_err(),
            "not multisampled"
        );
        assert!(srv(r, &ResourceDesc::buffer(16, 0), &base).is_err());
        assert!(
            srv(
                r,
                &rd,
                &ViewParams {
                    dimension: 99,
                    ..base
                }
            )
            .is_err()
        );
        // 3D.
        let vol = tex(DIM_TEXTURE3D, 16, 16, 16, 5, 28, 1);
        let v = srv(
            r,
            &vol,
            &ViewParams {
                dimension: 8,
                mip_count: 2,
                ..base
            },
        )
        .unwrap();
        assert_eq!(
            (v.dim, v.slice_count, v.mip_count),
            (TexViewDim::TEX3D, 0, 2)
        );
    }

    #[test]
    fn uav_and_target_descriptions() {
        let r = ResId(5);
        let rd = tex(DIM_TEXTURE2D, 64, 64, 4, 3, 28, 1);
        let p = ViewParams {
            dimension: 5,
            mip_first: 2,
            first_slice: 1,
            slice_count: ALL,
            ..Default::default()
        };
        let v = uav(r, &rd, &p).unwrap();
        assert!(v.uav);
        assert_eq!(
            (v.mip_first, v.mip_count, v.slice_first, v.slice_count),
            (2, 1, 1, 0)
        );
        assert_eq!(v.swizzle, swizzle::IDENTITY);
        assert!(uav(r, &rd, &ViewParams { dimension: 8, ..p }).is_err());
        assert!(uav(r, &rd, &ViewParams { mip_first: 3, ..p }).is_err());
        let vol = tex(DIM_TEXTURE3D, 16, 16, 16, 3, 28, 1);
        let v = uav(
            r,
            &vol,
            &ViewParams {
                dimension: 8,
                mip_first: 1,
                first_slice: 4,
                slice_count: 4,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            (v.dim, v.slice_first, v.slice_count),
            (TexViewDim::TEX3D, 4, 4)
        );
        // RTV numbering: 4 = 2D; DSV numbering: 3 = 2D.
        let t = rtv(
            r,
            &rd,
            &ViewParams {
                dimension: 4,
                mip_first: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            (t.dim, t.mip, t.first_slice, t.slice_count, t.depth_stencil),
            (TexViewDim::TEX2D, 1, 0, 1, false)
        );
        let d = dsv(
            r,
            &rd,
            &ViewParams {
                dimension: 3,
                flags: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            (d.dim, d.depth_stencil, d.read_only),
            (TexViewDim::TEX2D, true, 1)
        );
        let d = dsv(
            r,
            &rd,
            &ViewParams {
                dimension: 4,
                first_slice: 1,
                slice_count: 2,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            (d.dim, d.first_slice, d.slice_count),
            (TexViewDim::TEX2D_ARRAY, 1, 2)
        );
        assert!(
            dsv(
                r,
                &rd,
                &ViewParams {
                    dimension: 8,
                    ..Default::default()
                }
            )
            .is_err()
        );
        let w = default_target(r, &rd, false).unwrap();
        assert_eq!(
            (w.dim, w.slice_count, w.format),
            (TexViewDim::TEX2D_ARRAY, 4, 28)
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
