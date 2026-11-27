//! `GetCopyableFootprints` and friends. Checked byte for byte against
//! D3DMetal on Wine (tests/golden/d3dmetal-wine11.txt) for buffers, 1D/2D/3D
//! textures, mips, arrays, block-compressed formats, and a non-zero base.
//!
//! Layout rules (as D3D12 and D3DMetal give them):
//!
//! * a buffer is one row: `row_size = width`, pitch aligned to 256;
//! * a texture subresource is `rows x depth` rows of `row_size` bytes, the
//!   pitch is `row_size` aligned to 256 (`D3D12_TEXTURE_DATA_PITCH_ALIGNMENT`);
//!   block-compressed footprints are rounded up to whole blocks;
//! * each subresource starts at 512 bytes (`..._PLACEMENT_ALIGNMENT`) after
//!   the end of the previous one, counted from `base`;
//! * `total` is the end of the last row of the last subresource, counted
//!   from `base`.

use crate::format;
use crate::hresult::{Hr, Result};
use crate::resource::ResourceDesc;

pub const PITCH_ALIGNMENT: u64 = 256;
pub const PLACEMENT_ALIGNMENT: u64 = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Footprint {
    pub offset: u64,
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub row_pitch: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubresourceLayout {
    pub footprint: Footprint,
    pub num_rows: u32,
    /// Bytes of one row without the padding.
    pub row_size: u64,
}

fn align(v: u64, a: u64) -> u64 {
    v.div_ceil(a) * a
}

/// Mip / array / plane of a subresource index (`D3D12CalcSubresource`
/// inverted).
pub fn decompose(desc: &ResourceDesc, index: u32) -> (u32, u32, u32) {
    let mips = desc.mips as u32;
    let arr = desc.array_size();
    (index % mips, (index / mips) % arr, index / (mips * arr))
}

pub fn calc_subresource(mip: u32, array: u32, plane: u32, mips: u32, arrays: u32) -> u32 {
    mip + array * mips + plane * mips * arrays
}

/// Layout of one texture subresource without an offset.
fn texture_layout(desc: &ResourceDesc, index: u32) -> Result<SubresourceLayout> {
    let (mip, _arr, plane_index) = decompose(desc, index);
    let p = format::plane(desc.format, plane_index).ok_or(Hr::E_INVALIDARG)?;
    let (bw, bh) = (p.block_w as u32, p.block_h as u32);
    let w = ((desc.width >> mip).max(1) as u32).div_ceil(1 << p.shift_x);
    let h = (desc.height >> mip).max(1).div_ceil(1 << p.shift_y);
    let d = (desc.depth() >> mip).max(1);
    // The footprint is in whole blocks of texels.
    let fw = w.div_ceil(bw) * bw;
    let fh = h.div_ceil(bh) * bh;
    let row_size = (fw / bw) as u64 * p.bytes as u64;
    let rows = fh / bh;
    let pitch = align(row_size, PITCH_ALIGNMENT);
    if pitch > u32::MAX as u64 {
        return Err(Hr::E_INVALIDARG);
    }
    Ok(SubresourceLayout {
        footprint: Footprint {
            offset: 0,
            format: p.format,
            width: fw,
            height: fh,
            depth: d,
            row_pitch: pitch as u32,
        },
        num_rows: rows,
        row_size,
    })
}

/// Footprints of subresources `first .. first + count`, placed from `base`.
/// Returns them with the total size. An error means the caller fills its
/// outputs with all-ones, as D3D12 does.
pub fn copyable_footprints(
    desc: &ResourceDesc,
    first: u32,
    count: u32,
    base: u64,
) -> Result<(Vec<SubresourceLayout>, u64)> {
    if desc.is_buffer() {
        if first != 0 || count != 1 {
            return Err(Hr::E_INVALIDARG);
        }
        let pitch = align(desc.width, PITCH_ALIGNMENT);
        let l = SubresourceLayout {
            footprint: Footprint {
                offset: base,
                format: format::UNKNOWN,
                width: desc.width.min(u32::MAX as u64) as u32,
                height: 1,
                depth: 1,
                row_pitch: pitch.min(u32::MAX as u64) as u32,
            },
            num_rows: 1,
            row_size: desc.width,
        };
        return Ok((vec![l], desc.width));
    }
    let total_subs = desc.subresource_count();
    if desc.mips == 0 || first.checked_add(count).is_none_or(|e| e > total_subs) {
        return Err(Hr::E_INVALIDARG);
    }
    let mut out = Vec::with_capacity(count as usize);
    let mut rel = 0u64; // offset from base of the next subresource, aligned
    let mut total = 0u64;
    for i in first..first + count {
        let mut l = texture_layout(desc, i)?;
        rel = align(rel, PLACEMENT_ALIGNMENT);
        l.footprint.offset = base + rel;
        let pitch = l.footprint.row_pitch as u64;
        let slice = l.num_rows as u64 * pitch;
        let last_end = (l.footprint.depth as u64 - 1) * slice + (l.num_rows as u64 - 1) * pitch + l.row_size;
        total = rel + last_end;
        rel += l.footprint.depth as u64 * slice;
        out.push(l);
    }
    Ok((out, total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::*;

    fn tex(dim: u32, w: u64, h: u32, depth: u16, mips: u16, fmt: u32) -> ResourceDesc {
        ResourceDesc {
            dimension: dim,
            alignment: 0,
            width: w,
            height: h,
            depth_or_array: depth,
            mips,
            format: fmt,
            sample_count: 1,
            sample_quality: 0,
            layout: LAYOUT_UNKNOWN,
            flags: 0,
        }
    }

    fn line(name: &str) -> String {
        // "GOLD footprints NAME: ..." from the D3DMetal run.
        let text = include_str!("../tests/golden/d3dmetal-wine11.txt");
        let key = format!("GOLD footprints {name}: ");
        text.lines().find_map(|l| l.strip_prefix(&key)).unwrap_or_else(|| panic!("no golden line {name}")).to_string()
    }

    fn render(layouts: &[SubresourceLayout], total: u64) -> String {
        let mut s = String::new();
        for l in layouts {
            let f = &l.footprint;
            s += &format!(
                "[off={} fmt={} {}x{}x{} pitch={} rows={} rowsize={}]",
                f.offset, f.format, f.width, f.height, f.depth, f.row_pitch, l.num_rows, l.row_size
            );
        }
        format!("{s} total={total}")
    }

    fn check(name: &str, desc: ResourceDesc, first: u32, n: u32, base: u64) {
        let (l, total) = copyable_footprints(&desc, first, n, base).unwrap();
        assert_eq!(render(&l, total), line(name), "{name}");
    }

    #[test]
    fn matches_d3dmetal() {
        let t2 = DIM_TEXTURE2D;
        let mut buf = ResourceDesc::buffer(1000, 0);
        check("buffer 1000", buf, 0, 1, 0);
        buf.width = 65536;
        check("buffer 65536 base 512", buf, 0, 1, 512);
        check("rgba8 64x64 1 mip", tex(t2, 64, 64, 1, 1, 28), 0, 1, 0);
        check("rgba8 4x4 1 mip", tex(t2, 4, 4, 1, 1, 28), 0, 1, 0);
        check("rgba8 64x64 7 mips", tex(t2, 64, 64, 1, 7, 28), 0, 7, 0);
        check("rgba8 100x37 6 mips x3 layers", tex(t2, 100, 37, 3, 6, 28), 0, 18, 0);
        check("rgba8 100x37 6 mips x3 layers, subs 4..9 base 768", tex(t2, 100, 37, 3, 6, 28), 4, 5, 768);
        check("rgba32f 33x5 3 mips", tex(t2, 33, 5, 1, 3, 2), 0, 3, 0);
        check("rgb32f 17x3", tex(t2, 17, 3, 1, 1, 6), 0, 1, 0);
        check("r8 5x5 3 mips", tex(t2, 5, 5, 1, 3, 61), 0, 3, 0);
        check("r16 300x2", tex(t2, 300, 2, 1, 1, 57), 0, 1, 0);
        check("bc1 13x7 4 mips", tex(t2, 13, 7, 1, 4, 71), 0, 4, 0);
        check("bc7 64x64 7 mips", tex(t2, 64, 64, 1, 7, 98), 0, 7, 0);
        check("bc3 1x1", tex(t2, 1, 1, 1, 1, 77), 0, 1, 0);
        check("3d rgba8 16x16x8 3 mips", tex(DIM_TEXTURE3D, 16, 16, 8, 3, 28), 0, 3, 0);
        check("3d r32 5x6x7 3 mips", tex(DIM_TEXTURE3D, 5, 6, 7, 3, 41), 0, 3, 0);
        check("1d rgba8 100 4 mips x2", tex(DIM_TEXTURE1D, 100, 1, 2, 4, 28), 0, 8, 0);
        check("d32 64x64", tex(t2, 64, 64, 1, 1, 40), 0, 1, 0);
        check("r10g10b10a2 9x9 2 mips", tex(t2, 9, 9, 1, 2, 24), 0, 2, 0);
    }

    #[test]
    fn planar_formats_have_one_footprint_per_plane() {
        // NV12 is the same as D3DMetal gives it; depth-stencil planes are
        // shown in the smoke test but not compared (D3DMetal pads them).
        let (l, total) = copyable_footprints(&tex(DIM_TEXTURE2D, 64, 48, 1, 1, 103), 0, 2, 0).unwrap();
        assert_eq!(render(&l, total), "[off=0 fmt=60 64x48x1 pitch=256 rows=48 rowsize=64][off=12288 fmt=48 32x24x1 pitch=256 rows=24 rowsize=64] total=18240");
        let (l, _) = copyable_footprints(&tex(DIM_TEXTURE2D, 33, 17, 1, 2, 20), 0, 4, 0).unwrap();
        assert_eq!((l[0].footprint.format, l[2].footprint.format), (39, 60));
        assert_eq!(l[2].row_size, 33);
    }

    #[test]
    fn out_of_range_is_refused() {
        let d = tex(DIM_TEXTURE2D, 64, 64, 1, 7, 28);
        assert!(copyable_footprints(&d, 5, 3, 0).is_err());
        assert!(copyable_footprints(&d, u32::MAX, 2, 0).is_err());
        assert!(copyable_footprints(&ResourceDesc::buffer(16, 0), 1, 1, 0).is_err());
    }
}
