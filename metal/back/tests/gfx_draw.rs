//! Render passes, state and draws through the wire (`BEGIN_RENDER_PASS` ...
//! `DISPATCH_MESH`): HLSL compiled to DXIL by the repository's DXC front end
//! (`common_msc.rs`; the tests print `SKIPPED` and pass without one), Metal
//! Shader Converter, Metal 4. Every result is read back from the texture and
//! compared with a CPU rasterisation of the same triangles (pixel centres,
//! edge functions; the triangles are chosen so that no centre falls on an
//! edge, which leaves the fill rule out of the comparison).
#![cfg(target_vendor = "apple")]

mod common_msc;
mod exec_support;

use common_msc::dxil_or_skip;
use exec_support::*;
use victory_metal_proto::cmd::*;
use victory_metal_proto::ids::*;
use victory_metal_proto::indirect::{ExecuteIndirect, IndirectArg, IndirectArgKind};
use victory_metal_proto::pso::*;
use victory_metal_proto::render::*;
use victory_metal_proto::root::*;
use victory_metal_proto::status::Status;
use victory_metal_proto::sync::{CreateGraphicsPipelineArgs, CreateTextureArgs};
use victory_metal_proto::types::*;
use victory_metal_proto::wire::Wire;

const W: u32 = 16;
const H: u32 = 16;

const VS: &str = "struct VSIn { float3 pos : POSITION; float4 col : COLOR0; };\n\
struct VSOut { float4 pos : SV_Position; float4 col : COLOR0; };\n\
VSOut main(VSIn i) { VSOut o; o.pos = float4(i.pos, 1); o.col = i.col; return o; }\n";

const PS: &str = "struct VSOut { float4 pos : SV_Position; float4 col : COLOR0; };\n\
float4 main(VSOut i) : SV_Target0 { return i.col; }\n";

const MS: &str = "struct V { float4 pos : SV_Position; float4 col : COLOR0; };\n\
[numthreads(32,1,1)] [outputtopology(\"triangle\")]\n\
void main(uint tid : SV_GroupIndex, out vertices V verts[3], out indices uint3 tris[1]) {\n\
  SetMeshOutputCounts(3, 1);\n\
  if (tid < 3) {\n\
    float2 p[3] = { float2(-1,-1), float2(1.05,-1), float2(-1,1.05) };\n\
    verts[tid].pos = float4(p[tid], 0.5, 1); verts[tid].col = float4(0, 1, 0, 1); }\n\
  if (tid == 0) tris[0] = uint3(0,1,2);\n}\n";

const AS: &str = "struct Payload { uint count; };\n\
groupshared Payload p;\n\
[numthreads(32,1,1)] void main(uint tid : SV_GroupIndex) {\n\
  p.count = 1;\n  DispatchMesh(1, 1, 1, p);\n}\n";

const MS_WITH_PAYLOAD: &str = "struct Payload { uint count; };\n\
struct V { float4 pos : SV_Position; float4 col : COLOR0; };\n\
[numthreads(32,1,1)] [outputtopology(\"triangle\")]\n\
void main(uint tid : SV_GroupIndex, in payload Payload pl, out vertices V verts[3], out indices uint3 tris[1]) {\n\
  SetMeshOutputCounts(3, 1);\n\
  if (tid < 3) {\n\
    float2 p[3] = { float2(-1,-1), float2(1.05,-1), float2(-1,1.05) };\n\
    verts[tid].pos = float4(p[tid], 0.5, 1); verts[tid].col = float4(0, pl.count, 1, 1); }\n\
  if (tid == 0) tris[0] = uint3(0,1,2);\n}\n";

fn f(x: f32) -> u32 {
    x.to_bits()
}

/// A vertex: position and colour.
type Vtx = ([f32; 3], [f32; 4]);

fn vertex_words(v: &[Vtx]) -> Vec<u32> {
    v.iter()
        .flat_map(|(p, c)| p.iter().chain(c.iter()).map(|x| f(*x)).collect::<Vec<_>>())
        .collect()
}

const STRIDE: u32 = 28;

struct Gfx {
    fx: Fx,
    q: QueueId,
    fence: FenceId,
    sig: RootSigId,
    vs: Vec<u8>,
    ps: Vec<u8>,
}

fn setup() -> Option<Gfx> {
    let vs = dxil_or_skip("draw_vs", "vs_6_6", VS)?;
    let ps = dxil_or_skip("draw_ps", "ps_6_6", PS)?;
    let fx = Fx::new();
    let q = fx.queue();
    let fence = fx.fence(0);
    let sig = fx.root_sig(&RootSigDesc::default());
    Some(Gfx {
        fx,
        q,
        fence,
        sig,
        vs,
        ps,
    })
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

    fn desc(&self) -> GraphicsPsoDesc {
        let mut d = GraphicsPsoDesc::new();
        d.vs = StageCode::of(&self.vs);
        d.ps = StageCode::of(&self.ps);
        d.input_count = 2;
        d.inputs[0] = InputElement::new("POSITION", 0, format::R32G32B32_FLOAT, 0, 0);
        d.inputs[1] = InputElement::new("COLOR", 0, format::R32G32B32A32_FLOAT, 0, APPEND_ALIGNED);
        d.rt_count = 1;
        d.rt_formats[0] = format::R8G8B8A8_UNORM;
        d.raster.cull = CullMode::NONE;
        d.depth.depth_enable = 0;
        d.depth.depth_write = 0;
        d
    }

    fn pso(&self, d: GraphicsPsoDesc) -> PsoId {
        let mut a = CreateGraphicsPipelineArgs::new();
        a.root_sig = self.sig;
        a.desc = d;
        let (s, a) = self.fx.call(a);
        assert_eq!(s, Status::OK, "{:?}", self.fx.executor().last_error());
        a.pso
    }

    fn texture(&self, format: u32, usage: u32) -> ResId {
        let mut a = CreateTextureArgs::new();
        a.dimension = TexDim::TEX2D;
        a.format = format;
        a.width = W;
        a.height = H;
        a.depth_or_array = 1;
        a.mips = 1;
        a.samples = 1;
        a.usage = usage;
        self.fx.ok(a).res
    }

    fn vertex_buffer(&self, v: &[Vtx]) -> (ResId, u64) {
        let b = self.fx.buffer(BufferKind::UPLOAD, 4096);
        cpu_write(b.cpu_addr, &vertex_words(v));
        (b.res, b.gpu_va)
    }

    /// The commands every test starts with: signature, pipeline, topology,
    /// viewport and scissor of the whole target, vertex buffer.
    fn state(&self, pso: PsoId, vb_va: u64, vb_size: u32) -> Vec<Command<'static>> {
        vec![
            Command::SetRootSignature(SetRootSignature {
                bind_point: BindPoint::GRAPHICS,
                root_sig: self.sig,
            }),
            Command::SetPipeline(SetPipeline { pso, _pad: 0 }),
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
                    va: vb_va,
                    size: vb_size,
                    stride: STRIDE,
                }],
            )),
        ]
    }

    /// Reads the colour target back: `W * H` packed RGBA words.
    fn read(&self, tex: ResId, prefix: Vec<Command<'static>>) -> Vec<u32> {
        let rb = self.fx.buffer(BufferKind::READBACK, 4096);
        cpu_write(rb.cpu_addr, &[0xDEAD_BEEF; (W * H) as usize]);
        let mut cmds = prefix;
        cmds.push(Command::Barrier(Barrier::memory(
            stage::FRAGMENT | stage::VERTEX,
            stage::BLIT,
        )));
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
        cmds.push(Command::CopyTextureRegion(c));
        let mut all = vec![begin(1, self.q)];
        all.extend(cmds);
        all.push(end(1));
        assert_eq!(
            self.fx.submit(&all),
            Status::OK,
            "{:?}",
            self.fx.executor().last_error()
        );
        self.settle();
        cpu_read(rb.cpu_addr, (W * H) as usize)
    }
}

