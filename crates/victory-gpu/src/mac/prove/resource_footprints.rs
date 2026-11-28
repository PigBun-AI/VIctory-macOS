//! C-V-014 / C-V-003 (docs/mac/PATCHES.md G12): `GetCopyableFootprints` and `GetResourceAllocationInfo` of the title's
//! device against an independent model of the D3D12 rules, and (on a host GPU) against the host device's own answers.
//!
//! Reference: D3D12 functional spec and the `ID3D12Device::GetCopyableFootprints` documentation: each subresource's
//! footprint starts at an offset aligned to `D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT` (512) from `BaseOffset`; `RowPitch`
//! is the row size aligned to `D3D12_TEXTURE_DATA_PITCH_ALIGNMENT` (256); `NumRows` is in block rows; `RowSizeInBytes`
//! is unpadded; the total ends at the last row's unpadded size. `GetResourceAllocationInfo`: buffers 64 KiB aligned,
//! multisampled textures 4 MiB, sizes a multiple of the alignment; resources placed one after another.
use super::*;

const BC: [u32; 15] = [70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84];

fn desc(dimension: u32, format: u32, width: u64, height: u32, depth: u16, mips: u16) -> ResourceDesc {
    ResourceDesc {
        dimension,
        alignment: 0,
        width,
        height,
        depth_or_array_size: depth,
        mip_levels: mips,
        format,
        sample_count: 1,
        sample_quality: 0,
        layout: 1,
        flags: 0,
    }
}

/// What the spec says for subresource `i` of a colour texture whose rows are `row` bytes of `rows` block rows of `depth` slices.
/// `base`: where the previous ended. Returns (offset, pitch, rows, row bytes, end).
fn spec_one(base: u64, row: u64, rows: u64, depth: u64) -> (u64, u64, u64, u64, u64) {
    let offset = base.next_multiple_of(512);
    let pitch = row.next_multiple_of(256);
    let end = offset + pitch * (rows * depth - 1) + row;
    (offset, pitch, rows, row, end)
}

fn cases() -> Vec<ResourceDesc> {
    let mut v = Vec::new();
    for (format, name) in [(28u32, "RGBA8"), (2, "RGBA32F"), (10, "RGBA16F"), (54, "R16F"), (61, "R8"), (41, "R32F"), (35, "RG16"), (87, "BGRA8"), (24, "R10G10B10A2"), (11, "RGBA16"), (49, "RG8"), (85, "B5G6R5"), (86, "B5G5R5A1")] {
        let _ = name;
        v.push(desc(3, format, 1, 1, 1, 1));
        v.push(desc(3, format, 7, 5, 1, 3));
        v.push(desc(3, format, 256, 256, 1, 9));
        v.push(desc(3, format, 100, 33, 4, 3));
        v.push(desc(3, format, 64, 64, 6, 7));
        v.push(desc(2, format, 33, 1, 1, 6));
        v.push(desc(2, format, 100, 1, 3, 2));
        v.push(desc(4, format, 16, 8, 4, 3));
        v.push(desc(4, format, 5, 7, 9, 4));
        v.push(desc(4, format, 64, 64, 64, 7));
    }
    for format in BC {
        v.push(desc(3, format, 1, 1, 1, 1));
        v.push(desc(3, format, 5, 7, 1, 3));
        v.push(desc(3, format, 128, 128, 1, 8));
        v.push(desc(3, format, 100, 33, 3, 7));
        v.push(desc(4, format, 16, 8, 6, 3));
    }
    v
}

fn ours(d: &ResourceDesc) -> (Vec<PlacedFootprint>, Vec<u32>, Vec<u64>, u64) {
    let n = u32::from(d.mip_levels) * if d.dimension == TEXTURE_3D { 1 } else { u32::from(d.depth_or_array_size) };
    let mut f = vec![NO_FOOTPRINT; n as usize];
    let mut rows = vec![0u32; n as usize];
    let mut sizes = vec![0u64; n as usize];
    let mut total = 0u64;
    copyable_footprints(std::ptr::null(), d, 0, n, 0, f.as_mut_ptr(), rows.as_mut_ptr(), sizes.as_mut_ptr(), &mut total);
    (f, rows, sizes, total)
}

