//! Typed buffer views (`Buffer<T>`, `RWBuffer<T>`) on the GPU: DXIL from HLSL
//! through MSC, descriptors written with `DescriptorHeap::write_typed_buffer`
//! (texture buffers over the buffers), results compared with the CPU. Native
//! on arm64 and under Rosetta (`--target x86_64-apple-darwin`). The DXIL
//! tests skip without a DXC (see `common_msc.rs`).
#![cfg(target_vendor = "apple")]

mod common_msc;

use std::sync::{Mutex, MutexGuard, OnceLock};

use victory_metal_back::Error;
use victory_metal_back::descriptors::{DescriptorHeap, IrDescriptorTableEntry};
use victory_metal_back::device::Device;
use victory_metal_back::memory::{Buffer, BufferKind};
use victory_metal_back::msc::{
    CompileOptions, Param, ParamKind, RootSignatureDesc, Visibility, flags,
};
use victory_metal_back::pipeline::ComputePipeline;
use victory_metal_back::queue::Queue;
use victory_metal_back::typed::{TYPED_FORMATS, ViewAccess, dxgi, typed_format};

use common_msc::dxil_or_skip;

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

/// A pipeline over four root constants and the resource heap.
fn pipeline(d: &Device, dxil: &[u8]) -> (ComputePipeline, u32, u64) {
    let root = RootSignatureDesc {
        flags: flags::HEAP_INDEXED | flags::SAMPLER_HEAP_INDEXED,
        params: vec![Param {
            kind: ParamKind::Constants {
                register: 0,
                space: 0,
                num_32bit: 4,
            },
            visibility: Visibility::All,
        }],
        ..Default::default()
    };
    let sp = d
        .compute_pipeline_from_dxil(dxil, Some(&root), &CompileOptions::default())
        .expect("pipeline");
    let size = sp.reflection.layout.size;
    let offset = u64::from(sp.reflection.layout.entries[0].offset);
    (sp.pipeline, size, offset)
}

fn dispatch(
    q: &Queue,
    p: &(ComputePipeline, u32, u64),
    heap: &DescriptorHeap,
    constants: [u32; 4],
    group: u32,
) {
    let tlab = q.alloc_constants(u64::from(p.1), 8).unwrap();
    tlab.zero();
    tlab.write_u32s(p.2, &constants).unwrap();
    let mut l = q.begin_list().unwrap();
    l.set_heaps(heap, None).unwrap();
    l.set_root_arguments(&tlab).unwrap();
    l.dispatch(&p.0, [1, 1, 1], [group, 1, 1]).unwrap();
    l.end().unwrap();
    let v = q.submit(vec![l]).unwrap();
    q.wait_for(v, 20_000).unwrap();
}

fn r32(d: &Device) -> victory_metal_back::typed::TypedFormat {
    let _ = d;
    typed_format(dxgi::R32_UINT).unwrap()
}

const COPY: &str = "cbuffer c : register(b0) { uint si; uint di; uint n; uint pad; }
[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) {
  Buffer<uint> src = ResourceDescriptorHeap[si];
  RWBuffer<uint> dst = ResourceDescriptorHeap[di];
  uint sd, dd; src.GetDimensions(sd); dst.GetDimensions(dd);
  if (t.x < n) dst[t.x] = src[t.x] + (t.x == 0 ? 1000000 * sd + 1000 * dd : 0);
}";

