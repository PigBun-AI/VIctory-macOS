//! GPU integration tests of the M1 executor core. They need a Metal 4 GPU
//! (any Apple-silicon Mac on macOS 26); there is no software fallback, so
//! on a machine without one `Device::new` fails and these tests fail.
//!
//! Tests marked "observational" exercise a failure mode whose *absence* of
//! protection cannot be asserted (the outcome without the protection is
//! undefined or timing dependent); they print what happened and assert only
//! the protected side.
#![cfg(target_vendor = "apple")]

use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use victory_metal_back::descriptors::{
    ArgumentTable, DescriptorHeap, ENTRY_SIZE, IrDescriptorTableEntry, bind_point,
};
use victory_metal_back::device::Device;
use victory_metal_back::memory::BufferKind;
use victory_metal_back::queue::{DeviceStatus, Queue, QueueOptions};
use victory_metal_back::stages::{ALL, BLIT, DISPATCH, VIS_DEVICE};
use victory_metal_back::{Error, record::CommandList};

const WAIT_MS: u64 = 20_000;

/// The tests share one device and inspect device-wide state (residency
/// sets, retire queue), and a destroyed resource waits for *every* queue
/// that has work in flight; so they run one at a time.
fn setup() -> (MutexGuard<'static, ()>, Device) {
    static LOCK: Mutex<()> = Mutex::new(());
    static D: OnceLock<Device> = OnceLock::new();
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let d = D
        .get_or_init(|| Device::new().expect("Metal 4 device"))
        .clone();
    (guard, d)
}

fn submit_wait(q: &Queue, list: CommandList) {
    let v = q.submit(vec![list]).expect("submit");
    q.wait_for(v, WAIT_MS).expect("completes");
}

/// LCG the slow kernels run; the CPU computes the same value.
fn lcg(seed: u32, iters: u32) -> u32 {
    let mut x = seed;
    for _ in 0..iters {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    }
    x
}

const SLOW_A: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
kernel void slow_a(device const uint* in [[buffer(0)]], device uint* out [[buffer(1)]], \
constant uint& iters [[buffer(2)]]) {\n\
    uint x = in[0];\n\
    for (uint i = 0; i < iters; i++) { x = x * 1664525u + 1013904223u; }\n\
    out[0] = x;\n}\n\
kernel void plus_one(device const uint* in [[buffer(0)]], device uint* out [[buffer(1)]]) {\n\
    out[0] = in[0] + 1u;\n}\n";

#[test]
fn metal4_gate_and_caps() {
    let (_g, d) = setup();
    let c = d.caps();
    eprintln!("{c:#?}");
    assert!(c.metal4 && c.argument_buffers_tier2);
    assert!(c.unified_memory);
    assert!(c.max_buffer_len >= 1 << 30);
    assert_eq!(c.max_residency_sets_per_queue, 32);
    // 4 KiB pages exactly when translated, 16 KiB natively (Apple silicon).
    assert_eq!(c.page_size, if c.translated { 4096 } else { 16384 });
    assert_eq!(c.translated, cfg!(target_arch = "x86_64"));
    assert_eq!(c.simd_width, 32, "MSC needs wave size 32");
    assert!(c.wave32);
    assert!(c.max_threads_per_threadgroup[0] >= 1024);
    assert!(c.max_threadgroup_memory >= 32 * 1024);
}

#[test]
fn copy_and_fill_match_the_cpu() {
    let (_g, d) = setup();
    let q = Queue::new(&d).unwrap();
    let src = d.create_buffer(BufferKind::Upload, 4096).unwrap();
    let gpu = d.create_buffer(BufferKind::Default, 4096).unwrap();
    let out = d.create_buffer(BufferKind::Readback, 4096).unwrap();
    let data: Vec<u8> = (0..4096u32).map(|i| (i * 7 + 3) as u8).collect();
    // What CreateBuffer reports: cpu_addr 0 for GPU-only, alloc_size >= size.
    assert_eq!(gpu.cpu_addr(), 0);
    assert_ne!(src.cpu_addr(), 0);
    assert!(gpu.alloc_size() >= 4096 && src.alloc_size() >= 4096);
    src.write(0, &data).unwrap();
    out.write(0, &[0u8; 4096]).unwrap();

    let mut l = q.begin_list().unwrap();
    l.fill_buffer(&gpu, 0, 4096, 0xAB).unwrap();
    l.barrier(BLIT, BLIT, VIS_DEVICE).unwrap();
    l.copy_buffer(&gpu, 100, &src, 200, 1000).unwrap();
    l.barrier(BLIT, BLIT, VIS_DEVICE).unwrap();
    l.copy_buffer(&out, 0, &gpu, 0, 4096).unwrap();
    l.end().unwrap();
    let st = l.stats();
    assert_eq!((st.encoders, st.fills, st.copies), (1, 1, 2));
    assert_eq!(st.encoder_barriers, 2);
    submit_wait(&q, l);

    let mut got = vec![0u8; 4096];
    out.read(0, &mut got).unwrap();
    let mut want = vec![0xABu8; 4096];
    want[100..1100].copy_from_slice(&data[200..1200]);
    assert_eq!(got, want);
    assert_eq!(q.status(), DeviceStatus::Healthy);
    assert_eq!(q.completed_submissions(), 1);
}

