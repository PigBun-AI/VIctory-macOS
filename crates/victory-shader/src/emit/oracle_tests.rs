//! mac S6: the per-mnemonic opcode oracle (`inv.isa.opcode_oracle`).
//!
//! Table-driven: one row per mnemonic names the instruction and gives a CPU reference; the
//! instruction is built from its name (`gfx10::opcode_of`, the Mesa tables the decoder
//! uses), put in a one-thread compute shader (`valu_tests::build_valu_cs`: the four root
//! constants are `s0..s3` and, copied, `v1..v4`; the result goes to `v0` or `s12`), translated
//! and run on the device for a few hundred input sets, and compared with the reference.
//! The mnemonics are the ones the game uses and no test names (`research/map-plan/
//! isa-mnemonics.tsv`, `test_ref = -`), by frequency; a family is one `#[test]`.
//!
//! A row ends as passed, mismatched (the first cases are printed), untranslated (the
//! translator refused or panicked: a finding, listed), or refused by the device converter.
//! A mismatch or untranslated row fails its family test unless `KNOWN` lists it with the
//! reason (a Metal/MSC difference, or a translator bug handed over). With
//! `VICTORY_ORACLE_OUT=<file>` every row appends `family, mnemonic, outcome, cases, detail`
//! (tab separated) for the census.
//!
//! Float comparison: NaN equals any NaN; a subnormal equals any subnormal or zero (the
//! console's denormal mode is not known here); zeros keep their sign; otherwise the distance
//! in units of the last place must not exceed the row's tolerance (and, where a result near
//! 0 is a difference of large numbers, an absolute tolerance). Each float row gives
//! reference values for the raw inputs and for the inputs with subnormals flushed.

use super::host::Host;
use super::valu_tests::{build_valu_cs, run_cases, sgpr, vgpr};
use crate::gfx10::{Encoding, opcode_of};

/// What the reference accepts.
enum Want {
    /// One of these 32-bit patterns.
    Bits(Vec<u32>),
    /// One of these low-16-bit patterns (a half result; the upper half is not compared).
    Bits16(Vec<u32>),
    /// One of these floats, within `ulps` or `abs`.
    Float { cands: Vec<f32>, ulps: u32, abs: f32 },
}

#[derive(Clone, Copy, PartialEq)]
enum Ty {
    F32,
    U32,
}

struct Row {
    name: &'static str,
    ty: Ty,
    args: usize,
    want: Box<dyn Fn(&[u32; 3]) -> Want>,
}

// ---- encoders (the ones valu_tests has are reused) ----

const fn vop2(op: u16, vdst: u8, src0: u16, vsrc1: u8) -> u32 {
    (op as u32) << 25 | (vdst as u32) << 17 | (vsrc1 as u32) << 9 | (src0 as u32 & 0x1FF)
}
const fn vopc(op: u16, src0: u16, vsrc1: u8) -> u32 {
    0x7C00_0000 | (op as u32) << 17 | (vsrc1 as u32) << 9 | (src0 as u32 & 0x1FF)
}
const fn vop1(op: u16, vdst: u8, src0: u16) -> u32 {
    0x7E00_0000 | (vdst as u32) << 17 | (op as u32) << 9 | (src0 as u32 & 0x1FF)
}
const fn sop1(op: u16, sdst: u8, src0: u8) -> u32 {
    0xBE80_0000 | (sdst as u32) << 16 | (op as u32) << 8 | src0 as u32
}
const fn sop2(op: u16, sdst: u8, src0: u8, src1: u8) -> u32 {
    0x8000_0000 | (op as u32) << 23 | (sdst as u32) << 16 | (src1 as u32) << 8 | src0 as u32
}
const fn sopc(op: u16, src0: u8, src1: u8) -> u32 {
    0xBF00_0000 | (op as u32) << 16 | (src1 as u32) << 8 | src0 as u32
}
const fn vop3(op: u16, vdst: u8, s0: u16, s1: u16, s2: u16) -> [u32; 2] {
    [
        0x35 << 26 | (op as u32 & 0x3FF) << 16 | vdst as u32,
        (s0 as u32 & 0x1FF) | (s1 as u32 & 0x1FF) << 9 | (s2 as u32 & 0x1FF) << 18,
    ]
}

