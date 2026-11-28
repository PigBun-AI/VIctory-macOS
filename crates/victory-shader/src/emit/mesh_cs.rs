//! mac S5: a mesh program as a compute program plus a fetch vertex program.
//!
//! Metal's mesh pipeline counts `round_up(NV, 4) x vertex_stride + NP x primitive_stride` against
//! 32 KiB (S2 shrinks the padding, not the content) and Apple's GPU compiler crashes on a large
//! class of mesh programs (S1's notes). A compute kernel that writes the vertices and primitives to
//! memory, then an ordinary vertex + pixel draw that fetches them, touches neither: that is what
//! this module makes of one mesh program (`docs/metal/mesh-compute.md`, "Translator").
//!
//! * The **compute form** is the same lowering as the mesh program (`lower.rs`), run with a
//!   [`Layout`]: `setMeshOutputCounts`, `emitIndices`, `storeVertexOutput` and `storePrimitiveOutput`
//!   become `rawBufferStore`s into the scratch UAV, everything else (resource reads, group memory,
//!   S1's exchange of the other vertices) is kept. `finish` writes a `cs_6_6` of it.
//! * The **fetch form** ([`fetch_vs`]) is a `vs_6_6` that reads `SV_VertexID = 3 x primitive +
//!   corner` and `SV_InstanceID = group` and returns the outputs the pixel program reads.
//! * Outputs the pixel program does not read are left out ([`Needs`]): their stores are dropped and
//!   the code that computed them goes with them.
//!
//! The interface is `docs/metal/mesh-compute.md` section 8 ("Interface v1", the Metal back's): the layout
//! text `vmcl 1`, the container part `VMCS`, the binding convention. Per group `g` (the flat group of the
//! dispatch), at `g x group_stride`:
//!
//! ```text
//!   0                     u32 vertex count, u32 primitive count, 2 x u32 reserved
//!   vhead + v x vstride   vertex v: its elements, 4 bytes a component, in list order
//!   parea + p x pstride   primitive p: u32 (v0 | v1 << 8 | v2 << 16), its elements
//! ```
//!
//! The translator attaches the part to the mesh shader's own container (`attach`); a host that does not ask
//! for it ([`MeshComputeMode`]) gets the container as before. A lane writes only its own vertex and
//! primitive, so the counts are the declared maxima cut down to the lane count; a culled primitive has its
//! indices at 0, as in the mesh form (a triangle without area), so the scratch has no cull element.

use super::bitcode::{Attr, Bin, Cast, Term, V, pred};
use super::container::{self, SigElement};
use super::shader::{Scratch, Shader};
use super::vertex_flat::VertexRead;
use super::HostShader;
use crate::ShaderError;
use crate::dxbc::{Container, FourCC};
use crate::native::HwStage;

/// The scratch buffer's register space and register: `u0` of the compute form and `t0` of the
/// fetch form, in this space (the back appends one root parameter for it, last, so the title's
/// root arguments keep their places).
pub const SCRATCH_SPACE: u32 = 4_000_000_000;
pub const SCRATCH_REGISTER: u32 = 0;
/// Bytes before vertex 0 in a group: the counts, padded.
pub const VERTEX_HEAD: u32 = 16;

/// Whether the host asks for the compute form of a mesh program, and for which ones
/// (`set_host_mesh_compute`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshComputeMode {
    /// Never (the Windows line's behaviour, and the default).
    Off,
    /// Only for programs Metal's mesh pipeline cannot take for their size (over 32 KiB).
    Problem,
    /// For every mesh program (tests; the census; programs the host knows crash its compiler).
    All,
}

