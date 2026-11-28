//! mac S6, A batch of the opcode oracle (`inv.isa.opcode_oracle`): the VALU remainder, the
//! exec-writing and 64-bit scalar instructions, and the transcendentals the first ten families
//! left (`docs/mac/opcode-oracle.md`, the plan's batches 1 to 3).
//!
//! Same method as `oracle_tests.rs` (a row names a mnemonic, a CPU reference, an input family),
//! with two additions:
//! * rows with their own program (`Spec`): the instruction needs a literal, a register pair, a
//!   VOP3P or VOP3B encoding, an `M0`, a carry in `vcc`, or one result of two (`#hi`, `#carry`);
//!   they run on `build_single` (16 VGPRs) and several rows may share a mnemonic (`[imm]` tags);
//! * the lane family (`LaneRow`): 32 threads in one wave, the exec mask set from a root constant,
//!   a different value per lane, every lane's result read back: `v_cmpx_*`, the saveexec and
//!   wrexec forms, `v_mbcnt`, `v_writelane_b32`, `v_permlanex16_b32`.
//!
//! References: the RDNA2 ISA text (chapters 6 to 12), Mesa's ACO/LLVM semantics where the text
//! is silent, and the OpenGL cube map face table for the `v_cube*` family; each row says where.
//! A `v_cvt_pk_u8_f32` rounds to nearest even and saturates (the ISA leaves `flt32_to_uint8`
//! undefined; third-party measurements on RDNA2 agree).

use super::*;
use crate::emit::valu_tests::{build_words, s_mov_b32};

// ---- encoders ----

const VCC_LO: u8 = 106;
const M0: u8 = 124;
const EXEC_LO: u8 = 126;

fn o(name: &str) -> u16 {
    opcode_of(name).unwrap_or_else(|| panic!("{name} is not a GFX10 mnemonic")).1
}
const fn sopk(op: u16, sdst: u8, imm: u16) -> u32 {
    0xB000_0000 | (op as u32) << 23 | (sdst as u32) << 16 | imm as u32
}
/// VOP3B (`v_add_co_u32`, `v_mad_u64_u32`): `sdst` receives the carry mask.
const fn vop3b(op: u16, vdst: u8, sdst: u8, s0: u16, s1: u16, s2: u16) -> [u32; 2] {
    [
        0x35 << 26 | (op as u32 & 0x3FF) << 16 | (sdst as u32 & 0x7F) << 8 | vdst as u32,
        (s0 as u32 & 0x1FF) | (s1 as u32 & 0x1FF) << 9 | (s2 as u32 & 0x1FF) << 18,
    ]
}
/// VOP3P: `opsel` and `opsel_hi` pick the halves (for the `v_fma_mix_*` the `opsel_hi` bit says
/// "this source is a half"), `neg` flips signs, `neg_hi` is the absolute value of the mix forms.
fn vop3p(op: u16, vdst: u8, s: [u16; 3], opsel: u8, opsel_hi: u8, neg: u8, neg_hi: u8) -> [u32; 2] {
    [
        0x33 << 26
            | (op as u32 & 0x7F) << 16
            | u32::from(opsel_hi >> 2 & 1) << 14
            | u32::from(opsel & 7) << 11
            | u32::from(neg_hi & 7) << 8
            | vdst as u32,
        (s[0] as u32 & 0x1FF)
            | (s[1] as u32 & 0x1FF) << 9
            | (s[2] as u32 & 0x1FF) << 18
            | u32::from(opsel_hi & 3) << 27
            | u32::from(neg & 7) << 29,
    ]
}
/// `s_load_dwordx4 s[dst:dst+3], s[base:base+1], 0`.
const fn s_load_dwordx4(dst: u8, base: u8) -> [u32; 2] {
    [0x3D << 26 | 0x02 << 18 | (dst as u32) << 6 | (base as u32 >> 1), 125 << 25]
}
/// `buffer_store_dword vdata, vaddr, s[srsrc:srsrc+3], 0 offen offset:byte_off`.
const fn store_offen(vdata: u8, vaddr: u8, srsrc: u8, byte_off: u16) -> [u32; 2] {
    [
        0x38 << 26 | 0x1C << 18 | 1 << 12 | (byte_off as u32 & 0xFFF),
        0x80 << 24 | ((srsrc as u32 >> 2) & 0x1F) << 16 | (vdata as u32) << 8 | vaddr as u32,
    ]
}
const fn store_dword(vdata: u8, srsrc: u8, byte_off: u16) -> [u32; 2] {
    [
        0x38 << 26 | 0x1C << 18 | (byte_off as u32 & 0xFFF),
        0x80 << 24 | ((srsrc as u32 >> 2) & 0x1F) << 16 | (vdata as u32) << 8,
    ]
}
const S_ENDPGM: u32 = 0xBF81_0000;

/// A one-thread program with 16 VGPRs: `v1..v4` hold `s0..s3`, `body` leaves its result in `v0`.
pub(super) fn build_single(body: &[u32]) -> HostShader {
    let mut w = vec![s_mov_b32(5, 0x80)];
    w.extend_from_slice(&s_load_dwordx4(8, 4));
    for i in 0..4u8 {
        w.push(vop1(1, i + 1, u16::from(i)));
    }
    w.extend_from_slice(body);
    w.extend_from_slice(&store_dword(0, 8, 0));
    w.push(S_ENDPGM);
    build_words(&w, 1, 0, false, 16, 4)
}

/// `body`, then `v0 = vreg`.
fn prog(mut body: Vec<u32>, out: u8) -> Vec<u32> {
    body.push(vop1(1, 0, 256 + u16::from(out)));
    body
}
fn smov_to_v(sreg: u8, vreg: u8) -> u32 {
    vop1(1, vreg, u16::from(sreg))
}

// ---- rows with their own program ----

pub(super) struct Spec {
    row: Row,
    tag: String,
    words: Vec<u32>,
}
fn sp(row: Row, tag: impl Into<String>, words: Vec<u32>) -> Spec {
    Spec { row, tag: tag.into(), words }
}

/// Like `fl`, with a closure (a row that captures its literal) and an input family.
fn flc(name: &'static str, ty: Ty, args: usize, ulps: u32, abs: f32, g: impl Fn(&[f32; 3]) -> Vec<f32> + 'static) -> Row {
    Row {
        name,
        ty,
        args,
        want: Box::new(move |i| {
            let raw = [f(i[0]), f(i[1]), f(i[2])];
            let fl = raw.map(flushed);
            let mut cands = g(&raw);
            cands.extend(g(&fl));
            Want::Float { cands, ulps, abs }
        }),
    }
}
fn bitsrow(name: &'static str, ty: Ty, args: usize, g: impl Fn(&[u32; 4]) -> u32 + 'static) -> Row {
    Row { name, ty, args, want: Box::new(move |i| Want::Bits(vec![g(i)])) }
}
fn both_zeros(v: f32) -> Vec<f32> {
    if v == 0.0 { vec![0.0, -0.0] } else { vec![v] }
}

// ---- half helpers ----

fn hv(x: u32) -> f64 {
    f64::from(from_half(x & 0xffff))
}
fn hflush(x: f64) -> f64 {
    if x != 0.0 && x.abs() < 2f64.powi(-14) { 0.0f64.copysign(x) } else { x }
}
fn pick(word: u32, hi: bool) -> u32 {
    if hi { word >> 16 } else { word & 0xffff }
}
/// A half-precision function of up to three half inputs (the low halves).
fn hrow(name: &'static str, args: usize, g: fn(&[f64; 3]) -> f64) -> Row {
    Row {
        name,
        ty: Ty::F16,
        args,
        want: Box::new(move |i| {
            let raw = [hv(i[0]), hv(i[1]), hv(i[2])];
            let fl = raw.map(hflush);
            Want::Half(vec![to_half64(g(&raw), false), to_half64(g(&fl), false)])
        }),
    }
}
fn cmph(name: &'static str, g: fn(f64, f64) -> bool) -> Row {
    Row {
        name,
        ty: Ty::F16,
        args: 2,
        want: Box::new(move |i| {
            let (a, b) = (hv(i[0]), hv(i[1]));
            let mut v = vec![u32::from(g(a, b)), u32::from(g(hflush(a), hflush(b)))];
            v.dedup();
            Want::Bits(v)
        }),
    }
}
/// The ISA's float minimum or maximum: a NaN gives the other operand (both: NaN), -0 is below +0
/// (a zero result may come with either sign here).
fn minmax64(a: f64, b: f64, min: bool) -> Vec<f64> {
    if a.is_nan() || b.is_nan() {
        return vec![f64::NAN, if a.is_nan() { b } else { a }];
    }
    if a == 0.0 && b == 0.0 {
        return vec![0.0, -0.0];
    }
    vec![if min { a.min(b) } else { a.max(b) }]
}
fn med3(x: &[f64; 3]) -> Vec<f64> {
    let nans = x.iter().filter(|v| v.is_nan()).count();
    if nans == 3 {
        return vec![f64::NAN];
    }
    if nans > 0 {
        // RDNA2 ISA: any NaN: v_min3 of the three, a NaN operand ignored by v_min
        let m = x.iter().copied().filter(|v| !v.is_nan()).fold(f64::INFINITY, f64::min);
        return if m == 0.0 { vec![0.0, -0.0] } else { vec![m] };
    }
    let mut v = *x;
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if v[1] == 0.0 { vec![0.0, -0.0] } else { vec![v[1]] }
}

// ---- input families ----

fn half_specials() -> Vec<u32> {
    vec![
        0x0000, 0x8000, 0x3c00, 0xbc00, 0x3800, 0xb800, 0x4000, 0x4300, 0xc740, 0x7bff, 0xfbff, 0x7c00, 0xfc00, 0x7e00, 0x7c01,
        0x0001, 0x03ff, 0x0400, 0x8001, 0x3555, 0x5640, 0x2e66,
    ]
}
fn random_half(r: &mut Rng) -> u32 {
    (r.next() & 1) << 15 | (1 + r.next() % 29) << 10 | (r.next() & 0x3ff)
}
fn u64_specials() -> Vec<u64> {
    vec![
        0,
        1,
        2,
        63,
        64,
        0xffff_ffff,
        0x1_0000_0000,
        0x7fff_ffff_ffff_ffff,
        0x8000_0000_0000_0000,
        u64::MAX,
        0xdead_beef_1234_5678,
        0x0000_0001_0000_0001,
        0x5555_5555_5555_5555,
        0xaaaa_aaaa_aaaa_aaaa,
        0xffff_ffff_0000_0000,
    ]
}
fn mixed_word(r: &mut Rng) -> u32 {
    let (fs, hs) = (float_specials(), half_specials());
    match r.next() % 5 {
        0 => fs[r.next() as usize % fs.len()],
        1 => hs[r.next() as usize % hs.len()] << 16 | hs[r.next() as usize % hs.len()],
        2 => random_half(r) << 16 | random_half(r),
        3 => random_float(r),
        _ => r.next() ^ (r.next() << 16),
    }
}
fn finite_specials() -> Vec<u32> {
    float_specials().into_iter().filter(|&b| f32::from_bits(b).is_finite()).collect()
}
fn double_bits(x: f64) -> [u32; 2] {
    [x.to_bits() as u32, (x.to_bits() >> 32) as u32]
}

