//! The interpreter's sync calls: what each does, and that bad input is a
//! status (never a panic): bad handles, bad values, wrong state, limits.
#![cfg(target_vendor = "apple")]

mod exec_support;

use exec_support::*;
use victory_metal_back::exec::InProcess;
use victory_metal_proto::ids::*;
use victory_metal_proto::link::{Link, LinkExt};
use victory_metal_proto::status::Status;
use victory_metal_proto::sync::*;
use victory_metal_proto::types::*;
use victory_metal_proto::wire::Wire;
use victory_metal_proto::{Caps, PROTO_VERSION};

#[test]
fn nothing_but_nop_and_hello_works_before_hello() {
    let fx = Fx::unstarted();
    assert_eq!(fx.status(NopArgs::new()), Status::OK);
    assert_eq!(fx.status(CreateBufferArgs::new()), Status::NOT_INITIALIZED);
    assert_eq!(fx.status(QueryCapsArgs::new()), Status::NOT_INITIALIZED);
    assert_eq!(fx.status(CreateFenceArgs::new()), Status::NOT_INITIALIZED);
    assert_eq!(fx.status(FenceWaitArgs::new()), Status::NOT_INITIALIZED);
    assert_eq!(fx.status(DeviceStatusArgs::new()), Status::NOT_INITIALIZED);
    assert_eq!(fx.status(ShutdownArgs::new()), Status::NOT_INITIALIZED);
    assert_eq!(fx.status(DestroyQueueArgs::new()), Status::NOT_INITIALIZED);
    // A valid (empty) block, and a bad one: the bad one is bad whatever the state.
    assert_eq!(fx.submit(&[]), Status::NOT_INITIALIZED);
    assert_eq!(fx.link.submit(&[1, 2, 3]), Status::BAD_STREAM);
    assert!(fx.executor().device().is_none());
}

#[test]
fn hello_negotiates_and_is_idempotent() {
    let fx = Fx::unstarted();
    let mut h = HelloArgs::new();
    h.client_proto_version = PROTO_VERSION + 1;
    assert_eq!(fx.status(h), Status::BAD_VERSION);
    let mut h = HelloArgs::new();
    h.client_proto_version = PROTO_VERSION;
    h.required_caps = Caps(1 << 20);
    assert_eq!(fx.status(h), Status::UNSUPPORTED, "unknown capability bit");
    assert!(
        fx.executor().device().is_none(),
        "a refused HELLO makes nothing"
    );
    let mut h = HelloArgs::new();
    h.client_proto_version = PROTO_VERSION;
    h.required_caps = Caps::METAL4 | Caps::ARGUMENT_BUFFERS_TIER2 | Caps::WRAP_MEMORY;
    let h = fx.ok(h);
    assert_eq!(h.proto_version, PROTO_VERSION);
    assert!(h.caps.contains(Caps::METAL4));
    assert!(h.caps.contains(Caps::UNIFIED_MEMORY));
    assert_eq!(h.caps.0 & !Caps::KNOWN.0, 0);
    assert_eq!(
        h.caps.contains(Caps::TRANSLATED),
        cfg!(target_arch = "x86_64"),
        "TRANSLATED means Rosetta"
    );
    let name_len = h
        .device_name
        .iter()
        .position(|&b| b == 0)
        .expect("NUL-terminated");
    assert!(name_len > 0);
    // A second HELLO keeps the device: what was made is still there.
    let b = fx.buffer(BufferKind::SHARED, 64);
    let mut h2 = HelloArgs::new();
    h2.client_proto_version = PROTO_VERSION;
    let h2 = fx.ok(h2);
    assert_eq!(h2.caps, h.caps);
    let mut d = DestroyBufferArgs::new();
    d.res = b.res;
    assert_eq!(fx.status(d), Status::OK);
    // A second HELLO that asks for what the device lacks still fails.
    let mut h3 = HelloArgs::new();
    h3.client_proto_version = PROTO_VERSION;
    h3.required_caps = Caps(1 << 20);
    assert_eq!(fx.status(h3), Status::UNSUPPORTED);
}

