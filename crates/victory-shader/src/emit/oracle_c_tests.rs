//! mac S6, opcode oracle batch C (`docs/mac/opcode-oracle.md`): the mnemonics that need a draw.
//!
//! * `c_interp`: `v_interp_p1_f32`/`v_interp_p2_f32` (perspective center/centroid/sample and linear
//!   barycentrics), `v_interp_p1ll_f16`/`v_interp_p2_f16`, `v_interp_mov_f32` (P0/P10/P20).
//! * `c_export`: `exp mrt0` (every enable mask, 32-bit and compressed 16-bit pairs, `null`).
//! * `c_control`: the branch family (`s_branch`, `s_cbranch_vccz/vccnz/execz/execnz`, `s_swappc_b64`
//!   with `s_getpc_b64`/`s_setpc_b64`) and the hints (`s_nop`, `s_waitcnt`, `s_waitcnt_vscnt`,
//!   `s_clause`, `s_inst_prefetch`, `v_nop`) in pixel programs, whose result is a function of the
//!   pixel.
//!
//! One triangle with three known attribute vectors covers an 8x8 target (vertices (-1, 1), (-1, -3),
//! (3, 1), so pixel (col, row) has i = (row + 0.5) / 16 and j = (col + 0.5) / 16); a pixel program
//! under test exports its result to `mrt0`; every texel is compared with a CPU reference. The
//! vertex shader is a pass-through of a vertex buffer. Run: `tools/mac/shader-hw.sh` (the executor)
//! or on Windows the D3D12 host. `VICTORY_ORACLE_OUT=<file>` appends one line per row.

use super::container::{self, SigElement};
use super::host::{Draw, Host};
use super::{HostShader, emit};
use crate::dxbc::{Container, FourCC};
use crate::gfx10::opcode_of;
use crate::native::{HwStage, NativeProgram, PsInput, StageIo, Usage};
use crate::{ShaderError, io, ir, resource, structure};

const S_ENDPGM: u32 = 0xBF81_0000;
const SIZE: usize = 8;

// ---- encoders ----

const fn vop1(op: u16, vdst: u8, src0: u16) -> u32 {
    0x7E00_0000 | (vdst as u32) << 17 | (op as u32) << 9 | (src0 as u32 & 0x1FF)
}
const fn vop2(op: u16, vdst: u8, src0: u16, vsrc1: u8) -> u32 {
    (op as u32) << 25 | (vdst as u32) << 17 | (vsrc1 as u32) << 9 | (src0 as u32 & 0x1FF)
}
const fn vopc(op: u16, src0: u16, vsrc1: u8) -> u32 {
    0x7C00_0000 | (op as u32) << 17 | (vsrc1 as u32) << 9 | (src0 as u32 & 0x1FF)
}
const fn sop1(op: u16, sdst: u8, src0: u8) -> u32 {
    0xBE80_0000 | (sdst as u32) << 16 | (op as u32) << 8 | src0 as u32
}
const fn sop2(op: u16, sdst: u8, src0: u8, src1: u8) -> u32 {
    0x8000_0000 | (op as u32) << 23 | (sdst as u32) << 16 | (src1 as u32) << 8 | src0 as u32
}
const fn sopp(op: u16, imm: i16) -> u32 {
    0xBF80_0000 | (op as u32) << 16 | (imm as u16 as u32)
}
const fn vintrp(op: u32, vdst: u32, vsrc: u32, attr: u32, chan: u32) -> u32 {
    0xC800_0000 | vdst << 18 | op << 16 | attr << 10 | chan << 8 | vsrc
}
/// `exp` word 0 (`en` mask, `target`, `compr`, always `done` and `vm`) and word 1 (the four sources).
fn exp(target: u32, en: u32, compr: bool, v: [u32; 4]) -> [u32; 2] {
    [
        0xF800_0000 | en | target << 4 | u32::from(compr) << 10 | 1 << 11 | 1 << 12,
        v[0] | v[1] << 8 | v[2] << 16 | v[3] << 24,
    ]
}
/// SOPP opcodes (the oracle's name table has none): `gfx10/opcodes.rs` SOPP.
const S_NOP: u16 = 0;
const S_BRANCH: u16 = 2;
const S_CBRANCH_VCCZ: u16 = 6;
const S_CBRANCH_VCCNZ: u16 = 7;
const S_CBRANCH_EXECZ: u16 = 8;
const S_CBRANCH_EXECNZ: u16 = 9;
const S_WAITCNT: u16 = 0xC;
const S_SLEEP: u16 = 0xE;
const S_WAITCNT_DEPCTR: u16 = 0x23;
const S_INST_PREFETCH: u16 = 0x20;
const S_CLAUSE: u16 = 0x21;