pub(super) fn cases(ty: Ty, args: usize) -> Vec<[u32; 4]> {
    let mut r = Rng(0xC0FF_EE12_3456_789B);
    let mut out = Vec::new();
    match ty {
        Ty::F16 => {
            let hs = half_specials();
            let garbled = |h: u32, r: &mut Rng| (r.next() & 0xffff_0000) | h;
            let any = |r: &mut Rng| if r.next() % 3 == 0 { hs[r.next() as usize % hs.len()] } else { random_half(r) };
            match args {
                1 => {
                    for &a in &hs {
                        out.push([garbled(a, &mut r), 0, 0, 0]);
                    }
                    for _ in 0..150 {
                        let a = any(&mut r);
                        out.push([garbled(a, &mut r), 0, 0, 0]);
                    }
                }
                2 => {
                    for &a in &hs {
                        for &b in &hs {
                            out.push([garbled(a, &mut r), garbled(b, &mut r), 0, 0]);
                        }
                    }
                    for _ in 0..150 {
                        let (a, b) = (any(&mut r), any(&mut r));
                        out.push([garbled(a, &mut r), garbled(b, &mut r), 0, 0]);
                    }
                }
                _ => {
                    for _ in 0..450 {
                        let (a, b, c) = (any(&mut r), any(&mut r), any(&mut r));
                        out.push([garbled(a, &mut r), garbled(b, &mut r), garbled(c, &mut r), 0]);
                    }
                }
            }
        }
        Ty::Mix => {
            let n = match args {
                1 => 300,
                2 => 450,
                _ => 600,
            };
            for _ in 0..n {
                out.push([mixed_word(&mut r), mixed_word(&mut r), mixed_word(&mut r), mixed_word(&mut r)]);
            }
        }
        Ty::U64 => {
            let sp = u64_specials();
            let split = |v: u64| [v as u32, (v >> 32) as u32];
            if args == 1 {
                for &a in &sp {
                    out.push([split(a)[0], split(a)[1], 0, 0]);
                }
                for _ in 0..150 {
                    let v = u64::from(r.next()) << 32 | u64::from(r.next());
                    out.push([split(v)[0], split(v)[1], 0, 0]);
                }
            } else {
                for &a in &sp {
                    for &b in &sp {
                        out.push([split(a)[0], split(a)[1], split(b)[0], split(b)[1]]);
                    }
                }
                for _ in 0..150 {
                    let (a, b) = (u64::from(r.next()) << 32 | u64::from(r.next()), u64::from(r.next()) << 32 | u64::from(r.next()));
                    out.push([split(a)[0], split(a)[1], split(b)[0], split(b)[1]]);
                }
            }
        }
        Ty::Quad => {
            let sp = int_specials();
            for _ in 0..450 {
                let mut q = [0u32; 4];
                for x in &mut q {
                    *x = if r.next() % 2 == 0 { sp[r.next() as usize % sp.len()] } else { r.next() };
                }
                out.push(q);
            }
        }
        Ty::Mad => {
            for _ in 0..600 {
                // exponents -20..20: the product stays normal, its rounding error too
                let m = |r: &mut Rng| f32::from_bits((r.next() & 0x8000_0000) | ((107 + r.next() % 41) << 23) | (r.next() & 0x7f_ffff));
                let (a, b) = (m(&mut r), m(&mut r));
                let p = a * b;
                let c = match r.next() % 4 {
                    0 | 1 => -p,
                    2 => f32::from_bits((-p).to_bits().wrapping_add(1)),
                    _ => f32::from_bits((-p).to_bits().wrapping_sub(1)),
                };
                out.push([a.to_bits(), b.to_bits(), c.to_bits(), 0]);
            }
        }
        Ty::Div => {
            // upstream 11-28 07f56f5a's twelve cases (quotient, denominator, numerator)
            for (q, d, n) in [
                (0x7fc00000u32, 0x3f800000u32, 0x00000000u32),
                (0x7fc00000, 0xbf800000, 0x00000000),
                (0x7fc00000, 0xbf800000, 0x80000000),
                (0x7fc00000, 0x7f800000, 0x3f800000),
                (0x7fc00000, 0xff800000, 0x40400000),
                (0x7fc00000, 0x00000000, 0x3f800000),
                (0x7fc00000, 0x3f800000, 0x7f800000),
                (0x7fc00000, 0x3f800000, 0x3f800000),
                (0x7fc00000, 0x00000000, 0x00000000),
                (0x7fc00000, 0x7f800000, 0x7f800000),
                (0x40000000, 0x3f800000, 0x7fa00000),
                (0x40000000, 0xffa00000, 0x3f800000),
            ] {
                out.push([q, d, n, 0]);
            }
            let sp: Vec<u32> = [0u32, 0x8000_0000, 0x7f80_0000, 0xff80_0000, 0x7fc0_0000, 0xffc0_0000, 0x7fa0_0000, 0x0000_0001, 0x8000_0001, 0x007f_ffff, 0x3f80_0000, 0xbf80_0000, 0x4040_0000, 0x7f7f_ffff, 0x0080_0000, 0x4000_0000].to_vec();
            for &q in &[0x7fc0_0000u32, 0x3f80_0000, 0xbf80_0000, 0x7f80_0000, 0] {
                for &d in &sp {
                    for &n in &sp {
                        out.push([q, d, n, 0]);
                    }
                }
            }
        }
        Ty::DivH => {
            let sp = [0x0000u32, 0x8000, 0x7c00, 0xfc00, 0x7e00, 0xfe00, 0x7c01, 0x0001, 0x8001, 0x03ff, 0x3c00, 0xbc00, 0x4200, 0x7bff, 0x0400, 0x4000];
            for &q in &[0x7e00u32, 0x3c00, 0xbc00, 0x7c00, 0] {
                for &d in &sp {
                    for &n in &sp {
                        let g = |h: u32, r: &mut Rng| (r.next() & 0xffff_0000) | h;
                        out.push([g(q, &mut r), g(d, &mut r), g(n, &mut r), 0]);
                    }
                }
            }
        }
        Ty::Sq(abits) => {
            let a = f32::from_bits(abits);
            for da in [0i32, 0, 1, -1] {
                let x = f32::from_bits((abits as i32 + da) as u32);
                for db in [0i32, 1, -1] {
                    let y = f32::from_bits((abits as i32 + db) as u32);
                    out.push([x.to_bits(), y.to_bits(), (-(a * a)).to_bits(), 0]);
                }
            }
        }
        Ty::Fin => {
            let sp = finite_specials();
            for _ in 0..500 {
                let mut q = [0u32; 4];
                for x in q.iter_mut().take(3) {
                    *x = if r.next() % 2 == 0 { sp[r.next() as usize % sp.len()] } else { random_float(&mut r) };
                }
                out.push(q);
            }
        }
        Ty::Class => {
            let mut fs = float_specials();
            fs.extend([0x7f80_0001, 0xff80_0001, 0xffc0_0000, 0x0000_0001, 0x8000_0001, 0x007f_ffff]);
            let mut masks: Vec<u32> = (0..10).map(|b| 1u32 << b).collect();
            masks.extend([0, 0x3ff, 0xffff_ffff, 0x60, 0x198, 0x3, 0x7, 0x9a, 0x155, 0x3fc]);
            for &a in &fs {
                for &m in &masks {
                    out.push([a, m, 0, 0]);
                }
            }
        }
        Ty::Turn => {
            let mut v: Vec<u32> = float_specials()
                .into_iter()
                .filter(|&b| {
                    let x = f32::from_bits(b);
                    x.is_nan() || x.is_infinite() || x.abs() <= 256.0
                })
                .collect();
            v.extend([0.25f32, -0.25, 0.75, 255.0, 255.25, -255.5, 0.125, 0.5000001, 1e-5].map(f32::to_bits));
            for &a in &v {
                out.push([a, 0, 0, 0]);
            }
            for _ in 0..250 {
                let x = (r.next() % 1_000_000) as f32 / 1_000_000.0 * 512.0 - 256.0;
                out.push([x.to_bits(), 0, 0, 0]);
            }
        }
        Ty::F64 => {
            let sp = [0.0f64, -0.0, 1.0, -1.0, 0.5, 3.5, 1e300, -1e300, 1e-300, f64::INFINITY, f64::NEG_INFINITY, 5e-324, 123_456.789, f64::MAX, 4503599627370496.0];
            for &a in &sp {
                for &b in &sp {
                    let (x, y) = (double_bits(a), double_bits(b));
                    out.push([x[0], x[1], y[0], y[1]]);
                }
            }
        }
        Ty::F32 | Ty::U32 => unreachable!(),
    }
    out
}

// ---- the families ----

type Family = (&'static str, Vec<Spec>);

fn fract_row() -> Row {
    flc("v_fract_f32", Ty::F32, 1, 0, 0.0, |x| {
        let v = f64::from(x[0]);
        if !v.is_finite() {
            return vec![f32::NAN]; // ISA: x - floor(x) of an infinity is NaN
        }
        // ISA: x - floor(x), at most the largest float below 1
        vec![((v - v.floor()) as f32).min(f32::from_bits(0x3f7f_ffff))]
    })
}

fn cube_rows() -> Vec<Spec> {
    // OpenGL 4.6 table 8.19 (cube map face selection); RDNA2 ISA v_cube{id,sc,tc,ma}_f32 (S0 x, S1 y, S2 z)
    fn face(x: f32, y: f32, z: f32) -> [f32; 4] {
        let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
        if az >= ax && az >= ay {
            [if z < 0.0 { 5.0 } else { 4.0 }, if z < 0.0 { -x } else { x }, -y, 2.0 * z]
        } else if ay >= ax {
            [if y < 0.0 { 3.0 } else { 2.0 }, x, if y < 0.0 { -z } else { z }, 2.0 * y]
        } else {
            [if x < 0.0 { 1.0 } else { 0.0 }, if x < 0.0 { z } else { -z }, -y, 2.0 * x]
        }
    }
    ["v_cubeid_f32", "v_cubesc_f32", "v_cubetc_f32", "v_cubema_f32"]
        .into_iter()
        .enumerate()
        .map(|(k, name)| {
            // a zero's sign is not known to the ISA text's comparisons: both are accepted
            let row = flc(name, Ty::Fin, 3, 0, 0.0, move |x| {
                let mut v = Vec::new();
                for &a in &both_zeros(x[0]) {
                    for &b in &both_zeros(x[1]) {
                        for &c in &both_zeros(x[2]) {
                            v.push(face(a, b, c)[k]);
                        }
                    }
                }
                v
            });
            sp(row, "", prog(vop3(o(name), 10, vgpr(1), vgpr(2), vgpr(3)).to_vec(), 10))
        })
        .collect()
}

fn f32_misc() -> Vec<Spec> {
    let mut v = Vec::new();
    let vop2w = |name: &str| vec![vop2(o(name), 10, vgpr(1), 2)];
    v.push(sp(fract_row(), "", prog(vec![vop1(o("v_fract_f32"), 10, vgpr(1))], 10)));
    v.push(sp(
        // ISA v_med3_f32 (and the existing valu_tests): a NaN operand: v_min3 with the NaN ignored
        flc("v_med3_f32", Ty::F32, 3, 0, 0.0, |x| med3(&(*x).map(f64::from)).into_iter().map(|d| d as f32).collect()),
        "",
        prog(vop3(o("v_med3_f32"), 10, vgpr(1), vgpr(2), vgpr(3)).to_vec(), 10),
    ));
    // the DX9 multiply: 0 * anything (NaN, inf) is 0; then the add, fused or not
    v.push(sp(
        flc("v_mad_legacy_f32", Ty::F32, 3, 0, 0.0, |x| {
            // 0 * anything (NaN, inf) is 0; the product and the sum each round
            if x[0] == 0.0 || x[1] == 0.0 { vec![0.0 + x[2], -0.0 + x[2]] } else { vec![x[0] * x[1] + x[2]] }
        }),
        "",
        prog(vop3(o("v_mad_legacy_f32"), 10, vgpr(1), vgpr(2), vgpr(3)).to_vec(), 10),
    ));
    // GFX10.1's multiply-add family rounds twice (product, then sum); the inputs of `Ty::Mad` and
    // `Ty::Sq` come out different when it is fused (MSC may contract an fmul + fadd into an fma)
    let unfused = |name: &'static str, legacy: bool| {
        flc(name, Ty::Mad, 3, 0, 0.0, move |x| {
            let p = if legacy && (x[0] == 0.0 || x[1] == 0.0) { 0.0 } else { x[0] * x[1] };
            vec![p + x[2]]
        })
    };
    v.push(sp(unfused("v_mad_f32", false), "", prog(vop3(o("v_mad_f32"), 10, vgpr(1), vgpr(2), vgpr(3)).to_vec(), 10)));
    v.push(sp(unfused("v_mad_legacy_f32", true), "[mad]", prog(vop3(o("v_mad_legacy_f32"), 10, vgpr(1), vgpr(2), vgpr(3)).to_vec(), 10)));
    for name in ["v_mac_f32", "v_mac_legacy_f32"] {
        v.push(sp(unfused(name, name.contains("legacy")), "[mad]", prog(vec![smov_to_v(2, 10), vop2(o(name), 10, vgpr(1), 2)], 10)));
    }
    for a in [1.0f32 + 2f32.powi(-12), 1.0 + 3.0 * 2f32.powi(-13), 1.0001, 3.1, 1e-3 + 7e-8] {
        let tag = format!("[a={a:e}]");
        let k = (-(a * a)).to_bits();
        let ab = a.to_bits();
        v.push(sp(
            flc("v_madak_f32", Ty::Sq(ab), 2, 0, 0.0, move |x| vec![x[0] * x[1] + f32::from_bits(k)]),
            tag.clone(),
            prog([vec![vop2(o("v_madak_f32"), 10, vgpr(1), 2)], vec![k]].concat(), 10),
        ));
        v.push(sp(
            flc("v_madmk_f32", Ty::Sq(ab), 3, 0, 0.0, move |x| vec![x[0] * a + x[2]]),
            tag,
            prog([vec![vop2(o("v_madmk_f32"), 10, vgpr(1), 3)], vec![ab]].concat(), 10),
        ));
    }
    // the literal forms: the constant is the dword after the instruction
    for k in [1.0f32, -3.5, 1e20, 0.0] {
        let kb = k.to_bits();
        let tag = format!("[K={k:e}]");
        v.push(sp(
            flc("v_fmaak_f32", Ty::F32, 2, 0, 0.0, move |x| vec![x[0].mul_add(x[1], k)]),
            tag.clone(),
            prog([vop2w("v_fmaak_f32"), vec![kb]].concat(), 10),
        ));
        v.push(sp(
            flc("v_madak_f32", Ty::F32, 2, 0, 0.0, move |x| vec![x[0] * x[1] + k]),
            tag.clone(),
            prog([vop2w("v_madak_f32"), vec![kb]].concat(), 10),
        ));
        v.push(sp(
            flc("v_fmamk_f32", Ty::F32, 2, 0, 0.0, move |x| vec![x[0].mul_add(k, x[1])]),
            tag,
            prog([vop2w("v_fmamk_f32"), vec![kb]].concat(), 10),
        ));
    }
    v.extend(cube_rows());
    // v_div_fixup_f32 (S0 the quotient, S1 the denominator, S2 the numerator): RDNA2 ISA pseudocode
    // order: a NaN numerator, a NaN denominator (quieted), 0/0 and inf/inf (-QNaN), denominator 0 or
    // numerator inf (signed infinity), denominator inf or numerator 0 (signed zero), then a NaN
    // quotient (an overflow: signed infinity; upstream 07f56f5a's cases), else the quotient's
    // magnitude with the sign of the signs' xor. A denormal operand counts as itself or as zero.
    v.push(sp(
        Row {
            name: "v_div_fixup_f32",
            ty: Ty::Div,
            args: 3,
            want: Box::new(|i| {
                let one = |q: u32, d: u32, n: u32| -> u32 {
                    let nan = |x: u32| x & 0x7fff_ffff > 0x7f80_0000;
                    let inf = |x: u32| x & 0x7fff_ffff == 0x7f80_0000;
                    let zero = |x: u32| x & 0x7fff_ffff == 0;
                    let sign = (d ^ n) & 0x8000_0000;
                    if nan(n) {
                        n | 0x0040_0000
                    } else if nan(d) {
                        d | 0x0040_0000
                    } else if (zero(d) && zero(n)) || (inf(d) && inf(n)) {
                        0xffc0_0000
                    } else if zero(d) || inf(n) {
                        sign | 0x7f80_0000
                    } else if inf(d) || zero(n) {
                        sign
                    } else if nan(q) {
                        sign | 0x7f80_0000
                    } else {
                        sign | (q & 0x7fff_ffff)
                    }
                };
                let flush = |x: u32| if x & 0x7f80_0000 == 0 { x & 0x8000_0000 } else { x };
                let mut c = vec![one(i[0], i[1], i[2]), one(i[0], flush(i[1]), flush(i[2]))];
                c.dedup();
                Want::Bits(c)
            }),
        },
        "",
        prog(vop3(o("v_div_fixup_f32"), 10, vgpr(1), vgpr(2), vgpr(3)).to_vec(), 10),
    ));
    // D = {rtz(S1), rtz(S0)}: S0 low, toward zero (the ISA's name)
    v.push(sp(
        Row {
            name: "v_cvt_pkrtz_f16_f32",
            ty: Ty::F32,
            args: 2,
            want: Box::new(|i| {
                let (a, b) = (f64::from(f(i[0])), f64::from(f(i[1])));
                Want::Half2(vec![[to_half64(a, true), to_half64(b, true)], [to_half64(hflush32(a), true), to_half64(hflush32(b), true)]])
            }),
        },
        "",
        prog(vop2w("v_cvt_pkrtz_f16_f32"), 10),
    ));
    // byte S1[1:0] of S2 = clamp(round(S0)) (see the module note)
    v.push(sp(
        Row {
            name: "v_cvt_pk_u8_f32",
            ty: Ty::F32,
            args: 3,
            want: Box::new(|i| {
                let one = |x: f64| -> u32 { if x.is_nan() { 0 } else { x.clamp(0.0, 255.0).round_ties_even() as u32 } };
                let sh = 8 * (i[1] & 3);
                let put = |b: u32| (i[2] & !(0xff << sh)) | b << sh;
                Want::Bits(vec![put(one(f64::from(f(i[0])))), put(one(f64::from(flushed(f(i[0])))))])
            }),
        },
        "",
        prog(vop3(o("v_cvt_pk_u8_f32"), 10, vgpr(1), vgpr(2), vgpr(3)).to_vec(), 10),
    ));
    // double to float: the device refuses doubles; the row records that
    v.push(sp(
        Row {
            name: "v_cvt_f32_f64",
            ty: Ty::F64,
            args: 1,
            want: Box::new(|i| {
                let d = f64::from_bits(u64::from(i[1]) << 32 | u64::from(i[0]));
                Want::Float { cands: vec![d as f32], ulps: 0, abs: 0.0 }
            }),
        },
        "",
        prog(vec![vop1(o("v_cvt_f32_f64"), 10, vgpr(1))], 10),
    ));
    v
}

