//! Regressions for results that went wrong with no error (victory-gpu, found
//! 2026-11-27): a pipeline run under a root signature that is another object
//! with the same description, and a copy that follows a chain of two barriers.
#![cfg(target_vendor = "apple")]

mod exec_support;

use exec_support::*;
use victory_metal_proto::cmd::*;
use victory_metal_proto::ids::*;
use victory_metal_proto::root::*;
use victory_metal_proto::status::Status;
use victory_metal_proto::types::*;

const KERNELS: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
struct SlowA { ulong in; ulong out; uint iters; };\n\
struct PlusOne { ulong in; ulong out; };\n\
kernel void slow_a(constant SlowA& t [[buffer(2)]]) {\n\
    uint x = *(device const uint*)t.in;\n\
    for (uint i = 0; i < t.iters; i++) { x = x * 1664525u + 1013904223u; }\n\
    *(device uint*)t.out = x;\n}\n\
kernel void plus_one(constant PlusOne& t [[buffer(2)]]) {\n\
    *(device uint*)t.out = *(device const uint*)t.in + 1u;\n}\n";

fn srv_uav_sig() -> RootSigDesc {
    RootSigDesc {
        params: vec![
            descriptor_param(RootParamKind::SRV),
            descriptor_param(RootParamKind::UAV),
        ],
        ..RootSigDesc::default()
    }
}

#[test]
fn a_pipeline_runs_under_an_identical_root_signature_made_separately() {
    let fx = Fx::new();
    let queue = fx.queue();
    let fence = fx.fence(0);
    let input = fx.buffer(BufferKind::SHARED, 256);
    let out = fx.buffer(BufferKind::SHARED, 256);
    cpu_write(input.cpu_addr, &[41]);
    cpu_write(out.cpu_addr, &[0]);
    // D3D12 makes two root signature objects of the same bytes (the pipeline's
    // and the list's); they are compatible. The pipeline belongs to the first.
    let made_with = fx.root_sig(&srv_uav_sig());
    let bound = fx.root_sig(&srv_uav_sig());
    assert_ne!(made_with, bound);
    let pso = fx
        .executor()
        .create_compute_pipeline_from_msl(made_with, KERNELS, "plus_one", [1, 1, 1])
        .unwrap();
    let run = |sig: RootSigId, signal: u64| {
        let s = fx.submit(&[
            begin(1, queue),
            set_sig(sig),
            Command::SetPipeline(SetPipeline { pso, _pad: 0 }),
            root_va(0, RootParamKind::SRV, input.gpu_va),
            root_va(1, RootParamKind::UAV, out.gpu_va),
            dispatch(1, 1, 1),
            end(1),
            signal_cmd(queue, fence, signal),
        ]);
        if s == Status::OK {
            assert_eq!(fx.wait(fence, signal), Status::OK);
        }
        s
    };
    assert_eq!(run(bound, 1), Status::OK);
    assert_eq!(cpu_read(out.cpu_addr, 1), [42]);
    assert_eq!(run(made_with, 2), Status::OK);

    // A signature that differs is still refused, with the command named.
    let other = fx.root_sig(&RootSigDesc {
        params: vec![
            descriptor_param(RootParamKind::SRV),
            descriptor_param(RootParamKind::UAV),
            constants_param(1),
        ],
        ..RootSigDesc::default()
    });
    cpu_write(out.cpu_addr, &[0]);
    assert_eq!(run(other, 3), Status::INVALID_ARG);
    assert_eq!(cpu_read(out.cpu_addr, 1), [0]);
    let why = fx.executor().last_error().expect("the stop is explained");
    assert!(why.contains("Dispatch"), "{why}");
}

/// The chain the front sends for `UNORDERED_ACCESS -> INDIRECT_ARGUMENT ->
/// COPY_SOURCE`: the writer's stages, then the same writer stages again
/// against the blit. The copy must see the shader's result.
#[test]
fn a_copy_after_two_chained_barriers_sees_the_shader_write() {
    let fx = Fx::new();
    let queue = fx.queue();
    let fence = fx.fence(0);
    let sig = fx.root_sig(&RootSigDesc {
        params: vec![
            descriptor_param(RootParamKind::CBV),
            descriptor_param(RootParamKind::UAV),
            RootParam {
                kind: RootParamKind::CONSTANTS,
                num_32bit_values: 1,
                ..RootParam::default()
            },
        ],
        ..RootSigDesc::default()
    });
    // Layout (MSC or rule): the kernel reads `{in, out, iters}`; constants
    // after pointers matches what the converter gives for this signature when
    // its reflection says so, so the test asks the back.
    let layout = fx.executor().tlab_layout(sig).unwrap();
    let kernels = if layout.slot(2).map(|s| s.offset) == Some(16) {
        KERNELS.to_string()
    } else {
        // Constants first: iters at 0, pointers at 8 and 16.
        KERNELS.replace(
            "struct SlowA { ulong in; ulong out; uint iters; };",
            "struct SlowA { uint iters; ulong in; ulong out; };",
        )
    };
    let pso = fx
        .executor()
        .create_compute_pipeline_from_msl(sig, &kernels, "slow_a", [1, 1, 1])
        .unwrap();
    let input = fx.buffer(BufferKind::SHARED, 256);
    let out = fx.buffer(BufferKind::DEFAULT, 256);
    let readback = fx.buffer(BufferKind::READBACK, 256);
    let iters = 20_000_000u32;
    for round in 0..8u32 {
        let seed = 1000 + round;
        cpu_write(input.cpu_addr, &[seed]);
        cpu_write(readback.cpu_addr, &[0xDEAD_BEEF]);
        let consts = iters.to_le_bytes();
        let signal = u64::from(round) + 1;
        let s = fx.submit(&[
            begin(1, queue),
            set_sig(sig),
            Command::SetPipeline(SetPipeline { pso, _pad: 0 }),
            root_va(0, RootParamKind::CBV, input.gpu_va),
            root_va(1, RootParamKind::UAV, out.gpu_va),
            Command::SetRootConstants(SetRootConstants {
                bind_point: BindPoint::COMPUTE,
                root_index: 2,
                dest_offset: 0,
                data: &consts,
            }),
            dispatch(1, 1, 1),
            // UAV -> INDIRECT_ARGUMENT: writers' stages to the indirect stages.
            Command::Barrier(Barrier::memory(
                stage::DISPATCH | stage::VERTEX | stage::FRAGMENT | stage::OBJECT | stage::MESH,
                stage::DISPATCH | stage::VERTEX | stage::OBJECT | stage::MESH,
            )),
            // INDIRECT_ARGUMENT -> COPY_SOURCE: still the writers' stages.
            Command::Barrier(Barrier::memory(
                stage::DISPATCH | stage::VERTEX | stage::FRAGMENT | stage::OBJECT | stage::MESH,
                stage::BLIT,
            )),
            copy(readback.res, out.res, 256),
            end(1),
            signal_cmd(queue, fence, signal),
        ]);
        assert_eq!(s, Status::OK);
        assert_eq!(fx.wait(fence, signal), Status::OK);
        let mut x = seed;
        for _ in 0..iters {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        }
        assert_eq!(cpu_read(readback.cpu_addr, 1), [x], "round {round}");
    }
}
