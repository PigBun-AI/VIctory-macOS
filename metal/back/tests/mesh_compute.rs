//! Mesh shader as a compute pass (docs/metal/mesh-compute.md), end to end on the
//! GPU through the proto interpreter.
//!
//! A synthetic mesh shader (HLSL through DXC; no game data) whose declared
//! output is over Metal's 32 KB is lowered to a compute shader by
//! `tools/metal/ms2cs.py` (the prototype of the translator's lowering), run as a
//! `Dispatch`, and drawn by a vertex shader that fetches the scratch it wrote.
//! The picture is compared with Metal's own mesh path on a *small* mesh (the
//! same triangles and the same interpolants, 10 KB of output instead of 40 KB),
//! and the scratch is checked on the CPU.
//!
//! Needs DXC (`VICTORY_HELPER_DXC`, `DXC_LIB`) and the two text tools
//! (`tools/metal/dxc-text-tools.sh`, `VICTORY_DXC_TEXT_TOOLS`); prints SKIPPED
//! and passes without them. `real_pair_builds_where_the_mesh_pipeline_crashes`
//! also needs `VICTORY_MC_REAL_MS` and `VICTORY_MC_REAL_PS` (DXIL files of a
//! game pair, kept outside the repository).
#![cfg(target_vendor = "apple")]

mod common_msc;
mod exec_support;

use std::path::{Path, PathBuf};
use std::process::Command as Process;

use common_msc::{dxil_or_skip, dxil_result};
use exec_support::*;
use victory_metal_back::mesh_compute::{Element, Layout, Region, Scalar, kind};
use victory_metal_back::mesh_emu::{self, Bundle};
use victory_metal_back::mesh_fit;
use victory_metal_back::msc::RootSignatureDesc as BackRoot;
use victory_metal_back::pipeline_defaults::{Component, Stages, Variant, default_graphics};
use victory_metal_proto::cmd::*;
use victory_metal_proto::ids::*;
use victory_metal_proto::indirect::{ExecuteIndirect, IndirectArg, IndirectArgKind};
use victory_metal_proto::pso::*;
use victory_metal_proto::render::*;
use victory_metal_proto::root::*;
use victory_metal_proto::status::Status;
use victory_metal_proto::sync::{
    CreateComputePipelineArgs, CreateGraphicsPipelineArgs, CreateTextureArgs,
};
use victory_metal_proto::types::*;
use victory_metal_proto::wire::Wire;

const W: u32 = 48;
const H: u32 = 48;
/// Triangles one group makes: an 8 x 8 grid, one triangle a cell.
const TRIS: u32 = 64;
const GROUPS: u32 = 2;

// ---------------------------------------------------------------------------
// the lowering: DXC's disassembler, tools/metal/ms2cs.py, DXC's assembler
// ---------------------------------------------------------------------------

fn tools_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("VICTORY_DXC_TEXT_TOOLS").map(PathBuf::from)?;
    (dir.join("dxcdis").exists() && dir.join("dxcasm").exists()).then_some(dir)
}

fn run_x86(tool: &Path, args: &[&Path], stdout: Option<&Path>) {
    let mut p = Process::new("/usr/bin/arch");
    p.arg("-x86_64").arg(tool).args(args);
    let out = p.output().expect("run a DXC text tool");
    assert!(
        out.status.success(),
        "{} failed: {}",
        tool.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    if let Some(f) = stdout {
        std::fs::write(f, &out.stdout).unwrap();
    }
}

/// The mesh shader's compute form and its scratch layout, or `None` (SKIPPED)
/// without the tools. `Err` is the lowering's refusal.
fn lower(name: &str, ms: &[u8]) -> Option<Result<(Vec<u8>, Layout), String>> {
    lower_text(name, ms).map(|r| r.map(|(cs, l, _)| (cs, l)))
}

/// [`lower`] with the layout text too (what the `VMCS` part carries).
type Lowered = (Vec<u8>, Layout, String);

fn lower_text(name: &str, ms: &[u8]) -> Option<Result<Lowered, String>> {
    let Some(tools) = tools_dir() else {
        eprintln!("SKIPPED {name}: set VICTORY_DXC_TEXT_TOOLS (tools/metal/dxc-text-tools.sh)");
        return None;
    };
    static CALLS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("victory-ms2cs-{}-{n}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (src, ll, cs_ll, lay, out) = (
        dir.join("ms.dxil"),
        dir.join("ms.ll"),
        dir.join("cs.ll"),
        dir.join("cs.layout"),
        dir.join("cs.dxil"),
    );
    std::fs::write(&src, ms).unwrap();
    run_x86(&tools.join("dxcdis"), &[&src], Some(&ll));
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/metal/ms2cs.py");
    let conv = Process::new("python3")
        .arg(&script)
        .args([&ll, &cs_ll, &lay])
        .output()
        .expect("run python3");
    if !conv.status.success() {
        let e = String::from_utf8_lossy(&conv.stderr).trim().to_owned();
        let _ = std::fs::remove_dir_all(&dir);
        return Some(Err(e));
    }
    run_x86(&tools.join("dxcasm"), &[&cs_ll, &out], None);
    let text = std::fs::read_to_string(&lay).unwrap();
    let result = (
        std::fs::read(&out).unwrap(),
        Layout::parse(&text).expect("a layout"),
        text,
    );
    let _ = std::fs::remove_dir_all(&dir);
    Some(Ok(result))
}

// ---------------------------------------------------------------------------
// the vertex shader that reads the scratch (the translator makes it in
// production; HLSL here)
// ---------------------------------------------------------------------------

fn modifiers(e: &Element) -> &'static str {
    if e.scalar != Scalar::Float {
        return "nointerpolation ";
    }
    match e.interpolation {
        1 => "nointerpolation ",
        3 => "centroid ",
        4 => "noperspective ",
        5 => "noperspective centroid ",
        6 => "sample ",
        7 => "noperspective sample ",
        _ => "",
    }
}

fn hlsl_type(e: &Element) -> String {
    let base = match e.scalar {
        Scalar::Float => "float",
        Scalar::Uint | Scalar::Bool => "uint",
        Scalar::Int => "int",
    };
    let n = e.rows * e.cols;
    if n == 1 {
        base.to_owned()
    } else {
        format!("{base}{n}")
    }
}

/// `scr.Load..(at)` of an element, as its own type.
fn load(e: &Element, at: &str) -> String {
    let n = e.rows * e.cols;
    let raw = match n {
        1 => format!("scr.Load({at})"),
        _ => format!("scr.Load{n}({at})"),
    };
    match e.scalar {
        Scalar::Float => format!("asfloat({raw})"),
        Scalar::Int => format!("asint({raw})"),
        _ => raw,
    }
}

