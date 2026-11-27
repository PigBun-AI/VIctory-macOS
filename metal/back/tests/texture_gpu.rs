//! Textures, placement heaps and texture copies on the GPU (native arm64 and
//! under Rosetta: `cargo test --target x86_64-apple-darwin`). Every test
//! uploads through a shared buffer, runs the real copy or a shader, and
//! reads the result back, so a wrong layout, pitch or order shows as wrong
//! bytes.
#![cfg(target_vendor = "apple")]

use std::sync::{Mutex, MutexGuard, OnceLock};

use victory_metal_back::Error;
use victory_metal_back::copy::{BufLoc, TexLoc};
use victory_metal_back::device::Device;
use victory_metal_back::format::dxgi;
use victory_metal_back::memory::{Buffer, BufferKind};
use victory_metal_back::queue::Queue;
use victory_metal_back::record::CommandList;
use victory_metal_back::texture::{TexDim, Texture, TextureDesc};

const SRV: u32 = 1;
const UAV: u32 = 2;
const RT: u32 = 4;
const DS: u32 = 8;
const WAIT_MS: u64 = 20_000;

fn setup() -> (MutexGuard<'static, ()>, Device, Queue) {
    static LOCK: Mutex<()> = Mutex::new(());
    static D: OnceLock<Device> = OnceLock::new();
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let d = D
        .get_or_init(|| Device::new().expect("Metal 4 device"))
        .clone();
    let q = Queue::new(&d).expect("queue");
    (guard, d, q)
}

fn run(q: &Queue, l: CommandList) {
    let v = q.submit(vec![l]).expect("submit");
    q.wait_for(v, WAIT_MS).expect("completes");
}

/// Bytes with a pattern that depends on `seed`.
fn pattern(n: usize, seed: u32) -> Vec<u8> {
    (0..n as u32)
        .map(|i| {
            (i.wrapping_mul(2_654_435_761)
                .wrapping_add(seed.wrapping_mul(97))
                >> 11) as u8
        })
        .collect()
}

fn loc(t: &Texture, mip: u32, slice: u32) -> TexLoc<'_> {
    TexLoc {
        texture: t,
        mip,
        slice,
        plane: 0,
        origin: [0; 3],
    }
}

fn buf_loc(b: &Buffer, format: u32, row_pitch: u32, rows: u32) -> BufLoc<'_> {
    BufLoc {
        buffer: b,
        offset: 0,
        format,
        row_pitch,
        rows,
    }
}

/// Uploads `data` (rows of `row_pitch` bytes) into one subresource, whole.
fn upload(d: &Device, q: &Queue, t: &Texture, mip: u32, slice: u32, data: &[u8], pitch: u32) {
    let src = d
        .create_buffer(BufferKind::Upload, data.len() as u64)
        .unwrap();
    src.write(0, data).unwrap();
    let size = t.desc().mip_size(mip);
    let mut l = q.begin_list().unwrap();
    l.copy_buffer_to_texture(
        &loc(t, mip, slice),
        &buf_loc(&src, t.desc().format, pitch, size[1]),
        size,
    )
    .unwrap();
    l.end().unwrap();
    run(q, l);
}

fn download(d: &Device, q: &Queue, t: &Texture, mip: u32, slice: u32, pitch: u32) -> Vec<u8> {
    let size = t.desc().mip_size(mip);
    let f = victory_metal_back::format::lookup(t.desc().format).unwrap();
    let len = u64::from(pitch) * u64::from(f.block_rows(size[1])) * u64::from(size[2]);
    let dst = d.create_buffer(BufferKind::Readback, len).unwrap();
    let mut l = q.begin_list().unwrap();
    l.copy_texture_to_buffer(
        &buf_loc(&dst, t.desc().format, pitch, size[1]),
        &loc(t, mip, slice),
        size,
    )
    .unwrap();
    l.end().unwrap();
    run(q, l);
    let mut out = vec![0u8; len as usize];
    dst.read(0, &mut out).unwrap();
    out
}

#[test]
fn rgba8_mips_and_slices_round_trip_with_padded_pitch() {
    let (_g, d, q) = setup();
    let desc = TextureDesc {
        dim: TexDim::D2,
        format: dxgi::R8G8B8A8_UNORM,
        width: 13,
        height: 9,
        depth_or_array: 3,
        mips: 4,
        samples: 1,
        usage: SRV,
    };
    let t = d.create_texture(&desc).unwrap();
    assert!(t.is_resident());
    assert!(t.alloc_size() >= 13 * 9 * 4 * 3);
    let mut want = Vec::new();
    for slice in 0..3 {
        for mip in 0..4 {
            let [w, h, _] = desc.mip_size(mip);
            // A pitch wider than the row: 256-byte aligned like the original API.
            let pitch = (w * 4).next_multiple_of(256);
            let mut img = vec![0u8; (pitch * h) as usize];
            let seed = slice * 10 + mip;
            for y in 0..h {
                let row = pattern((w * 4) as usize, seed * 100 + y);
                img[(y * pitch) as usize..(y * pitch + w * 4) as usize].copy_from_slice(&row);
            }
            upload(&d, &q, &t, mip, slice, &img, pitch);
            want.push((mip, slice, pitch, img));
        }
    }
    for (mip, slice, pitch, img) in want {
        let [w, h, _] = desc.mip_size(mip);
        let got = download(&d, &q, &t, mip, slice, pitch);
        for y in 0..h {
            let (a, b) = ((y * pitch) as usize, (y * pitch + w * 4) as usize);
            assert_eq!(got[a..b], img[a..b], "mip {mip} slice {slice} row {y}");
        }
    }
}

#[test]
fn a_sub_rectangle_goes_where_the_origin_says() {
    let (_g, d, q) = setup();
    let t = d
        .create_texture(&TextureDesc::tex2d(dxgi::R32_UINT, 16, 16, SRV))
        .unwrap();
    upload(&d, &q, &t, 0, 0, &vec![0u8; 16 * 16 * 4], 64);
    // 4x3 block of u32 at (5, 7).
    let words: Vec<u32> = (0..12).map(|i| 1000 + i).collect();
    let src = d.create_buffer(BufferKind::Upload, 4096).unwrap();
    let mut bytes = vec![0u8; 4096];
    for r in 0..3usize {
        for c in 0..4usize {
            let o = 256 + r * 256 + c * 4;
            bytes[o..o + 4].copy_from_slice(&words[r * 4 + c].to_le_bytes());
        }
    }
    src.write(0, &bytes).unwrap();
    let mut l = q.begin_list().unwrap();
    let mut dst = loc(&t, 0, 0);
    dst.origin = [5, 7, 0];
    let mut s = buf_loc(&src, dxgi::R32_UINT, 256, 3);
    s.offset = 256;
    l.copy_buffer_to_texture(&dst, &s, [4, 3, 1]).unwrap();
    l.end().unwrap();
    run(&q, l);
    let got = download(&d, &q, &t, 0, 0, 64);
    for y in 0..16usize {
        for x in 0..16usize {
            let v = u32::from_le_bytes(got[y * 64 + x * 4..][..4].try_into().unwrap());
            let want = if (5..9).contains(&x) && (7..10).contains(&y) {
                1000 + ((y - 7) * 4 + (x - 5)) as u32
            } else {
                0
            };
            assert_eq!(v, want, "({x}, {y})");
        }
    }
}

#[test]
fn bc_formats_copy_in_blocks_including_odd_and_tiny_mips() {
    let (_g, d, q) = setup();
    for (fmt, block) in [
        (dxgi::BC1_UNORM, 8u32),
        (dxgi::BC3_UNORM_SRGB, 16),
        (dxgi::BC4_UNORM, 8),
        (dxgi::BC5_SNORM, 16),
        (dxgi::BC6H_UF16, 16),
        (dxgi::BC7_UNORM, 16),
    ] {
        // 20x12: mips 20x12, 10x6, 5x3, 2x1, 1x1: the last ones are smaller than a block.
        let desc = TextureDesc {
            dim: TexDim::D2,
            format: fmt,
            width: 20,
            height: 12,
            depth_or_array: 1,
            mips: 5,
            samples: 1,
            usage: SRV,
        };
        let t = d.create_texture(&desc).unwrap();
        for mip in 0..5 {
            let [w, h, _] = desc.mip_size(mip);
            let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
            let pitch = bw * block;
            let img = pattern((pitch * bh) as usize, fmt * 7 + mip);
            upload(&d, &q, &t, mip, 0, &img, pitch);
            let got = download(&d, &q, &t, mip, 0, pitch);
            assert_eq!(got, img, "format {fmt} mip {mip} ({w}x{h})");
        }
    }
}