/// The program of a row: reads `v1,v2,v3` / `s0,s1,s2`, leaves the result in `v0`.
fn body(name: &str) -> Vec<u32> {
    let (enc, op) = opcode_of(name).unwrap_or_else(|| panic!("{name} is not a GFX10 mnemonic"));
    const V0_FROM_S12: u32 = vop1(0x001, 0, 12);
    match enc {
        // its destination is a scalar register
        Encoding::Vop1 if name == "v_readfirstlane_b32" => vec![vop1(op, 12, vgpr(1)), V0_FROM_S12],
        Encoding::Vop1 => vec![vop1(op, 0, vgpr(1))],
        Encoding::Vop2 => match name {
            // v0 is also an input (the accumulator): it starts as c.
            "v_mac_f32" | "v_fmac_f32" => vec![vop1(0x001, 0, vgpr(3)), vop2(op, 0, vgpr(1), 2)],
            // vcc = (c != 0) picks b, else a.
            "v_cndmask_b32" => vec![
                vopc(opcode_of("v_cmp_ne_u32").expect("v_cmp_ne_u32").1, 128, 3),
                vop2(op, 0, vgpr(1), 2),
            ],
            _ => vec![vop2(op, 0, vgpr(1), 2)],
        },
        Encoding::Vop3 => vop3(op, 0, vgpr(1), vgpr(2), vgpr(3)).to_vec(),
        // v5 = 1; the compare sets vcc; v0 = vcc ? 1 : 0.
        Encoding::Vopc => vec![vop1(0x001, 5, 129), vopc(op, vgpr(1), 2), vop2(1, 0, 128, 5)],
        Encoding::Sop1 => vec![sop1(op, 12, 0), V0_FROM_S12],
        Encoding::Sop2 if name == "s_cselect_b32" => vec![
            // scc = (s2 != 0), then s12 = scc ? s0 : s1
            sopc(7, 2, 128),
            sop2(op, 12, 0, 1),
            V0_FROM_S12,
        ],
        Encoding::Sop2 => vec![sop2(op, 12, 0, 1), V0_FROM_S12],
        // scc = compare; s12 = scc ? 1 : 0
        Encoding::Sopc => vec![sopc(op, 0, 1), sop2(10, 12, 129, 128), V0_FROM_S12],
        other => panic!("{name}: {other:?} has no placement in the oracle"),
    }
}

// ---- inputs ----

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
}

fn float_specials() -> Vec<u32> {
    [
        0.0f32,
        -0.0,
        1.0,
        -1.0,
        0.5,
        -0.5,
        2.0,
        3.5,
        -7.25,
        0.1,
        1e-20,
        1e20,
        f32::MAX,
        -f32::MAX,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NAN,
        1e-40,
        -1e-40,
        65504.0,
        f32::MIN_POSITIVE,
        8_388_608.0,
        16_777_216.0,
        123_456.79,
    ]
    .iter()
    .map(|f| f.to_bits())
    .collect()
}

fn int_specials() -> Vec<u32> {
    vec![
        0,
        1,
        2,
        3,
        7,
        8,
        0x1f,
        0x20,
        0xff,
        0x100,
        0xffff,
        0x1_0000,
        0x7fff_ffff,
        0x8000_0000,
        0xffff_fffe,
        0xffff_ffff,
        0x00ff_ffff,
        0x0100_0000,
        0x0080_0000,
        0xdead_beef,
        0x1234_5678,
    ]
}

fn random_float(r: &mut Rng) -> u32 {
    let m = r.next() & 0x7f_ffff;
    let e = 100 + r.next() % 54;
    (r.next() & 1) << 31 | e << 23 | m
}

/// The input sets of a row: every pair of specials (or every special) and random ones.
fn cases(ty: Ty, args: usize) -> Vec<[u32; 4]> {
    let sp = if ty == Ty::F32 { float_specials() } else { int_specials() };
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    let mut rnd = |r: &mut Rng| match ty {
        Ty::F32 => random_float(r),
        Ty::U32 => {
            let v = r.next();
            // short numbers too: shifts, byte fields, small counts
            match r.next() % 4 {
                0 => v & 0xff,
                1 => v & 0xffff,
                _ => v,
            }
        }
    };
    let mut out = Vec::new();
    match args {
        1 => {
            out.extend(sp.iter().map(|&a| [a, 0, 0, 0]));
            for _ in 0..200 {
                out.push([rnd(&mut r), 0, 0, 0]);
            }
        }
        2 => {
            for &a in &sp {
                for &b in &sp {
                    out.push([a, b, 0, 0]);
                }
            }
            for _ in 0..150 {
                out.push([rnd(&mut r), rnd(&mut r), 0, 0]);
            }
        }
        _ => {
            for _ in 0..400 {
                let pick = |r: &mut Rng, rnd: &mut dyn FnMut(&mut Rng) -> u32| {
                    if r.next() % 3 == 0 {
                        sp[r.next() as usize % sp.len()]
                    } else {
                        rnd(r)
                    }
                };
                let a = pick(&mut r, &mut rnd);
                let b = pick(&mut r, &mut rnd);
                let c = pick(&mut r, &mut rnd);
                out.push([a, b, c, 0]);
            }
        }
    }
    out
}