/// The vertex shader of a layout: `SV_VertexID` is `3 * primitive + corner`,
/// `SV_InstanceID` the threadgroup; a primitive past the group's count, or
/// culled, is made degenerate (all three corners at one point, w = 0).
fn vs_hlsl(l: &Layout) -> String {
    let mut members = String::new();
    let mut fetch = String::new();
    let mut n = 0;
    for e in &l.elements {
        if e.kind == kind::CULL_PRIMITIVE {
            continue;
        }
        let name = format!("{}{}", e.name, e.index);
        let (member, record) = if e.kind == kind::POSITION {
            ("float4 pos : SV_Position;\n".to_owned(), "vb")
        } else {
            n += 1;
            (
                format!("{}{} m{n} : {name};\n", modifiers(e), hlsl_type(e)),
                if e.region == Region::Vertex {
                    "vb"
                } else {
                    "pb"
                },
            )
        };
        members.push_str(&format!("  {member}"));
        let field = if e.kind == kind::POSITION {
            "pos".to_owned()
        } else {
            format!("m{n}")
        };
        fetch.push_str(&format!(
            "    o.{field} = {};\n",
            load(e, &format!("{record} + {}", e.offset))
        ));
    }
    let cull = l
        .elements
        .iter()
        .find(|e| e.kind == kind::CULL_PRIMITIVE)
        .map_or(String::new(), |e| {
            format!("    if (scr.Load(pb + {}) != 0) dead = true;\n", e.offset)
        });
    format!(
        "ByteAddressBuffer scr : register(t{reg}, space{space});\n\
struct O {{\n{members}}};\n\
O main(uint vid : SV_VertexID, uint grp : SV_InstanceID) {{\n\
  O o = (O)0;\n\
  uint prim = vid / 3, corner = vid % 3;\n\
  uint base = grp * {gstride};\n\
  bool dead = prim >= scr.Load(base + 4);\n\
  uint pb = base + {parea} + prim * {pstride};\n\
  if (!dead) {{\n\
    uint v = (scr.Load(pb) >> (8 * corner)) & 255;\n\
    uint vb = base + {vhead} + v * {vstride};\n\
{cull}{fetch}  }}\n\
  if (dead) o.pos = float4(0, 0, 0, 0);\n\
  return o;\n}}\n",
        reg = l.register,
        space = l.space,
        gstride = l.group_stride,
        parea = l.prim_area,
        pstride = l.prim_stride,
        vhead = l.vertex_head,
        vstride = l.vertex_stride,
    )
}

// ---------------------------------------------------------------------------
// the synthetic shaders
// ---------------------------------------------------------------------------

/// The mesh shader: `GROUPS` groups of an 8 x 8 grid of triangles (group `g`
/// above group `g - 1`), every 7th culled with `SV_CullPrimitive`, a
/// per-primitive tint. `big` has the colour in pieces and eight `float` +
/// `float3` pairs a vertex: 192 vertices of 160 bytes by D3D12's count (DXC
/// accepts: 30.7 KB with the primitives), 192 bytes by Metal's, 38 KB, over its
/// 32 KB (the translator's S2 or `mesh_fit` repack would rescue this one; the
/// compute form does not care, and the test needs a shader Metal refuses as it
/// is). The small one carries the colour in one `float4`. Both give the pixel
/// shader the same values.
fn mesh_hlsl(big: bool) -> String {
    mesh_hlsl_reading(big, false)
}

/// `mesh_hlsl`, the tint scaled by the first word of a root SRV `src`
/// (`t0`, space 0) when `reads`: with the word 1 the picture is the same.
fn mesh_hlsl_reading(big: bool, reads: bool) -> String {
    mesh_hlsl_with(big, reads, false)
}

/// `unfixable`: two `SV_ClipDistance` floats instead of two of the `float`
/// members. D3D12 counts them once (the same 32000 bytes), Metal twice, which
/// no order of the members repacks: the mesh Metal refuses for good, which
/// only the compute form takes.
fn mesh_hlsl_with(big: bool, reads: bool, unfixable: bool) -> String {
    let (decl, scale) = if reads {
        (
            "ByteAddressBuffer src : register(t0);\n",
            "float(src.Load(0))",
        )
    } else {
        ("", "1.0")
    };
    let (members, writes) = if big {
        // A `float` and a `float3` a pair: Metal puts each `float3` at a
        // multiple of 12, so every pair costs it padding D3D12 does not count.
        let mut m = String::from("  float4 c : COLOR0;\n");
        let mut w = String::from("      verts[v].c = float4(tf, 0, 0, 1);\n");
        for j in 0..8 {
            if !(unfixable && j >= 6) {
                m.push_str(&format!("  float s{j} : TEXCOORD{};\n", 2 * j));
                w.push_str(&format!("      verts[v].s{j} = {j}.0 * 0.01;\n"));
            }
            m.push_str(&format!("  float3 v{j} : TEXCOORD{};\n", 2 * j + 1));
            w.push_str(&format!(
                "      verts[v].v{j} = float3({j}.0 * 0.02, k * 0.5, 0.25 + 0.5 * tf);\n"
            ));
        }
        if unfixable {
            m.push_str("  float2 clip : SV_ClipDistance;\n");
            w.push_str("      verts[v].clip = float2(1, 1);\n");
        }
        (m, w)
    } else {
        (
            "  float4 c : COLOR0;\n".to_owned(),
            "      verts[v].c = float4(tf, k * 0.5, 0.25 + 0.5 * tf, 1);\n".to_owned(),
        )
    };
    format!(
        "{decl}struct V {{\n  float4 pos : SV_Position;\n{members}  nointerpolation uint idv : TEXCOORD30;\n}};\n\
struct P {{ float4 tint : COLOR2; bool cull : SV_CullPrimitive; }};\n\
[numthreads(128,1,1)] [outputtopology(\"triangle\")]\n\
void main(uint tid : SV_GroupIndex, uint3 gid : SV_GroupID,\n\
          out vertices V verts[{nv}], out primitives P prims[{TRIS}], out indices uint3 tris[{TRIS}]) {{\n\
  SetMeshOutputCounts({nv}, {TRIS});\n\
  if (tid < {TRIS}) {{\n\
    uint t = tid;\n\
    float cw = 2.0 / 8.0, ch = 2.0 / 16.0;\n\
    float x0 = -1.0 + (t % 8) * cw, y0 = -1.0 + ((t / 8) + 8 * gid.x) * ch;\n\
    float2 p[3] = {{ float2(x0 + 0.1 * cw, y0 + 0.1 * ch), float2(x0 + 0.9 * cw, y0 + 0.2 * ch), float2(x0 + 0.3 * cw, y0 + 0.9 * ch) }};\n\
    float tf = (t + 64.0 * gid.x) / 128.0;\n\
    for (uint k = 0; k < 3; k++) {{\n\
      uint v = 3 * t + k;\n\
      verts[v].pos = float4(p[k], 0.5, 1);\n      verts[v].idv = v;\n{writes}    }}\n\
    tris[t] = uint3(3 * t, 3 * t + 1, 3 * t + 2);\n\
    prims[t].tint = float4((1.0 - 0.25 * (t & 1)) * {scale}, 0, 0, 0);\n\
    prims[t].cull = (t % 7) == 3;\n\
  }}\n}}\n",
        nv = 3 * TRIS,
    )
}

const PS_SMALL: &str = "struct I { float4 pos : SV_Position; float4 c : COLOR0; nointerpolation float4 tint : COLOR2;\n\
  nointerpolation uint idv : TEXCOORD30; };\n\
float4 main(I i) : SV_Target0 {\n\
  float3 c = i.c.xyz * i.tint.x;\n\
  c.z += (i.idv & 3) * 0.1;\n\
  return float4(c, 1); }\n";

