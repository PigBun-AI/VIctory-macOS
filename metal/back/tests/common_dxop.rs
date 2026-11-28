//! Rig of the dx.op node tests (`dxop_*.rs`; map-plan/inventory.md, "DXIL ->
//! MSC"): HLSL through DXC to DXIL, MSC, a Metal 4 compute dispatch, the
//! result read back for a CPU reference. Every program is checked to declare
//! the `dx.op.*` functions it is meant to prove, so a test cannot pass by DXC
//! having folded the operation away.
//!
//! A program sees `K` (eight root constants, b0), `I` (a root SRV byte
//! buffer, t0) and `O` (a root UAV byte buffer, u0): see [`PRELUDE`].
//! Without a DXC (`VICTORY_HELPER_DXC`, `DXC_LIB`; common_msc.rs) the tests
//! print `SKIPPED` and pass.
#![allow(dead_code)]
#![cfg(target_vendor = "apple")]

use std::collections::BTreeSet;
use std::sync::{Mutex, MutexGuard, OnceLock};

use victory_metal_back::bitcode::{Bitcode, Item};
use victory_metal_back::descriptors::DescriptorHeap;
use victory_metal_back::device::Device;
use victory_metal_back::memory::BufferKind;
use victory_metal_back::msc::{
    ArgKind, CompileOptions, Param, ParamKind, Reflection, RootSignatureDesc, Visibility, flags,
};
use victory_metal_back::draw::{DrawArgs, PassSpec, Primitive, Stores, ViewportRect};
use victory_metal_back::pipeline::ComputePipeline;
use victory_metal_back::pipeline_defaults::{Stages, Variant, default_graphics};
use victory_metal_back::render::GraphicsPipeline;
use victory_metal_back::queue::Queue;

use crate::common_msc::dxil_or_skip;

use victory_metal_back::copy::{BufLoc, TexLoc};
use victory_metal_back::descriptors::IrDescriptorTableEntry;
use victory_metal_back::format::dxgi;
use victory_metal_back::sampler::{Address, Filter, Sampler, SamplerSpec};
use victory_metal_back::texture::{Texture, TextureDesc};
use victory_metal_back::memory::Buffer;


/// Declarations every compute test program starts with.
pub const PRELUDE: &str = "cbuffer K : register(b0) { uint k0; uint k1; uint k2; uint k3; uint k4; uint k5; uint k6; uint k7; }\n\
ByteAddressBuffer I : register(t0);\n\
RWByteAddressBuffer O : register(u0);\n";

pub struct Rig {
    _g: MutexGuard<'static, ()>,
    pub d: Device,
    pub q: Queue,
}

pub fn rig() -> Rig {
    static LOCK: Mutex<()> = Mutex::new(());
    static D: OnceLock<Device> = OnceLock::new();
    let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let d = D
        .get_or_init(|| Device::new().expect("Metal 4 device"))
        .clone();
    let q = Queue::new(&d).expect("queue");
    Rig { _g: g, d, q }
}

/// The `dx.op.*` functions a DXIL container declares (the names in the
/// module's value symbol table).
pub fn declared_dxops(dxil: &[u8]) -> BTreeSet<String> {
    let part = victory_metal_back::msc::dxil::part(dxil, b"DXIL").expect("a DXIL part");
    assert_eq!(&part[8..12], b"DXIL");
    let off = u32::from_le_bytes(part[16..20].try_into().unwrap()) as usize;
    let size = u32::from_le_bytes(part[20..24].try_into().unwrap()) as usize;
    let bc = Bitcode::parse(&part[8 + off..8 + off + size]).expect("bitcode");
    let mut out = BTreeSet::new();
    for module in bc.blocks.iter().filter(|b| b.id == 8) {
        for item in &module.items {
            let Item::Block(vst) = item else { continue };
            if vst.id != 14 {
                continue;
            }
            for r in vst.items.iter().filter_map(|i| match i {
                Item::Record(r) => Some(r),
                _ => None,
            }) {
                let skip = match r.code {
                    1 => 1,
                    3 => 2,
                    _ => continue,
                };
                let name: String = r.ops[skip..].iter().map(|&c| c as u8 as char).collect();
                if name.starts_with("dx.op.") {
                    out.insert(name);
                }
            }
        }
    }
    out
}