/// D3D12 gives the footprint of a mip smaller than a block as one whole
/// block (a 2x2 or 1x1 mip of a BC texture is 4x4 in `GetCopyableFootprints`)
/// and `CopyTextureRegion` takes it: the game's streaming of BC mips does.
/// The region then reaches past the mip's texel size, up to its block-rounded
/// size, and nothing is refused.
#[test]
fn a_bc_footprint_of_a_mip_smaller_than_a_block_is_one_whole_block() {
    let (_g, d, q) = setup();
    for (fmt, block) in [
        (dxgi::BC1_UNORM, 8u32),
        (dxgi::BC4_UNORM, 8),
        (dxgi::BC5_SNORM, 16),
        (dxgi::BC7_UNORM, 16),
    ] {
        // 16x16 with a full chain: mips 3 (2x2) and 4 (1x1) are inside one block.
        let desc = TextureDesc {
            dim: TexDim::D2,
            format: fmt,
            width: 16,
            height: 16,
            depth_or_array: 1,
            mips: 5,
            samples: 1,
            usage: SRV,
        };
        let t = d.create_texture(&desc).unwrap();
        for mip in 2..5 {
            let img = pattern(block as usize, fmt * 13 + mip);
            let src = d.create_buffer(BufferKind::Upload, u64::from(block)).unwrap();
            src.write(0, &img).unwrap();
            let mut l = q.begin_list().unwrap();
            l.copy_buffer_to_texture(
                &loc(&t, mip, 0),
                &buf_loc(&src, fmt, block, 4),
                [4, 4, 1],
            )
            .unwrap_or_else(|e| panic!("format {fmt} mip {mip}: upload of the 4x4 footprint: {e}"));
            l.end().unwrap();
            run(&q, l);
            let dst = d.create_buffer(BufferKind::Readback, u64::from(block)).unwrap();
            let mut l = q.begin_list().unwrap();
            l.copy_texture_to_buffer(&buf_loc(&dst, fmt, block, 4), &loc(&t, mip, 0), [4, 4, 1])
                .unwrap_or_else(|e| panic!("format {fmt} mip {mip}: download of the 4x4 footprint: {e}"));
            l.end().unwrap();
            run(&q, l);
            let mut got = vec![0u8; block as usize];
            dst.read(0, &mut got).unwrap();
            assert_eq!(got, img, "format {fmt} mip {mip}");
        }
        // Past the rounded size is still an error.
        let src = d.create_buffer(BufferKind::Upload, 256).unwrap();
        let mut l = q.begin_list().unwrap();
        assert!(
            l.copy_buffer_to_texture(&loc(&t, 3, 0), &buf_loc(&src, fmt, 2 * block, 4), [8, 4, 1])
                .is_err(),
            "8 wide on a 2x2 mip"
        );
    }
}

#[test]
fn one_d_and_three_d_textures_round_trip() {
    let (_g, d, q) = setup();
    let t1 = d
        .create_texture(&TextureDesc {
            dim: TexDim::D1,
            format: dxgi::R16_UINT,
            width: 37,
            height: 1,
            depth_or_array: 2,
            mips: 1,
            samples: 1,
            usage: SRV,
        })
        .unwrap();
    for slice in 0..2 {
        let img = pattern(37 * 2, 5 + slice);
        upload(&d, &q, &t1, 0, slice, &img, 37 * 2);
        assert_eq!(download(&d, &q, &t1, 0, slice, 37 * 2), img);
    }
    let desc = TextureDesc {
        dim: TexDim::D3,
        format: dxgi::R8G8B8A8_UNORM,
        width: 6,
        height: 5,
        depth_or_array: 4,
        mips: 2,
        samples: 1,
        usage: SRV,
    };
    let t3 = d.create_texture(&desc).unwrap();
    for mip in 0..2 {
        let [w, h, dd] = desc.mip_size(mip);
        let img = pattern((w * 4 * h * dd) as usize, 40 + mip);
        // Tightly packed: row pitch w*4, h rows per image.
        let src = d
            .create_buffer(BufferKind::Upload, img.len() as u64)
            .unwrap();
        src.write(0, &img).unwrap();
        let mut l = q.begin_list().unwrap();
        l.copy_buffer_to_texture(
            &loc(&t3, mip, 0),
            &buf_loc(&src, dxgi::R8G8B8A8_UNORM, w * 4, h),
            [w, h, dd],
        )
        .unwrap();
        l.end().unwrap();
        run(&q, l);
        assert_eq!(download(&d, &q, &t3, mip, 0, w * 4), img, "3D mip {mip}");
    }
}

#[test]
fn texture_to_texture_and_copy_resource() {
    let (_g, d, q) = setup();
    let desc = TextureDesc {
        dim: TexDim::D2,
        format: dxgi::R8G8B8A8_UNORM,
        width: 16,
        height: 16,
        depth_or_array: 2,
        mips: 3,
        samples: 1,
        usage: SRV,
    };
    let a = d.create_texture(&desc).unwrap();
    let b = d.create_texture(&desc).unwrap();
    let c = d.create_texture(&desc).unwrap();
    let mut imgs = Vec::new();
    for slice in 0..2 {
        for mip in 0..3 {
            let [w, h, _] = desc.mip_size(mip);
            let img = pattern((w * h * 4) as usize, slice * 3 + mip);
            upload(&d, &q, &a, mip, slice, &img, w * 4);
            imgs.push((mip, slice, w, img));
        }
    }
    // Whole resource.
    let mut l = q.begin_list().unwrap();
    l.copy_texture_resource(&b, &a).unwrap();
    // A region of one subresource into another place.
    let mut dst = loc(&c, 1, 1);
    dst.origin = [2, 3, 0];
    let mut src = loc(&a, 0, 0);
    src.origin = [4, 4, 0];
    l.copy_texture(&dst, &src, [4, 2, 1]).unwrap();
    l.end().unwrap();
    run(&q, l);
    for (mip, slice, w, img) in imgs {
        assert_eq!(
            download(&d, &q, &b, mip, slice, w * 4),
            img,
            "{mip}/{slice}"
        );
    }
    let got = download(&d, &q, &c, 1, 1, 32);
    let full = download(&d, &q, &a, 0, 0, 64);
    for r in 0..2usize {
        let g = &got[(3 + r) * 32 + 2 * 4..][..16];
        let w = &full[(4 + r) * 64 + 4 * 4..][..16];
        assert_eq!(g, w, "row {r}");
    }
}