const PS_BIG: &str = "struct I { float4 pos : SV_Position; float4 c : COLOR0; float3 v3 : TEXCOORD7; float3 v5 : TEXCOORD11;\n\
  nointerpolation float4 tint : COLOR2; nointerpolation uint idv : TEXCOORD30; };\n\
float4 main(I i) : SV_Target0 {\n\
  float3 c = float3(i.c.x, i.v3.y, i.v5.z) * i.tint.x;\n\
  c.z += (i.idv & 3) * 0.1;\n\
  return float4(c, 1); }\n";

/// The pixel shader of the unfixable mesh: reads every output the mesh writes
/// (an output the pixel shader does not read is dropped by `mesh_fit`, which
/// would rescue the mesh), and draws what `PS_BIG` draws.
fn ps_all() -> String {
    let mut members = String::from("float4 pos : SV_Position; float4 c : COLOR0;\n");
    let mut sum = String::new();
    for j in 0..8 {
        if j < 6 {
            members.push_str(&format!("float s{j} : TEXCOORD{};\n", 2 * j));
            sum.push_str(&format!(" + i.s{j}"));
        }
        members.push_str(&format!("float3 v{j} : TEXCOORD{};\n", 2 * j + 1));
        sum.push_str(&format!(" + i.v{j}.x"));
    }
    format!(
        "struct I {{ {members}nointerpolation float4 tint : COLOR2; nointerpolation uint idv : TEXCOORD30; }};\n\
float4 main(I i) : SV_Target0 {{\n\
  float3 c = float3(i.c.x, i.v3.y, i.v5.z) * i.tint.x;\n\
  c.z += (i.idv & 3) * 0.1;\n\
  c.x += 0.0 * (0.0{sum});\n\
  return float4(c, 1); }}\n"
    )
}

// ---------------------------------------------------------------------------
// the machinery
// ---------------------------------------------------------------------------

struct Gfx {
    fx: Fx,
    q: QueueId,
    fence: FenceId,
}

fn setup() -> Option<Gfx> {
    setup_with(Fx::new())
}

fn setup_with(fx: Fx) -> Option<Gfx> {
    if !fx.executor().device().unwrap().caps().mesh_shaders {
        eprintln!("SKIPPED: no mesh shaders on this device");
        return None;
    }
    let q = fx.queue();
    let fence = fx.fence(0);
    Some(Gfx { fx, q, fence })
}

fn root_param(kind: RootParamKind, register: u32, space: u32) -> RootParam {
    RootParam {
        kind,
        shader_register: register,
        register_space: space,
        ..RootParam::default()
    }
}