/// Panics unless the DXIL declares every one of `ops` (`dx.op.<name>.<overload>`).
pub fn assert_declares(name: &str, dxil: &[u8], ops: &[&str]) {
    let have = declared_dxops(dxil);
    for op in ops {
        assert!(
            have.contains(*op),
            "{name}: the DXIL does not declare {op}; it declares {have:?}"
        );
    }
}

/// DXIL of `hlsl` for `profile`, with the declared-op check, or `None`
/// (SKIPPED, no DXC).
pub fn dxil_checked(name: &str, profile: &str, hlsl: &str, ops: &[&str]) -> Option<Vec<u8>> {
    let dxil = dxil_or_skip(name, profile, hlsl)?;
    assert_declares(name, &dxil, ops);
    Some(dxil)
}

pub struct Prog {
    pub pipeline: ComputePipeline,
    pub reflection: Reflection,
    consts_at: u64,
    srv_at: u64,
    uav_at: u64,
}

impl Rig {
    /// A compute program: `body` after [`PRELUDE`], `ops` the `dx.op`
    /// functions it must declare.
    pub fn program(&self, name: &str, body: &str, ops: &[&str]) -> Option<Prog> {
        self.program_in(name, "cs_6_6", &format!("{PRELUDE}{body}"), ops)
    }

    /// As [`Rig::program`] with a profile and the whole source.
    pub fn program_in(&self, name: &str, profile: &str, hlsl: &str, ops: &[&str]) -> Option<Prog> {
        let dxil = dxil_checked(name, profile, hlsl, ops)?;
        Some(self.prog_from(&dxil))
    }

    /// As [`Rig::program`] for a shader that reaches textures and samplers
    /// through `ResourceDescriptorHeap[]` / `SamplerDescriptorHeap[]`
    /// (run it with [`Rig::run_heaps`]).
    pub fn program_heaps(&self, name: &str, body: &str, ops: &[&str]) -> Option<Prog> {
        let dxil = dxil_checked(name, "cs_6_6", &format!("{PRELUDE}{body}"), ops)?;
        Some(self.prog_with(&dxil, flags::HEAP_INDEXED | flags::SAMPLER_HEAP_INDEXED))
    }

    pub fn prog_from(&self, dxil: &[u8]) -> Prog {
        self.prog_with(dxil, 0)
    }