// ---- comparison ----

fn ordered(x: f32) -> i64 {
    let b = x.to_bits();
    i64::from(if b >> 31 != 0 { !b } else { b | 0x8000_0000 })
}

fn float_ok(got: f32, cand: f32, ulps: u32, abs: f32) -> bool {
    if got.is_nan() || cand.is_nan() {
        return got.is_nan() && cand.is_nan();
    }
    let tiny = |x: f32| x.abs() < f32::MIN_POSITIVE;
    if (tiny(got) && cand.is_subnormal()) || (got.is_subnormal() && tiny(cand)) {
        return true;
    }
    if got.to_bits() == cand.to_bits() {
        return true;
    }
    if got.is_infinite() || cand.is_infinite() {
        return false;
    }
    if got == 0.0 && cand == 0.0 {
        return false; // zeros of different signs
    }
    (got - cand).abs() <= abs || (ordered(got) - ordered(cand)).unsigned_abs() <= u64::from(ulps)
}

fn matches(want: &Want, got: u32) -> bool {
    match want {
        Want::Bits(c) => c.contains(&got),
        Want::Bits16(c) => c.contains(&(got & 0xffff)),
        Want::Float { cands, ulps, abs } => cands
            .iter()
            .any(|&c| float_ok(f32::from_bits(got), c, *ulps, *abs)),
    }
}

fn shown(want: &Want) -> String {
    match want {
        Want::Bits(c) | Want::Bits16(c) => c.iter().map(|b| format!("{b:#x}")).collect::<Vec<_>>().join("|"),
        Want::Float { cands, .. } => cands
            .iter()
            .map(|c| format!("{c:e}/{:#x}", c.to_bits()))
            .collect::<Vec<_>>()
            .join("|"),
    }
}

// ---- row constructors ----

fn f(x: u32) -> f32 {
    f32::from_bits(x)
}
fn flushed(x: f32) -> f32 {
    if x.is_subnormal() { 0.0f32.copysign(x) } else { x }
}
fn bits(b: bool) -> Want {
    Want::Bits(vec![u32::from(b)])
}

/// A float function of 1, 2 or 3 float inputs; the reference runs on the raw and the
/// flushed inputs.
fn fl(name: &'static str, args: usize, ulps: u32, abs: f32, g: fn(&[f32; 3]) -> Vec<f32>) -> Row {
    Row {
        name,
        ty: Ty::F32,
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

/// A 32-bit integer function.
fn int(name: &'static str, args: usize, g: fn(&[u32; 3]) -> u32) -> Row {
    Row {
        name,
        ty: Ty::U32,
        args,
        want: Box::new(move |i| Want::Bits(vec![g(i)])),
    }
}

/// A boolean function of float inputs (raw and flushed accepted).
fn cmpf(name: &'static str, g: fn(f32, f32) -> bool) -> Row {
    Row {
        name,
        ty: Ty::F32,
        args: 2,
        want: Box::new(move |i| {
            let (a, b) = (f(i[0]), f(i[1]));
            let mut v = vec![u32::from(g(a, b)), u32::from(g(flushed(a), flushed(b)))];
            v.dedup();
            Want::Bits(v)
        }),
    }
}

fn cmpi(name: &'static str, g: fn(u32, u32) -> bool) -> Row {
    int_b(name, move |i| u32::from(g(i[0], i[1])))
}

fn int_b(name: &'static str, g: impl Fn(&[u32; 3]) -> u32 + 'static) -> Row {
    Row {
        name,
        ty: Ty::U32,
        args: 2,
        want: Box::new(move |i| Want::Bits(vec![g(i)])),
    }
}

fn sext(x: u32, width: u32) -> i32 {
    ((x << (32 - width)) as i32) >> (32 - width)
}

fn to_half(x: f32) -> u32 {
    // round to nearest even, with subnormal halves
    let b = x.to_bits();
    let sign = (b >> 16) & 0x8000;
    if x.is_nan() {
        return sign | 0x7e00;
    }
    let abs = f64::from(x.abs());
    if abs >= 65520.0 {
        return sign | 0x7c00;
    }
    if abs < 2f64.powi(-14) {
        // subnormal half: units of 2^-24
        let q = (abs / 2f64.powi(-24)).round_ties_even() as u32;
        return sign | q;
    }
    let e = abs.log2().floor() as i32;
    let m = abs / 2f64.powi(e) - 1.0;
    let q = (m * 1024.0).round_ties_even() as u32;
    let (e, q) = if q == 1024 { (e + 1, 0) } else { (e, q) };
    sign | (((e + 15) as u32) << 10) | q
}

fn from_half(h: u32) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0f32 } else { 1.0 };
    let e = (h >> 10) & 31;
    let m = (h & 1023) as f32;
    match e {
        0 => sign * m * 2f32.powi(-24),
        31 if m == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        _ => sign * (1.0 + m / 1024.0) * 2f32.powi(e as i32 - 15),
    }
}