fn op(name: &str) -> u16 {
    opcode_of(name).unwrap_or_else(|| panic!("{name} is not a GFX10 mnemonic")).1
}
const fn v(n: u16) -> u16 {
    256 + n
}
const LIT: u16 = 255;
const VCC_LO: u8 = 106;
const EXEC_LO: u8 = 126;
const C_ZERO: u16 = 128;
const C_HALF: u16 = 240;
const C_ONE: u16 = 242;
const C_TWO: u16 = 244;
const C_FOUR: u16 = 246;
const C_NEG_ONE: u16 = 243;
const C_NEG_TWO: u16 = 245;

/// `v_mov_b32 v<dst>, <src>` (a literal follows when `src` is `LIT`).
fn mov(dst: u8, src: u16) -> u32 {
    vop1(op("v_mov_b32"), dst, src)
}
fn lit(dst: u8, x: f32) -> [u32; 2] {
    [mov(dst, LIT), x.to_bits()]
}

// ---- the triangle ----

/// Three vertices (position, attribute), position as the 8x8 draw tests use.
const POSITIONS: [[f32; 4]; 3] = [[-1.0, 1.0, 0.5, 1.0], [-1.0, -3.0, 0.5, 1.0], [3.0, 1.0, 0.5, 1.0]];
const ATTRS: [[f32; 4]; 3] = [[2.0, -3.0, 0.5, 7.0], [6.0, 5.0, -1.0, 7.0], [10.0, 1.0, 4.0, -2.0]];

fn vertices() -> Vec<f32> {
    (0..3).flat_map(|k| POSITIONS[k].into_iter().chain(ATTRS[k])).collect()
}

fn element(name: &str, kind: u8, row: u32) -> SigElement {
    SigElement { name: name.into(), index: 0, kind, comp: 9, interp: 0, row: Some(row), col: 0, cols: 4, usage: 0, half: false }
}

/// A vertex shader that passes POSITION (row 0) and TEXCOORD (row 1) through to TEXCOORD0 (row 0)
/// and SV_Position (row 1).
fn vertex_shader() -> HostShader {
    use super::bitcode::{Attr, Term};
    use super::shader::Shader;
    let mut s = Shader::new(false);
    s.inputs = vec![element("POSITION", 0, 0), element("TEXCOORD", 0, 1)];
    for e in &mut s.inputs {
        e.usage = 15;
    }
    s.outputs = vec![element("TEXCOORD", 0, 0), element("SV_Position", 3, 1)];
    for (i, e) in s.outputs.iter_mut().enumerate() {
        e.usage = 15;
        e.interp = if i == 1 { 4 } else { 2 };
    }
    for row in 0..2 {
        for col in 0..4 {
            let args = [s.m.i32(row), s.m.i32(0), s.m.i8(col), s.m.undef(s.t.i32)];
            let x = s.op(s.entry, "loadInput.f32", Attr::ReadNone, s.t.f32, 4, &args);
            let args = [s.m.i32(1 - row), s.m.i32(0), s.m.i8(col), x];
            s.op(s.entry, "storeOutput.f32", Attr::NoUnwind, s.t.void, 5, &args);
        }
    }
    s.m.terminate(s.entry, Term::Ret);
    HostShader {
        stage: HwStage::Vs,
        dxil: s.finish(),
        user_data: Vec::new(),
        samplers: Vec::new(),
        static_samplers: Vec::new(),
        root_cbvs: Vec::new(),
        root_srvs: Vec::new(),
        root_uavs: Vec::new(),
        root_indexed_views: Vec::new(),
        vertex_reads: Vec::new(),
    }
}

/// Input address bits (`SPI_PS_INPUT_ADDR`).
const PERSP_SAMPLE: u32 = 1 << 0;
const PERSP_CENTER: u32 = 1 << 1;
const PERSP_CENTROID: u32 = 1 << 2;
const LINEAR_CENTER: u32 = 1 << 5;
const POS_X: u32 = 1 << 8;
const POS_Y: u32 = 1 << 9;

/// What the pixel program of a row sees of its pixel.
#[derive(Clone, Copy)]
struct Px {
    col: usize,
    row: usize,
    i: f32,
    j: f32,
}

impl Px {
    fn attr(&self, c: usize) -> f32 {
        let (p0, p1, p2) = (ATTRS[0][c], ATTRS[1][c], ATTRS[2][c]);
        p0 + self.i * (p1 - p0) + self.j * (p2 - p0)
    }
}

enum Want {
    /// Exactly these bits (any NaN equals any NaN).
    Bits(u32),
    /// Within an absolute and a relative tolerance.
    Near(f32, f32),
}