    /// Whether MSC and Metal take the program: the refusal's text if not.
    pub fn try_build(&self, dxil: &[u8], root_flags: u32) -> Result<(), String> {
        let root = standard_root(root_flags);
        self.d
            .compute_pipeline_from_dxil(dxil, Some(&root), &CompileOptions::default())
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn prog_with(&self, dxil: &[u8], root_flags: u32) -> Prog {
        let root = RootSignatureDesc {
            flags: root_flags,
            params: vec![
                Param {
                    kind: ParamKind::Constants {
                        register: 0,
                        space: 0,
                        num_32bit: 8,
                    },
                    visibility: Visibility::All,
                },
                Param {
                    kind: ParamKind::Srv {
                        register: 0,
                        space: 0,
                    },
                    visibility: Visibility::All,
                },
                Param {
                    kind: ParamKind::Uav {
                        register: 0,
                        space: 0,
                    },
                    visibility: Visibility::All,
                },
            ],
            static_samplers: vec![],
        };
        let sp = self
            .d
            .compute_pipeline_from_dxil(dxil, Some(&root), &CompileOptions::default())
            .expect("MSC and Metal accept the program");
        let at = |kind: ArgKind| {
            u64::from(
                sp.reflection
                    .layout
                    .entries
                    .iter()
                    .find(|e| e.kind == kind)
                    .map(|e| e.offset)
                    .unwrap_or_else(|| panic!("no {kind:?} entry in the layout")),
            )
        };
        Prog {
            consts_at: at(ArgKind::Constants),
            srv_at: at(ArgKind::Srv),
            uav_at: at(ArgKind::Uav),
            pipeline: sp.pipeline,
            reflection: sp.reflection,
        }
    }

    /// Runs `p` over `groups` of `threads`: `k` are the root constants,
    /// `input` the words of `I`, and `out_words` words of `O` (filled with
    /// 0xDEADBEEF first) come back.
    pub fn run(
        &self,
        p: &Prog,
        groups: [u32; 3],
        threads: [u32; 3],
        k: &[u32],
        input: &[u32],
        out_words: usize,
    ) -> Vec<u32> {
        self.run_heaps(p, groups, threads, k, input, out_words, None)
    }

    /// As [`Rig::run`] with the resource and sampler descriptor heaps bound.
    #[allow(clippy::too_many_arguments)]
    pub fn run_heaps(
        &self,
        p: &Prog,
        groups: [u32; 3],
        threads: [u32; 3],
        k: &[u32],
        input: &[u32],
        out_words: usize,
        heaps: Option<(&DescriptorHeap, &DescriptorHeap)>,
    ) -> Vec<u32> {
        let mut kk = [0u32; 8];
        kk[..k.len()].copy_from_slice(k);
        let inb = self
            .d
            .create_buffer(BufferKind::Shared, 4 * input.len().max(4) as u64)
            .unwrap();
        inb.write_u32s(0, input).unwrap();
        let out = self
            .d
            .create_buffer(BufferKind::Shared, 4 * out_words.max(1) as u64)
            .unwrap();
        out.write_u32s(0, &vec![0xDEAD_BEEF; out_words]).unwrap();
        let tlab = self
            .q
            .alloc_constants(u64::from(p.reflection.layout.size), 8)
            .unwrap();
        tlab.zero();
        tlab.write_u32s(p.consts_at, &kk).unwrap();
        tlab.write_u64(p.srv_at, inb.gpu_va()).unwrap();
        tlab.write_u64(p.uav_at, out.gpu_va()).unwrap();
        let mut l = self.q.begin_list().unwrap();
        if let Some((h, sh)) = heaps {
            l.set_heaps(h, Some(sh)).unwrap();
        }
        l.set_root_arguments(&tlab).unwrap();
        l.dispatch(&p.pipeline, groups, threads).unwrap();
        l.end().unwrap();
        let v = self.q.submit(vec![l]).expect("submit");
        self.q.wait_for(v, 20_000).expect("completes");
        out.read_u32s(0, out_words).unwrap()
    }
}

/// Words that differ: `(index, got, want)`, the first `limit`.
pub fn diffs(got: &[u32], want: &[u32], limit: usize) -> Vec<(usize, u32, u32)> {
    assert_eq!(got.len(), want.len(), "lengths");
    got.iter()
        .zip(want)
        .enumerate()
        .filter(|(_, (g, w))| g != w)
        .map(|(i, (g, w))| (i, *g, *w))
        .take(limit)
        .collect()
}

/// `assert!` that `got == want`, showing the first differences in hex.
#[track_caller]
pub fn assert_words(what: &str, got: &[u32], want: &[u32]) {
    let d = diffs(got, want, 8);
    assert!(
        d.is_empty(),
        "{what}: {} of {} words differ; first (index, got, want): {}",
        diffs(got, want, usize::MAX).len(),
        got.len(),
        d.iter()
            .map(|(i, g, w)| format!("({i}, {g:#010x}, {w:#010x})"))
            .collect::<Vec<_>>()
            .join(" ")
    );
}

/// A full-screen triangle from `SV_VertexID`, no vertex buffer.
pub const FULLSCREEN_VS: &str = "float4 main(uint id : SV_VertexID) : SV_Position {\n\
  float2 p[3] = { float2(-1, -1), float2(3, -1), float2(-1, 3) };\n\
  return float4(p[id], 0.5, 1); }\n";

/// One triangle over the lower-left half of an 8x8 target, its hypotenuse
/// (NDC x + y = 0.1) crossing the pixel centres of no pixel: pixel (i, j)
/// (column, row from the top) is covered when i <= j. Every quad on the
/// diagonal has one uncovered pixel, the upper right (i = j + 1, j even).
pub const HALF_VS: &str = "float4 main(uint id : SV_VertexID) : SV_Position {\n\
  float2 p[3] = { float2(-1, -1), float2(1.1, -1), float2(-1, 1.1) };\n\
  return float4(p[id], 0.5, 1); }\n";

/// A vertex + pixel pipeline with no render target (the pixel shader writes
/// `O`), and where its root arguments are.
pub struct PixProg {
    pub pipeline: GraphicsPipeline,
    /// A mesh pipeline: `run_pixels` calls its `size` the group count x.
    pub mesh: bool,
    pub layout_size: u32,
    pub consts_at: u64,
    pub srv_at: u64,
    pub uav_at: u64,
}

impl Rig {
    /// `ps` is the whole pixel shader after [`PRELUDE`]; `ops` are the
    /// `dx.op` it must declare. `heaps`: the shader indexes the descriptor
    /// heaps.
    pub fn pixel_program(&self, name: &str, ps: &str, ops: &[&str], heaps: bool) -> Option<PixProg> {
        self.pixel_program_vs(name, FULLSCREEN_VS, ps, ops, heaps)
    }