#[test]
fn query_caps_reports_what_the_device_has() {
    let fx = Fx::new();
    let q = fx.ok(QueryCapsArgs::new());
    assert!(q.caps.contains(Caps::METAL4));
    assert_eq!(q.descriptor_stride, 24, "an IRDescriptorTableEntry");
    assert_eq!(q.sampler_stride, 24);
    assert_eq!(
        q.page_size,
        if cfg!(target_arch = "x86_64") {
            4096
        } else {
            16384
        }
    );
    assert_eq!(q.simd_width, 32);
    assert!(q.max_threads_per_group >= 1024);
    assert!(q.max_buffer_len >= 1 << 30);
    assert!(q.working_set > 0);
    assert_eq!(q.max_queues, 16);
    assert_eq!(fx.ok(DeviceStatusArgs::new()).state, 0);
}

#[test]
fn call_framing_errors_are_statuses() {
    let fx = Fx::new();
    // Wrong head version.
    let mut a = CreateBufferArgs::new();
    a.size = 64;
    a.kind = BufferKind::SHARED.0;
    a.head_mut().version = 9;
    let p = (&mut a as *mut CreateBufferArgs).cast::<u8>();
    assert_eq!(
        fx.link
            .call(SyncOp::CREATE_BUFFER, p, CreateBufferArgs::SIZE),
        Status::BAD_VERSION
    );
    // Wrong length, null block, misaligned block.
    a.head_mut().version = 1;
    assert_eq!(
        fx.link.call(SyncOp::CREATE_BUFFER, p, 8),
        Status::INVALID_ARG
    );
    assert_eq!(
        fx.link.call(
            SyncOp::CREATE_BUFFER,
            std::ptr::null_mut(),
            CreateBufferArgs::SIZE
        ),
        Status::INVALID_ARG
    );
    let mut raw = vec![0u64; CreateBufferArgs::SIZE / 8 + 1];
    let misaligned = unsafe { raw.as_mut_ptr().cast::<u8>().add(1) };
    assert_eq!(
        fx.link
            .call(SyncOp::CREATE_BUFFER, misaligned, CreateBufferArgs::SIZE),
        Status::INVALID_ARG
    );
    // Unknown, reserved and never-assigned operation numbers.
    let n = std::ptr::null_mut();
    assert_eq!(
        fx.link.call(SyncOp(24), n, 0),
        Status::UNKNOWN_OP,
        "first unassigned number"
    );
    assert_eq!(fx.link.call(SyncOp(0x3f), n, 0), Status::UNKNOWN_OP);
    assert_eq!(
        fx.link.call(SyncOp(0x1234), n, 0),
        Status::UNSUPPORTED,
        "reserved range"
    );
    assert_eq!(
        fx.link.call(SyncOp(SYNC_RESERVED_START), n, 0),
        Status::UNSUPPORTED
    );
    assert_eq!(
        fx.link.call(SyncOp(SYNC_RESERVED_END), n, 0),
        Status::UNSUPPORTED
    );
    assert_eq!(fx.link.call(SyncOp(0x8000), n, 0), Status::UNKNOWN_OP);
    assert_eq!(fx.link.call(SyncOp(u32::MAX), n, 0), Status::UNKNOWN_OP);
    // Placeholders: defined by proto, not done by this back yet.
    assert_eq!(fx.status(GetSurfaceLayerArgs::new()), Status::UNSUPPORTED);
}

#[test]
fn buffers_get_back_made_handles_that_are_never_reused() {
    let fx = Fx::new();
    let a = fx.buffer(BufferKind::UPLOAD, 100);
    assert_ne!(a.res.0, 0);
    assert_ne!(a.gpu_va, 0);
    assert!(a.alloc_size >= 100);
    assert_ne!(a.cpu_addr, 0, "an upload buffer has a CPU address");
    assert_eq!(
        fx.buffer(BufferKind::DEFAULT, 100).cpu_addr,
        0,
        "a GPU-only buffer has none"
    );
    let mut d = DestroyBufferArgs::new();
    d.res = a.res;
    assert_eq!(fx.status(d), Status::OK);
    assert_eq!(fx.status(d), Status::NOT_FOUND, "destroyed twice");
    let b = fx.buffer(BufferKind::SHARED, 100);
    assert!(
        b.res.0 > a.res.0,
        "a destroyed handle is not handed out again"
    );
    let mut d = DestroyBufferArgs::new();
    d.res = ResId(0);
    assert_eq!(fx.status(d), Status::NOT_FOUND, "0 is null");
    d.res = ResId(9999);
    assert_eq!(fx.status(d), Status::NOT_FOUND);
}