fn ok(want: &Want, got: f32) -> bool {
    match *want {
        Want::Bits(b) => got.to_bits() == b || (f32::from_bits(b).is_nan() && got.is_nan()),
        Want::Near(x, tol) => (got - x).abs() <= tol * (1.0 + x.abs()),
    }
}
fn shown(w: &Want) -> String {
    match *w {
        Want::Bits(b) => format!("{b:#010x} ({})", f32::from_bits(b)),
        Want::Near(x, tol) => format!("{x} +-{tol}"),
    }
}
fn exactly(x: f32) -> Want {
    Want::Bits(x.to_bits())
}
/// A colour export as the translator makes it by default (`VICTORY_EXPORT_F16_SAT`, on): NaN -> 0 and
/// the value clamped to +-65504 (lower.rs, sunclamp.rs).
fn exported(x: f32) -> Want {
    Want::Bits(if x.is_nan() { 0.0f32 } else { x.clamp(-65504.0, 65504.0) }.to_bits())
}

struct Case {
    /// The mnemonics the row names (the census rows it witnesses).
    names: String,
    words: Vec<u32>,
    input_addr: u32,
    fp16: bool,
    /// The host has no `attributeAtVertex` (what MSC 4.0.3 is): per-vertex reads go through the flat inputs and a geometry stage (S1).
    flat: bool,
    want: Box<dyn Fn(&Px) -> [Want; 4]>,
}

fn pixel_shader(c: &Case) -> Result<HostShader, ShaderError> {
    let native = NativeProgram {
        stage: HwStage::Ps,
        wave64: false,
        vgprs: 16,
        layout: vec![],
        used: vec![],
        io: StageIo::Ps {
            inputs: vec![PsInput {
                usage: Usage::Generic,
                row: 0,
                mask: 0b1111,
                attr: 0,
                flat: false,
                fp16: c.fp16,
                pack2: false,
                row2: 0,
            }],
            input_addr: c.input_addr,
            db_shader_control: 0,
            patches: vec![],
        },
        code: c.words.iter().flat_map(|w| w.to_le_bytes()).collect(),
    };
    let mut target = element("SV_Target", 16, 0);
    target.usage = 15;
    let mut uv = element("TEXCOORD", 0, 0);
    uv.half = c.fp16;
    let mut position = element("SV_Position", 3, 1);
    position.usage = 15;
    position.interp = 4;
    let isg1 = container::signature(&[uv, position], true);
    let osg1 = container::signature(&[target], false);
    let bytes = crate::dxbc::build(&[(FourCC::ISG1, &isg1), (FourCC::OSG1, &osg1)]);
    let container = Container::parse(&bytes)?;
    let p = ir::build(&native.patched_code(), native.wave64, native.vgprs)?;
    let res = resource::track(&p, &native.layout, native.stage, None)?;
    let io = io::raise(&p, &native)?;
    let st = structure::structure(&p)?;
    emit(&container, &native, &p, &st, &io, &res)
}

enum Outcome {
    Passed(usize),
    Mismatch { bad: usize, examples: Vec<String> },
    Untranslated(String),
    Refused(String),
}

fn panic_text(e: Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<String>()
        .cloned()
        .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_else(|| "panic".into())
}

fn run_case(host: &Host, c: &Case) -> Outcome {
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if c.flat { super::vertex_flat_tests::without(|| pixel_shader(c)) } else { pixel_shader(c) }
    }));
    let ps = match built {
        Ok(Ok(ps)) => ps,
        Ok(Err(e)) => return Outcome::Untranslated(e.to_string()),
        Err(e) => return Outcome::Untranslated(panic_text(e)),
    };
    let vs = vertex_shader();
    let made = if ps.vertex_reads.is_empty() {
        host.pipeline(&vs, &ps)
    } else {
        match super::vertex_flat::vertex_pipeline_gs(&vs.dxil, &ps.dxil, &ps.vertex_reads) {
            Err(e) => return Outcome::Untranslated(format!("the vertex hand-over stage: {e}")),
            Ok(gs) => host.root_signature(&vs, &ps).and_then(|root| {
                let pso = host.pipeline_with_gs(&root, &vs.dxil, Some(&gs), &ps.dxil)?;
                Ok((root, pso))
            }),
        }
    };
    let (root, pso) = match made {
        Err(e) if e.contains("unsupported") => return Outcome::Refused(e),
        Err(e) => return Outcome::Refused(format!("pipeline: {e}")),
        Ok(made) => made,
    };
    let vertices = vertices();
    let texels = match host.draw(
        &root,
        &pso,
        &Draw {
            roots: Vec::new(),
            textures: &[],
            samplers: &[],
            vertices: &vertices,
            stride: 32,
            size: (SIZE as u32, SIZE as u32),
            mesh_groups: 0,
            scratch: None,
        },
    ) {
        Ok(t) => t,
        Err(e) if e.contains("unsupported") => return Outcome::Refused(e),
        Err(e) => return Outcome::Mismatch { bad: 1, examples: vec![format!("draw: {e}")] },
    };
    if texels.len() != SIZE * SIZE {
        return Outcome::Mismatch { bad: 1, examples: vec![format!("expected {} texels; got {}", SIZE * SIZE, texels.len())] };
    }
    let (mut bad, mut examples) = (0, Vec::new());
    for (n, texel) in texels.iter().enumerate() {
        let (col, row) = (n % SIZE, n / SIZE);
        let px = Px { col, row, i: (row as f32 + 0.5) / 16.0, j: (col as f32 + 0.5) / 16.0 };
        let want = (c.want)(&px);
        for ch in 0..4 {
            if !ok(&want[ch], texel[ch]) {
                bad += 1;
                if examples.len() < 4 {
                    examples.push(format!("pixel ({col},{row}).{ch}: got {} ({:#010x}), want {}", texel[ch], texel[ch].to_bits(), shown(&want[ch])));
                }
            }
        }
    }
    if bad == 0 { Outcome::Passed(texels.len()) } else { Outcome::Mismatch { bad, examples } }
}

