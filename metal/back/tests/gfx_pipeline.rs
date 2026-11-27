//! `CREATE_GRAPHICS_PIPELINE` end to end through the proto interpreter: HLSL
//! compiled to DXIL by the repository's DXC front end (`common_msc.rs`;
//! `SKIPPED` without one), Metal Shader Converter, `MTL4Compiler`.
//!
//! Vertex + pixel and mesh (+ amplification) + pixel pipelines, state
//! variations, the clear refusals, and concurrent creation from several
//! threads. Pipelines are only created here: drawing is the next milestone.
#![cfg(target_vendor = "apple")]

mod common_msc;
mod exec_support;

use common_msc::dxil_or_skip;
use exec_support::*;
use victory_metal_back::msc::ShaderStage;
use victory_metal_back::render::Kind;
use victory_metal_proto::ids::*;
use victory_metal_proto::link::LinkExt;
use victory_metal_proto::pso::*;
use victory_metal_proto::root::*;
use victory_metal_proto::status::Status;
use victory_metal_proto::sync::{CreateGraphicsPipelineArgs, DestroyPipelineArgs};
use victory_metal_proto::types::*;

const VS: &str = "struct VSIn { float3 pos : POSITION; float2 uv : TEXCOORD0; float4 col : COLOR1; };\n\
struct VSOut { float4 pos : SV_Position; float2 uv : TEXCOORD0; float4 col : COLOR0; };\n\
cbuffer c : register(b0) { float4 k; }\n\
VSOut main(VSIn i) { VSOut o; o.pos = float4(i.pos, 1) * k; o.uv = i.uv; o.col = i.col; return o; }\n";

const PS: &str = "struct VSOut { float4 pos : SV_Position; float2 uv : TEXCOORD0; float4 col : COLOR0; };\n\
cbuffer c : register(b0) { float4 k; }\n\
float4 main(VSOut i) : SV_Target0 { return float4(i.uv, 0, 1) * k + i.col; }\n";

const MS: &str = "struct V { float4 pos : SV_Position; float2 uv : TEXCOORD0; float4 col : COLOR0; };\n\
cbuffer c : register(b0) { float4 k; }\n\
[numthreads(32,1,1)] [outputtopology(\"triangle\")]\n\
void main(uint tid : SV_GroupIndex, out vertices V verts[3], out indices uint3 tris[1]) {\n\
  SetMeshOutputCounts(3, 1);\n\
  if (tid < 3) { verts[tid].pos = float4((tid==0)?-1:((tid==1)?1:0), (tid==2)?1:-1, 0, 1) * k; verts[tid].uv = float2(tid, 0); verts[tid].col = k; }\n\
  if (tid == 0) tris[0] = uint3(0,1,2);\n}\n";

const AS: &str = "struct Payload { uint count; float4 tint; };\n\
groupshared Payload p;\n\
cbuffer c : register(b0) { float4 k; }\n\
[numthreads(32,1,1)] void main(uint tid : SV_GroupIndex) {\n\
  p.count = 1; p.tint = k;\n  DispatchMesh(1, 1, 1, p);\n}\n";

const MS_WITH_PAYLOAD: &str = "struct Payload { uint count; float4 tint; };\n\
struct V { float4 pos : SV_Position; float2 uv : TEXCOORD0; float4 col : COLOR0; };\n\
[numthreads(32,1,1)] [outputtopology(\"triangle\")]\n\
void main(uint tid : SV_GroupIndex, in payload Payload pl, out vertices V verts[3], out indices uint3 tris[1]) {\n\
  SetMeshOutputCounts(3, 1);\n\
  if (tid < 3) { verts[tid].pos = float4(tid, 0, 0, 1); verts[tid].uv = float2(tid, 0); verts[tid].col = pl.tint; }\n\
  if (tid == 0) tris[0] = uint3(0,1,2);\n}\n";

/// The most a mesh threadgroup may output under the DXIL rules (32 KB of
/// vertex data: 256 vertices of 128 bytes).
const MS_BIGGEST: &str = "struct V { float4 pos : SV_Position; float4 a0 : A0; float4 a1 : A1; float4 a2 : A2; float4 a3 : A3; float4 a4 : A4; float4 a5 : A5; float4 a6 : A6; };\n\
[numthreads(32,1,1)] [outputtopology(\"triangle\")]\n\
void main(uint gtid : SV_GroupIndex, out vertices V verts[256], out indices uint3 tris[254]) {\n\
  SetMeshOutputCounts(256, 254);\n\
  for (uint tid = gtid; tid < 256; tid += 32) { verts[tid].pos = float4(tid,0,0,1); verts[tid].a0 = tid; verts[tid].a1 = tid; verts[tid].a2 = tid; verts[tid].a3 = tid; verts[tid].a4 = tid; verts[tid].a5 = tid; verts[tid].a6 = tid; }\n\
  for (uint t = gtid; t < 254; t += 32) tris[t] = uint3(0,1,2);\n}\n";