#[test]
fn bad_buffer_requests_are_invalid_arguments() {
    let fx = Fx::new();
    let make = |size: u64, kind: u32, flags: u32| {
        let mut a = CreateBufferArgs::new();
        a.size = size;
        a.kind = kind;
        a.flags = flags;
        fx.status(a)
    };
    assert_eq!(
        make(0, BufferKind::SHARED.0, 0),
        Status::INVALID_ARG,
        "size 0"
    );
    assert_eq!(
        make(64, BufferKind::WRAPPED.0, 0),
        Status::INVALID_ARG,
        "WRAPPED has WRAP_MEMORY"
    );
    assert_eq!(make(64, 77, 0), Status::INVALID_ARG, "unknown kind");
    assert_eq!(
        make(64, BufferKind::SHARED.0, 1),
        Status::INVALID_ARG,
        "flags are reserved"
    );
    let huge = make(u64::MAX / 2, BufferKind::SHARED.0, 0);
    assert!(
        huge == Status::INVALID_ARG || huge == Status::OUT_OF_MEMORY,
        "{huge:?}"
    );
}

/// Page-aligned anonymous memory for `WRAP_MEMORY`.
fn map_pages(pages: usize, page: usize) -> *mut u8 {
    // SAFETY: an anonymous private mapping; the result is checked.
    let p = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            pages * page,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_ANON | libc::MAP_PRIVATE,
            -1,
            0,
        )
    };
    assert_ne!(p, libc::MAP_FAILED);
    p.cast()
}

#[test]
fn wrap_memory_checks_the_range_before_the_gpu_sees_it() {
    let fx = Fx::new();
    let page = fx.ok(QueryCapsArgs::new()).page_size as usize;
    let wrap = |addr: u64, len: u64, flags: u32| {
        let mut a = WrapMemoryArgs::new();
        a.addr = addr;
        a.len = len;
        a.flags = flags;
        fx.call(a)
    };
    let mem = map_pages(2, page);
    let addr = mem as u64;
    assert_eq!(wrap(0, page as u64, 0).0, Status::INVALID_ARG, "null");
    assert_eq!(
        wrap(addr + 8, page as u64, 0).0,
        Status::INVALID_ARG,
        "not page aligned"
    );
    assert_eq!(
        wrap(addr, page as u64 + 8, 0).0,
        Status::INVALID_ARG,
        "not whole pages"
    );
    assert_eq!(wrap(addr, 0, 0).0, Status::INVALID_ARG, "empty");
    assert_eq!(wrap(addr, page as u64, 1).0, Status::INVALID_ARG, "flags");
    assert_eq!(
        wrap(addr, u64::MAX - page as u64 + 1, 0).0,
        Status::INVALID_ARG,
        "wraps around"
    );
    // A range with a hole in it (the second page unmapped) is not memory.
    // SAFETY: unmaps the second page of the mapping made above.
    unsafe { libc::munmap(mem.add(page).cast(), page) };
    assert_eq!(
        wrap(addr, 2 * page as u64, 0).0,
        Status::INVALID_ARG,
        "unmapped page"
    );
    // The mapped page is fine; the GPU address is returned.
    let (s, w) = wrap(addr, page as u64, 0);
    assert_eq!(s, Status::OK);
    assert_ne!(w.gpu_va, 0);
    // Nothing else may put the same page behind the GPU.
    assert_eq!(wrap(addr, page as u64, 0).0, Status::CONFLICT);
    let mut d = DestroyBufferArgs::new();
    d.res = w.res;
    assert_eq!(fx.status(d), Status::OK);
    // SAFETY: the wrapped buffer is destroyed; unmaps the first page.
    unsafe { libc::munmap(mem.cast(), page) };
}

