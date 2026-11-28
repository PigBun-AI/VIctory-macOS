//! D24S8 depth and stencil plane copies (white branch). The texture holds
//! 32-bit floats (`Depth32Float_Stencil8`); a buffer footprint of the depth
//! plane in `R24_UNORM_X8_TYPELESS` is 24-bit unorm words (`round(d * 16777215)`),
//! one in `R32_TYPELESS` moves the floats as they are. Upload, readback and
//! texture-to-texture, each through the real copy on the GPU.
#![cfg(target_vendor = "apple")]

use std::sync::{Mutex, MutexGuard, OnceLock};

use victory_metal_back::copy::{BufLoc, TexLoc};
use victory_metal_back::device::Device;
use victory_metal_back::format::dxgi;
use victory_metal_back::memory::{Buffer, BufferKind};
use victory_metal_back::queue::Queue;
use victory_metal_back::record::CommandList;
use victory_metal_back::texture::{Texture, TextureDesc};

const SRV: u32 = 1;
const DS: u32 = 8;

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
    q.wait_for(v, 20_000).expect("completes");
}

fn plane(t: &Texture, plane: u32) -> TexLoc<'_> {
    TexLoc {
        texture: t,
        mip: 0,
        slice: 0,
        plane,
        origin: [0; 3],
    }
}

fn foot(b: &Buffer, format: u32, offset: u64, row_pitch: u32, rows: u32) -> BufLoc<'_> {
    BufLoc {
        buffer: b,
        offset,
        format,
        row_pitch,
        rows,
    }
}