fn sat_i32(x: f64) -> u32 {
    (if x.is_nan() { 0 } else { x.clamp(-2147483648.0, 2147483647.0) as i32 }) as u32
}

// ---- the families ----

fn f32_arith() -> Vec<Row> {
    fn minmax(a: f32, b: f32, min: bool) -> Vec<f32> {
        let mut v = Vec::new();
        if a.is_nan() || b.is_nan() {
            v.push(f32::NAN);
            v.push(if a.is_nan() { b } else { a });
            return v;
        }
        if a == 0.0 && b == 0.0 {
            let neg = if min { a.is_sign_negative() || b.is_sign_negative() } else { a.is_sign_negative() && b.is_sign_negative() };
            v.push(if neg { -0.0 } else { 0.0 });
            return v;
        }
        v.push(if min { a.min(b) } else { a.max(b) });
        v
    }
    vec![
        fl("v_add_f32", 2, 0, 0.0, |x| vec![x[0] + x[1]]),
        fl("v_sub_f32", 2, 0, 0.0, |x| vec![x[0] - x[1]]),
        fl("v_subrev_f32", 2, 0, 0.0, |x| vec![x[1] - x[0]]),
        fl("v_mul_f32", 2, 0, 0.0, |x| vec![x[0] * x[1]]),
        fl("v_mul_legacy_f32", 2, 0, 0.0, |x| {
            vec![if x[0] == 0.0 || x[1] == 0.0 { 0.0 } else { x[0] * x[1] }]
        }),
        fl("v_min_f32", 2, 0, 0.0, |x| minmax(x[0], x[1], true)),
        fl("v_max_f32", 2, 0, 0.0, |x| minmax(x[0], x[1], false)),
        fl("v_fma_f32", 3, 0, 0.0, |x| vec![x[0].mul_add(x[1], x[2])]),
        fl("v_fmac_f32", 3, 0, 0.0, |x| vec![x[0].mul_add(x[1], x[2])]),
        // the multiply-add that rounds twice: a fused one is accepted too
        fl("v_mad_f32", 3, 0, 0.0, |x| vec![x[0] * x[1] + x[2], x[0].mul_add(x[1], x[2])]),
        fl("v_mac_f32", 3, 0, 0.0, |x| vec![x[0] * x[1] + x[2], x[0].mul_add(x[1], x[2])]),
    ]
}

fn move_select() -> Vec<Row> {
    vec![
        int("v_mov_b32", 1, |i| i[0]),
        int("v_readfirstlane_b32", 1, |i| i[0]),
        int("v_cndmask_b32", 3, |i| if i[2] != 0 { i[1] } else { i[0] }),
    ]
}

fn int_arith() -> Vec<Row> {
    vec![
        int("v_add_nc_u32", 2, |i| i[0].wrapping_add(i[1])),
        int("v_sub_nc_u32", 2, |i| i[0].wrapping_sub(i[1])),
        int("v_subrev_nc_u32", 2, |i| i[1].wrapping_sub(i[0])),
        int("v_mul_lo_u32", 2, |i| i[0].wrapping_mul(i[1])),
        int("v_mul_hi_u32", 2, |i| ((u64::from(i[0]) * u64::from(i[1])) >> 32) as u32),
        int("v_mul_hi_i32", 2, |i| ((i64::from(i[0] as i32) * i64::from(i[1] as i32)) >> 32) as u32),
        int("v_mul_u32_u24", 2, |i| (i[0] & 0xff_ffff).wrapping_mul(i[1] & 0xff_ffff)),
        int("v_mul_i32_i24", 2, |i| (sext(i[0], 24).wrapping_mul(sext(i[1], 24))) as u32),
        int("v_mad_u32_u24", 3, |i| (i[0] & 0xff_ffff).wrapping_mul(i[1] & 0xff_ffff).wrapping_add(i[2])),
        int("v_mad_i32_i24", 3, |i| (sext(i[0], 24).wrapping_mul(sext(i[1], 24))).wrapping_add(i[2] as i32) as u32),
        int("v_min_u32", 2, |i| i[0].min(i[1])),
        int("v_max_u32", 2, |i| i[0].max(i[1])),
        int("v_min_i32", 2, |i| (i[0] as i32).min(i[1] as i32) as u32),
        int("v_max_i32", 2, |i| (i[0] as i32).max(i[1] as i32) as u32),
    ]
}