fn graphics_root(fx: &Fx, extra: Vec<RootParam>, flags: u32) -> RootSigId {
    fx.root_sig(&RootSigDesc {
        flags,
        params: extra,
        ..RootSigDesc::default()
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

    fn graphics_pso(
        &self,
        sig: RootSigId,
        vs: Option<&[u8]>,
        ms: Option<&[u8]>,
        ps: &[u8],
    ) -> Result<PsoId, (Status, String)> {
        let mut d = GraphicsPsoDesc::new();
        if let Some(vs) = vs {
            d.vs = StageCode::of(vs);
        }
        if let Some(ms) = ms {
            d.ms = StageCode::of(ms);
        }
        d.ps = StageCode::of(ps);
        d.rt_count = 1;
        d.rt_formats[0] = format::R8G8B8A8_UNORM;
        d.raster.cull = CullMode::NONE;
        d.depth.depth_enable = 0;
        d.depth.depth_write = 0;
        let mut a = CreateGraphicsPipelineArgs::new();
        a.root_sig = sig;
        a.desc = d;
        let (s, a) = self.fx.call(a);
        if s == Status::OK {
            Ok(a.pso)
        } else {
            Err((s, self.fx.executor().last_error().unwrap_or_default()))
        }
    }

    fn compute_pso(&self, sig: RootSigId, cs: &[u8]) -> Result<PsoId, (Status, String)> {
        let mut a = CreateComputePipelineArgs::new();
        a.root_sig = sig;
        a.dxil_addr = cs.as_ptr() as u64;
        a.dxil_len = cs.len() as u64;
        let (s, a) = self.fx.call(a);
        if s == Status::OK {
            Ok(a.pso)
        } else {
            Err((s, self.fx.executor().last_error().unwrap_or_default()))
        }
    }

    /// Runs `before` (outside a pass), then `draw` inside a pass over a
    /// cleared target, and reads the target back.
    fn picture(
        &self,
        sig: RootSigId,
        pso: PsoId,
        before: Vec<Command<'static>>,
        srv_va: Option<u64>,
        draw: Command<'static>,
    ) -> Vec<u32> {
        let mut state = vec![
            Command::SetRootSignature(SetRootSignature {
                bind_point: BindPoint::GRAPHICS,
                root_sig: sig,
            }),
            Command::SetPipeline(SetPipeline { pso, _pad: 0 }),
        ];
        if let Some(va) = srv_va {
            state.push(graphics_va(0, RootParamKind::SRV, va));
        }
        self.picture_in(before, state, vec![draw])
    }

    /// `before` and `state` outside a pass, `in_pass` inside one over a
    /// cleared target; the target read back.
    fn picture_in(
        &self,
        before: Vec<Command<'static>>,
        state: Vec<Command<'static>>,
        in_pass: Vec<Command<'static>>,
    ) -> Vec<u32> {
        let (tex, rb) = self.target();
        let mut cmds = vec![begin(1, self.q)];
        cmds.extend(before);
        cmds.extend(state);
        cmds.extend(self.pass_around(tex, in_pass));
        cmds.extend(self.read_back(tex, &rb));
        cmds.push(end(1));
        assert_eq!(
            self.fx.submit(&cmds),
            Status::OK,
            "{:?}",
            self.fx.executor().last_error()
        );
        self.settle();
        cpu_read(rb.cpu_addr, (W * H) as usize)
    }

    fn target(&self) -> (ResId, CreateBufferArgs) {
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
        let rb = self.fx.buffer(BufferKind::READBACK, u64::from(W * H * 4));
        cpu_write(rb.cpu_addr, &vec![0xDEAD_BEEF; (W * H) as usize]);
        (tex, rb)
    }

    /// Viewport, scissor and topology, then a pass over `tex` (cleared to
    /// opaque black) holding `in_pass`.
    fn pass_around(&self, tex: ResId, in_pass: Vec<Command<'static>>) -> Vec<Command<'static>> {
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
        let mut cmds = vec![
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
            Command::BeginRenderPass(pass),
        ];
        cmds.extend(in_pass);
        cmds.push(Command::EndRenderPass(<EndRenderPass as Wire>::ZERO));
        cmds
    }

    fn read_back(&self, tex: ResId, rb: &CreateBufferArgs) -> Vec<Command<'static>> {
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
        vec![
            Command::Barrier(Barrier::memory(
                stage::FRAGMENT | stage::VERTEX,
                stage::BLIT,
            )),
            Command::CopyTextureRegion(c),
        ]
    }
}

fn graphics_va(root_index: u32, kind: RootParamKind, gpu_va: u64) -> Command<'static> {
    Command::SetRootDescriptor(SetRootDescriptor {
        bind_point: BindPoint::GRAPHICS,
        root_index,
        kind,
        _pad: 0,
        gpu_va,
    })
}

fn close(a: u32, b: u32, tol: i32) -> bool {
    (0..4).all(|i| {
        let (x, y) = (((a >> (8 * i)) & 255) as i32, ((b >> (8 * i)) & 255) as i32);
        (x - y).abs() <= tol
    })
}

fn f32_at(words: &[u32], byte: u32) -> f32 {
    f32::from_bits(words[(byte / 4) as usize])
}

/// What the compute form of `ms` and the vertex shader of its layout draw,
/// and the scratch the compute pass left (as words).
fn emulate(g: &Gfx, ms: &[u8], ps: &[u8]) -> Option<(Vec<u32>, Vec<u32>, Layout)> {
    let (cs, layout) = match lower("emulate", ms)? {
        Ok(x) => x,
        Err(e) => panic!("the lowering refused the shader: {e}"),
    };
    let vs = dxil_or_skip("emulate_vs", "vs_6_6", &vs_hlsl(&layout))?;
    // The compute form's root signature: a root UAV at the scratch's
    // register and space. The vertex and pixel stages read it as a root SRV.
    let csig = graphics_root(
        &g.fx,
        vec![root_param(
            RootParamKind::UAV,
            layout.register,
            layout.space,
        )],
        0,
    );
    let gsig = graphics_root(
        &g.fx,
        vec![root_param(
            RootParamKind::SRV,
            layout.register,
            layout.space,
        )],
        0,
    );
    let cpso = g
        .compute_pso(csig, &cs)
        .unwrap_or_else(|e| panic!("compute pipeline: {e:?}"));
    let gpso = g
        .graphics_pso(gsig, Some(&vs), None, ps)
        .unwrap_or_else(|e| panic!("vertex pipeline: {e:?}"));
    let scratch_bytes = layout.scratch_bytes(u64::from(GROUPS));
    let scratch = g.fx.buffer(BufferKind::SHARED, scratch_bytes);
    // Poison, so a word nothing wrote shows.
    cpu_write(
        scratch.cpu_addr,
        &vec![0xA5A5_A5A5; (scratch_bytes / 4) as usize],
    );
    // The compute pass, then (the scratch written, visible to the vertex
    // stage) the draw: one instance of the vertex shader's grid per group.
    let before = vec![
        set_sig(csig),
        Command::SetPipeline(SetPipeline { pso: cpso, _pad: 0 }),
        root_va(0, RootParamKind::UAV, scratch.gpu_va),
        dispatch(GROUPS, 1, 1),
        Command::Barrier(Barrier::memory(stage::DISPATCH, stage::VERTEX)),
    ];
    let words = scratch_bytes as usize / 4;
    let draw = Command::Draw(Draw {
        vertex_count: layout.draw_vertices(),
        instance_count: GROUPS,
        start_vertex: 0,
        start_instance: 0,
    });
    let image = g.picture(gsig, gpso, before, Some(scratch.gpu_va), draw);
    Some((image, cpu_read(scratch.cpu_addr, words), layout))
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

/// Metal's own mesh path on the small mesh: the picture the emulation must give.
fn native_picture(g: &Gfx) -> Option<Vec<u32>> {
    let ms = dxil_or_skip("mc_ms_small", "ms_6_6", &mesh_hlsl(false))?;
    let ps = dxil_or_skip("mc_ps_small", "ps_6_6", PS_SMALL)?;
    let sig = graphics_root(&g.fx, vec![], 0);
    let pso = g
        .graphics_pso(sig, None, Some(&ms), &ps)
        .unwrap_or_else(|e| panic!("the small mesh is refused: {e:?}"));
    Some(g.picture(
        sig,
        pso,
        vec![],
        None,
        Command::DispatchMesh(DispatchMesh {
            x: GROUPS,
            y: 1,
            z: 1,
            _pad: 0,
        }),
    ))
}

#[test]
fn metal_refuses_the_big_mesh() {
    let Some(ms) = dxil_or_skip("mc_ms_big_refused", "ms_6_6", &mesh_hlsl(true)) else {
        return;
    };
    let ps = dxil_or_skip("mc_ps_big_refused", "ps_6_6", PS_BIG).unwrap();
    let fx = Fx::new();
    let device = fx.executor().device().unwrap();
    if !device.caps().mesh_shaders {
        eprintln!("SKIPPED: no mesh shaders on this device");
        return;
    }
    let root = BackRoot::default();
    let desc = default_graphics(
        &root,
        Stages {
            ps: Some(&ps),
            ms: Some(&ms),
            ..Stages::default()
        },
        &[],
        &[Component::Float],
        false,
        Variant::Plain,
    );
    // 192 x 160 + 64 x 20 = 32000 by D3D12's count, which DXC accepts; Metal's
    // adds 4 bytes of padding per `float3` (the member order is the signature's).
    let refused = device
        .graphics_pipeline_from_dxil(&desc)
        .err()
        .expect("over 32 KB for Metal");
    assert!(mesh_fit::is_size_refusal(&refused), "{refused}");
    // This one is only padding: the repack would rescue it. What the compute
    // form is for is what no order of the members rescues.
    assert!(
        mesh_fit::retry(&device, &desc, &refused).is_some_and(|r| r.is_ok()),
        "the repack makes the synthetic mesh fit"
    );
}

#[test]
fn the_compute_form_draws_what_metal_draws_on_the_small_mesh() {
    let Some(g) = setup() else { return };
    let Some(native) = native_picture(&g) else {
        return;
    };
    let big = dxil_or_skip("mc_ms_big", "ms_6_6", &mesh_hlsl(true)).unwrap();
    let ps = dxil_or_skip("mc_ps_big", "ps_6_6", PS_BIG).unwrap();
    let Some((emulated, scratch, l)) = emulate(&g, &big, &ps) else {
        return;
    };
    // the shader the emulation lowered really was over the limit
    assert_eq!((l.max_vertices, l.max_primitives), (3 * TRIS, TRIS));
    // the picture: Metal's, within one level
    let mut lit = 0;
    for (i, (a, b)) in emulated.iter().zip(&native).enumerate() {
        assert!(
            close(*a, *b, 1),
            "pixel ({}, {}): emulated {a:#010x}, native {b:#010x}",
            i as u32 % W,
            i as u32 / W
        );
        lit += u32::from(*a != 0xFF00_0000);
    }
    eprintln!("emulated picture: {lit} of {} pixels lit", W * H);
    assert!(lit > 150, "the picture is almost empty: {lit} pixels");

    // the scratch: counts, packed indices, the cull flag, positions
    let cull = l
        .elements
        .iter()
        .find(|e| e.kind == kind::CULL_PRIMITIVE)
        .unwrap()
        .offset;
    let pos = l
        .elements
        .iter()
        .find(|e| e.kind == kind::POSITION)
        .unwrap()
        .offset;
    for grp in 0..GROUPS {
        let base = grp * l.group_stride;
        assert_eq!(scratch[(base / 4) as usize], 3 * TRIS, "vertex count");
        assert_eq!(scratch[(base / 4) as usize + 1], TRIS, "primitive count");
        for t in 0..TRIS {
            let pb = base + l.prim_area + t * l.prim_stride;
            let want = (3 * t) | ((3 * t + 1) << 8) | ((3 * t + 2) << 16);
            assert_eq!(scratch[(pb / 4) as usize], want, "indices of {t}");
            assert_eq!(
                scratch[((pb + cull) / 4) as usize],
                u32::from(t % 7 == 3),
                "cull of {t}"
            );
        }
        // vertex 0 of triangle 0: x0 + 0.1 cw, y0 + 0.1 ch
        let vb = base + l.vertex_head;
        let (cw, ch) = (2.0f32 / 8.0, 2.0f32 / 16.0);
        let y0 = -1.0 + (8 * grp) as f32 * ch;
        assert!((f32_at(&scratch, vb + pos) - (-1.0 + 0.1 * cw)).abs() < 1e-5);
        assert!((f32_at(&scratch, vb + pos + 4) - (y0 + 0.1 * ch)).abs() < 1e-5);
        assert_eq!(f32_at(&scratch, vb + pos + 12), 1.0);
    }
}

#[test]
fn the_lowering_refuses_what_it_does_not_handle() {
    // A mesh shader with a payload (an amplification stage in front).
    let src = "struct Payload { uint count; };\n\
struct V { float4 pos : SV_Position; };\n\
[numthreads(32,1,1)] [outputtopology(\"triangle\")]\n\
void main(uint tid : SV_GroupIndex, in payload Payload pl, out vertices V verts[3], out indices uint3 tris[1]) {\n\
  SetMeshOutputCounts(3, 1);\n\
  if (tid < 3) verts[tid].pos = float4(pl.count, 0, 0, 1);\n\
  if (tid == 0) tris[0] = uint3(0, 1, 2);\n}\n";
    let Some(ms) = dxil_or_skip("mc_payload", "ms_6_6", src) else {
        return;
    };
    let Some(r) = lower("payload", &ms) else {
        return;
    };
    let e = r.expect_err("a payload is refused");
    assert!(e.contains("payload"), "{e}");
}

/// A real pair (outside the repository): the mesh pipeline crashes Apple's GPU
/// compiler; the compute form and the vertex form build.
#[test]
fn real_pair_builds_where_the_mesh_pipeline_crashes() {
    let (Some(ms), Some(ps)) = (
        std::env::var_os("VICTORY_MC_REAL_MS"),
        std::env::var_os("VICTORY_MC_REAL_PS"),
    ) else {
        eprintln!("SKIPPED real pair: set VICTORY_MC_REAL_MS and VICTORY_MC_REAL_PS");
        return;
    };
    let Some(g) = setup() else { return };
    let (ms, ps) = (std::fs::read(ms).unwrap(), std::fs::read(ps).unwrap());
    let Some(lowered) = lower("real", &ms) else {
        return;
    };
    let (cs, layout) = lowered.expect("the real mesh shader is lowered");
    eprintln!(
        "real: {} vertices x {} B + {} primitives x {} B = {} B a group, {} elements",
        layout.max_vertices,
        layout.vertex_stride,
        layout.max_primitives,
        layout.prim_stride,
        layout.group_stride,
        layout.elements.len()
    );
    let vs = dxil_or_skip("real_vs", "vs_6_6", &vs_hlsl(&layout)).unwrap();
    let heap = root_flags::CBV_SRV_UAV_HEAP_INDEXED | root_flags::SAMPLER_HEAP_INDEXED;
    // What the translator's convention gives this mesh shader (census
    // `root_signature`): 16 dwords of user data (b0, space 0), the title's
    // view address directory (t255, space 1) and a view (t3, space 1).
    let app = vec![
        RootParam {
            kind: RootParamKind::CONSTANTS,
            num_32bit_values: 16,
            ..RootParam::default()
        },
        root_param(RootParamKind::SRV, 255, 1),
        root_param(RootParamKind::SRV, 3, 1),
    ];
    let with = |extra: RootParam| {
        let mut p = app.clone();
        p.push(extra);
        graphics_root(&g.fx, p, heap)
    };
    let csig = with(root_param(
        RootParamKind::UAV,
        layout.register,
        layout.space,
    ));
    let gsig = with(root_param(
        RootParamKind::SRV,
        layout.register,
        layout.space,
    ));
    let t0 = std::time::Instant::now();
    g.compute_pso(csig, &cs)
        .unwrap_or_else(|e| panic!("compute pipeline: {e:?}"));
    let t_cs = t0.elapsed();
    let t1 = std::time::Instant::now();
    g.graphics_pso(gsig, Some(&vs), None, &ps)
        .unwrap_or_else(|e| panic!("vertex pipeline: {e:?}"));
    eprintln!(
        "real pair: compute pipeline {:?}, vertex+pixel pipeline {:?}",
        t_cs,
        t1.elapsed()
    );
}

/// A directory of compute forms of real mesh shaders (`*.metallib` of
/// `metal-shaderconverter`, entry `main`; kept outside the repository): every one
/// must become a compute pipeline in Apple's GPU compiler (which crashes on 1,400
/// of the title's mesh pipelines).
#[test]
fn real_compute_forms_build_in_apples_compiler() {
    let Some(dir) = std::env::var_os("VICTORY_MC_REAL_METALLIBS") else {
        eprintln!("SKIPPED real compute forms: set VICTORY_MC_REAL_METALLIBS");
        return;
    };
    let device = victory_metal_back::device::Device::new().expect("a Metal 4 device");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "metallib"))
        .collect();
    files.sort();
    let (mut built, mut failed) = (0, Vec::new());
    let t0 = std::time::Instant::now();
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        match device.compute_pipeline_from_metallib(&bytes, "main") {
            Ok(_) => built += 1,
            Err(e) => failed.push(format!("{}: {e}", f.display())),
        }
    }
    eprintln!(
        "compute forms: {built} of {} built in {:?}; failures: {failed:#?}",
        files.len(),
        t0.elapsed()
    );
    assert!(failed.is_empty(), "{failed:#?}");

    // The vertex shader of each one's layout (`*.layout` next to the metallib),
    // with a pixel shader that reads nothing: DXC takes it, Apple's compiler
    // builds it.
    let Some(g) = setup() else { return };
    let ps = dxil_or_skip(
        "real_ps_empty",
        "ps_6_6",
        "float4 main(float4 p : SV_Position) : SV_Target0 { return 1; }\n",
    )
    .unwrap();
    let (mut vs_built, mut vs_failed) = (0, Vec::new());
    for f in &files {
        let layout = Layout::parse(&std::fs::read_to_string(f.with_extension("layout")).unwrap())
            .expect("a layout");
        let sig = graphics_root(
            &g.fx,
            vec![root_param(
                RootParamKind::SRV,
                layout.register,
                layout.space,
            )],
            0,
        );
        let name = format!("real_vs_{}", f.file_stem().unwrap().to_string_lossy());
        let built = dxil_result(&name, "vs_6_6", &vs_hlsl(&layout))
            .expect("DXC configured")
            .map_err(|e| format!("DXC: {e}"))
            .and_then(|vs| {
                g.graphics_pso(sig, Some(&vs), None, &ps)
                    .map_err(|e| format!("{e:?}"))
            });
        match built {
            Ok(_) => vs_built += 1,
            Err(e) => vs_failed.push(format!("{}: {e}", f.display())),
        }
    }
    eprintln!(
        "vertex forms: {vs_built} of {} built; failures: {vs_failed:#?}",
        files.len()
    );
    // (DXC itself crashes on a vertex shader with more than 32 output rows; the
    // translator, which makes the DXIL itself in production, has no such limit.
    // Whatever DXC took must build.)
    assert!(
        vs_failed.iter().all(|f| f.contains(": DXC:")),
        "{vs_failed:#?}"
    );
}

