//! `CREATE_COMPUTE_PIPELINE` end to end through the proto interpreter: DXIL
//! (HLSL compiled by the repository's DXC front end, see `common_msc.rs`;
//! `SKIPPED` without one) + a proto root signature -> Metal Shader Converter ->
//! pipeline, then a command stream runs it on the GPU and the result is
//! compared with a CPU reference. The thread-group size comes from the
//! shader's reflection (nothing passes it), and the argument-buffer layout
//! from MSC's reflection of the root signature.
#![cfg(target_vendor = "apple")]

mod common_msc;
mod exec_support;

use common_msc::dxil_or_skip;
use exec_support::*;
use victory_metal_proto::cmd::*;
use victory_metal_proto::ids::*;
use victory_metal_proto::root::*;
use victory_metal_proto::status::Status;
use victory_metal_proto::sync::CreateComputePipelineArgs;
use victory_metal_proto::types::*;

fn pipeline(fx: &Fx, sig: RootSigId, dxil: &[u8]) -> PsoId {
    let mut a = CreateComputePipelineArgs::new();
    a.root_sig = sig;
    a.dxil_addr = dxil.as_ptr() as u64;
    a.dxil_len = dxil.len() as u64;
    let (s, a) = fx.call(a);
    assert_eq!(s, Status::OK, "{:?}", fx.executor().last_error());
    a.pso
}

fn set_up<'a>(sig: RootSigId, pso: PsoId, q: QueueId) -> Vec<Command<'a>> {
    vec![
        begin(1, q),
        set_sig(sig),
        Command::SetPipeline(SetPipeline { pso, _pad: 0 }),
    ]
}

fn finish(fx: &Fx, q: QueueId, mut cmds: Vec<Command<'_>>) {
    cmds.push(end(1));
    let f = fx.fence(0);
    cmds.push(signal_cmd(q, f, 1));
    assert_eq!(fx.submit(&cmds), Status::OK);
    assert_eq!(fx.wait(f, 1), Status::OK);
}

fn constants(root_index: u32, data: &[u8]) -> Command<'_> {
    Command::SetRootConstants(SetRootConstants {
        bind_point: BindPoint::COMPUTE,
        root_index,
        dest_offset: 0,
        data,
    })
}

const ROOT_UAV_CONST: &str = "cbuffer c : register(b0) { uint k; }\n\
RWByteAddressBuffer o : register(u0);\n\
[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) { o.Store(t.x*4, t.x*3u + k); }\n";

/// One root constant (12 bytes of padding follow it) and a root UAV: the UAV
/// sits where MSC says (offset 8), not where a naive packing would put it.
#[test]
fn a_root_uav_and_a_root_constant() {
    let Some(dxil) = dxil_or_skip("exec_root_uav", "cs_6_6", ROOT_UAV_CONST) else {
        return;
    };
    let fx = Fx::new();
    let q = fx.queue();
    let sig = fx.root_sig(&RootSigDesc {
        params: vec![constants_param(1), descriptor_param(RootParamKind::UAV)],
        ..RootSigDesc::default()
    });
    let lay = fx.executor().tlab_layout(sig).unwrap();
    assert!(lay.from_reflection_data() || lay.slot(1).unwrap().offset == 8);
    let pso = pipeline(&fx, sig, &dxil);
    let out = fx.buffer(BufferKind::SHARED, 4096);
    cpu_write(out.cpu_addr, &[0xdead_beef; 256]);
    let mut c = set_up(sig, pso, q);
    let k = 1000u32.to_le_bytes();
    c.push(constants(0, &k));
    c.push(root_va(1, RootParamKind::UAV, out.gpu_va));
    c.push(dispatch(4, 1, 1));
    finish(&fx, q, c);
    let want: Vec<u32> = (0..128u32).map(|t| t * 3 + 1000).collect();
    assert_eq!(cpu_read(out.cpu_addr, 128), want);
    assert_eq!(
        cpu_read(out.cpu_addr, 129)[128],
        0xdead_beef,
        "no write past the grid"
    );
}

