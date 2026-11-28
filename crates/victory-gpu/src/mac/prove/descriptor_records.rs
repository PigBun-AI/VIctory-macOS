//! C-V-012, the title's side: what the descriptor methods do to the title's descriptor memory
//! (docs/mac/translation-matrix-mac.csv.txt; docs/mac/PATCHES.md G12). A child module of `descriptor`
//! (`#[path]`), so it sees the private record functions. The host half (record to host view, read by a shader)
//! is `descriptor/host.rs`'s child `mac/prove/descriptor_views.rs`.
//!
//! Reference: the D3D12 functional specification, "Descriptor heaps / CopyDescriptors": the sources are the
//! concatenation of their ranges, the destinations likewise, descriptor `i` of one goes to descriptor `i` of
//! the other, with a null size array meaning every range holds one descriptor
//! (https://microsoft.github.io/DirectX-Specs/d3d/ResourceBinding.html, and the `ID3D12Device::CopyDescriptors` /
//! `CopyDescriptorsSimple` documentation). What a "descriptor" holds is the console's: 32 bytes for a view, 16 for
//! a sampler (`increment`), of which the title's own writes are records of 12 bytes.
use super::*;

/// A small deterministic generator for the cases.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }
}

/// `count` slots of `step` bytes of title memory, each holding a different constant buffer view record
/// (inline records: no table entry), with the rest of each slot a marker.
fn heap_of_views(count: usize, step: usize, base: u64) -> Vec<u8> {
    let mut bytes = vec![0xA5u8; count * step];
    for i in 0..count {
        let at = bytes.as_mut_ptr() as u64 + (i * step) as u64;
        create_cbv(std::ptr::null(), &[(base + i as u64 * 0x100) as u32, 0x7, 256 + 256 * (i as u32 % 7), 0], at);
    }
    bytes
}

/// The spec's CopyDescriptors on bytes: sources concatenated, destinations concatenated, slot by slot.
fn model(
    heap: &mut [u8],
    step: usize,
    dst: &[(usize, usize)],
    src: &[(usize, usize)],
) {
    let from: Vec<usize> = src.iter().flat_map(|&(s, n)| (0..n).map(move |i| s + i)).collect();
    let to: Vec<usize> = dst.iter().flat_map(|&(s, n)| (0..n).map(move |i| s + i)).collect();
    assert_eq!(from.len(), to.len());
    let copies: Vec<Vec<u8>> = from.iter().map(|&f| heap[f * step..(f + 1) * step].to_vec()).collect();
    for (t, bytes) in to.iter().zip(copies) {
        heap[t * step..(t + 1) * step].copy_from_slice(&bytes);
    }
}

/// Splits `total` slots into `parts` ranges (sizes at least 1) starting at `first`, at random places of a heap
/// of `slots`, not overlapping each other.
fn ranges(rng: &mut Lcg, total: usize, parts: usize, lane: usize, lanes: usize, slots: usize) -> Vec<(usize, usize)> {
    let mut sizes = vec![1usize; parts];
    for _ in 0..total - parts {
        let k = rng.next(parts as u64) as usize;
        sizes[k] += 1;
    }
    // this side's lane of the heap: [lane*slots/lanes, (lane+1)*slots/lanes)
    let (lo, hi) = (lane * slots / lanes, (lane + 1) * slots / lanes);
    let mut cursor = lo;
    let room = hi - lo;
    let used: usize = sizes.iter().sum();
    assert!(used <= room);
    let mut spare = room - used;
    sizes
        .into_iter()
        .map(|n| {
            let gap = if spare > 0 { rng.next(spare as u64 + 1) as usize } else { 0 };
            spare -= gap;
            cursor += gap;
            let r = (cursor, n);
            cursor += n;
            r
        })
        .collect()
}