#[test]
fn descriptor_heaps_and_their_limits() {
    let fx = Fx::new();
    let make = |kind: u32, capacity: u32, flags: u32| {
        let mut a = CreateDescHeapArgs::new();
        a.kind = kind;
        a.capacity = capacity;
        a.flags = flags;
        fx.call(a)
    };
    let (s, h) = make(HeapKind::CBV_SRV_UAV.0, 16, 1);
    assert_eq!(s, Status::OK);
    assert_eq!(h.stride, 24);
    assert_ne!(h.gpu_va, 0);
    assert_ne!(h.cpu_addr, 0);
    assert_eq!(make(HeapKind::SAMPLER.0, 4, 1).0, Status::OK);
    assert_eq!(make(HeapKind::RTV.0, 4, 0).0, Status::UNSUPPORTED, "M3");
    assert_eq!(make(HeapKind::DSV.0, 4, 0).0, Status::UNSUPPORTED, "M3");
    assert_eq!(make(9, 4, 0).0, Status::INVALID_ARG);
    assert_eq!(make(0, 0, 0).0, Status::INVALID_ARG, "no slots");
    assert_eq!(make(0, 4, 2).0, Status::INVALID_ARG, "unknown flag");
    assert_eq!(
        make(0, (1 << 20) + 1, 0).0,
        Status::INVALID_ARG,
        "too many slots"
    );
    let mut d = DestroyDescHeapArgs::new();
    d.heap = h.heap;
    assert_eq!(fx.status(d), Status::OK);
    assert_eq!(fx.status(d), Status::NOT_FOUND);
}

/// The three words of a descriptor entry in a heap, read through its CPU address.
fn entry(heap: &CreateDescHeapArgs, index: u64) -> [u64; 3] {
    let w = cpu_read_u64s(heap.cpu_addr + 24 * index, 3);
    [w[0], w[1], w[2]]
}

const SIZE_MASK: u64 = 0xffff_ffff;

#[test]
fn write_descriptor_encodes_views_and_refuses_what_it_cannot() {
    let fx = Fx::new();
    let heap = fx.heap(HeapKind::CBV_SRV_UAV, 8, true);
    let smp = fx.heap(HeapKind::SAMPLER, 4, true);
    let b = fx.buffer(BufferKind::SHARED, 4096);
    let write = |h: HeapId, i: u32, v: ViewDesc| fx.write_view_status(h, i, v);
    let srv = |first, num| ViewDesc::raw(ViewKind::BUFFER_SRV, b.res, first, num);

    // What the shader will read: the address and the length in bytes.
    assert_eq!(write(heap.heap, 1, srv(2, 10)), Status::OK);
    assert_eq!(entry(&heap, 1)[0], b.gpu_va + 8);
    assert_eq!(entry(&heap, 1)[2] & SIZE_MASK, 40);
    let st = ViewDesc::structured(ViewKind::BUFFER_UAV, b.res, 16, 3, 5);
    assert_eq!(write(heap.heap, 2, st), Status::OK);
    assert_eq!(entry(&heap, 2)[0], b.gpu_va + 48);
    assert_eq!(entry(&heap, 2)[2] & SIZE_MASK, 80);
    assert_eq!(
        write(heap.heap, 3, ViewDesc::cbv(b.gpu_va + 256, 512)),
        Status::OK
    );
    assert_eq!(entry(&heap, 3)[0], b.gpu_va + 256);
    assert_eq!(entry(&heap, 3)[2] & SIZE_MASK, 512);
    assert_eq!(write(heap.heap, 3, ViewDesc::null()), Status::OK);
    assert_eq!(entry(&heap, 3), [0, 0, 0], "a null view clears the slot");
    // The whole buffer is the last word to fit.
    assert_eq!(write(heap.heap, 4, srv(0, 1024)), Status::OK);
    assert_eq!(
        write(heap.heap, 4, srv(1, 1024)),
        Status::INVALID_ARG,
        "one word too far"
    );
    assert_eq!(
        write(heap.heap, 4, srv(1 << 62, 4)),
        Status::INVALID_ARG,
        "overflow"
    );

    // Bad handles and indices.
    assert_eq!(write(HeapId(99), 0, srv(0, 1)), Status::NOT_FOUND);
    assert_eq!(write(HeapId(0), 0, srv(0, 1)), Status::NOT_FOUND);
    assert_eq!(
        write(heap.heap, 8, srv(0, 1)),
        Status::INVALID_ARG,
        "past the heap"
    );
    let dead = ViewDesc::raw(ViewKind::BUFFER_SRV, ResId(999), 0, 1);
    assert_eq!(write(heap.heap, 0, dead), Status::NOT_FOUND);
    // Bad descriptions.
    let mut bad = srv(0, 1);
    bad.kind = ViewKind(77);
    assert_eq!(write(heap.heap, 0, bad), Status::INVALID_ARG);
    let mut bad = srv(0, 1);
    bad.flags = 0x8000;
    assert_eq!(write(heap.heap, 0, bad), Status::INVALID_ARG);
    let zero_stride = ViewDesc::structured(ViewKind::BUFFER_SRV, b.res, 0, 0, 1);
    assert_eq!(write(heap.heap, 0, zero_stride), Status::INVALID_ARG);
    // A constant buffer view must lie in a live buffer.
    assert_eq!(
        write(heap.heap, 0, ViewDesc::cbv(0x10, 64)),
        Status::INVALID_ARG
    );
    assert_eq!(
        write(heap.heap, 0, ViewDesc::cbv(b.gpu_va + 4000, 512)),
        Status::INVALID_ARG
    );
    // A sampler heap holds samplers.
    assert_eq!(write(smp.heap, 0, srv(0, 1)), Status::INVALID_ARG);
    assert_eq!(
        write(smp.heap, 0, ViewDesc::cbv(b.gpu_va, 64)),
        Status::INVALID_ARG
    );
    assert_eq!(write(smp.heap, 0, ViewDesc::null()), Status::OK);
    // Not done yet: typed views, buffer counters, sampler states.
    let typed = ViewDesc::typed(ViewKind::BUFFER_SRV, b.res, format::R32_UINT, 0, 4);
    assert_eq!(write(heap.heap, 0, typed), Status::UNSUPPORTED);
    let mut counter = srv(0, 4);
    counter.flags = ViewDesc::HAS_COUNTER;
    assert_eq!(write(heap.heap, 0, counter), Status::UNSUPPORTED);
    let mut sampler = ViewDesc::null();
    sampler.kind = ViewKind::SAMPLER;
    assert_eq!(write(smp.heap, 0, sampler), Status::UNSUPPORTED);
    // A failed write leaves the slot as it was.
    assert_eq!(entry(&heap, 4)[0], b.gpu_va);
}