/// The scratch layout of one mesh program (see the module notes).
#[derive(Debug, Clone)]
pub(crate) struct Layout {
    pub threads: u32,
    /// Vertices and primitives of a group, the declared maxima cut to the lane count.
    pub max_vertices: u32,
    pub max_primitives: u32,
    pub vertex_stride: u32,
    pub prim_stride: u32,
    pub prim_area: u32,
    pub group_stride: u32,
    /// Byte offset in the vertex record of each element of the vertex list (`None`: left out).
    pub vertex: Vec<Option<u32>>,
    /// Likewise for the primitive list; the first word is the indices.
    pub prim: Vec<Option<u32>>,
}

fn align(n: u32, to: u32) -> u32 {
    n.next_multiple_of(to)
}

impl Layout {
    pub(crate) fn build(
        outputs: &[SigElement],
        prims: &[SigElement],
        threads: u32,
        max_vertices: u32,
        max_primitives: u32,
    ) -> Result<Self, ShaderError> {
        const KINDS: [u8; 6] = [0, 3, 4, 5, 6, 7];
        const REFUSED: ShaderError = ShaderError::Unsupported {
            offset: 0,
            what: "a mesh output the fetch shader cannot carry",
        };
        for e in outputs.iter().chain(prims) {
            if !KINDS.contains(&e.kind) || !matches!(e.comp, 4 | 5 | 9) || e.cols == 0 || e.cols > 4 {
                return Err(REFUSED);
            }
        }
        let place = |list: &[SigElement], start: u32| {
            let mut at = start;
            let slots: Vec<Option<u32>> = list
                .iter()
                .map(|e| {
                    let here = at;
                    at += 4 * u32::from(e.cols);
                    Some(here)
                })
                .collect();
            (slots, align(at, 16))
        };
        let (vertex, vertex_stride) = place(outputs, 0);
        let (prim, prim_stride) = place(prims, 4);
        let (vertex_stride, prim_stride) = (vertex_stride.max(16), prim_stride.max(16));
        let (nv, np) = (max_vertices.min(threads), max_primitives.min(threads));
        let prim_area = VERTEX_HEAD + nv * vertex_stride;
        Ok(Layout {
            threads,
            max_vertices: nv,
            max_primitives: np,
            vertex_stride,
            prim_stride,
            prim_area,
            group_stride: prim_area + np * prim_stride,
            vertex,
            prim,
        })
    }

    /// The `vmcl 1` text the back reads (`metal/back/src/mesh_compute.rs`).
    pub(crate) fn text(&self, outputs: &[SigElement], prims: &[SigElement]) -> String {
        let mut t = String::from("vmcl 1\n");
        let ty = |e: &SigElement| match e.comp {
            9 => "float",
            4 => "int",
            _ => "uint",
        };
        t += &format!("threads {} 1 1\n", self.threads);
        t += &format!("maxv {}\nmaxp {}\n", self.max_vertices, self.max_primitives);
        t += &format!("vstride {}\npstride {}\n", self.vertex_stride, self.prim_stride);
        t += &format!("vhead {VERTEX_HEAD}\nparea {}\ngstride {}\n", self.prim_area, self.group_stride);
        t += &format!("space {SCRATCH_SPACE}\nregister {SCRATCH_REGISTER}\n");
        for (tag, list, slots) in [("v", outputs, &self.vertex), ("p", prims, &self.prim)] {
            for (e, at) in list.iter().zip(slots) {
                if let Some(at) = at {
                    t += &format!("{tag} {} {} {} {} 1 {} {at} {}\n", e.name, e.index, e.kind, ty(e), e.cols, e.interp);
                }
            }
        }
        t
    }
}

/// What [`super::emit_program`] hands back of a compute form: the layout and the signature lists
/// it was made from (the fetch form is made of them).
#[derive(Debug, Clone)]
pub(crate) struct Built {
    pub layout: Layout,
    pub outputs: Vec<SigElement>,
    pub prims: Vec<SigElement>,
    /// What S2 would have made of the lists, in Metal's mesh struct: whether the mesh form fits.
    pub metal_bytes: u64,
}