fn color_attachment(res: ResId, load: LoadAction, clear: [f32; 4]) -> RenderAttachment {
    RenderAttachment {
        res,
        format: format::R8G8B8A8_UNORM,
        dimension: TexViewDim::TEX2D,
        mip: 0,
        load,
        _pad: 0,
        first_slice: 0,
        slice_count: 1,
        clear,
    }
}

fn begin_pass(color: ResId, load: LoadAction, clear: [f32; 4]) -> Command<'static> {
    let mut b = <BeginRenderPass as Wire>::ZERO;
    b.rt_count = 1;
    b.rt[0] = color_attachment(color, load, clear);
    Command::BeginRenderPass(b)
}

fn begin_pass_depth(color: ResId, depth: ResId, dformat: u32, clear: [f32; 4]) -> Command<'static> {
    let mut b = <BeginRenderPass as Wire>::ZERO;
    b.rt_count = 1;
    b.rt[0] = color_attachment(color, LoadAction::CLEAR, clear);
    b.ds = DepthAttachment {
        res: depth,
        format: dformat,
        dimension: TexViewDim::TEX2D,
        mip: 0,
        depth_load: LoadAction::CLEAR,
        stencil_load: LoadAction::CLEAR,
        first_slice: 0,
        slice_count: 1,
        clear_depth: 1.0,
        clear_stencil: 2,
        _pad: 0,
    };
    Command::BeginRenderPass(b)
}

fn end_pass() -> Command<'static> {
    Command::EndRenderPass(<EndRenderPass as Wire>::ZERO)
}

fn draw(n: u32) -> Command<'static> {
    Command::Draw(Draw {
        vertex_count: n,
        instance_count: 1,
        start_vertex: 0,
        start_instance: 0,
    })
}

fn pack(c: [f32; 4]) -> u32 {
    let b = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
    b(c[0]) | (b(c[1]) << 8) | (b(c[2]) << 16) | (b(c[3]) << 24)
}

/// Channels of two packed pixels differ by at most `tol`.
fn close(a: u32, b: u32, tol: i32) -> bool {
    (0..4).all(|i| {
        let (x, y) = (((a >> (8 * i)) & 255) as i32, ((b >> (8 * i)) & 255) as i32);
        (x - y).abs() <= tol
    })
}

/// A triangle in NDC with a constant colour and depth.
#[derive(Clone, Copy)]
struct Tri {
    p: [[f32; 2]; 3],
    z: f32,
    c: [f32; 4],
}

impl Tri {
    fn vertices(&self) -> [Vtx; 3] {
        [
            ([self.p[0][0], self.p[0][1], self.z], self.c),
            ([self.p[1][0], self.p[1][1], self.z], self.c),
            ([self.p[2][0], self.p[2][1], self.z], self.c),
        ]
    }

    fn covers(&self, px: u32, py: u32) -> bool {
        let x = (f64::from(px) + 0.5) / f64::from(W) * 2.0 - 1.0;
        let y = 1.0 - (f64::from(py) + 0.5) / f64::from(H) * 2.0;
        let e = |a: [f32; 2], b: [f32; 2]| {
            (f64::from(b[0]) - f64::from(a[0])) * (y - f64::from(a[1]))
                - (f64::from(b[1]) - f64::from(a[1])) * (x - f64::from(a[0]))
        };
        let (e0, e1, e2) = (
            e(self.p[0], self.p[1]),
            e(self.p[1], self.p[2]),
            e(self.p[2], self.p[0]),
        );
        (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0) || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0)
    }
}

/// The triangles' centres are never on an edge: asserted, so a change of
/// the test geometry cannot sneak a fill-rule dependence in.
fn assert_no_ties(t: &Tri) {
    for py in 0..H {
        for px in 0..W {
            let x = (f64::from(px) + 0.5) / f64::from(W) * 2.0 - 1.0;
            let y = 1.0 - (f64::from(py) + 0.5) / f64::from(H) * 2.0;
            for (a, b) in [(0, 1), (1, 2), (2, 0)] {
                let (a, b) = (t.p[a], t.p[b]);
                let e = (f64::from(b[0]) - f64::from(a[0])) * (y - f64::from(a[1]))
                    - (f64::from(b[1]) - f64::from(a[1])) * (x - f64::from(a[0]));
                assert!(e.abs() > 1e-4, "pixel ({px},{py}) is on an edge");
            }
        }
    }
}

const BIG: Tri = Tri {
    p: [[-1.0, -1.0], [1.05, -1.0], [-1.0, 1.05]],
    z: 0.5,
    c: [1.0, 0.5, 0.0, 1.0],
};

/// A triangle whose hypotenuse is off the pixel diagonal.
const HALF: Tri = Tri {
    p: [[-1.0, -1.0], [1.05, -1.0], [-1.0, 1.05]],
    z: 0.5,
    c: [1.0, 0.5, 0.0, 1.0],
};

