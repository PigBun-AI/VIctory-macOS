//! Runs a request on Metal through `victory-metal-back`'s plain-Rust API:
//! DXIL -> MSC (with the root signature the translator's host declares) ->
//! Metal 4 compute pipeline -> descriptor heap, top-level argument buffer ->
//! dispatch -> read back. The same objects the game's executor makes
//! (`exec/dxil.rs`), minus the wire protocol and the D3D12 front.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use victory_metal_back::copy::{BufLoc, TexLoc};
use victory_metal_back::descriptors::{DescriptorHeap, IrDescriptorTableEntry};
use victory_metal_back::device::Device;
use victory_metal_back::format::dxgi;
use victory_metal_back::memory::{Buffer, BufferKind};
use victory_metal_back::msc::{
    self, Compat, CompileOptions, MscError, ParamKind, RootSignatureDesc, ShaderPipeline,
    Visibility, flags as root_flags,
};
use victory_metal_back::queue::Queue;
use victory_metal_back::sampler::{Address, Border, Compare, Filter, SamplerSpec};
use victory_metal_back::stages;
use victory_metal_back::texture::{TexDim, Texture, TextureDesc, ViewDim, ViewSpec};
use victory_metal_back::typed::{ViewAccess, typed_format};

use crate::req::{self, Arg, Stride, View};

const SRV: u32 = 1;
const UAV: u32 = 2;
const WAIT_MS: u64 = 20_000;

/// Why a request did not give an answer.
pub enum Failure {
    /// The program or the device is outside what this host takes: skip.
    Unsupported(String),
    /// A real failure.
    Failed(String),
}

fn from_back(e: victory_metal_back::Error) -> Failure {
    match e {
        victory_metal_back::Error::Unsupported(_) => Failure::Unsupported(e.to_string()),
        e => Failure::Failed(e.to_string()),
    }
}

fn from_msc(e: MscError) -> Failure {
    match e {
        MscError::Library { .. }
        | MscError::Rejected { .. }
        | MscError::Limit(_)
        | MscError::Unsupported(_) => Failure::Unsupported(e.to_string()),
        e => Failure::Failed(e.to_string()),
    }
}

pub struct Caps {
    pub name: String,
    pub wave_min: u32,
    pub wave_max: u32,
    pub mesh: bool,
}

pub struct Runner {
    device: Device,
    queue: Queue,
    pipelines: HashMap<u64, ShaderPipeline>,
    compat: Compat,
}

fn visibility(v: u8) -> Visibility {
    match v {
        1 => Visibility::Vertex,
        2 => Visibility::Hull,
        3 => Visibility::Domain,
        4 => Visibility::Geometry,
        5 => Visibility::Pixel,
        6 => Visibility::Amplification,
        7 => Visibility::Mesh,
        _ => Visibility::All,
    }
}