#[test]
fn copy_argument_checks() {
    let (_g, d) = setup();
    let q = Queue::new(&d).unwrap();
    let a = d.create_buffer(BufferKind::Shared, 256).unwrap();
    let b = d.create_buffer(BufferKind::Shared, 256).unwrap();
    let mut l = q.begin_list().unwrap();
    assert!(matches!(
        l.copy_buffer(&a, 0, &b, 200, 100),
        Err(Error::BadArgument(_))
    ));
    assert!(
        matches!(
            l.copy_buffer(&a, 0, &a, 50, 100),
            Err(Error::BadArgument(_))
        ),
        "overlap"
    );
    assert!(
        l.copy_buffer(&a, 0, &a, 128, 128).is_ok(),
        "adjacent ranges are fine"
    );
    assert!(matches!(
        l.fill_buffer(&a, u64::MAX, 2, 0),
        Err(Error::BadArgument(_))
    ));
    l.end().unwrap();
    assert!(
        matches!(l.fill_buffer(&a, 0, 1, 0), Err(Error::BadArgument(_))),
        "list ended"
    );
    submit_wait(&q, l);
    assert!(d.create_buffer(BufferKind::Shared, 0).is_err());
    assert!(d.create_buffer(BufferKind::Wrapped, 16).is_err());
}

const SAXPY: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
kernel void saxpy(device const float* x [[buffer(0)]], device float* y [[buffer(1)]], \
uint i [[thread_position_in_grid]]) { y[i] = 2.0f * x[i] + y[i]; }\n";

#[test]
fn dispatch_matches_the_cpu() {
    let (_g, d) = setup();
    let q = Queue::new(&d).unwrap();
    let pso = d.compute_pipeline_from_msl(SAXPY, "saxpy").unwrap();
    assert_eq!(pso.thread_execution_width(), 32);
    let n = 1 << 16;
    let x = d.create_buffer(BufferKind::Shared, 4 * n).unwrap();
    let y = d.create_buffer(BufferKind::Shared, 4 * n).unwrap();
    let xs: Vec<f32> = (0..n).map(|i| i as f32 * 0.5).collect();
    let ys: Vec<f32> = (0..n).map(|i| (i % 17) as f32).collect();
    x.write(0, bytes(&xs)).unwrap();
    y.write(0, bytes(&ys)).unwrap();

    let mut l = q.begin_list().unwrap();
    l.set_address(0, x.gpu_va()).unwrap();
    l.set_address(1, y.gpu_va()).unwrap();
    l.dispatch(&pso, [(n / 256) as u32, 1, 1], [256, 1, 1])
        .unwrap();
    l.end().unwrap();
    submit_wait(&q, l);

    let mut raw = vec![0u8; 4 * n as usize];
    y.read(0, &mut raw).unwrap();
    for i in 0..n as usize {
        let got = f32::from_le_bytes(raw[4 * i..4 * i + 4].try_into().unwrap());
        assert_eq!(got, 2.0 * xs[i] + ys[i], "element {i}");
    }
    assert!(l_too_many_threads(&d, &q, &pso));
}

fn l_too_many_threads(
    _d: &Device,
    q: &Queue,
    pso: &victory_metal_back::pipeline::ComputePipeline,
) -> bool {
    let mut l = q.begin_list().unwrap();
    let r = l.dispatch(pso, [1, 1, 1], [4096, 1, 1]);
    l.end().unwrap();
    matches!(r, Err(Error::BadArgument(_)))
}

fn bytes<T>(v: &[T]) -> &[u8] {
    // SAFETY: plain-old-data slices of f32/u32.
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast(), std::mem::size_of_val(v)) }
}

/// Shader that follows the MSC binding model: heap at 0, TLAB at 2. The TLAB
/// holds a root constant, a root descriptor (GPU address) and a descriptor
/// table pointer, in the order and padding the test lays out itself.
const MSC_STYLE: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
struct Entry { ulong va; ulong tex; ulong md; };\n\
struct Tlab { uint scale; uint pad; ulong out_va; ulong table_va; };\n\
kernel void msc_style(device const Entry* heap [[buffer(0)]], \
constant Tlab& tlab [[buffer(2)]], uint i [[thread_position_in_grid]]) {\n\
    device const Entry* table = (device const Entry*)tlab.table_va;\n\
    Entry e = table[1];\n\
    device const uint* src = (device const uint*)e.va;\n\
    uint n = (uint)(e.md & 0xffffffffu) / 4u;\n\
    device uint* out = (device uint*)tlab.out_va;\n\
    if (i < n) out[i] = src[i] * tlab.scale + (uint)heap[0].md;\n}\n";