#[test]
fn copies_with_a_wrong_argument_are_errors_not_silent() {
    let (_g, d, q) = setup();
    let t = d
        .create_texture(&TextureDesc::tex2d(dxgi::R8G8B8A8_UNORM, 8, 8, SRV))
        .unwrap();
    let bc = d
        .create_texture(&TextureDesc::tex2d(dxgi::BC1_UNORM, 16, 16, SRV))
        .unwrap();
    let buf = d.create_buffer(BufferKind::Upload, 4096).unwrap();
    let small = d.create_buffer(BufferKind::Upload, 64).unwrap();
    let mut l = q.begin_list().unwrap();
    let ok = buf_loc(&buf, dxgi::R8G8B8A8_UNORM, 256, 8);
    let bad = |r: Result<(), Error>, what: &str| {
        assert!(
            matches!(r, Err(Error::BadArgument(_)) | Err(Error::Unsupported(_))),
            "{what}: {r:?}"
        );
    };
    // Row pitch smaller than the row.
    bad(
        l.copy_buffer_to_texture(
            &loc(&t, 0, 0),
            &buf_loc(&buf, dxgi::R8G8B8A8_UNORM, 16, 8),
            [8, 8, 1],
        ),
        "pitch under a row",
    );
    // Row pitch not a multiple of the texel size.
    bad(
        l.copy_buffer_to_texture(
            &loc(&t, 0, 0),
            &buf_loc(&buf, dxgi::R8G8B8A8_UNORM, 34, 8),
            [8, 8, 1],
        ),
        "pitch not a texel multiple",
    );
    // Offset not a multiple of the texel size.
    let mut off = ok;
    off.offset = 2;
    bad(
        l.copy_buffer_to_texture(&loc(&t, 0, 0), &off, [8, 8, 1]),
        "offset not a texel multiple",
    );
    // Buffer too small for the footprint.
    bad(
        l.copy_buffer_to_texture(
            &loc(&t, 0, 0),
            &buf_loc(&small, dxgi::R8G8B8A8_UNORM, 256, 8),
            [8, 8, 1],
        ),
        "buffer too small",
    );
    // Region outside the mip; mip and slice out of range.
    bad(
        l.copy_buffer_to_texture(&loc(&t, 0, 0), &ok, [9, 8, 1]),
        "region wider than the mip",
    );
    let mut o = loc(&t, 0, 0);
    o.origin = [4, 0, 0];
    bad(
        l.copy_buffer_to_texture(&o, &ok, [5, 1, 1]),
        "origin plus size",
    );
    bad(
        l.copy_buffer_to_texture(&loc(&t, 1, 0), &ok, [8, 8, 1]),
        "mip 1 of 1 is 4x4",
    );
    bad(
        l.copy_buffer_to_texture(&loc(&t, 0, 0), &ok, [8, 8, 4]),
        "depth on 2D",
    );
    bad(
        l.copy_buffer_to_texture(&loc(&t, 3, 0), &ok, [1, 1, 1]),
        "mip out of range",
    );
    bad(
        l.copy_buffer_to_texture(&loc(&t, 0, 1), &ok, [1, 1, 1]),
        "slice out of range",
    );
    // Footprint format of another texel size.
    bad(
        l.copy_buffer_to_texture(
            &loc(&t, 0, 0),
            &buf_loc(&buf, dxgi::R16_UINT, 256, 8),
            [8, 8, 1],
        ),
        "format of another size",
    );
    // Block compressed: unaligned origin, partial block, pitch in texels instead of blocks.
    let bcok = buf_loc(&buf, dxgi::BC1_UNORM, 32, 16);
    let mut o = loc(&bc, 0, 0);
    o.origin = [2, 0, 0];
    bad(
        l.copy_buffer_to_texture(&o, &bcok, [4, 4, 1]),
        "BC origin off the block grid",
    );
    bad(
        l.copy_buffer_to_texture(&loc(&bc, 0, 0), &bcok, [6, 4, 1]),
        "BC partial block inside the mip",
    );
    bad(
        l.copy_buffer_to_texture(
            &loc(&bc, 0, 0),
            &buf_loc(&buf, dxgi::BC1_UNORM, 12, 16),
            [16, 16, 1],
        ),
        "BC pitch smaller than a row of blocks",
    );
    bad(
        l.copy_buffer_to_texture(
            &loc(&bc, 0, 0),
            &buf_loc(&buf, dxgi::R8G8B8A8_UNORM, 64, 16),
            [16, 16, 1],
        ),
        "uncompressed footprint on a BC texture",
    );
    // Unsupported pair for a resource copy.
    let other = d
        .create_texture(&TextureDesc::tex2d(dxgi::R8G8B8A8_UNORM, 4, 4, SRV))
        .unwrap();
    bad(l.copy_texture_resource(&other, &t), "different sizes");
    bad(l.copy_texture_resource(&t, &t), "onto itself");
    // A list that took only errors still ends and runs; nothing was encoded.
    assert_eq!(l.stats().copies, 0);
    l.end().unwrap();
    run(&q, l);
}

#[test]
fn depth_stencil_copies_planes_and_refuses_the_d24_depth_plane() {
    let (_g, d, q) = setup();
    let t = d
        .create_texture(&TextureDesc::tex2d(
            dxgi::D32_FLOAT_S8X24_UINT,
            8,
            8,
            DS | SRV,
        ))
        .unwrap();
    assert_eq!(t.desc().subresources().unwrap(), 2);
    // Fill both planes from buffers: depth floats, stencil bytes.
    let depth: Vec<u8> = (0..64u32)
        .flat_map(|i| (i as f32 / 64.0).to_le_bytes())
        .collect();
    let stencil = pattern(64, 9);
    let up_d = d.create_buffer(BufferKind::Upload, 256).unwrap();
    let up_s = d.create_buffer(BufferKind::Upload, 64).unwrap();
    up_d.write(0, &depth).unwrap();
    up_s.write(0, &stencil).unwrap();
    let mut l = q.begin_list().unwrap();
    let mut p0 = loc(&t, 0, 0);
    p0.plane = 0;
    l.copy_buffer_to_texture(&p0, &buf_loc(&up_d, dxgi::R32_TYPELESS, 32, 8), [8, 8, 1])
        .unwrap();
    let mut p1 = loc(&t, 0, 0);
    p1.plane = 1;
    l.copy_buffer_to_texture(&p1, &buf_loc(&up_s, dxgi::R8_TYPELESS, 8, 8), [8, 8, 1])
        .unwrap();
    let rd = d.create_buffer(BufferKind::Readback, 256).unwrap();
    let rs = d.create_buffer(BufferKind::Readback, 64).unwrap();
    l.copy_texture_to_buffer(&buf_loc(&rd, dxgi::R32_TYPELESS, 32, 8), &p0, [8, 8, 1])
        .unwrap();
    l.copy_texture_to_buffer(&buf_loc(&rs, dxgi::R8_TYPELESS, 8, 8), &p1, [8, 8, 1])
        .unwrap();
    l.end().unwrap();
    run(&q, l);
    let mut got = vec![0u8; 256];
    rd.read(0, &mut got).unwrap();
    assert_eq!(got, depth);
    let mut got = vec![0u8; 64];
    rs.read(0, &mut got).unwrap();
    assert_eq!(got, stencil);

    // D24S8: the stencil plane copies, the depth plane is refused.
    let t24 = d
        .create_texture(&TextureDesc::tex2d(dxgi::D24_UNORM_S8_UINT, 8, 8, DS))
        .unwrap();
    let mut l = q.begin_list().unwrap();
    let mut p1 = loc(&t24, 0, 0);
    p1.plane = 1;
    l.copy_buffer_to_texture(&p1, &buf_loc(&up_s, dxgi::R8_TYPELESS, 8, 8), [8, 8, 1])
        .unwrap();
    let mut p0 = loc(&t24, 0, 0);
    p0.plane = 0;
    let r = l.copy_buffer_to_texture(&p0, &buf_loc(&up_d, dxgi::R32_TYPELESS, 32, 8), [8, 8, 1]);
    assert!(matches!(r, Err(Error::Unsupported(m)) if m.contains("depth plane")));
    l.end().unwrap();
    run(&q, l);
}

#[test]
fn texture_descriptions_metal_cannot_make_are_refused_with_a_reason() {
    let (_g, d, _q) = setup();
    let e = d
        .create_texture(&TextureDesc::tex2d(dxgi::R32G32B32_FLOAT, 4, 4, SRV))
        .unwrap_err();
    assert!(
        matches!(&e, Error::Unsupported(m) if m.contains("R32G32B32_FLOAT")),
        "{e}"
    );
    let e = d
        .create_texture(&TextureDesc::tex2d(dxgi::BC7_UNORM, 4, 4, UAV))
        .unwrap_err();
    assert!(matches!(e, Error::Unsupported(_)), "{e}");
    let e = d
        .create_texture(&TextureDesc::tex2d(dxgi::R8G8B8A8_UNORM, 0, 4, SRV))
        .unwrap_err();
    assert!(matches!(e, Error::BadArgument(_)), "{e}");
    // Render targets, UAVs and multisampling exist.
    d.create_texture(&TextureDesc::tex2d(dxgi::R8G8B8A8_UNORM, 64, 64, RT | SRV))
        .unwrap();
    d.create_texture(&TextureDesc::tex2d(dxgi::R32_UINT, 64, 64, UAV))
        .unwrap();
    let mut ms = TextureDesc::tex2d(dxgi::R8G8B8A8_UNORM, 64, 64, RT);
    ms.samples = 4;
    d.create_texture(&ms).unwrap();
}

// ---- shaders: descriptors, views, samplers, UAVs, heaps -------------------

use victory_metal_back::descriptors::{DescriptorHeap, IrDescriptorTableEntry};
use victory_metal_back::sampler::{Address, Border, Compare, Filter, SamplerSpec};
use victory_metal_back::texture::{SWIZZLE_IDENTITY, ViewDim, ViewSpec};

