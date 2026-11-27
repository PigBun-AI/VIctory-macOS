//! `GetAttributeAtVertex` on the Metal chain: the research prototype of the
//! `attr` branch (docs/metal/attribute-at-vertex.md).
//!
//! MSC 4.0.3 refuses `dx.op.attributeAtVertex` (`msc_features.rs`,
//! `gfx_pipeline.rs`). This file proves the lowering that does not need it:
//! the pixel stage reads the three vertices' values of a constant input as
//! three flat inputs (vertex 0 is the input itself, the provoking vertex),
//! and the mesh stage exchanges the values of a primitive's three vertices
//! through groupshared memory and writes the other two as per-primitive
//! outputs. A pixel shader that weights the three values with
//! `SV_Barycentrics` (what the translator emits for computed barycentrics,
//! `emit/lower.rs`, `per_vertex`) is drawn on the GPU and compared with a CPU
//! evaluation of the same weights, for primitives whose index order differs
//! (so the provoking vertex is not always vertex 0 of the vertex buffer).
//! Without a DXC front end the tests print `SKIPPED` and pass.
#![cfg(target_vendor = "apple")]

mod common_msc;
mod exec_support;

use common_msc::dxil_or_skip;
use exec_support::*;
use victory_metal_proto::cmd::*;
use victory_metal_proto::ids::*;
use victory_metal_proto::pso::*;
use victory_metal_proto::render::*;
use victory_metal_proto::root::*;
use victory_metal_proto::status::Status;
use victory_metal_proto::sync::{CreateGraphicsPipelineArgs, CreateTextureArgs};
use victory_metal_proto::types::*;
use victory_metal_proto::wire::Wire;

const W: u32 = 32;
const H: u32 = 32;

/// Two triangles, six vertices; the second primitive's index order is
/// rotated, so its provoking vertex is vertex 4.
const POS: [[f32; 2]; 6] = [
    [-0.93, -0.91],
    [0.07, -0.87],
    [-0.89, 0.11],
    [0.87, 0.95],
    [0.11, 0.13],
    [0.93, 0.09],
];
const COL: [[f32; 4]; 6] = [
    [1.0, 0.0, 0.0, 1.0],
    [0.0, 1.0, 0.0, 1.0],
    [0.0, 0.0, 1.0, 1.0],
    [1.0, 1.0, 0.0, 1.0],
    [0.0, 1.0, 1.0, 1.0],
    [1.0, 0.0, 1.0, 1.0],
];
const INDICES: [[usize; 3]; 2] = [[0, 1, 2], [4, 5, 3]];

/// The mesh stage after the lowering: `c0` is the vertex's own value (flat:
/// the provoking vertex's), `c1` and `c2` the values of the primitive's
/// second and third vertex, exchanged through groupshared memory.
const MS_LOWERED: &str = "\
struct V { float4 pos : SV_Position; nointerpolation float4 c0 : COLOR0; };\n\
struct P { nointerpolation float4 c1 : COLOR1; nointerpolation float4 c2 : COLOR2; };\n\
groupshared float4 gc[6];\n\
[numthreads(32,1,1)] [outputtopology(\"triangle\")]\n\
void main(uint tid : SV_GroupIndex, out vertices V verts[6], out primitives P prims[2], out indices uint3 tris[2]) {\n\
  SetMeshOutputCounts(6, 2);\n\
  float2 p[6] = { float2(-0.93,-0.91), float2(0.07,-0.87), float2(-0.89,0.11), float2(0.87,0.95), float2(0.11,0.13), float2(0.93,0.09) };\n\
  float4 c[6] = { float4(1,0,0,1), float4(0,1,0,1), float4(0,0,1,1), float4(1,1,0,1), float4(0,1,1,1), float4(1,0,1,1) };\n\
  if (tid < 6) { verts[tid].pos = float4(p[tid], 0.5, 1); verts[tid].c0 = c[tid]; gc[tid] = c[tid]; }\n\
  uint3 idx[2] = { uint3(0,1,2), uint3(4,5,3) };\n\
  GroupMemoryBarrierWithGroupSync();\n\
  if (tid < 2) { tris[tid] = idx[tid]; prims[tid].c1 = gc[idx[tid].y]; prims[tid].c2 = gc[idx[tid].z]; }\n}\n";

/// The pixel stage after the lowering: three flat inputs, weighted with the
/// barycentrics exactly as `P0 + i (P1 - P0) + j (P2 - P0)` would.
const PS_LOWERED: &str = "\
struct PSIn { float4 pos : SV_Position; nointerpolation float4 c0 : COLOR0; nointerpolation float4 c1 : COLOR1;\n\
  nointerpolation float4 c2 : COLOR2; float3 bary : SV_Barycentrics; };\n\
float4 main(PSIn i) : SV_Target0 { return i.c0 * i.bary.x + i.c1 * i.bary.y + i.c2 * i.bary.z; }\n";

fn pack(c: [f32; 4]) -> u32 {
    let b = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
    b(c[0]) | (b(c[1]) << 8) | (b(c[2]) << 16) | (b(c[3]) << 24)
}

