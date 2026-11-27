//! The proto interpreter, from the outside: hand-written command streams and
//! sync calls through an `InProcess` link, on the real GPU, compared with a
//! CPU reference. Nothing here talks to the executor core directly except
//! for the hooks the interpreter documents (`create_compute_pipeline_from_msl`,
//! `set_tlab_layout`, `tlab_layout`, `device`).
//!
//! The kernels are MSL written the way Metal Shader Converter output reads
//! its top-level argument buffer (a struct at argument-table slot 2 holding
//! constants and pointers; descriptor-table entries are `{va, texture, metadata}`).
#![cfg(target_vendor = "apple")]

mod exec_support;

use exec_support::*;
use victory_metal_proto::cmd::*;
use victory_metal_proto::ids::*;
use victory_metal_proto::root::*;
use victory_metal_proto::status::Status;
use victory_metal_proto::types::*;

const TABLE_KERNELS: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
struct Entry { ulong va; ulong tex; ulong md; };\n\
struct Tlab { uint n; float scale; ulong cbv; ulong table; };\n\
struct TlabCf { uint n; float scale; ulong table; ulong cbv; };\n\
kernel void scale_add(constant Tlab& t [[buffer(2)]], uint i [[thread_position_in_grid]]) {\n\
    device const Entry* e = (device const Entry*)t.table;\n\
    device const uint* src = (device const uint*)e[0].va;\n\
    device uint* out = (device uint*)e[1].va;\n\
    uint bias = *(device const uint*)t.cbv;\n\
    if (i < t.n) out[i] = (uint)((float)src[i] * t.scale) + bias;\n}\n\
kernel void scale_add_cf(constant TlabCf& t [[buffer(2)]], uint i [[thread_position_in_grid]]) {\n\
    device const Entry* e = (device const Entry*)t.table;\n\
    device const uint* src = (device const uint*)e[0].va;\n\
    device uint* out = (device uint*)e[1].va;\n\
    uint bias = *(device const uint*)t.cbv;\n\
    if (i < t.n) out[i] = (uint)((float)src[i] * t.scale) + bias;\n}\n";

/// Slow kernels over root descriptors: `slow_a` (CBV in, UAV out, 1 constant)
/// and `plus_one` (SRV in, UAV out).
const SLOW_KERNELS: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
struct SlowA { ulong in; ulong out; uint iters; };\n\
struct PlusOne { ulong in; ulong out; };\n\
kernel void slow_a(constant SlowA& t [[buffer(2)]]) {\n\
    uint x = *(device const uint*)t.in;\n\
    for (uint i = 0; i < t.iters; i++) { x = x * 1664525u + 1013904223u; }\n\
    *(device uint*)t.out = x;\n}\n\
kernel void plus_one(constant PlusOne& t [[buffer(2)]]) {\n\
    *(device uint*)t.out = *(device const uint*)t.in + 1u;\n}\n";

const ITERS: u32 = 40_000_000;

fn lcg(seed: u32, iters: u32) -> u32 {
    let mut x = seed;
    for _ in 0..iters {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    }
    x
}

/// `[CONSTANTS 2, CBV, TABLE(SRV, UAV)]`: the layout `TABLE_KERNELS::scale_add`
/// reads (constants at 0 and 4, CBV pointer at 8, table pointer at 16).
fn scale_add_sig() -> RootSigDesc {
    RootSigDesc {
        params: vec![
            constants_param(2),
            descriptor_param(RootParamKind::CBV),
            table_param(0, 2),
        ],
        ranges: vec![range(RangeKind::SRV), range(RangeKind::UAV)],
        ..RootSigDesc::default()
    }
}

/// Everything the scale-add scenarios use.
struct Scene {
    fx: Fx,
    queue: QueueId,
    fence: FenceId,
    out: CreateBufferArgs,
    readback: CreateBufferArgs,
    cbv: CreateBufferArgs,
    heap: CreateDescHeapArgs,
    sig: RootSigId,
}

const N: usize = 1024;
const BIAS: u32 = 7;

fn src_words() -> Vec<u32> {
    (0..N as u32).map(|i| i * 3).collect()
}

