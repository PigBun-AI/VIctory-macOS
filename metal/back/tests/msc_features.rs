//! What Metal Shader Converter does with each DXIL construct the user
//! manual lists as unsupported or limited ("Feature support" tables), and
//! the ones this backend relies on: HLSL snippets compiled by DXC (see
//! `common_msc.rs`; skipped without one), converted with no root signature
//! (MSC's default layout), and the outcome pinned. A change of MSC's
//! behaviour (a newer version accepting `SV_ViewID`, say) fails here and
//! the table in docs/metal/msc.md has to follow.
//!
//! "Accepted" is only that MSC converted it: where the manual says a feature
//! is unsupported and MSC converts it anyway (`SV_StencilRef`, `SV_ShadingRate`,
//! ROVs, 64-bit float arithmetic) the converter ignores or approximates it, so
//! acceptance says nothing about the result being right. For
//! `pack_unpack_sm66` it was not checked whether this DXC emits the SM 6.6
//! pack opcode the manual means or expands it to arithmetic.
#![cfg(target_vendor = "apple")]

mod common_msc;

use victory_metal_back::msc::{CompileOptions, Msc, MscError};

use common_msc::dxil_result;

/// Outcome of converting one snippet.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Accepted,
    Refused(&'static str),
    /// Turned away by the back's own guard before MSC ran (`msc/guard.rs`):
    /// the text of its reason.
    Guarded(&'static str),
}

struct Case {
    name: &'static str,
    profile: &'static str,
    /// The manual's verdict ("no", "limited", "yes") or `-` where the manual is silent.
    manual: &'static str,
    want: Outcome,
    hlsl: &'static str,
}

use Outcome::{Accepted, Guarded, Refused};

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "wave_size_32",
            profile: "cs_6_6",
            manual: "yes",
            want: Accepted,
            hlsl: "RWByteAddressBuffer o;\n[WaveSize(32)][numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) { o.Store(t.x*4, WaveActiveSum(t.x)); }",
        },
        Case {
            name: "wave_size_64",
            profile: "cs_6_6",
            manual: "no (error-detected)",
            want: Refused("UnsupportedWaveSize"),
            hlsl: "RWByteAddressBuffer o;\n[WaveSize(64)][numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) { o.Store(t.x*4, WaveActiveSum(t.x)); }",
        },
        Case {
            name: "wave_size_16",
            profile: "cs_6_6",
            manual: "no (error-detected)",
            want: Refused("UnsupportedWaveSize"),
            hlsl: "RWByteAddressBuffer o;\n[WaveSize(16)][numthreads(16,1,1)] void main(uint3 t : SV_DispatchThreadID) { o.Store(t.x*4, WaveActiveSum(t.x)); }",
        },
        Case {
            name: "wave_intrinsics",
            profile: "cs_6_6",
            manual: "yes",
            want: Accepted,
            hlsl: "RWByteAddressBuffer o;\n[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) { uint v = WaveReadLaneAt(t.x, 3) + WavePrefixSum(t.x) + QuadReadAcrossX(t.x) + (uint)WaveActiveBallot(t.x>4).x; o.Store(t.x*4, v); }",
        },
        Case {
            name: "int64_ops",
            profile: "cs_6_6",
            manual: "yes",
            want: Accepted,
            hlsl: "RWByteAddressBuffer o;\n[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) { uint64_t a = (uint64_t)t.x * 0x100000001ull; o.Store(t.x*4, (uint)(a >> 3)); }",
        },
        Case {
            name: "atomic64_buffer",
            profile: "cs_6_6",
            manual: "limited",
            want: Accepted,
            hlsl: "RWByteAddressBuffer o;\n[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) { uint64_t r; o.InterlockedMax64(0, (uint64_t)t.x, r); }",
        },
        Case {
            name: "atomic64_heap",
            profile: "cs_6_6",
            manual: "limited",
            want: Refused("UnsupportedInstruction"),
            hlsl: "cbuffer c { uint i; }\n[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) { RWByteAddressBuffer b = ResourceDescriptorHeap[i]; uint64_t r; b.InterlockedAdd64(0, (uint64_t)t.x, r); }",
        },
        Case {
            name: "atomic64_texture",
            profile: "cs_6_6",
            manual: "limited",
            want: Accepted,
            hlsl: "RWTexture2D<uint64_t> t;\n[numthreads(8,8,1)] void main(uint3 id : SV_DispatchThreadID) { InterlockedMax(t[id.xy], (uint64_t)id.x); }",
        },
        Case {
            name: "atomic64_texture_heap",
            profile: "cs_6_6",
            manual: "limited",
            want: Accepted,
            hlsl: "cbuffer c { uint i; }\n[numthreads(8,8,1)] void main(uint3 id : SV_DispatchThreadID) { RWTexture2D<uint64_t> t = ResourceDescriptorHeap[i]; InterlockedMax(t[id.xy], (uint64_t)id.x); }",
        },
        Case {
            name: "atomic32_groupshared",
            profile: "cs_6_6",
            manual: "yes",
            want: Accepted,
            hlsl: "RWByteAddressBuffer o; groupshared uint g;\n[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID, uint gi : SV_GroupIndex) { if (gi==0) g=0; GroupMemoryBarrierWithGroupSync(); InterlockedAdd(g, 1); GroupMemoryBarrierWithGroupSync(); o.Store(t.x*4, g); }",
        },
        Case {
            name: "double_arithmetic",
            profile: "cs_6_6",
            manual: "-",
            // MSC converts plain double arithmetic (with f32), but aborts the
            // process on a double wave reduction: the back refuses both.
            want: Guarded("MSC 4.0.3 cannot convert double-precision"),
            hlsl: "RWByteAddressBuffer o;\n[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) { double d = (double)t.x * 1.5; o.Store(t.x*4, asuint((float)(d*d))); }",
        },
        Case {
            name: "derivatives_in_compute",
            profile: "cs_6_6",
            manual: "-",
            want: Accepted,
            hlsl: "Texture2D<float4> tx; SamplerState s; RWByteAddressBuffer o;\n[numthreads(8,8,1)] void main(uint3 t : SV_DispatchThreadID) { float2 uv = t.xy / 64.0; o.Store((t.y*8+t.x)*4, asuint(tx.Sample(s, uv).x + ddx(uv.x))); }",
        },
        Case {
            name: "texture_sample_min_lod_clamp",
            profile: "ps_6_6",
            manual: "no (with texture read)",
            want: Accepted,
            hlsl: "Texture2D<float4> tx; SamplerState s;\nfloat4 main(float2 uv : TEXCOORD) : SV_Target { return tx.Sample(s, uv, int2(0,0), 2.0); }",
        },
        Case {
            name: "sv_view_id",
            profile: "ps_6_6",
            manual: "no",
            want: Refused("UnsupportedInstruction"),
            hlsl: "float4 main(uint v : SV_ViewID) : SV_Target { return (float)v; }",
        },
        Case {
            name: "sv_stencil_ref",
            profile: "ps_6_6",
            manual: "no",
            want: Accepted,
            hlsl: "struct O { float4 c : SV_Target; uint s : SV_StencilRef; };\nO main() { O o; o.c = 1; o.s = 2; return o; }",
        },
        Case {
            name: "sv_shading_rate",
            profile: "ps_6_6",
            manual: "no (VRS)",
            want: Accepted,
            hlsl: "float4 main(uint r : SV_ShadingRate) : SV_Target { return (float)r; }",
        },
        Case {
            name: "pixel_barycentrics",
            profile: "ps_6_6",
            manual: "-",
            want: Accepted,
            hlsl: "float4 main(float3 b : SV_Barycentrics) : SV_Target { return float4(b, 1); }",
        },
        Case {
            name: "pixel_barycentrics_noperspective",
            profile: "ps_6_6",
            manual: "-",
            want: Accepted,
            hlsl: "float4 main(noperspective float3 b : SV_Barycentrics) : SV_Target { return float4(b, 1); }",
        },
        Case {
            name: "pixel_barycentrics_centroid",
            profile: "ps_6_6",
            manual: "-",
            want: Refused("CompilationError"),
            hlsl: "float4 main(centroid float3 b : SV_Barycentrics) : SV_Target { return float4(b, 1); }",
        },
        Case {
            name: "get_attribute_at_vertex",
            profile: "ps_6_6",
            manual: "-",
            want: Refused("UnsupportedInstruction"),
            hlsl: "float4 main(nointerpolation float4 c : COLOR) : SV_Target { return GetAttributeAtVertex(c, 1); }",
        },
        Case {
            name: "sampler_feedback",
            profile: "ps_6_6",
            manual: "no",
            want: Refused("UnsupportedInstruction"),
            hlsl: "FeedbackTexture2D<SAMPLER_FEEDBACK_MIN_MIP> fb; Texture2D<float4> tx; SamplerState s;\nfloat4 main(float2 uv : TEXCOORD) : SV_Target { fb.WriteSamplerFeedback(tx, s, uv); return 0; }",
        },
        Case {
            name: "rov",
            profile: "ps_6_6",
            manual: "-",
            want: Accepted,
            hlsl: "RasterizerOrderedTexture2D<float4> r;\nfloat4 main(float4 p : SV_Position) : SV_Target { r[uint2(p.xy)] += 1; return 0; }",
        },
        Case {
            name: "dual_source_blend_outputs",
            profile: "ps_6_6",
            manual: "-",
            want: Accepted,
            hlsl: "struct O { float4 a : SV_Target0; float4 b : SV_Target1; };\nO main() { O o; o.a = 1; o.b = 0.5; return o; }",
        },
        Case {
            name: "sv_coverage_out",
            profile: "ps_6_6",
            manual: "-",
            want: Accepted,
            hlsl: "struct O { float4 c : SV_Target; uint m : SV_Coverage; };\nO main() { O o; o.c = 1; o.m = 1; return o; }",
        },
        Case {
            name: "sv_render_target_array_index_vs",
            profile: "vs_6_6",
            manual: "-",
            want: Accepted,
            hlsl: "struct O { float4 p : SV_Position; uint r : SV_RenderTargetArrayIndex; };\nO main(uint v : SV_VertexID) { O o; o.p = (float4)v; o.r = v & 1; return o; }",
        },
        Case {
            name: "pack_unpack_sm66",
            profile: "cs_6_6",
            manual: "no (error-detected)",
            want: Accepted,
            hlsl: "RWByteAddressBuffer o;\n[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) { int4 v = int4(t.x, 1, 2, 3); o.Store(t.x*4, pack_s8(v)); }",
        },
        Case {
            name: "dot4_packed",
            profile: "cs_6_6",
            manual: "-",
            want: Accepted,
            hlsl: "RWByteAddressBuffer o;\n[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) { o.Store(t.x*4, dot4add_u8packed(t.x, 0x01020304, 7)); }",
        },
        Case {
            name: "mesh_shader",
            profile: "ms_6_6",
            manual: "yes",
            want: Accepted,
            hlsl: "struct V { float4 p : SV_Position; };\n[numthreads(32,1,1)][outputtopology(\"triangle\")] void main(uint gi : SV_GroupIndex, out vertices V v[3], out indices uint3 i[1]) { SetMeshOutputCounts(3,1); if (gi<3) v[gi].p = (float4)gi; if (gi==0) i[0] = uint3(0,1,2); }",
        },
        Case {
            name: "amplification_shader",
            profile: "as_6_6",
            manual: "yes",
            want: Accepted,
            hlsl: "struct P { uint a; };\n[numthreads(32,1,1)] void main(uint gi : SV_GroupIndex) { P p; p.a = gi; DispatchMesh(1,1,1,p); }",
        },
        Case {
            name: "ray_query_compute",
            profile: "cs_6_6",
            manual: "yes",
            want: Accepted,
            hlsl: "RaytracingAccelerationStructure as; RWByteAddressBuffer o;\n[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) { RayQuery<RAY_FLAG_FORCE_OPAQUE> q; RayDesc r; r.Origin = 0; r.Direction = float3(0,0,1); r.TMin = 0; r.TMax = 10; q.TraceRayInline(as, 0, 0xff, r); q.Proceed(); o.Store(t.x*4, q.CommittedStatus()); }",
        },
        Case {
            name: "groupshared_32k",
            profile: "cs_6_6",
            manual: "-",
            want: Accepted,
            hlsl: "RWByteAddressBuffer o; groupshared float4 g[2048];\n[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID, uint gi : SV_GroupIndex) { g[gi] = gi; GroupMemoryBarrierWithGroupSync(); o.Store(t.x*4, asuint(g[(gi+1)&63].x)); }",
        },
        Case {
            name: "sampler_heap",
            profile: "cs_6_6",
            manual: "yes",
            want: Accepted,
            hlsl: "cbuffer c { uint i; uint j; }\nRWByteAddressBuffer o;\n[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) { Texture2D<float4> tx = ResourceDescriptorHeap[i]; SamplerState s = SamplerDescriptorHeap[j]; o.Store(t.x*4, asuint(tx.SampleLevel(s, float2(0.5,0.5), 0).x)); }",
        },
        Case {
            name: "geometry_shader",
            profile: "gs_6_6",
            manual: "-",
            want: Accepted,
            hlsl: "struct V { float4 p : SV_Position; };\n[maxvertexcount(3)] void main(triangle V i[3], inout TriangleStream<V> s) { for (int k=0;k<3;k++) s.Append(i[k]); }",
        },
    ]
}

