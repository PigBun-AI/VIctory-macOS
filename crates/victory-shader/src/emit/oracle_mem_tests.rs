//! mac S6, oracle batch B: the memory instructions (`inv.isa.opcode_oracle`, nodes `isa.smem.load`,
//! `isa.mem.buffer_load`, `isa.mem.buffer_store`, `isa.mem.atomic`, `isa.mem.flat`, `isa.lds.*`).
//!
//! As `oracle_tests` (the same row/outcome/known machinery), but the instruction under test reads
//! or writes memory: a one-thread (LDS: several) compute program whose instruction is built from
//! its mnemonic (`gfx10::opcode_of`), run for a sweep of addresses (offset, soffset, index, past
//! the view's end) and compared with a CPU model of the memory.
//!
//! References (what "right" means): the GFX10 (RDNA 2) ISA manual, chapters "Scalar Memory",
//! "Vector Memory Buffer Instructions" (MUBUF/MTBUF address = inst offset + VGPR offset + SGPR
//! offset, + index * stride; buffer data formats of LLVM's `UfmtGFX10`), "Data Share" (DS), and
//! Mesa's `ac_*` buffer-format tables; and the D3D11.3/D3D12 functional specification, "Out of
//! bounds addressing" (a read of a view past its end returns 0 for each 32-bit component that is
//! out of bounds; a write there is dropped). The comparison is therefore per dword: a vector load
//! that straddles the view's end reads its in-range dwords and 0 for the others.

use super::host::{Dispatch, Host, Root, Stride, View};
use super::oracle_tests::{Outcome, panic_text, report_family};
use super::valu_tests::{ONE_AT_A_TIME, s_mov_b32, vop1};
use super::HostShader;
use crate::gfx10::opcode_of;
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};

pub(super) const S_ENDPGM: u32 = 0xBF81_0000;

// ---- encoders ----

pub(super) fn opc(name: &str) -> u32 {
    u32::from(opcode_of(name).unwrap_or_else(|| panic!("{name} is not a GFX10 mnemonic")).1)
}

/// SMEM `name sdata, sbase, imm [soffset]`; `soffset`: an SGPR, or `None` (null).
pub(super) fn smem(name: &str, sdata: u8, sbase: u8, imm: u32, soffset: Option<u8>) -> [u32; 2] {
    [
        0x3D << 26 | opc(name) << 18 | u32::from(sdata) << 6 | u32::from(sbase) >> 1,
        soffset.map_or(125, u32::from) << 25 | imm & 0x1F_FFFF,
    ]
}

/// What a MUBUF/MTBUF instruction names (`soffset`: SGPR number, or 128 for the constant 0).
#[derive(Clone, Copy)]
pub(super) struct Buf {
    pub vdata: u8,
    pub vaddr: u8,
    pub srsrc: u8,
    pub soffset: u8,
    pub offset: u16,
    pub offen: bool,
    pub idxen: bool,
    pub glc: bool,
}

pub(super) fn mubuf(name: &str, b: Buf) -> [u32; 2] {
    let o = opc(name);
    [
        0x38 << 26
            | o << 18
            | u32::from(b.glc) << 14
            | u32::from(b.idxen) << 13
            | u32::from(b.offen) << 12
            | u32::from(b.offset) & 0xFFF,
        u32::from(b.soffset) << 24
            | u32::from(b.srsrc >> 2) << 16
            | u32::from(b.vdata) << 8
            | u32::from(b.vaddr),
    ]
}

pub(super) fn mtbuf(name: &str, b: Buf, format: u8) -> [u32; 2] {
    let o = opc(name);
    [
        0x3A << 26
            | u32::from(format & 0x7F) << 19
            | (o & 7) << 16
            | u32::from(b.glc) << 14
            | u32::from(b.idxen) << 13
            | u32::from(b.offen) << 12
            | u32::from(b.offset) & 0xFFF,
        u32::from(b.soffset) << 24
            | (o >> 3 & 1) << 21
            | u32::from(b.srsrc >> 2) << 16
            | u32::from(b.vdata) << 8
            | u32::from(b.vaddr),
    ]
}

/// `v_mov_b32 vdst, literal`.
pub(super) fn v_mov_lit(vdst: u8, value: u32) -> [u32; 2] {
    [vop1(0x001, vdst, 0xFF), value]
}

/// `buffer_store_dword v<src>, off, s[12:15], 0 offset:<byte>` (the result writer of the programs).
pub(super) fn store_out(src: u8, byte: u16) -> [u32; 2] {
    mubuf(
        "buffer_store_dword",
        Buf { vdata: src, vaddr: 0, srsrc: 12, soffset: 128, offset: byte, offen: false, idxen: false, glc: false },
    )
}

/// Which view the program's table entry 0 is (the UAV `s[12:15]` is the entry after it).
#[derive(Clone, Copy)]
pub(super) enum Srv {
    /// None: the UAV is entry 0.
    No,
    /// A buffer SRV and its used-resource stride (1: raw bytes by index; 16: structured; 4: typed).
    Buffer(u32),
    /// A constant buffer.
    Cbv,
    /// A typed buffer UAV (the program also writes it).
    Uav,
}

pub(super) struct Cfg {
    pub threads: u32,
    pub lds_bytes: u32,
    pub srv: Srv,
    pub vgprs: u32,
}