fn hflush32(x: f64) -> f64 {
    flushed(x as f32).into()
}

fn int_misc() -> Vec<Spec> {
    let mut v = Vec::new();
    let u3 = |name: &'static str, g: fn(&[u32; 3]) -> u32| {
        sp(int(name, 3, g), "", prog(vop3(o(name), 10, vgpr(1), vgpr(2), vgpr(3)).to_vec(), 10))
    };
    v.push(u3("v_add3_u32", |i| i[0].wrapping_add(i[1]).wrapping_add(i[2])));
    v.push(u3("v_lshl_add_u32", |i| (i[0] << (i[1] & 31)).wrapping_add(i[2])));
    v.push(u3("v_add_lshl_u32", |i| i[0].wrapping_add(i[1]) << (i[2] & 31)));
    v.push(u3("v_xad_u32", |i| (i[0] ^ i[1]).wrapping_add(i[2])));
    // ISA: ((1 << S0[4:0]) - 1) << S1[4:0]
    v.push(u3("v_bfm_b32", |i| ((1u32 << (i[0] & 31)).wrapping_sub(1)) << (i[1] & 31)));
    v.push(u3("v_alignbyte_b32", |i| (((u64::from(i[0]) << 32) | u64::from(i[1])) >> (8 * (i[2] & 3))) as u32));
    v.push(sp(int("v_xnor_b32", 2, |i| !(i[0] ^ i[1])), "", prog(vec![vop2(o("v_xnor_b32"), 10, vgpr(1), 2)], 10)));
    // ISA: the first bit (from the top) that differs from the sign bit; -1 when all bits agree
    v.push(sp(
        int("v_ffbh_i32", 1, |i| if i[0] == 0 || i[0] == u32::MAX { u32::MAX } else if i[0] >> 31 == 1 { (!i[0]).leading_zeros() } else { i[0].leading_zeros() }),
        "",
        prog(vec![vop1(o("v_ffbh_i32"), 10, vgpr(1))], 10),
    ));
    // 64-bit results: the low and the high dword are two rows
    for (hi, out) in [("#lo", 10u8), ("#hi", 11)] {
        let sel = move |x: u64| if out == 10 { x as u32 } else { (x >> 32) as u32 };
        let w = vop3b(o("v_mad_u64_u32"), 10, 20, vgpr(1), vgpr(2), vgpr(3)); // S2 = v[3:4]
        v.push(sp(
            bitsrow("v_mad_u64_u32", Ty::U64, 2, move |i| sel((u64::from(i[0]) * u64::from(i[1])).wrapping_add(u64::from(i[3]) << 32 | u64::from(i[2])))),
            hi,
            prog(w.to_vec(), out),
        ));
        for (name, left) in [("v_lshlrev_b64", true), ("v_lshrrev_b64", false)] {
            // S0 the amount (low 6 bits), S1 = v[2:3] the value
            let w = vop3(o(name), 10, vgpr(1), vgpr(2), 128);
            v.push(sp(
                bitsrow(name, Ty::U32, 3, move |i| {
                    let x = u64::from(i[2]) << 32 | u64::from(i[1]);
                    sel(if left { x << (i[0] & 63) } else { x >> (i[0] & 63) })
                }),
                hi,
                prog(w.to_vec(), out),
            ));
        }
    }
    // carries: the sum (or difference) and the carry-out bit as a select of 1 / 0
    let carry_out = |sdst: u8| vop3(0x100 | o("v_cndmask_b32"), 11, 128, 129, u16::from(sdst));
    let add_co = vop3b(o("v_add_co_u32"), 10, 20, vgpr(1), vgpr(2), 128);
    let mut w = add_co.to_vec();
    w.extend(carry_out(20));
    v.push(sp(int("v_add_co_u32", 2, |i| i[0].wrapping_add(i[1])), "#sum", prog(w.clone(), 10)));
    v.push(sp(int("v_add_co_u32", 2, |i| u32::from(i[0].checked_add(i[1]).is_none())), "#carry", prog(w, 11)));
    // the e32 forms: carry in from vcc (= c != 0), carry out in vcc
    let ne = o("v_cmp_ne_u32");
    for name in ["v_add_co_ci_u32", "v_sub_co_ci_u32", "v_subrev_co_ci_u32"] {
        let model = move |a: u32, b: u32, c: u32| -> (u32, u32) {
            let (x, y) = if name == "v_subrev_co_ci_u32" { (b, a) } else { (a, b) };
            let cin = u64::from(c != 0);
            if name == "v_add_co_ci_u32" {
                let s = u64::from(x) + u64::from(y) + cin;
                (s as u32, (s >> 32) as u32)
            } else {
                (x.wrapping_sub(y).wrapping_sub(cin as u32), u32::from(u64::from(x) < u64::from(y) + cin))
            }
        };
        let w = vec![vop1(1, 5, 129), vopc(ne, 128, 3), vop2(o(name), 10, vgpr(1), 2), vop2(1, 11, 128, 5)];
        v.push(sp(bitsrow(name, Ty::U32, 3, move |i| model(i[0], i[1], i[2]).0), "#sum", prog(w.clone(), 10)));
        v.push(sp(bitsrow(name, Ty::U32, 3, move |i| model(i[0], i[1], i[2]).1), "#carry", prog(w, 11)));
    }
    // v_swap_b32 v10, v11 exchanges them
    let w = vec![smov_to_v(0, 10), smov_to_v(1, 11), vop1(o("v_swap_b32"), 10, vgpr(11))];
    v.push(sp(int("v_swap_b32", 2, |i| i[1]), "#first", prog(w.clone(), 10)));
    v.push(sp(int("v_swap_b32", 2, |i| i[0]), "#second", prog(w, 11)));
    // M0-relative register moves: M0 = s3 & 3; v[1 + M0] is s[M0]
    let set_m0 = [sop2(o("s_and_b32"), 13, 3, 131), sop1(0x03, M0, 13)];
    v.push(sp(
        bitsrow("v_movrels_b32", Ty::Quad, 4, |i| i[(i[3] & 3) as usize]),
        "",
        prog([set_m0.to_vec(), vec![vop1(o("v_movrels_b32"), 10, vgpr(1))]].concat(), 10),
    ));
    for k in 0..4u8 {
        let mut w = set_m0.to_vec();
        for r in 5..9u16 {
            w.push(vop1(1, r as u8, 128));
        }
        w.push(vop1(o("v_movreld_b32"), 5, vgpr(1))); // v[5 + M0] = v1
        v.push(sp(
            bitsrow("v_movreld_b32", Ty::Quad, 4, move |i| if i[3] & 3 == u32::from(k) { i[0] } else { 0 }),
            format!("[v{}]", 5 + k),
            prog(w, 5 + k),
        ));
    }
    // 16-bit integers: the low half of the destination
    v.push(sp(
        Row { name: "v_lshrrev_b16", ty: Ty::U32, args: 2, want: Box::new(|i| Want::Bits16(vec![(i[1] & 0xffff) >> (i[0] & 15)])) },
        "",
        prog(vop3(o("v_lshrrev_b16"), 10, vgpr(1), vgpr(2), 128).to_vec(), 10),
    ));
    v.push(sp(
        Row { name: "v_add_nc_u16", ty: Ty::U32, args: 2, want: Box::new(|i| Want::Bits16(vec![(i[0] & 0xffff).wrapping_add(i[1] & 0xffff) & 0xffff])) },
        "",
        prog(vop3(o("v_add_nc_u16"), 10, vgpr(1), vgpr(2), 128).to_vec(), 10),
    ));
    // f64: the device converter refuses it; the row records that
    for (tag, out) in [("#lo", 10u8), ("#hi", 11)] {
        v.push(sp(
            bitsrow("v_add_f64", Ty::F64, 2, move |i| {
                let d = |a: u32, b: u32| f64::from_bits(u64::from(b) << 32 | u64::from(a));
                let r = (d(i[0], i[1]) + d(i[2], i[3])).to_bits();
                if out == 10 { r as u32 } else { (r >> 32) as u32 }
            }),
            tag,
            prog(vop3(o("v_add_f64"), 10, vgpr(1), vgpr(3), 128).to_vec(), out),
        ));
    }
    v
}

fn mix_value(word: u32, i: u8, opsel: u8, opsel_hi: u8, neg: u8, abs: u8) -> f32 {
    let mut x = if opsel_hi >> i & 1 == 1 { from_half(pick(word, opsel >> i & 1 == 1)) } else { f32::from_bits(word) };
    if abs >> i & 1 == 1 {
        x = x.abs();
    }
    if neg >> i & 1 == 1 {
        x = -x;
    }
    x
}

