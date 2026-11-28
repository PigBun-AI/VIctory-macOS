//! Node S-G-006 / plan S4, the title's own geometry shaders (mac S4): `GS_VoxelizeGeneric_*`, a
//! program of the console's on-chip geometry stage, translated by `victory_shader::emit::translate`,
//! draws what it draws on the console.
//!
//! The program takes a triangle from the ESGS ring (LDS: the vertex shader's outputs) and puts the
//! triangle out on the plane of its dominant normal axis, as the voxelisation pass needs (position
//! on the two other axes, the dominant coordinate as depth, the normal and the vertex attributes
//! beside it). Here the vertex and pixel shaders are HLSL through the DXC front end (`SKIPPED`
//! without one), the geometry stage is the lowered program, one triangle per draw, six triangles
//! facing the six directions. The image is compared with a CPU rasterisation of the triangle the
//! program computes: the constants it reads (the scale of the voxel grid) are in a constant buffer in
//! the heap, as the title binds them.
//!
//! The programs are the title's: they are not in the repository. `VICTORY_GS_CONTAINERS` names a
//! folder of their containers (`<hash>.dxbc`, as `victory-census translate --keep` writes them);
//! without it the test prints `SKIPPED`.
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
use victory_shader::HwStage;
use victory_shader::RootType;

const W: u32 = 32;
const H: u32 = 32;
const STRIDE: u32 = 20;

const VS: &str = "struct VSIn { float3 pos : POSITION; float2 uv : TEXCOORD0; };\n\
struct VSOut { float4 pos : SV_Position; float2 uv : TEXCOORD0; };\n\
VSOut main(VSIn i) { VSOut o; o.pos = float4(i.pos, 1); o.uv = i.uv; return o; }\n";

/// What the geometry stage writes (`OSG1`): the axis code, the box, a vertex's own uv, the normal.
const PS: &str = "struct PSIn { float4 pos : SV_Position; nointerpolation uint axis : TEXCOORD0;\n\
  nointerpolation float4 box : TEXCOORD1; float2 uv : TEXCOORD2; nointerpolation float3 n : TEXCOORD3; };\n\
float4 main(PSIn i) : SV_Target0 { return float4(float(i.axis) / 8.0, i.uv.x, i.uv.y, i.pos.z); }\n";

/// The programs of the second kind (five of the title's) take the position alone from the vertex
/// shader and write no uv: the normal takes its place.
const VS_B: &str = "struct VSIn { float3 pos : POSITION; float2 uv : TEXCOORD0; };\n\
struct VSOut { float4 pos : SV_Position; };\n\
VSOut main(VSIn i) { VSOut o; o.pos = float4(i.pos, 1); return o; }\n";

const PS_B: &str = "struct PSIn { float4 pos : SV_Position; nointerpolation uint axis : TEXCOORD0;\n\
  nointerpolation float4 box : TEXCOORD1; nointerpolation float3 n : TEXCOORD3; };\n\
float4 main(PSIn i) : SV_Target0 { return float4(float(i.axis) / 8.0, i.n.x * 0.5 + 0.5, i.n.y * 0.5 + 0.5, i.pos.z); }\n";

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

type V3 = [f64; 3];

/// The program's arithmetic on three vertices: the axis code (0/1: +x/-x ... 4/5: +z/-z) and, per
/// vertex, the output position `(x, y, depth)`.
fn voxelize(p: [V3; 3], scale: V3) -> (u32, [V3; 3], V3) {
    let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
    let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
    let n = [
        e1[1] * e2[2] - e1[2] * e2[1],
        e1[2] * e2[0] - e1[0] * e2[2],
        e1[0] * e2[1] - e1[1] * e2[0],
    ];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    let n = [n[0] / len, n[1] / len, n[2] / len];
    let xy = if n[1].abs() > n[0].abs() {
        if n[1] > 0.0 { 2 } else { 3 }
    } else if n[0] > 0.0 {
        0
    } else {
        1
    };
    let major = if xy >= 2 { n[1] } else { n[0] };
    let code = if n[2].abs() > major.abs() {
        if n[2] > 0.0 { 4 } else { 5 }
    } else {
        xy
    };
    let out = p.map(|v| {
        let s = [v[0] * scale[0], v[1] * scale[1], v[2] * scale[2]];
        let (x, y, d) = match code {
            0 | 1 => (s[2], s[1], s[0]),
            2 | 3 => (s[0], s[2], s[1]),
            _ => (s[0], s[1], s[2]),
        };
        [
            x.clamp(-1.0, 1.0),
            y.clamp(-1.0, 1.0),
            d.clamp(-1.0, 1.0) * 0.5 + 0.5,
        ]
    });
    (code, out, n)
}