impl Cfg {
    pub(super) fn new(srv: Srv) -> Self {
        Cfg { threads: 1, lds_bytes: 0, srv, vgprs: 16 }
    }
}

/// Builds the program: the table at s[4:5], the view (if any) at s[8:11], the UAV at s[12:15],
/// the four root constants in s0..s3 and copied to v1..v4.
pub(super) fn build(body: &[u32], cfg: &Cfg) -> Result<HostShader, String> {
    let uav_at: u32 = if matches!(cfg.srv, Srv::No) { 0 } else { 32 };
    let mut words = vec![s_mov_b32(5, 0x80)];
    if !matches!(cfg.srv, Srv::No) {
        words.extend(smem("s_load_dwordx4", 8, 4, 0, None));
    }
    words.extend(smem("s_load_dwordx4", 12, 4, uav_at, None));
    for i in 0..4u8 {
        words.push(vop1(0x001, i + 1, u16::from(i))); // v_mov_b32 v1+i, s_i
    }
    words.extend_from_slice(body);
    words.push(S_ENDPGM);
    let range = |kind, at: u32, stride: u32, lower_bound: u32| UsedResource {
        kind,
        root: RootType::Table,
        table_offset_dwords: at,
        lower_bound,
        stride,
        dwords: 4,
        user_data: 4,
        slot: 1,
    };
    let mut used = Vec::new();
    match cfg.srv {
        Srv::No => {}
        Srv::Buffer(stride) => used.push(range(ResourceType::Srv, 0, stride, 0)),
        Srv::Cbv => used.push(range(ResourceType::Cbv, 0, 0, 0)),
        Srv::Uav => used.push(range(ResourceType::Uav, 0, 4, 0)),
    }
    used.push(range(ResourceType::Uav, uav_at / 4, 0, u32::from(matches!(cfg.srv, Srv::Uav))));
    let native = NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
        vgprs: cfg.vgprs as u16,
        layout: vec![
            RootParam { root: RootType::Constants, table_offset_dwords: 0, sgprs: 4, base_register: 0, user_data: 0, slot: 0 },
            RootParam { root: RootType::Table, table_offset_dwords: 0, sgprs: 1, base_register: 0, user_data: 4, slot: 1 },
        ],
        used,
        io: StageIo::Cs(ComputeIo { rsrc2: (5 << 1) | ((cfg.lds_bytes / 512) << 15), threads: [cfg.threads, 1, 1] }),
        code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
    };
    shader_of(&native)
}

/// Translates `native` to a [`HostShader`] (the translator's refusal as text).
pub(super) fn shader_of(native: &NativeProgram) -> Result<HostShader, String> {
    super::intersect_tests::build_compute_shader(native)
        .map(|(s, _)| s)
        .map_err(|e| format!("{e:?}"))
}

// ---- running ----

/// Builds with the panic and refusal of the translator as an outcome.
pub(super) fn try_build(body: &[u32], cfg: &Cfg) -> Result<HostShader, Outcome> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(body, cfg))) {
        Ok(Ok(s)) => Ok(s),
        Ok(Err(why)) => Err(Outcome::Untranslated(why)),
        Err(e) => Err(Outcome::Untranslated(panic_text(e))),
    }
}

/// Dispatches `shader` once per input set of root constants over `views`, and hands what the UAVs
/// hold to `check`, which returns the text of a mismatch.
pub(super) fn sweep(
    shader: &HostShader,
    views: &[View],
    samplers: &[super::host::api::D3D12_SAMPLER_DESC],
    cases: &[[u32; 4]],
    mut check: impl FnMut([u32; 4], &[Vec<u32>]) -> Option<String>,
) -> Outcome {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(host) = Host::new(false) else {
        return Outcome::Refused("no device".into());
    };
    let (root, pso) = match host.compute_pipeline(shader) {
        Ok(p) => p,
        Err(e) if e.contains("unsupported") => return Outcome::Refused(e),
        Err(e) => return Outcome::Mismatch { cases: 0, bad: 1, examples: vec![format!("compute_pipeline: {e}")] },
    };
    let (mut bad, mut examples, mut ran) = (0usize, Vec::new(), 0usize);
    for &c in cases {
        let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
        for u in &shader.user_data {
            ud[u.index as usize] = match (u.root, u.slot) {
                (RootType::Constants, 0) => c[u.dword as usize],
                _ => 0,
            };
        }
        let got = host.dispatch(
            &root,
            &pso,
            &Dispatch { roots: vec![(0, Root::Constants(ud))], views, samplers, groups: [1, 1, 1] },
        );
        let got = match got {
            Ok(g) => g,
            Err(e) if e.contains("unsupported") => return Outcome::Refused(e),
            Err(e) => return Outcome::Mismatch { cases: ran, bad: bad.max(1), examples: vec![format!("dispatch: {e}")] },
        };
        ran += 1;
        if let Some(text) = check(c, &got) {
            bad += 1;
            if examples.len() < 3 {
                examples.push(text);
            }
        }
    }
    if bad == 0 { Outcome::Passed(ran) } else { Outcome::Mismatch { cases: ran, bad, examples } }
}