fn reference(bg: [f32; 4], tris: &[Tri]) -> Vec<u32> {
    let mut out = vec![pack(bg); (W * H) as usize];
    for t in tris {
        assert_no_ties(t);
        for py in 0..H {
            for px in 0..W {
                if t.covers(px, py) {
                    out[(py * W + px) as usize] = pack(t.c);
                }
            }
        }
    }
    out
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

#[test]
fn a_pass_with_only_a_clear_fills_the_target() {
    let Some(g) = setup() else { return };
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let bg = [0.25, 0.5, 0.75, 1.0];
    let got = g.read(t, vec![begin_pass(t, LoadAction::CLEAR, bg), end_pass()]);
    assert_image(&got, &vec![pack(bg); (W * H) as usize], "clear");
}

#[test]
fn a_triangle_is_drawn_where_the_cpu_says() {
    let Some(g) = setup() else { return };
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let (_vb, va) = g.vertex_buffer(&HALF.vertices());
    let pso = g.pso(g.desc());
    let bg = [0.0, 0.0, 0.25, 1.0];
    let mut cmds = g.state(pso, va, 3 * STRIDE);
    cmds.push(begin_pass(t, LoadAction::CLEAR, bg));
    cmds.push(draw(3));
    cmds.push(end_pass());
    let got = g.read(t, cmds);
    assert_image(&got, &reference(bg, &[HALF]), "triangle");
    assert!(got.iter().any(|p| *p == pack(HALF.c)) && got.iter().any(|p| *p == pack(bg)));
}

#[test]
fn scissor_and_viewport_limit_the_draw() {
    let Some(g) = setup() else { return };
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let (_vb, va) = g.vertex_buffer(&BIG.vertices());
    let pso = g.pso(g.desc());
    let bg = [0.0, 0.0, 0.0, 1.0];
    // Scissor to x in [4, 12), y in [2, 10).
    let mut cmds = g.state(pso, va, 3 * STRIDE);
    cmds.push(Command::SetScissors(SetScissors::from_slice(&[Rect::new(
        4, 2, 12, 10,
    )])));
    cmds.push(begin_pass(t, LoadAction::CLEAR, bg));
    cmds.push(draw(3));
    cmds.push(end_pass());
    let got = g.read(t, cmds);
    let mut want = reference(bg, &[BIG]);
    for py in 0..H {
        for px in 0..W {
            if !((4..12).contains(&px) && (2..10).contains(&py)) {
                want[(py * W + px) as usize] = pack(bg);
            }
        }
    }
    assert_image(&got, &want, "scissor");

    // The viewport maps the triangle into the right half only.
    let mut cmds = g.state(pso, va, 3 * STRIDE);
    cmds.push(Command::SetViewports(SetViewports::from_slice(&[
        Viewport::new(8.0, 0.0, 8.0, 16.0),
    ])));
    cmds.push(begin_pass(t, LoadAction::CLEAR, bg));
    cmds.push(draw(3));
    cmds.push(end_pass());
    let got = g.read(t, cmds);
    for py in 0..H {
        for px in 0..8 {
            assert_eq!(
                got[(py * W + px) as usize],
                pack(bg),
                "left half ({px},{py})"
            );
        }
    }
    assert!(
        got[(15 * W + 8) as usize] == pack(BIG.c),
        "the bottom left of the viewport is drawn"
    );
}

#[test]
fn indexed_draws_use_the_index_buffer_and_base_vertex() {
    let Some(g) = setup() else { return };
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    // A quad of two triangles, positions off the pixel grid; the vertices
    // start at vertex 2 of the buffer (base vertex 2) to prove it counts.
    let c = [0.0, 1.0, 0.0, 1.0];
    let quad = |z| -> Vec<Vtx> {
        vec![
            ([0.0; 3], [0.0; 4]),
            ([0.0; 3], [0.0; 4]),
            ([-0.83, -0.77, z], c),
            ([0.71, -0.77, z], c),
            ([0.71, 0.69, z], c),
            ([-0.83, 0.69, z], c),
        ]
    };
    let (_vb, va) = g.vertex_buffer(&quad(0.5));
    let ib = g.fx.buffer(BufferKind::UPLOAD, 4096);
    // 16-bit indices, three of padding to prove the start index counts.
    cpu_write(ib.cpu_addr, &[0, 0x0001_0000, 0x0000_0002, 0x0003_0000]);
    // indices at element 3: 0, 1, 2, 0, 2, 3
    cpu_write(
        ib.cpu_addr,
        &[
            0x0000_0000,
            0x0001_0000,
            0x0000_0000,
            0x0002_0001,
            0x0003_0002,
        ],
    );
    // elements: [0,0, 0,0, 1,0? ...] -- spelled out below instead.
    let idx: [u16; 9] = [0, 0, 0, 0, 1, 2, 0, 2, 3];
    let words: Vec<u32> = idx
        .chunks(2)
        .map(|p| u32::from(p[0]) | (u32::from(*p.get(1).unwrap_or(&0)) << 16))
        .collect();
    cpu_write(ib.cpu_addr, &words);
    let pso = g.pso(g.desc());
    let bg = [0.0, 0.0, 0.0, 1.0];
    let mut cmds = g.state(pso, va, 6 * STRIDE);
    cmds.push(Command::SetIndexBuffer(SetIndexBuffer {
        va: ib.gpu_va,
        size: 18,
        format: IndexFormat::U16,
    }));
    cmds.push(begin_pass(t, LoadAction::CLEAR, bg));
    cmds.push(Command::DrawIndexed(DrawIndexed {
        index_count: 6,
        instance_count: 1,
        start_index: 3,
        base_vertex: 2,
        start_instance: 0,
        _pad: 0,
    }));
    cmds.push(end_pass());
    let got = g.read(t, cmds);
    // The quad as two triangles (the shared diagonal has no centre on it for
    // these coordinates, asserted by the reference).
    let t1 = Tri {
        p: [[-0.83, -0.77], [0.71, -0.77], [0.71, 0.69]],
        z: 0.5,
        c,
    };
    let t2 = Tri {
        p: [[-0.83, -0.77], [0.71, 0.69], [-0.83, 0.69]],
        z: 0.5,
        c,
    };
    assert_image(&got, &reference(bg, &[t1, t2]), "indexed quad");
}

#[test]
fn depth_testing_keeps_the_nearer_triangle() {
    let Some(g) = setup() else { return };
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let d = g.texture(format::D32_FLOAT, tex_usage::DEPTH_STENCIL);
    let tri = |p: [[f32; 2]; 3], z: f32, c: [f32; 4]| Tri { p, z, c };
    let far = tri(
        [[-0.93, -0.87], [0.89, -0.87], [-0.93, 0.91]],
        0.8,
        [0.0, 1.0, 0.0, 1.0],
    );
    let near = tri(
        [[-0.51, -0.93], [0.93, -0.33], [-0.31, 0.77]],
        0.2,
        [1.0, 0.0, 0.0, 1.0],
    );
    let behind = tri(
        [[-0.77, -0.61], [0.67, -0.61], [-0.07, 0.83]],
        0.9,
        [0.0, 0.0, 1.0, 1.0],
    );
    let all: Vec<Vtx> = [far, near, behind]
        .iter()
        .flat_map(|t| t.vertices())
        .collect();
    let (_vb, va) = g.vertex_buffer(&all);
    let mut desc = g.desc();
    desc.ds_format = format::D32_FLOAT;
    desc.depth.depth_enable = 1;
    desc.depth.depth_write = 1;
    desc.depth.depth_func = CompareFunc::LESS;
    let pso = g.pso(desc);
    let bg = [0.0, 0.0, 0.0, 1.0];
    let mut cmds = g.state(pso, va, 9 * STRIDE);
    cmds.push(begin_pass_depth(t, d, format::D32_FLOAT, bg));
    cmds.push(draw(9));
    cmds.push(end_pass());
    let got = g.read(t, cmds);
    // CPU: per pixel the nearest covering triangle.
    let mut want = vec![pack(bg); (W * H) as usize];
    let mut depth = vec![1.0f32; (W * H) as usize];
    for tri in [far, near, behind] {
        assert_no_ties(&tri);
        for py in 0..H {
            for px in 0..W {
                let i = (py * W + px) as usize;
                if tri.covers(px, py) && tri.z < depth[i] {
                    depth[i] = tri.z;
                    want[i] = pack(tri.c);
                }
            }
        }
    }
    assert_image(&got, &want, "depth");
    assert!(
        want.contains(&pack(far.c))
            && want.contains(&pack(near.c))
            && want.contains(&pack(behind.c))
    );
}

#[test]
fn blending_uses_the_pipeline_factors_and_the_blend_factor_command() {
    let Some(g) = setup() else { return };
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let mut tri = BIG;
    tri.c = [1.0, 0.0, 0.0, 0.5];
    let (_vb, va) = g.vertex_buffer(&tri.vertices());
    // Source alpha over: 0.5 red on blue.
    let mut d = g.desc();
    d.blend.rt[0].enable = 1;
    d.blend.rt[0].src = BlendFactor::SRC_ALPHA;
    d.blend.rt[0].dst = BlendFactor::INV_SRC_ALPHA;
    d.blend.rt[0].op = BlendOp::ADD;
    d.blend.rt[0].src_alpha = BlendFactor::ONE;
    d.blend.rt[0].dst_alpha = BlendFactor::ZERO;
    d.blend.rt[0].op_alpha = BlendOp::ADD;
    let over = g.pso(d);
    let bg = [0.0, 0.0, 1.0, 1.0];
    let mut cmds = g.state(over, va, 3 * STRIDE);
    cmds.push(begin_pass(t, LoadAction::CLEAR, bg));
    cmds.push(draw(3));
    cmds.push(end_pass());
    let got = g.read(t, cmds);
    let mixed = [0.5, 0.0, 0.5, 0.5];
    let mut want = vec![pack(bg); (W * H) as usize];
    for py in 0..H {
        for px in 0..W {
            if BIG.covers(px, py) {
                want[(py * W + px) as usize] = pack(mixed);
            }
        }
    }
    for (i, (a, b)) in got.iter().zip(&want).enumerate() {
        // Alpha of the mix is the source alpha (factors ONE, ZERO).
        assert!(
            close(*a, *b, 1),
            "over: pixel {i} is {a:#010x}, wanted {b:#010x}"
        );
    }

    // Constant blend factor: result = source * factor.
    let mut d = g.desc();
    d.blend.rt[0].enable = 1;
    d.blend.rt[0].src = BlendFactor::CONSTANT_COLOR;
    d.blend.rt[0].dst = BlendFactor::ZERO;
    d.blend.rt[0].op = BlendOp::ADD;
    d.blend.rt[0].src_alpha = BlendFactor::ONE;
    d.blend.rt[0].dst_alpha = BlendFactor::ZERO;
    d.blend.rt[0].op_alpha = BlendOp::ADD;
    let constant = g.pso(d);
    let mut white = BIG;
    white.c = [1.0, 1.0, 1.0, 1.0];
    let (_vb2, va2) = g.vertex_buffer(&white.vertices());
    let mut cmds = g.state(constant, va2, 3 * STRIDE);
    cmds.push(Command::SetBlendFactor(SetBlendFactor {
        color: [0.5, 0.25, 1.0, 1.0],
    }));
    cmds.push(begin_pass(t, LoadAction::CLEAR, bg));
    cmds.push(draw(3));
    cmds.push(end_pass());
    let got = g.read(t, cmds);
    let i = (15 * W) as usize;
    assert!(
        close(got[i], pack([0.5, 0.25, 1.0, 1.0]), 1),
        "constant factor: {:#010x}",
        got[i]
    );
}

#[test]
fn a_stencil_reference_decides_what_passes() {
    let Some(g) = setup() else { return };
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let d = g.texture(format::D32_FLOAT_S8X24_UINT, tex_usage::DEPTH_STENCIL);
    let (_vb, va) = g.vertex_buffer(&BIG.vertices());
    let mut desc = g.desc();
    desc.ds_format = format::D32_FLOAT_S8X24_UINT;
    desc.depth.depth_enable = 0;
    desc.depth.depth_write = 0;
    desc.depth.stencil_enable = 1;
    desc.depth.front.func = CompareFunc::EQUAL;
    desc.depth.back.func = CompareFunc::EQUAL;
    let pso = g.pso(desc);
    let bg = [0.0, 0.0, 0.0, 1.0];
    // The stencil was cleared to 2.
    for (reference_value, drawn) in [(2u32, true), (3, false)] {
        let mut cmds = g.state(pso, va, 3 * STRIDE);
        cmds.push(Command::SetStencilRef(SetStencilRef {
            value: reference_value,
            _pad: 0,
        }));
        cmds.push(begin_pass_depth(t, d, format::D32_FLOAT_S8X24_UINT, bg));
        cmds.push(draw(3));
        cmds.push(end_pass());
        let got = g.read(t, cmds);
        let want = if drawn {
            reference(bg, &[BIG])
        } else {
            vec![pack(bg); (W * H) as usize]
        };
        assert_image(&got, &want, &format!("stencil ref {reference_value}"));
    }
}

#[test]
fn two_passes_keep_what_the_first_drew_and_a_copy_inside_one_does_not_lose_it() {
    let Some(g) = setup() else { return };
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let scratch = g.fx.buffer(BufferKind::DEFAULT, 4096);
    let scratch2 = g.fx.buffer(BufferKind::DEFAULT, 4096);
    let mut a = HALF;
    a.c = [1.0, 0.0, 0.0, 1.0];
    let mut b = Tri {
        p: [[0.93, 0.97], [0.93, -0.91], [-0.87, 0.97]],
        z: 0.5,
        c: [0.0, 1.0, 0.0, 1.0],
    };
    b.c = [0.0, 1.0, 0.0, 1.0];
    let c = Tri {
        p: [[-0.47, -0.83], [-0.13, -0.83], [-0.47, -0.45]],
        z: 0.5,
        c: [1.0, 1.0, 1.0, 1.0],
    };
    let all: Vec<Vtx> = [a, b, c].iter().flat_map(|t| t.vertices()).collect();
    let (_vb, va) = g.vertex_buffer(&all);
    let pso = g.pso(g.desc());
    let bg = [0.0, 0.0, 0.0, 1.0];
    let mut cmds = g.state(pso, va, 9 * STRIDE);
    // Pass 1 draws `a`; pass 2 loads and draws `b` (vertices 3..6) after a
    // buffer copy sat in the middle of it, which ends the encoder.
    cmds.push(begin_pass(t, LoadAction::CLEAR, bg));
    cmds.push(draw(3));
    cmds.push(end_pass());
    cmds.push(begin_pass(t, LoadAction::LOAD, [0.0; 4]));
    cmds.push(Command::Draw(Draw {
        vertex_count: 3,
        instance_count: 1,
        start_vertex: 3,
        start_instance: 0,
    }));
    cmds.push(copy(scratch.res, scratch2.res, 64));
    cmds.push(Command::Draw(Draw {
        vertex_count: 3,
        instance_count: 1,
        start_vertex: 6,
        start_instance: 0,
    }));
    cmds.push(end_pass());
    let got = g.read(t, cmds);
    let mut want = reference(bg, &[a, b, c]);
    // Where both cover, `b` (drawn last) wins: `reference` draws in order.
    want.truncate((W * H) as usize);
    assert_image(&got, &want, "two passes");
}

#[test]
fn draws_outside_a_pass_and_a_topology_of_another_class_are_refused() {
    let Some(g) = setup() else { return };
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let (_vb, va) = g.vertex_buffer(&BIG.vertices());
    let pso = g.pso(g.desc());
    let mut cmds = vec![begin(1, g.q)];
    cmds.extend(g.state(pso, va, 3 * STRIDE));
    cmds.push(draw(3));
    cmds.push(end(1));
    assert_eq!(g.fx.submit(&cmds), Status::INVALID_ARG, "no pass");

    let mut cmds = vec![begin(1, g.q)];
    cmds.extend(g.state(pso, va, 3 * STRIDE));
    cmds.push(Command::SetPrimitiveTopology(SetPrimitiveTopology {
        topology: Topology::LINE_LIST,
        _pad: 0,
    }));
    cmds.push(begin_pass(t, LoadAction::CLEAR, [0.0; 4]));
    cmds.push(draw(3));
    cmds.push(end(1));
    assert_eq!(
        g.fx.submit(&cmds),
        Status::INVALID_ARG,
        "lines with a triangle pipeline"
    );

    // An unbound vertex slot the pipeline reads.
    let mut cmds = vec![begin(1, g.q)];
    cmds.extend(g.state(pso, va, 3 * STRIDE));
    cmds.push(Command::SetVertexBuffers(SetVertexBuffers::from_slice(
        0,
        &[VertexBufferView {
            va: 0,
            size: 0,
            stride: 0,
        }],
    )));
    cmds.push(begin_pass(t, LoadAction::CLEAR, [0.0; 4]));
    cmds.push(draw(3));
    cmds.push(end(1));
    assert_eq!(g.fx.submit(&cmds), Status::INVALID_ARG, "no vertex buffer");
}

fn has_mesh(fx: &Fx) -> bool {
    fx.executor().device().unwrap().caps().mesh_shaders
}

#[test]
fn a_mesh_shader_draws_a_triangle_with_and_without_an_amplification_stage() {
    let Some(ms) = dxil_or_skip("draw_ms", "ms_6_6", MS) else {
        return;
    };
    let Some(ps) = dxil_or_skip("draw_ps_mesh", "ps_6_6", PS) else {
        return;
    };
    let Some(as_) = dxil_or_skip("draw_as", "as_6_6", AS) else {
        return;
    };
    let Some(ms_pl) = dxil_or_skip("draw_ms_pl", "ms_6_6", MS_WITH_PAYLOAD) else {
        return;
    };
    let Some(mut g) = setup() else { return };
    if !has_mesh(&g.fx) {
        eprintln!("SKIPPED: no mesh shaders on this device");
        return;
    }
    g.ps = ps;
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let bg = [0.0, 0.0, 0.0, 1.0];
    let mesh_tri = |ms: &[u8], amp: Option<&[u8]>| {
        let mut d = GraphicsPsoDesc::new();
        d.ms = StageCode::of(ms);
        if let Some(a) = amp {
            d.amp = StageCode::of(a);
        }
        d.ps = StageCode::of(&g.ps);
        d.rt_count = 1;
        d.rt_formats[0] = format::R8G8B8A8_UNORM;
        d.raster.cull = CullMode::NONE;
        d.depth.depth_enable = 0;
        d.depth.depth_write = 0;
        g.pso(d)
    };
    let plain = mesh_tri(&ms, None);
    let amplified = mesh_tri(&ms_pl, Some(&as_));
    for (pso, want_color) in [
        (plain, [0.0, 1.0, 0.0, 1.0]),
        (amplified, [0.0, 1.0, 1.0, 1.0]),
    ] {
        let mut cmds = vec![
            Command::SetRootSignature(SetRootSignature {
                bind_point: BindPoint::GRAPHICS,
                root_sig: g.sig,
            }),
            Command::SetPipeline(SetPipeline { pso, _pad: 0 }),
            Command::SetViewports(SetViewports::from_slice(&[Viewport::new(
                0.0, 0.0, W as f32, H as f32,
            )])),
            Command::SetScissors(SetScissors::from_slice(&[Rect::new(
                0, 0, W as i32, H as i32,
            )])),
        ];
        cmds.push(begin_pass(t, LoadAction::CLEAR, bg));
        cmds.push(Command::DispatchMesh(DispatchMesh {
            x: 1,
            y: 1,
            z: 1,
            _pad: 0,
        }));
        cmds.push(end_pass());
        let got = g.read(t, cmds);
        let mut tri = BIG;
        tri.c = want_color;
        assert_image(&got, &reference(bg, &[tri]), "mesh");
    }
}

fn clear_attachment(
    target: u32,
    flags: u32,
    rects: &[Rect],
    color: [f32; 4],
    depth: f32,
    stencil: u32,
) -> Command<'static> {
    let mut c = <ClearAttachment as Wire>::ZERO;
    c.target = target;
    c.flags = flags;
    c.rect_count = rects.len() as u32;
    c.color = color;
    c.clear_depth = depth;
    c.clear_stencil = stencil;
    c.rect[..rects.len()].copy_from_slice(rects);
    Command::ClearAttachment(c)
}