#[test]
fn descriptor_heap_and_root_arguments_reach_the_shader() {
    let (_g, d) = setup();
    let q = Queue::new(&d).unwrap();
    let pso = d.compute_pipeline_from_msl(MSC_STYLE, "msc_style").unwrap();
    let heap = DescriptorHeap::new(&d, 64).unwrap();
    assert_eq!(heap.stride(), 24);
    assert_eq!(heap.cpu_addr(), heap.buffer().cpu_addr());
    assert_eq!(
        heap.entry_va(2) - heap.gpu_va(),
        2 * u64::from(heap.stride())
    );
    let src = d.create_buffer(BufferKind::Shared, 1024).unwrap();
    let out = d.create_buffer(BufferKind::Shared, 1024).unwrap();
    let words: Vec<u32> = (0..256).collect();
    src.write_u32s(0, &words).unwrap();
    out.write_u32s(0, &[0; 256]).unwrap();

    // Heap entry 0 only carries a marker (metadata 1000); the descriptor
    // table starts at heap entry 10: entry 10 = unused, entry 11 = `src`.
    heap.write(0, IrDescriptorTableEntry::buffer(0, 1000))
        .unwrap();
    heap.write_buffer(11, &src, false).unwrap();
    assert_eq!(heap.read(11).unwrap().buffer_size(), 1024);
    assert_eq!(heap.read(11).unwrap().gpu_va, src.gpu_va());

    let tlab = q.alloc_constants(24, 8).unwrap();
    tlab.zero();
    tlab.write_u32s(0, &[3]).unwrap(); // root constant
    tlab.write_u64(8, out.gpu_va()).unwrap(); // root descriptor (CBV/UAV address)
    tlab.write_u64(16, heap.entry_va(10)).unwrap(); // table pointer

    let mut l = q.begin_list().unwrap();
    l.set_heaps(&heap, None).unwrap();
    l.set_root_arguments(&tlab).unwrap();
    l.dispatch(&pso, [1, 1, 1], [256, 1, 1]).unwrap();
    l.end().unwrap();
    submit_wait(&q, l);

    let got = out.read_u32s(0, 256).unwrap();
    let want: Vec<u32> = words.iter().map(|w| w * 3 + 1000).collect();
    assert_eq!(got, want);
    assert_eq!(bind_point::ARGUMENT_BUFFER, 2);
    assert_eq!(ENTRY_SIZE, 24);
}

#[test]
fn descriptor_writes_are_checked_against_live_resident_buffers() {
    let (_g, d) = setup();
    let heap = DescriptorHeap::new(&d, 8).unwrap();
    let buf = d.create_buffer(BufferKind::Shared, 4096).unwrap();
    let va = buf.gpu_va();
    assert!(buf.is_resident());
    // Inside the buffer: fine, also an interior range.
    heap.write_buffer_view(0, va + 256, 512, false).unwrap();
    // Outside any buffer, or running over the end: refused.
    assert!(matches!(
        heap.write_buffer_view(1, 0x1234_5678_0000, 16, false),
        Err(Error::NotResident(_))
    ));
    assert!(matches!(
        heap.write_buffer_view(1, va + 4000, 200, false),
        Err(Error::NotResident(_))
    ));
    // A buffer that never joined a residency set is refused too.
    let rogue = d.create_buffer_unmanaged(BufferKind::Shared, 4096).unwrap();
    assert!(!rogue.is_resident());
    assert!(matches!(
        heap.write_buffer(2, &rogue, false),
        Err(Error::NotResident(_))
    ));
    let q = Queue::new(&d).unwrap();
    let mut l = q.begin_list().unwrap();
    assert!(matches!(
        l.fill_buffer(&rogue, 0, 16, 1),
        Err(Error::NotResident(_))
    ));
    l.end().unwrap();
    submit_wait(&q, l);
    // Out-of-range heap index, and sizes the MSC descriptor cannot hold.
    assert!(heap.write(8, IrDescriptorTableEntry::default()).is_err());
    assert!(heap.write_buffer_view(3, va, 1 << 32, false).is_err());
    // After the last handle is gone the address no longer resolves.
    drop(buf);
    assert!(d.resolve_va(va, 1).is_none());
    // The table is usable.
    let t = ArgumentTable::new(&d, 4).unwrap();
    assert!(t.set_address(3, 0x1000).is_ok());
    assert!(t.set_address(4, 0x1000).is_err());
}