const KERNELS: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
struct EF { ulong va; texture2d<float> tex; ulong md; };\n\
struct EU { ulong va; texture2d<uint> tex; ulong md; };\n\
struct EA { ulong va; texture2d_array<float> tex; ulong md; };\n\
struct EC { ulong va; texturecube<float> tex; ulong md; };\n\
struct EW { ulong va; texture2d<float, access::read_write> tex; ulong md; };\n\
struct EWU { ulong va; texture2d<uint, access::read_write> tex; ulong md; };\n\
struct ED { ulong va; depth2d<float> tex; ulong md; };\n\
struct E1 { ulong va; texture1d<float, access::read_write> tex; ulong md; };\n\
struct ES { sampler s; ulong tex; ulong md; };\n\
struct P { uint index; uint x; uint y; uint z; };\n\
kernel void read_f(device const EF* heap [[buffer(0)]], device float4* out [[buffer(1)]], \
constant P& p [[buffer(2)]]) { out[0] = heap[p.index].tex.read(uint2(p.x, p.y), p.z); }\n\
kernel void read_u(device const EU* heap [[buffer(0)]], device uint4* out [[buffer(1)]], \
constant P& p [[buffer(2)]]) { out[0] = heap[p.index].tex.read(uint2(p.x, p.y), p.z); }\n\
kernel void read_array(device const EA* heap [[buffer(0)]], device float4* out [[buffer(1)]], \
constant P& p [[buffer(2)]]) { out[0] = heap[p.index].tex.read(uint2(p.x, p.y), p.z, 0); }\n\
kernel void sample_cube(device const EC* heap [[buffer(0)]], device const ES* sheap [[buffer(3)]], \
device float4* out [[buffer(1)]], constant P& p [[buffer(2)]]) {\n\
  out[0] = heap[p.index].tex.sample(sheap[0].s, float3(0.0, 0.0, 1.0)); }\n\
kernel void sample_f(device const EF* heap [[buffer(0)]], device const ES* sheap [[buffer(3)]], \
device float4* out [[buffer(1)]], constant float4& uv [[buffer(2)]]) {\n\
  out[0] = heap[0].tex.sample(sheap[0].s, uv.xy, level(0)); }\n\
kernel void sample_cmp(device const ED* heap [[buffer(0)]], device const ES* sheap [[buffer(3)]], \
device float4* out [[buffer(1)]], constant float4& uv [[buffer(2)]]) {\n\
  out[0] = float4(heap[0].tex.sample_compare(sheap[0].s, uv.xy, uv.z, level(0)), 0, 0, 0); }\n\
kernel void write_f(device const EW* heap [[buffer(0)]], uint2 t [[thread_position_in_grid]]) {\n\
  heap[0].tex.write(float4(float(t.x), float(t.y), 7.0, 1.0), t); }\n\
kernel void write_1d(device const E1* heap [[buffer(0)]], uint t [[thread_position_in_grid]]) {\n\
  heap[0].tex.write(float4(float(t) * 0.25, 0, 0, 1), t); }\n\
kernel void write_u(device const EWU* heap [[buffer(0)]], uint2 t [[thread_position_in_grid]]) {\n\
  heap[0].tex.write(uint4(t.x + 1u, t.y + 1u, 3u, 4u), t); }\n";

struct Rig {
    d: Device,
    q: Queue,
    heap: DescriptorHeap,
    out: Buffer,
    params: Buffer,
}

impl Rig {
    fn new(d: &Device, q: &Queue) -> Rig {
        let heap = DescriptorHeap::new(d, 16).unwrap();
        let out = d.create_buffer(BufferKind::Shared, 256).unwrap();
        let params = d.create_buffer(BufferKind::Shared, 256).unwrap();
        Rig {
            d: d.clone(),
            q: q.clone(),
            heap,
            out,
            params,
        }
    }

    fn srv(&self, index: u32, resource_id: u64) {
        self.heap
            .write(index, IrDescriptorTableEntry::texture(resource_id, 0.0, 0))
            .unwrap();
    }

    fn run_kernel(&self, entry: &str, params: &[u32], groups: [u32; 3], threads: [u32; 3]) {
        let pso = self.d.compute_pipeline_from_msl(KERNELS, entry).unwrap();
        self.params.write_u32s(0, params).unwrap();
        let mut l = self.q.begin_list().unwrap();
        l.set_heaps(&self.heap, None).unwrap();
        l.set_address(1, self.out.gpu_va()).unwrap();
        l.set_address(2, self.params.gpu_va()).unwrap();
        l.dispatch(&pso, groups, threads).unwrap();
        l.end().unwrap();
        run(&self.q, l);
    }

    fn read_f(&self, entry: &str, params: &[u32]) -> [f32; 4] {
        self.run_kernel(entry, params, [1, 1, 1], [1, 1, 1]);
        let w = self.out.read_u32s(0, 4).unwrap();
        [0, 1, 2, 3].map(|i| f32::from_bits(w[i]))
    }

    fn read_u(&self, entry: &str, params: &[u32]) -> [u32; 4] {
        self.run_kernel(entry, params, [1, 1, 1], [1, 1, 1]);
        let w = self.out.read_u32s(0, 4).unwrap();
        [w[0], w[1], w[2], w[3]]
    }
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 2e-3
}

#[test]
fn a_shader_reads_a_texture_through_a_descriptor() {
    let (_g, d, q) = setup();
    let rig = Rig::new(&d, &q);
    let t = d
        .create_texture(&TextureDesc::tex2d(dxgi::R8G8B8A8_UNORM, 4, 4, SRV))
        .unwrap();
    let mut img = vec![0u8; 4 * 4 * 4];
    for i in 0..16 {
        img[i * 4..i * 4 + 4].copy_from_slice(&[i as u8 * 10, 255, 0, 128]);
    }
    upload(&d, &q, &t, 0, 0, &img, 16);
    // The whole-texture view is the texture itself: no view object.
    let id = t.view(&ViewSpec::whole(ViewDim::D2)).unwrap();
    assert_eq!(id, t.resource_id());
    assert_eq!(t.view_count(), 0);
    rig.srv(3, id);
    let v = rig.read_f("read_f", &[3, 2, 1, 0]);
    // texel (2, 1) = index 6
    assert!(
        near(v[0], 60.0 / 255.0) && near(v[1], 1.0) && near(v[3], 128.0 / 255.0),
        "{v:?}"
    );
}

#[test]
fn typeless_textures_are_viewed_in_other_formats() {
    let (_g, d, q) = setup();
    let rig = Rig::new(&d, &q);
    let t = d
        .create_texture(&TextureDesc::tex2d(dxgi::R8G8B8A8_TYPELESS, 2, 2, SRV))
        .unwrap();
    upload(&d, &q, &t, 0, 0, &[128, 64, 255, 255].repeat(4), 8);
    let view = |format| {
        let mut s = ViewSpec::whole(ViewDim::D2);
        s.format = format;
        t.view(&s).unwrap()
    };
    rig.srv(0, view(dxgi::R8G8B8A8_UNORM));
    rig.srv(1, view(dxgi::R8G8B8A8_UNORM_SRGB));
    rig.srv(2, view(dxgi::R8G8B8A8_UINT));
    let lin = rig.read_f("read_f", &[0, 0, 0, 0]);
    assert!(
        near(lin[0], 128.0 / 255.0) && near(lin[1], 64.0 / 255.0),
        "{lin:?}"
    );
    let srgb = rig.read_f("read_f", &[1, 0, 0, 0]);
    // sRGB decode of 128/255 = 0.2158605, of 64/255 = 0.0513.
    assert!(
        near(srgb[0], 0.2158605) && near(srgb[1], 0.0513) && near(srgb[2], 1.0),
        "{srgb:?}"
    );
    let uint = rig.read_u("read_u", &[2, 0, 0, 0]);
    assert_eq!(uint, [128, 64, 255, 255]);
    // The same request is the same object.
    assert_eq!(
        view(dxgi::R8G8B8A8_UNORM_SRGB),
        view(dxgi::R8G8B8A8_UNORM_SRGB)
    );
    assert!(t.view_count() <= 3);
    // Another family is refused.
    let mut bad = ViewSpec::whole(ViewDim::D2);
    bad.format = dxgi::R16G16_FLOAT;
    assert!(t.view(&bad).is_err());

    // R32_TYPELESS as float and as uint.
    let t = d
        .create_texture(&TextureDesc::tex2d(dxgi::R32_TYPELESS, 2, 2, SRV | UAV))
        .unwrap();
    upload(&d, &q, &t, 0, 0, &1.5f32.to_le_bytes().repeat(4), 8);
    let mut s = ViewSpec::whole(ViewDim::D2);
    s.format = dxgi::R32_FLOAT;
    rig.srv(0, t.view(&s).unwrap());
    s.format = dxgi::R32_UINT;
    rig.srv(1, t.view(&s).unwrap());
    assert_eq!(rig.read_f("read_f", &[0, 0, 0, 0])[0], 1.5);
    assert_eq!(rig.read_u("read_u", &[1, 0, 0, 0])[0], 1.5f32.to_bits());
}