fn channel(p: u32, i: u32) -> i32 {
    ((p >> (8 * i)) & 255) as i32
}

/// Barycentric weights of the pixel centre in the triangle `p` (NDC), or
/// `None` outside / within `margin` of an edge (the fill rule and the
/// rasteriser's rounding stay out of the comparison).
fn weights(p: [[f32; 2]; 3], px: u32, py: u32, margin: f64) -> Option<[f64; 3]> {
    let x = (f64::from(px) + 0.5) / f64::from(W) * 2.0 - 1.0;
    let y = 1.0 - (f64::from(py) + 0.5) / f64::from(H) * 2.0;
    let q = |i: usize| (f64::from(p[i][0]), f64::from(p[i][1]));
    let (a, b, c) = (q(0), q(1), q(2));
    let det = (b.1 - c.1) * (a.0 - c.0) + (c.0 - b.0) * (a.1 - c.1);
    let w0 = ((b.1 - c.1) * (x - c.0) + (c.0 - b.0) * (y - c.1)) / det;
    let w1 = ((c.1 - a.1) * (x - c.0) + (a.0 - c.0) * (y - c.1)) / det;
    let w = [w0, w1, 1.0 - w0 - w1];
    if w.iter().all(|v| *v > margin) {
        Some(w)
    } else if w.iter().any(|v| *v > -margin && *v < margin) {
        None
    } else {
        Some([f64::NAN; 3]) // clearly outside
    }
}

/// The expected pixels: `None` where the comparison is skipped (near an
/// edge), the background where no triangle covers, otherwise the weighted
/// values of the primitive's vertices in index order.
fn reference(bg: [f32; 4]) -> Vec<Option<u32>> {
    let mut out = vec![Some(pack(bg)); (W * H) as usize];
    for py in 0..H {
        for px in 0..W {
            for idx in INDICES {
                let p = [POS[idx[0]], POS[idx[1]], POS[idx[2]]];
                match weights(p, px, py, 0.03) {
                    None => out[(py * W + px) as usize] = None,
                    Some(w) if w[0].is_nan() => {}
                    Some(w) => {
                        let mut c = [0.0f32; 4];
                        for ch in 0..4 {
                            c[ch] = (0..3).map(|k| w[k] as f32 * COL[idx[k]][ch]).sum();
                        }
                        out[(py * W + px) as usize] = Some(pack(c));
                    }
                }
            }
        }
    }
    out
}

struct Gfx {
    fx: Fx,
    q: QueueId,
    fence: FenceId,
    sig: RootSigId,
}

impl Gfx {
    fn new() -> Self {
        let fx = Fx::new();
        let q = fx.queue();
        let fence = fx.fence(0);
        let sig = fx.root_sig(&RootSigDesc::default());
        Gfx { fx, q, fence, sig }
    }

    fn settle(&self) {
        let v = self.fx.fence_value(self.fence) + 1;
        assert_eq!(
            self.fx.submit(&[signal_cmd(self.q, self.fence, v)]),
            Status::OK
        );
        assert_eq!(self.fx.wait(self.fence, v), Status::OK);
    }

    fn texture(&self) -> ResId {
        let mut a = CreateTextureArgs::new();
        a.dimension = TexDim::TEX2D;
        a.format = format::R8G8B8A8_UNORM;
        a.width = W;
        a.height = H;
        a.depth_or_array = 1;
        a.mips = 1;
        a.samples = 1;
        a.usage = tex_usage::RENDER_TARGET;
        self.fx.ok(a).res
    }

    fn mesh_pso(&self, ms: &[u8], ps: &[u8]) -> PsoId {
        let mut d = GraphicsPsoDesc::new();
        d.ms = StageCode::of(ms);
        d.ps = StageCode::of(ps);
        d.rt_count = 1;
        d.rt_formats[0] = format::R8G8B8A8_UNORM;
        d.raster.cull = CullMode::NONE;
        d.depth.depth_enable = 0;
        d.depth.depth_write = 0;
        let mut a = CreateGraphicsPipelineArgs::new();
        a.root_sig = self.sig;
        a.desc = d;
        let (s, a) = self.fx.call(a);
        assert_eq!(s, Status::OK, "{:?}", self.fx.executor().last_error());
        a.pso
    }