fn bits_ops() -> Vec<Row> {
    vec![
        int("v_and_b32", 2, |i| i[0] & i[1]),
        int("v_or_b32", 2, |i| i[0] | i[1]),
        int("v_xor_b32", 2, |i| i[0] ^ i[1]),
        int("v_not_b32", 1, |i| !i[0]),
        // the reversed shifts: amount first
        int("v_lshrrev_b32", 2, |i| i[1] >> (i[0] & 31)),
        int("v_lshlrev_b32", 2, |i| i[1] << (i[0] & 31)),
        int("v_ashrrev_i32", 2, |i| ((i[1] as i32) >> (i[0] & 31)) as u32),
        int("v_bfe_u32", 3, |i| {
            let w = i[2] & 31;
            if w == 0 { 0 } else { (i[0] >> (i[1] & 31)) & ((1u32 << w) - 1) }
        }),
        int("v_bfe_i32", 3, |i| {
            let w = i[2] & 31;
            // the shift is arithmetic (S0 is signed in the ISA's text)
            if w == 0 { 0 } else { sext((((i[0] as i32) >> (i[1] & 31)) as u32) & ((1u32 << w) - 1), w) as u32 }
        }),
        int("v_bfi_b32", 3, |i| (i[0] & i[1]) | (!i[0] & i[2])),
        int("v_alignbit_b32", 3, |i| (((u64::from(i[0]) << 32) | u64::from(i[1])) >> (i[2] & 31)) as u32),
        int("v_bcnt_u32_b32", 2, |i| i[0].count_ones().wrapping_add(i[1])),
        int("v_ffbh_u32", 1, |i| if i[0] == 0 { u32::MAX } else { i[0].leading_zeros() }),
        int("v_ffbl_b32", 1, |i| if i[0] == 0 { u32::MAX } else { i[0].trailing_zeros() }),
        int("v_bfrev_b32", 1, |i| i[0].reverse_bits()),
        int("v_lshl_or_b32", 3, |i| (i[0] << (i[1] & 31)) | i[2]),
        int("v_and_or_b32", 3, |i| (i[0] & i[1]) | i[2]),
        int("v_or3_b32", 3, |i| i[0] | i[1] | i[2]),
    ]
}

fn trans() -> Vec<Row> {
    vec![
        // The hardware's reciprocal, square root, exp2 and log2 are 1 ulp (denormals aside);
        // a few more are allowed for the host's fast forms.
        fl("v_rcp_f32", 1, 2, 0.0, |x| vec![(1.0 / f64::from(x[0])) as f32]),
        fl("v_rsq_f32", 1, 2, 0.0, |x| vec![(1.0 / f64::from(x[0]).sqrt()) as f32]),
        fl("v_sqrt_f32", 1, 2, 0.0, |x| vec![f64::from(x[0]).sqrt() as f32]),
        fl("v_exp_f32", 1, 4, 0.0, |x| vec![f64::from(x[0]).exp2() as f32]),
        fl("v_log_f32", 1, 4, 2e-6, |x| vec![f64::from(x[0]).log2() as f32]),
        fl("v_floor_f32", 1, 0, 0.0, |x| vec![x[0].floor()]),
        fl("v_ceil_f32", 1, 0, 0.0, |x| vec![x[0].ceil()]),
        fl("v_trunc_f32", 1, 0, 0.0, |x| vec![x[0].trunc()]),
        fl("v_rndne_f32", 1, 0, 0.0, |x| vec![x[0].round_ties_even()]),
    ]
}

