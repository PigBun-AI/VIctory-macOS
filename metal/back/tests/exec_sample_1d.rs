//! `SampleLevel` of a one-dimensional `R32_FLOAT` texture through the
//! descriptor heaps (the title's tone curve: a 4096x1 float table sampled
//! with a linear sampler). The result is compared with the linear
//! interpolation computed on the CPU.
//!
//! The Metal Shader Converter compiles `Texture1D` accesses as `texture2d`
//! ones (a `Texture1D.SampleLevel` and a `Texture2D.SampleLevel` give the same
//! metallib), so the back makes a 1D texture a 2D texture of height 1; a real
//! `MTLTextureType1D` read 0 in the shader (rec-w24: the 4096x1 tone-curve
//! table, whole world white).
#![cfg(target_vendor = "apple")]

mod common_msc;
mod exec_support;

use common_msc::dxil_or_skip;
use exec_support::*;
use victory_metal_proto::cmd::*;
use victory_metal_proto::ids::*;
use victory_metal_proto::root::*;
use victory_metal_proto::status::Status;
use victory_metal_proto::sync::*;
use victory_metal_proto::types::*;

const N: usize = 4096;

const SAMPLE_1D: &str = "cbuffer c : register(b0) { uint ti; uint si; uint di; uint pad; }\n\
[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
  Texture1D<float> tx = ResourceDescriptorHeap[ti];\n\
  SamplerState s = SamplerDescriptorHeap[si];\n\
  RWByteAddressBuffer dst = ResourceDescriptorHeap[di];\n\
  float u = (t.x + 0.5) / 64.0;\n\
  dst.Store(t.x*4, asuint(tx.SampleLevel(s, u, 0)));\n\
  dst.Store(256 + t.x*4, asuint(tx.Load(int2(uint(u * 4095.0), 0))));\n\
}\n";

fn table(i: usize) -> f32 {
    // Like the title's curve: monotone, -5 .. 2.5.
    let x = i as f32 / (N as f32 - 1.0);
    2.5 - 7.5 * x.powf(0.6)
}

fn lerp_ref(u: f32) -> f32 {
    let p = (u * N as f32 - 0.5).clamp(0.0, N as f32 - 1.0);
    let i0 = p.floor() as usize;
    let i1 = (i0 + 1).min(N - 1);
    let f = p - i0 as f32;
    table(i0) * (1.0 - f) + table(i1) * f
}