/// Observational: a kernel reads a buffer that is in no residency set. The
/// backend never lets this happen (previous test); this is what it would
/// look like. Not asserted: Metal documents it as undefined.
#[test]
#[ignore = "observational; the GPU may fault"]
fn unresident_read_observation() {
    let (_g, d) = setup();
    let q = Queue::new(&d).unwrap();
    let pso = d.compute_pipeline_from_msl(SLOW_A, "plus_one").unwrap();
    let rogue = d.create_buffer_unmanaged(BufferKind::Shared, 4096).unwrap();
    let out = d.create_buffer(BufferKind::Shared, 4096).unwrap();
    rogue.write_u32s(0, &[41]).unwrap();
    out.write_u32s(0, &[0]).unwrap();
    let mut l = q.begin_list().unwrap();
    l.set_address(0, rogue.gpu_va()).unwrap();
    l.set_address(1, out.gpu_va()).unwrap();
    l.dispatch(&pso, [1, 1, 1], [1, 1, 1]).unwrap();
    l.end().unwrap();
    let v = q.submit(vec![l]).unwrap();
    let r = q.wait_for(v, 5000);
    eprintln!(
        "unresident read: wait={r:?} out={:?} status={:?} failure={:?}",
        out.read_u32s(0, 1),
        q.status(),
        q.last_failure()
    );
}

const ITERS: u32 = 40_000_000;

/// Runs `slow_a` (single thread, `ITERS` LCG steps: out = f(in)) and then
/// `plus_one` (out2 = out + 1); returns (out, out2) and the list stats.
fn run_chain(
    d: &Device,
    q: &Queue,
    barrier: bool,
    split_encoder: bool,
) -> (u32, u32, victory_metal_back::record::ListStats) {
    let a = d.compute_pipeline_from_msl(SLOW_A, "slow_a").unwrap();
    let b = d.compute_pipeline_from_msl(SLOW_A, "plus_one").unwrap();
    let input = d.create_buffer(BufferKind::Shared, 256).unwrap();
    let mid = d.create_buffer(BufferKind::Shared, 256).unwrap();
    let outb = d.create_buffer(BufferKind::Shared, 256).unwrap();
    let iters = d.create_buffer(BufferKind::Shared, 256).unwrap();
    input.write_u32s(0, &[12345]).unwrap();
    mid.write_u32s(0, &[0]).unwrap();
    outb.write_u32s(0, &[0]).unwrap();
    iters.write_u32s(0, &[ITERS]).unwrap();
    let mut l = q.begin_list().unwrap();
    l.set_address(0, input.gpu_va()).unwrap();
    l.set_address(1, mid.gpu_va()).unwrap();
    l.set_address(2, iters.gpu_va()).unwrap();
    l.dispatch(&a, [1, 1, 1], [1, 1, 1]).unwrap();
    if barrier {
        l.barrier(DISPATCH, DISPATCH, VIS_DEVICE).unwrap();
    }
    if split_encoder {
        l.split_encoder().unwrap();
    }
    l.set_address(0, mid.gpu_va()).unwrap();
    l.set_address(1, outb.gpu_va()).unwrap();
    l.dispatch(&b, [1, 1, 1], [1, 1, 1]).unwrap();
    l.end().unwrap();
    let st = l.stats();
    submit_wait(q, l);
    (
        mid.read_u32s(0, 1).unwrap()[0],
        outb.read_u32s(0, 1).unwrap()[0],
        st,
    )
}

#[test]
fn barrier_between_dependent_dispatches_orders_them() {
    let (_g, d) = setup();
    let q = Queue::new(&d).unwrap();
    let x = lcg(12345, ITERS);
    // Same encoder (barrier encoded in the encoder).
    for round in 0..5 {
        let (mid, out, st) = run_chain(&d, &q, true, false);
        assert_eq!((mid, out), (x, x.wrapping_add(1)), "round {round}");
        // One encoder: the head barrier (consumer form) and the one in the encoder.
        assert_eq!(
            (st.encoders, st.encoder_barriers, st.consumer_barriers),
            (1, 1, 1)
        );
    }
    // Across two encoders: each starts with its head barrier, which already
    // covers the recorded device-visibility barrier (no third one).
    for round in 0..5 {
        let (mid, out, st) = run_chain(&d, &q, true, true);
        assert_eq!((mid, out), (x, x.wrapping_add(1)), "round {round}");
        assert_eq!(
            (st.encoders, st.encoder_barriers, st.consumer_barriers),
            (2, 0, 2)
        );
    }
    // The same with the queue's own ordering off: the recorded barrier alone,
    // a consumer barrier at the start of the second encoder.
    q.set_inter_submit_barrier(false);
    let (mid, out, st) = run_chain(&d, &q, true, true);
    assert_eq!((mid, out), (x, x.wrapping_add(1)));
    assert_eq!(
        (
            st.encoders,
            st.encoder_barriers,
            st.consumer_barriers,
            st.producer_barriers
        ),
        (2, 0, 1, 0),
        "one consumer barrier covers the earlier encoder and the earlier lists"
    );
}