/// What the emulation costs against Metal's own mesh path (printed, not asserted;
/// `VICTORY_MC_BENCH=<groups>`): the same triangles, the small mesh natively and
/// the big one as compute + vertex fetch. Wall time of a submission, median of 7.
#[test]
fn bench_emulated_against_native() {
    let Some(groups) = std::env::var("VICTORY_MC_BENCH")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
    else {
        eprintln!("SKIPPED bench: set VICTORY_MC_BENCH=<groups>");
        return;
    };
    let Some(g) = setup() else { return };
    let (Some(small), Some(big), Some(ps_s), Some(ps_b)) = (
        dxil_or_skip("bench_small", "ms_6_6", &mesh_hlsl(false)),
        dxil_or_skip("bench_big", "ms_6_6", &mesh_hlsl(true)),
        dxil_or_skip("bench_ps_s", "ps_6_6", PS_SMALL),
        dxil_or_skip("bench_ps_b", "ps_6_6", PS_BIG),
    ) else {
        return;
    };
    let Some(lowered) = lower("bench", &big) else {
        return;
    };
    let (cs, l) = lowered.expect("lowered");
    let vs = dxil_or_skip("bench_vs", "vs_6_6", &vs_hlsl(&l)).unwrap();
    let nsig = graphics_root(&g.fx, vec![], 0);
    let npso = g.graphics_pso(nsig, None, Some(&small), &ps_s).unwrap();
    let csig = graphics_root(
        &g.fx,
        vec![root_param(RootParamKind::UAV, l.register, l.space)],
        0,
    );
    let gsig = graphics_root(
        &g.fx,
        vec![root_param(RootParamKind::SRV, l.register, l.space)],
        0,
    );
    let cpso = g.compute_pso(csig, &cs).unwrap();
    let gpso = g.graphics_pso(gsig, Some(&vs), None, &ps_b).unwrap();
    let scratch =
        g.fx.buffer(BufferKind::DEFAULT, l.scratch_bytes(u64::from(groups)));
    let time = |f: &dyn Fn() -> Vec<u32>| {
        let mut t: Vec<f64> = (0..7)
            .map(|_| {
                let t0 = std::time::Instant::now();
                f();
                t0.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        t.sort_by(|a, b| a.total_cmp(b));
        t[3]
    };
    let native = time(&|| {
        g.picture(
            nsig,
            npso,
            vec![],
            None,
            Command::DispatchMesh(DispatchMesh {
                x: groups,
                y: 1,
                z: 1,
                _pad: 0,
            }),
        )
    });
    let emulated = time(&|| {
        g.picture(
            gsig,
            gpso,
            vec![
                set_sig(csig),
                Command::SetPipeline(SetPipeline { pso: cpso, _pad: 0 }),
                root_va(0, RootParamKind::UAV, scratch.gpu_va),
                dispatch(groups, 1, 1),
                Command::Barrier(Barrier::memory(stage::DISPATCH, stage::VERTEX)),
            ],
            Some(scratch.gpu_va),
            Command::Draw(Draw {
                vertex_count: l.draw_vertices(),
                instance_count: groups,
                start_vertex: 0,
                start_instance: 0,
            }),
        )
    });
    // Only the compute pass.
    let compute_only = time(&|| {
        g.picture(
            gsig,
            gpso,
            vec![
                set_sig(csig),
                Command::SetPipeline(SetPipeline { pso: cpso, _pad: 0 }),
                root_va(0, RootParamKind::UAV, scratch.gpu_va),
                dispatch(groups, 1, 1),
                Command::Barrier(Barrier::memory(stage::DISPATCH, stage::VERTEX)),
            ],
            Some(scratch.gpu_va),
            Command::Draw(Draw {
                vertex_count: 3,
                instance_count: 1,
                start_vertex: 0,
                start_instance: 0,
            }),
        )
    });
    eprintln!(
        "bench {groups} groups ({} B scratch, {} triangles each): native small mesh {native:.2} ms, \
         emulated {emulated:.2} ms (compute + 1 tiny draw {compute_only:.2} ms)",
        l.scratch_bytes(u64::from(groups)),
        TRIS
    );
}

// ---------------------------------------------------------------------------
// the back's runtime: mesh pipelines Metal cannot take, through the executor
// (docs/metal/mesh-compute.md, section 8)
// ---------------------------------------------------------------------------

/// The mesh shader as the translator hands it over: its container with the
/// lowering (compute form, fetch shader, layout) attached as the `VMCS` part.
fn with_vmcs(name: &str, ms: &[u8]) -> Option<(Vec<u8>, Layout)> {
    let (cs, layout, text) = match lower_text(name, ms)? {
        Ok(x) => x,
        Err(e) => panic!("the lowering refused the shader: {e}"),
    };
    let vs = dxil_or_skip(&format!("{name}_vs"), "vs_6_6", &vs_hlsl(&layout))?;
    let bundle = Bundle {
        layout: &text,
        compute: &cs,
        vertex: &vs,
    };
    Some((bundle.attach(ms).expect("attach the part"), layout))
}

fn mesh_signature() -> Vec<u8> {
    let mut v = Vec::new();
    IndirectArg {
        kind: IndirectArgKind::DISPATCH_MESH,
        index: 0,
        dest_offset: 0,
        count: 0,
    }
    .put(&mut v);
    v
}

fn execute_mesh(args: ResId, max_count: u32, count: Option<ResId>) -> Command<'static> {
    let sig: &'static [u8] = Box::leak(mesh_signature().into_boxed_slice());
    Command::ExecuteIndirect(ExecuteIndirect {
        args,
        args_offset: 0,
        count: count.unwrap_or(ResId::NULL),
        count_offset: 0,
        max_count,
        stride: 12,
        root_sig: RootSigId::NULL,
        signature: sig,
    })
}

fn dispatch_mesh(x: u32) -> Command<'static> {
    Command::DispatchMesh(DispatchMesh {
        x,
        y: 1,
        z: 1,
        _pad: 0,
    })
}