/// A triangle whose normal points along `axis` (0-5), in the plane's own two coordinates
/// `(-0.6,-0.5) (0.7,-0.3) (0.1,0.65)` tilted a little, the facing flipped by the order.
fn triangle(axis: usize) -> [V3; 3] {
    let flat = [(-0.6, -0.5), (0.7, -0.3), (0.1, 0.65)];
    let depth = |p: f64, q: f64| 0.1 + 0.1 * p - 0.05 * q;
    let positive = axis % 2 == 0;
    let corners: Vec<(f64, f64)> = if positive {
        flat.to_vec()
    } else {
        vec![flat[0], flat[2], flat[1]]
    };
    corners
        .into_iter()
        .map(|(p, q)| match axis / 2 {
            // the plane x = d(p, q), spanned by (y, z) = (p, q), and so on cyclically
            0 => [depth(p, q), p, q],
            1 => [q, depth(p, q), p],
            _ => [p, q, depth(p, q)],
        })
        .collect::<Vec<_>>()
        .try_into()
        .unwrap()
}

const UVS: [[f64; 2]; 3] = [[0.1, 0.8], [0.5, 0.2], [0.9, 0.6]];

struct Fxs {
    fx: Fx,
    q: QueueId,
    fence: FenceId,
    /// Vertex and pixel shader of each kind of program.
    kinds: [(Vec<u8>, Vec<u8>); 2],
}

fn setup() -> Option<Fxs> {
    let kinds = [
        (
            dxil_or_skip("voxel_vs", "vs_6_6", VS)?,
            dxil_or_skip("voxel_ps", "ps_6_6", PS)?,
        ),
        (
            dxil_or_skip("voxel_vs_b", "vs_6_6", VS_B)?,
            dxil_or_skip("voxel_ps_b", "ps_6_6", PS_B)?,
        ),
    ];
    let fx = Fx::new();
    let q = fx.queue();
    let fence = fx.fence(0);
    Some(Fxs {
        fx,
        q,
        fence,
        kinds,
    })
}

/// The container translated, as the host does (the settings of the run's device).
fn translate(bytes: &[u8]) -> Option<victory_shader::emit::HostShader> {
    victory_shader::emit::set_host_max_wave(32);
    victory_shader::emit::set_view_heap_limit((1 << 21) - 1);
    victory_shader::emit::set_raw_views(1 << 21);
    victory_shader::emit::set_host_rdna(false);
    let h = victory_shader::emit::translate(bytes).ok()?;
    (h.stage == HwStage::Gs).then_some(h)
}

impl Fxs {
    fn settle(&self) {
        let v = self.fx.fence_value(self.fence) + 1;
        assert_eq!(
            self.fx.submit(&[signal_cmd(self.q, self.fence, v)]),
            Status::OK
        );
        assert_eq!(self.fx.wait(self.fence, v), Status::OK);
    }