fn f16_rows() -> Vec<Spec> {
    let mut v = Vec::new();
    let w2 = |name: &str| prog(vec![vop2(o(name), 10, vgpr(1), 2)], 10);
    let w3 = |name: &str| prog(vop3(o(name), 10, vgpr(1), vgpr(2), vgpr(3)).to_vec(), 10);
    // v_div_fixup_f16: the same order on halves
    v.push(sp(
        Row {
            name: "v_div_fixup_f16",
            ty: Ty::DivH,
            args: 3,
            want: Box::new(|i| {
                let one = |q: u32, d: u32, n: u32| -> u32 {
                    let (q, d, n) = (q & 0xffff, d & 0xffff, n & 0xffff);
                    let nan = |x: u32| x & 0x7fff > 0x7c00;
                    let inf = |x: u32| x & 0x7fff == 0x7c00;
                    let zero = |x: u32| x & 0x7fff == 0;
                    let sign = (d ^ n) & 0x8000;
                    if nan(n) {
                        n | 0x0200
                    } else if nan(d) {
                        d | 0x0200
                    } else if (zero(d) && zero(n)) || (inf(d) && inf(n)) {
                        0xfe00
                    } else if zero(d) || inf(n) {
                        sign | 0x7c00
                    } else if inf(d) || zero(n) {
                        sign
                    } else if nan(q) {
                        sign | 0x7c00
                    } else {
                        sign | (q & 0x7fff)
                    }
                };
                let flush = |x: u32| if x & 0x7c00 == 0 { x & 0x8000 } else { x & 0xffff };
                Want::Half(vec![one(i[0], i[1], i[2]), one(i[0], flush(i[1]), flush(i[2]))])
            }),
        },
        "",
        w3("v_div_fixup_f16"),
    ));
    v.push(sp(hrow("v_mul_f16", 2, |x| x[0] * x[1]), "", w2("v_mul_f16")));
    v.push(sp(hrow("v_sub_f16", 2, |x| x[0] - x[1]), "", w2("v_sub_f16")));
    v.push(sp(hrow("v_fma_f16", 3, |x| x[0].mul_add(x[1], x[2])), "", w3("v_fma_f16")));
    // D = S0 * S1 + D: D starts as c
    v.push(sp(
        hrow("v_fmac_f16", 3, |x| x[0].mul_add(x[1], x[2])),
        "",
        prog(vec![smov_to_v(2, 10), vop2(o("v_fmac_f16"), 10, vgpr(1), 2)], 10),
    ));
    v.push(sp(
        Row {
            name: "v_med3_f16",
            ty: Ty::F16,
            args: 3,
            want: Box::new(|i| {
                let raw = [hv(i[0]), hv(i[1]), hv(i[2])];
                let mut c: Vec<u32> = med3(&raw).into_iter().map(|d| to_half64(d, false)).collect();
                c.extend(med3(&raw.map(hflush)).into_iter().map(|d| to_half64(d, false)));
                Want::Half(c)
            }),
        },
        "",
        w3("v_med3_f16"),
    ));
    v.push(sp(hrow("v_trunc_f16", 1, |x| x[0].trunc()), "", prog(vec![vop1(o("v_trunc_f16"), 10, vgpr(1))], 10)));
    // D = {S1.lo, S0.lo}
    v.push(sp(
        bitsrow("v_pack_b32_f16", Ty::F16, 2, |i| (i[1] & 0xffff) << 16 | (i[0] & 0xffff)),
        "",
        w3("v_pack_b32_f16"),
    ));
    // v_fma_mix: each source is f32 (opsel_hi bit 0) or a half picked by opsel (opsel_hi bit 1);
    // neg flips, neg_hi is the absolute value; the fused multiply-add in f32
    #[allow(clippy::type_complexity)]
    let variants: [(u8, u8, u8, u8); 8] = [
        (0, 0, 0, 0),
        (0, 7, 0, 0),
        (7, 7, 0, 0),
        (0, 2, 0, 0),
        (0, 5, 0, 0),
        (4, 4, 0, 0),
        (0, 0, 7, 0),
        (0, 7, 5, 3),
    ];
    for (opsel, opsel_hi, neg, abs) in variants {
        let tag = format!("[sel{opsel:x} hi{opsel_hi:x} neg{neg:x} abs{abs:x}]");
        let enc = |name: &str| {
            let e = vop3p(o(name), 10, [vgpr(1), vgpr(2), vgpr(3)], opsel, opsel_hi, neg, abs);
            let mut w = vec![smov_to_v(3, 10)];
            w.extend(e);
            w
        };
        let fused = move |i: &[u32; 4], flush: bool| -> f32 {
            let mut x = [0f32; 3];
            for k in 0..3u8 {
                let t = mix_value(i[k as usize], k, opsel, opsel_hi, neg, abs);
                x[k as usize] = if flush { flushed(t) } else { t };
            }
            x[0].mul_add(x[1], x[2])
        };
        v.push(sp(
            Row {
                name: "v_fma_mix_f32",
                ty: Ty::Mix,
                args: 3,
                want: Box::new(move |i| Want::Float { cands: vec![fused(i, false), fused(i, true)], ulps: 0, abs: 0.0 }),
            },
            tag.clone(),
            prog(enc("v_fma_mix_f32"), 10),
        ));
        for (name, hi) in [("v_fma_mixlo_f16", false), ("v_fma_mixhi_f16", true)] {
            v.push(sp(
                Row {
                    name,
                    ty: Ty::Mix,
                    args: 3,
                    want: Box::new(move |i| Want::HalfPart {
                        cands: vec![to_half64(f64::from(fused(i, false)), false), to_half64(f64::from(fused(i, true)), false)],
                        hi,
                        old: i[3],
                    }),
                },
                tag.clone(),
                prog(enc(name), 10),
            ));
        }
    }
    // packed halves: lo = f(src[opsel], ..), hi = f(src[opsel_hi], ..)
    type Pk = fn(&[f64; 3]) -> Vec<f64>;
    let pk_float = |name: &'static str, nsrc: usize, opsel: u8, opsel_hi: u8, g: Pk| {
        let tag = format!("[sel{opsel:x} hi{opsel_hi:x}]");
        let w = vop3p(o(name), 10, [vgpr(1), vgpr(2), if nsrc == 3 { vgpr(3) } else { 128 }], opsel, opsel_hi, 0, 0);
        let row = Row {
            name,
            ty: Ty::Mix,
            args: nsrc,
            want: Box::new(move |i| {
                let half = |hi: bool, sel: u8| -> [f64; 3] { [0, 1, 2].map(|k| hv(pick(i[k], if hi { opsel_hi >> k & 1 == 1 } else { sel >> k & 1 == 1 }))) };
                let lo = [half(false, opsel), half(false, opsel).map(hflush)];
                let hi = [half(true, opsel_hi), half(true, opsel_hi).map(hflush)];
                let lo_c: Vec<u32> = lo.iter().flat_map(|x| g(x)).map(|d| to_half64(d, false)).collect();
                let hi_c: Vec<u32> = hi.iter().flat_map(|x| g(x)).map(|d| to_half64(d, false)).collect();
                Want::Half2(lo_c.iter().flat_map(|&l| hi_c.iter().map(move |&h| [l, h])).collect())
            }),
        };
        sp(row, tag, prog(w.to_vec(), 10))
    };
    for (opsel, opsel_hi) in [(0u8, 3u8), (3, 0), (1, 1)] {
        v.push(pk_float("v_pk_add_f16", 2, opsel, opsel_hi, |x| vec![x[0] + x[1]]));
        v.push(pk_float("v_pk_mul_f16", 2, opsel, opsel_hi, |x| vec![x[0] * x[1]]));
        v.push(pk_float("v_pk_max_f16", 2, opsel, opsel_hi, |x| minmax64(x[0], x[1], false)));
        v.push(pk_float("v_pk_min_f16", 2, opsel, opsel_hi, |x| minmax64(x[0], x[1], true)));
    }
    v.push(pk_float("v_pk_fma_f16", 3, 0, 7, |x| vec![x[0].mul_add(x[1], x[2])]));
    v.push(pk_float("v_pk_fma_f16", 3, 5, 3, |x| vec![x[0].mul_add(x[1], x[2])]));
    // VOP2 packed fmac: D = S0 * S1 + D, D starts as s3's dword
    v.push(sp(
        Row {
            name: "v_pk_fmac_f16",
            ty: Ty::Mix,
            args: 3,
            want: Box::new(|i| {
                let c = |h: fn(u32) -> u32, fl: bool| {
                    let g = |x: u32| if fl { hflush(hv(x)) } else { hv(x) };
                    to_half64(g(h(i[0])).mul_add(g(h(i[1])), g(h(i[3]))), false)
                };
                let lo = |x: u32| x & 0xffff;
                let hi = |x: u32| x >> 16;
                let mut pairs = Vec::new();
                for fl in [false, true] {
                    for fl2 in [false, true] {
                        pairs.push([c(lo, fl), c(hi, fl2)]);
                    }
                }
                Want::Half2(pairs)
            }),
        },
        "",
        prog(vec![smov_to_v(3, 10), vop2(o("v_pk_fmac_f16"), 10, vgpr(1), 2)], 10),
    ));
    // 16-bit integer lanes, halves in place
    let pk_int = |name: &'static str, g: fn(u32, u32) -> u32| {
        let w = vop3p(o(name), 10, [vgpr(1), vgpr(2), 128], 0, 3, 0, 0);
        sp(
            bitsrow(name, Ty::Mix, 2, move |i| (g(i[0] >> 16, i[1] >> 16) & 0xffff) << 16 | (g(i[0] & 0xffff, i[1] & 0xffff) & 0xffff)),
            "",
            prog(w.to_vec(), 10),
        )
    };
    v.push(pk_int("v_pk_add_u16", |a, b| a.wrapping_add(b)));
    v.push(pk_int("v_pk_sub_u16", |a, b| a.wrapping_sub(b)));
    // the reversed shifts: amount (low 4 bits) first
    v.push(pk_int("v_pk_lshlrev_b16", |a, b| b << (a & 15)));
    v.push(pk_int("v_pk_lshrrev_b16", |a, b| b >> (a & 15)));
    // 8-bit dot products: signed bytes, sum + c
    fn dot(a: u32, b: u32) -> u32 {
        (0..4u32).map(|k| i32::from((a >> (8 * k)) as u8 as i8) * i32::from((b >> (8 * k)) as u8 as i8)).sum::<i32>() as u32
    }
    v.push(sp(
        int("v_dot4c_i32_i8", 3, |i| dot(i[0], i[1]).wrapping_add(i[2])),
        "",
        prog(vec![smov_to_v(2, 10), vop2(o("v_dot4c_i32_i8"), 10, vgpr(1), 2)], 10),
    ));
    v.push(sp(
        int("v_dot4_i32_i8", 3, |i| dot(i[0], i[1]).wrapping_add(i[2])),
        "",
        prog(vop3p(o("v_dot4_i32_i8"), 10, [vgpr(1), vgpr(2), vgpr(3)], 0, 7, 0, 0).to_vec(), 10),
    ));
    v
}

fn vcmp_misc() -> Vec<Spec> {
    let mut v = Vec::new();
    // vcc = (S0 class & S1 mask) != 0; S1 bit: 0 signaling NaN, 1 quiet NaN, 2 -inf, 3 -normal,
    // 4 -denormal, 5 -0, 6 +0, 7 +denormal, 8 +normal, 9 +inf (RDNA2 ISA v_cmp_class_f32)
    fn class_bit(x: u32) -> u32 {
        let (e, m, neg) = ((x >> 23) & 0xff, x & 0x7f_ffff, x >> 31 == 1);
        match (e, m) {
            (0xff, 0) => if neg { 2 } else { 9 },
            (0xff, _) => if m >> 22 & 1 == 1 { 1 } else { 0 },
            (0, 0) => if neg { 5 } else { 6 },
            (0, _) => if neg { 4 } else { 7 },
            _ => if neg { 3 } else { 8 },
        }
    }
    let default = |name: &str| prog(vec![vop1(1, 5, 129), vopc(o(name), vgpr(1), 2), vop2(1, 10, 128, 5)], 10);
    v.push(sp(
        Row { name: "v_cmp_class_f32", ty: Ty::Class, args: 2, want: Box::new(|i| Want::Bits(vec![(i[1] >> class_bit(i[0])) & 1])) },
        "",
        default("v_cmp_class_f32"),
    ));
    // 64-bit operands: S0 = v[1:2], S1 = v[3:4]
    let wide = |name: &str| prog(vec![vop1(1, 5, 129), vopc(o(name), vgpr(1), 3), vop2(1, 10, 128, 5)], 10);
    fn p64(name: &'static str, g: fn(u64, u64) -> bool) -> (&'static str, fn(u64, u64) -> bool) {
        (name, g)
    }
    for (name, g) in [
        p64("v_cmp_eq_u64", |a, b| a == b),
        p64("v_cmp_ne_u64", |a, b| a != b),
        p64("v_cmp_gt_u64", |a, b| a > b),
        p64("v_cmp_le_u64", |a, b| a <= b),
        p64("v_cmp_lt_u64", |a, b| a < b),
        p64("v_cmp_ge_u64", |a, b| a >= b),
    ] {
        v.push(sp(
            Row {
                name,
                ty: Ty::U64,
                args: 2,
                want: Box::new(move |i| Want::Bits(vec![u32::from(g(u64::from(i[1]) << 32 | u64::from(i[0]), u64::from(i[3]) << 32 | u64::from(i[2])))])),
            },
            "",
            wide(name),
        ));
    }
    v.push(sp(
        Row { name: "v_cmp_eq_u16", ty: Ty::F16, args: 2, want: Box::new(|i| Want::Bits(vec![u32::from(i[0] & 0xffff == i[1] & 0xffff)])) },
        "",
        default("v_cmp_eq_u16"),
    ));
    fn pf(name: &'static str, g: fn(f64, f64) -> bool) -> (&'static str, fn(f64, f64) -> bool) {
        (name, g)
    }
    for (name, g) in [
        pf("v_cmp_lt_f16", |a, b| a < b),
        pf("v_cmp_gt_f16", |a, b| a > b),
        pf("v_cmp_le_f16", |a, b| a <= b),
        pf("v_cmp_eq_f16", |a, b| a == b),
        pf("v_cmp_neq_f16", |a, b| !(a == b)),
        pf("v_cmp_nlt_f16", |a, b| !(a < b)),
    ] {
        v.push(sp(cmph(name, g), "", default(name)));
    }
    v
}