/// GPU readback, not just descriptor bytes: both forms of b0 must preserve
/// a 16-byte lane offset, including the two sides of a 256-byte boundary.
#[test]
fn cbv_suballocation_offsets_survive_table_and_root_binding() {
    let shader = "cbuffer C : register(b0) { uint4 sentinel; };\n\
                  RWByteAddressBuffer dst : register(u0);\n\
                  [numthreads(1,1,1)] void main() { dst.Store4(0, sentinel); }";
    let Some(dxil) = dxil_or_skip("exec_cbv_suballocation", "cs_6_6", shader) else {
        return;
    };
    let fx = Fx::new();
    let q = fx.queue();
    let src = fx.buffer(BufferKind::SHARED, 512);
    let out = fx.buffer(BufferKind::SHARED, 48);
    assert_eq!(src.gpu_va % 256, 0, "aligned control must be aligned");
    let offsets = [16u64, 240, 256];
    let expected: [u32; 12] = std::array::from_fn(|i| 0xa511_0001 + i as u32);
    cpu_write(src.cpu_addr, &[0xabad_1dea; 128]);
    let heap = fx.heap(HeapKind::CBV_SRV_UAV, 3, true);
    for (i, offset) in offsets.into_iter().enumerate() {
        cpu_write(src.cpu_addr + offset, &expected[i * 4..i * 4 + 4]);
        fx.write_view(heap.heap, i as u32, ViewDesc::cbv(src.gpu_va + offset, 16));
    }
    for table in [true, false] {
        let mut sig = RootSigDesc::default();
        if table {
            sig.params.push(table_param(0, 1));
            sig.ranges.push(DescriptorRange {
                num_descriptors: 1,
                ..range(RangeKind::CBV)
            });
        } else {
            sig.params.push(descriptor_param(RootParamKind::CBV));
        }
        sig.params.push(descriptor_param(RootParamKind::UAV));
        let sig = fx.root_sig(&sig);
        let mut c = set_up(sig, pipeline(&fx, sig, &dxil), q);
        cpu_write(out.cpu_addr, &[0xdead_beef; 12]);
        if table {
            c.push(Command::SetDescriptorHeaps(SetDescriptorHeaps {
                cbv_srv_uav: heap.heap,
                sampler: HeapId::NULL,
            }));
        }
        for (i, offset) in offsets.into_iter().enumerate() {
            c.push(if table {
                Command::SetRootTable(SetRootTable {
                    bind_point: BindPoint::COMPUTE,
                    root_index: 0,
                    heap: heap.heap,
                    index: i as u32,
                })
            } else {
                root_va(0, RootParamKind::CBV, src.gpu_va + offset)
            });
            c.push(root_va(1, RootParamKind::UAV, out.gpu_va + i as u64 * 16));
            c.push(dispatch(1, 1, 1));
        }
        finish(&fx, q, c);
        assert_eq!(cpu_read(out.cpu_addr, 12), expected, "table={table}");
    }
}

const HEAP_INDEXED: &str = "cbuffer c : register(b0) { uint si; uint di; uint k; uint pad; }\n\
[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
  ByteAddressBuffer src = ResourceDescriptorHeap[si];\n\
  RWByteAddressBuffer dst = ResourceDescriptorHeap[di];\n\
  dst.Store(t.x*4, src.Load(t.x*4) * k + 7u); }\n";

/// `ResourceDescriptorHeap[]`: the shader finds its buffers by index in the
/// descriptor heap the stream bound.
#[test]
fn compute_through_the_resource_descriptor_heap() {
    let Some(dxil) = dxil_or_skip("exec_heap", "cs_6_6", HEAP_INDEXED) else {
        return;
    };
    let fx = Fx::new();
    let q = fx.queue();
    let sig = fx.root_sig(&RootSigDesc {
        flags: root_flags::CBV_SRV_UAV_HEAP_INDEXED | root_flags::SAMPLER_HEAP_INDEXED,
        params: vec![constants_param(4)],
        ..RootSigDesc::default()
    });
    let pso = pipeline(&fx, sig, &dxil);
    let n = 64usize;
    let src = fx.buffer(BufferKind::SHARED, 4 * n as u64);
    let dst = fx.buffer(BufferKind::SHARED, 4 * n as u64);
    let words: Vec<u32> = (0..n as u32).map(|i| i * 5 + 2).collect();
    cpu_write(src.cpu_addr, &words);
    cpu_write(dst.cpu_addr, &vec![0; n]);
    let heap = fx.heap(HeapKind::CBV_SRV_UAV, 64, true);
    let (si, di, k) = (17u32, 40u32, 3u32);
    fx.write_view(
        heap.heap,
        si,
        ViewDesc::raw(ViewKind::BUFFER_SRV, src.res, 0, n as u32),
    );
    fx.write_view(
        heap.heap,
        di,
        ViewDesc::raw(ViewKind::BUFFER_UAV, dst.res, 0, n as u32),
    );
    let mut data = Vec::new();
    for v in [si, di, k, 0] {
        data.extend_from_slice(&v.to_le_bytes());
    }
    let mut c = set_up(sig, pso, q);
    c.push(Command::SetDescriptorHeaps(SetDescriptorHeaps {
        cbv_srv_uav: heap.heap,
        sampler: HeapId::NULL,
    }));
    c.push(constants(0, &data));
    c.push(dispatch(2, 1, 1));
    finish(&fx, q, c);
    let want: Vec<u32> = words.iter().map(|w| w * k + 7).collect();
    assert_eq!(cpu_read(dst.cpu_addr, n), want);
}