/// Runs `shader` once over a view of 64 non-zero dwords (small buffers share 256-byte slots) and drops the answer: the buffers of
/// the next dispatches come from memory that holds this pattern past their end, so a vector load
/// that reads beyond its view's end shows it (the executor sizes a view by its data exactly, and
/// the memory behind it is whatever the allocator left).
pub(super) fn warm(shader: &HostShader, outs: u32) {
    let big = pattern(64);
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(host) = Host::new(false) else { return };
    let Ok((root, pso)) = host.compute_pipeline(shader) else { return };
    let ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
    for _ in 0..2 {
        let _ = host.dispatch(
            &root,
            &pso,
            &Dispatch {
                roots: vec![(0, Root::Constants(ud.clone()))],
                views: &[View::Buffer { data: &big, stride: Stride::Raw }, View::RawUav(outs)],
                samplers: &[],
                groups: [1, 1, 1],
            },
        );
    }
}

/// One outcome out of the outcomes of a row's forms (each named).
pub(super) fn merge(parts: Vec<(String, Outcome)>) -> Outcome {
    let mut cases = 0;
    let (mut bad, mut examples) = (0, Vec::new());
    let (mut untranslated, mut refused) = (None, None);
    for (form, o) in parts {
        match o {
            Outcome::Passed(n) => cases += n,
            Outcome::Mismatch { cases: n, bad: b, examples: ex } => {
                cases += n;
                bad += b;
                examples.extend(ex.into_iter().take(2).map(|t| format!("[{form}] {t}")));
            }
            Outcome::Untranslated(t) => untranslated = untranslated.or(Some(format!("[{form}] {t}"))),
            Outcome::Refused(t) => refused = refused.or(Some(format!("[{form}] {t}"))),
        }
    }
    if let Some(t) = untranslated {
        return Outcome::Untranslated(t);
    }
    if bad > 0 {
        return Outcome::Mismatch { cases, bad, examples };
    }
    match (cases, refused) {
        (0, Some(t)) => Outcome::Refused(t),
        _ => Outcome::Passed(cases),
    }
}

/// Dwords no value of the model is zero in.
pub(super) fn pattern(n: usize) -> Vec<u32> {
    (0..n as u32).map(|i| 0x1111_1111u32.wrapping_mul(i + 1) ^ 0x8000_0000 ^ i).collect()
}

/// The dword a load reads at byte `at` of a view of `data`: 0 outside it (D3D12: out of bounds
/// reads 0 for each dword) and for an address that is not the dword's own.
pub(super) fn dword_at(data: &[u32], at: u64) -> u32 {
    if at % 4 == 0 && at + 4 <= 4 * data.len() as u64 { data[(at / 4) as usize] } else { 0 }
}

// ---- buffer_load_dword* from a raw or structured SRV, and from a CBV ----

/// A MUBUF address form.
#[derive(Clone, Copy, Debug)]
struct Form {
    offen: bool,
    idxen: bool,
    imm: u16,
    soff: bool,
    /// Used-resource stride (idxen): 1 = the index is the byte; 16 = elements of 16 bytes.
    stride: u32,
}

const FORMS: &[Form] = &[
    Form { offen: false, idxen: false, imm: 0, soff: false, stride: 0 },
    Form { offen: true, idxen: false, imm: 0, soff: false, stride: 0 },
    Form { offen: true, idxen: false, imm: 8, soff: false, stride: 0 },
    Form { offen: true, idxen: false, imm: 4, soff: true, stride: 0 },
    Form { offen: false, idxen: false, imm: 12, soff: true, stride: 0 },
    Form { offen: false, idxen: true, imm: 0, soff: false, stride: 1 },
    Form { offen: false, idxen: true, imm: 4, soff: false, stride: 16 },
    Form { offen: true, idxen: true, imm: 4, soff: false, stride: 16 },
];

/// Input sets (index in c0 -> v1, offset in c1 -> v2, soffset in c3 -> s3) for a form.
fn form_cases(f: Form) -> Vec<[u32; 4]> {
    let offs: &[u32] = if f.offen {
        &[0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 64, 100, 0x8000_0000]
    } else {
        &[0]
    };
    let idxs: &[u32] = if f.idxen {
        if f.stride == 1 { &[0, 4, 8, 20, 36, 44, 48, 200] } else { &[0, 1, 2, 3, 4, 100] }
    } else {
        &[0]
    };
    let sos: &[u32] = if f.soff { &[0, 4, 20] } else { &[0] };
    let mut v = Vec::new();
    for &i in idxs {
        for &o in offs {
            for &s in sos {
                v.push([i, o, 0, s]);
            }
        }
    }
    v
}

fn form_addr(f: Form, c: [u32; 4]) -> u32 {
    let mut a = u32::from(f.imm);
    if f.offen {
        a = a.wrapping_add(c[1]);
    }
    if f.soff {
        a = a.wrapping_add(c[3]);
    }
    if f.idxen {
        a = a.wrapping_add(c[0].wrapping_mul(f.stride));
    }
    a
}

const VIEW_DWORDS: usize = 12;