/// Observational: the same chain with no barrier. Metal 4 may overlap the
/// two dispatches, so `plus_one` can read the old value. Results are
/// printed; the test passes either way.
#[test]
fn missing_barrier_observation() {
    let (_g, d) = setup();
    let q = Queue::new(&d).unwrap();
    // The queue's own head barrier would order the two encoders too.
    q.set_inter_submit_barrier(false);
    let x = lcg(12345, ITERS);
    for (name, split) in [("same encoder", false), ("two encoders", true)] {
        let mut stale = 0;
        for _ in 0..10 {
            let (mid, out, _) = run_chain(&d, &q, false, split);
            assert_eq!(mid, x, "the slow kernel itself is always right");
            if out != x.wrapping_add(1) {
                stale += 1;
            }
        }
        eprintln!("no barrier, {name}: {stale}/10 runs read a stale value");
    }
}

#[test]
fn consecutive_lists_are_ordered_by_the_inter_submit_barrier() {
    let (_g, d) = setup();
    let q = Queue::new(&d).unwrap();
    assert!(q.inter_submit_barrier());
    let a = d.compute_pipeline_from_msl(SLOW_A, "slow_a").unwrap();
    let b = d.compute_pipeline_from_msl(SLOW_A, "plus_one").unwrap();
    let input = d.create_buffer(BufferKind::Shared, 256).unwrap();
    let mid = d.create_buffer(BufferKind::Shared, 256).unwrap();
    let outb = d.create_buffer(BufferKind::Shared, 256).unwrap();
    let iters = d.create_buffer(BufferKind::Shared, 256).unwrap();
    input.write_u32s(0, &[12345]).unwrap();
    iters.write_u32s(0, &[ITERS]).unwrap();
    let x = lcg(12345, ITERS);
    for round in 0..3 {
        mid.write_u32s(0, &[0]).unwrap();
        outb.write_u32s(0, &[0]).unwrap();
        let mut l1 = q.begin_list().unwrap();
        l1.set_address(0, input.gpu_va()).unwrap();
        l1.set_address(1, mid.gpu_va()).unwrap();
        l1.set_address(2, iters.gpu_va()).unwrap();
        l1.dispatch(&a, [1, 1, 1], [1, 1, 1]).unwrap();
        l1.end().unwrap();
        let mut l2 = q.begin_list().unwrap();
        l2.set_address(0, mid.gpu_va()).unwrap();
        l2.set_address(1, outb.gpu_va()).unwrap();
        l2.dispatch(&b, [1, 1, 1], [1, 1, 1]).unwrap();
        l2.end().unwrap();
        // The ordering is the head barrier of each list's encoder: no
        // barrier at the end, no extra encoder.
        for l in [&l1, &l2] {
            let st = l.stats();
            assert_eq!((st.encoders, st.consumer_barriers), (1, 1));
            assert_eq!(st.producer_barriers, 0);
        }
        // One commit of both lists, in order.
        let v = q.submit(vec![l1, l2]).unwrap();
        q.wait_for(v, WAIT_MS).unwrap();
        assert_eq!(
            outb.read_u32s(0, 1).unwrap()[0],
            x.wrapping_add(1),
            "round {round}, one commit"
        );
    }
}

/// The same, as two commits (two `ExecuteCommandLists` calls), ten times:
/// without the barrier the observational test below reads the stale value
/// every time, with it the dependent kernel is always right.
#[test]
fn consecutive_submits_are_ordered_by_the_inter_submit_barrier() {
    let (_g, d) = setup();
    let q = Queue::new(&d).unwrap();
    let a = d.compute_pipeline_from_msl(SLOW_A, "slow_a").unwrap();
    let b = d.compute_pipeline_from_msl(SLOW_A, "plus_one").unwrap();
    let input = d.create_buffer(BufferKind::Shared, 256).unwrap();
    let mid = d.create_buffer(BufferKind::Shared, 256).unwrap();
    let outb = d.create_buffer(BufferKind::Shared, 256).unwrap();
    let iters = d.create_buffer(BufferKind::Shared, 256).unwrap();
    input.write_u32s(0, &[12345]).unwrap();
    iters.write_u32s(0, &[ITERS]).unwrap();
    let want = lcg(12345, ITERS).wrapping_add(1);
    for round in 0..10 {
        mid.write_u32s(0, &[0]).unwrap();
        outb.write_u32s(0, &[0]).unwrap();
        let mut l1 = q.begin_list().unwrap();
        l1.set_address(0, input.gpu_va()).unwrap();
        l1.set_address(1, mid.gpu_va()).unwrap();
        l1.set_address(2, iters.gpu_va()).unwrap();
        l1.dispatch(&a, [1, 1, 1], [1, 1, 1]).unwrap();
        l1.end().unwrap();
        q.submit(vec![l1]).unwrap();
        // An empty list in between costs no encoder and breaks nothing.
        let mut empty = q.begin_list().unwrap();
        empty.end().unwrap();
        assert_eq!(empty.stats().encoders, 0);
        q.submit(vec![empty]).unwrap();
        let mut l2 = q.begin_list().unwrap();
        l2.set_address(0, mid.gpu_va()).unwrap();
        l2.set_address(1, outb.gpu_va()).unwrap();
        l2.dispatch(&b, [1, 1, 1], [1, 1, 1]).unwrap();
        l2.end().unwrap();
        let v = q.submit(vec![l2]).unwrap();
        q.wait_for(v, WAIT_MS).unwrap();
        assert_eq!(outb.read_u32s(0, 1).unwrap()[0], want, "round {round}");
    }
}