/// Regression (victory-gpu `a_depth_texture_written_through_unordered_views_reads_as_depth`):
/// a shader reads the stencil plane as D3D12 shows it, in the green channel
/// (`X32_TYPELESS_G8X24_UINT`). Metal's stencil view has it in red; without the
/// mapping the shader read 0 and nothing failed.
#[test]
fn the_stencil_plane_reads_in_the_green_channel() {
    let (_g, d, q) = setup();
    let rig = Rig::new(&d, &q);
    let t = d
        .create_texture(&TextureDesc::tex2d(
            dxgi::D32_FLOAT_S8X24_UINT,
            8,
            8,
            DS | SRV,
        ))
        .unwrap();
    let stencil = pattern(64, 5);
    let up = d.create_buffer(BufferKind::Upload, 64).unwrap();
    up.write(0, &stencil).unwrap();
    let mut l = q.begin_list().unwrap();
    let mut p1 = loc(&t, 0, 0);
    p1.plane = 1;
    l.copy_buffer_to_texture(&p1, &buf_loc(&up, dxgi::R8_TYPELESS, 8, 8), [8, 8, 1])
        .unwrap();
    l.end().unwrap();
    run(&q, l);
    // The view the executor makes for a plane-1 SRV of the identity mapping.
    let mut spec = ViewSpec::whole(ViewDim::D2);
    spec.format = dxgi::X32_TYPELESS_G8X24_UINT;
    spec.plane = 1;
    spec.swizzle = victory_metal_back::texture::stencil_plane_swizzle(SWIZZLE_IDENTITY);
    assert_eq!(spec.swizzle, [4, 0, 4, 5]);
    rig.srv(0, t.view(&spec).unwrap());
    for (x, y) in [(0u32, 0u32), (5, 0), (3, 4), (7, 7)] {
        let v = rig.read_u("read_u", &[0, x, y, 0]);
        assert_eq!(
            v,
            [0, u32::from(stencil[(y * 8 + x) as usize]), 0, 1],
            "texel {x},{y}"
        );
    }
    // Without the mapping Metal's own view has the stencil in red (the bug).
    spec.swizzle = SWIZZLE_IDENTITY;
    rig.srv(1, t.view(&spec).unwrap());
    let raw = rig.read_u("read_u", &[1, 5, 0, 0]);
    assert_eq!(raw[0], u32::from(stencil[5]));
    assert_eq!(raw[1], 0);
    // The application's own mapping chooses from D3D12's channels: .rrrr is
    // the red channel, which the stencil format leaves at 0.
    assert_eq!(
        victory_metal_back::texture::stencil_plane_swizzle([0; 4]),
        [4, 4, 4, 4]
    );
    assert_eq!(
        victory_metal_back::texture::stencil_plane_swizzle([1, 1, 1, 5]),
        [0, 0, 0, 5]
    );
}

/// `B8G8R8X8` is BGRA in Metal: the bytes are the same, the channel order is
/// right, and a shader view reads the unused byte as 1 (D3D12 never shows it).
#[test]
fn the_unused_channel_of_a_bgrx_texture_reads_as_one() {
    let (_g, d, q) = setup();
    let rig = Rig::new(&d, &q);
    let t = d
        .create_texture(&TextureDesc::tex2d(dxgi::B8G8R8X8_UNORM, 4, 4, SRV))
        .unwrap();
    // Every texel B=0x40 G=0x80 R=0xC0 and the unused byte 0.
    let mut bytes = Vec::new();
    for _ in 0..16 {
        bytes.extend_from_slice(&[0x40, 0x80, 0xC0, 0x00]);
    }
    let up = d.create_buffer(BufferKind::Upload, 64).unwrap();
    up.write(0, &bytes).unwrap();
    let mut l = q.begin_list().unwrap();
    l.copy_buffer_to_texture(
        &loc(&t, 0, 0),
        &buf_loc(&up, dxgi::B8G8R8X8_UNORM, 16, 4),
        [4, 4, 1],
    )
    .unwrap();
    l.end().unwrap();
    run(&q, l);
    let mut spec = ViewSpec::whole(ViewDim::D2);
    spec.swizzle = victory_metal_back::texture::unused_alpha_swizzle(SWIZZLE_IDENTITY);
    assert_eq!(spec.swizzle, [0, 1, 2, 5]);
    rig.srv(0, t.view(&spec).unwrap());
    let v = rig.read_f("read_f", &[0, 1, 1, 0]);
    assert!((v[0] - 192.0 / 255.0).abs() < 1e-3, "{v:?}");
    assert!((v[1] - 128.0 / 255.0).abs() < 1e-3, "{v:?}");
    assert!((v[2] - 64.0 / 255.0).abs() < 1e-3, "{v:?}");
    assert_eq!(v[3], 1.0);
    // The application's mapping still chooses: alpha everywhere.
    assert_eq!(
        victory_metal_back::texture::unused_alpha_swizzle([3, 3, 3, 3]),
        [5, 5, 5, 5]
    );
}

/// A texture made castable (`CreateCommittedResource3`'s list) shows its
/// texels in another format family of the same size: one memory, no copy.
#[test]
fn a_castable_texture_is_read_in_another_format_family() {
    let (_g, d, q) = setup();
    let rig = Rig::new(&d, &q);
    // R32_UINT holding the bits of 1.5f.
    let t = d
        .create_texture(&TextureDesc::tex2d(dxgi::R32_UINT, 4, 4, SRV | 16))
        .unwrap();
    let bytes: Vec<u8> = (0..16).flat_map(|_| 1.5f32.to_le_bytes()).collect();
    let up = d.create_buffer(BufferKind::Upload, 64).unwrap();
    up.write(0, &bytes).unwrap();
    let mut l = q.begin_list().unwrap();
    l.copy_buffer_to_texture(
        &loc(&t, 0, 0),
        &buf_loc(&up, dxgi::R32_UINT, 16, 4),
        [4, 4, 1],
    )
    .unwrap();
    l.end().unwrap();
    run(&q, l);
    let mut spec = ViewSpec::whole(ViewDim::D2);
    spec.format = dxgi::R32_FLOAT;
    rig.srv(0, t.view(&spec).unwrap());
    assert_eq!(rig.read_f("read_f", &[0, 1, 1, 0])[0], 1.5);
    // The same texture made without the flag refuses the view.
    let plain = d
        .create_texture(&TextureDesc::tex2d(dxgi::R32_UINT, 4, 4, SRV))
        .unwrap();
    spec.format = dxgi::R9G9B9E5_SHAREDEXP;
    assert!(plain.view(&spec).is_err());
}

#[test]
fn mip_ranges_array_slices_cubes_and_swizzles() {
    let (_g, d, q) = setup();
    let rig = Rig::new(&d, &q);
    let desc = TextureDesc {
        dim: TexDim::D2,
        format: dxgi::R8G8B8A8_UNORM,
        width: 4,
        height: 4,
        depth_or_array: 6,
        mips: 3,
        samples: 1,
        usage: SRV,
    };
    let t = d.create_texture(&desc).unwrap();
    for slice in 0..6u8 {
        for mip in 0..3u32 {
            let [w, h, _] = desc.mip_size(mip);
            let px = [slice * 40, mip as u8 * 80, 7, 255];
            upload(
                &d,
                &q,
                &t,
                mip,
                u32::from(slice),
                &px.repeat((w * h) as usize),
                w * 4,
            );
        }
    }
    // 2D view of slice 4, mip 1 only.
    let mut s = ViewSpec::whole(ViewDim::D2);
    s.mip_first = 1;
    s.mip_count = 1;
    s.slice_first = 4;
    s.slice_count = 1;
    rig.srv(0, t.view(&s).unwrap());
    let v = rig.read_f("read_f", &[0, 1, 1, 0]);
    assert!(
        near(v[0], 160.0 / 255.0) && near(v[1], 80.0 / 255.0),
        "{v:?}"
    );
    // 2D array view of slices 2..5, from mip 2: read slice index 1 = slice 3.
    let mut s = ViewSpec::whole(ViewDim::D2Array);
    s.mip_first = 2;
    s.slice_first = 2;
    s.slice_count = 3;
    rig.srv(1, t.view(&s).unwrap());
    let v = rig.read_f("read_array", &[1, 0, 0, 1]);
    assert!(
        near(v[0], 120.0 / 255.0) && near(v[1], 160.0 / 255.0),
        "{v:?}"
    );
    // Swizzle: B, G, R, ONE.
    let mut s = ViewSpec::whole(ViewDim::D2);
    s.mip_count = 1;
    s.slice_first = 1;
    s.slice_count = 1;
    s.swizzle = [2, 1, 0, 5];
    rig.srv(2, t.view(&s).unwrap());
    let v = rig.read_f("read_f", &[2, 0, 0, 0]);
    assert!(
        near(v[0], 7.0 / 255.0) && near(v[1], 0.0) && near(v[2], 40.0 / 255.0) && v[3] == 1.0,
        "{v:?}"
    );
    // A cube view needs six faces; a range off the end is refused.
    let mut s = ViewSpec::whole(ViewDim::Cube);
    s.slice_count = 6;
    s.mip_count = 1;
    let cube = t.view(&s).unwrap();
    s.slice_count = 5;
    assert!(t.view(&s).is_err());
    s.slice_count = 6;
    s.slice_first = 1;
    assert!(t.view(&s).is_err());
    let mut s = ViewSpec::whole(ViewDim::D2);
    s.mip_first = 3;
    assert!(t.view(&s).is_err());
    // Sample the +Z face (face 4: slice 4) of the cube.
    rig.srv(3, cube);
    let sampler = d
        .create_sampler(&SamplerSpec {
            min: Filter::Nearest,
            mag: Filter::Nearest,
            ..Default::default()
        })
        .unwrap();
    let sheap = DescriptorHeap::new(&d, 4).unwrap();
    sheap.write(0, sampler.entry()).unwrap();
    let pso = d.compute_pipeline_from_msl(KERNELS, "sample_cube").unwrap();
    rig.params.write_u32s(0, &[3, 0, 0, 0]).unwrap();
    let mut l = q.begin_list().unwrap();
    l.set_heaps(&rig.heap, Some(&sheap)).unwrap();
    l.set_address(1, rig.out.gpu_va()).unwrap();
    l.set_address(2, rig.params.gpu_va()).unwrap();
    l.set_address(3, sheap.gpu_va()).unwrap();
    l.dispatch(&pso, [1, 1, 1], [1, 1, 1]).unwrap();
    l.end().unwrap();
    run(&q, l);
    let w = rig.out.read_u32s(0, 1).unwrap();
    assert!(
        near(f32::from_bits(w[0]), 160.0 / 255.0),
        "+Z face is slice 4"
    );
    let _ = SWIZZLE_IDENTITY;
}

