//! `VICTORY_METAL_GPUPROF`: every sample a list takes is written, in order, also when the slices
//! of the counter heaps are used again and again. Own process: the switch is read once.
#![cfg(target_vendor = "apple")]

use victory_metal_back::device::Device;
use victory_metal_back::memory::BufferKind;
use victory_metal_back::queue::Queue;

#[test]
fn every_sample_is_written_in_order_when_slices_are_reused() {
    // SAFETY: the only test of this process.
    unsafe { std::env::set_var("VICTORY_METAL_GPUPROF", "1") };
    if std::env::var_os("PROF_RELAXED").is_some() {
        // SAFETY: as above.
        unsafe { std::env::set_var("VICTORY_METAL_GPUPROF_RELAXED", "1") };
    }
    let d = Device::new().unwrap();
    let a = d.create_buffer(BufferKind::Default, 1 << 20).unwrap();
    let b = d.create_buffer(BufferKind::Default, 1 << 20).unwrap();
    let nop = d
        .compute_pipeline_from_msl(
            "#include <metal_stdlib>\nusing namespace metal;\nkernel void nop(device uint* o [[buffer(0)]], uint i [[thread_position_in_grid]]) { o[i] = o[i] + 1u; }\n",
            "nop",
        )
        .unwrap();
    let q = Queue::new(&d).unwrap();
    let mut last = 0;
    for _ in 0..200 {
        let mut l = q.begin_list().unwrap();
        l.set_address(0, a.gpu_va()).unwrap();
        for i in 0..400u64 {
            let mix = std::env::var_os("PROF_MIX").is_some();
            if std::env::var_os("PROF_COPY").is_none() && !(mix && i % 2 == 1) {
                l.dispatch(&nop, [1024, 1, 1], [64, 1, 1]).unwrap();
            } else {
                l.copy_buffer(&b, 0, &a, 0, 65536 + i).unwrap();
            }
            l.prof_sample(|| "op".into());
        }
        l.end().unwrap();
        last = q.submit(vec![l]).unwrap();
    }
    q.wait_for(last, 60_000).unwrap();
    let table = victory_metal_back::prof::flush_table();
    println!("{table}");
    assert!(!table.contains("never written"), "{table}");
    assert!(!table.contains("out of order"), "{table}");
}