/// Differences that are known: (mnemonic, reason).
const KNOWN: &[(&str, &str)] = &[];

fn run_family(family: &str, cases: &[Case]) {
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(host) = Host::new(false) else {
        eprintln!("{family}: no device; not run");
        return;
    };
    let mut failed = Vec::new();
    for c in cases {
        let (outcome, n, detail) = match run_case(&host, c) {
            Outcome::Passed(n) => ("pass", n, String::new()),
            Outcome::Mismatch { bad, examples } => ("MISMATCH", bad, examples.join(" / ")),
            Outcome::Untranslated(t) => ("UNTRANSLATED", 0, t),
            Outcome::Refused(t) => ("refused", 0, t),
        };
        let names = c.names.clone();
        let known = KNOWN.iter().find(|(k, _)| c.names.starts_with(k));
        let tag = if known.is_some() { " (known)" } else { "" };
        eprintln!("oracle {family:10} {names:48} {outcome}{tag} {n} {detail}");
        if let Some(path) = std::env::var_os("VICTORY_ORACLE_OUT") {
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(file, "{family}\t{names}\t{outcome}{tag}\t{n}\t{detail}");
            }
        }
        let required = names.starts_with("s_swappc_b64") || names.starts_with("exp mrt0 compr");
        if (required && outcome != "pass") || ((outcome == "MISMATCH" || outcome == "UNTRANSLATED") && known.is_none()) {
            failed.push(format!("{names}: {detail}"));
        }
    }
    assert!(failed.is_empty(), "{family}: {} rows differ from the reference:\n{}", failed.len(), failed.join("\n"));
}

// ---- interpolation ----

fn exp_mrt0_v2() -> [u32; 2] {
    exp(0, 0xF, false, [2, 3, 4, 5])
}

/// The four channels of attribute 0 by the first and second half, results in v2..v5.
fn interp_pair() -> Vec<u32> {
    let mut w = Vec::new();
    for c in 0..4 {
        w.push(vintrp(0, 2 + c, 0, 0, c)); // v_interp_p1_f32 v<2+c>, v0, attr0.<c>
        w.push(vintrp(1, 2 + c, 1, 0, c)); // v_interp_p2_f32 v<2+c>, v1, attr0.<c>
    }
    w.extend(exp_mrt0_v2());
    w.push(S_ENDPGM);
    w
}

fn interpolated(tol: f32) -> Box<dyn Fn(&Px) -> [Want; 4]> {
    Box::new(move |p| [0, 1, 2, 3].map(|c| Want::Near(p.attr(c), tol)))
}