fn draw_from(start: u32, n: u32) -> Command<'static> {
    Command::Draw(Draw {
        vertex_count: n,
        instance_count: 1,
        start_vertex: start,
        start_instance: 0,
    })
}

#[test]
fn clearing_a_rectangle_inside_a_pass_changes_only_that_rectangle() {
    let Some(g) = setup() else { return };
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let (_vb, va) = g.vertex_buffer(&HALF.vertices());
    let pso = g.pso(g.desc());
    let bg = [0.0, 0.0, 0.25, 1.0];
    let blue = [0.0, 0.0, 1.0, 1.0];
    let mut cmds = g.state(pso, va, 3 * STRIDE);
    cmds.push(begin_pass(t, LoadAction::CLEAR, bg));
    cmds.push(draw(3));
    cmds.push(clear_attachment(
        0,
        0,
        &[Rect::new(4, 4, 10, 10), Rect::new(12, 0, 16, 3)],
        blue,
        0.0,
        0,
    ));
    // The pipeline and state of the draw are back after the clear.
    cmds.push(draw(3));
    cmds.push(end_pass());
    let got = g.read(t, cmds);
    let mut want = reference(bg, &[HALF]);
    for py in 0..H {
        for px in 0..W {
            if ((4..10).contains(&px) && (4..10).contains(&py)) || (px >= 12 && py < 3) {
                want[(py * W + px) as usize] = pack(blue);
            }
        }
    }
    // The second draw repaints the triangle over the clear.
    for py in 0..H {
        for px in 0..W {
            if HALF.covers(px, py) {
                want[(py * W + px) as usize] = pack(HALF.c);
            }
        }
    }
    assert_image(&got, &want, "clear rect then draw");

    // Without the second draw the rectangles stay blue.
    let mut cmds = g.state(pso, va, 3 * STRIDE);
    cmds.push(begin_pass(t, LoadAction::CLEAR, bg));
    cmds.push(draw(3));
    cmds.push(clear_attachment(
        0,
        0,
        &[Rect::new(4, 4, 10, 10)],
        blue,
        0.0,
        0,
    ));
    cmds.push(end_pass());
    let got = g.read(t, cmds);
    let mut want = reference(bg, &[HALF]);
    for py in 4..10 {
        for px in 4..10 {
            want[(py * W + px) as usize] = pack(blue);
        }
    }
    assert_image(&got, &want, "clear rect");
}