#[test]
fn feature_table() {
    let m = Msc::get().expect("MSC is required");
    let mut report = Vec::new();
    let mut mismatches = Vec::new();
    for c in cases() {
        let Some(made) = dxil_result(c.name, c.profile, c.hlsl) else {
            eprintln!(
                "SKIPPED feature table: set VICTORY_HELPER_DXC and DXC_LIB (docs/metal/msc.md)"
            );
            return;
        };
        let got = match made {
            Err(e) => format!("DxcRefused {e}"),
            Ok(dx) => match m.compile(&dx, None, &CompileOptions::default()) {
                Ok(_) => "Accepted".to_owned(),
                Err(MscError::Rejected { code, message }) => format!(
                    "Refused({}) {}",
                    code.name(),
                    message.lines().next().unwrap_or("")
                ),
                Err(e) => format!("Other({e})"),
            },
        };
        report.push(format!(
            "{:36} {:8} manual: {:24} -> {got}",
            c.name, c.profile, c.manual
        ));
        let ok = match &c.want {
            Accepted => got == "Accepted",
            Refused("?") => got.starts_with("Refused"),
            Refused(code) => got.starts_with(&format!("Refused({code})")),
            Guarded(why) => got == format!("Other(not supported: {why})"),
        };
        if !ok {
            mismatches.push(format!("{}: wanted {:?}, got {got}", c.name, c.want));
        }
    }
    eprintln!("{}", report.join("\n"));
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}