/// What the compute-form tests need: the native picture, the big mesh with
/// its part, the pixel shader.
struct Rig {
    g: Gfx,
    native: Vec<u32>,
    ms: Vec<u8>,
    ps: Vec<u8>,
    layout: Layout,
}

fn rig_with(fx: Fx, reads: bool) -> Option<Rig> {
    let g = setup_with(fx)?;
    let native = native_picture(&g)?;
    let big = dxil_or_skip("mcb_ms_big", "ms_6_6", &mesh_hlsl_with(true, reads, true))?;
    let ps = dxil_or_skip("mcb_ps_all", "ps_6_6", &ps_all())?;
    let (ms, layout) = with_vmcs("mcb", &big)?;
    Some(Rig {
        g,
        native,
        ms,
        ps,
        layout,
    })
}

fn rig() -> Option<Rig> {
    rig_with(Fx::new(), false)
}

fn same_picture(got: &[u32], want: &[u32], what: &str) {
    let mut lit = 0;
    for (i, (a, b)) in got.iter().zip(want).enumerate() {
        assert!(
            close(*a, *b, 1),
            "{what}: pixel ({}, {}): {a:#010x}, native {b:#010x}",
            i as u32 % W,
            i as u32 / W
        );
        lit += u32::from(*a != 0xFF00_0000);
    }
    assert!(
        lit > 150,
        "{what}: the picture is almost empty ({lit} pixels)"
    );
}

