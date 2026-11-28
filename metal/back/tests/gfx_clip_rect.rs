//! Node S-M-003, the fix (mac S4): a rectangle list whose vertex shader writes `SV_ClipDistance` is
//! drawn, and the pixels are the ones D3D12's clipping leaves.
//!
//! Metal Shader Converter 4.0.3 cannot make the geometry emulation of a pipeline whose geometry stage
//! writes a clip distance (`gfx_clip.rs`), so the translator writes the vertex shader's clip distances as
//! a plain output (`VICTORYCLIP`) and its rectangle stage (`victory_shader::emit::rect_list_gs`) does the
//! clipping: the rectangle's parallelogram is cut by each plane and goes out as one strip. The vertex and
//! pixel shaders here are HLSL through the DXC front end (`SKIPPED` without one); the geometry stage is the
//! translator's own. Every result is read back and compared with a CPU rasterisation of the rectangle
//! (or triangle) intersected with the half planes `d >= 0` of the interpolated distances, with the pixel
//! colour an attribute that the stage continues into the fourth corner.
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

const W: u32 = 16;
const H: u32 = 16;
const STRIDE: u32 = 28;

/// The vertex shader as the translator writes it for a rectangle list on the Metal chain: the clip
/// distances (here the vertex colour's four channels) as the plain output `VICTORYCLIP`.
const VS: &str = "struct VSIn { float3 pos : POSITION; float4 d : COLOR0; };\n\
struct VSOut { float4 pos : SV_Position; float4 col : TEXCOORD0; float4 clip : VICTORYCLIP; };\n\
VSOut main(VSIn i) { VSOut o; o.pos = float4(i.pos, 1); o.col = float4(i.pos.xy * 0.5 + 0.5, 0.25, 1); o.clip = i.d; return o; }\n";

const PS: &str = "struct VSOut { float4 pos : SV_Position; float4 col : TEXCOORD0; };\n\
float4 main(VSOut i) : SV_Target0 { return i.col; }\n";

/// An affine clip distance `a x + b y + c` (NDC).
type Plane = [f32; 3];

fn dist(p: Plane, x: f64, y: f64) -> f64 {
    f64::from(p[0]) * x + f64::from(p[1]) * y + f64::from(p[2])
}

fn centre(px: u32, py: u32) -> (f64, f64) {
    (
        (f64::from(px) + 0.5) / f64::from(W) * 2.0 - 1.0,
        1.0 - (f64::from(py) + 0.5) / f64::from(H) * 2.0,
    )
}

fn pack(c: [f64; 4]) -> u32 {
    let b = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
    b(c[0]) | (b(c[1]) << 8) | (b(c[2]) << 16) | (b(c[3]) << 24)
}

fn close(a: u32, b: u32, tol: i32) -> bool {
    (0..4).all(|i| {
        let (x, y) = (((a >> (8 * i)) & 255) as i32, ((b >> (8 * i)) & 255) as i32);
        (x - y).abs() <= tol
    })
}

/// The pixel colour the pixel shader returns at NDC `(x, y)`.
fn colour(x: f64, y: f64) -> u32 {
    pack([x * 0.5 + 0.5, y * 0.5 + 0.5, 0.25, 1.0])
}

struct Fxs {
    fx: Fx,
    q: QueueId,
    fence: FenceId,
    sig: RootSigId,
    vs: Vec<u8>,
    ps: Vec<u8>,
    gs: Vec<u8>,
}

fn setup() -> Option<Fxs> {
    let vs = dxil_or_skip("clip_rect_vs", "vs_6_6", VS)?;
    let ps = dxil_or_skip("clip_rect_ps", "ps_6_6", PS)?;
    let gs = victory_shader::emit::rect_list_gs(&vs, Some(&ps)).expect("the rectangle stage");
    assert!(
        victory_shader::emit::writes_clip_distance(&vs),
        "the test's vertex shader writes no clip distance"
    );
    let fx = Fx::new();
    let q = fx.queue();
    let fence = fx.fence(0);
    let sig = fx.root_sig(&RootSigDesc::default());
    Some(Fxs {
        fx,
        q,
        fence,
        sig,
        vs,
        ps,
        gs,
    })
}