fn buffer_load_row(name: &'static str, dwords: u32, cbv: bool) -> Outcome {
    let data = pattern(if cbv { 64 } else { VIEW_DWORDS });
    let mut parts = Vec::new();
    for &f in FORMS {
        if cbv && f.idxen && f.stride == 1 {
            continue;
        }
        let srv = if cbv { Srv::Cbv } else { Srv::Buffer(f.stride) };
        let mut body = Vec::new();
        body.extend(mubuf(
            name,
            Buf {
                vdata: 8,
                vaddr: if f.idxen { 1 } else { 2 },
                srsrc: 8,
                soffset: if f.soff { 3 } else { 128 },
                offset: f.imm,
                offen: f.offen,
                idxen: f.idxen,
                glc: false,
            },
        ));
        for k in 0..dwords {
            body.extend(store_out(8 + k as u8, 4 * k as u16));
        }
        let shader = match try_build(&body, &Cfg::new(srv)) {
            Ok(s) => s,
            Err(o) => {
                parts.push((format!("{f:?}"), o));
                continue;
            }
        };
        if !cbv {
            warm(&shader, 16);
        }
        let views = [
            if cbv { View::Cbv(&data) } else { View::Buffer { data: &data, stride: Stride::Raw } },
            View::RawUav(16),
        ];
        let o = sweep(&shader, &views, &[], &form_cases(f), |c, got| {
            let a = u64::from(form_addr(f, c));
            let want: Vec<u32> = (0..dwords).map(|k| dword_at(&data, a + 4 * u64::from(k))).collect();
            (got[0][..dwords as usize] != want[..]).then(|| format!("c {c:x?} addr {a:#x}: got {:x?} want {want:x?}", &got[0][..dwords as usize]))
        });
        parts.push((format!("{f:?}"), o));
    }
    merge(parts)
}

/// The loads that read a dword and more (`buffer_load_dword`, `x2`, `x3`, `x4`) from a raw view.
fn buffer_loads() -> Vec<(String, Outcome)> {
    [("buffer_load_dword", 1), ("buffer_load_dwordx2", 2), ("buffer_load_dwordx3", 3), ("buffer_load_dwordx4", 4)]
        .into_iter()
        .map(|(n, d)| (n.to_string(), buffer_load_row(n, d, false)))
        .collect()
}

/// `buffer_load_dword` on a constant buffer (an indexed array in a cbuffer).
fn buffer_loads_cbv() -> Vec<(String, Outcome)> {
    vec![("buffer_load_dword".to_string(), buffer_load_row("buffer_load_dword", 1, true))]
}

// ---- buffer_store_dword* ----

fn buffer_store_row(name: &'static str, dwords: u32) -> Outcome {
    let datas = [0xA0A0_0001u32, 0xB0B0_0002, 0xC0C0_0003, 0xD0D0_0004];
    let mut parts = Vec::new();
    for f in FORMS.iter().filter(|f| !f.idxen) {
        let mut body = Vec::new();
        for (k, &d) in datas.iter().enumerate() {
            body.extend(v_mov_lit(8 + k as u8, d));
        }
        body.extend(mubuf(
            name,
            Buf {
                vdata: 8,
                vaddr: 2,
                srsrc: 12,
                soffset: if f.soff { 3 } else { 128 },
                offset: f.imm,
                offen: f.offen,
                idxen: false,
                glc: false,
            },
        ));
        let shader = match try_build(&body, &Cfg::new(Srv::No)) {
            Ok(s) => s,
            Err(o) => {
                parts.push((format!("{f:?}"), o));
                continue;
            }
        };
        let o = sweep(&shader, &[View::RawUav(16)], &[], &form_cases(*f), |c, got| {
            let a = u64::from(form_addr(*f, c));
            let mut want = vec![0u32; 16];
            for k in 0..u64::from(dwords) {
                let at = a + 4 * k;
                // a write past the view is dropped, per dword
                if at % 4 == 0 && at + 4 <= 64 {
                    want[(at / 4) as usize] = datas[k as usize];
                }
            }
            (got[0][..16] != want[..]).then(|| format!("c {c:x?} addr {a:#x}: got {:x?} want {want:x?}", &got[0][..16]))
        });
        parts.push((format!("{f:?}"), o));
    }
    merge(parts)
}

fn buffer_stores() -> Vec<(String, Outcome)> {
    [("buffer_store_dword", 1), ("buffer_store_dwordx2", 2), ("buffer_store_dwordx3", 3), ("buffer_store_dwordx4", 4)]
        .into_iter()
        .map(|(n, d)| (n.to_string(), buffer_store_row(n, d)))
        .collect()
}