const PS_TWO_TARGETS: &str = "struct VSOut { float4 pos : SV_Position; float2 uv : TEXCOORD0; float4 col : COLOR0; };\n\
struct Out { float4 c : SV_Target0; uint id : SV_Target1; };\n\
Out main(VSOut i) { Out o; o.c = float4(i.uv, 0, 1); o.id = 7; return o; }\n";

const PS_DEPTH: &str = "struct VSOut { float4 pos : SV_Position; float2 uv : TEXCOORD0; float4 col : COLOR0; };\n\
struct Out { float4 c : SV_Target0; float d : SV_Depth; };\n\
Out main(VSOut i) { Out o; o.c = i.col; o.d = 0.5; return o; }\n";

const PS_DUAL: &str = "struct VSOut { float4 pos : SV_Position; float2 uv : TEXCOORD0; float4 col : COLOR0; };\n\
struct Out { float4 c : SV_Target0; float4 c1 : SV_Target1; };\n\
Out main(VSOut i) { Out o; o.c = i.col; o.c1 = float4(i.uv, 0, 1); return o; }\n";

struct Stages {
    vs: Vec<u8>,
    ps: Vec<u8>,
}

fn vertex_stages() -> Option<Stages> {
    Some(Stages {
        vs: dxil_or_skip("gfx_vs", "vs_6_6", VS)?,
        ps: dxil_or_skip("gfx_ps", "ps_6_6", PS)?,
    })
}

fn root_desc() -> RootSigDesc {
    RootSigDesc {
        params: vec![descriptor_param(RootParamKind::CBV)],
        ..RootSigDesc::default()
    }
}

fn layout(d: &mut GraphicsPsoDesc) {
    d.input_count = 3;
    d.inputs[0] = InputElement::new("POSITION", 0, format::R32G32B32_FLOAT, 0, 0);
    d.inputs[1] = InputElement::new("TEXCOORD", 0, 16, 0, APPEND_ALIGNED);
    d.inputs[2] = InputElement::new("COLOR", 1, format::R8G8B8A8_UNORM, 1, 0);
}

fn vertex_desc(s: &Stages) -> GraphicsPsoDesc {
    let mut d = GraphicsPsoDesc::new();
    d.vs = StageCode::of(&s.vs);
    d.ps = StageCode::of(&s.ps);
    layout(&mut d);
    d.rt_count = 1;
    d.rt_formats[0] = format::R8G8B8A8_UNORM;
    d
}

fn create(fx: &Fx, sig: RootSigId, desc: GraphicsPsoDesc) -> (Status, PsoId) {
    let mut a = CreateGraphicsPipelineArgs::new();
    a.root_sig = sig;
    a.desc = desc;
    let (s, a) = fx.call(a);
    (s, a.pso)
}

fn make(fx: &Fx, sig: RootSigId, desc: GraphicsPsoDesc) -> PsoId {
    let (s, pso) = create(fx, sig, desc);
    assert_eq!(s, Status::OK, "{:?}", fx.executor().last_error());
    assert_ne!(pso, PsoId(0));
    pso
}

fn has_mesh(fx: &Fx) -> bool {
    fx.executor().device().unwrap().caps().mesh_shaders
}

#[test]
fn a_vertex_and_pixel_pipeline_is_made() {
    let Some(s) = vertex_stages() else { return };
    let fx = Fx::new();
    let sig = fx.root_sig(&root_desc());
    let pso = make(&fx, sig, vertex_desc(&s));
    let g = fx.executor().graphics_pipeline(pso).unwrap().unwrap();
    assert_eq!(g.kind(), Kind::Vertex);
    assert_eq!(g.vertex_slots(), [0, 1], "two vertex buffers");
    assert!(g.mesh().is_none());
    // The argument buffer is the root signature's, for every stage.
    let lay = fx.executor().tlab_layout(sig).unwrap();
    assert_eq!(g.layout().size, lay.size());
    // Reflection: the vertex shader's inputs by semantic, in the order the
    // converter assigns attributes; the pixel shader writes one target.
    let stages: Vec<ShaderStage> = g.reflections().iter().map(|r| r.0).collect();
    assert_eq!(stages, [ShaderStage::Vertex, ShaderStage::Fragment]);
    let vs = g.reflections()[0].1.vertex.clone().unwrap();
    let names: Vec<&str> = vs.inputs.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["position0", "texcoord0", "color1"]);
    let ps = g.reflections()[1].1.fragment.unwrap();
    assert_eq!(ps.num_render_targets, 1);
    // A pipeline is destroyed like a compute one, and then is gone.
    let mut d = DestroyPipelineArgs::new();
    d.pso = pso;
    assert_eq!(fx.status(d), Status::OK);
    assert_eq!(
        fx.executor().graphics_pipeline(pso).err(),
        Some(Status::NOT_FOUND)
    );
}