const TWO_D: &str = "RWByteAddressBuffer o : register(u0);\n\
[numthreads(8,4,1)] void main(uint3 t : SV_DispatchThreadID) { o.Store((t.y*16u + t.x)*4, t.y*16u + t.x + 1u); }\n";

/// The thread-group size is the shader's: 8x4, never a number the stream or
/// the call carries. Wrong groups would leave words unwritten or misplaced.
#[test]
fn the_thread_group_size_comes_from_the_shader() {
    let Some(dxil) = dxil_or_skip("exec_2d", "cs_6_6", TWO_D) else {
        return;
    };
    let fx = Fx::new();
    let q = fx.queue();
    let sig = fx.root_sig(&RootSigDesc {
        params: vec![descriptor_param(RootParamKind::UAV)],
        ..RootSigDesc::default()
    });
    let pso = pipeline(&fx, sig, &dxil);
    let out = fx.buffer(BufferKind::SHARED, 4096);
    cpu_write(out.cpu_addr, &[0; 1024]);
    let mut c = set_up(sig, pso, q);
    c.push(root_va(0, RootParamKind::UAV, out.gpu_va));
    c.push(dispatch(2, 1, 1));
    finish(&fx, q, c);
    let want: Vec<u32> = (1..=64).collect();
    let got = cpu_read(out.cpu_addr, 65);
    assert_eq!(&got[..64], &want[..]);
    assert_eq!(got[64], 0);
}

/// A root signature with a static sampler gets MSC's extra argument-buffer
/// entry; the interpreter fills it with the address of the sampler table it
/// keeps resident, and the other parameters are where MSC says.
#[test]
fn a_static_sampler_does_not_move_the_other_parameters() {
    let Some(dxil) = dxil_or_skip("exec_static_sampler", "cs_6_6", ROOT_UAV_CONST) else {
        return;
    };
    let fx = Fx::new();
    let q = fx.queue();
    let sig = fx.root_sig(&RootSigDesc {
        params: vec![constants_param(1), descriptor_param(RootParamKind::UAV)],
        static_samplers: vec![StaticSampler::default()],
        ..RootSigDesc::default()
    });
    assert_eq!(
        fx.executor()
            .tlab_layout(sig)
            .unwrap()
            .static_table_offset(),
        Some(16)
    );
    // One 24-byte sampler entry: the sampler's resource id, then no texture, then the LOD bias.
    let table = fx.executor().static_sampler_table(sig).unwrap().unwrap();
    let e = table.buffer().read_u32s(0, 6).unwrap();
    assert!(e[0] != 0 || e[1] != 0, "a sampler resource id: {e:?}");
    assert_eq!((e[2], e[3]), (0, 0), "no texture view in a sampler entry");
    let pso = pipeline(&fx, sig, &dxil);
    let out = fx.buffer(BufferKind::SHARED, 4096);
    cpu_write(out.cpu_addr, &[0; 64]);
    let mut c = set_up(sig, pso, q);
    let k = 5u32.to_le_bytes();
    c.push(constants(0, &k));
    c.push(root_va(1, RootParamKind::UAV, out.gpu_va));
    c.push(dispatch(1, 1, 1));
    finish(&fx, q, c);
    let want: Vec<u32> = (0..32u32).map(|t| t * 3 + 5).collect();
    assert_eq!(cpu_read(out.cpu_addr, 32), want);
}

/// Two root signatures, one shader: a pipeline is tied to the signature it
/// was made for, and a dispatch under another one is refused.
#[test]
fn a_pipeline_is_tied_to_its_root_signature() {
    let Some(dxil) = dxil_or_skip("exec_tied", "cs_6_6", ROOT_UAV_CONST) else {
        return;
    };
    let fx = Fx::new();
    let q = fx.queue();
    let desc = RootSigDesc {
        params: vec![constants_param(1), descriptor_param(RootParamKind::UAV)],
        ..RootSigDesc::default()
    };
    let a = fx.root_sig(&desc);
    // (An identical description is the same signature to D3D12, which
    // `RootSig::compatible_with` accepts; a different one is refused.)
    let other = fx.root_sig(&RootSigDesc {
        params: vec![constants_param(2), descriptor_param(RootParamKind::UAV)],
        ..RootSigDesc::default()
    });
    let pso = pipeline(&fx, a, &dxil);
    let b = other;
    let c = vec![
        begin(1, q),
        set_sig(b),
        Command::SetPipeline(SetPipeline { pso, _pad: 0 }),
        dispatch(1, 1, 1),
        end(1),
    ];
    assert_eq!(fx.submit(&c), Status::INVALID_ARG);
}