#[test]
fn destroying_a_buffer_in_use_is_deferred() {
    let (_g, d) = setup();
    let q = Queue::new(&d).unwrap();
    d.maintain();
    let a = d.compute_pipeline_from_msl(SLOW_A, "slow_a").unwrap();
    let input = d.create_buffer(BufferKind::Shared, 256).unwrap();
    let mid = d.create_buffer(BufferKind::Shared, 256).unwrap();
    let iters = d.create_buffer(BufferKind::Shared, 256).unwrap();
    input.write_u32s(0, &[12345]).unwrap();
    mid.write_u32s(0, &[0]).unwrap();
    iters.write_u32s(0, &[ITERS * 2]).unwrap();
    let sets_before = d
        .residency()
        .len(victory_metal_back::residency::SetKind::Resource);
    let (in_va, iters_va) = (input.gpu_va(), iters.gpu_va());

    let mut l = q.begin_list().unwrap();
    l.set_address(0, in_va).unwrap();
    l.set_address(1, mid.gpu_va()).unwrap();
    l.set_address(2, iters_va).unwrap();
    l.dispatch(&a, [1, 1, 1], [1, 1, 1]).unwrap();
    l.end().unwrap();
    let v = q.submit(vec![l]).unwrap();
    let t = Instant::now();
    // The front end "destroys" the input and the iteration count while the
    // kernel is still running.
    drop(input);
    drop(iters);
    let pending = d.residency().pending_retired();
    let still_in_set = d
        .residency()
        .len(victory_metal_back::residency::SetKind::Resource);
    let freed_now = d.maintain();
    let running = q.timeline().completed() < v;
    eprintln!(
        "destroyed while running: kernel still running={running} after {:?}; pending={pending}, freed immediately={freed_now}",
        t.elapsed()
    );
    // The address stops resolving for new descriptors at once ...
    assert!(d.resolve_va(in_va, 1).is_none());
    // ... but the resources stay in their set until the GPU is done.
    assert!(pending >= 2);
    assert_eq!(still_in_set, sets_before);
    if running {
        assert_eq!(freed_now, 0, "nothing may be removed while the kernel runs");
    }
    q.wait_for(v, WAIT_MS).unwrap();
    assert_eq!(mid.read_u32s(0, 1).unwrap()[0], lcg(12345, ITERS * 2));
    assert!(d.maintain() >= pending - freed_now);
    assert_eq!(d.residency().pending_retired(), 0);
    assert_eq!(q.status(), DeviceStatus::Healthy);
    eprintln!("kernel was still running when buffers were destroyed: {running}");
    assert!(
        running,
        "the test kernel must outlive the drop; raise ITERS"
    );
}

#[test]
fn wrapped_memory_page_rules() {
    let (_g, d) = setup();
    let page = d.caps().page_size as usize;
    // Anonymous mapping: 6 pages, page aligned.
    // SAFETY: mmap of fresh anonymous memory, unmapped at the end.
    let base = unsafe { libc_mmap(6 * page) };
    assert!(!base.is_null());
    // SAFETY: inside the mapping, which outlives every buffer below.
    unsafe {
        let a = d.wrap(base, 2 * page).expect("first range");
        assert_eq!(a.kind(), BufferKind::Wrapped);
        assert_eq!(a.set(), victory_metal_back::residency::SetKind::Title);
        assert!(a.is_resident());
        // Overlap (one shared page) is refused, in both directions.
        assert!(matches!(
            d.wrap(base.add(page), 2 * page),
            Err(Error::PageConflict { .. })
        ));
        assert!(matches!(
            d.wrap(base, page),
            Err(Error::PageConflict { .. })
        ));
        // Adjacent range is fine; misaligned or odd sizes are not.
        let b = d.wrap(base.add(2 * page), 2 * page).expect("adjacent");
        assert!(matches!(
            d.wrap(base.add(4 * page + 8), page),
            Err(Error::BadArgument(_))
        ));
        assert!(matches!(
            d.wrap(base.add(4 * page), page + 4),
            Err(Error::BadArgument(_))
        ));
        assert!(matches!(
            d.wrap(std::ptr::null_mut(), page),
            Err(Error::BadArgument(_))
        ));
        // The GPU sees the caller's memory.
        let q = Queue::new(&d).unwrap();
        let mut l = q.begin_list().unwrap();
        l.fill_buffer(&a, 0, 64, 0x5A).unwrap();
        l.end().unwrap();
        submit_wait(&q, l);
        assert_eq!(*base, 0x5A);
        assert_eq!(*base.add(63), 0x5A);
        assert_eq!(*base.add(64), 0);
        // Release: dropping frees the claim only after the retirement is
        // reclaimed, then the range can be wrapped again.
        drop(a);
        assert!(
            matches!(d.wrap(base, 2 * page), Err(Error::PageConflict { .. })),
            "still retired, not released"
        );
        d.maintain();
        let again = d.wrap(base, 2 * page).expect("range free again");
        drop((again, b));
        d.maintain();
        libc_munmap(base, 6 * page);
    }
}