fn interp_cases() -> Vec<Case> {
    let mut cases = vec![
        Case {
            names: ["v_interp_p1_f32", "v_interp_p2_f32"].join("+"),
            words: interp_pair(),
            input_addr: PERSP_CENTER,
            fp16: false,
            flat: false,
            want: interpolated(2e-6),
        },
        Case {
            names: ["v_interp_p1_f32", "v_interp_p2_f32 (linear center)"].join("+"),
            words: interp_pair(),
            input_addr: LINEAR_CENTER,
            fp16: false,
            flat: false,
            want: interpolated(2e-6),
        },
        Case {
            names: ["v_interp_p1_f32", "v_interp_p2_f32 (persp centroid)"].join("+"),
            words: interp_pair(),
            input_addr: PERSP_CENTROID,
            fp16: false,
            flat: false,
            want: interpolated(2e-6),
        },
        Case {
            names: ["v_interp_p1_f32", "v_interp_p2_f32 (persp sample)"].join("+"),
            words: interp_pair(),
            input_addr: PERSP_SAMPLE,
            fp16: false,
            flat: false,
            want: interpolated(2e-6),
        },
    ];
    // The pair reads i and j from other registers when the program computed them: i' = i, j' = j via
    // a move through other registers (the translator must not tie the pair to v0 and v1).
    let mut moved = vec![mov(8, v(0)), mov(9, v(1))];
    for c in 0..4 {
        moved.push(vintrp(0, 2 + c, 8, 0, c));
        moved.push(vintrp(1, 2 + c, 9, 0, c));
    }
    moved.extend(exp_mrt0_v2());
    moved.push(S_ENDPGM);
    cases.push(Case {
        names: ["v_interp_p1_f32", "v_interp_p2_f32 (i, j moved to other registers)"].join("+"),
        words: moved,
        input_addr: PERSP_CENTER,
        fp16: false,
        flat: true,
        want: interpolated(2e-6),
    });
    // 16-bit attribute: p1ll (VOP3) into v2 as the first half, p2_f16 completing it into v3 (low
    // half), then the half converted to f32. Channels x and y.
    let mut halves = vec![mov(3, C_ZERO), mov(5, C_ZERO)];
    for (c, d) in [(0u32, 3u32), (1, 5)] {
        // v_interp_p1ll_f16 v2, v0, attr0.<c>
        halves.extend([0x35u32 << 26 | 0x342 << 16 | 2, c << 6 | u32::from(v(0)) << 9]);
        // v_interp_p2_f16 v<d>, attr0.<c>, v1, v2
        halves.extend([0x35u32 << 26 | 0x35a << 16 | d, c << 6 | u32::from(v(1)) << 9 | u32::from(v(2)) << 18]);
    }
    halves.push(vop1(op("v_cvt_f32_f16"), 6, v(3)));
    halves.push(vop1(op("v_cvt_f32_f16"), 7, v(5)));
    halves.extend(exp(0, 0xF, false, [6, 7, 6, 7]));
    halves.push(S_ENDPGM);
    cases.push(Case {
        names: ["v_interp_p1ll_f16", "v_interp_p2_f16"].join("+"),
        words: halves,
        input_addr: PERSP_CENTER,
        fp16: true,
        flat: true,
        want: Box::new(|p| [Want::Near(p.attr(0), 4e-3), Want::Near(p.attr(1), 4e-3), Want::Near(p.attr(0), 4e-3), Want::Near(p.attr(1), 4e-3)]),
    });
    // v_interp_mov_f32 P0: the provoking vertex's value of channels x..w.
    let mut p0 = Vec::new();
    for c in 0..4 {
        p0.push(vintrp(2, 2 + c, 2, 0, c));
    }
    p0.extend(exp_mrt0_v2());
    p0.push(S_ENDPGM);
    cases.push(Case {
        names: "v_interp_mov_f32 (p0)".into(),
        words: p0,
        input_addr: PERSP_CENTER,
        fp16: false,
        flat: true,
        want: Box::new(|_| ATTRS[0].map(exactly)),
    });
    // P10 and P20 beside an interpolated read of the same attribute: v2 = interpolated x, v3 = P0.x,
    // v4 = P10.x, v5 = P20.x.
    let mut p1020 = vec![vintrp(0, 2, 0, 0, 0), vintrp(1, 2, 1, 0, 0)];
    p1020.extend([vintrp(2, 3, 2, 0, 0), vintrp(2, 4, 0, 0, 0), vintrp(2, 5, 1, 0, 0)]);
    p1020.extend(exp_mrt0_v2());
    p1020.push(S_ENDPGM);
    cases.push(Case {
        names: "v_interp_mov_f32 (p10, p20)".into(),
        words: p1020,
        input_addr: PERSP_CENTER,
        fp16: false,
        flat: true,
        want: Box::new(|p| {
            [
                Want::Near(p.attr(0), 2e-6),
                exactly(ATTRS[0][0]),
                exactly(ATTRS[1][0] - ATTRS[0][0]),
                exactly(ATTRS[2][0] - ATTRS[0][0]),
            ]
        }),
    });
    cases
}

// ---- export ----

fn constant(r: [f32; 4]) -> Box<dyn Fn(&Px) -> [Want; 4]> {
    Box::new(move |_| r.map(exported))
}