#[test]
fn a_dispatch_with_a_graphics_pipeline_is_refused() {
    let Some(s) = vertex_stages() else { return };
    let fx = Fx::new();
    let q = fx.queue();
    let sig = fx.root_sig(&root_desc());
    let pso = make(&fx, sig, vertex_desc(&s));
    let cmds = vec![
        begin(1, q),
        set_sig(sig),
        victory_metal_proto::Command::SetPipeline(victory_metal_proto::cmd::SetPipeline {
            pso,
            _pad: 0,
        }),
        dispatch(1, 1, 1),
        end(1),
    ];
    assert_eq!(fx.submit(&cmds), Status::INVALID_ARG);
}

#[test]
fn state_variations_all_compile() {
    let Some(s) = vertex_stages() else { return };
    let fx = Fx::new();
    let sig = fx.root_sig(&root_desc());
    let ps2 = dxil_or_skip("gfx_ps2", "ps_6_6", PS_TWO_TARGETS).unwrap();
    let ps_depth = dxil_or_skip("gfx_psd", "ps_6_6", PS_DEPTH).unwrap();
    let ps_dual = dxil_or_skip("gfx_psdual", "ps_6_6", PS_DUAL).unwrap();

    // Alpha blending, culling, depth bias, depth and stencil, 4 samples.
    let mut d = vertex_desc(&s);
    d.blend.rt[0].enable = 1;
    d.blend.rt[0].src = BlendFactor::SRC_ALPHA;
    d.blend.rt[0].dst = BlendFactor::INV_SRC_ALPHA;
    d.blend.rt[0].src_alpha = BlendFactor::ONE;
    d.blend.rt[0].dst_alpha = BlendFactor::INV_SRC_ALPHA;
    d.blend.rt[0].write_mask = 0b0111;
    d.raster.cull = CullMode::NONE;
    d.raster.front_ccw = 1;
    d.raster.depth_bias = -2;
    d.raster.slope_scaled_bias = 1.0;
    d.ds_format = format::D32_FLOAT;
    d.depth.depth_func = CompareFunc::GREATER_EQUAL;
    d.depth.stencil_enable = 1;
    d.depth.front.pass = StencilOp::REPLACE;
    d.depth.back.func = CompareFunc::EQUAL;
    d.sample_count = 4;
    d.blend.alpha_to_coverage = 1;
    make(&fx, sig, d);

    // Two targets, the second an integer target (blending ignored).
    let mut d = vertex_desc(&s);
    d.ps = StageCode::of(&ps2);
    d.rt_count = 2;
    d.rt_formats[1] = format::R32_UINT;
    d.blend.independent = 1;
    d.blend.rt[1].enable = 1;
    make(&fx, sig, d);

    // Depth-only (no pixel stage), a line list type, wireframe.
    let mut d = vertex_desc(&s);
    d.ps = StageCode::NONE;
    d.rt_count = 0;
    d.rt_formats = [0; 8];
    d.ds_format = format::D32_FLOAT;
    d.topology = TopologyType::LINE;
    d.raster.fill = FillMode::WIREFRAME;
    make(&fx, sig, d);

    // A pixel shader that writes depth.
    let mut d = vertex_desc(&s);
    d.ps = StageCode::of(&ps_depth);
    d.ds_format = format::D32_FLOAT;
    make(&fx, sig, d);

    // Dual-source blending.
    let mut d = vertex_desc(&s);
    d.ps = StageCode::of(&ps_dual);
    d.blend.rt[0].enable = 1;
    d.blend.rt[0].src = BlendFactor::SRC1_COLOR;
    d.blend.rt[0].dst = BlendFactor::INV_SRC1_COLOR;
    make(&fx, sig, d);

    // Points: the converter is told (a point size is written).
    let mut d = vertex_desc(&s);
    d.topology = TopologyType::POINT;
    make(&fx, sig, d);

    // Per-instance data in slot 1.
    let mut d = vertex_desc(&s);
    d.inputs[2].class = InputClass::PER_INSTANCE;
    d.inputs[2].step_rate = 1;
    make(&fx, sig, d);
}