#[test]
fn footprints_are_what_the_spec_says_for_colour_textures() {
    let mut bad = Vec::new();
    for d in cases() {
        let (f, rows, sizes, total) = ours(&d);
        let (element, block) = element(d.format).unwrap();
        let mips = u32::from(d.mip_levels);
        let mut end = 0u64;
        for i in 0..f.len() as u32 {
            let mip = i % mips;
            let w = (d.width >> mip).max(1).next_multiple_of(block);
            let h = (u64::from(d.height) >> mip).max(1).next_multiple_of(block);
            let depth = if d.dimension == TEXTURE_3D { (u64::from(d.depth_or_array_size) >> mip).max(1) } else { 1 };
            let (offset, pitch, nrows, row, e) = spec_one(end, w / block * element, h / block, depth);
            end = e;
            let got = (u64::from(f[i as usize].offset), u64::from(f[i as usize].row_pitch), u64::from(rows[i as usize]), sizes[i as usize]);
            let want = (offset, pitch, nrows, row);
            let shape = (f[i as usize].width, f[i as usize].height, f[i as usize].depth);
            let want_shape = (w as u32, h as u32, depth as u32);
            if got != want || shape != want_shape || f[i as usize].format != d.format {
                bad.push(format!("dim {} fmt {} {}x{}x{} mips {} sub {i}: got (offset, pitch, rows, row) {got:?} {shape:?}, spec {want:?} {want_shape:?}", d.dimension, d.format, d.width, d.height, d.depth_or_array_size, d.mip_levels));
            }
        }
        if total != end {
            bad.push(format!("dim {} fmt {} {}x{}x{}: total {total}, spec {end}", d.dimension, d.format, d.width, d.height, d.depth_or_array_size));
        }
    }
    assert!(bad.is_empty(), "{} differences, first:\n{}", bad.len(), bad.iter().take(10).cloned().collect::<Vec<_>>().join("\n"));
}

#[test]
fn a_base_offset_moves_the_whole_layout_by_its_alignment() {
    let d = desc(3, 28, 100, 33, 2, 3);
    let n = 6;
    let (mut a, mut b) = (vec![NO_FOOTPRINT; n], vec![NO_FOOTPRINT; n]);
    let (mut ta, mut tb) = (0u64, 0u64);
    copyable_footprints(std::ptr::null(), &d, 0, n as u32, 0, a.as_mut_ptr(), std::ptr::null_mut(), std::ptr::null_mut(), &mut ta);
    copyable_footprints(std::ptr::null(), &d, 0, n as u32, 1000, b.as_mut_ptr(), std::ptr::null_mut(), std::ptr::null_mut(), &mut tb);
    // the first starts at the next 512 from the base: 1024; the others keep their distances
    assert_eq!(b[0].offset, 1024);
    for i in 1..n {
        assert_eq!(b[i].offset - b[0].offset, a[i].offset - a[0].offset);
    }
    assert_eq!(tb, ta + 24, "the total is measured from the base: the 24 bytes of alignment padding are in it");
    // a middle range of subresources starts at its own first
    let mut m = [NO_FOOTPRINT; 2];
    let mut tm = 0;
    copyable_footprints(std::ptr::null(), &d, 2, 2, 0, m.as_mut_ptr(), std::ptr::null_mut(), std::ptr::null_mut(), &mut tm);
    assert_eq!(m[0].offset, 0);
    assert_eq!(m[0].width, 25);
}

#[test]
fn unknown_formats_and_subresources_give_the_documented_all_ones() {
    let d = desc(3, 0, 8, 8, 1, 1);
    let mut f = [NO_FOOTPRINT; 1];
    let (mut rows, mut sizes, mut total) = ([0u32; 1], [0u64; 1], 0u64);
    copyable_footprints(std::ptr::null(), &d, 0, 1, 0, f.as_mut_ptr(), rows.as_mut_ptr(), sizes.as_mut_ptr(), &mut total);
    assert_eq!((rows[0], sizes[0], total, f[0].offset), (u32::MAX, u64::MAX, u64::MAX, u64::MAX));
}

#[test]
fn allocation_info_aligns_and_places_in_order() {
    let infos = |ds: &[ResourceDesc]| {
        let mut out = AllocationInfo { size: 0, alignment: 0 };
        let mut each: Vec<AllocationInfo1> = ds.iter().map(|_| AllocationInfo1 { offset: 0, alignment: 0, size: 0 }).collect();
        let wrapped: Vec<ResourceDesc1> = ds.iter().map(|d| ResourceDesc1 { desc: *d, mip_region: [0; 3] }).collect();
        allocation_info2(&mut out, std::ptr::null(), 0, ds.len() as u32, wrapped.as_ptr(), each.as_mut_ptr());
        let mut plain = AllocationInfo { size: 0, alignment: 0 };
        allocation_info(&mut plain, std::ptr::null(), 0, ds.len() as u32, ds.as_ptr());
        assert_eq!(out, plain, "the plain and the newer call agree");
        (out, each)
    };
    // a buffer of 100 bytes: 64 KiB, aligned 64 KiB
    let (one, _) = infos(&[desc(BUFFER, 0, 100, 1, 1, 1)]);
    assert_eq!(one, AllocationInfo { size: 65536, alignment: 65536 });
    // buffers one after another: each at the next multiple of 64 KiB
    let (two, each) = infos(&[desc(BUFFER, 0, 70_000, 1, 1, 1), desc(BUFFER, 0, 10, 1, 1, 1)]);
    assert_eq!((each[0].offset, each[1].offset), (0, 131_072));
    assert_eq!(two.size, 131_072 + 65_536);
    // multisampled: 4 MiB alignment
    let mut ms = desc(3, 28, 64, 64, 1, 1);
    ms.sample_count = 4;
    ms.layout = 1;
    let (m, _) = infos(&[ms]);
    assert_eq!(m.alignment, 4 << 20);
    // an unknown size is the documented all ones
    let (bad, _) = infos(&[desc(3, 0, 8, 8, 1, 1)]);
    assert_eq!(bad, AllocationInfo { size: u64::MAX, alignment: 0 });
}