fn scene() -> Scene {
    let fx = Fx::new();
    let queue = fx.queue();
    let fence = fx.fence(0);
    let src = fx.buffer(BufferKind::UPLOAD, 4096);
    let out = fx.buffer(BufferKind::DEFAULT, 4096);
    let readback = fx.buffer(BufferKind::READBACK, 4096);
    let cbv = fx.buffer(BufferKind::SHARED, 256);
    cpu_write(src.cpu_addr, &src_words());
    cpu_write(cbv.cpu_addr, &[BIAS]);
    // A staging heap (not shader visible) holds the two views; they are copied
    // to slots 4 and 5 of the shader-visible heap.
    let staging = fx.heap(HeapKind::CBV_SRV_UAV, 8, false);
    let heap = fx.heap(HeapKind::CBV_SRV_UAV, 16, true);
    fx.write_view(
        staging.heap,
        0,
        ViewDesc::raw(ViewKind::BUFFER_SRV, src.res, 0, N as u32),
    );
    fx.write_view(
        staging.heap,
        1,
        ViewDesc::raw(ViewKind::BUFFER_UAV, out.res, 0, N as u32),
    );
    fx.copy_descriptors(heap.heap, 4, staging.heap, 0, 2);
    let sig = fx.root_sig(&scale_add_sig());
    Scene {
        fx,
        queue,
        fence,
        out,
        readback,
        cbv,
        heap,
        sig,
    }
}

impl Scene {
    fn pso(&self, entry: &str) -> PsoId {
        self.fx
            .executor()
            .create_compute_pipeline_from_msl(self.sig, TABLE_KERNELS, entry, [64, 1, 1])
            .expect("pipeline")
    }

    /// One list: the bindings, one dispatch of `N` threads, copy `out` to
    /// the readback buffer, signal.
    fn run(&self, pso: PsoId, scale: f32, signal: u64) {
        let mut c = vec![
            begin(1, self.queue),
            Command::SetRootSignature(SetRootSignature {
                bind_point: BindPoint::COMPUTE,
                root_sig: self.sig,
            }),
            Command::SetPipeline(SetPipeline { pso, _pad: 0 }),
            Command::SetDescriptorHeaps(SetDescriptorHeaps {
                cbv_srv_uav: self.heap.heap,
                sampler: HeapId::NULL,
            }),
        ];
        let mut consts = [0u8; 8];
        consts[..4].copy_from_slice(&(N as u32).to_le_bytes());
        consts[4..].copy_from_slice(&scale.to_bits().to_le_bytes());
        c.push(Command::SetRootConstants(SetRootConstants {
            bind_point: BindPoint::COMPUTE,
            root_index: 0,
            dest_offset: 0,
            data: &consts,
        }));
        c.push(Command::SetRootDescriptor(SetRootDescriptor {
            bind_point: BindPoint::COMPUTE,
            root_index: 1,
            kind: RootParamKind::CBV,
            _pad: 0,
            gpu_va: self.cbv.gpu_va,
        }));
        c.push(Command::SetRootTable(SetRootTable {
            bind_point: BindPoint::COMPUTE,
            root_index: 2,
            heap: self.heap.heap,
            index: 4,
        }));
        c.push(dispatch(N as u32 / 64, 1, 1));
        c.push(Command::Barrier(Barrier::memory(
            stage::DISPATCH,
            stage::BLIT,
        )));
        c.push(copy(self.readback.res, self.out.res, 4096));
        c.push(end(1));
        c.push(signal_cmd(self.queue, self.fence, signal));
        assert_eq!(self.fx.submit(&c), Status::OK);
        assert_eq!(self.fx.wait(self.fence, signal), Status::OK);
    }

    fn readback(&self) -> Vec<u32> {
        cpu_read(self.readback.cpu_addr, N)
    }
}

fn expect(scale: f32) -> Vec<u32> {
    src_words()
        .iter()
        .map(|&v| (v as f32 * scale) as u32 + BIAS)
        .collect()
}

#[test]
fn the_scenario_matches_the_cpu() {
    let s = scene();
    let pso = s.pso("scale_add");
    s.run(pso, 2.0, 1);
    assert_eq!(s.readback(), expect(2.0));
    // The same list again with other constants: the root arguments of the
    // new list are set from scratch, nothing leaks over from the last one.
    s.run(pso, 0.5, 2);
    assert_eq!(s.readback(), expect(0.5));
    // The queue's work is done and the fence says so.
    assert_eq!(s.fx.fence_value(s.fence), 2);
}