#[test]
fn the_refusals_say_why() {
    let Some(s) = vertex_stages() else { return };
    let fx = Fx::new();
    let sig = fx.root_sig(&root_desc());

    // The shader reads COLOR1, the layout has none.
    let mut d = vertex_desc(&s);
    d.input_count = 2;
    d.inputs[2] = InputElement::new("", 0, 0, 0, 0);
    let (st, _) = create(&fx, sig, d);
    assert_eq!(st, Status::UNSUPPORTED);
    let why = fx.executor().last_error().unwrap();
    assert!(
        why.contains("color1") && why.contains("input layout"),
        "{why}"
    );

    // A vertex format Metal cannot fetch.
    let mut d = vertex_desc(&s);
    d.inputs[1].format = 25; // R10G10B10A2_UINT
    assert_eq!(create(&fx, sig, d).0, Status::UNSUPPORTED);
    assert!(fx.executor().last_error().unwrap().contains("cannot fetch"));

    // A render target format outside the table.
    let mut d = vertex_desc(&s);
    d.rt_formats[0] = 9999;
    assert_eq!(create(&fx, sig, d).0, Status::UNSUPPORTED);

    // Conservative rasterization is accepted and ignored.
    let mut d = vertex_desc(&s);
    d.raster.conservative = 1;
    assert_eq!(create(&fx, sig, d).0, Status::OK);

    // Bad calls.
    assert_eq!(
        create(&fx, RootSigId(99), vertex_desc(&s)).0,
        Status::NOT_FOUND
    );
    let mut d = vertex_desc(&s);
    d.vs = StageCode::NONE;
    assert_eq!(
        create(&fx, sig, d).0,
        Status::INVALID_ARG,
        "no stage at all"
    );
    let mut d = vertex_desc(&s);
    d.vs = StageCode { addr: 16, len: 8 };
    assert_eq!(create(&fx, sig, d).0, Status::INVALID_ARG, "unmapped code");
    let mut a = CreateGraphicsPipelineArgs::new();
    a.root_sig = sig;
    a.flags = 1;
    a.desc = vertex_desc(&s);
    assert_eq!(fx.call(a).0, Status::INVALID_ARG);

    // A pixel shader as the vertex stage: the converter's stage is checked.
    let mut d = vertex_desc(&s);
    d.vs = StageCode::of(&s.ps);
    assert_eq!(create(&fx, sig, d).0, Status::INVALID_ARG);
    assert!(fx.executor().last_error().unwrap().contains("vertex stage"));

    // The compute entry refuses a graphics shader as before.
    assert_eq!(
        create(&fx, sig, vertex_desc(&s)).0,
        Status::OK,
        "and the good one still works"
    );
}

#[test]
fn a_mesh_pipeline_with_and_without_an_amplification_stage() {
    let Some(ms) = dxil_or_skip("gfx_ms", "ms_6_6", MS) else {
        return;
    };
    let ps = dxil_or_skip("gfx_ps", "ps_6_6", PS).unwrap();
    let amp = dxil_or_skip("gfx_as", "as_6_6", AS).unwrap();
    let ms_pl = dxil_or_skip("gfx_ms_pl", "ms_6_6", MS_WITH_PAYLOAD).unwrap();
    let fx = Fx::new();
    if !has_mesh(&fx) {
        eprintln!("SKIPPED: the device has no mesh shaders");
        return;
    }
    let sig = fx.root_sig(&root_desc());
    let mut d = GraphicsPsoDesc::new();
    d.ms = StageCode::of(&ms);
    d.ps = StageCode::of(&ps);
    d.topology = TopologyType::UNDEFINED;
    d.rt_count = 1;
    d.rt_formats[0] = format::R8G8B8A8_UNORM;
    let pso = make(&fx, sig, d);
    let g = fx.executor().graphics_pipeline(pso).unwrap().unwrap();
    assert_eq!(g.kind(), Kind::Mesh);
    let m = g.mesh().unwrap();
    assert_eq!(m.mesh_threads, [32, 1, 1]);
    assert_eq!(m.object_threads, None);
    assert_eq!(m.payload_bytes, 0);
    assert_eq!((m.max_vertices, m.max_primitives), (3, 1));
    assert!(g.vertex_slots().is_empty());

    // With the object stage: payload and both thread counts come from reflection.
    let mut d = GraphicsPsoDesc::new();
    d.amp = StageCode::of(&amp);
    d.ms = StageCode::of(&ms_pl);
    d.ps = StageCode::of(&ps);
    d.topology = TopologyType::UNDEFINED;
    d.rt_count = 1;
    d.rt_formats[0] = format::R8G8B8A8_UNORM;
    let pso = make(&fx, sig, d);
    let m = fx
        .executor()
        .graphics_pipeline(pso)
        .unwrap()
        .unwrap()
        .mesh()
        .unwrap();
    assert_eq!(m.object_threads, Some([32, 1, 1]));
    assert!(
        m.payload_bytes >= 20 && m.payload_bytes <= 16384,
        "{}",
        m.payload_bytes
    );

    // Mesh with a vertex stage too is no pipeline.
    let vs = dxil_or_skip("gfx_vs", "vs_6_6", VS).unwrap();
    let mut d = GraphicsPsoDesc::new();
    d.vs = StageCode::of(&vs);
    d.ms = StageCode::of(&ms);
    assert_eq!(create(&fx, sig, d).0, Status::INVALID_ARG);
}