fn trans_misc() -> Vec<Spec> {
    let mut v = Vec::new();
    let one = |name: &str| prog(vec![vop1(o(name), 10, vgpr(1))], 10);
    let turn = |name: &'static str, g: fn(f64) -> f64| {
        let row = flc(name, Ty::Turn, 1, 0, 1e-5, move |x| {
            let t = f64::from(x[0]);
            vec![if t.is_finite() { g(t * std::f64::consts::TAU) as f32 } else { f32::NAN }]
        });
        sp(row, "", one(name))
    };
    // ISA: the argument in turns, valid within +-256; bigger or non-finite arguments are not tested for value
    v.push(turn("v_sin_f32", f64::sin));
    v.push(turn("v_cos_f32", f64::cos));
    v.push(sp(
        fl("v_rcp_iflag_f32", 1, 2, 0.0, |x| vec![(1.0 / f64::from(x[0])) as f32]),
        "",
        one("v_rcp_iflag_f32"),
    ));
    let half_fn = |name: &'static str, ulps: u32, abs: f64, g: fn(f64) -> f64| {
        let row = Row {
            name,
            ty: Ty::F16,
            args: 1,
            want: Box::new(move |i| {
                let x = hv(i[0]);
                Want::HalfNear { cands: vec![g(x), g(hflush(x))], ulps, abs }
            }),
        };
        sp(row, "", one(name))
    };
    v.push(half_fn("v_rcp_f16", 1, 0.0, |x| 1.0 / x));
    v.push(half_fn("v_rsq_f16", 1, 0.0, |x| 1.0 / x.sqrt()));
    v.push(half_fn("v_sqrt_f16", 1, 0.0, f64::sqrt));
    v.push(half_fn("v_exp_f16", 1, 0.0, f64::exp2));
    v.push(half_fn("v_sin_f16", 1, 2e-3, |x| if x.is_finite() { (x * std::f64::consts::TAU).sin() } else { f64::NAN }));
    v.push(half_fn("v_cos_f16", 1, 2e-3, |x| if x.is_finite() { (x * std::f64::consts::TAU).cos() } else { f64::NAN }));
    v
}

// ---- scalar: 64-bit, the K forms, bit fields ----

fn salu_b64() -> Vec<Spec> {
    let mut v = Vec::new();
    // a = s[0:1], b = s[2:3]; the result s[12:13] is two rows (#lo, #hi)
    let both = |v: &mut Vec<Spec>, name: &'static str, ty: Ty, args: usize, words: Vec<u32>, g: fn(&[u32; 4]) -> u64| {
        let mut w = words;
        w.push(smov_to_v(12, 10));
        w.push(smov_to_v(13, 11));
        v.push(sp(bitsrow(name, ty, args, move |i| g(i) as u32), "#lo", prog(w.clone(), 10)));
        v.push(sp(bitsrow(name, ty, args, move |i| (g(i) >> 32) as u32), "#hi", prog(w, 11)));
    };
    let a = |i: &[u32; 4]| u64::from(i[1]) << 32 | u64::from(i[0]);
    let b = |i: &[u32; 4]| u64::from(i[3]) << 32 | u64::from(i[2]);
    let _ = (a, b);
    macro_rules! bin64 {
        ($name:literal, $e:expr) => {
            both(&mut v, $name, Ty::U64, 2, vec![sop2(o($name), 12, 0, 2)], |i| {
                let (a, b) = (u64::from(i[1]) << 32 | u64::from(i[0]), u64::from(i[3]) << 32 | u64::from(i[2]));
                let f: fn(u64, u64) -> u64 = $e;
                f(a, b)
            })
        };
    }
    bin64!("s_and_b64", |a, b| a & b);
    bin64!("s_or_b64", |a, b| a | b);
    bin64!("s_xor_b64", |a, b| a ^ b);
    bin64!("s_andn2_b64", |a, b| a & !b);
    bin64!("s_orn2_b64", |a, b| a | !b);
    bin64!("s_nor_b64", |a, b| !(a | b));
    // the shifts: S1 is a 32-bit amount (low 6 bits), here s2
    bin64!("s_lshl_b64", |a, b| a << (b & 63));
    bin64!("s_lshr_b64", |a, b| a >> (b & 63));
    both(&mut v, "s_mov_b64", Ty::U64, 1, vec![sop1(o("s_mov_b64"), 12, 0)], |i| u64::from(i[1]) << 32 | u64::from(i[0]));
    both(&mut v, "s_not_b64", Ty::U64, 1, vec![sop1(o("s_not_b64"), 12, 0)], |i| !(u64::from(i[1]) << 32 | u64::from(i[0])));
    // ISA: each group of four bits becomes all ones when any is set
    fn wqm(x: u64) -> u64 {
        (0..16).fold(0, |r, q| r | if x >> (4 * q) & 0xf != 0 { 0xfu64 << (4 * q) } else { 0 })
    }
    both(&mut v, "s_wqm_b64", Ty::U64, 1, vec![sop1(o("s_wqm_b64"), 12, 0)], |i| wqm(u64::from(i[1]) << 32 | u64::from(i[0])));
    // scc = (s1 != s3) picks a or b
    both(
        &mut v,
        "s_cselect_b64",
        Ty::U64,
        2,
        vec![sopc(o("s_cmp_lg_u32"), 1, 3), sop2(o("s_cselect_b64"), 12, 0, 2)],
        |i| if i[1] != i[3] { u64::from(i[1]) << 32 | u64::from(i[0]) } else { u64::from(i[3]) << 32 | u64::from(i[2]) },
    );
    // ISA: size S0[5:0], offset S1[5:0]: ((1 << size) - 1) << offset
    both(&mut v, "s_bfm_b64", Ty::U32, 2, vec![sop2(o("s_bfm_b64"), 12, 0, 1)], |i| ((1u64 << (i[0] & 63)).wrapping_sub(1)) << (i[1] & 63));
    // 32-bit results of 64-bit operands
    let one32 = |v: &mut Vec<Spec>, name: &'static str, ty: Ty, args: usize, words: Vec<u32>, g: fn(&[u32; 4]) -> u32| {
        let mut w = words;
        w.push(smov_to_v(12, 10));
        v.push(sp(bitsrow(name, ty, args, move |i| g(i)), "", prog(w, 10)));
    };
    one32(&mut v, "s_bcnt1_i32_b64", Ty::U64, 1, vec![sop1(o("s_bcnt1_i32_b64"), 12, 0)], |i| (u64::from(i[1]) << 32 | u64::from(i[0])).count_ones());
    one32(&mut v, "s_ff1_i32_b64", Ty::U64, 1, vec![sop1(o("s_ff1_i32_b64"), 12, 0)], |i| {
        let x = u64::from(i[1]) << 32 | u64::from(i[0]);
        if x == 0 { u32::MAX } else { x.trailing_zeros() }
    });
    // scc as a 0 / 1: s_cselect_b32 s12, 1, 0
    let scc12 = sop2(o("s_cselect_b32"), 12, 129, 128);
    one32(&mut v, "s_cmp_eq_u64", Ty::U64, 2, vec![sopc(o("s_cmp_eq_u64"), 0, 2), scc12], |i| u32::from(i[0] == i[2] && i[1] == i[3]));
    one32(&mut v, "s_cmp_lg_u64", Ty::U64, 2, vec![sopc(o("s_cmp_lg_u64"), 0, 2), scc12], |i| u32::from(!(i[0] == i[2] && i[1] == i[3])));
    v
}

fn salu_misc() -> Vec<Spec> {
    let mut v = Vec::new();
    let to12 = |mut w: Vec<u32>| {
        w.push(smov_to_v(12, 10));
        w
    };
    let scc12 = sop2(o("s_cselect_b32"), 12, 129, 128);
    // SOPK: a 16-bit immediate, sign-extended (the u32 compares zero-extend it)
    let sext16 = |x: u16| i32::from(x as i16) as u32;
    for imm in [0u16, 1, 0x8000, 0xffff] {
        let tag = format!("[{imm:#x}]");
        v.push(sp(bitsrow("s_movk_i32", Ty::U32, 1, move |_| sext16(imm)), tag.clone(), prog(to12(vec![sopk(o("s_movk_i32"), 12, imm)]), 10)));
        v.push(sp(
            bitsrow("s_addk_i32", Ty::U32, 1, move |i| i[0].wrapping_add(sext16(imm))),
            tag.clone(),
            prog(to12(vec![sop1(0x03, 12, 0), sopk(o("s_addk_i32"), 12, imm)]), 10),
        ));
        v.push(sp(
            bitsrow("s_mulk_i32", Ty::U32, 1, move |i| i[0].wrapping_mul(sext16(imm))),
            tag.clone(),
            prog(to12(vec![sop1(0x03, 12, 0), sopk(o("s_mulk_i32"), 12, imm)]), 10),
        ));
        fn ck(name: &'static str, g: fn(u32, u32) -> bool, signed: bool) -> (&'static str, fn(u32, u32) -> bool, bool) {
            (name, g, signed)
        }
        let cmpk = [
            ck("s_cmpk_eq_i32", |a, b| a == b, true),
            ck("s_cmpk_lg_i32", |a, b| a != b, true),
            ck("s_cmpk_gt_i32", |a, b| (a as i32) > b as i32, true),
            ck("s_cmpk_lt_i32", |a, b| (a as i32) < b as i32, true),
            ck("s_cmpk_eq_u32", |a, b| a == b, false),
            ck("s_cmpk_lg_u32", |a, b| a != b, false),
            ck("s_cmpk_gt_u32", |a, b| a > b, false),
            ck("s_cmpk_lt_u32", |a, b| a < b, false),
        ];
        for (name, g, signed) in cmpk {
            let k = if signed { sext16(imm) } else { u32::from(imm) };
            v.push(sp(bitsrow(name, Ty::U32, 1, move |i| u32::from(g(i[0], k))), tag.clone(), prog(to12(vec![sopk(o(name), 0, imm), scc12]), 10)));
        }
    }
    let s2 = |name: &'static str, g: fn(&[u32; 3]) -> u32| sp(int(name, 2, g), "", prog(to12(vec![sop2(o(name), 12, 0, 1)]), 10));
    // ISA: S0 shifted by S1[4:0], the field S1[22:16] bits wide, sign-extended from its top bit
    // A field that reaches past bit 31 (width + offset > 32) is not defined by the ISA text (it
    // reads (1 << w) - 1 of a 32-bit shift): the literal reading (no sign bit inside the value) and
    // the clamped one (the field ends at bit 31, which is its sign) are both accepted.
    v.push(sp(
        Row {
            name: "s_bfe_i32",
            ty: Ty::U32,
            args: 2,
            want: Box::new(|i| {
                let (w, off) = ((i[1] >> 16) & 0x7f, i[1] & 31);
                let t = u64::from(i[0] >> off);
                let sext = |w: u32| -> u32 {
                    if w == 0 {
                        return 0;
                    }
                    let m = (1u64 << w) - 1;
                    let f = t & m;
                    (if f >> (w - 1) & 1 == 1 { f | !m } else { f }) as u32
                };
                if w + off <= 32 {
                    Want::Bits(vec![sext(w)])
                } else {
                    Want::Bits(vec![t as u32, sext(32 - off)])
                }
            }),
        },
        "",
        prog(to12(vec![sop2(o("s_bfe_i32"), 12, 0, 1)]), 10),
    ));
    v.push(s2("s_mul_hi_u32", |i| ((u64::from(i[0]) * u64::from(i[1])) >> 32) as u32));
    v.push(s2("s_bfm_b32", |i| ((1u32 << (i[0] & 31)).wrapping_sub(1)) << (i[1] & 31)));
    v.push(s2("s_lshl2_add_u32", |i| (i[0] << 2).wrapping_add(i[1])));
    v.push(s2("s_lshl4_add_u32", |i| (i[0] << 4).wrapping_add(i[1])));
    v.push(sp(int("s_brev_b32", 1, |i| i[0].reverse_bits()), "", prog(to12(vec![sop1(o("s_brev_b32"), 12, 0)]), 10)));
    // ISA: each group of four bits becomes all ones when any is set
    fn wqm32(x: u32) -> u32 {
        (0..8).fold(0, |r, q| r | if x >> (4 * q) & 0xf != 0 { 0xf << (4 * q) } else { 0 })
    }
    v.push(sp(int("s_wqm_b32", 1, |i| wqm32(i[0])), "", prog(to12(vec![sop1(o("s_wqm_b32"), 12, 0)]), 10)));
    // D[S0[4:0]] = 0 / 1 in a copy of s1
    v.push(sp(
        int("s_bitset0_b32", 2, |i| i[1] & !(1 << (i[0] & 31))),
        "",
        prog(to12(vec![sop1(0x03, 12, 1), sop1(o("s_bitset0_b32"), 12, 0)]), 10),
    ));
    v.push(sp(
        int("s_bitset1_b32", 2, |i| i[1] | (1 << (i[0] & 31))),
        "",
        prog(to12(vec![sop1(0x03, 12, 1), sop1(o("s_bitset1_b32"), 12, 0)]), 10),
    ));
    // D = S0 - S1 - SCC, SCC = borrow; scc = (s2 != 0) comes in
    let subb = vec![sopc(o("s_cmp_lg_u32"), 2, 128), sop2(o("s_subb_u32"), 12, 0, 1)];
    let diff = |i: &[u32; 3]| i[0].wrapping_sub(i[1]).wrapping_sub(u32::from(i[2] != 0));
    v.push(sp(int("s_subb_u32", 3, diff), "#diff", prog(to12(subb.clone()), 10)));
    let mut w = subb;
    w.extend([scc12, smov_to_v(12, 11)]);
    v.push(sp(
        int("s_subb_u32", 3, |i| u32::from(u64::from(i[0]) < u64::from(i[1]) + u64::from(i[2] != 0))),
        "#borrow",
        prog(w, 11),
    ));
    v
}