/// 16-bit buffer accesses: `buffer_load_ushort`, `buffer_store_short`.
fn buffer_short() -> Vec<(String, Outcome)> {
    let data = pattern(VIEW_DWORDS);
    let bytes: Vec<u8> = data.iter().flat_map(|d| d.to_le_bytes()).collect();
    let offs: Vec<u32> = (0..14).map(|k| 2 * k).chain([48, 50, 0x8000_0000]).collect();
    let cases: Vec<[u32; 4]> = offs.iter().map(|&o| [0, o, 0, 0]).collect();
    let load = {
        let mut body = mubuf(
            "buffer_load_ushort",
            Buf { vdata: 8, vaddr: 2, srsrc: 8, soffset: 128, offset: 0, offen: true, idxen: false, glc: false },
        )
        .to_vec();
        body.extend(store_out(8, 0));
        match try_build(&body, &Cfg::new(Srv::Buffer(0))) {
            Err(o) => o,
            Ok(shader) => {
                let views = [View::Buffer { data: &data, stride: Stride::Raw }, View::RawUav(4)];
                sweep(&shader, &views, &[], &cases, |c, got| {
                    let a = c[1] as usize;
                    // a 16-bit read inside the view; the console splits at a dword's end only
                    // for an odd address, which the program never makes
                    let want = if a + 2 <= bytes.len() { u32::from(bytes[a]) | u32::from(bytes[a + 1]) << 8 } else { 0 };
                    (got[0][0] != want).then(|| format!("c {c:x?}: got {:#x} want {want:#x}", got[0][0]))
                })
            }
        }
    };
    let store = {
        let mut body = v_mov_lit(8, 0xDEAD_BEEF).to_vec();
        body.extend(mubuf(
            "buffer_store_short",
            Buf { vdata: 8, vaddr: 2, srsrc: 12, soffset: 128, offset: 0, offen: true, idxen: false, glc: false },
        ));
        match try_build(&body, &Cfg::new(Srv::No)) {
            Err(o) => o,
            Ok(shader) => sweep(&shader, &[View::RawUav(16)], &[], &cases, |c, got| {
                let a = c[1] as usize;
                let mut want = vec![0u8; 64];
                if a + 2 <= 64 {
                    want[a] = 0xEF;
                    want[a + 1] = 0xBE;
                }
                let want: Vec<u32> = want.chunks(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
                (got[0][..16] != want[..]).then(|| format!("c {c:x?}: got {:x?} want {want:x?}", &got[0][..16]))
            }),
        }
    };
    vec![("buffer_load_ushort".to_string(), load), ("buffer_store_short".to_string(), store)]
}

// ---- typed buffers: buffer_load_format_*, buffer_store_format_* ----

/// `buffer_load_format_x/xy/xyz/xyzw` of an RGBA32F typed SRV by index (the translator takes this
/// form only: an offset on a typed load is refused, a shortcut).
fn typed_loads() -> Vec<(String, Outcome)> {
    let floats: Vec<u32> = (0..16u32).map(|i| (1.5f32 + i as f32 * 0.25).to_bits()).collect();
    let cases: Vec<[u32; 4]> = [0u32, 1, 2, 3, 4, 7, 1000].iter().map(|&i| [i, 0, 0, 0]).collect();
    [
        ("buffer_load_format_x", 1usize),
        ("buffer_load_format_xy", 2),
        ("buffer_load_format_xyz", 3),
        ("buffer_load_format_xyzw", 4),
    ]
    .into_iter()
    .map(|(name, n)| {
        let mut body = mubuf(
            name,
            Buf { vdata: 8, vaddr: 1, srsrc: 8, soffset: 128, offset: 0, offen: false, idxen: true, glc: false },
        )
        .to_vec();
        for k in 0..n {
            body.extend(store_out(8 + k as u8, 4 * k as u16));
        }
        let o = match try_build(&body, &Cfg::new(Srv::Buffer(4))) {
            Err(o) => o,
            Ok(shader) => {
                let views = [View::Buffer { data: &floats, stride: Stride::Typed }, View::RawUav(8)];
                sweep(&shader, &views, &[], &cases, |c, got| {
                    // elements of four floats; out of range: 0 (D3D12: a typed load out of range
                    // returns 0 in every channel)
                    let want: Vec<u32> = (0..n).map(|k| if c[0] < 4 { floats[4 * c[0] as usize + k] } else { 0 }).collect();
                    (got[0][..n] != want[..]).then(|| format!("index {}: got {:x?} want {want:x?}", c[0], &got[0][..n]))
                })
            }
        };
        (name.to_string(), o)
    })
    .collect()
}

/// A typed UAV of half4 elements written by `buffer_store_format_xyzw` and
/// `buffer_store_format_d16_xyzw` and read back by the matching loads, `buffer_load_format_d16_x`
/// and `buffer_load_format_d16_xyzw` (values that are exact halves).
fn typed_half_roundtrip() -> Vec<(String, Outcome)> {
    let halves: [f32; 4] = [1.0, -2.5, 0.5, 65504.0];
    let f16 = |x: f32| -> u32 {
        // exact for the values above
        let b = x.to_bits();
        let sign = (b >> 16) & 0x8000;
        let e = ((b >> 23) & 0xFF) as i32 - 127 + 15;
        sign | ((e as u32) << 10) | ((b >> 13) & 0x3FF)
    };
    let (h0, h1, h2, h3) = (f16(halves[0]), f16(halves[1]), f16(halves[2]), f16(halves[3]));
    let packed = [h0 | h1 << 16, h2 | h3 << 16];
    // (store, load, which registers the load fills, the dwords stored, the dwords loaded)
    let mut rows = Vec::new();
    for (store, load, nload, d16) in [
        ("buffer_store_format_xyzw", "buffer_load_format_xyzw", 4usize, false),
        ("buffer_store_format_d16_xyzw", "buffer_load_format_d16_xyzw", 2, true),
        ("buffer_store_format_d16_xyzw", "buffer_load_format_d16_x", 1, true),
    ] {
        let mut body = Vec::new();
        if d16 {
            body.extend(v_mov_lit(8, packed[0]));
            body.extend(v_mov_lit(9, packed[1]));
        } else {
            for (k, &x) in halves.iter().enumerate() {
                body.extend(v_mov_lit(8 + k as u8, x.to_bits()));
            }
        }
        let buf = Buf { vdata: 8, vaddr: 1, srsrc: 8, soffset: 128, offset: 0, offen: false, idxen: true, glc: false };
        body.extend(mubuf(store, buf));
        body.extend(mubuf(load, Buf { vdata: 12, ..buf }));
        for k in 0..nload {
            body.extend(store_out(12 + k as u8, 4 * k as u16));
        }
        let o = match try_build(&body, &Cfg::new(Srv::Uav)) {
            Err(o) => o,
            Ok(shader) => {
                let views = [View::HalfUav(2), View::RawUav(8)];
                // the UAV is the first view here (heap order): the program's UAV descriptor
                // is entry 1, the typed view entry 0
                sweep(&shader, &views, &[], &[[0, 0, 0, 0], [1, 0, 0, 0]], |c, got| {
                    // the readback is the second UAV in heap order
                    let out = &got[got.len() - 1];
                    let want: Vec<u32> = if d16 {
                        if nload == 2 { packed.to_vec() } else { vec![h0] }
                    } else {
                        halves.iter().map(|x| x.to_bits()).collect()
                    };
                    (out[..want.len()] != want[..]).then(|| format!("index {}: got {:x?} want {want:x?}", c[0], &out[..want.len()]))
                })
            }
        };
        rows.push((format!("{store}+{load}"), o));
    }
    rows
}

// ---- tbuffer_load_format_*: the buffer data formats ----

#[derive(Clone, Copy, PartialEq)]
enum Num {
    Unorm,
    Snorm,
    Uscaled,
    Sscaled,
    Uint,
    Sint,
    Float,
}

/// LLVM `UfmtGFX10` (the GFX10 unified buffer format numbers): channel widths and numeric type.
fn format_of(n: u8) -> Option<(&'static [u32], Num)> {
    use Num::*;
    const SIX: [Num; 6] = [Unorm, Snorm, Uscaled, Sscaled, Uint, Sint];
    let six = |bits: &'static [u32], first: u8| {
        let k = n.checked_sub(first)? as usize;
        (k < 6).then(|| (bits, SIX[k]))
    };
    match n {
        1..=6 => six(&[8], 1),
        7..=12 => six(&[16], 7),
        13 => Some((&[16], Float)),
        14..=19 => six(&[8, 8], 14),
        20 => Some((&[32], Uint)),
        21 => Some((&[32], Sint)),
        22 => Some((&[32], Float)),
        23..=28 => six(&[16, 16], 23),
        29 => Some((&[16, 16], Float)),
        50..=55 => six(&[10, 10, 10, 2], 50),
        56..=61 => six(&[8, 8, 8, 8], 56),
        62 => Some((&[32, 32], Uint)),
        63 => Some((&[32, 32], Sint)),
        64 => Some((&[32, 32], Float)),
        65..=70 => six(&[16, 16, 16, 16], 65),
        71 => Some((&[16, 16, 16, 16], Float)),
        72 => Some((&[32, 32, 32], Uint)),
        73 => Some((&[32, 32, 32], Sint)),
        74 => Some((&[32, 32, 32], Float)),
        75 => Some((&[32, 32, 32, 32], Uint)),
        76 => Some((&[32, 32, 32, 32], Sint)),
        77 => Some((&[32, 32, 32, 32], Float)),
        _ => None,
    }
}

