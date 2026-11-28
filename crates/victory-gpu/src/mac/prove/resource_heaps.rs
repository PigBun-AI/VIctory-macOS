//! C-V-014 (docs/mac/PATCHES.md G12): committed and placed resources and heaps, with the console's memory model
//! (one memory: the GPU address of a resource is the address `Map` hands out), against the D3D12 rules.
//!
//! Reference: D3D12 functional spec and API documentation: `CreateHeap` (size, alignment 64 KiB or 4 MiB for
//! multisampled), `CreatePlacedResource` (the offset a multiple of the resource's alignment and the resource inside the
//! heap; resources placed at overlapping offsets alias the same memory), `CreateCommittedResource` (memory zeroed,
//! `GetResourceAllocationInfo` size), `ID3D12Resource::Map` (pointer to the subresource's data; a null data pointer is
//! allowed and only asks for residency; `Map` may be called again and gives the same pointer while mapped),
//! `GetCustomHeapProperties` (UMA values), `GetGPUVirtualAddress` non-zero and stable.
use super::*;

const HEAP: HeapProperties = HeapProperties { kind: 1, cpu_pages: 0, pool: 0, creation_nodes: 0, visible_nodes: 0 };

fn buffer_desc(width: u64) -> ResourceDesc {
    ResourceDesc {
        dimension: BUFFER,
        alignment: 0,
        width,
        height: 1,
        depth_or_array_size: 1,
        mip_levels: 1,
        format: 0,
        sample_count: 1,
        sample_quality: 0,
        layout: 1,
        flags: 0,
    }
}

fn committed(desc: &ResourceDesc) -> This {
    let mut out = std::ptr::null();
    assert_eq!(create_committed(std::ptr::null(), &HEAP, 0, desc, 0, std::ptr::null(), &RESOURCE_IIDS[0], &mut out), S_OK);
    out
}

fn heap(size: u64, alignment: u64) -> This {
    let desc = HeapDesc { size, properties: HEAP, alignment, flags: 0 };
    let mut heap = std::ptr::null();
    assert_eq!(create_heap(std::ptr::null(), &desc, &HEAP_IIDS[0], &mut heap), S_OK);
    heap
}

fn placed(heap: This, offset: u64, desc: &ResourceDesc) -> Result<This, HRESULT> {
    let mut out = std::ptr::null();
    match create_placed(std::ptr::null(), heap, offset, desc, 0, std::ptr::null(), &RESOURCE_IIDS[0], &mut out) {
        S_OK => Ok(out),
        hr => Err(hr),
    }
}

fn mapped(r: This) -> *mut u8 {
    let mut p = std::ptr::null_mut();
    assert_eq!(map(r, 0, std::ptr::null(), &mut p), S_OK);
    p
}

#[test]
fn a_committed_buffer_is_zeroed_aligned_and_its_gpu_address_is_the_address_map_gives() {
    for width in [1u64, 100, 65_536, 70_000, 1 << 20] {
        let r = committed(&buffer_desc(width));
        let at = mapped(r);
        assert_eq!(gpu_address(r), at as u64, "width {width}: GPU address = mapped address");
        assert_eq!(at as u64 % KIB_64, 0, "width {width}: 64 KiB aligned");
        // SAFETY: the buffer's `width` bytes.
        let bytes = unsafe { std::slice::from_raw_parts(at, width as usize) };
        assert!(bytes.iter().all(|&b| b == 0), "width {width}: zeroed");
        // what is written through the mapping is what is read at the GPU address, and a second Map is the same pointer
        // SAFETY: inside the buffer.
        unsafe { at.add(width as usize - 1).write(0xab) };
        assert_eq!(mapped(r), at);
        // SAFETY: the GPU address is the CPU address (console memory model).
        assert_eq!(unsafe { (gpu_address(r) as *const u8).add(width as usize - 1).read() }, 0xab);
        assert_eq!(map(r, 0, std::ptr::null(), std::ptr::null_mut()), S_OK, "a null data pointer only asks for residency");
        unmap(r, 0, std::ptr::null());
        assert_eq!(gpu_address(r), at as u64, "stable after Unmap");
        let mut back = std::mem::MaybeUninit::<ResourceDesc>::uninit();
        resource_desc(r, back.as_mut_ptr());
        // SAFETY: written by the call.
        assert_eq!(unsafe { back.assume_init() }.width, width, "GetDesc");
        assert_eq!(resource_release(r), 0);
    }
}

