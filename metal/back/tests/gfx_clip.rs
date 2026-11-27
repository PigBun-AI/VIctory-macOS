//! Nodes S-M-003 and S-G-006: a geometry pipeline whose vertex stage writes `SV_ClipDistance`.
//!
//! Metal cannot make the geometry shader's mesh function from the converter's AIR when the
//! stage carries a clip distance ("Failed to materializeAll."), and `MTL4Compiler` does not
//! return that error: it takes the function as nil and asserts in the descriptor validation
//! (`meshFunction must not be nil`, SIGABRT). 29 of the title's 2192 vertex shaders write a
//! clip distance and abort the census run behind the rectangle-list geometry shader. The
//! back now makes the function first and refuses the pipeline with the reason. DXIL is
//! synthetic (HLSL through the DXC front end; `SKIPPED` without one).
#![cfg(target_vendor = "apple")]

mod common_msc;
mod exec_support;

use common_msc::dxil_or_skip;
use exec_support::*;
use victory_metal_proto::pso::*;
use victory_metal_proto::root::*;
use victory_metal_proto::status::Status;
use victory_metal_proto::sync::CreateGraphicsPipelineArgs;
use victory_metal_proto::types::*;

const VS_CLIP: &str = "struct VSIn { float3 pos : POSITION; float4 col : COLOR0; };\n\
struct VSOut { float4 pos : SV_Position; float4 col : COLOR0; float clip : SV_ClipDistance0; };\n\
VSOut main(VSIn i) { VSOut o; o.pos = float4(i.pos, 1); o.col = i.col; o.clip = i.pos.x + 0.5; return o; }\n";

const GS_CLIP: &str = "struct V { float4 pos : SV_Position; float4 col : COLOR0; float clip : SV_ClipDistance0; };\n\
[maxvertexcount(3)]\n\
void main(triangle V i[3], inout TriangleStream<V> s) {\n\
  for (int k = 0; k < 3; k++) { s.Append(i[k]); }\n\
}\n";

const PS: &str = "struct VSOut { float4 pos : SV_Position; float4 col : COLOR0; float clip : SV_ClipDistance0; };\n\
float4 main(VSOut i) : SV_Target0 { return i.col; }\n";

fn create_with_gs(vs: &[u8], gs: &[u8], ps: &[u8]) -> (Fx, Status) {
    let fx = Fx::new();
    let sig = fx.root_sig(&RootSigDesc::default());
    let mut d = GraphicsPsoDesc::new();
    d.vs = StageCode::of(vs);
    d.gs = StageCode::of(gs);
    d.ps = StageCode::of(ps);
    d.input_count = 2;
    d.inputs[0] = InputElement::new("POSITION", 0, format::R32G32B32_FLOAT, 0, 0);
    d.inputs[1] = InputElement::new("COLOR", 0, format::R32G32B32A32_FLOAT, 0, APPEND_ALIGNED);
    d.rt_count = 1;
    d.rt_formats[0] = format::R8G8B8A8_UNORM;
    let mut a = CreateGraphicsPipelineArgs::new();
    a.root_sig = sig;
    a.desc = d;
    let (s, _) = fx.call(a);
    (fx, s)
}

#[test]
fn a_geometry_pipeline_with_a_clip_distance_is_refused_not_aborted() {
    let (Some(vs), Some(gs), Some(ps)) = (
        dxil_or_skip("clip_vs", "vs_6_6", VS_CLIP),
        dxil_or_skip("clip_gs", "gs_6_6", GS_CLIP),
        dxil_or_skip("clip_ps", "ps_6_6", PS),
    ) else {
        return;
    };
    let (fx, st) = create_with_gs(&vs, &gs, &ps);
    // An error with the reason, and the process is still here to read it.
    assert_eq!(st, Status::UNSUPPORTED);
    let why = fx.executor().last_error().unwrap();
    assert!(
        why.contains("geometry emulation mesh function") && why.contains("materializeAll"),
        "{why}"
    );
}

const MS_CLIP: &str = "struct V { float4 pos : SV_Position; float4 col : COLOR0; float clip : SV_ClipDistance0; };\n\
[numthreads(32,1,1)] [outputtopology(\"triangle\")]\n\
void main(uint tid : SV_GroupIndex, out vertices V verts[3], out indices uint3 tris[1]) {\n\
  SetMeshOutputCounts(3, 1);\n\
  if (tid < 3) {\n\
    float2 p[3] = { float2(-1,-1), float2(1.05,-1), float2(-1,1.05) };\n\
    verts[tid].pos = float4(p[tid], 0.5, 1); verts[tid].col = float4(0, 1, 0, 1); verts[tid].clip = p[tid].x + 0.5; }\n\
  if (tid == 0) tris[0] = uint3(0,1,2);\n}\n";

/// The control: a mesh pipeline with a clip distance builds, so the failure is in the geometry
/// emulation path (object + mesh stage with the vertex stage's output as the stage-in), not in
/// the clip distance alone.
#[test]
fn a_mesh_pipeline_with_a_clip_distance_is_made() {
    let Some(ms) = dxil_or_skip("clip_ms", "ms_6_6", MS_CLIP) else {
        return;
    };
    let Some(ps) = dxil_or_skip("clip_ps_ms", "ps_6_6", PS) else {
        return;
    };
    let fx = Fx::new();
    let sig = fx.root_sig(&RootSigDesc::default());
    let mut d = GraphicsPsoDesc::new();
    d.ms = StageCode::of(&ms);
    d.ps = StageCode::of(&ps);
    d.rt_count = 1;
    d.rt_formats[0] = format::R8G8B8A8_UNORM;
    let mut a = CreateGraphicsPipelineArgs::new();
    a.root_sig = sig;
    a.desc = d;
    let (st, _) = fx.call(a);
    assert_eq!(st, Status::OK, "{:?}", fx.executor().last_error());
}