use victory_metal_proto::types::format;

#[test]
fn copy_descriptors_moves_whole_entries_like_memmove() {
    let fx = Fx::new();
    let a = fx.heap(HeapKind::CBV_SRV_UAV, 8, true);
    let staging = fx.heap(HeapKind::CBV_SRV_UAV, 8, false);
    let smp = fx.heap(HeapKind::SAMPLER, 8, true);
    let smp2 = fx.heap(HeapKind::SAMPLER, 8, false);
    let b = fx.buffer(BufferKind::SHARED, 4096);
    for i in 0..4u32 {
        fx.write_view(
            a.heap,
            i,
            ViewDesc::cbv(b.gpu_va + 256 * u64::from(i), 16 * (i + 1)),
        );
    }
    let want: Vec<[u64; 3]> = (0..4).map(|i| entry(&a, i)).collect();
    // Overlapping inside one heap: slots 0..4 move to 2..6.
    fx.copy_descriptors(a.heap, 2, a.heap, 0, 4);
    for i in 0..4 {
        assert_eq!(entry(&a, 2 + i), want[i as usize], "slot {}", 2 + i);
    }
    // And back the other way: 2..6 to 1..5.
    fx.copy_descriptors(a.heap, 1, a.heap, 2, 4);
    for i in 0..4 {
        assert_eq!(entry(&a, 1 + i), want[i as usize]);
    }
    // From a staging heap.
    fx.copy_descriptors(staging.heap, 0, a.heap, 1, 2);
    assert_eq!(entry(&staging, 1), want[1]);
    // Nothing to copy is fine; refusals leave things alone.
    fx.copy_descriptors(a.heap, 7, a.heap, 0, 0);
    assert_eq!(
        fx.copy_descriptors_status(a.heap, 6, a.heap, 0, 3),
        Status::INVALID_ARG
    );
    assert_eq!(
        fx.copy_descriptors_status(a.heap, 0, a.heap, 6, 3),
        Status::INVALID_ARG
    );
    assert_eq!(
        fx.copy_descriptors_status(a.heap, u32::MAX, a.heap, 0, 2),
        Status::INVALID_ARG
    );
    assert_eq!(
        fx.copy_descriptors_status(a.heap, 0, smp.heap, 0, 1),
        Status::INVALID_ARG,
        "kinds differ"
    );
    assert_eq!(
        fx.copy_descriptors_status(HeapId(99), 0, a.heap, 0, 1),
        Status::NOT_FOUND
    );
    assert_eq!(
        fx.copy_descriptors_status(a.heap, 0, HeapId(99), 0, 1),
        Status::NOT_FOUND
    );
    assert_eq!(
        fx.copy_descriptors_status(smp.heap, 0, smp2.heap, 0, 2),
        Status::OK
    );
}