impl Fxs {
    fn pso(&self) -> PsoId {
        let mut d = GraphicsPsoDesc::new();
        d.vs = StageCode::of(&self.vs);
        d.gs = StageCode::of(&self.gs);
        d.ps = StageCode::of(&self.ps);
        d.input_count = 2;
        d.inputs[0] = InputElement::new("POSITION", 0, format::R32G32B32_FLOAT, 0, 0);
        d.inputs[1] = InputElement::new("COLOR", 0, format::R32G32B32A32_FLOAT, 0, APPEND_ALIGNED);
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

    fn settle(&self) {
        let v = self.fx.fence_value(self.fence) + 1;
        assert_eq!(
            self.fx.submit(&[signal_cmd(self.q, self.fence, v)]),
            Status::OK
        );
        assert_eq!(self.fx.wait(self.fence, v), Status::OK);
    }

    /// Draws the three vertices `(x, y)` with the clip distances `planes` (up to four; the rest are
    /// far inside) and reads the target back.
    fn draw(&self, corners: [(f32, f32); 3], planes: &[Plane]) -> Vec<u32> {
        let mut words = Vec::new();
        for (x, y) in corners {
            words.extend([x.to_bits(), y.to_bits(), 0.5f32.to_bits()]);
            for k in 0..4 {
                let d = planes
                    .get(k)
                    .map_or(8.0, |&p| dist(p, f64::from(x), f64::from(y)) as f32);
                words.push(d.to_bits());
            }
        }
        let vb = self.fx.buffer(BufferKind::UPLOAD, 4096);
        cpu_write(vb.cpu_addr, &words);
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
            clear: [0.0, 0.0, 0.0, 1.0],
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
        let mut copy = <CopyTextureRegion as Wire>::ZERO;
        copy.dst = foot;
        copy.src = loc;
        let cmds = vec![
            begin(1, self.q),
            Command::SetRootSignature(SetRootSignature {
                bind_point: BindPoint::GRAPHICS,
                root_sig: self.sig,
            }),
            Command::SetPipeline(SetPipeline {
                pso: self.pso(),
                _pad: 0,
            }),
            Command::SetPrimitiveTopology(SetPrimitiveTopology {
                topology: Topology::TRIANGLE_LIST,
                _pad: 0,
            }),
            Command::SetViewports(SetViewports::from_slice(&[Viewport::new(
                0.0, 0.0, W as f32, H as f32,
            )])),
            Command::SetScissors(SetScissors::from_slice(&[Rect::new(
                0, 0, W as i32, H as i32,
            )])),
            Command::SetVertexBuffers(SetVertexBuffers::from_slice(
                0,
                &[VertexBufferView {
                    va: vb.gpu_va,
                    size: 3 * STRIDE,
                    stride: STRIDE,
                }],
            )),
            Command::BeginRenderPass(pass),
            Command::Draw(Draw {
                vertex_count: 3,
                instance_count: 1,
                start_vertex: 0,
                start_instance: 0,
            }),
            Command::EndRenderPass(<EndRenderPass as Wire>::ZERO),
            Command::Barrier(Barrier::memory(
                stage::FRAGMENT | stage::VERTEX,
                stage::BLIT,
            )),
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

/// The picture D3D12 draws: `inside(x, y)` where the shape covers and every plane is not negative.
fn reference(inside: impl Fn(f64, f64) -> bool, planes: &[Plane]) -> Vec<u32> {
    let mut out = vec![pack([0.0, 0.0, 0.0, 1.0]); (W * H) as usize];
    for py in 0..H {
        for px in 0..W {
            let (x, y) = centre(px, py);
            if inside(x, y) {
                let near = planes
                    .iter()
                    .map(|&p| dist(p, x, y).abs())
                    .fold(f64::MAX, f64::min);
                assert!(
                    near > 1e-3,
                    "pixel ({px},{py}) is on a clip line: the test geometry must avoid that"
                );
                if planes.iter().all(|&p| dist(p, x, y) >= 0.0) {
                    out[(py * W + px) as usize] = colour(x, y);
                }
            }
        }
    }
    out
}

fn assert_image(got: &[u32], want: &[u32], what: &str) {
    let drawn = want
        .iter()
        .filter(|&&w| w != pack([0.0, 0.0, 0.0, 1.0]))
        .count();
    for py in 0..H {
        for px in 0..W {
            let i = (py * W + px) as usize;
            assert!(
                close(got[i], want[i], 2),
                "{what}: pixel ({px},{py}) is {:#010x}, wanted {:#010x} ({drawn} pixels drawn in the reference)",
                got[i],
                want[i]
            );
        }
    }
}

/// The rectangle's three-vertex forms: the corner between the other two first, second or third.
const X0: f32 = -0.83;
const X1: f32 = 0.71;
const Y0: f32 = -0.77;
const Y1: f32 = 0.69;

fn orders() -> [(&'static str, [(f32, f32); 3]); 3] {
    let (tl, bl, br, tr) = ((X0, Y1), (X0, Y0), (X1, Y0), (X1, Y1));
    [
        ("corner first", [tl, bl, tr]),
        ("corner second", [tl, bl, br]),
        ("corner third", [bl, tr, tl]),
    ]
}

fn in_rect(x: f64, y: f64) -> bool {
    x >= f64::from(X0) && x <= f64::from(X1) && y >= f64::from(Y0) && y <= f64::from(Y1)
}

fn check(name: &str, planes: &[Plane]) {
    let Some(g) = setup() else { return };
    let want = reference(in_rect, planes);
    let drawn = want
        .iter()
        .filter(|&&w| w != pack([0.0, 0.0, 0.0, 1.0]))
        .count();
    for (order, corners) in orders() {
        let got = g.draw(corners, planes);
        assert_image(&got, &want, &format!("{name}, {order}"));
    }
    eprintln!("{name}: {drawn} pixels");
}

#[test]
fn a_clip_distance_cuts_a_rectangle_where_it_goes_negative() {
    // Right of x = -0.11 stays.
    check("vertical line", &[[1.0, 0.0, 0.11]]);
}

#[test]
fn a_slanted_distance_continues_into_the_fourth_corner() {
    // d = 0.9 x - 0.6 y + 0.07: the fourth corner's value is the sum of the others' less the corner's.
    check("slanted", &[[0.9, -0.6, 0.07]]);
}

#[test]
fn two_planes_cut_twice_and_the_corner_they_share_goes() {
    check("two planes", &[[1.0, 0.0, 0.11], [-0.3, 1.0, 0.43]]);
}

#[test]
fn four_planes_leave_the_convex_region_between_them() {
    check(
        "four planes",
        &[
            [1.0, 0.0, 0.43],
            [-1.0, 0.0, 0.31],
            [0.0, 1.0, 0.37],
            [0.5, -1.0, 0.61],
        ],
    );
}

#[test]
fn a_rectangle_wholly_inside_or_outside_is_drawn_whole_or_not_at_all() {
    check("inside", &[[0.0, 0.0, 1.0]]);
    check("outside", &[[0.0, 0.0, -1.0]]);
}

#[test]
fn a_triangle_that_is_no_rectangle_is_cut_as_a_triangle() {
    let Some(g) = setup() else { return };
    let planes = [[0.8f32, -0.5, 0.05]];
    let tri = [(-0.83f32, -0.77f32), (0.91, -0.67), (-0.73, 0.89)];
    let covers = |x: f64, y: f64| {
        let e = |a: (f32, f32), b: (f32, f32)| {
            (f64::from(b.0) - f64::from(a.0)) * (y - f64::from(a.1))
                - (f64::from(b.1) - f64::from(a.1)) * (x - f64::from(a.0))
        };
        let (e0, e1, e2) = (e(tri[0], tri[1]), e(tri[1], tri[2]), e(tri[2], tri[0]));
        (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0) || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0)
    };
    let got = g.draw(tri, &planes);
    // Position-based colour is affine, so a triangle's is the same function: the same reference.
    assert_image(&got, &reference(covers, &planes), "triangle");
}