#[test]
fn two_committed_resources_do_not_overlap() {
    let (a, b) = (committed(&buffer_desc(70_000)), committed(&buffer_desc(70_000)));
    let (pa, pb) = (gpu_address(a), gpu_address(b));
    assert!(pa + 70_000 <= pb || pb + 70_000 <= pa);
    resource_release(a);
    resource_release(b);
}

#[test]
fn placed_resources_sit_in_their_heap_and_overlapping_ones_alias() {
    let h = heap(1 << 20, 0);
    // SAFETY: a heap we just made.
    let base = unsafe { (*h.cast::<Heap>()).memory.ptr.as_ptr() } as u64;
    let a = placed(h, 0, &buffer_desc(65_536)).expect("at 0");
    let b = placed(h, 128 << 10, &buffer_desc(10_000)).expect("at 128 KiB");
    assert_eq!((gpu_address(a), gpu_address(b)), (base, base + (128 << 10)));
    assert_eq!(mapped(a) as u64, base);
    // aliasing: two resources at the same offset see the same bytes
    let c = placed(h, 0, &buffer_desc(1_000)).expect("also at 0");
    // SAFETY: inside both buffers.
    unsafe { mapped(a).add(500).write(0x5a) };
    // SAFETY: as above.
    assert_eq!(unsafe { mapped(c).add(500).read() }, 0x5a);
    // the end of the heap is allowed, past it is not; an offset that is not a multiple of the alignment is not
    assert!(placed(h, (1 << 20) - 65_536, &buffer_desc(65_536)).is_ok());
    assert_eq!(placed(h, (1 << 20) - 65_536, &buffer_desc(65_537)).err(), Some(E_INVALIDARG));
    assert_eq!(placed(h, 1 << 20, &buffer_desc(1)).err(), Some(E_INVALIDARG));
    assert_eq!(placed(h, 4096, &buffer_desc(100)).err(), Some(E_INVALIDARG));
    assert_eq!(placed(h, u64::MAX - 65_535, &buffer_desc(100)).err(), Some(E_INVALIDARG), "no overflow");
    assert_eq!(placed(std::ptr::null(), 0, &buffer_desc(100)).err(), Some(E_INVALIDARG));
    for r in [a, b, c] {
        resource_release(r);
    }
    heap_release(h);
}

#[test]
fn placed_textures_follow_the_alignment_allocation_info_gives() {
    let h = heap(8 << 20, 0);
    // The console's alignment is its layout's, not PC's 64 KiB (docs/mac/PATCHES.md; the title keeps these sizes in its
    // texture tables): linear 256 bytes, the 4 KiB modes 4 KiB, the 64 KiB modes 64 KiB.
    let mut d = ResourceDesc { dimension: 3, format: 28, width: 256, height: 256, mip_levels: 1, layout: 1, ..buffer_desc(0) };
    for (layout, flags, alignment) in [(1u32, 0u32, 256u64), (0, 0, 4096), (0, 1, 65_536), (0, 2, 65_536)] {
        d.layout = layout;
        d.flags = flags;
        let info = footprint(&d).unwrap();
        assert_eq!((info.alignment, info.size % info.alignment), (alignment, 0), "layout {layout} flags {flags}");
        assert!(placed(h, info.alignment, &d).is_ok());
        assert_eq!(placed(h, info.alignment / 2, &d).err(), Some(E_INVALIDARG), "not a multiple of the alignment");
    }
    d.layout = 1;
    d.flags = 0;
    // multisampled: 4 MiB
    d.sample_count = 4;
    let ms = footprint(&d).unwrap();
    assert_eq!(ms.alignment, 4 << 20);
    assert!(placed(h, 0, &d).is_ok());
    assert!(placed(h, 4 << 20, &d).is_ok());
    assert_eq!(placed(h, 65_536, &d).err(), Some(E_INVALIDARG));
    heap_release(h);
}

/// On the console (and on PC with a unified memory architecture) every heap type is in one pool
/// (`GetCustomHeapProperties`, "UMA": default not CPU-visible, upload and readback write-back).
#[test]
fn custom_heap_properties_are_the_uma_ones() {
    type Custom = extern "system" fn(*mut HeapProperties, This, u32, u32) -> *mut HeapProperties;
    let device = crate::device::device();
    let get = crate::object::slot::<Custom>(device, 0xd0);
    for (kind, cpu_pages, pool) in [(1u32, 1u32, 1u32), (2, 3, 1), (3, 3, 1)] {
        let mut p = HEAP;
        get(&mut p, device, 1, kind);
        assert_eq!((p.kind, p.cpu_pages, p.pool, p.creation_nodes, p.visible_nodes), (4, cpu_pages, pool, 1, 1), "heap type {kind}");
    }
}