fn export_cases() -> Vec<Case> {
    let values = [0.25f32, -2.0, 1.0e10, -0.0];
    let mut load = Vec::new();
    for (k, x) in values.iter().enumerate() {
        load.extend(lit(2 + k as u8, *x));
    }
    let mut cases = Vec::new();
    for en in [0xFu32, 0x1, 0x3, 0x7, 0x5, 0xA, 0x8, 0xC, 0x6] {
        let mut w = load.clone();
        w.extend(exp(0, en, false, [2, 3, 4, 5]));
        w.push(S_ENDPGM);
        let mut want = [0.0f32; 4];
        for c in 0..4 {
            if en >> c & 1 == 1 {
                want[c] = values[c];
            }
        }
        cases.push(Case {
            names: format!("exp mrt0 en={en:#x}"),
            words: w,
            input_addr: PERSP_CENTER,
            fp16: false,
            flat: false,
            want: constant(want),
        });
    }
    // Specials through the export: inf, -inf, NaN (quiet), the smallest normal.
    let specials = [f32::INFINITY, f32::NEG_INFINITY, f32::from_bits(0x7FC0_0000), f32::MIN_POSITIVE];
    let mut w = Vec::new();
    for (k, x) in specials.iter().enumerate() {
        w.extend(lit(2 + k as u8, *x));
    }
    w.extend(exp(0, 0xF, false, [2, 3, 4, 5]));
    w.push(S_ENDPGM);
    cases.push(Case {
        names: ["exp mrt0 (inf, -inf, NaN, min normal)"].join("+"),
        words: w,
        input_addr: PERSP_CENTER,
        fp16: false,
        flat: false,
        want: constant(specials),
    });
    // A value the program computed per pixel, through p1/p2 and an add.
    let mut w = interp_pair();
    w.truncate(w.len() - 3);
    w.push(vop2(op("v_add_f32"), 2, v(2), 3));
    w.extend(exp(0, 0x3, false, [2, 2, 2, 2]));
    w.push(S_ENDPGM);
    cases.push(Case {
        names: ["exp mrt0 en=0x3 (computed)"].join("+"),
        words: w,
        input_addr: PERSP_CENTER,
        fp16: false,
        flat: false,
        want: Box::new(|p| [Want::Near(p.attr(0) + p.attr(1), 2e-6), Want::Near(p.attr(0) + p.attr(1), 2e-6), exactly(0.0), exactly(0.0)]),
    });
    // Compressed: v_cvt_pkrtz_f16_f32 v2, 1.0, -2.0; v3 = (0.5, 4.0); exp mrt0 v2, v3 compr.
    let pk = op("v_cvt_pkrtz_f16_f32");
    for (name, en) in [("exp mrt0 compr en=0xf", 0xFu32), ("exp mrt0 compr en=0x3", 0x3), ("exp mrt0 compr en=0xc", 0xC)] {
        let mut w = Vec::new();
        w.extend(lit(8, -2.0));
        w.extend(lit(9, 4.0));
        w.push(vop2(pk, 2, C_ONE, 8)); // v2 = pk(1.0, -2.0)
        w.push(vop2(pk, 3, C_HALF, 9)); // v3 = pk(0.5, 4.0)
        w.extend(exp(0, en, true, [2, 3, 0, 0]));
        w.push(S_ENDPGM);
        let all = [1.0f32, -2.0, 0.5, 4.0];
        let mut want = [0.0f32; 4];
        for (c, x) in all.iter().enumerate() {
            if en >> c & 1 == 1 {
                want[c] = *x;
            }
        }
        cases.push(Case {
            names: format!("{name}+v_cvt_pkrtz_f16_f32"),
            words: w,
            input_addr: PERSP_CENTER,
            fp16: false,
            flat: false,
            want: constant(want),
        });
    }
    // exp null beside mrt0: the null export changes nothing.
    let mut w = load.clone();
    w.extend(exp(9, 0xF, false, [2, 3, 4, 5]));
    w.extend(exp(0, 0xF, false, [2, 3, 4, 5]));
    w.push(S_ENDPGM);
    cases.push(Case {
        names: ["exp null + exp mrt0"].join("+"),
        words: w,
        input_addr: PERSP_CENTER,
        fp16: false,
        flat: false,
        want: constant(values),
    });
    cases
}

// ---- control flow and hints ----

/// The pixel program `body` exporting v6 to mrt0 (all channels). v2 and v3 are the pixel's x, y.
fn exporting(mut body: Vec<u32>) -> Vec<u32> {
    body.extend(exp(0, 0xF, false, [6, 6, 6, 6]));
    body.push(S_ENDPGM);
    body
}

