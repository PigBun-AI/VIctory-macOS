//! Timing of the executor core's hot paths. `#[ignore]`d: run with
//! `cargo test --release -p victory-metal-back --test perf -- --ignored --nocapture --test-threads=1`.
//! Prints one line per measurement (`PERF name = value unit`).
#![cfg(target_vendor = "apple")]

use std::time::Instant;

use victory_metal_back::descriptors::ConstantRing;
use victory_metal_back::device::Device;
use victory_metal_back::memory::BufferKind;
use victory_metal_back::queue::{Queue, QueueOptions};
use victory_metal_back::stages::{DISPATCH, VIS_DEVICE};

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn queue(d: &Device, barrier: bool) -> Queue {
    Queue::with_options(
        d,
        QueueOptions {
            inter_submit_barrier: barrier,
            max_lists: 2048,
            ..Default::default()
        },
    )
    .unwrap()
}

const NOP: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
kernel void nop(device uint* o [[buffer(0)]], uint i [[thread_position_in_grid]]) { if (i == 0xffffffffu) o[0] = 1; }\n\
kernel void spin(device uint* o [[buffer(0)]], constant uint& n [[buffer(1)]], uint t [[thread_position_in_grid]]) { uint x = t; for (uint i = 0; i < n; i++) { x = x * 1664525u + 1013904223u; } o[t] = x; }\n\
kernel void touch(device uint* o [[buffer(0)]], uint i [[thread_position_in_grid]]) { o[i] = o[i] + 1u; }\n";