fn msc_root(r: &req::Root) -> Result<RootSignatureDesc, Failure> {
    let mut f = 0;
    if r.flags & 1 != 0 {
        f |= root_flags::HEAP_INDEXED;
    }
    if r.flags & 2 != 0 {
        f |= root_flags::SAMPLER_HEAP_INDEXED;
    }
    if r.flags & 4 != 0 {
        f |= root_flags::ALLOW_INPUT_LAYOUT;
    }
    let params = r
        .params
        .iter()
        .map(|p| {
            let kind = match p.kind {
                0 => ParamKind::Constants {
                    register: p.register,
                    space: p.space,
                    num_32bit: p.n32,
                },
                1 => ParamKind::Cbv {
                    register: p.register,
                    space: p.space,
                },
                2 => ParamKind::Srv {
                    register: p.register,
                    space: p.space,
                },
                3 => ParamKind::Uav {
                    register: p.register,
                    space: p.space,
                },
                k => return Err(Failure::Failed(format!("root parameter kind {k}"))),
            };
            Ok(msc::Param {
                kind,
                visibility: visibility(p.vis),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let static_samplers = r
        .samplers
        .iter()
        .map(|s| msc::StaticSampler {
            filter: s.filter,
            address: s.address,
            mip_lod_bias: s.mip_lod_bias,
            max_anisotropy: s.max_anisotropy,
            comparison: s.comparison,
            border: s.border,
            min_lod: s.min_lod,
            max_lod: s.max_lod,
            register: s.register,
            space: s.space,
            visibility: visibility(s.vis),
        })
        .collect();
    Ok(RootSignatureDesc {
        flags: f,
        params,
        static_samplers,
    })
}

/// A `SamplerSpec` from a D3D12 sampler description's numbers.
fn sampler_spec(s: &req::Sampler) -> SamplerSpec {
    let f = s.filter;
    let aniso = f & 0x40 != 0;
    let lin = |bit: u32| {
        if aniso || f & bit != 0 {
            Filter::Linear
        } else {
            Filter::Nearest
        }
    };
    let addr = |a: u32| match a {
        2 => Address::MirrorRepeat,
        3 => Address::ClampToEdge,
        4 => Address::ClampToBorder,
        5 => Address::MirrorClampToEdge,
        _ => Address::Repeat,
    };
    let border = match s.border {
        [0.0, 0.0, 0.0, 1.0] => Border::OpaqueBlack,
        [1.0, 1.0, 1.0, 1.0] => Border::OpaqueWhite,
        _ => Border::TransparentBlack,
    };
    let compare = ((f >> 7) & 3 == 1).then_some(match s.comparison {
        1 => Compare::Never,
        2 => Compare::Less,
        3 => Compare::Equal,
        4 => Compare::LessEqual,
        5 => Compare::Greater,
        6 => Compare::NotEqual,
        7 => Compare::GreaterEqual,
        _ => Compare::Always,
    });
    SamplerSpec {
        min: lin(0x10),
        mag: lin(0x04),
        mip: Some(lin(0x01)),
        address: [addr(s.address[0]), addr(s.address[1]), addr(s.address[2])],
        border,
        compare,
        min_lod: s.min_lod,
        max_lod: s.max_lod,
        max_anisotropy: s.max_anisotropy.max(1),
        lod_bias: s.mip_lod_bias,
    }
}

fn shared(device: &Device, words: &[u32], min_dwords: usize) -> Result<Buffer, Failure> {
    let n = words.len().max(min_dwords).max(4);
    let b = device
        .create_buffer(BufferKind::Shared, 4 * n as u64)
        .map_err(from_back)?;
    let mut padded = words.to_vec();
    padded.resize(n, 0);
    b.write_u32s(0, &padded).map_err(from_back)?;
    Ok(b)
}

/// A texture read or written by the shader, and what to read back.
struct TexUav {
    texture: Texture,
    width: u32,
    height: u32,
    /// dwords per texel
    words: u32,
    format: u32,
    pitch: u32,
    readback: Buffer,
}

enum Out {
    Buffer(Buffer, u32),
    Texture(TexUav),
}

fn texels_bytes(t: &[f32]) -> Vec<u8> {
    t.iter().flat_map(|f| f.to_le_bytes()).collect()
}

impl Runner {
    pub fn new() -> Result<Runner, Failure> {
        let device =
            Device::new().map_err(|e| Failure::Unsupported(format!("no Metal 4 device: {e}")))?;
        let queue = Queue::new(&device).map_err(from_back)?;
        // The game's executor builds with bounds checking (exec/mod.rs). The
        // proofs compare against D3D12's NaN and infinity results, so MSC's
        // NaN/Inf optimisation is off too (a NaN converted to an integer is
        // 0 on D3D12; with it on, Metal gives the int-min). Another decimal
        // number in VICTORY_SHADER_EXEC_COMPAT replaces both.
        let compat = std::env::var("VICTORY_SHADER_EXEC_COMPAT")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .map_or(
                Compat::BOUNDS_CHECK | Compat::DISABLE_NAN_INF_OPTIMIZATION,
                Compat,
            );
        Ok(Runner {
            device,
            queue,
            pipelines: HashMap::new(),
            compat,
        })
    }

    pub fn caps(&self) -> Caps {
        let c = self.device.caps();
        Caps {
            name: c.name.clone(),
            wave_min: c.simd_width,
            wave_max: c.simd_width,
            mesh: c.mesh_shaders,
        }
    }

    /// Converts and makes the pipeline for `key`, once.
    pub fn compile(&mut self, key: u64, root: &req::Root, dxil: &[u8]) -> Result<(), Failure> {
        if self.pipelines.contains_key(&key) {
            return Ok(());
        }
        let desc = msc_root(root)?;
        let options = CompileOptions {
            compat: self.compat,
            ..CompileOptions::default()
        };
        let made = self
            .device
            .compute_pipeline_from_dxil(dxil, Some(&desc), &options)
            .map_err(from_msc)?;
        if made.reflection.threadgroup_size.contains(&0) {
            return Err(Failure::Failed(
                "the shader reports no thread group size".into(),
            ));
        }
        self.pipelines.insert(key, made);
        Ok(())
    }

    fn run_list(&self, mut l: victory_metal_back::record::CommandList) -> Result<(), Failure> {
        l.end().map_err(from_back)?;
        let v = self.queue.submit(vec![l]).map_err(from_back)?;
        self.queue.wait_for(v, WAIT_MS).map_err(from_back)
    }

    fn texture_desc(&self, w: u32, h: u32, format: u32, usage: u32) -> TextureDesc {
        TextureDesc::tex2d(format, w, h, usage)
    }

    pub fn dispatch(
        &mut self,
        key: u64,
        root: &req::Root,
        dxil: &[u8],
        d: &req::Dispatch,
    ) -> Result<Vec<Vec<u32>>, Failure> {
        self.compile(key, root, dxil)?;
        let sp = &self.pipelines[&key];
        let dev = &self.device;
        let heap = DescriptorHeap::new(dev, d.views.len().max(1) as u32).map_err(from_back)?;
        let mut keep: Vec<Buffer> = Vec::new();
        let mut keep_tex: Vec<Texture> = Vec::new();
        let mut outs: Vec<Out> = Vec::new();
        // Uploads (one list), then the dispatch and the texture read-backs.
        let mut upload = self.queue.begin_list().map_err(from_back)?;
        let mut uploads = 0;
        for (i, v) in d.views.iter().enumerate() {
            let at = i as u32;
            match v {
                View::Texture { .. } | View::Cube { .. } => {
                    let (w, h, faces, mips, texels) = match v {
                        View::Cube { size, mips, texels } => {
                            (*size, *size, 6u32, (*mips).max(1), texels)
                        }
                        View::Texture { w, h, texels } => (*w, *h, 1, 1, texels),
                        _ => unreachable!(),
                    };
                    let desc = TextureDesc {
                        dim: TexDim::D2,
                        format: dxgi::R32G32B32A32_FLOAT,
                        width: w,
                        height: h,
                        depth_or_array: faces,
                        mips,
                        samples: 1,
                        usage: SRV,
                    };
                    let tex = dev.create_texture(&desc).map_err(from_back)?;
                    let mut rest: &[f32] = texels;
                    for sub in 0..mips * faces {
                        let (mip, slice) = (sub % mips, sub / mips);
                        let (mw, mh) = ((w >> mip).max(1), (h >> mip).max(1));
                        let pitch = (mw * 16).next_multiple_of(256);
                        let n = (mw * mh * 4) as usize;
                        if rest.len() < n {
                            return Err(Failure::Failed("texture data is short".into()));
                        }
                        let (level, tail) = rest.split_at(n);
                        rest = tail;
                        let mut bytes = vec![0u8; (pitch * mh) as usize];
                        for y in 0..mh as usize {
                            let row = texels_bytes(
                                &level[y * mw as usize * 4..(y + 1) * mw as usize * 4],
                            );
                            bytes[y * pitch as usize..y * pitch as usize + row.len()]
                                .copy_from_slice(&row);
                        }
                        let up = dev
                            .create_buffer(BufferKind::Upload, bytes.len() as u64)
                            .map_err(from_back)?;
                        up.write(0, &bytes).map_err(from_back)?;
                        upload
                            .copy_buffer_to_texture(
                                &TexLoc {
                                    texture: &tex,
                                    mip,
                                    slice,
                                    plane: 0,
                                    origin: [0; 3],
                                },
                                &BufLoc {
                                    buffer: &up,
                                    offset: 0,
                                    format: desc.format,
                                    row_pitch: pitch,
                                    rows: mh,
                                },
                                [mw, mh, 1],
                            )
                            .map_err(from_back)?;
                        uploads += 1;
                        keep.push(up);
                    }
                    let spec = ViewSpec::whole(if faces == 6 {
                        ViewDim::Cube
                    } else {
                        ViewDim::D2
                    });
                    let id = tex.view(&spec).map_err(from_back)?;
                    heap.write(at, IrDescriptorTableEntry::texture(id, 0.0, 0))
                        .map_err(from_back)?;
                    keep_tex.push(tex);
                }
                View::RawUav(n) => {
                    let b = shared(dev, &[], *n as usize)?;
                    heap.write_buffer(at, &b, false).map_err(from_back)?;
                    outs.push(Out::Buffer(b, *n));
                }
                View::HalfUav(e) => {
                    let n = 2 * *e;
                    let b = shared(dev, &[], n as usize)?;
                    let f = typed_format(dxgi::R16G16B16A16_FLOAT).ok_or_else(|| {
                        Failure::Unsupported("no typed R16G16B16A16_FLOAT".into())
                    })?;
                    heap.write_typed_buffer(at, &b, &f, 0, u64::from(*e), ViewAccess::ReadWrite)
                        .map_err(from_back)?;
                    outs.push(Out::Buffer(b, n));
                }
                View::TypedUav {
                    data,
                    format,
                    elem_bytes,
                } => {
                    let b = shared(dev, data, 0)?;
                    let f = typed_format(*format).ok_or_else(|| {
                        Failure::Unsupported(format!("no typed view of DXGI format {format}"))
                    })?;
                    let elements = (data.len() as u64 * 4) / u64::from(*elem_bytes);
                    heap.write_typed_buffer(at, &b, &f, 0, elements, ViewAccess::ReadWrite)
                        .map_err(from_back)?;
                    outs.push(Out::Buffer(b, data.len() as u32));
                }
                View::TextureUav { w, h } | View::UintTextureUav { w, h } => {
                    let (format, words) = if matches!(v, View::TextureUav { .. }) {
                        (dxgi::R32G32B32A32_FLOAT, 4)
                    } else {
                        (dxgi::R32_UINT, 1)
                    };
                    let tex = dev
                        .create_texture(&self.texture_desc(*w, *h, format, UAV | SRV))
                        .map_err(from_back)?;
                    let id = tex.view(&ViewSpec::whole(ViewDim::D2)).map_err(from_back)?;
                    heap.write(at, IrDescriptorTableEntry::texture(id, 0.0, 0))
                        .map_err(from_back)?;
                    let pitch = (*w * 4 * words).next_multiple_of(256);
                    let readback = dev
                        .create_buffer(BufferKind::Readback, u64::from(pitch * *h))
                        .map_err(from_back)?;
                    outs.push(Out::Texture(TexUav {
                        texture: tex,
                        width: *w,
                        height: *h,
                        words,
                        format,
                        pitch,
                        readback,
                    }));
                }
                View::Cbv(data) => {
                    let size = (data.len() * 4).next_multiple_of(256);
                    let b = shared(dev, data, size / 4)?;
                    heap.write_buffer_view(at, b.gpu_va(), size as u64, false)
                        .map_err(from_back)?;
                    keep.push(b);
                }
                View::Buffer { data, stride } => {
                    let b = shared(dev, data, 0)?;
                    match stride {
                        Stride::Raw | Stride::Structured(_) => {
                            heap.write_buffer(at, &b, false).map_err(from_back)?
                        }
                        Stride::Typed | Stride::Format(..) => {
                            let (format, bytes) = match stride {
                                Stride::Format(f, bytes) => (*f, *bytes),
                                _ => (dxgi::R32G32B32A32_FLOAT, 16),
                            };
                            let f = typed_format(format).ok_or_else(|| {
                                Failure::Unsupported(format!(
                                    "no typed view of DXGI format {format}"
                                ))
                            })?;
                            let elements = (data.len() as u64 * 4) / u64::from(bytes);
                            heap.write_typed_buffer(at, &b, &f, 0, elements, ViewAccess::Read)
                                .map_err(from_back)?;
                        }
                    }
                    keep.push(b);
                }
            }
        }
        if uploads > 0 {
            upload
                .barrier(stages::BLIT, stages::DISPATCH, stages::VIS_DEVICE)
                .map_err(from_back)?;
            self.run_list(upload)?;
        }
        // Samplers.
        let sampler_heap =
            DescriptorHeap::new(dev, d.samplers.len().max(1) as u32).map_err(from_back)?;
        let mut samplers = Vec::new();
        for (i, s) in d.samplers.iter().enumerate() {
            let smp = dev.create_sampler(&sampler_spec(s)).map_err(from_back)?;
            sampler_heap
                .write(i as u32, smp.entry())
                .map_err(from_back)?;
            samplers.push(smp);
        }
        // The top-level argument buffer, laid out by MSC.
        let r = &sp.reflection;
        let tlab = self
            .queue
            .alloc_constants(u64::from(r.layout.size).max(16), 8)
            .map_err(from_back)?;
        tlab.zero();
        for (param, arg) in &d.roots {
            let Some(entry) = r.layout.entries.get(*param as usize) else {
                return Err(Failure::Failed(format!(
                    "root parameter {param} is not in the layout"
                )));
            };
            let off = u64::from(entry.offset);
            match arg {
                Arg::Constants(words) => tlab.write_u32s(off, words).map_err(from_back)?,
                Arg::View { data, .. } => {
                    let b = shared(dev, data, (data.len() * 4).next_multiple_of(256) / 4)?;
                    tlab.write_u64(off, b.gpu_va()).map_err(from_back)?;
                    keep.push(b);
                }
                Arg::Uav(n) => {
                    let b = shared(dev, &[], *n as usize)?;
                    tlab.write_u64(off, b.gpu_va()).map_err(from_back)?;
                    keep.push(b);
                }
            }
        }
        // The static samplers' table.
        let mut static_table = None;
        if let Some(idx) = r.layout.static_sampler_table
            && let Some(entry) = r.layout.entries.get(idx)
        {
            let table =
                DescriptorHeap::new(dev, root.samplers.len().max(1) as u32).map_err(from_back)?;
            for (i, s) in root.samplers.iter().enumerate() {
                let spec = sampler_spec(&req::Sampler {
                    filter: s.filter,
                    address: s.address,
                    mip_lod_bias: s.mip_lod_bias,
                    max_anisotropy: s.max_anisotropy,
                    comparison: s.comparison,
                    border: match s.border {
                        1 => [0.0, 0.0, 0.0, 1.0],
                        2 => [1.0; 4],
                        _ => [0.0; 4],
                    },
                    min_lod: s.min_lod,
                    max_lod: s.max_lod,
                });
                let smp = dev.create_sampler(&spec).map_err(from_back)?;
                table.write(i as u32, smp.entry()).map_err(from_back)?;
                samplers.push(smp);
            }
            tlab.write_u64(u64::from(entry.offset), table.gpu_va())
                .map_err(from_back)?;
            static_table = Some(table);
        }
        let mut l = self.queue.begin_list().map_err(from_back)?;
        l.set_heaps(&heap, Some(&sampler_heap)).map_err(from_back)?;
        l.set_root_arguments(&tlab).map_err(from_back)?;
        l.dispatch(&sp.pipeline, d.groups, r.threadgroup_size)
            .map_err(from_back)?;
        if outs.iter().any(|o| matches!(o, Out::Texture(_))) {
            l.barrier(stages::DISPATCH, stages::BLIT, stages::VIS_DEVICE)
                .map_err(from_back)?;
            for o in &outs {
                if let Out::Texture(t) = o {
                    l.copy_texture_to_buffer(
                        &BufLoc {
                            buffer: &t.readback,
                            offset: 0,
                            format: t.format,
                            row_pitch: t.pitch,
                            rows: t.height,
                        },
                        &TexLoc {
                            texture: &t.texture,
                            mip: 0,
                            slice: 0,
                            plane: 0,
                            origin: [0; 3],
                        },
                        [t.width, t.height, 1],
                    )
                    .map_err(from_back)?;
                }
            }
        }
        self.run_list(l)?;
        let mut out = Vec::new();
        for o in &outs {
            out.push(match o {
                Out::Buffer(b, n) => b.read_u32s(0, *n as usize).map_err(from_back)?,
                Out::Texture(t) => {
                    let row = (t.width * t.words) as usize;
                    let mut all = Vec::with_capacity(row * t.height as usize);
                    for y in 0..t.height {
                        all.extend(
                            t.readback
                                .read_u32s(u64::from(y * t.pitch), row)
                                .map_err(from_back)?,
                        );
                    }
                    all
                }
            });
        }
        drop((keep, keep_tex, samplers, static_table));
        Ok(out)
    }
}

/// The cache key of a request prefix.
pub fn key_of(prefix: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    prefix.hash(&mut h);
    h.finish()
}