#[test]
fn copy_descriptors_is_the_specs_gather_and_scatter() {
    let mut rng = Lcg(0x5eed_d3d1_2c0f);
    const SLOTS: usize = 96;
    for kind in [0u32, 1] {
        let step = increment(kind).unwrap() as usize;
        for round in 0..200 {
            let heap = heap_of_views(SLOTS, step, 0x10_0000 * (round + 1));
            let mut want = heap.clone();
            // distinct halves for sources and destinations: the spec leaves overlapping copies undefined
            let total = 1 + rng.next(24) as usize;
            let src_parts = 1 + rng.next(total.min(5) as u64) as usize;
            let dst_parts = 1 + rng.next(total.min(5) as u64) as usize;
            let src = ranges(&mut rng, total, src_parts, 0, 2, SLOTS);
            let dst = ranges(&mut rng, total, dst_parts, 1, 2, SLOTS);
            model(&mut want, step, &dst, &src);
            let base = heap.as_ptr() as u64;
            let starts = |r: &[(usize, usize)]| r.iter().map(|&(s, _)| base + (s * step) as u64).collect::<Vec<_>>();
            let sizes = |r: &[(usize, usize)]| r.iter().map(|&(_, n)| n as u32).collect::<Vec<_>>();
            let (ds, dn, ss, sn) = (starts(&dst), sizes(&dst), starts(&src), sizes(&src));
            copy(std::ptr::null(), ds.len() as u32, ds.as_ptr(), dn.as_ptr(), ss.len() as u32, ss.as_ptr(), sn.as_ptr(), kind);
            assert_eq!(heap, want, "kind {kind} round {round}: dst {dst:?} src {src:?}");
        }
    }
}

#[test]
fn copy_descriptors_with_null_sizes_copies_one_each() {
    let step = 32usize;
    let heap = heap_of_views(16, step, 0x40_0000);
    let mut want = heap.clone();
    let base = heap.as_ptr() as u64;
    let at = |i: usize| base + (i * step) as u64;
    let (dst, src) = ([at(9), at(12), at(8)], [at(1), at(0), at(5)]);
    model(&mut want, step, &[(9, 1), (12, 1), (8, 1)], &[(1, 1), (0, 1), (5, 1)]);
    copy(std::ptr::null(), 3, dst.as_ptr(), std::ptr::null(), 3, src.as_ptr(), std::ptr::null(), 0);
    assert_eq!(heap, want);
}

#[test]
fn copy_descriptors_simple_is_the_specs_contiguous_copy() {
    let mut rng = Lcg(77);
    for kind in [0u32, 1] {
        let step = increment(kind).unwrap() as usize;
        for round in 0..100 {
            let heap = heap_of_views(64, step, 0x80_0000 * (round + 1));
            let mut want = heap.clone();
            let n = 1 + rng.next(30) as usize;
            let (from, to) = (rng.next(32 - n.min(31) as u64 + 1) as usize % (32 - n.min(31)).max(1), 32 + rng.next(32 - n.min(31) as u64 + 1) as usize % (32 - n.min(31)).max(1));
            model(&mut want, step, &[(to, n)], &[(from, n)]);
            let base = heap.as_ptr() as u64;
            copy_simple(std::ptr::null(), n as u32, base + (to * step) as u64, base + (from * step) as u64, kind);
            assert_eq!(heap, want, "kind {kind} n {n} {from}->{to}");
        }
    }
    // a kind the device has no descriptors of copies nothing
    let mut heap = heap_of_views(4, 32, 0x1000);
    let before = heap.clone();
    let base = heap.as_mut_ptr() as u64;
    copy_simple(std::ptr::null(), 2, base + 64, base, 9);
    assert_eq!(heap, before);
    // null handles are ignored
    copy_simple(std::ptr::null(), 2, 0, base, 0);
    copy_simple(std::ptr::null(), 2, base, 0, 0);
    assert_eq!(heap, before);
}