#[test]
fn root_signatures_are_checked_and_laid_out() {
    let fx = Fx::new();
    // Not a root signature at all.
    assert_eq!(fx.root_sig_status(&[0xFF; 16]).0, Status::INVALID_ARG);
    assert_eq!(fx.root_sig_status(&[]).0, Status::INVALID_ARG);
    let mut a = CreateRootSignatureArgs::new();
    assert_eq!(fx.status(a), Status::INVALID_ARG, "no bytes");
    a.data_addr = 8;
    a.data_len = 1 << 30;
    assert_eq!(fx.status(a), Status::INVALID_ARG, "too long");
    // A table that mixes samplers and resources has no argument layout.
    let mixed = victory_metal_proto::root::RootSigDesc {
        params: vec![table_param(0, 2)],
        ranges: vec![range(RangeKind::SAMPLER), range(RangeKind::SRV)],
        ..Default::default()
    };
    assert_eq!(
        fx.root_sig_status(&mixed.encode().unwrap()).0,
        Status::INVALID_ARG
    );
    // An empty one is fine: no argument buffer.
    let empty = fx.root_sig(&Default::default());
    assert_eq!(fx.executor().tlab_layout(empty).unwrap().size(), 0);
    // MSC reflection: 3 constants take 12 bytes, the UAV address starts at 16, and a
    // static sampler adds one pointer (to its sampler table) after the parameters.
    let sig = victory_metal_proto::root::RootSigDesc {
        params: vec![constants_param(3), descriptor_param(RootParamKind::UAV)],
        static_samplers: vec![Default::default()],
        ..Default::default()
    };
    let id = fx.root_sig(&sig);
    let l = fx.executor().tlab_layout(id).unwrap();
    assert_eq!(
        (
            l.slot(0).unwrap().offset,
            l.slot(1).unwrap().offset,
            l.size()
        ),
        (0, 16, 32)
    );
    assert_eq!(
        l.static_table_offset(),
        Some(24),
        "MSC adds a pointer to the static-sampler table after the parameters"
    );
    let mut d = DestroyRootSignatureArgs::new();
    d.root_sig = id;
    assert_eq!(fx.status(d), Status::OK);
    assert_eq!(fx.status(d), Status::NOT_FOUND);
    assert!(fx.executor().tlab_layout(id).is_err());
}