fn cvt() -> Vec<Row> {
    let ubyte = |name, k: u32| -> Row { Row { name, ty: Ty::U32, args: 1, want: Box::new(move |i| Want::Float { cands: vec![((i[0] >> (8 * k)) & 0xff) as f32], ulps: 0, abs: 0.0 }) } };
    let f_to_u = |name, g: fn(f64) -> u32| -> Row {
        Row { name, ty: Ty::F32, args: 1, want: Box::new(move |i| {
            let x = f64::from(f(i[0]));
            Want::Bits(vec![g(x), g(f64::from(flushed(f(i[0]))))])
        }) }
    };
    vec![
        Row { name: "v_cvt_f32_u32", ty: Ty::U32, args: 1, want: Box::new(|i| Want::Float { cands: vec![i[0] as f32], ulps: 0, abs: 0.0 }) },
        Row { name: "v_cvt_f32_i32", ty: Ty::U32, args: 1, want: Box::new(|i| Want::Float { cands: vec![i[0] as i32 as f32], ulps: 0, abs: 0.0 }) },
        f_to_u("v_cvt_u32_f32", |x| if x.is_nan() { 0 } else { x.clamp(0.0, 4294967295.0) as u32 }),
        f_to_u("v_cvt_i32_f32", |x| sat_i32(x.trunc())),
        f_to_u("v_cvt_flr_i32_f32", |x| sat_i32(x.floor())),
        f_to_u("v_cvt_rpi_i32_f32", |x| sat_i32(f64::from((x as f32 + 0.5).floor()))),
        ubyte("v_cvt_f32_ubyte0", 0),
        ubyte("v_cvt_f32_ubyte1", 1),
        ubyte("v_cvt_f32_ubyte2", 2),
        ubyte("v_cvt_f32_ubyte3", 3),
        Row { name: "v_cvt_f32_f16", ty: Ty::U32, args: 1, want: Box::new(|i| Want::Float { cands: vec![from_half(i[0] & 0xffff)], ulps: 0, abs: 0.0 }) },
        Row { name: "v_cvt_f16_f32", ty: Ty::F32, args: 1, want: Box::new(|i| Want::Bits16(vec![to_half(f(i[0])), to_half(flushed(f(i[0])))])) },
    ]
}

fn vcmp_f32() -> Vec<Row> {
    vec![
        cmpf("v_cmp_f_f32", |_, _| false),
        cmpf("v_cmp_lt_f32", |a, b| a < b),
        cmpf("v_cmp_eq_f32", |a, b| a == b),
        cmpf("v_cmp_le_f32", |a, b| a <= b),
        cmpf("v_cmp_gt_f32", |a, b| a > b),
        cmpf("v_cmp_lg_f32", |a, b| a < b || a > b),
        cmpf("v_cmp_ge_f32", |a, b| a >= b),
        cmpf("v_cmp_o_f32", |a, b| !a.is_nan() && !b.is_nan()),
        cmpf("v_cmp_u_f32", |a, b| a.is_nan() || b.is_nan()),
        cmpf("v_cmp_nge_f32", |a, b| !(a >= b)),
        cmpf("v_cmp_nlg_f32", |a, b| !(a < b || a > b)),
        cmpf("v_cmp_ngt_f32", |a, b| !(a > b)),
        cmpf("v_cmp_nle_f32", |a, b| !(a <= b)),
        cmpf("v_cmp_neq_f32", |a, b| !(a == b)),
        cmpf("v_cmp_nlt_f32", |a, b| !(a < b)),
        cmpf("v_cmp_tru_f32", |_, _| true),
    ]
}

fn vcmp_int() -> Vec<Row> {
    vec![
        cmpi("v_cmp_f_u32", |_, _| false),
        cmpi("v_cmp_lt_u32", |a, b| a < b),
        cmpi("v_cmp_eq_u32", |a, b| a == b),
        cmpi("v_cmp_le_u32", |a, b| a <= b),
        cmpi("v_cmp_gt_u32", |a, b| a > b),
        cmpi("v_cmp_ne_u32", |a, b| a != b),
        cmpi("v_cmp_ge_u32", |a, b| a >= b),
        cmpi("v_cmp_t_u32", |_, _| true),
        cmpi("v_cmp_f_i32", |_, _| false),
        cmpi("v_cmp_lt_i32", |a, b| (a as i32) < b as i32),
        cmpi("v_cmp_eq_i32", |a, b| a == b),
        cmpi("v_cmp_le_i32", |a, b| (a as i32) <= b as i32),
        cmpi("v_cmp_gt_i32", |a, b| (a as i32) > b as i32),
        cmpi("v_cmp_ne_i32", |a, b| a != b),
        cmpi("v_cmp_ge_i32", |a, b| (a as i32) >= b as i32),
        cmpi("v_cmp_t_i32", |_, _| true),
    ]
}