fn half_to_f32(h: u32) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0f32 } else { 1.0 };
    let (e, m) = ((h >> 10) & 31, (h & 1023) as f32);
    match e {
        0 => sign * m * 2f32.powi(-24),
        31 if m == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        _ => sign * (1.0 + m / 1024.0) * 2f32.powi(e as i32 - 15),
    }
}

/// What a `tbuffer_load_format_*` of `channels` registers holds for the format at the bytes
/// `raw` (little endian, zeros past the view): the converted channels, missing ones 0 and the
/// alpha 1 (1.0 for the float kinds).
fn decode_format(n: u8, raw: &[u8], channels: usize) -> Vec<u32> {
    let (bits, num) = format_of(n).expect("a format");
    let mut word = 0u128;
    for (i, &b) in raw.iter().take(16).enumerate() {
        word |= u128::from(b) << (8 * i);
    }
    let mut out = Vec::new();
    let mut at = 0;
    for &w in bits {
        let field = ((word >> at) & ((1u128 << w) - 1)) as u64;
        at += w;
        let signed = ((field << (64 - w)) as i64) >> (64 - w);
        let max_u = ((1u64 << w) - 1) as f32;
        let max_s = ((1u64 << (w - 1)) - 1) as f32;
        out.push(match num {
            Num::Unorm => (field as f32 / max_u).to_bits(),
            Num::Snorm => (signed as f32 / max_s).max(-1.0).to_bits(),
            Num::Uscaled => (field as f32).to_bits(),
            Num::Sscaled => (signed as f32).to_bits(),
            Num::Uint => field as u32,
            Num::Sint => signed as u32,
            Num::Float if w == 32 => field as u32,
            Num::Float => half_to_f32(field as u32).to_bits(),
        });
    }
    let one = if matches!(num, Num::Uint | Num::Sint) { 1 } else { 1.0f32.to_bits() };
    while out.len() < channels {
        out.push(if out.len() == 3 { one } else { 0 });
    }
    out.truncate(channels);
    out
}

