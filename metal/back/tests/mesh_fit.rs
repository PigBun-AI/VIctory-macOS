//! A mesh shader whose declared output is over Metal's 32 KB total
//! (docs/metal/mesh32k.md): HLSL through DXC, the repository's chain, Metal 4.
//!
//! The shader is synthetic (no game data): `PAIRS` alternating `float` and
//! `float3` outputs, so that the padding Metal puts between members (each is
//! aligned to its own size) decides whether 128 vertices fit. Pictures are
//! compared with a CPU rasterisation of the triangle; the colour is made of
//! values that only the right members carry, so a pixel shader reading a
//! wrong member shows.
#![cfg(target_vendor = "apple")]

mod common_msc;
mod exec_support;

use common_msc::dxil_or_skip;
use exec_support::*;
use victory_metal_back::mesh_fit;
use victory_metal_back::mesh_size;
use victory_metal_back::msc::RootSignatureDesc;
use victory_metal_back::pipeline_defaults::{Component, Stages, Variant, default_graphics};
use victory_metal_back::render::GraphicsDesc;
use victory_metal_proto::cmd::*;
use victory_metal_proto::ids::*;
use victory_metal_proto::pso::*;
use victory_metal_proto::render::*;
use victory_metal_proto::root::*;
use victory_metal_proto::status::Status;
use victory_metal_proto::sync::{CreateGraphicsPipelineArgs, CreateTextureArgs};
use victory_metal_proto::types::*;
use victory_metal_proto::wire::Wire;

const W: u32 = 16;
const H: u32 = 16;
/// Pairs of `float a<i>` (TEXCOORD 2i) and `float3 b<i>` (TEXCOORD 2i+1).
const PAIRS: usize = 11;

#[derive(Clone, Copy)]
enum Order {
    /// a0 b0 a1 b1 ...: each `float3` lands 4 bytes past its 12-byte slot.
    Alternating,
    /// b0 b1 ... a0 a1 ...: 8 bytes of padding, in front of the first.
    Packed,
}

fn value(slot: usize) -> f32 {
    0.01 * (slot + 1) as f32
}

/// The mesh shader: 128 vertices and 128 primitives declared, one triangle
/// written; member values are distinct.
fn mesh_hlsl(order: Order) -> String {
    mesh_hlsl_of(order, PAIRS)
}

fn mesh_hlsl_of(order: Order, pairs: usize) -> String {
    let a = |i: usize| {
        (
            format!("float a{i} : TEXCOORD{};\n", 2 * i),
            format!("    verts[tid].a{i} = {:.3};\n", value(2 * i)),
        )
    };
    let b = |i: usize| {
        let s = 2 * i + 1;
        (
            format!("float3 b{i} : TEXCOORD{s};\n"),
            format!(
                "    verts[tid].b{i} = float3({:.3}, {:.3}, {:.3});\n",
                value(s),
                2.0 * value(s),
                3.0 * value(s)
            ),
        )
    };
    let seq: Vec<(String, String)> = match order {
        Order::Alternating => (0..pairs).flat_map(|i| [a(i), b(i)]).collect(),
        Order::Packed => (0..pairs).map(b).chain((0..pairs).map(a)).collect(),
    };
    let members: String = seq.iter().map(|(m, _)| format!("  {m}")).collect();
    let writes: String = seq.iter().map(|(_, w)| w.clone()).collect();
    format!(
        "struct V {{\n  float4 pos : SV_Position;\n{members}}};\n\
[numthreads(32,1,1)] [outputtopology(\"triangle\")]\n\
void main(uint tid : SV_GroupIndex, out vertices V verts[128], out indices uint3 tris[128]) {{\n\
  SetMeshOutputCounts(3, 1);\n\
  if (tid < 3) {{\n\
    float2 p[3] = {{ float2(-1,-1), float2(1.05,-1), float2(-1,1.05) }};\n\
    verts[tid].pos = float4(p[tid], 0.5, 1);\n{writes}  }}\n\
  if (tid == 0) tris[0] = uint3(0,1,2);\n}}\n"
    )
}

/// Reads `b3.x`, `a5` and `b8.z`.
const PS: &str = "struct V {\n  float4 pos : SV_Position;\n  float a5 : TEXCOORD10;\n  float3 b3 : TEXCOORD7;\n  float3 b8 : TEXCOORD17;\n};\n\
float4 main(V i) : SV_Target0 { return float4(i.b3.x * 5, i.a5 * 5, i.b8.z, 1); }\n";

/// What the pixel shader makes of those members.
fn colour() -> [f32; 4] {
    [value(7) * 5.0, value(10) * 5.0, 3.0 * value(17), 1.0]
}