#[test]
fn pipelines_from_dxil_check_their_arguments_and_refuse_garbage() {
    let fx = Fx::new();
    let sig = fx.root_sig(&victory_metal_proto::root::RootSigDesc {
        params: vec![constants_param(1)],
        ..Default::default()
    });
    let garbage = b"this is not DXIL at all, only bytes the call can read".to_vec();
    let make_at = |sig: RootSigId, flags: u32, addr: u64, len: u64| {
        let mut a = CreateComputePipelineArgs::new();
        a.root_sig = sig;
        a.flags = flags;
        a.dxil_addr = addr;
        a.dxil_len = len;
        fx.status(a)
    };
    let make = |sig: RootSigId, flags: u32| {
        make_at(sig, flags, garbage.as_ptr() as u64, garbage.len() as u64)
    };
    // Bytes MSC cannot read are a refusal (or, without MSC, UNSUPPORTED),
    // never OK and never a crash.
    let refused = make(sig, 0);
    assert!(
        refused == Status::UNSUPPORTED || refused == Status::INVALID_ARG,
        "{refused:?}"
    );
    assert!(fx.executor().last_error().is_some());
    assert_eq!(make(sig, 1), Status::INVALID_ARG);
    assert_eq!(make(RootSigId(99), 0), Status::NOT_FOUND);
    assert_eq!(make_at(sig, 0, 0, 4), Status::INVALID_ARG, "no address");
    assert_eq!(make_at(sig, 0, 8, 4), Status::INVALID_ARG, "unmapped address");
    assert_eq!(make_at(sig, 0, garbage.as_ptr() as u64, 0), Status::INVALID_ARG);
    let mut d = DestroyPipelineArgs::new();
    d.pso = PsoId(5);
    assert_eq!(fx.status(d), Status::NOT_FOUND);
    // The test hook.
    let ex = fx.executor();
    let src = "#include <metal_stdlib>\nusing namespace metal;\nkernel void k(device uint* o [[buffer(0)]]) { o[0] = 1; }";
    let ok = ex
        .create_compute_pipeline_from_msl(sig, src, "k", [1, 1, 1])
        .unwrap();
    assert_eq!(
        ex.create_compute_pipeline_from_msl(RootSigId(99), src, "k", [1, 1, 1]),
        Err(Status::NOT_FOUND)
    );
    assert_eq!(
        ex.create_compute_pipeline_from_msl(sig, src, "k", [0, 1, 1]),
        Err(Status::INVALID_ARG)
    );
    assert!(
        ex.create_compute_pipeline_from_msl(sig, "not msl", "k", [1, 1, 1])
            .is_err()
    );
    assert!(
        ex.create_compute_pipeline_from_msl(sig, src, "nope", [1, 1, 1])
            .is_err()
    );
    let mut d = DestroyPipelineArgs::new();
    d.pso = ok;
    assert_eq!(fx.status(d), Status::OK);
    assert_eq!(fx.status(d), Status::NOT_FOUND);
}

#[test]
fn fences_count_up_and_waits_time_out() {
    let fx = Fx::new();
    let f = fx.fence(5);
    assert_eq!(fx.fence_value(f), 5);
    fx.signal_fence(f, 3);
    assert_eq!(fx.fence_value(f), 5, "a signal never lowers a fence");
    fx.signal_fence(f, 9);
    assert_eq!(fx.fence_value(f), 9);
    assert_eq!(
        fx.wait_for(f, 9, 0),
        Status::OK,
        "already there: no waiting"
    );
    assert_eq!(fx.wait_for(f, 4, 0), Status::OK);
    assert_eq!(fx.wait_for(f, 10, 0), Status::TIMEOUT);
    let t = std::time::Instant::now();
    assert_eq!(fx.wait_for(f, 10, 80), Status::TIMEOUT);
    assert!(t.elapsed() >= std::time::Duration::from_millis(70));
    let mut c = CreateFenceArgs::new();
    c.flags = 1;
    assert_eq!(fx.status(c), Status::INVALID_ARG);
    let mut v = FenceValueArgs::new();
    v.fence = FenceId(99);
    assert_eq!(fx.status(v), Status::NOT_FOUND);
    let mut s = FenceSignalArgs::new();
    s.fence = FenceId(0);
    assert_eq!(fx.status(s), Status::NOT_FOUND);
    let mut d = DestroyFenceArgs::new();
    d.fence = f;
    assert_eq!(fx.status(d), Status::OK);
    assert_eq!(fx.status(d), Status::NOT_FOUND);
    assert_eq!(fx.wait_for(f, 1, 0), Status::NOT_FOUND);
}

#[test]
fn a_fence_wait_does_not_block_the_other_calls() {
    // `FENCE_WAIT` forever on one thread; another thread signals it with a
    // call that needs the same executor. If the wait held the executor the
    // signal could never get in.
    let fx = Fx::new();
    let f = fx.fence(0);
    let link = fx.link.clone();
    let waiter = std::thread::spawn(move || {
        let mut a = FenceWaitArgs::new();
        a.fence = f;
        a.value = 5;
        a.timeout_ms = u64::MAX;
        let t = std::time::Instant::now();
        let s = link.call_args(&mut a);
        (s, t.elapsed())
    });
    std::thread::sleep(std::time::Duration::from_millis(150));
    assert_eq!(
        fx.fence_value(f),
        0,
        "other calls go through while it waits"
    );
    fx.signal_fence(f, 5);
    let (s, waited) = waiter.join().unwrap();
    assert_eq!(s, Status::OK);
    assert!(
        waited >= std::time::Duration::from_millis(100),
        "{waited:?}"
    );
}