fn salu_alu() -> Vec<Row> {
    vec![
        int("s_mov_b32", 1, |i| i[0]),
        int("s_not_b32", 1, |i| !i[0]),
        int("s_and_b32", 2, |i| i[0] & i[1]),
        int("s_or_b32", 2, |i| i[0] | i[1]),
        int("s_xor_b32", 2, |i| i[0] ^ i[1]),
        int("s_andn2_b32", 2, |i| i[0] & !i[1]),
        int("s_orn2_b32", 2, |i| i[0] | !i[1]),
        int("s_nand_b32", 2, |i| !(i[0] & i[1])),
        int("s_nor_b32", 2, |i| !(i[0] | i[1])),
        int("s_xnor_b32", 2, |i| !(i[0] ^ i[1])),
        int("s_lshl_b32", 2, |i| i[0] << (i[1] & 31)),
        int("s_lshr_b32", 2, |i| i[0] >> (i[1] & 31)),
        int("s_ashr_i32", 2, |i| ((i[0] as i32) >> (i[1] & 31)) as u32),
        int("s_add_u32", 2, |i| i[0].wrapping_add(i[1])),
        int("s_sub_u32", 2, |i| i[0].wrapping_sub(i[1])),
        int("s_add_i32", 2, |i| i[0].wrapping_add(i[1])),
        int("s_sub_i32", 2, |i| i[0].wrapping_sub(i[1])),
        int("s_mul_i32", 2, |i| i[0].wrapping_mul(i[1])),
        int("s_min_i32", 2, |i| (i[0] as i32).min(i[1] as i32) as u32),
        int("s_min_u32", 2, |i| i[0].min(i[1])),
        int("s_max_i32", 2, |i| (i[0] as i32).max(i[1] as i32) as u32),
        int("s_max_u32", 2, |i| i[0].max(i[1])),
        int("s_abs_i32", 1, |i| (i[0] as i32).wrapping_abs() as u32),
        int("s_bcnt1_i32_b32", 1, |i| i[0].count_ones()),
        int("s_ff1_i32_b32", 1, |i| if i[0] == 0 { u32::MAX } else { i[0].trailing_zeros() }),
        int("s_flbit_i32_b32", 1, |i| if i[0] == 0 { u32::MAX } else { i[0].leading_zeros() }),
        int("s_bfe_u32", 2, |i| {
            let w = (i[1] >> 16) & 0x7f;
            let m = if w >= 32 { u32::MAX } else { (1u32 << w) - 1 };
            (i[0] >> (i[1] & 31)) & m
        }),
        int("s_lshl1_add_u32", 2, |i| (i[0] << 1).wrapping_add(i[1])),
    ]
}

fn salu_cmp() -> Vec<Row> {
    let mut rows = vec![
        cmpi("s_cmp_eq_u32", |a, b| a == b),
        cmpi("s_cmp_lg_u32", |a, b| a != b),
        cmpi("s_cmp_gt_u32", |a, b| a > b),
        cmpi("s_cmp_ge_u32", |a, b| a >= b),
        cmpi("s_cmp_lt_u32", |a, b| a < b),
        cmpi("s_cmp_le_u32", |a, b| a <= b),
        cmpi("s_cmp_eq_i32", |a, b| a == b),
        cmpi("s_cmp_lg_i32", |a, b| a != b),
        cmpi("s_cmp_gt_i32", |a, b| (a as i32) > b as i32),
        cmpi("s_cmp_ge_i32", |a, b| (a as i32) >= b as i32),
        cmpi("s_cmp_lt_i32", |a, b| (a as i32) < b as i32),
        cmpi("s_cmp_le_i32", |a, b| (a as i32) <= b as i32),
    ];
    rows.push(int("s_cselect_b32", 3, |i| if i[2] != 0 { i[0] } else { i[1] }));
    rows
}

/// The families of this round: name and rows. The rest of the census is planned in
/// `docs/mac/opcode-oracle.md`.
fn families() -> Vec<(&'static str, Vec<Row>)> {
    vec![
        ("valu_f32_arith", f32_arith()),
        ("valu_move_select", move_select()),
        ("valu_int_arith", int_arith()),
        ("valu_bit_ops", bits_ops()),
        ("valu_trans_round", trans()),
        ("valu_cvt", cvt()),
        ("vcmp_f32", vcmp_f32()),
        ("vcmp_int", vcmp_int()),
        ("salu_alu", salu_alu()),
        ("salu_cmp_select", salu_cmp()),
    ]
}

/// Differences that are known and not translator bugs, or bugs handed over: (mnemonic, reason).
const KNOWN: &[(&str, &str)] = &[(
    "v_cvt_f16_f32",
    "shortcut, documented: by default the translator takes the host's f32->f16 conversion, which rounds toward zero (lower.rs f16_nearest_on); with VICTORY_F16_NEAREST=1 it matches (oracle_f16_nearest_matches)",
)];

enum Outcome {
    Passed(usize),
    Mismatch { cases: usize, bad: usize, examples: Vec<String> },
    /// The translator refused the program or panicked.
    Untranslated(String),
    /// The device's converter refused it.
    Refused(String),
}