fn mode_rows() -> Vec<Spec> {
    // MODE register, bits 4..7 (the denormal modes), hwreg(HW_REG_MODE, 4, 4): id 1, offset 4, size 4.
    // The translator takes the register as no operation (a "shortcut"); the rows only prove that
    // the instruction translates and leaves the program's results alone.
    let hwreg = 1 | 4 << 6 | 3 << 11;
    vec![
        sp(
            bitsrow("s_setreg_imm32_b32", Ty::U32, 1, |i| i[0]),
            "",
            prog(vec![sopk(o("s_setreg_imm32_b32"), 0, hwreg), 0xf, vop1(1, 10, vgpr(1))], 10),
        ),
        sp(
            bitsrow("s_setreg_b32", Ty::U32, 2, |i| i[0]),
            "",
            prog(vec![sopk(o("s_setreg_b32"), 1, hwreg), vop1(1, 10, vgpr(1))], 10),
        ),
    ]
}

// ---- the lane family: 32 threads, one wave ----
//
// Program: `s0..s7` = [M0, P, A0, A1, B0, B1, 0, 0] are the root constants (copied to `v1..v8`).
// Every lane first stores 0 to its marker word (`out[l]`) and its aux word (`out[32 + l]`); the
// exec mask is then set from M0 through a compare (`v_cmp_ne_u32 vcc` of bit l, `s_mov_b32
// exec_lo, vcc_lo`: the way a title narrows it); v14:v15 is A where bit l of P is clear, B where
// it is set (so the operands differ from lane to lane); the row's body runs; the lanes still
// active store 1 and `v22` (the body leaves its datum there). A lane that is not active at the
// end leaves its words 0.

use crate::emit::HostShader;
use crate::emit::host::{Dispatch, Root, View};
use crate::emit::valu_tests::ONE_AT_A_TIME;
use crate::native::RootType;

#[derive(Clone, Copy, PartialEq)]
enum Gen {
    F32,
    I32,
    F16,
    U64,
}

struct LaneRow {
    name: &'static str,
    tag: String,
    body: Vec<u32>,
    gk: Gen,
    /// the permute reads other lanes: all of them stay active (M0 is all ones)
    full: bool,
    /// a console wave of 64 (64 threads; the inputs are [M0, M1, P0, P1, A0, A1, B0, B1])
    wave64: bool,
    model: Box<dyn Fn(&[u32; 8]) -> Vec<u32>>,
}

fn lane_program(body: &[u32], wave64: bool) -> Vec<u32> {
    if wave64 {
        return lane_program64(body);
    }
    let (lshl, lshr, and, cnd, ne) = (o("v_lshlrev_b32"), o("v_lshrrev_b32"), o("v_and_b32"), o("v_cndmask_b32"), o("v_cmp_ne_u32"));
    let mut w = vec![s_mov_b32(9, 0x80)];
    w.extend_from_slice(&s_load_dwordx4(12, 8));
    for i in 0..8u8 {
        w.push(vop1(1, i + 1, u16::from(i)));
    }
    w.push(vop2(lshl, 9, 130, 0)); // v9 = tid * 4
    w.extend([vop1(1, 20, 128), vop1(1, 21, 129), vop1(1, 22, 128)]);
    w.extend_from_slice(&store_offen(20, 9, 12, 0));
    w.extend_from_slice(&store_offen(20, 9, 12, 128));
    // exec = lanes whose bit of M0 (v1) is set
    w.extend([vop2(lshr, 12, vgpr(0), 1), vop2(and, 12, 129, 12), vopc(ne, 128, 12), sop1(0x03, EXEC_LO, VCC_LO)]);
    // v14:v15 = bit l of P (v2) ? B (v5:v6) : A (v3:v4)
    w.extend([vop2(lshr, 13, vgpr(0), 2), vop2(and, 13, 129, 13), vopc(ne, 128, 13)]);
    w.extend([vop2(cnd, 14, vgpr(3), 5), vop2(cnd, 15, vgpr(4), 6)]);
    w.extend_from_slice(body);
    w.extend_from_slice(&store_offen(21, 9, 12, 0));
    w.extend_from_slice(&store_offen(22, 9, 12, 128));
    w.push(S_ENDPGM);
    w
}

/// The wave of 64: `s0..s7` = [M0, M1, P0, P1, A0, A1, B0, B1] (`v1..v8`); the exec mask is
/// M, L (`v14:v15`) is B where the lane's bit of P is set, else A; the out words are 64 markers,
/// then 64 aux words.
fn lane_program64(body: &[u32]) -> Vec<u32> {
    let (lshl, lshr, and, cnd, ne, le) = (o("v_lshlrev_b32"), o("v_lshrrev_b32"), o("v_and_b32"), o("v_cndmask_b32"), o("v_cmp_ne_u32"), o("v_cmp_le_u32"));
    let mut w = vec![s_mov_b32(9, 0x80)];
    w.extend_from_slice(&s_load_dwordx4(12, 8));
    for i in 0..8u8 {
        w.push(vop1(1, i + 1, u16::from(i)));
    }
    w.push(vop2(lshl, 9, 130, 0)); // v9 = tid * 4
    w.extend([vop1(1, 20, 128), vop1(1, 21, 129), vop1(1, 22, 128), vop1(1, 23, 128)]);
    w.extend_from_slice(&store_offen(20, 9, 12, 0));
    w.extend_from_slice(&store_offen(20, 9, 12, 256));
    // v12 = tid & 31; vcc = (tid >= 32); v13 = bit (tid & 31) of M0 or M1; exec = those lanes
    w.extend([vop2(and, 12, 159, 0), vopc(le, 160, 0), vop2(cnd, 13, vgpr(1), 2), vop2(lshr, 13, vgpr(12), 13)]);
    w.extend([vop2(and, 13, 129, 13), vopc(ne, 128, 13), 0xBEFE_046A]); // s_mov_b64 exec, vcc
    // v14:v15 = bit (tid & 31) of P0 or P1 ? B : A
    w.extend([vopc(le, 160, 0), vop2(cnd, 13, vgpr(3), 4), vop2(lshr, 13, vgpr(12), 13), vop2(and, 13, 129, 13), vopc(ne, 128, 13)]);
    w.extend([vop2(cnd, 14, vgpr(5), 7), vop2(cnd, 15, vgpr(6), 8)]);
    w.extend_from_slice(body);
    w.extend_from_slice(&store_offen(21, 9, 12, 0));
    w.extend_from_slice(&store_offen(22, 9, 12, 256));
    w.push(S_ENDPGM);
    w
}

/// The lanes that end active: (1, aux) each.
fn lane_out(f: impl Fn(usize) -> Option<u32>) -> Vec<u32> {
    let mut out = vec![0u32; 64];
    for l in 0..32 {
        if let Some(aux) = f(l) {
            out[l] = 1;
            out[32 + l] = aux;
        }
    }
    out
}

fn lane_cases(gk: Gen, full: bool, wave64: bool) -> Vec<[u32; 8]> {
    let mut r = Rng(0x1357_9BDF_2468_ACE1);
    let masks = [u32::MAX, u32::MAX, 0x0000_ffff, 0xaaaa_aaaa, 0x5555_5555, 1, 0x8000_0000, 0, 0x00ff_ff00];
    let pats = [0u32, u32::MAX, 0xaaaa_aaaa, 0x0f0f_0f0f, 0x8000_0001, 0x5555_5555, 0xffff_0000];
    let fs: Vec<u32> = float_specials().into_iter().filter(|&b| !f32::from_bits(b).is_subnormal()).collect();
    let is = int_specials();
    let hs: Vec<u32> = half_specials().into_iter().filter(|&h| h & 0x7c00 != 0 || h & 0x3ff == 0).collect();
    let u6 = u64_specials();
    let mut out = Vec::new();
    for n in 0..300 {
        let m0 = if full { u32::MAX } else if n % 3 == 0 { r.next() } else { masks[r.next() as usize % masks.len()] };
        let p = if n % 3 == 1 { r.next() } else { pats[r.next() as usize % pats.len()] };
        let v = |r: &mut Rng| -> [u32; 2] {
            match gk {
                Gen::F32 => [if r.next() % 4 == 0 { random_float(r) } else { fs[r.next() as usize % fs.len()] }, 0],
                Gen::I32 => [if r.next() % 3 == 0 { r.next() } else { is[r.next() as usize % is.len()] }, 0],
                Gen::F16 => [(r.next() & 0xffff_0000) | if r.next() % 3 == 0 { random_half(r) } else { hs[r.next() as usize % hs.len()] }, 0],
                Gen::U64 => {
                    let x = if r.next() % 4 == 0 { u64::from(r.next()) << 32 | u64::from(r.next()) } else { u6[r.next() as usize % u6.len()] };
                    [x as u32, (x >> 32) as u32]
                }
            }
        };
        let (a, b) = (v(&mut r), v(&mut r));
        if wave64 {
            // M and P are 64-bit now: two words each, the high one from the same sets
            let m1 = if full { u32::MAX } else if n % 3 == 0 { r.next() } else { masks[r.next() as usize % masks.len()] };
            let p1 = if n % 3 == 1 { r.next() } else { pats[r.next() as usize % pats.len()] };
            out.push([m0, m1, p, p1, a[0], a[1], b[0], b[1]]);
        } else {
            out.push([m0, p, a[0], a[1], b[0], b[1], 0, 0]);
        }
    }
    out
}

fn run_lane_cases(shader: &HostShader, out_words: u32, cases: &[[u32; 8]], mut check: impl FnMut(&str, &[u32; 8], &[u32])) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut ran = 0;
    for (label, warp) in [("hardware", false), ("warp", true)] {
        let host = match Host::new(warp) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("no SM 6.6 {label} device ({e}); skipped");
                continue;
            }
        };
        let (root, pso) = host.compute_pipeline(shader).unwrap_or_else(|e| panic!("{label}: compute_pipeline failed: {e}"));
        for inputs in cases {
            let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
            for u in &shader.user_data {
                ud[u.index as usize] = match (u.root, u.slot) {
                    (RootType::Constants, 0) => inputs[u.dword as usize],
                    (RootType::Table, 1) => 0,
                    other => panic!("unexpected user data {other:?}"),
                };
            }
            let out = host
                .dispatch(&root, &pso, &Dispatch { roots: vec![(0, Root::Constants(ud))], views: &[View::RawUav(out_words)], samplers: &[], groups: [1, 1, 1] })
                .unwrap_or_else(|e| panic!("{label}: dispatch failed: {e}"));
            check(label, inputs, &out[0]);
        }
        ran += 1;
    }
    if ran == 0 {
        eprintln!("no SM 6.6 hardware device; not run");
    }
}