#[test]
#[ignore = "timing"]
fn perf() {
    let d = Device::new().unwrap();
    let arch = if d.caps().translated {
        "x86_64/Rosetta"
    } else {
        "arm64"
    };
    println!("PERF arch = {arch}");
    println!("PERF device = {}", d.caps().name);

    // 1. Empty submit round trip: begin, end, submit, CPU waits for the GPU.
    for barrier in [false, true] {
        let q = queue(&d, barrier);
        let mut t = Vec::new();
        for i in 0..2200 {
            let s = Instant::now();
            let mut l = q.begin_list().unwrap();
            l.end().unwrap();
            let v = q.submit(vec![l]).unwrap();
            q.wait_for(v, 10_000).unwrap();
            if i >= 200 {
                t.push(s.elapsed().as_secs_f64() * 1e6);
            }
        }
        println!(
            "PERF empty_submit_roundtrip[inter_submit_barrier={barrier}] = p50 {:.0} / p99 {:.0} us (n=2000)",
            pct(&mut t.clone(), 0.5),
            pct(&mut t, 0.99)
        );
    }

    // 2. Cost of the conservative barrier between lists: 400 lists, each a
    //    1M-thread dispatch touching 4 MiB, committed one by one without
    //    waiting; wall time of the whole batch.
    let pso = d.compute_pipeline_from_msl(NOP, "touch").unwrap();
    let big = d.create_buffer(BufferKind::Default, 4 << 20).unwrap();
    for (label, threads) in [("tiny (256 threads)", 256u32), ("1M threads", 1 << 20)] {
        let mut res = Vec::new();
        for barrier in [false, true] {
            let q = queue(&d, barrier);
            let mut best = f64::MAX;
            for _ in 0..5 {
                let s = Instant::now();
                let mut last = 0;
                for _ in 0..400 {
                    let mut l = q.begin_list().unwrap();
                    l.set_address(0, big.gpu_va()).unwrap();
                    l.dispatch(&pso, [threads / 256, 1, 1], [256, 1, 1])
                        .unwrap();
                    l.end().unwrap();
                    last = q.submit(vec![l]).unwrap();
                }
                q.wait_for(last, 30_000).unwrap();
                best = best.min(s.elapsed().as_secs_f64() * 1e3);
            }
            res.push(best);
        }
        println!(
            "PERF 400_lists_one_dispatch_each[{label}] = barrier off {:.2} ms, on {:.2} ms (best of 5; {:+.0}%)",
            res[0],
            res[1],
            (res[1] / res[0] - 1.0) * 100.0
        );
    }

    // 2b. The same question where it matters: 64 lists, each one small
    //     kernel (one threadgroup, ~0.3 ms of GPU time) on its own output;
    //     with no barrier the GPU may run them side by side.
    {
        let spin = d.compute_pipeline_from_msl(NOP, "spin").unwrap();
        let iters = d.create_buffer(BufferKind::Shared, 256).unwrap();
        iters.write_u32s(0, &[300_000]).unwrap();
        let out = d.create_buffer(BufferKind::Default, 64 * 256 * 4).unwrap();
        let mut res = Vec::new();
        for barrier in [false, true] {
            let q = queue(&d, barrier);
            let mut best = f64::MAX;
            for _ in 0..5 {
                let s = Instant::now();
                let mut last = 0;
                for k in 0..64u64 {
                    let mut l = q.begin_list().unwrap();
                    l.set_address(0, out.gpu_va() + k * 1024).unwrap();
                    l.set_address(1, iters.gpu_va()).unwrap();
                    l.dispatch(&spin, [1, 1, 1], [256, 1, 1]).unwrap();
                    l.end().unwrap();
                    last = q.submit(vec![l]).unwrap();
                }
                q.wait_for(last, 30_000).unwrap();
                best = best.min(s.elapsed().as_secs_f64() * 1e3);
            }
            res.push(best);
        }
        println!(
            "PERF 64_lists_independent_small_kernels = barrier off {:.2} ms, on {:.2} ms (best of 5; x{:.1})",
            res[0],
            res[1],
            res[1] / res[0]
        );
    }

    // 3. 1000 small dispatches in one list / one submit.
    for (label, between) in [("no barriers", false), ("barrier between each", true)] {
        let q = queue(&d, true);
        let mut rec = Vec::new();
        let mut total = Vec::new();
        for _ in 0..30 {
            let s = Instant::now();
            let mut l = q.begin_list().unwrap();
            l.set_address(0, big.gpu_va()).unwrap();
            for _ in 0..1000 {
                l.dispatch(&pso, [4, 1, 1], [256, 1, 1]).unwrap();
                if between {
                    l.barrier(DISPATCH, DISPATCH, VIS_DEVICE).unwrap();
                }
            }
            l.end().unwrap();
            let recorded = s.elapsed();
            let v = q.submit(vec![l]).unwrap();
            q.wait_for(v, 30_000).unwrap();
            rec.push(recorded.as_secs_f64() * 1e6);
            total.push(s.elapsed().as_secs_f64() * 1e6);
        }
        println!(
            "PERF 1000_dispatches_one_submit[{label}] = record p50 {:.0} us ({:.2} us/dispatch), record+submit+GPU p50 {:.0} us (n=30)",
            pct(&mut rec, 0.5),
            pct(&mut rec.clone(), 0.5) / 1000.0,
            pct(&mut total, 0.5)
        );
    }

    // 4. Constant ring: allocate + write a 64-byte root-argument block
    //    (4 root constants, 4 root descriptors, 1 table pointer, padded),
    //    sealing a segment's worth the way a submission does.
    {
        let mut ring = ConstantRing::new(&d, 1 << 20, 4).unwrap();
        let n = 4_000_000u64;
        let mut value = 0u64;
        let mut allocs = 0u64;
        let s = Instant::now();
        let mut written = 0u64;
        while allocs < n {
            match ring.try_alloc(64, 8, value) {
                Ok(a) => {
                    a.write_u32s(0, &[1, 2, 3, 4]).unwrap();
                    a.write_u64(16, 0x1000).unwrap();
                    a.write_u64(24, 0x2000).unwrap();
                    a.write_u64(32, 0x3000).unwrap();
                    a.write_u64(40, 0x4000).unwrap();
                    a.write_u64(48, 0x5000).unwrap();
                    allocs += 1;
                    written += 64;
                }
                Err(_) => {
                    value += 1;
                    ring.seal(value);
                }
            }
        }
        let secs = s.elapsed().as_secs_f64();
        println!(
            "PERF constant_ring_64B_alloc_and_write = {:.1} M/s ({:.0} ns each, {:.1} GB/s)",
            allocs as f64 / secs / 1e6,
            secs * 1e9 / allocs as f64,
            written as f64 / secs / 1e9
        );
    }

    // 5. Residency bookkeeping: create + drop of 1000 buffers, maintain().
    {
        let s = Instant::now();
        let bufs: Vec<_> = (0..1000)
            .map(|_| d.create_buffer(BufferKind::Default, 4096).unwrap())
            .collect();
        let created = s.elapsed();
        d.maintain();
        let s2 = Instant::now();
        drop(bufs);
        let dropped = s2.elapsed();
        let s3 = Instant::now();
        let freed = d.maintain();
        println!(
            "PERF 1000_buffers = create {:.0} us each, drop {:.1} us each, maintain (commit + remove {freed}) {:.2} ms",
            created.as_secs_f64() * 1e3,
            dropped.as_secs_f64() * 1e3,
            s3.elapsed().as_secs_f64() * 1e3
        );
    }
}