fn sample(rig: &Rig, sheap: &DescriptorHeap, entry: &str, uvz: [f32; 3]) -> [f32; 4] {
    let pso = rig.d.compute_pipeline_from_msl(KERNELS, entry).unwrap();
    rig.params
        .write_u32s(
            0,
            &[uvz[0].to_bits(), uvz[1].to_bits(), uvz[2].to_bits(), 0],
        )
        .unwrap();
    let mut l = rig.q.begin_list().unwrap();
    l.set_heaps(&rig.heap, Some(sheap)).unwrap();
    l.set_address(1, rig.out.gpu_va()).unwrap();
    l.set_address(2, rig.params.gpu_va()).unwrap();
    l.set_address(3, sheap.gpu_va()).unwrap();
    l.dispatch(&pso, [1, 1, 1], [1, 1, 1]).unwrap();
    l.end().unwrap();
    run(&rig.q, l);
    let w = rig.out.read_u32s(0, 4).unwrap();
    [0, 1, 2, 3].map(|i| f32::from_bits(w[i]))
}

#[test]
fn samplers_filter_wrap_border_and_compare() {
    let (_g, d, q) = setup();
    let rig = Rig::new(&d, &q);
    let t = d
        .create_texture(&TextureDesc::tex2d(dxgi::R32_FLOAT, 2, 2, SRV))
        .unwrap();
    let texels = [0.0f32, 1.0, 2.0, 3.0];
    upload(
        &d,
        &q,
        &t,
        0,
        0,
        &texels
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect::<Vec<_>>(),
        8,
    );
    rig.srv(0, t.view(&ViewSpec::whole(ViewDim::D2)).unwrap());
    let sheap = DescriptorHeap::new(&d, 4).unwrap();
    let set = |spec: SamplerSpec| {
        sheap
            .write(0, d.create_sampler(&spec).unwrap().entry())
            .unwrap()
    };
    let base = SamplerSpec {
        min: Filter::Linear,
        mag: Filter::Linear,
        mip: None,
        address: [Address::ClampToEdge; 3],
        ..Default::default()
    };
    set(base);
    let v = sample(&rig, &sheap, "sample_f", [0.5, 0.5, 0.0]);
    assert!(near(v[0], 1.5), "bilinear in the middle: {v:?}");
    let v = sample(&rig, &sheap, "sample_f", [0.75, 0.25, 0.0]);
    assert!(near(v[0], 1.0), "{v:?}");
    set(SamplerSpec {
        min: Filter::Nearest,
        mag: Filter::Nearest,
        ..base
    });
    let v = sample(&rig, &sheap, "sample_f", [0.75, 0.75, 0.0]);
    assert!(near(v[0], 3.0), "point: {v:?}");
    set(SamplerSpec {
        min: Filter::Nearest,
        mag: Filter::Nearest,
        address: [Address::Repeat; 3],
        ..base
    });
    let v = sample(&rig, &sheap, "sample_f", [1.25, 0.25, 0.0]);
    assert!(near(v[0], 0.0), "repeat: {v:?}");
    set(SamplerSpec {
        min: Filter::Nearest,
        mag: Filter::Nearest,
        address: [Address::MirrorRepeat; 3],
        ..base
    });
    let v = sample(&rig, &sheap, "sample_f", [1.25, 0.25, 0.0]);
    assert!(near(v[0], 1.0), "mirror: {v:?}");
    set(SamplerSpec {
        min: Filter::Nearest,
        mag: Filter::Nearest,
        address: [Address::ClampToBorder; 3],
        border: Border::OpaqueWhite,
        ..base
    });
    let v = sample(&rig, &sheap, "sample_f", [-0.5, 0.25, 0.0]);
    assert!(near(v[0], 1.0) && near(v[3], 1.0), "border white: {v:?}");
    set(SamplerSpec {
        min: Filter::Nearest,
        mag: Filter::Nearest,
        address: [Address::ClampToBorder; 3],
        border: Border::TransparentBlack,
        ..base
    });
    let v = sample(&rig, &sheap, "sample_f", [-0.5, 0.25, 0.0]);
    // A one-channel texture reads (r, 0, 0, 1): the border colour only has r to give.
    assert!(near(v[0], 0.0) && near(v[1], 0.0), "border black: {v:?}");

    // Comparison sampler on a depth texture.
    let dt = d
        .create_texture(&TextureDesc::tex2d(dxgi::D32_FLOAT, 2, 2, DS | SRV))
        .unwrap();
    let up = d.create_buffer(BufferKind::Upload, 64).unwrap();
    up.write(0, &0.5f32.to_le_bytes().repeat(4)).unwrap();
    let mut l = q.begin_list().unwrap();
    l.copy_buffer_to_texture(
        &loc(&dt, 0, 0),
        &buf_loc(&up, dxgi::D32_FLOAT, 8, 2),
        [2, 2, 1],
    )
    .unwrap();
    l.end().unwrap();
    run(&q, l);
    let mut s = ViewSpec::whole(ViewDim::D2);
    s.format = dxgi::R32_FLOAT;
    rig.srv(0, dt.view(&s).unwrap());
    set(SamplerSpec {
        min: Filter::Nearest,
        mag: Filter::Nearest,
        compare: Some(Compare::LessEqual),
        ..base
    });
    assert!(
        near(sample(&rig, &sheap, "sample_cmp", [0.5, 0.5, 0.25])[0], 1.0),
        "0.25 <= 0.5"
    );
    assert!(
        near(sample(&rig, &sheap, "sample_cmp", [0.5, 0.5, 0.75])[0], 0.0),
        "0.75 > 0.5"
    );
}

#[test]
fn a_shader_writes_a_texture_uav() {
    let (_g, d, q) = setup();
    let rig = Rig::new(&d, &q);
    let t = d
        .create_texture(&TextureDesc::tex2d(
            dxgi::R32G32B32A32_FLOAT,
            8,
            8,
            UAV | SRV,
        ))
        .unwrap();
    rig.srv(0, t.view(&ViewSpec::whole(ViewDim::D2)).unwrap());
    rig.run_kernel("write_f", &[0, 0, 0, 0], [1, 1, 1], [8, 8, 1]);
    let got = download(&d, &q, &t, 0, 0, 128);
    for y in 0..8usize {
        for x in 0..8usize {
            let o = y * 128 + x * 16;
            let f =
                |i: usize| f32::from_le_bytes(got[o + 4 * i..o + 4 * i + 4].try_into().unwrap());
            assert_eq!(
                [f(0), f(1), f(2), f(3)],
                [x as f32, y as f32, 7.0, 1.0],
                "({x},{y})"
            );
        }
    }
    // A uint texture through a typeless one.
    let t = d
        .create_texture(&TextureDesc::tex2d(
            dxgi::R8G8B8A8_TYPELESS,
            4,
            4,
            UAV | SRV,
        ))
        .unwrap();
    let mut s = ViewSpec::whole(ViewDim::D2);
    s.format = dxgi::R8G8B8A8_UINT;
    rig.srv(0, t.view(&s).unwrap());
    rig.run_kernel("write_u", &[0, 0, 0, 0], [1, 1, 1], [4, 4, 1]);
    let got = download(&d, &q, &t, 0, 0, 16);
    for y in 0..4usize {
        for x in 0..4usize {
            assert_eq!(
                got[y * 16 + x * 4..][..4],
                [x as u8 + 1, y as u8 + 1, 3, 4],
                "({x},{y})"
            );
        }
    }
}

