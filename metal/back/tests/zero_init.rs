//! `VICTORY_METAL_ZERO_INIT`: what the zeroing helpers do to dirty memory (white branch).
#![cfg(target_vendor = "apple")]

use victory_metal_back::copy::{BufLoc, TexLoc};
use victory_metal_back::device::Device;
use victory_metal_back::format::dxgi;
use victory_metal_back::memory::BufferKind;
use victory_metal_back::queue::Queue;
use victory_metal_back::texture::TextureDesc;
use victory_metal_back::zero_init;

fn run(q: &Queue, mut l: victory_metal_back::record::CommandList) {
    l.end().unwrap();
    let v = q.submit(vec![l]).unwrap();
    q.wait_for(v, 20_000).unwrap();
}

#[test]
fn zero_init_clears_a_shared_and_a_private_buffer_and_a_mipped_texture() {
    let d = Device::new().expect("Metal 4 device");
    let q = Queue::new(&d).unwrap();
    // Shared buffer: dirty it, zero it.
    let b = d.create_buffer(BufferKind::Shared, 4096).unwrap();
    b.write(0, &[0xAB; 4096]).unwrap();
    zero_init::buffer(&d, &b);
    let mut got = vec![1u8; 4096];
    b.read(0, &mut got).unwrap();
    assert!(got.iter().all(|&x| x == 0));
    // Private buffer: fill with a pattern through a list, zero it, read back.
    let p = d.create_buffer(BufferKind::Default, 4096).unwrap();
    let mut l = q.begin_list().unwrap();
    l.fill_buffer(&p, 0, 4096, 0x5A).unwrap();
    run(&q, l);
    zero_init::buffer(&d, &p);
    let rb = d.create_buffer(BufferKind::Readback, 4096).unwrap();
    let mut l = q.begin_list().unwrap();
    l.copy_buffer(&rb, 0, &p, 0, 4096).unwrap();
    run(&q, l);
    let mut got = vec![1u8; 4096];
    rb.read(0, &mut got).unwrap();
    assert!(got.iter().all(|&x| x == 0));
    // A texture of 3 mips and 2 slices: dirty every texel of mip 1 slice 1, zero, read it back.
    let mut desc = TextureDesc::tex2d(dxgi::R32_FLOAT, 16, 16, 1);
    desc.mips = 3;
    desc.depth_or_array = 2;
    let t = d.create_texture(&desc).unwrap();
    let up = d.create_buffer(BufferKind::Upload, 4096).unwrap();
    up.write(0, &[0x77; 4096]).unwrap();
    let loc = TexLoc {
        texture: &t,
        mip: 1,
        slice: 1,
        plane: 0,
        origin: [0; 3],
    };
    let mut l = q.begin_list().unwrap();
    l.copy_buffer_to_texture(
        &loc,
        &BufLoc {
            buffer: &up,
            offset: 0,
            format: dxgi::R32_FLOAT,
            row_pitch: 256,
            rows: 8,
        },
        [8, 8, 1],
    )
    .unwrap();
    run(&q, l);
    zero_init::texture(&d, &t);
    let mut l = q.begin_list().unwrap();
    l.copy_texture_to_buffer(
        &BufLoc {
            buffer: &rb,
            offset: 0,
            format: dxgi::R32_FLOAT,
            row_pitch: 256,
            rows: 8,
        },
        &loc,
        [8, 8, 1],
    )
    .unwrap();
    run(&q, l);
    let mut got = vec![1u8; 4096];
    rb.read(0, &mut got).unwrap();
    assert!(
        (0..8).all(|r| got[r * 256..r * 256 + 32].iter().all(|&x| x == 0)),
        "mip 1 slice 1 is zeros"
    );
}