unsafe fn libc_mmap(len: usize) -> *mut u8 {
    unsafe extern "C" {
        fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    }
    // PROT_READ|PROT_WRITE = 3; MAP_PRIVATE|MAP_ANON = 0x0002|0x1000.
    let p = unsafe { mmap(std::ptr::null_mut(), len, 3, 0x1002, -1, 0) };
    if p as isize == -1 {
        std::ptr::null_mut()
    } else {
        p
    }
}

unsafe fn libc_munmap(p: *mut u8, len: usize) {
    unsafe extern "C" {
        fn munmap(addr: *mut u8, len: usize) -> i32;
    }
    assert_eq!(unsafe { munmap(p, len) }, 0);
}

#[test]
fn fences_order_the_queue_and_the_cpu() {
    let (_g, d) = setup();
    let q = Queue::new(&d).unwrap();
    let fence = d.create_fence().unwrap();
    let buf = d.create_buffer(BufferKind::Shared, 256).unwrap();
    buf.write_u32s(0, &[0]).unwrap();

    // Queue waits for a CPU signal; the fill must not run before it.
    q.wait_fence(&fence, 1);
    let mut l = q.begin_list().unwrap();
    l.fill_buffer(&buf, 0, 4, 0xFF).unwrap();
    l.end().unwrap();
    let v = q.submit(vec![l]).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        q.timeline().completed() < v,
        "queue must be blocked on the fence"
    );
    assert_eq!(buf.read_u32s(0, 1).unwrap()[0], 0);
    fence.signal_cpu(1);
    q.wait_for(v, WAIT_MS).unwrap();
    assert_eq!(buf.read_u32s(0, 1).unwrap()[0], 0xFFFF_FFFF);

    // Queue signals; CPU waits (timeout path included).
    assert!(!fence.wait(2, 50), "nobody signalled 2 yet");
    q.signal(&fence, 2);
    assert!(fence.wait(2, 5000));
    assert!(fence.completed_value() >= 2);
}

#[test]
fn many_submissions_recycle_allocators_and_ring() {
    let (_g, d) = setup();
    let q = Queue::with_options(
        &d,
        QueueOptions {
            in_flight: 2,
            max_lists: 4,
            ring_segment_bytes: 4096,
            ring_segments: 2,
            inter_submit_barrier: true,
        },
    )
    .unwrap();
    let pso = d.compute_pipeline_from_msl(MSC_STYLE, "msc_style").unwrap();
    let heap = DescriptorHeap::new(&d, 16).unwrap();
    let src = d.create_buffer(BufferKind::Shared, 1024).unwrap();
    let out = d.create_buffer(BufferKind::Shared, 1024).unwrap();
    src.write_u32s(0, &(0..256).collect::<Vec<u32>>()).unwrap();
    heap.write(0, IrDescriptorTableEntry::buffer(0, 0)).unwrap();
    heap.write_buffer(1, &src, false).unwrap();
    let mut last = 0;
    for round in 0..200u32 {
        // 3 KiB per round: forces the 2-segment ring to wrap and wait.
        let tlab = q.alloc_constants(24, 8).unwrap();
        let pad = q.alloc_constants(3000, 16).unwrap();
        pad.zero();
        tlab.write_u32s(0, &[round]).unwrap();
        tlab.write_u64(8, out.gpu_va()).unwrap();
        tlab.write_u64(16, heap.entry_va(0)).unwrap();
        let mut l = q.begin_list().unwrap();
        l.set_heaps(&heap, None).unwrap();
        l.set_root_arguments(&tlab).unwrap();
        l.dispatch(&pso, [1, 1, 1], [256, 1, 1]).unwrap();
        l.end().unwrap();
        last = q.submit(vec![l]).unwrap();
    }
    q.wait_for(last, WAIT_MS).unwrap();
    assert_eq!(out.read_u32s(0, 4).unwrap(), vec![0, 199, 398, 597]);
    assert_eq!(q.submissions(), 200);
    assert!(q.alloc_constants(8192, 8).is_err(), "bigger than a segment");
    assert_eq!(q.completed_submissions(), 200);
}

#[test]
fn residency_sets_are_attached_and_limited() {
    let (_g, d) = setup();
    let q = Queue::new(&d).unwrap();
    assert_eq!(q.residency_sets(), 4);
    let sets = d.residency().sets();
    // Attaching the same set again counts against the limit (the queue
    // would refuse duplicates silently; we count what we attached).
    for _ in 0..28 {
        q.add_residency_set(&sets[0]).unwrap();
    }
    assert_eq!(q.residency_sets(), 32);
    assert!(q.add_residency_set(&sets[0]).is_err());
}