fn blank(got: &[u32], what: &str) {
    let n = got.iter().filter(|&&p| p != 0xFF00_0000).count();
    assert_eq!(n, 0, "{what}: {n} pixels drawn");
}

#[test]
fn a_mesh_pipeline_metal_refuses_is_made_and_draws_what_the_native_path_draws() {
    let Some(r) = rig() else { return };
    let sig = graphics_root(&r.g.fx, vec![], 0);
    let before = mesh_emu::counters();
    let pso =
        r.g.graphics_pso(sig, None, Some(&r.ms), &r.ps)
            .unwrap_or_else(|e| panic!("the emulation is not made: {e:?}"));
    assert_eq!(mesh_emu::counters().pipelines, before.pipelines + 1);
    let got = r.g.picture(sig, pso, vec![], None, dispatch_mesh(GROUPS));
    same_picture(&got, &r.native, "DispatchMesh");
    let after = mesh_emu::counters();
    assert_eq!(
        after.hoisted,
        before.hoisted + 1,
        "the kernel ran before the pass"
    );
    assert_eq!(
        (after.split, after.in_place),
        (before.split, before.in_place)
    );
    // again, on the same pipeline: the ring and the per-draw arguments are reused
    let again = r.g.picture(sig, pso, vec![], None, dispatch_mesh(GROUPS));
    same_picture(&again, &r.native, "DispatchMesh, second frame");
}

#[test]
fn an_indirect_mesh_dispatch_draws_what_the_native_path_draws() {
    let Some(r) = rig() else { return };
    let sig = graphics_root(&r.g.fx, vec![], 0);
    let pso = r.g.graphics_pso(sig, None, Some(&r.ms), &r.ps).unwrap();
    let args = r.g.fx.buffer(BufferKind::UPLOAD, 4096);
    let count = r.g.fx.buffer(BufferKind::UPLOAD, 4096);
    // {x, y, z}: the groups the GPU holds
    cpu_write(args.cpu_addr, &[GROUPS, 1, 1]);
    let before = mesh_emu::counters();
    for (n, max, drawn) in [(None, 1, true), (Some(1u32), 4, true), (Some(0), 4, false)] {
        if let Some(n) = n {
            cpu_write(count.cpu_addr, &[n]);
        }
        let got = r.g.picture(
            sig,
            pso,
            vec![],
            None,
            execute_mesh(args.res, max, n.map(|_| count.res)),
        );
        if drawn {
            same_picture(&got, &r.native, &format!("indirect, count {n:?}"));
        } else {
            blank(&got, "indirect, count 0");
        }
    }
    assert_eq!(mesh_emu::counters().indirect, before.indirect + 3);
}

#[test]
fn an_indirect_record_the_arena_cannot_hold_draws_nothing_and_the_next_one_draws() {
    let Some(r) = rig() else { return };
    let sig = graphics_root(&r.g.fx, vec![], 0);
    let pso = r.g.graphics_pso(sig, None, Some(&r.ms), &r.ps).unwrap();
    let args = r.g.fx.buffer(BufferKind::UPLOAD, 4096);
    // Over the arena by far, then a valid record, then a count whose product
    // overflows 32 bits.
    cpu_write(
        args.cpu_addr,
        &[1_000_000, 1, 1, GROUPS, 1, 1, 70_000, 70_000, 1],
    );
    let got =
        r.g.picture(sig, pso, vec![], None, execute_mesh(args.res, 3, None));
    // The valid record is the picture the native path draws.
    same_picture(&got, &r.native, "the record after a refused one");
}

#[test]
fn with_the_switch_off_the_big_mesh_is_refused_as_before() {
    let mut options = small_options();
    options.mesh_compute = false;
    let Some(r) = rig_with(Fx::with_options(options), false) else {
        return;
    };
    let sig = graphics_root(&r.g.fx, vec![], 0);
    let (s, why) =
        r.g.graphics_pso(sig, None, Some(&r.ms), &r.ps)
            .expect_err("refused");
    assert_ne!(s, Status::OK);
    assert!(!why.is_empty());
}

#[test]
fn the_switch_reads_its_values() {
    assert!(mesh_emu::enabled_from(None));
    assert!(mesh_emu::enabled_from(Some("1")));
    assert!(!mesh_emu::enabled_from(Some("0")));
}

/// A full-screen triangle whose pixel shader stores 1 through the root UAV
/// at `u1`, and writes black over the black the pass cleared to.
const VS_FULL: &str = "float4 main(uint v : SV_VertexID) : SV_Position {\n\
  float2 p = float2((v << 1) & 2, v & 2);\n\
  return float4(p * 2.0 - 1.0, 0.5, 1); }\n";
const PS_STORE: &str = "RWByteAddressBuffer bits : register(u1);\n\
float4 main(float4 p : SV_Position) : SV_Target0 { bits.Store(0, 1); return float4(0, 0, 0, 1); }\n";

/// The pass: a draw that stores 1 into `written`, then the emulated mesh
/// draw reading `read` through its root SRV. Gives the picture; the mesh
/// shader tints by the word it reads, so the picture is the native one only if
/// the kernel saw the 1 (`read` was prefilled with 0).
fn write_then_read(
    r: &Rig,
    same_buffer: bool,
) -> (Vec<u32>, mesh_emu::Counters, mesh_emu::Counters) {
    let g = &r.g;
    let wsig = graphics_root(&g.fx, vec![root_param(RootParamKind::UAV, 1, 0)], 0);
    let wvs = dxil_or_skip("mcb_vs_full", "vs_6_6", VS_FULL).unwrap();
    let wps = dxil_or_skip("mcb_ps_store", "ps_6_6", PS_STORE).unwrap();
    let wpso = g.graphics_pso(wsig, Some(&wvs), None, &wps).unwrap();
    let msig = graphics_root(&g.fx, vec![root_param(RootParamKind::SRV, 0, 0)], 0);
    let mpso = g.graphics_pso(msig, None, Some(&r.ms), &r.ps).unwrap();
    let a = g.fx.buffer(BufferKind::DEFAULT, 256);
    let b = if same_buffer {
        a
    } else {
        g.fx.buffer(BufferKind::DEFAULT, 256)
    };
    let up = g.fx.buffer(BufferKind::UPLOAD, 256);
    cpu_write(up.cpu_addr, &[0, 0, 0, 0]);
    let before = vec![
        copy(a.res, up.res, 256),
        // the other buffer already holds the 1 the mesh reads
        Command::Barrier(Barrier::memory(
            stage::BLIT,
            stage::BLIT | stage::VERTEX | stage::FRAGMENT | stage::DISPATCH,
        )),
    ];
    let one = g.fx.buffer(BufferKind::UPLOAD, 256);
    cpu_write(one.cpu_addr, &[1, 0, 0, 0]);
    let mut before = before;
    if !same_buffer {
        before.insert(1, copy(b.res, one.res, 256));
    }
    let in_pass = vec![
        Command::SetRootSignature(SetRootSignature {
            bind_point: BindPoint::GRAPHICS,
            root_sig: wsig,
        }),
        Command::SetPipeline(SetPipeline { pso: wpso, _pad: 0 }),
        graphics_va(0, RootParamKind::UAV, a.gpu_va),
        Command::Draw(Draw {
            vertex_count: 3,
            instance_count: 1,
            start_vertex: 0,
            start_instance: 0,
        }),
        Command::SetRootSignature(SetRootSignature {
            bind_point: BindPoint::GRAPHICS,
            root_sig: msig,
        }),
        Command::SetPipeline(SetPipeline { pso: mpso, _pad: 0 }),
        graphics_va(0, RootParamKind::SRV, b.gpu_va),
        dispatch_mesh(GROUPS),
    ];
    let c0 = mesh_emu::counters();
    let got = g.picture_in(before, vec![], in_pass);
    (got, c0, mesh_emu::counters())
}