#[test]
fn queues_are_limited_destroyed_and_counted() {
    let fx = Fx::new();
    let mut a = CreateQueueArgs::new();
    a.kind = 9;
    assert_eq!(fx.status(a), Status::INVALID_ARG);
    let q: Vec<QueueId> = (0..16).map(|_| fx.queue()).collect();
    let mut a = CreateQueueArgs::new();
    a.kind = QueueKind::COMPUTE.0;
    assert_eq!(fx.status(a), Status::OUT_OF_MEMORY, "max_queues is 16");
    let mut d = DestroyQueueArgs::new();
    d.queue = q[3];
    assert_eq!(fx.status(d), Status::OK);
    assert_eq!(fx.status(d), Status::NOT_FOUND);
    d.queue = QueueId(0);
    assert_eq!(fx.status(d), Status::NOT_FOUND);
    let again = fx.queue();
    assert!(!q.contains(&again), "handles are not reused");
    assert_eq!(fx.ok(DeviceStatusArgs::new()).state, 0);
}

#[test]
fn shutdown_ends_the_session_and_hello_starts_a_new_one() {
    let fx = Fx::new();
    let q = fx.queue();
    let b = fx.buffer(BufferKind::SHARED, 256);
    let f = fx.fence(0);
    let cmds = [
        begin(1, q),
        fill(b.res, 0x0101_0101, 0, 256),
        end(1),
        signal_cmd(q, f, 1),
    ];
    assert_eq!(fx.submit(&cmds), Status::OK);
    assert_eq!(fx.status(ShutdownArgs::new()), Status::OK);
    assert!(fx.executor().device().is_none());
    assert_eq!(fx.submit(&cmds), Status::NOT_INITIALIZED);
    assert_eq!(fx.status(CreateQueueArgs::new()), Status::NOT_INITIALIZED);
    assert_eq!(fx.status(ShutdownArgs::new()), Status::NOT_INITIALIZED);
    assert_eq!(fx.status(NopArgs::new()), Status::OK);
    let mut h = HelloArgs::new();
    h.client_proto_version = PROTO_VERSION;
    assert_eq!(fx.status(h), Status::OK);
    // A new session: nothing of the old one exists.
    let mut d = DestroyBufferArgs::new();
    d.res = ResId(999);
    assert_eq!(fx.status(d), Status::NOT_FOUND);
    let q2 = fx.queue();
    assert_eq!(
        fx.status(DestroyQueueArgs {
            queue: q2,
            ..DestroyQueueArgs::new()
        }),
        Status::OK
    );
}

#[test]
fn shutdown_waits_for_work_in_flight() {
    // Work is queued behind a fence nobody signals yet, `SHUTDOWN` would wait
    // for it: the signal comes from another thread.
    let fx = Fx::new();
    let q = fx.queue();
    let gate = fx.fence(0);
    let done = fx.fence(0);
    let b = fx.buffer(BufferKind::SHARED, 256);
    cpu_write(b.cpu_addr, &[0; 64]);
    let cmds = [
        wait_cmd(q, gate, 1),
        begin(1, q),
        fill(b.res, 0x0505_0505, 0, 256),
        end(1),
        signal_cmd(q, done, 1),
    ];
    assert_eq!(fx.submit(&cmds), Status::OK);
    assert_eq!(
        fx.wait_for(done, 1, 100),
        Status::TIMEOUT,
        "the queue waits for the gate"
    );
    assert_eq!(cpu_read(b.cpu_addr, 1)[0], 0);
    let ex = fx.link.executor().clone();
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let mut a = FenceSignalArgs::new();
        a.fence = gate;
        a.value = 1;
        InProcess::from_executor(ex).call_args(&mut a)
    });
    let started = std::time::Instant::now();
    let mut d = DestroyQueueArgs::new();
    d.queue = q;
    assert_eq!(
        fx.status(d),
        Status::OK,
        "DESTROY_QUEUE waits for the queue to drain"
    );
    assert!(started.elapsed() >= std::time::Duration::from_millis(150));
    assert_eq!(t.join().unwrap(), Status::OK);
    assert_eq!(cpu_read(b.cpu_addr, 1)[0], 0x0505_0505);
    assert_eq!(fx.fence_value(done), 1);
    let _ = Caps::NONE;
}