fn tbuffer_loads() -> Vec<(String, Outcome)> {
    let data = pattern(VIEW_DWORDS);
    let bytes: Vec<u8> = data.iter().flat_map(|d| d.to_le_bytes()).collect();
    // The formats the loads are tried with: one of each width and numeric type.
    let formats: &[u8] = &[1, 2, 5, 6, 7, 8, 11, 12, 13, 14, 18, 20, 22, 23, 29, 50, 52, 53, 56, 57, 60, 62, 64, 65, 66, 71, 72, 74, 75, 77];
    let mut rows = Vec::new();
    for (name, n) in [
        ("tbuffer_load_format_x", 1usize),
        ("tbuffer_load_format_xy", 2),
        ("tbuffer_load_format_xyz", 3),
        ("tbuffer_load_format_xyzw", 4),
    ] {
        let mut parts = Vec::new();
        for &fmt in formats {
            let (bits, _) = format_of(fmt).expect("a format");
            let width = (bits.iter().sum::<u32>() / 8) as u64;
            let mut body = mtbuf(
                name,
                Buf { vdata: 8, vaddr: 2, srsrc: 8, soffset: 128, offset: 0, offen: true, idxen: false, glc: false },
                fmt,
            )
            .to_vec();
            for k in 0..n {
                body.extend(store_out(8 + k as u8, 4 * k as u16));
            }
            let form = format!("format {fmt}");
            let shader = match try_build(&body, &Cfg::new(Srv::Buffer(0))) {
                Ok(s) => s,
                Err(o) => {
                    parts.push((form, o));
                    continue;
                }
            };
            // offsets: the format's own size apart (so the access is aligned as its element),
            // up to and past the end
            let step = width.min(4);
            let cases: Vec<[u32; 4]> =
                (0..=(4 * VIEW_DWORDS as u64 + 8)).step_by(step as usize).map(|o| [0, o as u32, 0, 0]).collect();
            warm(&shader, 8);
            let views = [View::Buffer { data: &data, stride: Stride::Raw }, View::RawUav(8)];
            let o = sweep(&shader, &views, &[], &cases, |c, got| {
                let off = c[1] as usize;
                // the element's bytes, past the view's end zeros (per dword: the dword holding
                // the element, or the dwords of a wide one)
                let dwords = if width < 4 { 1 } else { width as usize / 4 };
                let first = off & !3;
                let mut raw = vec![0u8; 16];
                for d in 0..dwords {
                    let at = first + 4 * d;
                    if at + 4 <= bytes.len() {
                        raw[4 * d..4 * d + 4].copy_from_slice(&bytes[at..at + 4]);
                    }
                }
                let shift = if width < 4 { off & 3 } else { 0 };
                let raw: Vec<u8> = raw[shift..].to_vec();
                let want = decode_format(fmt, &raw, n);
                let float = format_of(fmt).is_some_and(|(_, num)| num == Num::Float);
                let same = |g: u32, w: u32| g == w || float && f32::from_bits(g).is_nan() && f32::from_bits(w).is_nan();
                (!got[0][..n].iter().zip(&want).all(|(&g, &w)| same(g, w)))
                    .then(|| format!("offset {off}: got {:x?} want {want:x?}", &got[0][..n]))
            });
            parts.push((form, o));
        }
        rows.push((name.to_string(), merge(parts)));
    }
    rows
}

// ---- smem: s_buffer_load_dword*, from a constant buffer and from a raw SRV ----

fn s_buffer_row(name: &'static str, dwords: u32, cbv: bool) -> Outcome {
    let data = pattern(if cbv { 64 } else { 62 });
    let mut parts = Vec::new();
    // (immediate byte offset, soffset register used)
    for (imm, soff) in [(0u32, false), (4, false), (16, false), (0, true), (8, true), (252, false)] {
        let mut body = smem(name, 32, 8, imm, soff.then_some(3)).to_vec();
        for k in 0..dwords {
            body.push(vop1(0x001, 16 + k as u8, 32 + k as u16)); // v_mov_b32 v16+k, s32+k
        }
        for k in 0..dwords {
            body.extend(store_out(16 + k as u8, 4 * k as u16));
        }
        let mut cfg = Cfg::new(if cbv { Srv::Cbv } else { Srv::Buffer(4) });
        cfg.vgprs = 40;
        let shader = match try_build(&body, &cfg) {
            Ok(s) => s,
            Err(o) => {
                parts.push((format!("imm {imm} soff {soff}"), o));
                continue;
            }
        };
        let views = [
            if cbv { View::Cbv(&data) } else { View::Buffer { data: &data, stride: Stride::Raw } },
            View::RawUav(16),
        ];
        if !cbv {
            warm(&shader, 16);
        }
        let sos: &[u32] = if soff { &[0, 4, 8, 64, 200, 240, 252, 256, 400] } else { &[0] };
        let cases: Vec<[u32; 4]> = sos.iter().map(|&s| [0, 0, 0, s]).collect();
        let o = sweep(&shader, &views, &[], &cases, |c, got| {
            let a = u64::from(imm) + if soff { u64::from(c[3]) } else { 0 };
            let want: Vec<u32> = (0..dwords).map(|k| dword_at(&data, a + 4 * u64::from(k))).collect();
            (got[0][..dwords as usize] != want[..]).then(|| format!("soffset {}: got {:x?} want {want:x?}", c[3], &got[0][..dwords as usize]))
        });
        parts.push((format!("imm {imm} soff {soff}"), o));
    }
    merge(parts)
}

fn s_buffer_loads() -> Vec<(String, Outcome)> {
    let names = [
        ("s_buffer_load_dword", 1),
        ("s_buffer_load_dwordx2", 2),
        ("s_buffer_load_dwordx4", 4),
        ("s_buffer_load_dwordx8", 8),
        ("s_buffer_load_dwordx16", 16),
    ];
    names.into_iter().map(|(n, d)| (n.to_string(), s_buffer_row(n, d, true))).collect()
}