    /// Draws the six triangles with the geometry shader `gs` (of `h`) and returns the images.
    fn run(
        &self,
        h: &victory_shader::emit::HostShader,
        kind: usize,
    ) -> Result<Vec<Vec<u32>>, String> {
        let dwords = h.user_data_dwords();
        let sig = self.fx.root_sig(&RootSigDesc {
            flags: root_flags::CBV_SRV_UAV_HEAP_INDEXED | root_flags::SAMPLER_HEAP_INDEXED,
            params: vec![constants_param(dwords)],
            ..RootSigDesc::default()
        });
        let mut d = GraphicsPsoDesc::new();
        d.vs = StageCode::of(&self.kinds[kind].0);
        d.gs = StageCode::of(&h.dxil);
        d.ps = StageCode::of(&self.kinds[kind].1);
        d.input_count = 2;
        d.inputs[0] = InputElement::new("POSITION", 0, format::R32G32B32_FLOAT, 0, 0);
        d.inputs[1] = InputElement::new("TEXCOORD", 0, format::R32G32_FLOAT, 0, APPEND_ALIGNED);
        d.rt_count = 1;
        d.rt_formats[0] = format::R8G8B8A8_UNORM;
        d.raster.cull = CullMode::NONE;
        d.depth.depth_enable = 0;
        d.depth.depth_write = 0;
        let mut a = CreateGraphicsPipelineArgs::new();
        a.root_sig = sig;
        a.desc = d;
        let (s, a) = self.fx.call(a);
        if s != Status::OK {
            return Err(format!("{:?}", self.fx.executor().last_error()));
        }
        let pso = a.pso;

        // The constants of the voxel grid at +0x40 of the constant buffer the program's descriptor
        // names, in every slot of the heap it may look in; the shift in the root constants makes the
        // grid half the size, so the scale 2 the buffer holds comes out as 1.
        let cb = self.fx.buffer(BufferKind::UPLOAD, 256);
        let mut words = [0u32; 64];
        for k in 0..3 {
            words[16 + k] = 2.0f32.to_bits();
        }
        cpu_write(cb.cpu_addr, &words);
        let heap = self.fx.heap(HeapKind::CBV_SRV_UAV, 256, true);
        for i in 0..256 {
            self.fx
                .write_view(heap.heap, i, ViewDesc::cbv(cb.gpu_va, 256));
        }
        let mut user = vec![0u32; dwords as usize];
        for u in &h.user_data {
            user[u.index as usize] = match u.root {
                RootType::Constants => 1,
                _ => 0,
            };
        }
        let user_bytes: Vec<u8> = user.iter().flat_map(|w| w.to_le_bytes()).collect();

        Ok((0..6)
            .map(|axis| {
                let tri = triangle(axis);
                let mut vw = Vec::new();
                for (k, p) in tri.iter().enumerate() {
                    vw.extend(p.map(|c| (c as f32).to_bits()));
                    vw.extend(UVS[k].map(|c| (c as f32).to_bits()));
                }
                let vb = self.fx.buffer(BufferKind::UPLOAD, 4096);
                cpu_write(vb.cpu_addr, &vw);
                self.draw(sig, pso, &vb, &heap, &user_bytes)
            })
            .collect())
    }