#[test]
fn clearing_depth_in_a_rectangle_lets_a_farther_triangle_through_there() {
    let Some(g) = setup() else { return };
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let d = g.texture(format::D32_FLOAT, tex_usage::DEPTH_STENCIL);
    let near = Tri {
        p: [[-0.93, -0.87], [0.89, -0.87], [-0.93, 0.91]],
        z: 0.2,
        c: [1.0, 0.0, 0.0, 1.0],
    };
    let far = Tri {
        p: [[-0.87, -0.93], [0.91, -0.81], [-0.77, 0.93]],
        z: 0.8,
        c: [0.0, 1.0, 0.0, 1.0],
    };
    let all: Vec<Vtx> = [near, far].iter().flat_map(|t| t.vertices()).collect();
    let (_vb, va) = g.vertex_buffer(&all);
    let mut desc = g.desc();
    desc.ds_format = format::D32_FLOAT;
    desc.depth.depth_enable = 1;
    desc.depth.depth_write = 1;
    desc.depth.depth_func = CompareFunc::LESS;
    let pso = g.pso(desc);
    let bg = [0.0, 0.0, 0.0, 1.0];
    let mut cmds = g.state(pso, va, 6 * STRIDE);
    cmds.push(begin_pass_depth(t, d, format::D32_FLOAT, bg));
    cmds.push(draw_from(0, 3));
    cmds.push(clear_attachment(
        8,
        CLEAR_DEPTH,
        &[Rect::new(0, 0, 8, 16)],
        [0.0; 4],
        1.0,
        0,
    ));
    cmds.push(draw_from(3, 3));
    cmds.push(end_pass());
    let got = g.read(t, cmds);
    assert_no_ties(&near);
    assert_no_ties(&far);
    let mut want = vec![pack(bg); (W * H) as usize];
    for py in 0..H {
        for px in 0..W {
            let i = (py * W + px) as usize;
            if near.covers(px, py) {
                want[i] = pack(near.c);
            }
            if far.covers(px, py) && (px < 8 || !near.covers(px, py)) {
                want[i] = pack(far.c);
            }
        }
    }
    assert_image(&got, &want, "depth clear rect");
    assert!(want.contains(&pack(near.c)) && want.contains(&pack(far.c)));
}