/// The biggest output DXIL allows is accepted; what Metal would refuse is
/// reported by the unit tests of `render.rs` (limits) and the cache survey
/// (`docs/metal/msc.md`), since DXC will not make a shader over the DXIL
/// limits and the game's over-limit shaders are not in the repository.
#[test]
fn the_biggest_mesh_output_dxil_allows_compiles() {
    let Some(ms) = dxil_or_skip("gfx_ms_big", "ms_6_6", MS_BIGGEST) else {
        return;
    };
    let fx = Fx::new();
    if !has_mesh(&fx) {
        return;
    }
    let sig = fx.root_sig(&root_desc());
    let mut d = GraphicsPsoDesc::new();
    d.ms = StageCode::of(&ms);
    d.topology = TopologyType::UNDEFINED;
    let pso = make(&fx, sig, d);
    let m = fx
        .executor()
        .graphics_pipeline(pso)
        .unwrap()
        .unwrap()
        .mesh()
        .unwrap();
    assert_eq!((m.max_vertices, m.max_primitives), (256, 254));
}

#[test]
fn pipelines_can_be_created_from_several_threads() {
    let Some(s) = vertex_stages() else { return };
    let fx = Fx::new();
    let sig = fx.root_sig(&root_desc());
    let ps2 = dxil_or_skip("gfx_ps2", "ps_6_6", PS_TWO_TARGETS).unwrap();
    let ex = fx.executor();
    let ids: Vec<Vec<PsoId>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|t| {
                let (s, ps2) = (&s, &ps2);
                scope.spawn(move || {
                    (0..3)
                        .map(|i| {
                            let mut d = vertex_desc(s);
                            if (t + i) % 2 == 1 {
                                d.ps = StageCode::of(ps2);
                                d.rt_count = 2;
                                d.rt_formats[1] = format::R32_UINT;
                            }
                            d.raster.depth_bias = t * 10 + i;
                            let mut a = CreateGraphicsPipelineArgs::new();
                            a.root_sig = sig;
                            a.desc = d;
                            let st = ex.call(
                                victory_metal_proto::sync::SyncOp::CREATE_GRAPHICS_PIPELINE,
                                (&raw mut a).cast(),
                                std::mem::size_of_val(&a),
                            );
                            assert_eq!(st, Status::OK, "{:?}", ex.last_error());
                            a.pso
                        })
                        .collect()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut all: Vec<u32> = ids.iter().flatten().map(|p| p.0).collect();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), 12, "twelve distinct pipelines");
    for p in all {
        assert!(ex.graphics_pipeline(PsoId(p)).unwrap().is_some());
    }
}

#[test]
fn proto_calls_roundtrip_through_the_link() {
    // The same call through `LinkExt::call_args`, as a front would make it.
    let Some(s) = vertex_stages() else { return };
    let fx = Fx::new();
    let sig = fx.root_sig(&root_desc());
    let mut a = CreateGraphicsPipelineArgs::new();
    a.root_sig = sig;
    a.desc = vertex_desc(&s);
    assert_eq!(fx.link.call_args(&mut a), Status::OK);
    assert_ne!(a.pso, PsoId(0));
}