#[test]
fn root_arguments_persist_inside_a_list_and_update_in_place() {
    // Two dispatches in one list; between them only one root constant
    // changes. The other constant, the CBV and the table must survive.
    let s = scene();
    let pso = s.pso("scale_add");
    let q = s.queue;
    let mut first = [0u8; 8];
    first[..4].copy_from_slice(&(N as u32).to_le_bytes());
    first[4..].copy_from_slice(&2.0f32.to_bits().to_le_bytes());
    let second = 4.0f32.to_bits().to_le_bytes();
    let cmds = vec![
        begin(1, q),
        Command::SetRootSignature(SetRootSignature {
            bind_point: BindPoint::COMPUTE,
            root_sig: s.sig,
        }),
        Command::SetPipeline(SetPipeline { pso, _pad: 0 }),
        Command::SetRootConstants(SetRootConstants {
            bind_point: BindPoint::COMPUTE,
            root_index: 0,
            dest_offset: 0,
            data: &first,
        }),
        Command::SetRootDescriptor(SetRootDescriptor {
            bind_point: BindPoint::COMPUTE,
            root_index: 1,
            kind: RootParamKind::CBV,
            _pad: 0,
            gpu_va: s.cbv.gpu_va,
        }),
        Command::SetRootTable(SetRootTable {
            bind_point: BindPoint::COMPUTE,
            root_index: 2,
            heap: s.heap.heap,
            index: 4,
        }),
        dispatch(16, 1, 1),
        Command::Barrier(Barrier::memory(stage::DISPATCH, stage::DISPATCH)),
        // Only constant 1 (the scale) changes.
        Command::SetRootConstants(SetRootConstants {
            bind_point: BindPoint::COMPUTE,
            root_index: 0,
            dest_offset: 1,
            data: &second,
        }),
        dispatch(16, 1, 1),
        Command::Barrier(Barrier::memory(stage::DISPATCH, stage::BLIT)),
        copy(s.readback.res, s.out.res, 4096),
        end(1),
        signal_cmd(q, s.fence, 1),
    ];
    assert_eq!(s.fx.submit(&cmds), Status::OK);
    assert_eq!(s.fx.wait(s.fence, 1), Status::OK);
    assert_eq!(s.readback(), expect(4.0));
}