fn run(
    dim: TexDim,
    view_dim: TexViewDim,
    hlsl_name: &str,
    hlsl: &str,
) -> Option<(Vec<f32>, Vec<f32>)> {
    let dxil = dxil_or_skip(hlsl_name, "cs_6_6", hlsl)?;
    let fx = Fx::new();
    let q = fx.queue();
    let f = fx.fence(0);
    let sig = fx.root_sig(&RootSigDesc {
        flags: root_flags::CBV_SRV_UAV_HEAP_INDEXED | root_flags::SAMPLER_HEAP_INDEXED,
        params: vec![constants_param(4)],
        ..RootSigDesc::default()
    });
    let mut a = CreateComputePipelineArgs::new();
    a.root_sig = sig;
    a.dxil_addr = dxil.as_ptr() as u64;
    a.dxil_len = dxil.len() as u64;
    let (s, a) = fx.call(a);
    assert_eq!(s, Status::OK, "{:?}", fx.executor().last_error());
    let pso = a.pso;

    let mut ta = CreateTextureArgs::new();
    ta.dimension = dim;
    ta.format = format::R32_FLOAT;
    ta.width = N as u32;
    ta.height = 1;
    ta.depth_or_array = 1;
    ta.mips = 1;
    ta.samples = 1;
    ta.usage = tex_usage::SRV;
    let tex = fx.ok(ta);
    let up = fx.buffer(BufferKind::UPLOAD, 4 * N as u64);
    let words: Vec<u32> = (0..N).map(|i| table(i).to_bits()).collect();
    cpu_write(up.cpu_addr, &words);
    let out = fx.buffer(BufferKind::SHARED, 1024);
    cpu_write(out.cpu_addr, &vec![0xDEAD_BEEF; 256]);

    let heap = fx.heap(HeapKind::CBV_SRV_UAV, 16, true);
    let smp = fx.heap(HeapKind::SAMPLER, 4, true);
    let mut v = ViewDesc::null();
    v.kind = ViewKind::TEXTURE_SRV;
    v.resource = tex.res;
    v.format = format::R32_FLOAT;
    v.dimension = view_dim;
    v.swizzle = swizzle::IDENTITY;
    v.mip_count = 1;
    fx.write_view(heap.heap, 3, v);
    fx.write_view(
        heap.heap,
        5,
        ViewDesc::raw(ViewKind::BUFFER_UAV, out.res, 0, 256),
    );
    let mut sv = ViewDesc::null();
    sv.kind = ViewKind::SAMPLER;
    let mut wa = WriteDescriptorArgs::new();
    wa.heap = smp.heap;
    wa.index = 1;
    wa.view = sv;
    wa.sampler = SamplerDesc {
        min_filter: Filter::LINEAR,
        mag_filter: Filter::LINEAR,
        mip_filter: Filter::LINEAR,
        address_u: AddressMode::CLAMP,
        address_v: AddressMode::CLAMP,
        address_w: AddressMode::CLAMP,
        max_lod: 1000.0,
        ..<SamplerDesc as victory_metal_proto::wire::Wire>::ZERO
    };
    assert_eq!(fx.status(wa), Status::OK);

    let mut data = Vec::new();
    for w in [3u32, 1, 5, 0] {
        data.extend_from_slice(&w.to_le_bytes());
    }
    let cmds = vec![
        begin(1, q),
        copy_texture_whole(tex.res, up.res, N as u32),
        Command::Barrier(Barrier::memory(stage::BLIT, stage::ALL)),
        set_sig(sig),
        Command::SetPipeline(SetPipeline { pso, _pad: 0 }),
        Command::SetDescriptorHeaps(SetDescriptorHeaps {
            cbv_srv_uav: heap.heap,
            sampler: smp.heap,
        }),
        Command::SetRootConstants(SetRootConstants {
            bind_point: BindPoint::COMPUTE,
            root_index: 0,
            dest_offset: 0,
            data: &data,
        }),
        dispatch(1, 1, 1),
        end(1),
        signal_cmd(q, f, 1),
    ];
    assert_eq!(fx.submit(&cmds), Status::OK);
    assert_eq!(fx.wait(f, 1), Status::OK);
    let got = cpu_read(out.cpu_addr, 128);
    let samples = got[..64].iter().map(|w| f32::from_bits(*w)).collect();
    let loads = got[64..128].iter().map(|w| f32::from_bits(*w)).collect();
    Some((samples, loads))
}

fn copy_texture_whole(dst: ResId, src: ResId, width: u32) -> Command<'static> {
    let mut d = <TextureLocation as victory_metal_proto::wire::Wire>::ZERO;
    d.res = dst;
    d.kind = LocKind::SUBRESOURCE;
    let mut s = <TextureLocation as victory_metal_proto::wire::Wire>::ZERO;
    s.res = src;
    s.kind = LocKind::FOOTPRINT;
    s.format = format::R32_FLOAT;
    s.row_pitch = width * 4;
    s.width = width;
    s.height = 1;
    s.depth = 1;
    let mut c = <CopyTextureRegion as victory_metal_proto::wire::Wire>::ZERO;
    c.dst = d;
    c.src = s;
    Command::CopyTextureRegion(c)
}

#[test]
fn a_1d_float_table_sampled_linearly_matches_the_cpu() {
    let Some((samples, loads)) = run(TexDim::TEX1D, TexViewDim::TEX1D, "sample_1d", SAMPLE_1D)
    else {
        return;
    };
    for t in 0..64usize {
        let u = (t as f32 + 0.5) / 64.0;
        let want = lerp_ref(u);
        let want_load = table((u * 4095.0) as usize);
        assert!(
            (loads[t] - want_load).abs() < 1e-5,
            "Load at {t}: got {} want {want_load}",
            loads[t]
        );
        assert!(
            (samples[t] - want).abs() < 2e-3,
            "SampleLevel at u={u}: got {} want {want}",
            samples[t]
        );
    }
}

const SAMPLE_2D: &str = "cbuffer c : register(b0) { uint ti; uint si; uint di; uint pad; }\n\
[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
  Texture2D<float> tx = ResourceDescriptorHeap[ti];\n\
  SamplerState s = SamplerDescriptorHeap[si];\n\
  RWByteAddressBuffer dst = ResourceDescriptorHeap[di];\n\
  float u = (t.x + 0.5) / 64.0;\n\
  dst.Store(t.x*4, asuint(tx.SampleLevel(s, float2(u, 0.5), 0)));\n\
  dst.Store(256 + t.x*4, asuint(tx.Load(int3(uint(u * 4095.0), 0, 0))));\n\
}\n";