/// Bytes of Metal's mesh output for these lists (the rule S2 sorts by: `mesh_order`).
pub(crate) fn metal_bytes(outputs: &[SigElement], prims: &[SigElement], threads: u32, max_v: u32, max_p: u32) -> u64 {
    use super::container::{MESH_OUTPUT_LIMIT, mesh_output_bytes};
    let after = |end: u32, size: u32| end.next_multiple_of(size.max(1)) + size;
    // D3D12's cap first: the mesh form cuts its maxima to the lanes when the outputs do not fit.
    let (mut nv, mut np) = (max_v, max_p);
    if mesh_output_bytes(outputs, prims, nv, np) > MESH_OUTPUT_LIMIT {
        nv = nv.min(threads);
        np = np.min(threads);
    }
    let (mut v, mut p) = (outputs.to_vec(), prims.to_vec());
    super::mesh_order::vertex(&mut v);
    super::mesh_order::primitive(&mut p);
    let mut end = 0;
    for e in v.iter().filter(|e| matches!(e.kind, 6 | 7)) {
        for _ in 0..e.cols {
            end = after(end, 4);
        }
    }
    end = after(end, 16);
    for e in v.iter().filter(|e| e.kind != 3) {
        end = after(end, 4 * u32::from(e.cols.max(1)));
    }
    let mut pend = 0;
    for e in &p {
        pend = after(pend, 4 * u32::from(e.cols.max(1)));
    }
    u64::from(nv.next_multiple_of(4)) * u64::from(end) + u64::from(np) * u64::from(pend)
}