fn pack(c: [f32; 4]) -> u32 {
    let b = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
    b(c[0]) | (b(c[1]) << 8) | (b(c[2]) << 16) | (b(c[3]) << 24)
}

fn close(a: u32, b: u32, tol: i32) -> bool {
    (0..4).all(|i| {
        let (x, y) = (((a >> (8 * i)) & 255) as i32, ((b >> (8 * i)) & 255) as i32);
        (x - y).abs() <= tol
    })
}

const TRI: [[f32; 2]; 3] = [[-1.0, -1.0], [1.05, -1.0], [-1.0, 1.05]];

/// The CPU picture: pixel centres, edge functions. No centre is on an edge
/// for this triangle (checked).
fn reference(bg: [f32; 4], c: [f32; 4]) -> Vec<u32> {
    let mut out = vec![pack(bg); (W * H) as usize];
    for py in 0..H {
        for px in 0..W {
            let x = (f64::from(px) + 0.5) / f64::from(W) * 2.0 - 1.0;
            let y = 1.0 - (f64::from(py) + 0.5) / f64::from(H) * 2.0;
            let e: Vec<f64> = [(0, 1), (1, 2), (2, 0)]
                .iter()
                .map(|&(a, b)| {
                    let (a, b) = (TRI[a], TRI[b]);
                    (f64::from(b[0]) - f64::from(a[0])) * (y - f64::from(a[1]))
                        - (f64::from(b[1]) - f64::from(a[1])) * (x - f64::from(a[0]))
                })
                .collect();
            assert!(
                e.iter().all(|e| e.abs() > 1e-4),
                "pixel ({px},{py}) is on an edge"
            );
            if e.iter().all(|e| *e >= 0.0) || e.iter().all(|e| *e <= 0.0) {
                out[(py * W + px) as usize] = pack(c);
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

fn setup() -> Option<Gfx> {
    let fx = Fx::new();
    if !fx.executor().device().unwrap().caps().mesh_shaders {
        eprintln!("SKIPPED: no mesh shaders on this device");
        return None;
    }
    let q = fx.queue();
    let fence = fx.fence(0);
    let sig = fx.root_sig(&RootSigDesc::default());
    Some(Gfx { fx, q, fence, sig })
}

impl Gfx {
    fn settle(&self) {
        let v = self.fx.fence_value(self.fence) + 1;
        assert_eq!(
            self.fx.submit(&[signal_cmd(self.q, self.fence, v)]),
            Status::OK
        );
        assert_eq!(self.fx.wait(self.fence, v), Status::OK);
    }

    /// The pipeline of a mesh and a pixel shader, or the executor's refusal.
    fn pso(&self, ms: &[u8], ps: &[u8]) -> Result<PsoId, (Status, String)> {
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
        if s == Status::OK {
            Ok(a.pso)
        } else {
            Err((s, self.fx.executor().last_error().unwrap_or_default()))
        }
    }

    /// Draws one mesh group into a cleared target and reads it back.
    fn draw(&self, pso: PsoId, bg: [f32; 4]) -> Vec<u32> {
        let mut a = CreateTextureArgs::new();
        a.dimension = TexDim::TEX2D;
        a.format = format::R8G8B8A8_UNORM;
        a.width = W;
        a.height = H;
        a.depth_or_array = 1;
        a.mips = 1;
        a.samples = 1;
        a.usage = tex_usage::RENDER_TARGET;
        let tex = self.fx.ok(a).res;
        let rb = self.fx.buffer(BufferKind::READBACK, 4096);
        cpu_write(rb.cpu_addr, &[0xDEAD_BEEF; (W * H) as usize]);
        let mut pass = <BeginRenderPass as Wire>::ZERO;
        pass.rt_count = 1;
        pass.rt[0] = RenderAttachment {
            res: tex,
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
        loc.res = tex;
        loc.kind = LocKind::SUBRESOURCE;
        let mut foot = <TextureLocation as Wire>::ZERO;
        foot.res = rb.res;
        foot.kind = LocKind::FOOTPRINT;
        foot.format = format::R8G8B8A8_UNORM;
        foot.row_pitch = W * 4;
        foot.width = W;
        foot.height = H;
        foot.depth = 1;
        let mut c = <CopyTextureRegion as Wire>::ZERO;
        c.dst = foot;
        c.src = loc;
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
            Command::Barrier(Barrier::memory(
                stage::FRAGMENT | stage::VERTEX,
                stage::BLIT,
            )),
            Command::CopyTextureRegion(c),
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

fn assert_image(got: &[u32], want: &[u32], what: &str) {
    for py in 0..H {
        for px in 0..W {
            let i = (py * W + px) as usize;
            assert!(
                close(got[i], want[i], 1),
                "{what}: pixel ({px},{py}) is {:#010x}, wanted {:#010x}",
                got[i],
                want[i]
            );
        }
    }
}

const BG: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

fn shaders() -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let alt = dxil_or_skip("fit_alt", "ms_6_6", &mesh_hlsl(Order::Alternating))?;
    let packed = dxil_or_skip("fit_packed", "ms_6_6", &mesh_hlsl(Order::Packed))?;
    let ps = dxil_or_skip("fit_ps", "ps_6_6", PS)?;
    Some((alt, packed, ps))
}

/// The proof of the refusal, below the executor (which repacks): Metal says
/// no to the alternating order, yes to the packed one, and the second try
/// of `mesh_fit::retry` makes the first a pipeline.
#[test]
fn metal_refuses_the_padded_order_and_the_repacked_one_is_made() {
    let Some((alt, packed, ps)) = shaders() else {
        return;
    };
    let Some(fx) = Some(Fx::new()) else { return };
    let device = fx.executor().device().unwrap();
    if !device.caps().mesh_shaders {
        eprintln!("SKIPPED: no mesh shaders on this device");
        return;
    }
    let root = RootSignatureDesc::default();
    let desc = |ms: &[u8]| -> GraphicsDesc<'_> {
        // (lifetimes: the shaders outlive the descriptions)
        let ms: &'static [u8] = Box::leak(ms.to_vec().into_boxed_slice());
        let ps: &'static [u8] = Box::leak(ps.clone().into_boxed_slice());
        let root: &'static RootSignatureDesc = Box::leak(Box::new(root.clone()));
        default_graphics(
            root,
            Stages {
                vs: None,
                ps: Some(ps),
                ms: Some(ms),
                gs: None,
            },
            &[],
            &[Component::Float],
            false,
            Variant::Plain,
        )
    };
    let d = desc(&alt);
    let refused = device
        .graphics_pipeline_from_dxil(&d)
        .err()
        .expect("over 32 KB");
    let text = refused.to_string();
    assert!(mesh_fit::is_size_refusal(&refused), "{text}");
    // 128 x 276 = 35328: what the model says too.
    assert!(text.contains("35328"), "{text}");
    assert!(
        device.graphics_pipeline_from_dxil(&desc(&packed)).is_ok(),
        "the packed order fits"
    );
    // Nothing else is retried: the packed order is not repacked, and a refusal
    // of another kind is left alone.
    let other = victory_metal_back::msc::MscError::Limit(
        "mesh pipeline over a Metal limit: payload".into(),
    );
    assert!(mesh_fit::retry(&device, &d, &other).is_none());
    let before = mesh_fit::repacked();
    let made = mesh_fit::retry(&device, &d, &refused).expect("repackable");
    assert!(made.is_ok(), "{:?}", made.err());
    assert!(mesh_fit::repacked() > before);
}

#[test]
fn the_repacked_container_is_the_same_shader_in_another_order() {
    let Some((alt, packed, _)) = shaders() else {
        return;
    };
    let fit = mesh_fit::repack_outputs(&alt, None)
        .unwrap()
        .expect("repacked");
    assert_eq!((fit.stride_before, fit.stride_after), (276, 192));
    assert_eq!(fit.elements, 2 * PAIRS + 1);
    // one call per component: the position's four, a float, three of a float3
    assert_eq!(fit.stores, 4 + PAIRS * 4);
    // The same container but for the bitcode: same parts, same part sizes
    // but the DXIL part's.
    assert_eq!(&fit.dxil[..4], b"DXBC");
    assert_eq!(fit.dxil.len() % 4, 0);
    // Again: nothing more to gain; and the packed one is as it should be.
    assert!(mesh_fit::repack_outputs(&fit.dxil, None).unwrap().is_none());
    // The `float3`s first still leaves 8 bytes in front of the first.
    let other = mesh_fit::repack_outputs(&packed, None)
        .unwrap()
        .expect("8 bytes to gain");
    assert_eq!((other.stride_before, other.stride_after), (200, 192));
    // Repacked, the shader's total is what Metal takes.
    assert!(128 * u64::from(fit.stride_after) <= mesh_size::LIMIT);
    assert!(128 * u64::from(fit.stride_before) > mesh_size::LIMIT);
}

#[test]
fn what_is_not_a_repackable_mesh_shader_is_left_alone() {
    let Some((alt, _, ps)) = shaders() else {
        return;
    };
    // A pixel shader has no mesh outputs.
    assert!(mesh_fit::repack_outputs(&ps, None).unwrap().is_none());
    // Not a container, and one cut short.
    assert!(matches!(
        mesh_fit::repack_outputs(b"DXBX0000", None),
        Err(mesh_fit::FitError::Malformed(_))
    ));
    assert!(mesh_fit::repack_outputs(&alt[..alt.len() / 2], None).is_err());
    // Bitcode damaged inside a container that is whole.
    let mut broken = alt.clone();
    // (the container has a second copy in its reflection part, the DXIL part's is the last)
    let at = broken
        .windows(4)
        .rposition(|w| w == [b'B', b'C', 0xC0, 0xDE])
        .unwrap();
    broken[at + 1] = b'X';
    assert!(mesh_fit::repack_outputs(&broken, None).is_err());
}

/// The picture, through the executor (which retries what Metal refuses): the
/// alternating order is drawn, and equals the CPU's and the packed order's.
#[test]
fn the_repacked_shader_draws_what_the_cpu_draws_and_what_the_packed_order_draws() {
    let Some((alt, packed, ps)) = shaders() else {
        return;
    };
    let Some(g) = setup() else { return };
    let want = reference(BG, colour());
    let pso = g.pso(&alt, &ps).expect("repacked by the executor");
    assert_image(&g.draw(pso, BG), &want, "repacked");
    let pso = g.pso(&packed, &ps).expect("fits");
    assert_image(&g.draw(pso, BG), &want, "packed");
}

/// Twenty `float3` outputs: 256 bytes a vertex for DXIL (which DXC accepts),
/// 264 for Metal (the position is 16 bytes and a `float3` is aligned to 12:
/// 8 bytes of padding that no order removes). Only dropping what the pixel
/// shader does not read fits.
const BIG_PS: &str = "struct V {\n  float4 pos : SV_Position;\n  float3 c3 : TEXCOORD3;\n  float3 c10 : TEXCOORD10;\n  float3 c17 : TEXCOORD17;\n};\n\
float4 main(V i) : SV_Target0 { return float4(i.c3.x * 5, i.c10.y * 5, i.c17.z, 1); }\n";

fn big_hlsl() -> String {
    let members: String = (0..20)
        .map(|i| format!("  float3 c{i} : TEXCOORD{i};\n"))
        .collect();
    let writes: String = (0..20)
        .map(|i| {
            let v = value(i);
            format!(
                "    verts[tid].c{i} = float3({v:.3}, {:.3}, {:.3});\n",
                2.0 * v,
                3.0 * v
            )
        })
        .collect();
    format!(
        "struct V {{\n  float4 pos : SV_Position;\n{members}}};\n\
[numthreads(32,1,1)] [outputtopology(\"triangle\")]\n\
void main(uint tid : SV_GroupIndex, out vertices V verts[128], out indices uint3 tris[128]) {{\n\
  SetMeshOutputCounts(3, 1);\n\
  if (tid < 3) {{\n\
    float2 p[3] = {{ float2(-1,-1), float2(1.05,-1), float2(-1,1.05) }};\n\
    verts[tid].pos = float4(p[tid], 0.5, 1);\n{writes}  }}\n\
  if (tid == 0) tris[0] = uint3(0,1,2);\n}}\n"
    )
}

#[test]
fn outputs_the_pixel_shader_does_not_read_are_dropped_when_packing_is_not_enough() {
    let (Some(ms), Some(ps)) = (
        dxil_or_skip("fit_big", "ms_6_6", &big_hlsl()),
        dxil_or_skip("fit_big_ps", "ps_6_6", BIG_PS),
    ) else {
        return;
    };
    // Packing alone: nothing to gain (all members are alike).
    assert!(mesh_fit::repack_outputs(&ms, None).unwrap().is_none());
    let fit = mesh_fit::repack_outputs(&ms, Some(&ps))
        .unwrap()
        .expect("repacked");
    assert_eq!(fit.dropped, 17);
    assert_eq!((fit.stride_before, fit.stride_after), (264, 16 + 8 + 36));
    assert!(
        128 * u64::from(fit.stride_before) > mesh_size::LIMIT,
        "Metal refuses it as it is"
    );
    // What is left is what the pixel shader reads, and nothing else changed.
    assert!(
        mesh_fit::repack_outputs(&fit.dxil, Some(&ps))
            .unwrap()
            .is_none()
    );
    // Through the executor, which finds out by itself: the picture is the CPU's.
    let Some(g) = setup() else { return };
    let (s, why) = g
        .pso(&ms, &ps)
        .err()
        .map(|e| (e.0, e.1))
        .unwrap_or((Status::OK, String::new()));
    assert_eq!(s, Status::OK, "{why}");
    let pso = g.pso(&ms, &ps).unwrap();
    let want = reference(
        BG,
        [value(3) * 5.0, 2.0 * value(10) * 5.0, 3.0 * value(17), 1.0],
    );
    assert_image(&g.draw(pso, BG), &want, "pruned");
}