fn panic_text(e: Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<String>()
        .cloned()
        .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_else(|| "panic".into())
}

fn run_row(row: &Row) -> Outcome {
    let words = body(row.name);
    let shader = match std::panic::catch_unwind(|| build_valu_cs(&words)) {
        Ok(s) => s,
        Err(e) => return Outcome::Untranslated(panic_text(e)),
    };
    let inputs = cases(row.ty, row.args);
    let mut bad = 0usize;
    let mut examples = Vec::new();
    let mut ran = 0usize;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_cases(&shader, &inputs, |backend, _i, inp, got| {
            ran += 1;
            let want = (row.want)(&[inp[0], inp[1], inp[2]]);
            if !matches(&want, got) {
                bad += 1;
                if examples.len() < 4 {
                    examples.push(format!(
                        "{backend}: in {:#010x} {:#010x} {:#010x} -> {got:#010x}, want {}",
                        inp[0], inp[1], inp[2], shown(&want)
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

fn run_family(family: &str, rows: &[Row]) {
    if Host::new(false).is_err() {
        eprintln!("{family}: no device; not run");
        return;
    }
    let mut failed = Vec::new();
    for row in rows {
        let (outcome, cases, detail) = match run_row(row) {
            Outcome::Passed(n) => ("pass", n, String::new()),
            Outcome::Mismatch { cases, bad, examples } => ("MISMATCH", cases, format!("{bad} of {cases}; {}", examples.join(" / "))),
            Outcome::Untranslated(t) => ("UNTRANSLATED", 0, t),
            Outcome::Refused(t) => ("refused", 0, t),
        };
        let known = KNOWN.iter().find(|(n, _)| *n == row.name);
        eprintln!("oracle {family:18} {:24} {outcome}{} {cases} {detail}", row.name, if known.is_some() { " (known)" } else { "" });
        if let Some(path) = std::env::var_os("VICTORY_ORACLE_OUT") {
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(file, "{family}\t{}\t{outcome}{}\t{cases}\t{detail}", row.name, if known.is_some() { " (known)" } else { "" });
            }
        }
        if (outcome == "MISMATCH" || outcome == "UNTRANSLATED") && known.is_none() {
            failed.push(format!("{}: {detail}", row.name));
        }
    }
    assert!(failed.is_empty(), "{family}: {} rows differ from the reference:\n{}", failed.len(), failed.join("\n"));
}

macro_rules! family_test {
    ($test:ident, $family:literal) => {
        #[test]
        fn $test() {
            let (name, rows) = families().into_iter().find(|(n, _)| *n == $family).unwrap();
            run_family(name, &rows);
        }
    };
}

family_test!(oracle_valu_f32_arith, "valu_f32_arith");
family_test!(oracle_valu_move_select, "valu_move_select");
family_test!(oracle_valu_int_arith, "valu_int_arith");
family_test!(oracle_valu_bit_ops, "valu_bit_ops");
family_test!(oracle_valu_trans_round, "valu_trans_round");
family_test!(oracle_valu_cvt, "valu_cvt");
family_test!(oracle_vcmp_f32, "vcmp_f32");
family_test!(oracle_vcmp_int, "vcmp_int");
family_test!(oracle_salu_alu, "salu_alu");
family_test!(oracle_salu_cmp_select, "salu_cmp_select");

/// Every row names a GFX10 mnemonic the census has with no test naming it (or a named one; the
/// oracle then adds a second witness), so the table cannot drift from `isa-mnemonics.tsv`.
#[test]
fn every_row_is_a_gfx10_mnemonic() {
    for (family, rows) in families() {
        for row in rows {
            assert!(opcode_of(row.name).is_some(), "{family}: {}", row.name);
            let _ = body(row.name);
        }
    }
    let _ = (sgpr(0), vgpr(0));
}

/// The console's rounding of `v_cvt_f16_f32`, which `VICTORY_F16_NEAREST=1` asks for, is exact.
#[test]
fn oracle_f16_nearest_matches() {
    if Host::new(false).is_err() {
        return;
    }
    let row = cvt().into_iter().find(|r| r.name == "v_cvt_f16_f32").unwrap();
    super::lower::TEST_F16_NEAREST.set(Some(true));
    let outcome = run_row(&row);
    super::lower::TEST_F16_NEAREST.set(None);
    match outcome {
        Outcome::Passed(_) | Outcome::Refused(_) => {}
        Outcome::Mismatch { examples, .. } => panic!("{examples:?}"),
        Outcome::Untranslated(t) => panic!("{t}"),
    }
}