fn scalar_result(f: impl Fn(&Px) -> f32 + 'static) -> Box<dyn Fn(&Px) -> [Want; 4]> {
    Box::new(move |p| {
        let x = f(p);
        [x, x, x, x].map(exactly)
    })
}

fn control_case(names: &[&str], body: Vec<u32>, f: impl Fn(&Px) -> f32 + 'static) -> Case {
    Case { names: names.join("+"), words: exporting(body), input_addr: PERSP_CENTER | POS_X | POS_Y, fp16: false, flat: false, want: scalar_result(f) }
}

fn control_cases() -> Vec<Case> {
    let cmp_gt = op("v_cmp_gt_f32");
    let saveexec = op("s_and_saveexec_b32");
    let s_mov = op("s_mov_b32");
    let s_xor = op("s_xor_b32");
    let s_or = op("s_or_b32");
    let s_and = op("s_and_b32");
    let add_f32 = op("v_add_f32");
    let cvt_u32 = op("v_cvt_u32_f32");
    let and_b32 = op("v_and_b32");
    let add_nc = op("v_add_nc_u32");
    let cmp_gt_u32 = op("v_cmp_gt_u32");
    let (branch, vccz, vccnz, execz, execnz) = (S_BRANCH, S_CBRANCH_VCCZ, S_CBRANCH_VCCNZ, S_CBRANCH_EXECZ, S_CBRANCH_EXECNZ);
    let mut cases = Vec::new();

    // s_branch over one instruction.
    cases.push(control_case(
        &["s_branch"],
        vec![mov(6, C_ONE), sopp(branch, 1), mov(6, C_FOUR)],
        |_| 1.0,
    ));
    // s_cbranch_vccnz taken (every lane's compare is true), skipping a write.
    cases.push(control_case(
        &["s_cbranch_vccnz"],
        vec![mov(6, C_ONE), vopc(cmp_gt, LIT, 2), 100.0f32.to_bits(), sopp(vccnz, 1), mov(6, C_FOUR)],
        |_| 1.0,
    ));
    // s_cbranch_vccnz not taken (no lane's compare is true): the write happens.
    cases.push(control_case(
        &["s_cbranch_vccnz (not taken)"],
        vec![mov(6, C_ONE), vopc(cmp_gt, C_ZERO, 2), sopp(vccnz, 1), mov(6, C_FOUR)],
        |_| 4.0,
    ));
    // s_cbranch_vccz taken (no lane's compare is true).
    cases.push(control_case(
        &["s_cbranch_vccz"],
        vec![mov(6, C_ONE), vopc(cmp_gt, C_ZERO, 2), sopp(vccz, 1), mov(6, C_FOUR)],
        |_| 1.0,
    ));
    // s_cbranch_vccz not taken (all lanes true): the write happens.
    cases.push(control_case(
        &["s_cbranch_vccz (not taken)"],
        vec![mov(6, C_ONE), vopc(cmp_gt, LIT, 2), 100.0f32.to_bits(), sopp(vccz, 1), mov(6, C_FOUR)],
        |_| 4.0,
    ));
    // s_cbranch_vccnz as a loop's back edge: three trips, the same in every lane.
    cases.push(control_case(
        &["s_cbranch_vccnz (loop)"],
        vec![
            mov(6, C_ZERO),
            mov(7, C_ZERO),
            // loop: v6 += 1.0; v7 += 1; vcc = 3 > v7 (unsigned); branch back while any lane's is true
            vop2(add_f32, 6, C_ONE, 6),
            vop2(add_nc, 7, 129, 7),
            vopc(cmp_gt_u32, 131, 7),
            sopp(vccnz, -4),
        ],
        |_| 3.0,
    ));
    // Divergent if with s_cbranch_execz (the body is skipped by a wave none of whose lanes pass).
    let divergent_if = vec![
        mov(6, C_ZERO),
        vopc(cmp_gt, C_FOUR, 2), // 4.0 > x
        sop1(saveexec, 0, VCC_LO),
        sopp(execz, 1),
        mov(6, C_ONE),
        sop2(s_or, EXEC_LO, EXEC_LO, 0),
    ];
    cases.push(control_case(&["s_cbranch_execz", "s_and_saveexec_b32"], divergent_if, |p| if (p.col as f32 + 0.5) < 4.0 { 1.0 } else { 0.0 }));
    // if / else: both sides by execz.
    let if_else = vec![
        vopc(cmp_gt, C_FOUR, 2),
        sop1(saveexec, 0, VCC_LO),                // s0 = exec, exec &= cond
        sop2(s_xor, 1, EXEC_LO, 0),               // s1 = else mask
        sopp(execz, 1),
        mov(6, C_ONE),
        sop1(s_mov, EXEC_LO, 1),                  // exec = else mask
        sopp(execz, 1),
        mov(6, C_TWO),
        sop1(s_mov, EXEC_LO, 0),                  // exec = s0
    ];
    cases.push(control_case(&["s_cbranch_execz (if/else)"], if_else, |p| if (p.col as f32 + 0.5) < 4.0 { 1.0 } else { 2.0 }));
    // A divergent loop with s_cbranch_execnz: the lanes leave one by one, trip count max(1, col & 3).
    // v_cmp_gt_u32 vcc, v8, v7 has a VGPR src0: encoded v(8) in src0, vsrc1 = 7: v8 > v7.
    let looped = vec![
        mov(6, C_ZERO),
        mov(7, C_ZERO),
        vop1(cvt_u32, 8, v(2)),
        vop2(and_b32, 8, 131, 8),
        sop1(s_mov, 2, EXEC_LO),
        vop2(add_f32, 6, C_ONE, 6),
        vop2(add_nc, 7, 129, 7),
        vopc(cmp_gt_u32, v(8), 7),
        sop2(s_and, EXEC_LO, EXEC_LO, VCC_LO),
        sopp(execnz, -5),
        sop1(s_mov, EXEC_LO, 2),
    ];
    cases.push(control_case(&["s_cbranch_execnz"], looped, |p| ((p.col & 3).max(1)) as f32));
    // A call: s_getpc_b64, s_add_u32 (literal), s_addc_u32, s_swappc_b64, the callee's s_setpc_b64.
    let (getpc, setpc, swappc) = (op("s_getpc_b64"), op("s_setpc_b64"), op("s_swappc_b64"));
    let (add_u32, addc_u32) = (op("s_add_u32"), op("s_addc_u32"));
    let call = vec![
        mov(6, C_ONE),
        sop1(getpc, 4, 0),
        sop2(add_u32, 4, 4, 255),
        24,                       // the literal: bytes from here to the callee
        sop2(addc_u32, 5, 5, 128),
        sop1(swappc, 2, 4),
        vop2(add_f32, 6, C_ONE, 6), // after the return: v6 = 4 + 1
        sopp(branch, 2),
        mov(6, C_FOUR),           // the callee
        sop1(setpc, 0, 2),
    ];
    cases.push(control_case(&["s_swappc_b64", "s_getpc_b64", "s_setpc_b64"], call, |_| 5.0));
    // The hints change nothing.
    let hints = vec![
        sopp(S_NOP, 3),
        mov(6, C_ONE),
        sopp(S_WAITCNT, 0),
        0xBBFD_0000, // s_waitcnt_vscnt null, 0
        sopp(S_INST_PREFETCH, 3),
        sopp(S_CLAUSE, 1),
        vop2(add_f32, 6, C_ONE, 6),
        vop2(add_f32, 6, C_ONE, 6),
        vop1(op("v_nop"), 0, 0),
        sopp(S_WAITCNT, 0x3F70),
        sopp(S_SLEEP, 1),
        sopp(S_WAITCNT_DEPCTR, 0xFFE3u16 as i16),
    ];
    cases.push(control_case(
        &["s_nop", "s_waitcnt", "s_waitcnt_vscnt", "s_inst_prefetch", "s_clause", "v_nop", "s_sleep", "s_waitcnt_depctr"],
        hints,
        |_| 3.0,
    ));
    cases
}

