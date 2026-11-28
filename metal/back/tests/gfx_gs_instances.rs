//! A rectangle list drawn with several instances: every instance's rectangle is drawn once.
//!
//! The runtime header's helper puts the draw's instance count into `IRRuntimeDrawInfo.gsInstanceCount`
//! as well as into the grid's height, and the emulation's stages then run each instance's threadgroup
//! for all instances: a draw of N instances drew N times N rectangles (the title's tile pass,
//! 14,580 instances at 1440p, took five seconds a frame). `gsInstanceCount` is the geometry shader's
//! own instance count. Additive blending counts the layers: three instances must give three layers.
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
struct VSOut { float4 pos : SV_Position; float4 col : TEXCOORD0; };\n\
VSOut main(VSIn i) { VSOut o; o.pos = float4(i.pos, 1); o.col = float4(0.1, 0.1, 0.1, 0.0); return o; }\n";

const PS: &str = "struct VSOut { float4 pos : SV_Position; float4 col : TEXCOORD0; };\n\
float4 main(VSOut i) : SV_Target0 { return i.col; }\n";

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
        d.blend.rt[0].enable = 1;
        d.blend.rt[0].src = BlendFactor::ONE;
        d.blend.rt[0].dst = BlendFactor::ONE;
        d.blend.rt[0].op = BlendOp::ADD;
        d.blend.rt[0].src_alpha = BlendFactor::ONE;
        d.blend.rt[0].dst_alpha = BlendFactor::ONE;
        d.blend.rt[0].op_alpha = BlendOp::ADD;
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
    fn draw(&self, corners: [(f32, f32); 3], instances: u32) -> Vec<u32> {
        let mut words = Vec::new();
        for (x, y) in corners {
            words.extend([x.to_bits(), y.to_bits(), 0.5f32.to_bits()]);
            words.extend([8.0f32.to_bits(); 4]);
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
            clear: [0.0, 0.0, 0.0, 0.0],
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
                instance_count: instances,
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

#[test]
fn every_instance_of_a_rectangle_list_is_drawn_once() {
    let Some(g) = setup() else { return };
    let corners = [(-0.5f32, 0.5f32), (-0.5, -0.5), (0.5, 0.5)];
    let centre = (W * (H / 2) + W / 2) as usize;
    for n in [1u32, 3, 7] {
        let got = g.draw(corners, n);
        // 0.1 a layer is 25.5 of 255 levels.
        let red = (got[centre] & 255) as i32;
        let want = (25.5 * f64::from(n)).round() as i32;
        assert!(
            (red - want).abs() <= n as i32 + 1,
            "{n} instances: the centre pixel's red is {red}, {want} for {n} layers"
        );
    }
}