#[test]
fn clearing_stencil_in_a_pass_sets_the_value_the_stencil_test_reads() {
    let Some(g) = setup() else { return };
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let d = g.texture(format::D32_FLOAT_S8X24_UINT, tex_usage::DEPTH_STENCIL);
    let (_vb, va) = g.vertex_buffer(&BIG.vertices());
    let mut desc = g.desc();
    desc.ds_format = format::D32_FLOAT_S8X24_UINT;
    desc.depth.depth_enable = 0;
    desc.depth.depth_write = 0;
    desc.depth.stencil_enable = 1;
    desc.depth.front.func = CompareFunc::EQUAL;
    desc.depth.back.func = CompareFunc::EQUAL;
    let pso = g.pso(desc);
    let bg = [0.0, 0.0, 0.0, 1.0];
    // The stencil starts at 2, the right half is cleared to 7; reference 7
    // draws only there.
    let mut cmds = g.state(pso, va, 3 * STRIDE);
    cmds.push(Command::SetStencilRef(SetStencilRef { value: 7, _pad: 0 }));
    cmds.push(begin_pass_depth(t, d, format::D32_FLOAT_S8X24_UINT, bg));
    cmds.push(clear_attachment(
        8,
        CLEAR_STENCIL,
        &[Rect::new(8, 0, 16, 16)],
        [0.0; 4],
        0.0,
        7,
    ));
    cmds.push(draw(3));
    cmds.push(end_pass());
    let got = g.read(t, cmds);
    let mut want = reference(bg, &[BIG]);
    for py in 0..H {
        for px in 0..8 {
            want[(py * W + px) as usize] = pack(bg);
        }
    }
    assert_image(&got, &want, "stencil clear rect");
}

const PS_UAV: &str = "struct VSOut { float4 pos : SV_Position; float4 col : COLOR0; };\n\
RWByteAddressBuffer bits : register(u1);\n\
[earlydepthstencil] float4 main(VSOut i) : SV_Target0 { bits.Store(0, 0x1234); return i.col; }\n";

/// A pixel shader that writes a buffer through a root UAV, behind the
/// depth test (early depth), then a barrier and a copy of the buffer: the
/// order the Wine suite records (barrier before the pass is ended by the copy).
#[test]
fn a_pixel_shader_buffer_write_is_visible_after_the_barrier_and_copy_and_the_depth_test_gates_it() {
    let Some(ps) = dxil_or_skip("draw_ps_uav", "ps_6_6", PS_UAV) else {
        return;
    };
    let Some(mut g) = setup() else { return };
    g.ps = ps;
    let mut rd = RootSigDesc::default();
    let mut p = descriptor_param(RootParamKind::UAV);
    p.shader_register = 1;
    rd.params.push(p);
    g.sig = g.fx.root_sig(&rd);
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let d = g.texture(format::D32_FLOAT, tex_usage::DEPTH_STENCIL);
    let out = g.fx.buffer(BufferKind::DEFAULT, 256);
    let rb = g.fx.buffer(BufferKind::READBACK, 256);
    cpu_write(rb.cpu_addr, &[0; 8]);
    let mut desc = g.desc();
    desc.ds_format = format::D32_FLOAT;
    desc.depth.depth_enable = 1;
    desc.depth.depth_write = 1;
    desc.depth.depth_func = CompareFunc::LESS;
    let pso = g.pso(desc);
    for (z, want) in [(0.9f32, 0u32), (0.2, 0x1234)] {
        let mut tri = BIG;
        tri.z = z;
        let (_vb, va) = g.vertex_buffer(&tri.vertices());
        let mut cmds = vec![begin(1, g.q)];
        cmds.extend(g.state(pso, va, 3 * STRIDE));
        cmds.push(root_va_graphics(0, RootParamKind::UAV, out.gpu_va));
        // As the Wine suite does it: the clears come as passes of their own
        // (the targets are not bound yet), then a pass that loads both.
        let mut clear_color = begin_pass(t, LoadAction::CLEAR, [0.0, 0.0, 0.0, 1.0]);
        let mut clear_depth = <BeginRenderPass as Wire>::ZERO;
        clear_depth.ds = DepthAttachment {
            res: d,
            format: format::D32_FLOAT,
            dimension: TexViewDim::TEX2D,
            mip: 0,
            depth_load: LoadAction::CLEAR,
            stencil_load: LoadAction::LOAD,
            first_slice: 0,
            slice_count: 1,
            clear_depth: 0.5,
            clear_stencil: 0,
            _pad: 0,
        };
        cmds.push(fill(out.res, 0, 0, 256));
        cmds.push(Command::BeginRenderPass(clear_depth));
        cmds.push(end_pass());
        if let Command::BeginRenderPass(bp) = &mut clear_color {
            bp.rt_count = 1;
        }
        cmds.push(clear_color);
        cmds.push(end_pass());
        let mut load = begin_pass_depth(t, d, format::D32_FLOAT, [0.0; 4]);
        if let Command::BeginRenderPass(bp) = &mut load {
            bp.rt[0].load = LoadAction::LOAD;
            bp.ds.depth_load = LoadAction::LOAD;
            bp.ds.stencil_load = LoadAction::LOAD;
        }
        cmds.push(load);
        cmds.push(draw(3));
        cmds.push(Command::Barrier(Barrier::memory(
            stage::FRAGMENT,
            stage::BLIT,
        )));
        cmds.push(end_pass());
        cmds.push(copy(rb.res, out.res, 256));
        cmds.push(end(1));
        assert_eq!(
            g.fx.submit(&cmds),
            Status::OK,
            "{:?}",
            g.fx.executor().last_error()
        );
        g.settle();
        assert_eq!(cpu_read(rb.cpu_addr, 1)[0], want, "z = {z}");
    }
}