    fn draw(
        &self,
        sig: RootSigId,
        pso: PsoId,
        vb: &CreateBufferArgs,
        heap: &CreateDescHeapArgs,
        user: &[u8],
    ) -> Vec<u32> {
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
        let rb = self.fx.buffer(BufferKind::READBACK, 8192);
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
                root_sig: sig,
            }),
            Command::SetPipeline(SetPipeline { pso, _pad: 0 }),
            Command::SetDescriptorHeaps(SetDescriptorHeaps {
                cbv_srv_uav: heap.heap,
                sampler: HeapId::NULL,
            }),
            Command::SetRootConstants(SetRootConstants {
                bind_point: BindPoint::GRAPHICS,
                root_index: 0,
                dest_offset: 0,
                data: user,
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

fn centre(px: u32, py: u32) -> (f64, f64) {
    (
        (f64::from(px) + 0.5) / f64::from(W) * 2.0 - 1.0,
        1.0 - (f64::from(py) + 0.5) / f64::from(H) * 2.0,
    )
}

/// The picture of the triangle the program makes of `axis`'s triangle: pixels inside by barycentric
/// coordinates (those near an edge are `None`: either side is right), colour as the pixel shader
/// returns it from the interpolated outputs.
fn reference(axis: usize, kind: usize) -> (u32, Vec<Option<u32>>) {
    let (code, out, n) = voxelize(triangle(axis), [1.0, 1.0, 1.0]);
    let area = (out[1][0] - out[0][0]) * (out[2][1] - out[0][1])
        - (out[2][0] - out[0][0]) * (out[1][1] - out[0][1]);
    let mut img = vec![Some(0); (W * H) as usize];
    for py in 0..H {
        for px in 0..W {
            let (x, y) = centre(px, py);
            let b1 = ((x - out[0][0]) * (out[2][1] - out[0][1])
                - (out[2][0] - out[0][0]) * (y - out[0][1]))
                / area;
            let b2 = ((out[1][0] - out[0][0]) * (y - out[0][1])
                - (x - out[0][0]) * (out[1][1] - out[0][1]))
                / area;
            let b = [1.0 - b1 - b2, b1, b2];
            let least = b.iter().copied().fold(f64::MAX, f64::min);
            img[(py * W + px) as usize] = if least.abs() < 0.04 {
                None
            } else if least < 0.0 {
                Some(0)
            } else {
                let at = |f: &dyn Fn(usize) -> f64| (0..3).map(|k| b[k] * f(k)).sum::<f64>();
                let (g, b) = if kind == 0 {
                    (at(&|k| UVS[k][0]), at(&|k| UVS[k][1]))
                } else {
                    (n[0] * 0.5 + 0.5, n[1] * 0.5 + 0.5)
                };
                Some(pack([f64::from(code) / 8.0, g, b, at(&|k| out[k][2])]))
            };
        }
    }
    (code, img)
}

fn containers() -> Option<Vec<(String, victory_shader::emit::HostShader)>> {
    let Some(dir) = std::env::var_os("VICTORY_GS_CONTAINERS") else {
        eprintln!(
            "SKIPPED the title's geometry shaders: VICTORY_GS_CONTAINERS (a folder of <hash>.dxbc) is not set"
        );
        return None;
    };
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("the container folder")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "dxbc"))
        .collect();
    files.sort();
    let limit: usize = std::env::var("VICTORY_GS_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX);
    let mut out = Vec::new();
    for f in files {
        let Some(h) = translate(&std::fs::read(&f).unwrap()) else {
            continue;
        };
        out.push((f.file_stem().unwrap().to_string_lossy().into_owned(), h));
    }
    // Every distinct program, spread over the folder.
    let every = (out.len() / limit.min(out.len().max(1))).max(1);
    Some(out.into_iter().step_by(every).take(limit).collect())
}

#[test]
fn the_voxelisation_stage_puts_each_triangle_on_the_plane_of_its_dominant_axis() {
    let Some(g) = setup() else { return };
    let Some(all) = containers() else { return };
    assert!(!all.is_empty(), "no geometry program in the folder");
    let mut bad = Vec::new();
    let mut kinds = [0usize; 2];
    for (hash, h) in &all {
        // The first kind whose pixel shader the program's outputs fit.
        let mut made = Err(String::new());
        for kind in 0..2 {
            made = g.run(h, kind).map(|i| (kind, i));
            if made.is_ok() {
                break;
            }
        }
        let (kind, images) = match made {
            Ok(i) => i,
            Err(e) => {
                bad.push(format!("{hash}: no pipeline: {e}"));
                continue;
            }
        };
        kinds[kind] += 1;
        for (axis, got) in images.iter().enumerate() {
            let (code, want) = reference(axis, kind);
            assert_eq!(
                code as usize, axis,
                "the test triangle {axis} faces another way"
            );
            let drawn = want.iter().flatten().filter(|&&w| w != 0).count();
            assert!(
                drawn > 40,
                "the reference of triangle {axis} is nearly empty"
            );
            for i in 0..(W * H) as usize {
                if let Some(w) = want[i] {
                    if !close(got[i], w, 4) {
                        bad.push(format!(
                            "{hash}, triangle {axis}: pixel ({},{}) is {:#010x}, wanted {w:#010x}",
                            i as u32 % W,
                            i as u32 / W,
                            got[i]
                        ));
                        break;
                    }
                }
            }
        }
    }
    eprintln!(
        "{} geometry programs ({} with uvs, {} without), 6 draws each",
        all.len(),
        kinds[0],
        kinds[1]
    );
    assert!(
        bad.is_empty(),
        "{} of {} images differ:\n{}",
        bad.len(),
        all.len() * 6,
        bad.join("\n")
    );
}