fn words(w: &[u32]) -> Vec<u8> {
    w.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn upload_buf(d: &Device, bytes: &[u8]) -> Buffer {
    let b = d
        .create_buffer(BufferKind::Upload, bytes.len().max(256) as u64)
        .unwrap();
    b.write(0, bytes).unwrap();
    b
}

fn read_words(b: &Buffer, n: usize) -> Vec<u32> {
    let mut got = vec![0u8; n * 4];
    b.read(0, &mut got).unwrap();
    got.chunks(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

const W: u32 = 11;
const H: u32 = 5;
/// Row pitch in bytes, padded past a row (44 bytes) and to 4.
const PITCH: u32 = 64;
const OFFSET: u64 = 128;

/// The 24-bit values of the region, with the ends and the middle in.
fn unorm_values() -> Vec<u32> {
    (0..W * H)
        .map(|i| match i {
            0 => 0,
            1 => 0x00FF_FFFF,
            2 => 0x0080_0000,
            3 => 1,
            _ => (i.wrapping_mul(2_654_435_761) >> 8) & 0x00FF_FFFF,
        })
        .collect()
}

/// A region as words in a padded buffer: row `y` at `OFFSET + y * PITCH`.
fn laid_out(v: &[u32], fill: u32) -> Vec<u32> {
    let mut out = vec![fill; (OFFSET as usize + PITCH as usize * H as usize) / 4];
    for y in 0..H as usize {
        for x in 0..W as usize {
            out[OFFSET as usize / 4 + y * PITCH as usize / 4 + x] = v[y * W as usize + x];
        }
    }
    out
}

fn region(out: &[u32]) -> Vec<u32> {
    let mut v = Vec::new();
    for y in 0..H as usize {
        for x in 0..W as usize {
            v.push(out[OFFSET as usize / 4 + y * PITCH as usize / 4 + x]);
        }
    }
    v
}

fn d24(d: &Device) -> Texture {
    d.create_texture(&TextureDesc::tex2d(dxgi::R24G8_TYPELESS, W, H, DS | SRV))
        .unwrap()
}

/// Floats of the depth plane through the `R32_TYPELESS` footprint, as they are.
fn read_floats(d: &Device, q: &Queue, t: &Texture) -> Vec<f32> {
    let rb = d.create_buffer(BufferKind::Readback, 1024).unwrap();
    let mut l = q.begin_list().unwrap();
    l.copy_texture_to_buffer(
        &foot(&rb, dxgi::R32_TYPELESS, 0, 64, H),
        &plane(t, 0),
        [W, H, 1],
    )
    .unwrap();
    l.end().unwrap();
    run(q, l);
    let mut v = Vec::new();
    let w = read_words(&rb, 16 * H as usize);
    for y in 0..H as usize {
        for x in 0..W as usize {
            v.push(f32::from_bits(w[y * 16 + x]));
        }
    }
    v
}

#[test]
fn a_d24_footprint_upload_becomes_floats_in_the_depth_plane() {
    let (_g, d, q) = setup();
    let t = d24(&d);
    let vals = unorm_values();
    // The top byte is garbage in the source: the X8 is ignored.
    let src = upload_buf(&d, &words(&laid_out(&vals, 0xAB00_0000)));
    let mut l = q.begin_list().unwrap();
    l.copy_buffer_to_texture(
        &plane(&t, 0),
        &foot(&src, dxgi::R24_UNORM_X8_TYPELESS, OFFSET, PITCH, H),
        [W, H, 1],
    )
    .unwrap();
    l.end().unwrap();
    run(&q, l);
    let got = read_floats(&d, &q, &t);
    for (i, (g, v)) in got.iter().zip(&vals).enumerate() {
        let want = *v as f32 / 16_777_215.0;
        assert!(
            (g - want).abs() <= want.abs() * 2e-7 + 1e-9,
            "texel {i}: {g} vs {want} (unorm {v:#x})"
        );
    }
    assert_eq!(got[1], 1.0, "the largest value is exactly 1");
    assert_eq!(got[0], 0.0);
}

#[test]
fn a_d24_depth_plane_reads_back_as_unorm24_words_with_a_zero_top_byte() {
    let (_g, d, q) = setup();
    let t = d24(&d);
    let floats: Vec<f32> = unorm_values()
        .iter()
        .enumerate()
        .map(|(i, u)| match i {
            5 => 1.5,   // above the range: saturates
            6 => -0.25, // below: saturates
            _ => *u as f32 / 16_777_215.0,
        })
        .collect();
    let mut flat = vec![0u32; 16 * H as usize];
    for y in 0..H as usize {
        for x in 0..W as usize {
            flat[y * 16 + x] = floats[y * W as usize + x].to_bits();
        }
    }
    let src = upload_buf(&d, &words(&flat));
    let rb = d
        .create_buffer(
            BufferKind::Readback,
            OFFSET + u64::from(PITCH) * u64::from(H),
        )
        .unwrap();
    rb.write(
        0,
        &vec![0xEEu8; (OFFSET + u64::from(PITCH) * u64::from(H)) as usize],
    )
    .unwrap();
    let mut l = q.begin_list().unwrap();
    l.copy_buffer_to_texture(
        &plane(&t, 0),
        &foot(&src, dxgi::R32_TYPELESS, 0, 64, H),
        [W, H, 1],
    )
    .unwrap();
    l.copy_texture_to_buffer(
        &foot(&rb, dxgi::R24_UNORM_X8_TYPELESS, OFFSET, PITCH, H),
        &plane(&t, 0),
        [W, H, 1],
    )
    .unwrap();
    l.end().unwrap();
    run(&q, l);
    let out = read_words(&rb, (OFFSET as usize + PITCH as usize * H as usize) / 4);
    let got = region(&out);
    let vals = unorm_values();
    for (i, g) in got.iter().enumerate() {
        let want = match i {
            5 => 0x00FF_FFFF,
            6 => 0,
            _ => vals[i],
        };
        assert!(
            g.abs_diff(want) <= 1 && g >> 24 == 0,
            "texel {i}: {g:#x} vs {want:#x}"
        );
    }
    // Bytes outside the region (before it, and the padding of a row) are untouched.
    assert_eq!(out[0], 0xEEEE_EEEE, "before the region");
    assert_eq!(
        out[OFFSET as usize / 4 + W as usize],
        0xEEEE_EEEE,
        "row padding"
    );
}

#[test]
fn d24_planes_copy_between_textures_and_stencil_footprints_of_the_title_work() {
    let (_g, d, q) = setup();
    let (a, b) = (d24(&d), d24(&d));
    let vals = unorm_values();
    let src = upload_buf(&d, &words(&laid_out(&vals, 0)));
    let stencil: Vec<u8> = (0..(W * H) as usize).map(|i| (i * 7 + 3) as u8).collect();
    // Stencil rows of 16 bytes.
    let mut st = vec![0u8; 16 * H as usize];
    for y in 0..H as usize {
        for x in 0..W as usize {
            st[y * 16 + x] = stencil[y * W as usize + x];
        }
    }
    let st_src = upload_buf(&d, &st);
    let st_rb = d.create_buffer(BufferKind::Readback, 256).unwrap();
    let mut l = q.begin_list().unwrap();
    l.copy_buffer_to_texture(
        &plane(&a, 0),
        &foot(&src, dxgi::R24_UNORM_X8_TYPELESS, OFFSET, PITCH, H),
        [W, H, 1],
    )
    .unwrap();
    // The stencil plane through the name D3D12 gives the footprint of a D24S8 stencil plane.
    l.copy_buffer_to_texture(
        &plane(&a, 1),
        &foot(&st_src, dxgi::X24_TYPELESS_G8_UINT, 0, 16, H),
        [W, H, 1],
    )
    .unwrap();
    // Plane to plane (b's stencil stays as it was), then b's both planes out.
    l.copy_texture(&plane(&b, 0), &plane(&a, 0), [W, H, 1])
        .unwrap();
    l.copy_texture(&plane(&b, 1), &plane(&a, 1), [W, H, 1])
        .unwrap();
    l.copy_texture_to_buffer(
        &foot(&st_rb, dxgi::X24_TYPELESS_G8_UINT, 0, 16, H),
        &plane(&b, 1),
        [W, H, 1],
    )
    .unwrap();
    l.end().unwrap();
    run(&q, l);
    let mut got = vec![0u8; 16 * H as usize];
    st_rb.read(0, &mut got).unwrap();
    for y in 0..H as usize {
        assert_eq!(
            &got[y * 16..y * 16 + W as usize],
            &st[y * 16..y * 16 + W as usize],
            "stencil row {y}"
        );
    }
    let fl = read_floats(&d, &q, &b);
    for (i, (g, v)) in fl.iter().zip(&vals).enumerate() {
        let want = *v as f32 / 16_777_215.0;
        assert!(
            (g - want).abs() <= want.abs() * 2e-7 + 1e-9,
            "texel {i}: {g} vs {want}"
        );
    }
}

#[test]
fn the_d32s8_depth_footprint_name_of_d3d12_is_accepted() {
    let (_g, d, q) = setup();
    let t = d
        .create_texture(&TextureDesc::tex2d(dxgi::R32G8X24_TYPELESS, W, H, DS | SRV))
        .unwrap();
    let vals: Vec<u32> = (0..W * H).map(|i| (i as f32 / 100.0).to_bits()).collect();
    let mut flat = vec![0u32; 16 * H as usize];
    for y in 0..H as usize {
        for x in 0..W as usize {
            flat[y * 16 + x] = vals[y * W as usize + x];
        }
    }
    let src = upload_buf(&d, &words(&flat));
    let rb = d.create_buffer(BufferKind::Readback, 1024).unwrap();
    let mut l = q.begin_list().unwrap();
    l.copy_buffer_to_texture(
        &plane(&t, 0),
        &foot(&src, dxgi::R32_FLOAT_X8X24_TYPELESS, 0, 64, H),
        [W, H, 1],
    )
    .unwrap();
    l.copy_texture_to_buffer(
        &foot(&rb, dxgi::R32_FLOAT_X8X24_TYPELESS, 0, 64, H),
        &plane(&t, 0),
        [W, H, 1],
    )
    .unwrap();
    l.end().unwrap();
    run(&q, l);
    let w = read_words(&rb, 16 * H as usize);
    for y in 0..H as usize {
        assert_eq!(
            &w[y * 16..y * 16 + W as usize],
            &vals[y * W as usize..(y + 1) * W as usize]
        );
    }
}