fn root_va_graphics(root_index: u32, kind: RootParamKind, gpu_va: u64) -> Command<'static> {
    Command::SetRootDescriptor(SetRootDescriptor {
        bind_point: BindPoint::GRAPHICS,
        root_index,
        kind,
        _pad: 0,
        gpu_va,
    })
}

const PS_TWO_OUTPUTS: &str = "struct VSOut { float4 pos : SV_Position; float4 col : COLOR0; };\n\
struct Out { float4 c : SV_Target0; float4 extra : SV_Target1; };\n\
Out main(VSOut i) { Out o; o.c = i.col; o.extra = float4(1, 0, 1, 1); return o; }\n";

/// D3D12 drops what a pixel shader writes beyond the pipeline's targets:
/// the pipeline is made (and conservative rasterization is ignored) and the
/// first target is drawn as usual.
#[test]
fn outputs_beyond_the_pipelines_targets_are_dropped() {
    let Some(ps) = dxil_or_skip("draw_ps2", "ps_6_6", PS_TWO_OUTPUTS) else {
        return;
    };
    let Some(mut g) = setup() else { return };
    g.ps = ps;
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let (_vb, va) = g.vertex_buffer(&HALF.vertices());
    let mut d = g.desc();
    d.raster.conservative = 1;
    let pso = g.pso(d);
    let bg = [0.0, 0.0, 0.25, 1.0];
    let mut cmds = g.state(pso, va, 3 * STRIDE);
    cmds.push(begin_pass(t, LoadAction::CLEAR, bg));
    cmds.push(draw(3));
    cmds.push(end_pass());
    let got = g.read(t, cmds);
    assert_image(&got, &reference(bg, &[HALF]), "extra output dropped");
}

// ---------------------------------------------------------------------------
// Geometry shaders (emulated with an object and a mesh stage).

const GS_RECOLOUR: &str = "struct V { float4 pos : SV_Position; float4 col : COLOR0; };\n\
[maxvertexcount(3)]\n\
void main(triangle V i[3], inout TriangleStream<V> s) {\n\
  for (int k = 0; k < 3; k++) { V a = i[k]; a.col = float4(1, 0, 0, 1); s.Append(a); }\n\
}\n";

/// The triangle and a fourth vertex: a strip of two triangles.
const GS_STRIP: &str = "struct V { float4 pos : SV_Position; float4 col : COLOR0; };\n\
[maxvertexcount(4)]\n\
void main(triangle V i[3], inout TriangleStream<V> s) {\n\
  V a = i[0]; a.col = float4(0, 1, 0, 1); s.Append(a);\n\
  V b = i[1]; b.col = a.col; s.Append(b);\n\
  V c = i[2]; c.col = a.col; s.Append(c);\n\
  V d = i[2]; d.pos = float4(0.7, 0.8, 0.5, 1); d.col = a.col; s.Append(d);\n\
}\n";

fn geometry_pso(g: &Gfx, gs: &[u8]) -> PsoId {
    let mut d = g.desc();
    d.gs = StageCode::of(gs);
    g.pso(d)
}

#[test]
fn a_geometry_shader_rewrites_the_triangles_it_is_given() {
    let Some(gs) = dxil_or_skip("draw_gs_recolour", "gs_6_6", GS_RECOLOUR) else {
        return;
    };
    let Some(g) = setup() else { return };
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let (_vb, va) = g.vertex_buffer(&HALF.vertices());
    let pso = geometry_pso(&g, &gs);
    let bg = [0.0, 0.0, 0.25, 1.0];
    let mut cmds = g.state(pso, va, 3 * STRIDE);
    cmds.push(begin_pass(t, LoadAction::CLEAR, bg));
    cmds.push(draw(3));
    cmds.push(end_pass());
    let got = g.read(t, cmds);
    let red = Tri {
        c: [1.0, 0.0, 0.0, 1.0],
        ..HALF
    };
    assert_image(&got, &reference(bg, &[red]), "gs recolour");
}

#[test]
fn a_geometry_shader_can_make_more_primitives_than_it_reads() {
    let Some(gs) = dxil_or_skip("draw_gs_strip", "gs_6_6", GS_STRIP) else {
        return;
    };
    let Some(g) = setup() else { return };
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let c = [0.0, 1.0, 0.0, 1.0];
    let tri = Tri {
        p: [[-0.93, -0.87], [0.89, -0.87], [-0.93, 0.91]],
        z: 0.5,
        c,
    };
    let (_vb, va) = g.vertex_buffer(&tri.vertices());
    let pso = geometry_pso(&g, &gs);
    let bg = [0.0, 0.0, 0.0, 1.0];
    let mut cmds = g.state(pso, va, 3 * STRIDE);
    cmds.push(begin_pass(t, LoadAction::CLEAR, bg));
    cmds.push(draw(3));
    cmds.push(end_pass());
    let got = g.read(t, cmds);
    let second = Tri {
        p: [[0.89, -0.87], [-0.93, 0.91], [0.7, 0.8]],
        z: 0.5,
        c,
    };
    assert_image(&got, &reference(bg, &[tri, second]), "gs strip");
}

#[test]
fn indexed_draws_and_several_primitives_go_through_a_geometry_shader() {
    let Some(gs) = dxil_or_skip("draw_gs_recolour2", "gs_6_6", GS_RECOLOUR) else {
        return;
    };
    let Some(g) = setup() else { return };
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let c = [0.0, 1.0, 0.0, 1.0];
    let quad = |z| -> Vec<Vtx> {
        vec![
            ([0.0; 3], [0.0; 4]),
            ([0.0; 3], [0.0; 4]),
            ([-0.83, -0.77, z], c),
            ([0.71, -0.77, z], c),
            ([0.71, 0.69, z], c),
            ([-0.83, 0.69, z], c),
        ]
    };
    let (_vb, va) = g.vertex_buffer(&quad(0.5));
    let ib = g.fx.buffer(BufferKind::UPLOAD, 4096);
    let idx: [u16; 9] = [0, 0, 0, 0, 1, 2, 0, 2, 3];
    let words: Vec<u32> = idx
        .chunks(2)
        .map(|p| u32::from(p[0]) | (u32::from(*p.get(1).unwrap_or(&0)) << 16))
        .collect();
    cpu_write(ib.cpu_addr, &words);
    let pso = geometry_pso(&g, &gs);
    let bg = [0.0, 0.0, 0.0, 1.0];
    let mut cmds = g.state(pso, va, 6 * STRIDE);
    cmds.push(Command::SetIndexBuffer(SetIndexBuffer {
        va: ib.gpu_va,
        size: 18,
        format: IndexFormat::U16,
    }));
    cmds.push(begin_pass(t, LoadAction::CLEAR, bg));
    cmds.push(Command::DrawIndexed(DrawIndexed {
        index_count: 6,
        instance_count: 1,
        start_index: 3,
        base_vertex: 2,
        start_instance: 0,
        _pad: 0,
    }));
    cmds.push(end_pass());
    let got = g.read(t, cmds);
    let red = [1.0, 0.0, 0.0, 1.0];
    let t1 = Tri {
        p: [[-0.83, -0.77], [0.71, -0.77], [0.71, 0.69]],
        z: 0.5,
        c: red,
    };
    let t2 = Tri {
        p: [[-0.83, -0.77], [0.71, 0.69], [-0.83, 0.69]],
        z: 0.5,
        c: red,
    };
    assert_image(&got, &reference(bg, &[t1, t2]), "gs indexed quad");
}