    /// As [`Rig::pixel_program`] with a vertex shader of its own (it draws
    /// three vertices).
    pub fn pixel_program_vs(&self, name: &str, vs: &str, ps: &str, ops: &[&str], heaps: bool) -> Option<PixProg> {
        let vs = dxil_or_skip(&format!("{name}_vs"), "vs_6_6", vs)?;
        let psd = dxil_checked(name, "ps_6_6", &format!("{PRELUDE}{ps}"), ops)?;
        let root = RootSignatureDesc {
            flags: if heaps { flags::HEAP_INDEXED | flags::SAMPLER_HEAP_INDEXED } else { 0 },
            params: vec![
                Param { kind: ParamKind::Constants { register: 0, space: 0, num_32bit: 8 }, visibility: Visibility::All },
                Param { kind: ParamKind::Srv { register: 0, space: 0 }, visibility: Visibility::All },
                Param { kind: ParamKind::Uav { register: 0, space: 0 }, visibility: Visibility::All },
            ],
            static_samplers: vec![],
        };
        let desc = default_graphics(
            &root,
            Stages { vs: Some(&vs), ps: Some(&psd), ms: None, gs: None },
            &[],
            &[],
            false,
            Variant::Plain,
        );
        let pipeline = self
            .d
            .graphics_pipeline_from_dxil(&desc)
            .unwrap_or_else(|e| panic!("{name}: MSC and Metal accept the pipeline: {e}"));
        let layout = pipeline.layout().clone();
        let at = |kind: ArgKind| {
            u64::from(
                layout
                    .entries
                    .iter()
                    .find(|e| e.kind == kind)
                    .map(|e| e.offset)
                    .unwrap_or_else(|| panic!("no {kind:?} entry in the layout")),
            )
        };
        Some(PixProg {
            consts_at: at(ArgKind::Constants),
            srv_at: at(ArgKind::Srv),
            uav_at: at(ArgKind::Uav),
            layout_size: layout.size,
            mesh: false,
            pipeline,
        })
    }

    /// A mesh + pixel pipeline (no target); `run_mesh` runs it.
    pub fn mesh_program(&self, name: &str, ms: &str, ps: &str, ms_ops: &[&str], ps_ops: &[&str]) -> Option<PixProg> {
        let msd = dxil_checked(&format!("{name}_ms"), "ms_6_6", &format!("{PRELUDE}{ms}"), ms_ops)?;
        let psd = dxil_checked(&format!("{name}_ps"), "ps_6_6", &format!("{PRELUDE}{ps}"), ps_ops)?;
        let root = RootSignatureDesc {
            flags: 0,
            params: vec![
                Param { kind: ParamKind::Constants { register: 0, space: 0, num_32bit: 8 }, visibility: Visibility::All },
                Param { kind: ParamKind::Srv { register: 0, space: 0 }, visibility: Visibility::All },
                Param { kind: ParamKind::Uav { register: 0, space: 0 }, visibility: Visibility::All },
            ],
            static_samplers: vec![],
        };
        let desc = default_graphics(
            &root,
            Stages { vs: None, ps: Some(&psd), ms: Some(&msd), gs: None },
            &[],
            &[],
            false,
            Variant::Plain,
        );
        let pipeline = self
            .d
            .graphics_pipeline_from_dxil(&desc)
            .unwrap_or_else(|e| panic!("{name}: MSC and Metal accept the mesh pipeline: {e}"));
        let layout = pipeline.layout().clone();
        let at = |kind: ArgKind| {
            u64::from(
                layout.entries.iter().find(|e| e.kind == kind).map(|e| e.offset)
                    .unwrap_or_else(|| panic!("no {kind:?} entry in the layout")),
            )
        };
        Some(PixProg {
            consts_at: at(ArgKind::Constants),
            srv_at: at(ArgKind::Srv),
            uav_at: at(ArgKind::Uav),
            layout_size: layout.size,
            mesh: true,
            pipeline,
        })
    }