/// The fetch vertex program of a compute form, as a container: `SV_VertexID = 3 x p + corner`,
/// `SV_InstanceID` the flat group. A dead primitive (past the count, an index past the vertex count) returns
/// zeros everywhere, a triangle without area.
pub(crate) fn fetch_vs(b: &Built) -> Result<Vec<u8>, ShaderError> {
    let l = &b.layout;
    let mut s = Shader::new(false);
    s.scratch = Some(Scratch { space: SCRATCH_SPACE, register: SCRATCH_REGISTER, uav: false, handle: None, dims: None });
    let sys = |name: &str, kind: u8, row: u32| SigElement {
        name: name.into(),
        index: 0,
        kind,
        comp: 5,
        interp: 0,
        row: Some(row),
        col: 0,
        cols: 1,
        usage: 1,
        half: false,
    };
    s.inputs = vec![sys("SV_VertexID", 1, 0), sys("SV_InstanceID", 2, 1)];
    // The outputs: the vertex elements, then the primitive's, as flat.
    let mut written: Vec<(usize, bool, u32)> = Vec::new(); // (output id, from the primitive record, offset)
    for (list, slots, primitive) in [(&b.outputs, &l.vertex, false), (&b.prims, &l.prim, true)] {
        for (e, at) in list.iter().zip(slots) {
            if let Some(at) = *at {
                let mut e = e.clone();
                e.usage = ((1u16 << e.cols) - 1) as u8;
                if primitive {
                    e.interp = 1;
                }
                written.push((s.outputs.len(), primitive, at));
                s.outputs.push(e);
            }
        }
    }
    let entry = s.entry;
    let (i32t, void) = (s.t.i32, s.t.void);
    let iu = s.m.undef(i32t);
    let input = |s: &mut Shader, id: u32| {
        let args = [s.m.i32(id), s.m.i32(0), s.m.i8(0), s.m.undef(i32t)];
        s.op(entry, "loadInput.i32", Attr::ReadNone, i32t, 4, &args)
    };
    let (vid, iid) = (input(&mut s, 0), input(&mut s, 1));
    let k = |s: &mut Shader, v: u32| s.m.i32(v);
    let three = k(&mut s, 3);
    let prim = s.m.bin(entry, Bin::UDiv, vid, three);
    let corner = s.m.bin(entry, Bin::URem, vid, three);
    let stride = k(&mut s, l.group_stride);
    let group = s.m.bin(entry, Bin::Mul, iid, stride);
    let h = s.scratch_handle();
    s.scratch_dims_handle(); // (declared, not read: the back's convention)
    let align_k = k(&mut s, 4);
    let load = |s: &mut Shader, at: V, n: u8| {
        let mask = s.m.i8((1u8 << n) - 1);
        let res = s.t.res_ret;
        s.op(entry, "rawBufferLoad.f32", Attr::ReadOnly, res, 139, &[h, at, iu, mask, align_k])
    };
    let bits = |s: &mut Shader, r: V, c: u32| {
        let x = s.m.extract(entry, r, c);
        s.m.cast(entry, Cast::Bitcast, x, i32t)
    };
    // The counts, and the primitive's index word.
    let counts = load(&mut s, group, 2);
    let (vc, pc) = (bits(&mut s, counts, 0), bits(&mut s, counts, 1));
    let pstride = k(&mut s, l.prim_stride);
    let off = s.m.bin(entry, Bin::Mul, prim, pstride);
    let area = k(&mut s, l.prim_area);
    let off = s.m.bin(entry, Bin::Add, off, area);
    let prec = s.m.bin(entry, Bin::Add, group, off);
    let w = load(&mut s, prec, 1);
    let word = bits(&mut s, w, 0);
    // dead = p >= primitive count || any index >= vertex count
    let mut dead = s.m.cmp(entry, pred::IUGE, prim, pc);
    let ff = k(&mut s, 0xFF);
    for c in 0..3u32 {
        let shift = k(&mut s, 8 * c);
        let idx = s.m.bin(entry, Bin::LShr, word, shift);
        let idx = s.m.bin(entry, Bin::And, idx, ff);
        let past = s.m.cmp(entry, pred::IUGE, idx, vc);
        dead = s.m.bin(entry, Bin::Or, dead, past);
    }
    // The corner's vertex index.
    let k8 = k(&mut s, 8);
    let shift = s.m.bin(entry, Bin::Mul, corner, k8);
    let idx = s.m.bin(entry, Bin::LShr, word, shift);
    let idx = s.m.bin(entry, Bin::And, idx, ff);
    let vstride = k(&mut s, l.vertex_stride);
    let voff = s.m.bin(entry, Bin::Mul, idx, vstride);
    let head = k(&mut s, VERTEX_HEAD);
    let voff = s.m.bin(entry, Bin::Add, voff, head);
    let vrec = s.m.bin(entry, Bin::Add, group, voff);
    let zf = s.m.f32(0);
    let zi = k(&mut s, 0);
    // Every load first, then the stores.
    let mut values: Vec<(usize, u8, V, bool)> = Vec::new();
    for &(id, primitive, at) in &written {
        let e = s.outputs[id].clone();
        let rec = if primitive { prec } else { vrec };
        let atk = k(&mut s, at);
        let at = s.m.bin(entry, Bin::Add, rec, atk);
        let r = load(&mut s, at, e.cols);
        for c in 0..e.cols {
            let x = s.m.extract(entry, r, u32::from(c));
            let float = e.comp == 9;
            let v = if float { x } else { s.m.cast(entry, Cast::Bitcast, x, i32t) };
            let v = s.m.select(entry, dead, if float { zf } else { zi }, v);
            values.push((id, c, v, float));
        }
    }
    for (id, c, v, float) in values {
        let args = [s.m.i32(id as u32), s.m.i32(0), s.m.i8(c), v];
        let name = if float { "storeOutput.f32" } else { "storeOutput.i32" };
        s.op(entry, name, Attr::NoUnwind, void, 5, &args);
    }
    s.m.terminate(entry, Term::Ret);
    Ok(s.finish())
}

/// What Metal's mesh output may be, in bytes (the pipeline does not build above it).
pub const METAL_MESH_LIMIT: u64 = 32 * 1024;