// ---------------------------------------------------------------------------
// ExecuteIndirect of draws and mesh dispatches.

const TRI_A: Tri = Tri {
    p: [[-0.91, -0.87], [0.13, -0.79], [-0.83, 0.17]],
    z: 0.5,
    c: [1.0, 0.0, 0.0, 1.0],
};
const TRI_B: Tri = Tri {
    p: [[0.89, 0.83], [-0.07, 0.77], [0.81, -0.11]],
    z: 0.5,
    c: [0.0, 0.0, 1.0, 1.0],
};

fn signature(kind: IndirectArgKind) -> Vec<u8> {
    let mut v = Vec::new();
    IndirectArg {
        kind,
        index: 0,
        dest_offset: 0,
        count: 0,
    }
    .put(&mut v);
    v
}

impl Gfx {
    fn execute_indirect(
        &self,
        kind: IndirectArgKind,
        stride: u32,
        max_count: u32,
        args: ResId,
        count: Option<ResId>,
    ) -> Command<'static> {
        // The signature bytes live as long as the test.
        let sig: &'static [u8] = Box::leak(signature(kind).into_boxed_slice());
        Command::ExecuteIndirect(ExecuteIndirect {
            args,
            args_offset: 0,
            count: count.unwrap_or(ResId::NULL),
            count_offset: 0,
            max_count,
            stride,
            root_sig: RootSigId::NULL,
            signature: sig,
        })
    }
}

#[test]
fn indirect_draws_run_the_records_a_count_buffer_allows() {
    let Some(g) = setup() else { return };
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let mut v = TRI_A.vertices().to_vec();
    v.extend(TRI_B.vertices());
    let (_vb, va) = g.vertex_buffer(&v);
    let pso = g.pso(g.desc());
    let bg = [0.0, 0.0, 0.0, 1.0];
    let args = g.fx.buffer(BufferKind::UPLOAD, 4096);
    // {vertex count, instance count, start vertex, start instance}
    cpu_write(args.cpu_addr, &[3, 1, 0, 0, 3, 1, 3, 0]);
    let count = g.fx.buffer(BufferKind::UPLOAD, 4096);
    for (n, want) in [
        (None, vec![TRI_A, TRI_B]),
        (Some(1u32), vec![TRI_A]),
        (Some(0), vec![]),
    ] {
        if let Some(n) = n {
            cpu_write(count.cpu_addr, &[n]);
        }
        let mut cmds = g.state(pso, va, 6 * STRIDE);
        cmds.push(begin_pass(t, LoadAction::CLEAR, bg));
        cmds.push(g.execute_indirect(IndirectArgKind::DRAW, 16, 2, args.res, n.map(|_| count.res)));
        cmds.push(end_pass());
        let got = g.read(t, cmds);
        assert_image(
            &got,
            &reference(bg, &want),
            &format!("indirect draws, count {n:?}"),
        );
    }
}

#[test]
fn indirect_indexed_draws_use_the_index_buffer_of_the_list() {
    let Some(g) = setup() else { return };
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let mut v = TRI_A.vertices().to_vec();
    v.extend(TRI_B.vertices());
    let (_vb, va) = g.vertex_buffer(&v);
    let ib = g.fx.buffer(BufferKind::UPLOAD, 4096);
    // 32-bit indices 0..5, so the second record starts at index 3.
    cpu_write(ib.cpu_addr, &[0, 1, 2, 3, 4, 5]);
    let pso = g.pso(g.desc());
    let bg = [0.0, 0.0, 0.0, 1.0];
    let args = g.fx.buffer(BufferKind::UPLOAD, 4096);
    // {index count, instance count, start index, base vertex, start instance}
    cpu_write(args.cpu_addr, &[3, 1, 0, 0, 0, 3, 1, 3, 0, 0]);
    let mut cmds = g.state(pso, va, 6 * STRIDE);
    cmds.push(Command::SetIndexBuffer(SetIndexBuffer {
        va: ib.gpu_va,
        size: 24,
        format: IndexFormat::U32,
    }));
    cmds.push(begin_pass(t, LoadAction::CLEAR, bg));
    cmds.push(g.execute_indirect(IndirectArgKind::DRAW_INDEXED, 20, 2, args.res, None));
    cmds.push(end_pass());
    let got = g.read(t, cmds);
    assert_image(
        &got,
        &reference(bg, &[TRI_A, TRI_B]),
        "indirect indexed draws",
    );
}

#[test]
fn an_indirect_mesh_dispatch_reads_its_groups_and_the_count_buffer() {
    let Some(ms) = dxil_or_skip("draw_ms", "ms_6_6", MS) else {
        return;
    };
    let Some(ps) = dxil_or_skip("draw_ps_mesh", "ps_6_6", PS) else {
        return;
    };
    let Some(mut g) = setup() else { return };
    if !has_mesh(&g.fx) {
        eprintln!("SKIPPED: no mesh shaders on this device");
        return;
    }
    g.ps = ps;
    let t = g.texture(format::R8G8B8A8_UNORM, tex_usage::RENDER_TARGET);
    let mut d = GraphicsPsoDesc::new();
    d.ms = StageCode::of(&ms);
    d.ps = StageCode::of(&g.ps);
    d.rt_count = 1;
    d.rt_formats[0] = format::R8G8B8A8_UNORM;
    d.raster.cull = CullMode::NONE;
    d.depth.depth_enable = 0;
    d.depth.depth_write = 0;
    let pso = g.pso(d);
    let bg = [0.0, 0.0, 0.0, 1.0];
    let args = g.fx.buffer(BufferKind::UPLOAD, 4096);
    // Two records of {x, y, z}: the first dispatches, the second is empty.
    cpu_write(args.cpu_addr, &[1, 1, 1, 0, 1, 1]);
    let count = g.fx.buffer(BufferKind::UPLOAD, 4096);
    let mut green = BIG;
    green.c = [0.0, 1.0, 0.0, 1.0];
    // (count buffer value, records, what is drawn): the plain form reads
    // the application's buffer in place, the others go through the kernel.
    for (n, max, want) in [
        (None, 1, vec![green]),
        (Some(1u32), 2, vec![green]),
        (Some(0), 2, vec![]),
    ] {
        if let Some(n) = n {
            cpu_write(count.cpu_addr, &[n]);
        }
        let mut cmds = vec![
            Command::SetRootSignature(SetRootSignature {
                bind_point: BindPoint::GRAPHICS,
                root_sig: g.sig,
            }),
            Command::SetPipeline(SetPipeline { pso, _pad: 0 }),
            Command::SetViewports(SetViewports::from_slice(&[Viewport::new(
                0.0, 0.0, W as f32, H as f32,
            )])),
            Command::SetScissors(SetScissors::from_slice(&[Rect::new(
                0, 0, W as i32, H as i32,
            )])),
        ];
        cmds.push(begin_pass(t, LoadAction::CLEAR, bg));
        cmds.push(g.execute_indirect(
            IndirectArgKind::DISPATCH_MESH,
            12,
            max,
            args.res,
            n.map(|_| count.res),
        ));
        cmds.push(end_pass());
        let got = g.read(t, cmds);
        assert_image(
            &got,
            &reference(bg, &want),
            &format!("indirect mesh, count {n:?}"),
        );
    }
}