// ---- heaps ------------------------------------------------------------------

#[test]
fn heap_sizes_alignments_and_placement_errors() {
    let (_g, d, _q) = setup();
    let desc = TextureDesc {
        dim: TexDim::D2,
        format: dxgi::R8G8B8A8_UNORM,
        width: 256,
        height: 256,
        depth_or_array: 1,
        mips: 9,
        samples: 1,
        usage: SRV,
    };
    let ta = d.heap_texture_size_align(&desc).unwrap();
    assert!(ta.size >= 256 * 256 * 4, "{ta:?}");
    assert!(ta.align.is_power_of_two(), "{ta:?}");
    let ba = d.heap_buffer_size_align(BufferKind::Default, 1000);
    assert!(ba.size >= 1000 && ba.align.is_power_of_two(), "{ba:?}");
    // Block compressed textures are smaller than the same size uncompressed.
    let bc = TextureDesc {
        format: dxgi::BC7_UNORM,
        ..desc
    };
    assert!(d.heap_texture_size_align(&bc).unwrap().size < ta.size);

    let heap = d
        .create_heap(BufferKind::Default, 4 * ta.size.max(ba.size))
        .unwrap();
    assert!(heap.size() >= 4 * ta.size);
    let t = heap.create_texture(&d, 0, &desc).unwrap();
    assert!(t.is_resident());
    // Misaligned, past the end, wrong heap kind, zero size.
    let mis = heap.create_texture(&d, ta.align.max(2) / 2 + 1, &desc);
    assert!(matches!(mis, Err(Error::BadArgument(m)) if m.contains("alignment")));
    let past = heap.create_texture(&d, heap.size(), &desc);
    assert!(matches!(past, Err(Error::BadArgument(m)) if m.contains("past the heap")));
    let up = d.create_heap(BufferKind::Upload, 1 << 20).unwrap();
    assert!(matches!(
        up.create_texture(&d, 0, &desc),
        Err(Error::Unsupported(_))
    ));
    assert!(d.create_heap(BufferKind::Default, 0).is_err());
    assert!(heap.create_buffer(&d, 0, 0).is_err());
}

#[test]
fn placed_buffers_and_textures_work_and_live_as_long_as_their_heap_is_needed() {
    let (_g, d, q) = setup();
    // Settle what earlier tests retired, then count.
    q.wait_idle(WAIT_MS).unwrap();
    d.maintain();
    let before = d
        .residency()
        .len(victory_metal_back::residency::SetKind::Resource);
    let rig = Rig::new(&d, &q);
    let heap = d.create_heap(BufferKind::Default, 8 << 20).unwrap();
    let desc = TextureDesc::tex2d(dxgi::R32_UINT, 64, 64, SRV | UAV);
    let ta = d.heap_texture_size_align(&desc).unwrap();
    let t = heap.create_texture(&d, 0, &desc).unwrap();
    let off = ta.size.next_multiple_of(1 << 16);
    let b = heap.create_buffer(&d, off, 4096).unwrap();
    assert!(b.is_resident());
    let img = pattern(64 * 64 * 4, 77);
    upload(&d, &q, &t, 0, 0, &img, 256);
    // Drop the heap handle first: the placed resources keep it.
    drop(heap);
    d.maintain();
    assert_eq!(download(&d, &q, &t, 0, 0, 256), img);
    // A shader reads the placed texture through a descriptor.
    rig.srv(0, t.view(&ViewSpec::whole(ViewDim::D2)).unwrap());
    let v = rig.read_u("read_u", &[0, 3, 2, 0]);
    let px =
        |x: usize, y: usize| u32::from_le_bytes(img[y * 256 + x * 4..][..4].try_into().unwrap());
    assert_eq!(v[0], px(3, 2));
    // The placed buffer copies like any buffer.
    let up = d.create_buffer(BufferKind::Upload, 4096).unwrap();
    up.write(0, &pattern(4096, 5)).unwrap();
    let rb = d.create_buffer(BufferKind::Readback, 4096).unwrap();
    let mut l = q.begin_list().unwrap();
    l.copy_buffer(&b, 0, &up, 0, 4096).unwrap();
    l.copy_buffer(&rb, 0, &b, 0, 4096).unwrap();
    l.end().unwrap();
    run(&q, l);
    let mut got = vec![0u8; 4096];
    rb.read(0, &mut got).unwrap();
    assert_eq!(got, pattern(4096, 5));
    drop((t, b, up, rb, rig));
    q.wait_idle(WAIT_MS).unwrap();
    d.maintain();
    assert_eq!(
        d.residency()
            .len(victory_metal_back::residency::SetKind::Resource),
        before,
        "everything placed was released from the residency set"
    );
}

#[test]
fn aliased_placements_are_ordered_by_the_aliasing_barrier() {
    let (_g, d, q) = setup();
    let heap = d.create_heap(BufferKind::Default, 4 << 20).unwrap();
    // Two buffers and two textures on the same bytes.
    let a = heap.create_buffer(&d, 0, 1 << 16).unwrap();
    let b = heap.create_buffer(&d, 0, 1 << 16).unwrap();
    let desc = TextureDesc::tex2d(dxgi::R32_UINT, 64, 64, SRV);
    let sa = d.heap_texture_size_align(&desc).unwrap();
    let off = (1u64 << 20).next_multiple_of(sa.align);
    let t1 = heap.create_texture(&d, off, &desc).unwrap();
    let t2 = heap.create_texture(&d, off, &desc).unwrap();
    for round in 0..20u32 {
        let up1 = d.create_buffer(BufferKind::Upload, 1 << 16).unwrap();
        let up2 = d.create_buffer(BufferKind::Upload, 1 << 16).unwrap();
        up1.write(0, &pattern(1 << 16, round)).unwrap();
        up2.write(0, &pattern(1 << 16, round + 1000)).unwrap();
        let rb = d.create_buffer(BufferKind::Readback, 1 << 16).unwrap();
        let rt = d.create_buffer(BufferKind::Readback, 64 * 64 * 4).unwrap();
        let mut l = q.begin_list().unwrap();
        // Use a, then alias to b: b's write must not overtake a's reads.
        l.copy_buffer(&a, 0, &up1, 0, 1 << 16).unwrap();
        l.aliasing_barrier().unwrap();
        l.copy_buffer(&b, 0, &up2, 0, 1 << 16).unwrap();
        l.aliasing_barrier().unwrap();
        l.copy_buffer(&rb, 0, &b, 0, 1 << 16).unwrap();
        // Textures: t1 filled, aliased by t2 which is filled and read back.
        l.copy_buffer_to_texture(
            &loc(&t1, 0, 0),
            &buf_loc(&up1, dxgi::R32_UINT, 256, 64),
            [64, 64, 1],
        )
        .unwrap();
        l.aliasing_barrier().unwrap();
        l.copy_buffer_to_texture(
            &loc(&t2, 0, 0),
            &buf_loc(&up2, dxgi::R32_UINT, 256, 64),
            [64, 64, 1],
        )
        .unwrap();
        l.aliasing_barrier().unwrap();
        l.copy_texture_to_buffer(
            &buf_loc(&rt, dxgi::R32_UINT, 256, 64),
            &loc(&t2, 0, 0),
            [64, 64, 1],
        )
        .unwrap();
        l.end().unwrap();
        run(&q, l);
        let mut got = vec![0u8; 1 << 16];
        rb.read(0, &mut got).unwrap();
        assert_eq!(
            got,
            pattern(1 << 16, round + 1000),
            "buffer alias, round {round}"
        );
        rt.read(0, &mut got[..64 * 64 * 4]).unwrap();
        assert_eq!(
            got[..64 * 64 * 4],
            pattern(1 << 16, round + 1000)[..64 * 64 * 4],
            "texture alias, round {round}"
        );
    }
}

#[test]
fn upload_and_readback_heaps_place_buffers_with_a_cpu_pointer() {
    let (_g, d, _q) = setup();
    let heap = d.create_heap(BufferKind::Upload, 1 << 20).unwrap();
    let sa = d.heap_buffer_size_align(BufferKind::Upload, 1024);
    let b = heap.create_buffer(&d, 0, 1024).unwrap();
    assert_ne!(b.cpu_addr(), 0);
    b.write(0, &[9; 16]).unwrap();
    let c = heap
        .create_buffer(&d, sa.size.next_multiple_of(sa.align), 1024)
        .unwrap();
    assert_ne!(c.cpu_addr(), b.cpu_addr());
    assert_ne!(c.gpu_va(), b.gpu_va());
}

