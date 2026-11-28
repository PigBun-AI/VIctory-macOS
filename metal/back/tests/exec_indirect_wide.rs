//! An indirect dispatch wider than D3D12's 65,535 groups in x runs every group on Metal (the title's tile
//! lists are 67,440 and 47,760 groups at 2560x1440; the back puts the record to
//! `dispatchThreadgroupsWithIndirectBuffer` as it is, and Metal takes 32-bit counts).
#![cfg(target_vendor = "apple")]

mod exec_support;

use exec_support::*;
use victory_metal_proto::cmd::*;
use victory_metal_proto::ids::*;
use victory_metal_proto::indirect::*;
use victory_metal_proto::root::*;
use victory_metal_proto::status::Status;
use victory_metal_proto::types::*;
use victory_metal_proto::wire::Wire;

const KERNELS: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
struct T { ulong out; };\n\
kernel void count(constant T& t [[buffer(2)]], uint3 g [[threadgroup_position_in_grid]]) {\n\
    device atomic_uint* o = (device atomic_uint*)t.out;\n\
    atomic_fetch_add_explicit(o, 1u, memory_order_relaxed);\n\
    atomic_fetch_max_explicit(o + 1, g.x, memory_order_relaxed);\n}\n";

#[test]
fn an_indirect_dispatch_of_more_than_65535_groups_runs_them_all() {
    let fx = Fx::new();
    let queue = fx.queue();
    let fence = fx.fence(0);
    let sig = fx.root_sig(&RootSigDesc {
        params: vec![descriptor_param(RootParamKind::UAV)],
        ..RootSigDesc::default()
    });
    let pso = fx
        .executor()
        .create_compute_pipeline_from_msl(sig, KERNELS, "count", [1, 1, 1])
        .unwrap();
    let out = fx.buffer(BufferKind::SHARED, 4096);
    let args = fx.buffer(BufferKind::SHARED, 4096);
    let signature = {
        let mut v = Vec::new();
        IndirectArg {
            kind: IndirectArgKind::DISPATCH,
            index: 0,
            dest_offset: 0,
            count: 0,
        }
        .put(&mut v);
        v
    };
    for (n, want) in [
        (65_535u32, 65_535u32),
        (67_440, 67_440),
        (1 << 20, 1 << 20),
        (3_000_000, 3_000_000),
    ] {
        cpu_write(out.cpu_addr, &[0; 2]);
        cpu_write(args.cpu_addr, &[n, 1, 1, 0]);
        let cmds = vec![
            begin(1, queue),
            set_sig(sig),
            Command::SetPipeline(SetPipeline { pso, _pad: 0 }),
            root_va(0, RootParamKind::UAV, out.gpu_va),
            Command::ExecuteIndirect(ExecuteIndirect {
                args: args.res,
                args_offset: 0,
                count: ResId::NULL,
                count_offset: 0,
                max_count: 1,
                stride: 16,
                root_sig: RootSigId::NULL,
                signature: &signature,
            }),
            end(1),
            signal_cmd(queue, fence, u64::from(n)),
        ];
        assert_eq!(fx.submit(&cmds), Status::OK);
        assert_eq!(fx.wait(fence, u64::from(n)), Status::OK);
        let got = cpu_read(out.cpu_addr, 2);
        assert_eq!(got[0], want, "{n} groups asked: groups that ran");
        assert_eq!(got[1], n - 1, "{n} groups asked: the highest group number");
    }
}