/// The same table as a 4096x1 two-dimensional texture: the control.
#[test]
fn a_4096x1_2d_float_table_sampled_linearly_matches_the_cpu() {
    let Some((samples, loads)) = run(TexDim::TEX2D, TexViewDim::TEX2D, "sample_2d", SAMPLE_2D)
    else {
        return;
    };
    for t in 0..64usize {
        let u = (t as f32 + 0.5) / 64.0;
        assert!(
            (loads[t] - table((u * 4095.0) as usize)).abs() < 1e-5,
            "Load at {t}: {}",
            loads[t]
        );
        assert!(
            (samples[t] - lerp_ref(u)).abs() < 2e-3,
            "Sample at {t}: {} vs {}",
            samples[t],
            lerp_ref(u)
        );
    }
}

/// Narrowing: the copy commands reach a 1D texture (so a zero read is the
/// view or the shader, not the upload).
#[test]
fn a_1d_texture_round_trips_through_copy_commands() {
    let fx = Fx::new();
    let q = fx.queue();
    let f = fx.fence(0);
    let mut ta = CreateTextureArgs::new();
    ta.dimension = TexDim::TEX1D;
    ta.format = format::R32_FLOAT;
    ta.width = N as u32;
    ta.height = 1;
    ta.depth_or_array = 1;
    ta.mips = 1;
    ta.samples = 1;
    ta.usage = tex_usage::SRV;
    let tex = fx.ok(ta);
    let up = fx.buffer(BufferKind::UPLOAD, 4 * N as u64);
    let rb = fx.buffer(BufferKind::READBACK, 4 * N as u64);
    let words: Vec<u32> = (0..N).map(|i| table(i).to_bits()).collect();
    cpu_write(up.cpu_addr, &words);
    let mut back = copy_texture_whole(tex.res, up.res, N as u32);
    if let Command::CopyTextureRegion(c) = &mut back {
        std::mem::swap(&mut c.dst, &mut c.src);
        c.dst.res = rb.res;
        c.src.res = tex.res;
        c.src.kind = LocKind::SUBRESOURCE;
        c.dst.kind = LocKind::FOOTPRINT;
    }
    let cmds = vec![
        begin(1, q),
        copy_texture_whole(tex.res, up.res, N as u32),
        Command::Barrier(Barrier::memory(stage::BLIT, stage::BLIT)),
        back,
        end(1),
        signal_cmd(q, f, 1),
    ];
    assert_eq!(fx.submit(&cmds), Status::OK);
    assert_eq!(fx.wait(f, 1), Status::OK);
    assert_eq!(cpu_read(rb.cpu_addr, 16), words[..16].to_vec());
}

const SAMPLE_1D_V4: &str = "cbuffer c : register(b0) { uint ti; uint si; uint di; uint pad; }\n\
[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
  Texture1D<float4> tx = ResourceDescriptorHeap[ti];\n\
  SamplerState s = SamplerDescriptorHeap[si];\n\
  RWByteAddressBuffer dst = ResourceDescriptorHeap[di];\n\
  float u = (t.x + 0.5) / 64.0;\n\
  dst.Store(t.x*4, asuint(tx.SampleLevel(s, u, 0).x));\n\
  dst.Store(256 + t.x*4, asuint(tx.Load(int2(uint(u * 4095.0), 0)).x));\n\
}\n";

#[test]
fn a_1d_texture_declared_float4_reads_the_same_table() {
    let Some((samples, loads)) = run(
        TexDim::TEX1D,
        TexViewDim::TEX1D,
        "sample_1d_v4",
        SAMPLE_1D_V4,
    ) else {
        return;
    };
    for t in 0..64usize {
        let u = (t as f32 + 0.5) / 64.0;
        assert!(
            (loads[t] - table((u * 4095.0) as usize)).abs() < 1e-5,
            "Load at {t}: {}",
            loads[t]
        );
        assert!(
            (samples[t] - lerp_ref(u)).abs() < 2e-3,
            "Sample at {t}: {}",
            samples[t]
        );
    }
}