#[test]
fn a_reflection_layout_replaces_the_rule_based_one() {
    // `[TABLE, CBV, CONSTANTS 2]`: by the rules table@0, CBV@8, constants@16;
    // what MSC reflection might say instead: constants@0, table@8, CBV@16.
    let s = scene();
    let fx = &s.fx;
    let desc = RootSigDesc {
        params: vec![
            table_param(0, 2),
            descriptor_param(RootParamKind::CBV),
            constants_param(2),
        ],
        ranges: vec![range(RangeKind::SRV), range(RangeKind::UAV)],
        ..RootSigDesc::default()
    };
    let sig = fx.root_sig(&desc);
    let ex = fx.executor();
    let rule = ex.tlab_layout(sig).unwrap();
    assert_eq!(
        (0..3)
            .map(|i| rule.slot(i).unwrap().offset)
            .collect::<Vec<_>>(),
        [0, 8, 16]
    );
    assert_eq!(rule.size(), 24);
    // A pipeline made before the override keeps the old layout: using it with
    // the new one is refused rather than run against the wrong offsets.
    let before = ex
        .create_compute_pipeline_from_msl(sig, TABLE_KERNELS, "scale_add", [64, 1, 1])
        .unwrap();
    // Bad overrides are refused: misaligned, overlapping, outside, wrong count.
    for bad in [[4u32, 8, 16], [0, 8, 12], [0, 8, 24], [0, 8, 0]] {
        assert_eq!(
            ex.set_tlab_layout(sig, &bad, 24),
            Err(Status::INVALID_ARG),
            "{bad:?}"
        );
    }
    assert_eq!(
        ex.set_tlab_layout(sig, &[8, 16], 24),
        Err(Status::INVALID_ARG)
    );
    assert_eq!(
        ex.set_tlab_layout(RootSigId(99), &[8, 16, 0], 24),
        Err(Status::NOT_FOUND)
    );
    ex.set_tlab_layout(sig, &[8, 16, 0], 24).unwrap();
    let cf = ex.tlab_layout(sig).unwrap();
    assert!(cf.from_reflection_data());
    let pso = ex
        .create_compute_pipeline_from_msl(sig, TABLE_KERNELS, "scale_add_cf", [64, 1, 1])
        .unwrap();
    let mut consts = [0u8; 8];
    consts[..4].copy_from_slice(&(N as u32).to_le_bytes());
    consts[4..].copy_from_slice(&3.0f32.to_bits().to_le_bytes());
    let body = |pso: PsoId, fence_value: u64| {
        vec![
            begin(1, s.queue),
            Command::SetRootSignature(SetRootSignature {
                bind_point: BindPoint::COMPUTE,
                root_sig: sig,
            }),
            Command::SetPipeline(SetPipeline { pso, _pad: 0 }),
            Command::SetRootConstants(SetRootConstants {
                bind_point: BindPoint::COMPUTE,
                root_index: 2,
                dest_offset: 0,
                data: &consts,
            }),
            Command::SetRootDescriptor(SetRootDescriptor {
                bind_point: BindPoint::COMPUTE,
                root_index: 1,
                kind: RootParamKind::CBV,
                _pad: 0,
                gpu_va: s.cbv.gpu_va,
            }),
            Command::SetRootTable(SetRootTable {
                bind_point: BindPoint::COMPUTE,
                root_index: 0,
                heap: s.heap.heap,
                index: 4,
            }),
            dispatch(16, 1, 1),
            Command::Barrier(Barrier::memory(stage::DISPATCH, stage::BLIT)),
            copy(s.readback.res, s.out.res, 4096),
            end(1),
            signal_cmd(s.queue, s.fence, fence_value),
        ]
    };
    // The failed block still gives its fence signal (nothing may wait for ever for it), so the
    // second block signals the next value.
    assert_eq!(fx.submit(&body(before, 1)), Status::INVALID_ARG);
    assert_eq!(fx.submit(&body(pso, 2)), Status::OK);
    assert_eq!(fx.wait(s.fence, 2), Status::OK);
    assert_eq!(s.readback(), expect(3.0));
}

#[test]
fn two_submits_are_ordered_without_the_cpu_waiting_in_between() {
    // The first block's kernel is slow, the second block's depends on it and
    // is submitted at once. Without the inter-submit barrier the second one
    // runs on stale data every time (see the observational tests in gpu.rs).
    let fx = Fx::new();
    let q = fx.queue();
    let fence = fx.fence(0);
    let input = fx.buffer(BufferKind::SHARED, 256);
    let mid = fx.buffer(BufferKind::SHARED, 256);
    let out = fx.buffer(BufferKind::SHARED, 256);
    let heap = fx.heap(HeapKind::CBV_SRV_UAV, 4, true);
    let _ = heap;
    let sig_a = fx.root_sig(&RootSigDesc {
        params: vec![
            descriptor_param(RootParamKind::CBV),
            descriptor_param(RootParamKind::UAV),
            constants_param(1),
        ],
        ..RootSigDesc::default()
    });
    let sig_b = fx.root_sig(&RootSigDesc {
        params: vec![
            descriptor_param(RootParamKind::SRV),
            descriptor_param(RootParamKind::UAV),
        ],
        ..RootSigDesc::default()
    });
    let ex = fx.executor();
    let pso_a = ex
        .create_compute_pipeline_from_msl(sig_a, SLOW_KERNELS, "slow_a", [1, 1, 1])
        .unwrap();
    let pso_b = ex
        .create_compute_pipeline_from_msl(sig_b, SLOW_KERNELS, "plus_one", [1, 1, 1])
        .unwrap();
    cpu_write(input.cpu_addr, &[12345]);
    let want = lcg(12345, ITERS).wrapping_add(1);
    let iters = ITERS.to_le_bytes();
    for round in 0..5u64 {
        cpu_write(mid.cpu_addr, &[0]);
        cpu_write(out.cpu_addr, &[0]);
        let a = vec![
            begin(1, q),
            Command::SetRootSignature(SetRootSignature {
                bind_point: BindPoint::COMPUTE,
                root_sig: sig_a,
            }),
            Command::SetPipeline(SetPipeline {
                pso: pso_a,
                _pad: 0,
            }),
            root_va(0, RootParamKind::CBV, input.gpu_va),
            root_va(1, RootParamKind::UAV, mid.gpu_va),
            Command::SetRootConstants(SetRootConstants {
                bind_point: BindPoint::COMPUTE,
                root_index: 2,
                dest_offset: 0,
                data: &iters,
            }),
            dispatch(1, 1, 1),
            end(1),
        ];
        let b = vec![
            begin(1, q),
            Command::SetRootSignature(SetRootSignature {
                bind_point: BindPoint::COMPUTE,
                root_sig: sig_b,
            }),
            Command::SetPipeline(SetPipeline {
                pso: pso_b,
                _pad: 0,
            }),
            root_va(0, RootParamKind::SRV, mid.gpu_va),
            root_va(1, RootParamKind::UAV, out.gpu_va),
            dispatch(1, 1, 1),
            end(1),
            signal_cmd(q, fence, round + 1),
        ];
        assert_eq!(fx.submit(&a), Status::OK);
        assert_eq!(fx.submit(&b), Status::OK);
        assert_eq!(fx.wait(fence, round + 1), Status::OK);
        assert_eq!(cpu_read(out.cpu_addr, 1)[0], want, "round {round}");
    }
}