/// The products of one mesh program, for the `VMCS` part (`docs/metal/mesh-compute.md`, 8.1).
#[derive(Debug, Clone)]
pub struct MeshCompute {
    /// The compute form (a complete DXIL container): `numthreads` as the mesh program's.
    pub cs: HostShader,
    /// The fetch vertex program (a complete DXIL container).
    pub vs: HostShader,
    /// The scratch layout, `vmcl 1` text.
    pub layout: String,
    /// Bytes of one group in the scratch.
    pub group_bytes: u32,
    /// Vertices to draw per group: three a primitive.
    pub draw_vertices: u32,
    /// What the mesh pipeline of the same program would take, in Metal's mesh struct, in bytes.
    pub metal_mesh_bytes: u64,
}

/// The part's FourCC.
pub const VMCS: FourCC = FourCC(*b"VMCS");

impl MeshCompute {
    pub(super) fn new(cs: HostShader, built: &Built) -> Result<Self, ShaderError> {
        let dxil = fetch_vs(built)?;
        let vs = HostShader {
            stage: HwStage::Vs,
            dxil,
            user_data: Vec::new(),
            samplers: Vec::new(),
            static_samplers: Vec::new(),
            root_cbvs: Vec::new(),
            root_srvs: Vec::new(),
            root_uavs: Vec::new(),
            root_indexed_views: Vec::new(),
            vertex_reads: Vec::<VertexRead>::new(),
        };
        let l = &built.layout;
        Ok(MeshCompute {
            cs,
            vs,
            layout: l.text(&built.outputs, &built.prims),
            group_bytes: l.group_stride,
            draw_vertices: 3 * l.max_primitives,
            metal_mesh_bytes: built.metal_bytes,
        })
    }

    /// The payload of the `VMCS` part: version 1, flags 0, then the layout text, the compute container and
    /// the fetch container, each 4-aligned.
    pub fn vmcs(&self) -> Vec<u8> {
        let (layout, cs, vs) = (self.layout.as_bytes(), &self.cs.dxil, &self.vs.dxil);
        let mut out = Vec::new();
        for v in [1u32, 0, 0, 0, 0, 0, 0, 0] {
            out.extend(v.to_le_bytes());
        }
        let place = |out: &mut Vec<u8>, at: usize, data: &[u8]| {
            let off = out.len() as u32;
            out.extend(data);
            out.resize(out.len().next_multiple_of(4), 0);
            out[at..at + 4].copy_from_slice(&off.to_le_bytes());
            out[at + 4..at + 8].copy_from_slice(&(data.len() as u32).to_le_bytes());
        };
        place(&mut out, 8, layout);
        place(&mut out, 16, cs);
        place(&mut out, 24, vs);
        out
    }

    /// `container` (the translated mesh program) with the `VMCS` part appended, rehashed.
    pub fn attach(&self, container: &[u8]) -> Result<Vec<u8>, ShaderError> {
        let c = Container::parse(container)?;
        let payload = self.vmcs();
        let mut parts: Vec<(&[u8; 4], &[u8])> = c.parts.iter().map(|p| (&p.fourcc.0, p.data)).collect();
        parts.push((&VMCS.0, &payload));
        Ok(container::build(&parts))
    }
}

/// The three products of a `VMCS` part (a reader of the contract: tests, the census): the layout text, the
/// compute container and the fetch container. `None` for a container without the part, or of another version.
pub fn read_vmcs(container: &[u8]) -> Option<(String, Vec<u8>, Vec<u8>)> {
    let c = Container::parse(container).ok()?;
    let d = c.part(VMCS)?.data;
    let word = |at: usize| d.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    if word(0)? != 1 || word(4)? != 0 {
        return None;
    }
    let range = |at: usize| d.get(word(at)?..word(at)?.checked_add(word(at + 4)?)?);
    Some((String::from_utf8(range(8)?.to_vec()).ok()?, range(16)?.to_vec(), range(24)?.to_vec()))
}