/// `FirstElement` and `NumElements` are honored on both sides: the shader
/// reads the elements it was given, `GetDimensions` reports `NumElements`,
/// and nothing outside the written range changes.
#[test]
fn a_typed_srv_and_uav_with_first_element_and_count() {
    let Some(dxil) = dxil_or_skip("typed_copy", "cs_6_6", COPY) else {
        return;
    };
    let (_g, d, q) = setup();
    let p = pipeline(&d, &dxil);
    let f = r32(&d);
    // 16-byte alignment: first elements are multiples of 4.
    for (sfirst, dfirst, n) in [(0u64, 0u64, 16u64), (4, 0, 12), (0, 8, 16), (8, 4, 5)] {
        let src = d.create_buffer(BufferKind::Shared, 4096).unwrap();
        let dst = d.create_buffer(BufferKind::Shared, 4096).unwrap();
        let words: Vec<u32> = (0..1024u32).map(|i| i + 1).collect();
        src.write_u32s(0, &words).unwrap();
        dst.write_u32s(0, &[0xdead_beef; 1024]).unwrap();
        let heap = DescriptorHeap::new(&d, 8).unwrap();
        heap.write_typed_buffer(1, &src, &f, sfirst, n, ViewAccess::Read)
            .unwrap();
        heap.write_typed_buffer(2, &dst, &f, dfirst, n, ViewAccess::ReadWrite)
            .unwrap();
        dispatch(&q, &p, &heap, [1, 2, n as u32, 0], 32);
        let got = dst.read_u32s(0, 32).unwrap();
        let mut want = vec![0xdead_beefu32; 32];
        for i in 0..n {
            want[(dfirst + i) as usize] = (sfirst + i + 1) as u32;
        }
        want[dfirst as usize] += (1_000_000 + 1000) * n as u32;
        assert_eq!(got, want, "src first {sfirst}, dst first {dfirst}, n {n}");
    }
}

const HALF4: &str = "cbuffer c : register(b0) { uint si; uint di; uint n; uint pad; }
[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) {
  Buffer<float4> src = ResourceDescriptorHeap[si];
  RWBuffer<float4> dst = ResourceDescriptorHeap[di];
  if (t.x < n) dst[t.x] = src[t.x] * 0.5;
}";

fn half_to_f32(h: u16) -> f32 {
    let (s, e, m) = (
        u32::from(h >> 15),
        u32::from((h >> 10) & 31),
        u32::from(h & 1023),
    );
    let v = match e {
        0 => m as f32 / 1024.0 / 16384.0,
        31 => f32::INFINITY,
        _ => (1.0 + m as f32 / 1024.0) * 2f32.powi(e as i32 - 15),
    };
    if s == 1 { -v } else { v }
}

/// The format is the table's: a UNORM8 view is read as floats and an F16 view
/// is written as halves, in the memory of two different buffers.
#[test]
fn formats_convert_on_read_and_write() {
    let Some(dxil) = dxil_or_skip("typed_formats", "cs_6_6", HALF4) else {
        return;
    };
    let (_g, d, q) = setup();
    let p = pipeline(&d, &dxil);
    let src = d.create_buffer(BufferKind::Shared, 4096).unwrap();
    let dst = d.create_buffer(BufferKind::Shared, 4096).unwrap();
    // Eight texels of R8G8B8A8_UNORM: channel k of texel i is (i * 4 + k) * 8.
    let texels: Vec<u32> = (0..8u32)
        .map(|i| u32::from_le_bytes(std::array::from_fn(|k| ((i * 4 + k as u32) * 8) as u8)))
        .collect();
    src.write_u32s(0, &texels).unwrap();
    dst.write_u32s(0, &[0xffff_ffff; 64]).unwrap();
    let heap = DescriptorHeap::new(&d, 8).unwrap();
    let unorm = typed_format(dxgi::R8G8B8A8_UNORM).unwrap();
    let half4 = typed_format(dxgi::R16G16B16A16_FLOAT).unwrap();
    heap.write_typed_buffer(1, &src, &unorm, 0, 8, ViewAccess::Read)
        .unwrap();
    heap.write_typed_buffer(2, &dst, &half4, 0, 8, ViewAccess::ReadWrite)
        .unwrap();
    dispatch(&q, &p, &heap, [1, 2, 8, 0], 32);
    let words = dst.read_u32s(0, 17).unwrap();
    for i in 0..8usize {
        for k in 0..4usize {
            let w = words[i * 2 + k / 2];
            let h = (w >> (16 * (k % 2))) as u16;
            let want = f32::from((i * 4 + k) as u8 * 8) / 255.0 * 0.5;
            let got = half_to_f32(h);
            assert!(
                (got - want).abs() <= want * 1.0e-3 + 1.0e-6,
                "texel {i} channel {k}: {got} vs {want}"
            );
        }
    }
    assert_eq!(words[16], 0xffff_ffff, "nothing past the view is written");
}