#[test]
fn a_draw_that_reads_what_the_pass_wrote_splits_the_pass() {
    let Some(r) = rig_with(Fx::new(), true) else {
        return;
    };
    if dxil_or_skip("mcb_probe", "vs_6_6", VS_FULL).is_none() {
        return;
    }
    let (got, c0, c1) = write_then_read(&r, true);
    same_picture(&got, &r.native, "split pass");
    // The picture is the native one only if the kernel read the 1 the pass
    // stored: a kernel hoisted before the pass reads the 0 and tints
    // everything black (checked by making the rule say "no conflict").
    // The rule says it split the pass before the draw, the kernel between
    // the two render encoders:
    assert_eq!(c1.split, c0.split + 1, "the rule split the pass");
    assert_eq!(c1.hoisted, c0.hoisted, "nothing was hoisted past the write");
}

#[test]
fn a_draw_that_reads_something_else_is_hoisted() {
    let Some(r) = rig_with(Fx::new(), true) else {
        return;
    };
    if dxil_or_skip("mcb_probe", "vs_6_6", VS_FULL).is_none() {
        return;
    }
    let (got, c0, c1) = write_then_read(&r, false);
    assert_eq!(c1.split, c0.split, "no conflict, no split");
    assert_eq!(c1.hoisted, c0.hoisted + 1);
    same_picture(&got, &r.native, "hoisted");
}

// ---- the scratch ring ---------------------------------------------------------

#[test]
fn the_scratch_ring_does_not_hand_out_a_segment_a_submission_in_flight_may_use() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use victory_metal_back::device::Device;
    use victory_metal_back::queue::{Queue, QueueOptions};
    let fx = Fx::new(); // the GPU lock
    let _ = &fx;
    let d = Device::new().expect("Metal 4 device");
    let q = Queue::with_options(
        &d,
        QueueOptions {
            in_flight: 2,
            max_lists: 4,
            ring_segment_bytes: 4096,
            ring_segments: 2,
            inter_submit_barrier: true,
        },
    )
    .unwrap();
    assert!(q.configure_scratch(4096, 2));
    let a = q.alloc_scratch(4096, 256).unwrap();
    let b = q.alloc_scratch(4096, 256).unwrap();
    assert_ne!(a.gpu_va, b.gpu_va);
    // Both segments belong to the submission being recorded: a third is "more
    // than the whole ring" and an error, not a wait for ever.
    assert!(q.alloc_scratch(4096, 256).is_err());
    assert!(q.alloc_scratch(8192, 256).is_err(), "over a segment");
    assert!(
        !q.configure_scratch(8192, 4),
        "a ring in use keeps its size"
    );
    // The submission that holds them stays in flight until the CPU signals.
    let fence = d.create_fence().unwrap();
    q.wait_fence(&fence, 1);
    let mut l = q.begin_list().unwrap();
    l.end().unwrap();
    q.submit(vec![l]).unwrap();
    let signalled = Arc::new(AtomicBool::new(false));
    let flag = signalled.clone();
    let f2 = fence.clone();
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        flag.store(true, Ordering::SeqCst);
        f2.signal_cpu(1);
    });
    let c = q.alloc_scratch(4096, 256).unwrap();
    assert!(
        signalled.load(Ordering::SeqCst),
        "the ring gave a segment back while the submission using it was still in flight"
    );
    assert!(c.gpu_va == a.gpu_va || c.gpu_va == b.gpu_va);
    t.join().unwrap();
}

#[test]
fn frames_in_flight_do_not_overwrite_each_others_scratch() {
    let Some(r) = rig_with(Fx::new(), true) else {
        return;
    };
    let g = &r.g;
    // A ring of two segments, each just a frame's scratch: the third frame
    // has to wait for the first to complete, and so on.
    let queue = g.fx.executor().queue(g.q).expect("the queue");
    let segment = r
        .layout
        .scratch_bytes(u64::from(GROUPS))
        .next_multiple_of(256);
    assert!(queue.configure_scratch(segment, 2));
    let sig = graphics_root(&g.fx, vec![root_param(RootParamKind::SRV, 0, 0)], 0);
    let pso = g.graphics_pso(sig, None, Some(&r.ms), &r.ps).unwrap();
    // The mesh tints by the word it reads: every frame has its own picture.
    let frames = 8u32;
    let src: Vec<_> = (0..frames)
        .map(|k| {
            let b = g.fx.buffer(BufferKind::UPLOAD, 256);
            cpu_write(b.cpu_addr, &[1 + k % 3, 0, 0, 0]);
            b
        })
        .collect();
    let frame = |k: u32, tex: ResId, rb: &CreateBufferArgs| {
        let mut cmds = vec![
            begin(1, g.q),
            Command::SetRootSignature(SetRootSignature {
                bind_point: BindPoint::GRAPHICS,
                root_sig: sig,
            }),
            Command::SetPipeline(SetPipeline { pso, _pad: 0 }),
            graphics_va(0, RootParamKind::SRV, src[k as usize].gpu_va),
        ];
        cmds.extend(g.pass_around(tex, vec![dispatch_mesh(GROUPS)]));
        cmds.extend(g.read_back(tex, rb));
        cmds.push(end(1));
        cmds
    };
    // each frame alone first: the pictures to expect
    let mut want = Vec::new();
    for k in 0..frames {
        let (tex, rb) = g.target();
        assert_eq!(g.fx.submit(&frame(k, tex, &rb)), Status::OK);
        g.settle();
        want.push(cpu_read(rb.cpu_addr, (W * H) as usize));
    }
    assert_ne!(want[0], want[1], "the frames differ (tint 1 against 2)");
    // all of them in flight at once
    let targets: Vec<_> = (0..frames).map(|_| g.target()).collect();
    for k in 0..frames {
        let (tex, rb) = &targets[k as usize];
        assert_eq!(
            g.fx.submit(&frame(k, *tex, rb)),
            Status::OK,
            "{:?}",
            g.fx.executor().last_error()
        );
    }
    g.settle();
    for k in 0..frames as usize {
        let got = cpu_read(targets[k].1.cpu_addr, (W * H) as usize);
        assert_eq!(
            got, want[k],
            "frame {k} was drawn from scratch another frame wrote"
        );
    }
}