#[test]
fn end_of_empty_list_with_barrier_is_valid() {
    let (_g, d) = setup();
    let q = Queue::new(&d).unwrap();
    // An empty list opens no encoder even with the inter-submit barrier on:
    // the barrier is the head of the next list's encoder.
    let mut l = q.begin_list().unwrap();
    l.end().unwrap();
    assert_eq!(l.stats().encoders, 0);
    assert_eq!(l.stats().producer_barriers, 0);
    submit_wait(&q, l);
    q.set_inter_submit_barrier(false);
    let mut l = q.begin_list().unwrap();
    l.end().unwrap();
    assert_eq!(
        l.stats().encoders,
        0,
        "an empty list without barrier has no encoder"
    );
    submit_wait(&q, l);
    // A global barrier nobody consumes still ends up in the list.
    let mut l = q.begin_list().unwrap();
    l.barrier(ALL, ALL, VIS_DEVICE).unwrap();
    l.end().unwrap();
    submit_wait(&q, l);
}

#[test]
fn without_the_inter_submit_barrier_a_barrier_reaches_back_to_earlier_lists() {
    let (_g, d) = setup();
    let q = Queue::new(&d).unwrap();
    q.set_inter_submit_barrier(false);
    let x = lcg(12345, ITERS);
    let (mid, out, st) = run_chain(&d, &q, true, false);
    assert_eq!((mid, out), (x, x.wrapping_add(1)));
    // The recorded barrier is encoded in the encoder, and, because nothing
    // orders this list after earlier ones, once more as a consumer barrier
    // for work of earlier lists; no trailing barrier.
    assert_eq!(
        (
            st.encoders,
            st.encoder_barriers,
            st.consumer_barriers,
            st.producer_barriers
        ),
        (1, 1, 1, 0)
    );
}

/// Observational: do two lists of one queue overlap when nothing orders
/// them? Slow kernel in list 1, dependent kernel in list 2, no barrier of
/// any kind (inter-submit barrier off), once as one commit of two lists and
/// once as two commits. Printed, not asserted.
#[test]
fn unordered_lists_observation() {
    let (_g, d) = setup();
    let q = Queue::new(&d).unwrap();
    q.set_inter_submit_barrier(false);
    let a = d.compute_pipeline_from_msl(SLOW_A, "slow_a").unwrap();
    let b = d.compute_pipeline_from_msl(SLOW_A, "plus_one").unwrap();
    let input = d.create_buffer(BufferKind::Shared, 256).unwrap();
    let mid = d.create_buffer(BufferKind::Shared, 256).unwrap();
    let outb = d.create_buffer(BufferKind::Shared, 256).unwrap();
    let iters = d.create_buffer(BufferKind::Shared, 256).unwrap();
    input.write_u32s(0, &[12345]).unwrap();
    iters.write_u32s(0, &[ITERS]).unwrap();
    let want = lcg(12345, ITERS).wrapping_add(1);
    for (name, one_commit) in [("one commit", true), ("two commits", false)] {
        let mut stale = 0;
        for _ in 0..10 {
            mid.write_u32s(0, &[0]).unwrap();
            outb.write_u32s(0, &[0]).unwrap();
            let mut l1 = q.begin_list().unwrap();
            l1.set_address(0, input.gpu_va()).unwrap();
            l1.set_address(1, mid.gpu_va()).unwrap();
            l1.set_address(2, iters.gpu_va()).unwrap();
            l1.dispatch(&a, [1, 1, 1], [1, 1, 1]).unwrap();
            l1.end().unwrap();
            let mut l2 = q.begin_list().unwrap();
            l2.set_address(0, mid.gpu_va()).unwrap();
            l2.set_address(1, outb.gpu_va()).unwrap();
            l2.dispatch(&b, [1, 1, 1], [1, 1, 1]).unwrap();
            l2.end().unwrap();
            let v = if one_commit {
                q.submit(vec![l1, l2]).unwrap()
            } else {
                q.submit(vec![l1]).unwrap();
                q.submit(vec![l2]).unwrap()
            };
            q.wait_for(v, WAIT_MS).unwrap();
            if outb.read_u32s(0, 1).unwrap()[0] != want {
                stale += 1;
            }
        }
        eprintln!("no inter-list barrier, {name}: {stale}/10 runs read a stale value");
    }
}

#[test]
fn a_missing_entry_point_is_an_error_not_an_abort() {
    // The Metal 4 compiler aborts the process on a function that is not in
    // the library; the back asks the library first.
    let (_g, d) = setup();
    assert!(matches!(
        d.compute_pipeline_from_msl(SLOW_A, "nope"),
        Err(Error::BadArgument(_))
    ));
    let lib = d.library_from_msl(SLOW_A).unwrap();
    assert!(d.compute_pipeline(&lib, "").is_err());
    assert!(d.compute_pipeline(&lib, "slow_a").is_ok());
    assert!(d.compute_pipeline_from_msl("kernel void {", "k").is_err());
}