    /// Draws the mesh pipeline into a fresh target and reads it back.
    fn draw_and_read(&self, pso: PsoId, bg: [f32; 4]) -> Vec<u32> {
        let t = self.texture();
        let rb = self.fx.buffer(BufferKind::READBACK, (W * H * 4) as u64);
        cpu_write(rb.cpu_addr, &vec![0xDEAD_BEEF; (W * H) as usize]);
        let mut pass = <BeginRenderPass as Wire>::ZERO;
        pass.rt_count = 1;
        pass.rt[0] = RenderAttachment {
            res: t,
            format: format::R8G8B8A8_UNORM,
            dimension: TexViewDim::TEX2D,
            mip: 0,
            load: LoadAction::CLEAR,
            _pad: 0,
            first_slice: 0,
            slice_count: 1,
            clear: bg,
        };
        let mut loc = <TextureLocation as Wire>::ZERO;
        loc.res = t;
        loc.kind = LocKind::SUBRESOURCE;
        let mut foot = <TextureLocation as Wire>::ZERO;
        foot.res = rb.res;
        foot.kind = LocKind::FOOTPRINT;
        foot.format = format::R8G8B8A8_UNORM;
        foot.row_pitch = W * 4;
        foot.width = W;
        foot.height = H;
        foot.depth = 1;
        let mut copy = <CopyTextureRegion as Wire>::ZERO;
        copy.dst = foot;
        copy.src = loc;
        let cmds = vec![
            begin(1, self.q),
            Command::SetRootSignature(SetRootSignature {
                bind_point: BindPoint::GRAPHICS,
                root_sig: self.sig,
            }),
            Command::SetPipeline(SetPipeline { pso, _pad: 0 }),
            Command::SetViewports(SetViewports::from_slice(&[Viewport::new(
                0.0, 0.0, W as f32, H as f32,
            )])),
            Command::SetScissors(SetScissors::from_slice(&[Rect::new(
                0, 0, W as i32, H as i32,
            )])),
            Command::BeginRenderPass(pass),
            Command::DispatchMesh(DispatchMesh {
                x: 1,
                y: 1,
                z: 1,
                _pad: 0,
            }),
            Command::EndRenderPass(<EndRenderPass as Wire>::ZERO),
            Command::Barrier(Barrier::memory(stage::FRAGMENT, stage::BLIT)),
            Command::CopyTextureRegion(copy),
            end(1),
        ];
        assert_eq!(
            self.fx.submit(&cmds),
            Status::OK,
            "{:?}",
            self.fx.executor().last_error()
        );
        self.settle();
        cpu_read(rb.cpu_addr, (W * H) as usize)
    }
}

#[test]
fn the_three_vertex_values_reach_the_pixel_stage_as_flat_inputs() {
    let Some(ms) = dxil_or_skip("attr_ms", "ms_6_6", MS_LOWERED) else {
        return;
    };
    let Some(ps) = dxil_or_skip("attr_ps", "ps_6_6", PS_LOWERED) else {
        return;
    };
    let g = Gfx::new();
    if !g.fx.executor().device().unwrap().caps().mesh_shaders {
        eprintln!("SKIPPED: no mesh shaders on this device");
        return;
    }
    let bg = [0.0, 0.0, 0.0, 1.0];
    let got = g.draw_and_read(g.mesh_pso(&ms, &ps), bg);
    let want = reference(bg);
    let (mut compared, mut painted) = (0, 0);
    for py in 0..H {
        for px in 0..W {
            let i = (py * W + px) as usize;
            let Some(w) = want[i] else { continue };
            compared += 1;
            if w != pack(bg) {
                painted += 1;
            }
            assert!(
                (0..4).all(|c| (channel(got[i], c) - channel(w, c)).abs() <= 3),
                "pixel ({px},{py}) is {:#010x}, wanted {w:#010x}",
                got[i]
            );
        }
    }
    assert!(
        painted > 150,
        "the triangles must cover many pixels ({painted})"
    );
    assert!(compared > 700);
}

#[test]
fn the_first_value_is_the_provoking_vertex_of_the_index_order() {
    // Only `c0` (flat) is read: for the second primitive, whose indices are
    // (4, 5, 3), it must be vertex 4's value, not vertex 3's (the last
    // vertex in the buffer) and not vertex 5's.
    const PS_FIRST: &str = "\
struct PSIn { float4 pos : SV_Position; nointerpolation float4 c0 : COLOR0; nointerpolation float4 c1 : COLOR1; nointerpolation float4 c2 : COLOR2; };\n\
float4 main(PSIn i) : SV_Target0 { return i.c0 + 0 * (i.c1 + i.c2); }\n";
    let Some(ms) = dxil_or_skip("attr_ms2", "ms_6_6", MS_LOWERED) else {
        return;
    };
    let Some(ps) = dxil_or_skip("attr_ps2", "ps_6_6", PS_FIRST) else {
        return;
    };
    let g = Gfx::new();
    if !g.fx.executor().device().unwrap().caps().mesh_shaders {
        eprintln!("SKIPPED: no mesh shaders on this device");
        return;
    }
    let bg = [0.0, 0.0, 0.0, 1.0];
    let got = g.draw_and_read(g.mesh_pso(&ms, &ps), bg);
    let want = reference(bg);
    for (idx, prov) in [(0usize, 0usize), (1, 4)] {
        let p = [
            POS[INDICES[idx][0]],
            POS[INDICES[idx][1]],
            POS[INDICES[idx][2]],
        ];
        let mut seen = 0;
        for py in 0..H {
            for px in 0..W {
                if weights(p, px, py, 0.03).is_some_and(|w| !w[0].is_nan()) {
                    seen += 1;
                    assert_eq!(
                        got[(py * W + px) as usize],
                        pack(COL[prov]),
                        "primitive {idx}: pixel ({px},{py})"
                    );
                }
            }
        }
        assert!(seen > 50, "primitive {idx} covers {seen} pixels");
    }
    let _ = want;
}