fn family_run(name: &str, cases: Vec<Case>) {
    run_family(name, &cases);
}

#[test]
fn oracle_c_interp() {
    family_run("c_interp", interp_cases());
}

#[test]
fn oracle_c_export() {
    family_run("c_export", export_cases());
}

#[test]
fn oracle_c_control() {
    family_run("c_control", control_cases());
}

/// Every mnemonic a row names is a GFX10 mnemonic (the table cannot drift).
#[test]
fn oracle_c_rows_name_gfx10_mnemonics() {
    for c in interp_cases().iter().chain(&export_cases()).chain(&control_cases()) {
        for name in c.names.split('+') {
            let base = name.split([' ', '(']).next().unwrap();
            let sopp = ["s_branch", "s_cbranch_vccz", "s_cbranch_vccnz", "s_cbranch_execz", "s_cbranch_execnz", "s_nop", "s_waitcnt", "s_inst_prefetch", "s_clause", "s_sleep", "s_waitcnt_depctr"];
            // (VINTRP names are in none of `opcode_of`'s tables either.)
            if (base.starts_with("v_") || base.starts_with("s_")) && !sopp.contains(&base) && !base.starts_with("v_interp_p") && base != "v_interp_mov_f32" {
                assert!(opcode_of(base).is_some(), "{base} is not a GFX10 mnemonic");
            }
        }
    }
}