fn s_buffer_loads_raw() -> Vec<(String, Outcome)> {
    let names = [
        ("s_buffer_load_dword", 1),
        ("s_buffer_load_dwordx2", 2),
        ("s_buffer_load_dwordx4", 4),
        ("s_buffer_load_dwordx8", 8),
        ("s_buffer_load_dwordx16", 16),
    ];
    names.into_iter().map(|(n, d)| (format!("{n}"), s_buffer_row(n, d, false))).collect()
}

fn run_family_rows(family: &str, rows: Vec<(String, Outcome)>) {
    if Host::new(false).is_err() {
        eprintln!("{family}: no device; not run");
        return;
    }
    report_family(family, rows);
}

#[test]
fn oracle_mem_buffer_load() {
    let _chain = super::oracle_tests::metal_chain();
    if Host::new(false).is_err() {
        eprintln!("oracle_mem_buffer_load: no device; not run");
        return;
    }
    let mut rows = buffer_loads();
    rows.extend(buffer_short().into_iter().filter(|(n, _)| n == "buffer_load_ushort"));
    report_family("buffer_load", rows);
}

#[test]
fn oracle_mem_buffer_load_cbv() {
    let _chain = super::oracle_tests::metal_chain();
    run_family_rows("buffer_load_cbv", buffer_loads_cbv());
}

#[test]
fn oracle_mem_buffer_store() {
    let _chain = super::oracle_tests::metal_chain();
    if Host::new(false).is_err() {
        eprintln!("oracle_mem_buffer_store: no device; not run");
        return;
    }
    let mut rows = buffer_stores();
    rows.extend(buffer_short().into_iter().filter(|(n, _)| n == "buffer_store_short"));
    report_family("buffer_store", rows);
}

#[test]
fn oracle_mem_buffer_typed() {
    let _chain = super::oracle_tests::metal_chain();
    if Host::new(false).is_err() {
        eprintln!("oracle_mem_buffer_typed: no device; not run");
        return;
    }
    let mut rows = typed_loads();
    rows.extend(typed_half_roundtrip());
    report_family("buffer_typed", rows);
}

#[test]
fn oracle_mem_tbuffer_formats() {
    let _chain = super::oracle_tests::metal_chain();
    run_family_rows("tbuffer_formats", tbuffer_loads());
}

#[test]
fn oracle_mem_smem_cbv() {
    let _chain = super::oracle_tests::metal_chain();
    run_family_rows("smem_cbv", s_buffer_loads());
}

#[test]
fn oracle_mem_smem_srv() {
    let _chain = super::oracle_tests::metal_chain();
    run_family_rows("smem_srv", s_buffer_loads_raw());
}

/// The model's own check: the decoder agrees with the format table of the translator on the
/// widths (a table of formats cannot drift unnoticed).
#[test]
fn oracle_format_table_widths_are_the_gfx10_ones() {
    let _chain = super::oracle_tests::metal_chain();
    for (n, bits) in [(56u8, 32u32), (24, 32), (50, 32), (13, 16), (77, 128), (1, 8), (64, 64), (74, 96)] {
        let (b, _) = format_of(n).expect("a format");
        assert_eq!(b.iter().sum::<u32>(), bits, "format {n}");
    }
}

/// Probe (kept as a test of its own): the same straddling read as `buffer_load_dwordx2`,
/// `buffer_load_dwordx4`, `tbuffer_load_format_xy` (format 62) and `s_buffer_load_dwordx2` at the
/// view's last dword, after warming, repeated; prints what each returns for the dwords past the end.
#[test]
fn probe_straddle_past_the_view_end() {
    let _chain = super::oracle_tests::metal_chain();
    if Host::new(false).is_err() {
        return;
    }
    let data = pattern(VIEW_DWORDS);
    let buf = Buf { vdata: 8, vaddr: 2, srsrc: 8, soffset: 128, offset: 0, offen: true, idxen: false, glc: false };
    let progs: Vec<(&str, Vec<u32>, usize)> = vec![
        ("buffer_load_dwordx2", mubuf("buffer_load_dwordx2", buf).to_vec(), 2),
        ("buffer_load_dwordx4", mubuf("buffer_load_dwordx4", buf).to_vec(), 4),
        ("tbuffer_load_format_xy/62", mtbuf("tbuffer_load_format_xy", buf, 62).to_vec(), 2),
        ("tbuffer_load_format_xyzw/77", mtbuf("tbuffer_load_format_xyzw", buf, 77).to_vec(), 4),
    ];
    for (name, mut body, n) in progs {
        for k in 0..n {
            body.extend(store_out(8 + k as u8, 4 * k as u16));
        }
        let Ok(shader) = try_build(&body, &Cfg::new(Srv::Buffer(0))) else { continue };
        for round in 0..3 {
            warm(&shader, 8);
            let views = [View::Buffer { data: &data, stride: Stride::Raw }, View::RawUav(8)];
            let cases: Vec<[u32; 4]> = (0..60u32).map(|i| [0, 4 * (i % 15), 0, 0]).collect();
            let mut leaks = 0;
            let o = sweep(&shader, &views, &[], &cases, |c, got| {
                if c[1] == 44 && got[0][1] != 0 {
                    leaks += 1;
                }
                None
            });
            eprintln!("probe {name} round {round}: {leaks} of 4 straddling reads leaked");
            let _ = o;
        }
    }
}