const ATOMIC: &str = "cbuffer c : register(b0) { uint di; uint a; uint b; uint c; }
[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) {
  RWBuffer<uint> dst = ResourceDescriptorHeap[di];
  InterlockedAdd(dst[0], 1);
  InterlockedMax(dst[1], t.x);
}";

/// A 32-bit integer typed UAV takes atomics (the texture gets `ShaderAtomic`).
#[test]
fn a_typed_uav_takes_atomics() {
    let Some(dxil) = dxil_or_skip("typed_atomic", "cs_6_6", ATOMIC) else {
        return;
    };
    let (_g, d, q) = setup();
    let p = pipeline(&d, &dxil);
    let dst = d.create_buffer(BufferKind::Shared, 4096).unwrap();
    dst.write_u32s(0, &[0; 8]).unwrap();
    let heap = DescriptorHeap::new(&d, 8).unwrap();
    heap.write_typed_buffer(1, &dst, &r32(&d), 0, 8, ViewAccess::ReadWrite)
        .unwrap();
    dispatch(&q, &p, &heap, [1, 0, 0, 0], 32);
    let got = dst.read_u32s(0, 3).unwrap();
    assert_eq!((got[0], got[1], got[2]), (32, 31, 0));
}

fn entry(heap: &DescriptorHeap, i: u32) -> IrDescriptorTableEntry {
    heap.read(i).unwrap()
}

/// The entry a typed view writes: the buffer address of its first element, a
/// texture id, its length in bytes and the typed flag; the same request twice
/// shares one texture.
#[test]
fn a_typed_view_writes_the_entry_msc_reads_and_shares_textures() {
    let (_g, d, _q) = setup();
    let buf = d.create_buffer(BufferKind::Shared, 4096).unwrap();
    let heap = DescriptorHeap::new(&d, 8).unwrap();
    let f = r32(&d);
    heap.write_typed_buffer(0, &buf, &f, 4, 8, ViewAccess::Read)
        .unwrap();
    heap.write_typed_buffer(1, &buf, &f, 4, 8, ViewAccess::Read)
        .unwrap();
    heap.write_typed_buffer(2, &buf, &f, 4, 8, ViewAccess::ReadWrite)
        .unwrap();
    heap.write_typed_buffer(3, &buf, &f, 8, 8, ViewAccess::Read)
        .unwrap();
    let (a, b, c, e) = (
        entry(&heap, 0),
        entry(&heap, 1),
        entry(&heap, 2),
        entry(&heap, 3),
    );
    assert_eq!(a.gpu_va, buf.gpu_va() + 16);
    assert_ne!(a.texture_view_id, 0);
    assert_eq!(a, b, "same request, same entry");
    assert_ne!(
        a.texture_view_id, c.texture_view_id,
        "UAV is its own texture"
    );
    assert_ne!(a.texture_view_id, e.texture_view_id);
    assert_eq!(buf.texture_view_count(), 3);
    // Length in bytes (32) in the low half, the typed flag on top.
    assert_eq!(a.metadata & 0xffff_ffff, 32);
    assert_ne!(a.metadata >> 63, 0);
}