fn run_lane_row(row: &LaneRow) -> Outcome {
    let words = lane_program(&row.body, row.wave64);
    let wave64 = row.wave64;
    let threads = if wave64 { 64 } else { 32 };
    let shader = match std::panic::catch_unwind(|| {
        // a console wave of 64 on the host's waves of 32 (what the Mac has)
        let was = super::super::lower::TEST_HOST_WAVE.replace(wave64.then_some(32));
        let r = std::panic::catch_unwind(|| build_words(&words, threads, 0, wave64, 24, 8));
        super::super::lower::TEST_HOST_WAVE.set(was);
        r.unwrap_or_else(|e| std::panic::resume_unwind(e))
    }) {
        Ok(s) => s,
        Err(e) => return Outcome::Untranslated(panic_text(e)),
    };
    let inputs = lane_cases(row.gk, row.full, row.wave64);
    let (mut bad, mut ran, mut examples) = (0usize, 0usize, Vec::new());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_lane_cases(&shader, 2 * threads, &inputs, |backend, inp, got| {
            ran += 1;
            let want = (row.model)(inp);
            if got[..want.len()] != want[..] {
                bad += 1;
                if examples.len() < 3 {
                    let l = (0..want.len()).find(|&l| got[l] != want[l]).unwrap_or(0);
                    examples.push(format!(
                        "{backend}: in M0 {:#010x} P {:#010x} A {:#x}:{:#x} B {:#x}:{:#x} -> {} {l} got {:#010x} want {:#010x}",
                        inp[0], inp[1], inp[3], inp[2], inp[5], inp[4], if l < threads as usize { "active lane" } else { "aux of lane" }, got[l], want[l]
                    ));
                }
            }
        });
    }));
    if let Err(e) = result {
        let t = panic_text(e);
        return if t.contains("unsupported") { Outcome::Refused(t) } else { Outcome::Mismatch { cases: ran, bad: bad.max(1), examples: vec![t] } };
    }
    if ran == 0 {
        return Outcome::Refused("no device".into());
    }
    if bad == 0 { Outcome::Passed(ran) } else { Outcome::Mismatch { cases: ran, bad, examples } }
}

fn run_lane_family(family: &str, rows: &[LaneRow]) {
    if Host::new(false).is_err() {
        eprintln!("{family}: no device; not run");
        return;
    }
    let mut failed = Vec::new();
    for row in rows {
        let label = format!("{}{}", row.name, row.tag);
        let (outcome, cases, detail) = match run_lane_row(row) {
            Outcome::Passed(n) => ("pass", n, String::new()),
            Outcome::Mismatch { cases, bad, examples } => ("MISMATCH", cases, format!("{bad} of {cases}; {}", examples.join(" / "))),
            Outcome::Untranslated(t) => ("UNTRANSLATED", 0, t),
            Outcome::Refused(t) => ("refused", 0, t),
        };
        let known = KNOWN.iter().find(|(n, _)| *n == label);
        eprintln!("oracle {family:18} {label:24} {outcome}{} {cases} {detail}", if known.is_some() { " (known)" } else { "" });
        if let Some(path) = std::env::var_os("VICTORY_ORACLE_OUT") {
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(file, "{family}\t{label}\t{outcome}{}\t{cases}\t{detail}", if known.is_some() { " (known)" } else { "" });
            }
        }
        if (outcome == "MISMATCH" || outcome == "UNTRANSLATED") && known.is_none() {
            failed.push(format!("{label}: {detail}"));
        }
    }
    assert!(failed.is_empty(), "{family}: {} rows differ from the reference:\n{}", failed.len(), failed.join("\n"));
}

type Cmp = Box<dyn Fn([u32; 2], [u32; 2]) -> bool>;

/// The `v_cmpx_*` rows: exec = exec & (A op L) per lane; the datum is the final mask read as a number.
fn cmpx_rows() -> Vec<LaneRow> {
    let mut rows = Vec::new();
    let mut add = |name: &'static str, gk: Gen, cmp: Cmp| {
        let wide = gk == Gen::U64;
        let _ = wide;
        let body = vec![vopc(o(name), vgpr(3), 14), sop1(0x03, 20, EXEC_LO), smov_to_v(20, 22)];
        rows.push(LaneRow {
            name,
            tag: String::new(),
            body,
            gk,
            full: false,
            wave64: false,
            model: Box::new(move |c| {
                let (m0, p, a, b) = (c[0], c[1], [c[2], c[3]], [c[4], c[5]]);
                let mut fin = 0u32;
                for l in 0..32 {
                    if m0 >> l & 1 == 1 && cmp(a, if p >> l & 1 == 1 { b } else { a }) {
                        fin |= 1 << l;
                    }
                }
                lane_out(|l| (fin >> l & 1 == 1).then_some(fin))
            }),
        });
    };
    let fc = |g: fn(f32, f32) -> bool| -> Cmp { Box::new(move |a, b| g(f32::from_bits(a[0]), f32::from_bits(b[0]))) };
    add("v_cmpx_lt_f32", Gen::F32, fc(|a, b| a < b));
    add("v_cmpx_gt_f32", Gen::F32, fc(|a, b| a > b));
    add("v_cmpx_lg_f32", Gen::F32, fc(|a, b| a < b || a > b));
    add("v_cmpx_eq_f32", Gen::F32, fc(|a, b| a == b));
    add("v_cmpx_le_f32", Gen::F32, fc(|a, b| a <= b));
    add("v_cmpx_ge_f32", Gen::F32, fc(|a, b| a >= b));
    add("v_cmpx_neq_f32", Gen::F32, fc(|a, b| !(a == b)));
    add("v_cmpx_nge_f32", Gen::F32, fc(|a, b| !(a >= b)));
    add("v_cmpx_nle_f32", Gen::F32, fc(|a, b| !(a <= b)));
    add("v_cmpx_ngt_f32", Gen::F32, fc(|a, b| !(a > b)));
    add("v_cmpx_nlt_f32", Gen::F32, fc(|a, b| !(a < b)));
    let ic = |g: fn(u32, u32) -> bool| -> Cmp { Box::new(move |a, b| g(a[0], b[0])) };
    add("v_cmpx_eq_u32", Gen::I32, ic(|a, b| a == b));
    add("v_cmpx_gt_u32", Gen::I32, ic(|a, b| a > b));
    add("v_cmpx_lt_u32", Gen::I32, ic(|a, b| a < b));
    add("v_cmpx_le_u32", Gen::I32, ic(|a, b| a <= b));
    add("v_cmpx_ge_u32", Gen::I32, ic(|a, b| a >= b));
    add("v_cmpx_eq_i32", Gen::I32, ic(|a, b| a == b));
    add("v_cmpx_gt_i32", Gen::I32, ic(|a, b| (a as i32) > b as i32));
    add("v_cmpx_lt_i32", Gen::I32, ic(|a, b| (a as i32) < b as i32));
    add("v_cmpx_ge_i32", Gen::I32, ic(|a, b| (a as i32) >= b as i32));
    add("v_cmpx_le_i32", Gen::I32, ic(|a, b| (a as i32) <= b as i32));
    let hc = |g: fn(f64, f64) -> bool| -> Cmp { Box::new(move |a, b| g(hv(a[0]), hv(b[0]))) };
    add("v_cmpx_gt_f16", Gen::F16, hc(|a, b| a > b));
    add("v_cmpx_ge_f16", Gen::F16, hc(|a, b| a >= b));
    add("v_cmpx_lt_f16", Gen::F16, hc(|a, b| a < b));
    add("v_cmpx_eq_f16", Gen::F16, hc(|a, b| a == b));
    add("v_cmpx_eq_u16", Gen::F16, Box::new(|a, b| a[0] & 0xffff == b[0] & 0xffff));
    add("v_cmpx_ne_u16", Gen::F16, Box::new(|a, b| a[0] & 0xffff != b[0] & 0xffff));
    let wc = |g: fn(u64, u64) -> bool| -> Cmp { Box::new(move |a, b| g(u64::from(a[1]) << 32 | u64::from(a[0]), u64::from(b[1]) << 32 | u64::from(b[0]))) };
    add("v_cmpx_eq_u64", Gen::U64, wc(|a, b| a == b));
    add("v_cmpx_ne_u64", Gen::U64, wc(|a, b| a != b));
    add("v_cmpx_gt_u64", Gen::U64, wc(|a, b| a > b));
    add("v_cmpx_lt_u64", Gen::U64, wc(|a, b| a < b));
    rows
}

/// saveexec / wrexec with a mask held as a number (`s_mov_b32 vcc_lo, s1`: the translator bug
/// of the first oracle report), lane reads and permutes.
fn exec_rows() -> Vec<LaneRow> {
    let mut rows = Vec::new();
    let set_vcc = sop1(0x03, VCC_LO, 1);
    // ISA: D = EXEC; EXEC = S0 & EXEC (or |); SCC = EXEC != 0
    for (name, or) in [("s_and_saveexec_b32", false), ("s_or_saveexec_b32", true)] {
        rows.push(LaneRow {
            name,
            tag: String::new(),
            body: vec![set_vcc, sop1(o(name), 20, VCC_LO), smov_to_v(20, 22)],
            gk: Gen::I32,
            full: false,
            wave64: false,
            model: Box::new(move |c| {
                let new = if or { c[0] | c[1] } else { c[0] & c[1] };
                lane_out(|l| (new >> l & 1 == 1).then_some(c[0]))
            }),
        });
    }
    // RDNA2 ISA 12.3: EXEC = S0 & ~EXEC; D = EXEC
    rows.push(LaneRow {
        name: "s_andn2_wrexec_b32",
        tag: String::new(),
        body: vec![set_vcc, sop1(o("s_andn2_wrexec_b32"), 20, VCC_LO), smov_to_v(20, 22)],
        gk: Gen::I32,
        full: false,
        wave64: false,
        model: Box::new(|c| {
            let new = c[1] & !c[0];
            lane_out(|l| (new >> l & 1 == 1).then_some(new))
        }),
    });
    // the same without reading the result as data: the lanes of the new mask run on
    rows.push(LaneRow {
        name: "s_andn2_wrexec_b32",
        tag: "[exec only]".into(),
        body: vec![set_vcc, sop1(o("s_andn2_wrexec_b32"), 20, VCC_LO)],
        gk: Gen::I32,
        full: false,
        wave64: false,
        model: Box::new(|c| {
            let new = c[1] & !c[0];
            lane_out(|l| (new >> l & 1 == 1).then_some(0))
        }),
    });
    // ISA: s_wqm: every group of four bits is all ones where any of them is set. On EXEC the
    // quads with a live lane all run (the helper lanes of a pixel quad).
    let wqm = |m: u32| (0..8).fold(0u32, |a, q| if m >> (4 * q) & 15 != 0 { a | 15 << (4 * q) } else { a });
    rows.push(LaneRow {
        name: "s_wqm_b32",
        tag: "[exec]".into(),
        body: vec![sop1(o("s_wqm_b32"), EXEC_LO, EXEC_LO)],
        gk: Gen::I32,
        full: false,
        wave64: false,
        model: Box::new(move |c| {
            let new = wqm(c[0]);
            lane_out(|l| (new >> l & 1 == 1).then_some(0))
        }),
    });
    // the quad mask of EXEC as the select's mask: the lanes of a live quad get 1
    rows.push(LaneRow {
        name: "s_wqm_b32",
        tag: "[vcc select]".into(),
        body: vec![sop1(o("s_wqm_b32"), VCC_LO, EXEC_LO), vop2(o("v_cndmask_b32"), 22, 128, 21)],
        gk: Gen::I32,
        full: false,
        wave64: false,
        model: Box::new(move |c| {
            let q = wqm(c[0]);
            lane_out(|l| (c[0] >> l & 1 == 1).then_some(q >> l & 1))
        }),
    });
    // ISA: popcount(S0 & lanes below this one) + S1; the high form of a wave of 32 adds nothing
    rows.push(LaneRow {
        name: "v_mbcnt_lo_u32_b32",
        tag: String::new(),
        body: vop3(o("v_mbcnt_lo_u32_b32"), 22, vgpr(3), vgpr(5), 128).to_vec(),
        gk: Gen::I32,
        full: false,
        wave64: false,
        model: Box::new(|c| lane_out(|l| (c[0] >> l & 1 == 1).then(|| (c[2] & ((1u32 << l) - 1)).count_ones().wrapping_add(c[4])))),
    });
    rows.push(LaneRow {
        name: "v_mbcnt_hi_u32_b32",
        tag: String::new(),
        body: vop3(o("v_mbcnt_hi_u32_b32"), 22, vgpr(3), vgpr(5), 128).to_vec(),
        gk: Gen::I32,
        full: false,
        wave64: false,
        model: Box::new(|c| lane_out(|l| (c[0] >> l & 1 == 1).then_some(c[4]))),
    });
    // ISA: lane S1[4:0] of the destination takes the scalar S0; the others keep their value (the tid)
    let mut w = vec![vop1(1, 22, vgpr(0))];
    w.extend(vop3(o("v_writelane_b32"), 22, 2, 4, 128));
    rows.push(LaneRow {
        name: "v_writelane_b32",
        tag: String::new(),
        body: w,
        gk: Gen::I32,
        full: false,
        wave64: false,
        model: Box::new(|c| lane_out(|l| (c[0] >> l & 1 == 1).then(|| if l as u32 == c[4] & 31 { c[2] } else { l as u32 }))),
    });
    // ISA 12.12: each lane of a row of 16 reads the lane its nibble of {S2, S1} names in the other row
    let mut w = vec![vop1(1, 22, vgpr(9))];
    w.extend(vop3(o("v_permlanex16_b32"), 22, vgpr(9), 2, 3));
    rows.push(LaneRow {
        name: "v_permlanex16_b32",
        tag: String::new(),
        body: w,
        gk: Gen::I32,
        full: true,
        wave64: false,
        model: Box::new(|c| {
            let sel = u64::from(c[3]) << 32 | u64::from(c[2]);
            lane_out(|l| {
                let (row, j) = (l / 16, l % 16);
                let nib = (sel >> (4 * j)) & 15;
                Some((((1 - row) * 16) as u32 + nib as u32) * 4)
            })
        }),
    });
    rows
}