/// The same answers as the host device's own `GetCopyableFootprints`, colour and depth/stencil, 1D to 3D: the
/// title uploads with ours into host copies whose rows the host lays out.
#[cfg(windows)]
#[test]
fn footprints_agree_with_the_host_devices() {
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
    let Some(host) = crate::host::host() else { return };
    let mut all = cases();
    // depth and depth/stencil: 19 D32S8X24, 20 R32_FLOAT_X8X24, 39..=40 D32 family, 44 R24G8, 45 D24S8, 46, 47, 55 D16, 56
    for format in [19u32, 45, 40, 55] {
        all.push(desc(3, format, 64, 64, 1, 1));
        all.push(desc(3, format, 100, 33, 1, 4));
        all.push(desc(3, format, 64, 64, 3, 2));
    }
    let mut differ = Vec::new();
    for d in all {
        let kind = match d.dimension {
            2 => D3D12_RESOURCE_DIMENSION_TEXTURE1D,
            3 => D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            _ => D3D12_RESOURCE_DIMENSION_TEXTURE3D,
        };
        let depth_format = matches!(d.format, 19..=22 | 39..=47 | 55);
        let hd = D3D12_RESOURCE_DESC {
            Dimension: kind,
            Width: d.width,
            Height: d.height,
            DepthOrArraySize: d.depth_or_array_size,
            MipLevels: d.mip_levels,
            Format: DXGI_FORMAT(d.format as i32),
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Flags: if depth_format { D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL } else { D3D12_RESOURCE_FLAG_NONE },
            ..Default::default()
        };
        let planes = if matches!(d.format, 19 | 45 | 44) { 2 } else { 1 };
        let n = u32::from(d.mip_levels) * if d.dimension == TEXTURE_3D { 1 } else { u32::from(d.depth_or_array_size) } * planes;
        let mut hf = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); n as usize];
        let (mut hr, mut hs, mut ht) = (vec![0u32; n as usize], vec![0u64; n as usize], 0u64);
        // SAFETY: outputs for `n` subresources.
        unsafe { host.device.GetCopyableFootprints(&hd, 0, n, 0, Some(hf.as_mut_ptr()), Some(hr.as_mut_ptr()), Some(hs.as_mut_ptr()), Some(&mut ht)) };
        let mut f = vec![NO_FOOTPRINT; n as usize];
        let (mut rows, mut sizes, mut total) = (vec![0u32; n as usize], vec![0u64; n as usize], 0u64);
        copyable_footprints(std::ptr::null(), &d, 0, n, 0, f.as_mut_ptr(), rows.as_mut_ptr(), sizes.as_mut_ptr(), &mut total);
        for i in 0..n as usize {
            let h = (hf[i].Offset, hf[i].Footprint.RowPitch, hr[i], hs[i], hf[i].Footprint.Width, hf[i].Footprint.Height, hf[i].Footprint.Depth);
            let o = (f[i].offset, f[i].row_pitch, rows[i], sizes[i], f[i].width, f[i].height, f[i].depth);
            if h != o {
                differ.push(format!("dim {} fmt {} {}x{}x{} mips {} sub {i}: ours {o:?}, host {h:?}", d.dimension, d.format, d.width, d.height, d.depth_or_array_size, d.mip_levels));
            }
        }
        if ht != total {
            differ.push(format!("dim {} fmt {} {}x{}x{} mips {}: total ours {total}, host {ht}", d.dimension, d.format, d.width, d.height, d.depth_or_array_size, d.mip_levels));
        }
    }
    assert!(differ.is_empty(), "{} differ from the host, first:\n{}", differ.len(), differ.iter().take(14).cloned().collect::<Vec<_>>().join("\n"));
}