/// What cannot be a texture buffer is refused and the slot stays as it was.
#[test]
fn refused_views_write_nothing() {
    let (_g, d, _q) = setup();
    let buf = d.create_buffer(BufferKind::Shared, 4096).unwrap();
    let heap = DescriptorHeap::new(&d, 8).unwrap();
    let f = r32(&d);
    heap.write_typed_buffer(0, &buf, &f, 4, 8, ViewAccess::Read)
        .unwrap();
    let before = entry(&heap, 0);
    let align = d.typed_view_alignment(&f);
    assert!(align >= 4 && align.is_power_of_two(), "{align}");
    // A UAV not on the alignment: unsupported (a SRV gets a mirror, below).
    let e = heap
        .write_typed_buffer(0, &buf, &f, 1, 8, ViewAccess::ReadWrite)
        .unwrap_err();
    assert!(matches!(e, Error::Unsupported(_)), "{e}");
    // Past the end, empty, and overflowing ranges.
    for (first, n) in [(1020u64, 8u64), (0, 1025), (0, 0), (u64::MAX / 2, 4)] {
        let e = heap
            .write_typed_buffer(0, &buf, &f, first, n, ViewAccess::Read)
            .unwrap_err();
        assert!(matches!(e, Error::BadArgument(_)), "{first} {n}: {e}");
    }
    assert_eq!(entry(&heap, 0), before);
    assert_eq!(buf.texture_view_count(), 1);
}

/// Every format of the table makes a texture buffer the device accepts, as a
/// SRV and as a UAV, at the first element and at the next aligned one.
#[test]
fn every_table_format_makes_a_texture_buffer() {
    let (_g, d, _q) = setup();
    let buf = d.create_buffer(BufferKind::Shared, 1 << 16).unwrap();
    let heap = DescriptorHeap::new(&d, 4).unwrap();
    for &id in TYPED_FORMATS {
        let f = typed_format(id).unwrap();
        let align = d.typed_view_alignment(&f);
        let step = (align / u64::from(f.bytes)).max(1);
        for access in [ViewAccess::Read, ViewAccess::ReadWrite] {
            for first in [0, step] {
                heap.write_typed_buffer(0, &buf, &f, first, 16, access)
                    .unwrap_or_else(|e| panic!("format {id} {access:?} at {first}: {e}"));
            }
        }
    }
    // No pixel format for three 32-bit channels, typeless or depth formats.
    for id in [0, 1, 5, 6, 7, 8, 15, 19, 39, 40, 71] {
        assert!(typed_format(id).is_none(), "{id}");
    }
}

/// The textures go with the buffer: dropping a buffer that has views while a
/// dispatch that reads it is in flight neither crashes nor corrupts the
/// result, and the device keeps working after the retire.
#[test]
fn views_outlive_the_gpu_work_that_uses_them() {
    let Some(dxil) = dxil_or_skip("typed_life", "cs_6_6", COPY) else {
        return;
    };
    let (_g, d, q) = setup();
    let p = pipeline(&d, &dxil);
    let f = r32(&d);
    let dst = d.create_buffer(BufferKind::Shared, 4096).unwrap();
    dst.write_u32s(0, &[0; 64]).unwrap();
    for round in 0..8u32 {
        let src: Buffer = d.create_buffer(BufferKind::Shared, 1 << 20).unwrap();
        src.write_u32s(0, &vec![round + 1; 1024]).unwrap();
        let heap = DescriptorHeap::new(&d, 4).unwrap();
        heap.write_typed_buffer(1, &src, &f, 0, 32, ViewAccess::Read)
            .unwrap();
        heap.write_typed_buffer(2, &dst, &f, 0, 32, ViewAccess::ReadWrite)
            .unwrap();
        let tlab = q.alloc_constants(u64::from(p.1), 8).unwrap();
        tlab.zero();
        tlab.write_u32s(p.2, &[1, 2, 32, 0]).unwrap();
        let mut l = q.begin_list().unwrap();
        l.set_heaps(&heap, None).unwrap();
        l.set_root_arguments(&tlab).unwrap();
        l.dispatch(&p.0, [1, 1, 1], [32, 1, 1]).unwrap();
        l.end().unwrap();
        let v = q.submit(vec![l]).unwrap();
        // The source goes out of scope before the GPU is known to be done.
        drop(src);
        q.wait_for(v, 20_000).unwrap();
        let got = dst.read_u32s(0, 4).unwrap();
        assert_eq!(got[1], round + 1, "round {round}");
    }
}