    /// Dispatches `groups` mesh groups into a `size` pass; `O` comes back.
    pub fn run_mesh(&self, p: &PixProg, groups: u32, size: [u32; 2], k: &[u32], input: &[u32], out_words: usize) -> Vec<u32> {
        self.run_pixels_with(p, size, groups, k, input, out_words, None)
    }

    /// Draws the full-screen triangle over a `size` pass with no
    /// attachment; `O` (`out_words` words, first 0xDEADBEEF) comes back.
    #[allow(clippy::too_many_arguments)]
    pub fn run_pixels(
        &self,
        p: &PixProg,
        size: [u32; 2],
        k: &[u32],
        input: &[u32],
        out_words: usize,
        heaps: Option<(&DescriptorHeap, &DescriptorHeap)>,
    ) -> Vec<u32> {
        self.run_pixels_with(p, size, 0, k, input, out_words, heaps)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_pixels_with(
        &self,
        p: &PixProg,
        size: [u32; 2],
        groups: u32,
        k: &[u32],
        input: &[u32],
        out_words: usize,
        heaps: Option<(&DescriptorHeap, &DescriptorHeap)>,
    ) -> Vec<u32> {
        let mut kk = [0u32; 8];
        kk[..k.len()].copy_from_slice(k);
        let inb = self.d.create_buffer(BufferKind::Shared, 4 * input.len().max(4) as u64).unwrap();
        inb.write_u32s(0, input).unwrap();
        let out = self.d.create_buffer(BufferKind::Shared, 4 * out_words.max(1) as u64).unwrap();
        out.write_u32s(0, &vec![0xDEAD_BEEF; out_words]).unwrap();
        let tlab = self.q.alloc_constants(u64::from(p.layout_size), 8).unwrap();
        tlab.zero();
        tlab.write_u32s(p.consts_at, &kk).unwrap();
        tlab.write_u64(p.srv_at, inb.gpu_va()).unwrap();
        tlab.write_u64(p.uav_at, out.gpu_va()).unwrap();
        let mut l = self.q.begin_list().unwrap();
        if let Some((h, sh)) = heaps {
            l.set_heaps(h, Some(sh)).unwrap();
        }
        l.set_root_arguments(&tlab).unwrap();
        l.begin_pass(PassSpec { colors: vec![], depth: None, empty_size: Some(size) }).unwrap();
        l.set_viewports(&[ViewportRect {
            x: 0.0,
            y: 0.0,
            width: size[0] as f32,
            height: size[1] as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        }])
        .unwrap();
        l.set_scissors(&[[0, 0, size[0] as i32, size[1] as i32]]).unwrap();
        if !p.mesh {
            l.set_primitive(Primitive::TriangleList).unwrap();
        }
        if p.mesh {
            l.dispatch_mesh(&p.pipeline, [groups, 1, 1]).unwrap();
        } else {
            l.draw(&p.pipeline, DrawArgs { vertex_count: 3, instance_count: 1, start_vertex: 0, start_instance: 0 }).unwrap();
        }
        l.end_pass(&Stores::default()).unwrap();
        l.end().unwrap();
        let v = self.q.submit(vec![l]).expect("submit");
        self.q.wait_for(v, 20_000).expect("completes");
        out.read_u32s(0, out_words).unwrap()
    }
}

// ---- textures and sampler heaps (dxop_tex.rs, dxop_pixel.rs) ----

pub const SRV: u32 = 1;
pub const DS: u32 = 8;

pub fn upload(d: &Device, q: &Queue, t: &Texture, mip: u32, data: &[u8], pitch: u32) {
    let src: Buffer = d
        .create_buffer(BufferKind::Upload, data.len() as u64)
        .unwrap();
    src.write(0, data).unwrap();
    let size = t.desc().mip_size(mip);
    let mut l = q.begin_list().unwrap();
    l.copy_buffer_to_texture(
        &TexLoc {
            texture: t,
            mip,
            slice: 0,
            plane: 0,
            origin: [0; 3],
        },
        &BufLoc {
            buffer: &src,
            offset: 0,
            format: t.desc().format,
            row_pitch: pitch,
            rows: size[1],
        },
        size,
    )
    .unwrap();
    l.end().unwrap();
    let v = q.submit(vec![l]).unwrap();
    q.wait_for(v, 20_000).unwrap();
}

pub fn f32s(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// An 8x8 RGBA32F texture of four mip levels, level m filled with `1 + m`
/// in every channel.
pub fn lod_texture(d: &Device, q: &Queue) -> Texture {
    let mut desc = TextureDesc::tex2d(dxgi::R32G32B32A32_FLOAT, 8, 8, SRV);
    desc.mips = 4;
    let t = d.create_texture(&desc).unwrap();
    for m in 0..4u32 {
        let n = (8 >> m) as usize;
        upload(d, q, &t, m, &f32s(&vec![1.0 + m as f32; n * n * 4]), (n * 16) as u32);
    }
    t
}

/// The two heaps; the samplers are returned too: a descriptor holds only a
/// sampler's id, the state object has to stay alive while it is used.
pub fn heaps(
    d: &Device,
    tex_views: &[u64],
    samplers: &[SamplerSpec],
) -> (DescriptorHeap, DescriptorHeap, Vec<Sampler>) {
    let h = DescriptorHeap::new(d, 16).unwrap();
    for (i, &v) in tex_views.iter().enumerate() {
        h.write(i as u32, IrDescriptorTableEntry::texture(v, 0.0, 0))
            .unwrap();
    }
    let sh = DescriptorHeap::new(d, 16).unwrap();
    let mut keep = Vec::new();
    for (i, s) in samplers.iter().enumerate() {
        let smp = d.create_sampler(s).unwrap();
        sh.write(i as u32, smp.entry()).unwrap();
        keep.push(smp);
    }
    (h, sh, keep)
}

pub fn trilinear() -> SamplerSpec {
    SamplerSpec {
        min: Filter::Linear,
        mag: Filter::Linear,
        mip: Some(Filter::Linear),
        address: [Address::ClampToEdge; 3],
        ..Default::default()
    }
}

/// A 4x4 RGBA32F texture, texel (x, y) channel c = 1000 c + 10 y + x.
pub fn gather_texture(d: &Device, q: &Queue) -> Texture {
    let t = d
        .create_texture(&TextureDesc::tex2d(dxgi::R32G32B32A32_FLOAT, 4, 4, SRV))
        .unwrap();
    let mut v = Vec::new();
    for y in 0..4 {
        for x in 0..4 {
            for c in 0..4 {
                v.push((1000 * c + 10 * y + x) as f32);
            }
        }
    }
    upload(d, q, &t, 0, &f32s(&v), 64);
    t
}

pub fn texel(x: i32, y: i32, c: usize) -> f32 {
    let (x, y) = (x.clamp(0, 3), y.clamp(0, 3));
    (1000 * c as i32 + 10 * y + x) as f32
}


pub fn close(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

/// Reads texture mip 0, slice 0 back: rows of `pitch` bytes.
pub fn download(d: &Device, q: &Queue, t: &Texture, pitch: u32) -> Vec<u8> {
    let size = t.desc().mip_size(0);
    let f = victory_metal_back::format::lookup(t.desc().format).unwrap();
    let len = u64::from(pitch) * u64::from(f.block_rows(size[1])) * u64::from(size[2]);
    let dst: Buffer = d.create_buffer(BufferKind::Readback, len).unwrap();
    let mut l = q.begin_list().unwrap();
    l.copy_texture_to_buffer(
        &BufLoc { buffer: &dst, offset: 0, format: t.desc().format, row_pitch: pitch, rows: size[1] },
        &TexLoc { texture: t, mip: 0, slice: 0, plane: 0, origin: [0; 3] },
        size,
    )
    .unwrap();
    l.end().unwrap();
    let v = q.submit(vec![l]).unwrap();
    q.wait_for(v, 20_000).unwrap();
    let mut out = vec![0u8; len as usize];
    dst.read(0, &mut out).unwrap();
    out
}

pub const UAV: u32 = 2;

/// The root signature of every program here: eight constants at b0, a root
/// SRV at t0 and a root UAV at u0.
pub fn standard_root(root_flags: u32) -> RootSignatureDesc {
    RootSignatureDesc {
        flags: root_flags,
        params: vec![
            Param { kind: ParamKind::Constants { register: 0, space: 0, num_32bit: 8 }, visibility: Visibility::All },
            Param { kind: ParamKind::Srv { register: 0, space: 0 }, visibility: Visibility::All },
            Param { kind: ParamKind::Uav { register: 0, space: 0 }, visibility: Visibility::All },
        ],
        static_samplers: vec![],
    }
}