// ---- UAV clears ---------------------------------------------------------------

use victory_metal_back::clear::ClearRequest;

fn clear_req(values: [u32; 4], float_values: bool) -> ClearRequest {
    ClearRequest {
        format: 0,
        mip: 0,
        first_slice: 0,
        slice_count: 0,
        values,
        float_values,
        rect: None,
    }
}

fn do_clear(q: &Queue, d: &Device, t: &Texture, req: &ClearRequest) -> Result<(), Error> {
    let kernels = d.clear_kernels();
    let mut l = q.begin_list().unwrap();
    let r = l.clear_texture(&kernels, t, req);
    l.end().unwrap();
    if r.is_ok() {
        run(q, l);
    }
    r
}

#[test]
fn clearing_a_texture_uav_covers_the_rectangle_slices_and_mips() {
    let (_g, d, q) = setup();
    // Float clear of a 2D array, one slice of one mip, with a rectangle.
    let desc = TextureDesc {
        dim: TexDim::D2,
        format: dxgi::R8G8B8A8_UNORM,
        width: 8,
        height: 8,
        depth_or_array: 3,
        mips: 2,
        samples: 1,
        usage: SRV | UAV,
    };
    let t = d.create_texture(&desc).unwrap();
    for s in 0..3 {
        for m in 0..2 {
            let [w, h, _] = desc.mip_size(m);
            upload(&d, &q, &t, m, s, &vec![0u8; (w * h * 4) as usize], w * 4);
        }
    }
    let mut r = clear_req(
        [1.0f32.to_bits(), 0.5f32.to_bits(), 0, 1.0f32.to_bits()],
        true,
    );
    r.first_slice = 1;
    r.slice_count = 1;
    r.rect = Some([2, 1, 6, 3]);
    do_clear(&q, &d, &t, &r).unwrap();
    for s in 0..3u32 {
        let got = download(&d, &q, &t, 0, s, 32);
        for y in 0..8usize {
            for x in 0..8usize {
                let inside = s == 1 && (2..6).contains(&x) && (1..3).contains(&y);
                let px = &got[y * 32 + x * 4..][..4];
                let want: [u8; 4] = if inside { [255, 128, 0, 255] } else { [0; 4] };
                assert_eq!(px, want, "slice {s} ({x},{y})");
            }
        }
        // mip 1 untouched
        assert!(download(&d, &q, &t, 1, s, 16).iter().all(|&b| b == 0));
    }
    // All slices of mip 1.
    let mut r = clear_req(
        [
            0.0f32.to_bits(),
            1.0f32.to_bits(),
            0.0f32.to_bits(),
            1.0f32.to_bits(),
        ],
        true,
    );
    r.mip = 1;
    do_clear(&q, &d, &t, &r).unwrap();
    for s in 0..3 {
        let got = download(&d, &q, &t, 1, s, 16);
        assert!(got.chunks(4).all(|p| p == [0, 255, 0, 255]), "slice {s}");
    }
}

#[test]
fn clearing_uint_1d_and_3d_and_through_typeless_views() {
    let (_g, d, q) = setup();
    let t1 = d
        .create_texture(&TextureDesc {
            dim: TexDim::D1,
            format: dxgi::R32_UINT,
            width: 16,
            height: 1,
            depth_or_array: 1,
            mips: 1,
            samples: 1,
            usage: UAV,
        })
        .unwrap();
    let mut r = clear_req([77, 0, 0, 0], false);
    r.rect = Some([4, 0, 10, 1]);
    do_clear(&q, &d, &t1, &r).unwrap();
    let got = download(&d, &q, &t1, 0, 0, 64);
    for x in 0..16usize {
        let v = u32::from_le_bytes(got[x * 4..][..4].try_into().unwrap());
        assert_eq!(v, if (4..10).contains(&x) { 77 } else { 0 }, "x {x}");
    }
    let t3 = d
        .create_texture(&TextureDesc {
            dim: TexDim::D3,
            format: dxgi::R16G16B16A16_UINT,
            width: 4,
            height: 4,
            depth_or_array: 4,
            mips: 1,
            samples: 1,
            usage: UAV,
        })
        .unwrap();
    do_clear(&q, &d, &t3, &clear_req([1, 2, 3, 4], false)).unwrap();
    let got = download(&d, &q, &t3, 0, 0, 32);
    for texel in got.chunks(8) {
        let v: Vec<u16> = texel
            .chunks(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(v, [1, 2, 3, 4]);
    }
    // Typeless texture cleared as a uint view.
    let tt = d
        .create_texture(&TextureDesc::tex2d(dxgi::R8G8B8A8_TYPELESS, 4, 4, UAV))
        .unwrap();
    let mut r = clear_req([9, 8, 7, 6], false);
    r.format = dxgi::R8G8B8A8_UINT;
    do_clear(&q, &d, &tt, &r).unwrap();
    assert!(
        download(&d, &q, &tt, 0, 0, 16)
            .chunks(4)
            .all(|p| p == [9, 8, 7, 6])
    );
}

#[test]
fn clears_that_cannot_be_done_exactly_are_refused() {
    let (_g, d, q) = setup();
    let t = d
        .create_texture(&TextureDesc::tex2d(dxgi::R8G8B8A8_UNORM, 4, 4, UAV))
        .unwrap();
    // Uint pattern on an 8-bit unorm view: undefined here, refused; zero is fine.
    assert!(matches!(
        do_clear(&q, &d, &t, &clear_req([1, 2, 3, 4], false)),
        Err(Error::Unsupported(_))
    ));
    do_clear(&q, &d, &t, &clear_req([0; 4], false)).unwrap();
    let u = d
        .create_texture(&TextureDesc::tex2d(dxgi::R32_UINT, 4, 4, UAV))
        .unwrap();
    assert!(matches!(
        do_clear(&q, &d, &u, &clear_req([0; 4], true)),
        Err(Error::Unsupported(_))
    ));
    let mut r = clear_req([0; 4], false);
    r.rect = Some([0, 0, 5, 4]);
    assert!(matches!(
        do_clear(&q, &d, &u, &r),
        Err(Error::BadArgument(_))
    ));
    r.rect = Some([3, 0, 3, 4]);
    assert!(matches!(
        do_clear(&q, &d, &u, &r),
        Err(Error::BadArgument(_))
    ));
    let mut r = clear_req([0; 4], false);
    r.mip = 1;
    assert!(matches!(
        do_clear(&q, &d, &u, &r),
        Err(Error::BadArgument(_))
    ));
    let no_uav = d
        .create_texture(&TextureDesc::tex2d(dxgi::R32_UINT, 4, 4, SRV))
        .unwrap();
    assert!(matches!(
        do_clear(&q, &d, &no_uav, &clear_req([0; 4], false)),
        Err(Error::BadArgument(_))
    ));
}

/// A 1D texture written through a shader view and read back through another:
/// the view of a 1D texture is a `texture1d`, whole-texture views included.
#[test]
fn a_1d_texture_is_written_and_read_through_views() {
    let (_g, d, q) = setup();
    let rig = Rig::new(&d, &q);
    let t = d
        .create_texture(&TextureDesc {
            dim: TexDim::D1,
            format: dxgi::R32_FLOAT,
            width: 4096,
            height: 1,
            depth_or_array: 1,
            mips: 1,
            samples: 1,
            usage: SRV | UAV,
        })
        .unwrap();
    let id = t.view(&ViewSpec::whole(ViewDim::D1)).unwrap();
    rig.heap
        .write(0, IrDescriptorTableEntry::texture(id, 0.0, 0))
        .unwrap();
    rig.run_kernel("write_1d", &[0, 0, 0, 0], [512, 1, 1], [8, 1, 1]);
    let out = d.create_buffer(BufferKind::Readback, 4096 * 4).unwrap();
    let mut l = q.begin_list().unwrap();
    l.copy_texture_to_buffer(
        &buf_loc(&out, dxgi::R32_FLOAT, 4096 * 4, 1),
        &loc(&t, 0, 0),
        [4096, 1, 1],
    )
    .unwrap();
    l.end().unwrap();
    run(&q, l);
    let w = out.read_u32s(0, 4096).unwrap();
    assert_eq!(f32::from_bits(w[135]), 135.0 * 0.25);
    assert_eq!(f32::from_bits(w[4095]), 4095.0 * 0.25);
}