/// A typed SRV off the texture-buffer alignment reads an aligned copy of its
/// range (`mirror`): the game's `R16_UINT` views start at odd element
/// numbers. The shader sees the right elements, `GetDimensions` the count,
/// a change made by the CPU between lists shows in the next list, and the
/// same view is one mirror.
#[test]
fn a_misaligned_typed_srv_reads_a_refreshed_copy() {
    let Some(dxil) = dxil_or_skip("typed_copy", "cs_6_6", COPY) else {
        return;
    };
    let (_g, d, q) = setup();
    let p = pipeline(&d, &dxil);
    let f16 = typed_format(dxgi::R16_UINT).unwrap();
    let f32f = r32(&d);
    for (first, n) in [(1u64, 7u64), (3, 20), (83, 9), (8, 8), (4093, 3)] {
        let src = d.create_buffer(BufferKind::Shared, 8192).unwrap();
        let dst = d.create_buffer(BufferKind::Shared, 4096).unwrap();
        let halves: Vec<u8> = (0..4096u32)
            .flat_map(|i| ((i % 60000 + 1) as u16).to_le_bytes())
            .collect();
        src.write(0, &halves).unwrap();
        let heap = DescriptorHeap::new(&d, 8).unwrap();
        heap.write_typed_buffer(1, &src, &f16, first, n, ViewAccess::Read)
            .unwrap_or_else(|e| panic!("first {first}: {e}"));
        heap.write_typed_buffer(2, &dst, &f32f, 0, n, ViewAccess::ReadWrite)
            .unwrap();
        let again = src.mirror_count();
        heap.write_typed_buffer(3, &src, &f16, first, n, ViewAccess::Read)
            .unwrap();
        assert_eq!(src.mirror_count(), again, "the same view is one mirror");
        for round in 0..2u32 {
            if round == 1 {
                // The CPU changes the source between lists.
                let bytes: Vec<u8> = (0..4096u32)
                    .flat_map(|i| ((i % 5000 + 7) as u16).to_le_bytes())
                    .collect();
                src.write(0, &bytes).unwrap();
            }
            dst.write_u32s(0, &[0xdead_beef; 32]).unwrap();
            dispatch(&q, &p, &heap, [1, 2, n as u32, 0], 32);
            let got = dst.read_u32s(0, 32).unwrap();
            let (m, add) = if round == 0 { (60000, 1) } else { (5000, 7) };
            let mut want = [0xdead_beefu32; 32];
            for i in 0..n.min(4096 - first) {
                want[i as usize] = ((first + i) as u32 % m) + add;
            }
            want[0] += 1_000_000 * n as u32 + 1000 * n as u32;
            assert_eq!(
                &got[..n as usize],
                &want[..n as usize],
                "first {first} n {n} round {round}"
            );
            assert_eq!(&got[n as usize..], &want[n as usize..], "nothing past n");
        }
    }
}

/// Mirrors are counted and given back with their source buffer.
#[test]
fn mirrors_are_released_with_their_buffer() {
    let (_g, d, _q) = setup();
    let f16 = typed_format(dxgi::R16_UINT).unwrap();
    let heap = DescriptorHeap::new(&d, 8).unwrap();
    let before = d.mirror_totals();
    {
        let src = d.create_buffer(BufferKind::Shared, 4096).unwrap();
        heap.write_typed_buffer(0, &src, &f16, 1, 100, ViewAccess::Read)
            .unwrap();
        heap.write_typed_buffer(1, &src, &f16, 3, 100, ViewAccess::Read)
            .unwrap();
        assert_eq!(d.mirror_totals(), (before.0 + 2, before.1 + 400));
    }
    assert_eq!(d.mirror_totals(), before);
}