#[test]
fn lists_of_one_block_are_ordered_too() {
    // Two lists in one block are one commit; the second reads what the first wrote.
    let fx = Fx::new();
    let q = fx.queue();
    let fence = fx.fence(0);
    let input = fx.buffer(BufferKind::SHARED, 256);
    let mid = fx.buffer(BufferKind::SHARED, 256);
    let out = fx.buffer(BufferKind::SHARED, 256);
    let sig_a = fx.root_sig(&RootSigDesc {
        params: vec![
            descriptor_param(RootParamKind::CBV),
            descriptor_param(RootParamKind::UAV),
            constants_param(1),
        ],
        ..RootSigDesc::default()
    });
    let sig_b = fx.root_sig(&RootSigDesc {
        params: vec![
            descriptor_param(RootParamKind::SRV),
            descriptor_param(RootParamKind::UAV),
        ],
        ..RootSigDesc::default()
    });
    let ex = fx.executor();
    let pso_a = ex
        .create_compute_pipeline_from_msl(sig_a, SLOW_KERNELS, "slow_a", [1, 1, 1])
        .unwrap();
    let pso_b = ex
        .create_compute_pipeline_from_msl(sig_b, SLOW_KERNELS, "plus_one", [1, 1, 1])
        .unwrap();
    cpu_write(input.cpu_addr, &[777]);
    let want = lcg(777, ITERS).wrapping_add(1);
    let iters = ITERS.to_le_bytes();
    for round in 0..3u64 {
        cpu_write(mid.cpu_addr, &[0]);
        cpu_write(out.cpu_addr, &[0]);
        let cmds = vec![
            begin(1, q),
            Command::SetRootSignature(SetRootSignature {
                bind_point: BindPoint::COMPUTE,
                root_sig: sig_a,
            }),
            Command::SetPipeline(SetPipeline {
                pso: pso_a,
                _pad: 0,
            }),
            root_va(0, RootParamKind::CBV, input.gpu_va),
            root_va(1, RootParamKind::UAV, mid.gpu_va),
            Command::SetRootConstants(SetRootConstants {
                bind_point: BindPoint::COMPUTE,
                root_index: 2,
                dest_offset: 0,
                data: &iters,
            }),
            dispatch(1, 1, 1),
            end(1),
            begin(2, q),
            Command::SetRootSignature(SetRootSignature {
                bind_point: BindPoint::COMPUTE,
                root_sig: sig_b,
            }),
            Command::SetPipeline(SetPipeline {
                pso: pso_b,
                _pad: 0,
            }),
            root_va(0, RootParamKind::SRV, mid.gpu_va),
            root_va(1, RootParamKind::UAV, out.gpu_va),
            dispatch(1, 1, 1),
            end(2),
            signal_cmd(q, fence, round + 1),
        ];
        assert_eq!(fx.submit(&cmds), Status::OK);
        assert_eq!(fx.wait(fence, round + 1), Status::OK);
        assert_eq!(cpu_read(out.cpu_addr, 1)[0], want, "round {round}");
    }
}
