//! `VICTORY_METAL_GPUPROF`: the GPU time of a scope is read back from a timestamp heap and sums
//! by label. One test in its own process: the switch is read once.
#![cfg(target_vendor = "apple")]

use victory_metal_back::device::Device;
use victory_metal_back::memory::BufferKind;
use victory_metal_back::queue::Queue;

const KERNELS: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
kernel void spin(device uint* o [[buffer(0)]], constant uint& n [[buffer(1)]], uint t [[thread_position_in_grid]]) { uint x = t; for (uint i = 0; i < n; i++) { x = x * 1664525u + 1013904223u; } o[t] = x; }\n\
kernel void nop(device uint* o [[buffer(0)]], uint i [[thread_position_in_grid]]) { if (i == 0xffffffffu) o[0] = 1; }\n";

#[test]
fn scopes_are_timed_by_label_and_the_heavy_one_is_first() {
    // SAFETY: the only test of this process, nothing reads the environment concurrently.
    unsafe { std::env::set_var("VICTORY_METAL_GPUPROF", "1") };
    let d = Device::new().unwrap();
    let spin = d.compute_pipeline_from_msl(KERNELS, "spin").unwrap();
    let nop = d.compute_pipeline_from_msl(KERNELS, "nop").unwrap();
    let out = d.create_buffer(BufferKind::Default, 4 << 20).unwrap();
    let n = d.create_buffer(BufferKind::Shared, 256).unwrap();
    n.write_u32s(0, &[200_000]).unwrap();
    let q = Queue::new(&d).unwrap();
    let mut last = 0;
    let t0 = std::time::Instant::now();
    for _ in 0..3 {
        let mut l = q.begin_list().unwrap();
        l.set_address(0, out.gpu_va()).unwrap();
        l.set_address(1, n.gpu_va()).unwrap();
        l.dispatch(&nop, [1, 1, 1], [32, 1, 1]).unwrap();
        l.prof_sample(|| "cs light".into());
        l.dispatch(&spin, [4096, 1, 1], [256, 1, 1]).unwrap();
        l.prof_sample(|| "cs heavy".into());
        l.end().unwrap();
        last = q.submit(vec![l]).unwrap();
    }
    q.wait_for(last, 30_000).unwrap();
    println!(
        "WALL {:.1} ms for 3 lists",
        t0.elapsed().as_secs_f64() * 1e3
    );
    // The card's completion is signalled; the heaps are read when a list is looked at again.
    let table = victory_metal_back::prof::flush_table();
    println!("{table}");
    let heavy = table.find("cs heavy").expect("heavy scope listed");
    let light = table.find("cs light").expect("light scope listed");
    assert!(
        heavy < light,
        "the slow dispatch is the first row:\n{table}"
    );
    assert!(table.contains("3 lists read"), "{table}");
}

#[test]
#[ignore = "probe"]
fn many_heaps() {
    unsafe { std::env::set_var("VICTORY_METAL_GPUPROF", "1") };
    let d = Device::new().unwrap();
    let q = Queue::new(&d).unwrap();
    let mut keep = Vec::new();
    for i in 0..200 {
        let l = q.begin_list().unwrap();
        keep.push(l);
        if i % 20 == 19 {
            eprintln!("lists {}", i + 1);
        }
    }
}