/// The same for a console wave of 64: 128 words (64 markers, 64 aux).
fn lane_out64(f: impl Fn(usize) -> Option<u32>) -> Vec<u32> {
    let mut out = vec![0u32; 128];
    for l in 0..64 {
        if let Some(aux) = f(l) {
            out[l] = 1;
            out[64 + l] = aux;
        }
    }
    out
}

/// `exec` forms of a wave of 64, on the host's 32 lanes (the lowering of `wave64_tests`): the
/// `_b64` compares into exec, the `_b64` saveexec/wrexec, the 64-bit logic written to exec.
fn exec64_rows() -> Vec<LaneRow> {
    let mut rows = Vec::new();
    let m64 = |c: &[u32; 8]| u64::from(c[1]) << 32 | u64::from(c[0]);
    let p64 = |c: &[u32; 8]| u64::from(c[3]) << 32 | u64::from(c[2]);
    // vcc = P (s[2:3]); s[20:21] = EXEC read as data into v22
    let set_vcc = sop1(o("s_mov_b64"), 106, 2);
    let read_exec = [sop1(o("s_mov_b64"), 20, 126), smov_to_v(20, 22)];
    for (name, or) in [("s_and_saveexec_b64", false), ("s_or_saveexec_b64", true)] {
        let mut body = vec![set_vcc, sop1(o(name), 20, 106)];
        body.push(smov_to_v(20, 22)); // the saved exec's low dword
        rows.push(LaneRow {
            name,
            tag: "[wave64]".into(),
            body,
            gk: Gen::I32,
            full: false,
            wave64: true,
            model: Box::new(move |c| {
                let new = if or { m64(c) | p64(c) } else { m64(c) & p64(c) };
                lane_out64(|l| (new >> l & 1 == 1).then_some(c[0]))
            }),
        });
    }
    // ISA: EXEC = S0 & ~EXEC (the lanes the old mask lacked), D = the new EXEC
    rows.push(LaneRow {
        name: "s_andn2_wrexec_b64",
        tag: "[wave64 exec only]".into(),
        body: vec![set_vcc, sop1(o("s_andn2_wrexec_b64"), 20, 106)],
        gk: Gen::I32,
        full: false,
        wave64: true,
        model: Box::new(move |c| {
            let new = p64(c) & !m64(c);
            lane_out64(|l| (new >> l & 1 == 1).then_some(0))
        }),
    });
    let wqm64 = |m: u64| (0..16).fold(0u64, |a, q| if m >> (4 * q) & 15 != 0 { a | 15 << (4 * q) } else { a });
    rows.push(LaneRow {
        name: "s_wqm_b64",
        tag: "[exec]".into(),
        body: vec![sop1(o("s_wqm_b64"), EXEC_LO, EXEC_LO)],
        gk: Gen::I32,
        full: false,
        wave64: true,
        model: Box::new(move |c| {
            let new = wqm64(m64(c));
            lane_out64(|l| (new >> l & 1 == 1).then_some(0))
        }),
    });
    rows.push(LaneRow {
        name: "s_wqm_b64",
        tag: "[vcc select]".into(),
        body: vec![sop1(o("s_wqm_b64"), VCC_LO, EXEC_LO), vop2(o("v_cndmask_b32"), 22, 128, 21)],
        gk: Gen::I32,
        full: false,
        wave64: true,
        model: Box::new(move |c| {
            let q = wqm64(m64(c));
            lane_out64(|l| (m64(c) >> l & 1 == 1).then_some((q >> l & 1) as u32))
        }),
    });
    type F = fn(u64, u64) -> u64;
    for (name, f) in [
        ("s_and_b64", (|e, v| e & v) as F),
        ("s_or_b64", |e, v| e | v),
        ("s_xor_b64", |e, v| e ^ v),
        ("s_andn2_b64", |e, v| e & !v),
        ("s_orn2_b64", |e, v| e | !v),
    ] {
        rows.push(LaneRow {
            name,
            tag: "[exec, exec, vcc]".into(),
            body: vec![set_vcc, sop2(o(name), EXEC_LO, EXEC_LO, VCC_LO)],
            gk: Gen::I32,
            full: false,
            wave64: true,
            model: Box::new(move |c| {
                let new = f(m64(c), p64(c));
                lane_out64(|l| (new >> l & 1 == 1).then_some(0))
            }),
        });
    }
    // `v_cmpx_*` writes the 64-bit EXEC; L is B or A per lane
    let mut cmpx = |name: &'static str, gk: Gen, cmp: Cmp| {
        let mut body = vec![vopc(o(name), vgpr(5), 14)];
        body.extend(read_exec);
        rows.push(LaneRow {
            name,
            tag: "[wave64]".into(),
            body,
            gk,
            full: false,
            wave64: true,
            model: Box::new(move |c| {
                let (a, b) = ([c[4], c[5]], [c[6], c[7]]);
                let mut fin = 0u64;
                for l in 0..64 {
                    if m64(c) >> l & 1 == 1 && cmp(a, if p64(c) >> l & 1 == 1 { b } else { a }) {
                        fin |= 1 << l;
                    }
                }
                lane_out64(|l| (fin >> l & 1 == 1).then_some(fin as u32))
            }),
        });
    };
    let fc = |g: fn(f32, f32) -> bool| -> Cmp { Box::new(move |a, b| g(f32::from_bits(a[0]), f32::from_bits(b[0]))) };
    cmpx("v_cmpx_lt_f32", Gen::F32, fc(|a, b| a < b));
    cmpx("v_cmpx_ge_f32", Gen::F32, fc(|a, b| a >= b));
    cmpx("v_cmpx_neq_f32", Gen::F32, fc(|a, b| !(a == b)));
    let ic = |g: fn(u32, u32) -> bool| -> Cmp { Box::new(move |a, b| g(a[0], b[0])) };
    cmpx("v_cmpx_gt_u32", Gen::I32, ic(|a, b| a > b));
    cmpx("v_cmpx_lt_i32", Gen::I32, ic(|a, b| (a as i32) < b as i32));
    cmpx("v_cmpx_eq_u32", Gen::I32, ic(|a, b| a == b));
    let wc = |g: fn(u64, u64) -> bool| -> Cmp { Box::new(move |a, b| g(u64::from(a[1]) << 32 | u64::from(a[0]), u64::from(b[1]) << 32 | u64::from(b[0]))) };
    cmpx("v_cmpx_lt_u64", Gen::U64, wc(|a, b| a < b));
    cmpx("v_cmpx_eq_u64", Gen::U64, wc(|a, b| a == b));
    rows
}

// ---- the families, the tests ----

fn families() -> Vec<Family> {
    vec![
        ("a_valu_f32_misc", f32_misc()),
        ("a_valu_int_misc", int_misc()),
        ("a_valu_f16_packed", f16_rows()),
        ("a_vcmp_misc", vcmp_misc()),
        ("a_trans_misc", trans_misc()),
        ("a_salu_b64", salu_b64()),
        ("a_salu_misc", salu_misc()),
        ("a_mode", mode_rows()),
    ]
}

fn lane_families() -> Vec<(&'static str, Vec<LaneRow>)> {
    vec![("a_lane_cmpx", cmpx_rows()), ("a_lane_exec", exec_rows()), ("a_lane_exec64", exec64_rows())]
}

fn run_a(family: &str) {
    let (name, specs) = families().into_iter().find(|(n, _)| *n == family).unwrap();
    let items = || specs.iter().map(|s| (&s.row, s.tag.clone(), Some(s.words.as_slice()))).collect();
    run_list(name, items());
    // the 16-bit float families again by the other way the default rounds: the sequence of
    // operations (D3DMetal, `VICTORY_F16_NATIVE=0`) where the first run was one `fptrunc`
    if family == "a_valu_f16_packed" || family == "a_trans_misc" {
        super::super::lower::TEST_F16_NATIVE.set(Some(false));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_list(name, items())));
        super::super::lower::TEST_F16_NATIVE.set(None);
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }
}

/// The same 16-bit rows with the switch off (`VICTORY_F16_NEAREST=0`: the host's conversion,
/// toward zero on the Mac): reports the differences (the last place of a result that is not
/// exact is one lower), never fails.
#[test]
fn oracle_a_f16_switch_off_report() {
    super::TOLERATE.set(true);
    super::super::lower::TEST_F16_NEAREST.set(Some(false));
    let (name, specs) = families().into_iter().find(|(n, _)| *n == "a_valu_f16_packed").unwrap();
    run_list(name, specs.iter().map(|s| (&s.row, s.tag.clone(), Some(s.words.as_slice()))).collect());
    super::super::lower::TEST_F16_NEAREST.set(None);
    super::TOLERATE.set(false);
}

macro_rules! a_test {
    ($test:ident, $family:literal) => {
        #[test]
        fn $test() {
            run_a($family);
        }
    };
}
a_test!(oracle_a_valu_f32_misc, "a_valu_f32_misc");
a_test!(oracle_a_valu_int_misc, "a_valu_int_misc");
a_test!(oracle_a_valu_f16_packed, "a_valu_f16_packed");
a_test!(oracle_a_vcmp_misc, "a_vcmp_misc");
a_test!(oracle_a_trans_misc, "a_trans_misc");
a_test!(oracle_a_salu_b64, "a_salu_b64");
a_test!(oracle_a_salu_misc, "a_salu_misc");
a_test!(oracle_a_mode, "a_mode");

#[test]
fn oracle_a_lane_cmpx() {
    run_lane_family("a_lane_cmpx", &cmpx_rows());
}
#[test]
fn oracle_a_lane_exec64() {
    run_lane_family("a_lane_exec64", &exec64_rows());
}
#[test]
fn oracle_a_lane_exec() {
    run_lane_family("a_lane_exec", &exec_rows());
}

/// The A rows name GFX10 mnemonics, build their programs, and the references agree with
/// each other where two of them overlap (no device needed).
#[test]
fn every_a_row_is_a_gfx10_mnemonic() {
    for (family, specs) in families() {
        for s in specs {
            assert!(opcode_of(s.row.name).is_some(), "{family}: {}", s.row.name);
            assert!(!s.words.is_empty());
        }
    }
    for (family, rows) in lane_families() {
        for r in rows {
            assert!(opcode_of(r.name).is_some(), "{family}: {}", r.name);
            let _ = lane_program(&r.body, r.wave64);
        }
    }
    // the half converter against the older f32 one, and toward zero below it
    let mut r = Rng(5);
    for _ in 0..2000 {
        let x = random_float(&mut r);
        let x = f32::from_bits(x);
        assert_eq!(to_half64(f64::from(x), false), to_half(x), "{x:e}");
        assert!(to_half64(f64::from(x), true) <= to_half64(f64::from(x), false) || x < 0.0);
    }
    assert_eq!(to_half64(65519.0, true), 0x7bff);
    assert_eq!(to_half64(65520.0, false), 0x7c00);
    // the multiply-add inputs do tell a fused result from a double-rounded one (else the rows prove nothing)
    let tells = |ty: Ty, args: usize, k: Option<f32>| {
        cases(ty, args)
            .iter()
            .filter(|c| {
                let (a, b) = (f32::from_bits(c[0]), f32::from_bits(c[1]));
                let c = k.unwrap_or(f32::from_bits(c[2]));
                a * b + c != a.mul_add(b, c)
            })
            .count()
    };
    assert!(tells(Ty::Mad, 3, None) > 300, "{}", tells(Ty::Mad, 3, None));
    let a = f32::from_bits(0x3f80_1000);
    assert!(tells(Ty::Sq(0x3f80_1000), 2, Some(-(a * a))) >= 4);
    // cases exist for every input family and are not empty
    for ty in [Ty::F16, Ty::Mix, Ty::U64, Ty::Quad, Ty::Fin, Ty::Class, Ty::Turn, Ty::F64, Ty::Mad, Ty::Sq(0x3f80_1000), Ty::Div, Ty::DivH] {
        for args in 1..=3 {
            assert!(!cases(ty, args).is_empty());
        }
    }
}