/// Every shape of view description the title writes comes back whole from its record: the dimension and the
/// words after it are the title's own, nothing of them is changed by the record (the host reads them later).
/// SRV dimensions 1-12 of `D3D12_SRV_DIMENSION` (BUFFER 1, TEXTURE1D 2, 1DARRAY 3, 2D 4, 2DARRAY 5, 2DMS 6,
/// 2DMSARRAY 7, 3D 8, CUBE 9, CUBEARRAY 10, RAYTRACING_ACCELERATION_STRUCTURE 11), UAV dimensions 1-8.
#[test]
fn every_view_dimension_is_recorded_whole() {
    let null = std::ptr::null();
    let mut slots = vec![0xC3u8; 64 * 32];
    let base = slots.as_mut_ptr() as u64;
    let mut n = 0u64;
    for dimension in 1..=11u32 {
        for format in [0u32, 2, 10, 28, 41, 45, 71, 98, 115, 118] {
            let words: [u32; 10] =
                [format, dimension, 5768, 0x1000 + dimension, 3, 0x4d, 0, 0xffff_ffff, 0x3f80_0000, 7 + format];
            let at = base + 32 * (n % 64);
            n += 1;
            let resource = 0x7a00_0000usize + n as usize * 16;
            create_srv(null, resource as This, &words, at);
            assert_eq!(view(at), Some(View::Shader(resource, Some(words))), "SRV dimension {dimension} format {format}");
            // the record is 12 bytes; the rest of the descriptor is not touched
            // SAFETY: the slot is in `slots`.
            let tail = unsafe { std::slice::from_raw_parts((at + 12) as *const u8, 20) };
            assert!(tail.iter().all(|&b| b == 0xC3), "the record is 12 bytes");
            forget(resource);
        }
    }
    for dimension in 1..=8u32 {
        for format in [0u32, 2, 10, 28, 41, 54, 61] {
            let words: [u32; 10] = [format, dimension, 0, 0, 0x100, 0x10, 4, 0, 0, 0];
            let at = base + 32 * (n % 64);
            n += 1;
            let (resource, counter) = (0x7b00_0000usize + n as usize * 16, if n % 2 == 0 { 0x7c00_0000usize + n as usize * 16 } else { 0 });
            create_uav(null, resource as This, counter as This, &words, at);
            assert_eq!(view(at), Some(View::Unordered(resource, counter, Some(words))), "UAV dimension {dimension} format {format}");
            forget(resource);
            forget(counter);
        }
    }
    // a view with no description is a view of the resource as it is (a null description is allowed by the spec)
    create_srv(null, 0x7d00_0010usize as This, std::ptr::null(), base);
    assert_eq!(view(base), Some(View::Shader(0x7d00_0010, None)));
    create_uav(null, 0x7d00_0020usize as This, std::ptr::null(), std::ptr::null(), base);
    assert_eq!(view(base), Some(View::Unordered(0x7d00_0020, 0, None)));
    // a null handle is ignored, and so is a placed view with no description
    create_srv(null, 0x7d00_0030usize as This, std::ptr::null(), 0);
    let before = slots.clone();
    create_placed_srv(null, std::ptr::null(), std::ptr::null(), base);
    assert_eq!(slots, before);
}

/// Samplers: the plain 13-word description and the console's 27-word one come back whole.
#[test]
fn sampler_records_come_back_whole() {
    let null = std::ptr::null();
    let slots = vec![0u8; 8 * 16];
    let base = slots.as_ptr() as u64;
    for k in 0..40u32 {
        let words: [u32; 13] = std::array::from_fn(|i| k * 0x101 + i as u32 * 7);
        create_sampler(null, &words, base + 16 * u64::from(k % 8));
        assert_eq!(view(base + 16 * u64::from(k % 8)), Some(View::Sampler(words)));
        let wide: [u32; 27] = std::array::from_fn(|i| k * 0x303 + i as u32 * 11);
        create_sampler_x(null, &wide, base + 16 * u64::from((k + 3) % 8));
        assert_eq!(view(base + 16 * u64::from((k + 3) % 8)), Some(View::ConsoleSampler(wide)));
    }
}

/// Constant buffer views over the title's whole address space and the sizes the spec allows (a multiple of
/// 256 bytes, up to 64 KiB for a view; 0 is allowed). The record keeps the address and size exactly; the
/// 4th word the title passes (reserved) is not part of the view.
#[test]
fn constant_buffer_views_keep_address_and_size() {
    let null = std::ptr::null();
    let slots = [0u64; 4];
    let at = slots.as_ptr() as u64;
    let mut n = 0u64;
    for address in [0u64, 0x100, 0x2602_0000, 0xd_6b60_0000, 0x7ffd_a34f_0100, 0x1_0000_0000, 0xffff_ffff_ff00] {
        for size in [0u32, 256, 512, 4096, 65536, 0x1_0000 - 256, 1 << 20] {
            for reserved in [0u32, 0xdead_beef] {
                create_cbv(null, &[address as u32, (address >> 32) as u32, size, reserved], at);
                assert_eq!(view(at), Some(View::Constant(address, size)), "{address:#x} {size}");
                n += 1;
            }
        }
    }
    assert!(n > 90);
    // a null description writes nothing
    let before = slots;
    create_cbv(null, std::ptr::null(), at);
    assert_eq!(slots, before);
}

/// Descriptor increments: views 32 bytes, samplers 16, render targets and depth views 128 (the console's, which
/// the title's own descriptor arithmetic uses); a heap type the device does not have answers 0.
#[test]
fn descriptor_increments_by_heap_type() {
    let null = std::ptr::null();
    assert_eq!([0u32, 1, 2, 3].map(|k| increment_size(null, k)), [32, 16, 128, 128]);
    for kind in [4u32, 5, 9, 0x8000_0000, u32::MAX] {
        assert_eq!(increment_size(null, kind), 0, "kind {kind}");
    }
}
