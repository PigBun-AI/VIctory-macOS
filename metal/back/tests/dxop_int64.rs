//! 64-bit integer ALU (SFI0 bit 15 `Int64Ops`, 6,641 programs: 2,111 pixel,
//! 1,011 vertex, 1,643 compute, 1,876 mesh): the LLVM integer instructions at
//! i64 that DXC emits for `uint64_t` / `int64_t` HLSL, and the i64 forms of
//! the `dx.op` bit counts. Each case runs 4,096 operand pairs (edge values
//! and pseudo-random ones) through the GPU and compares every result with
//! the CPU's `u64` / `i64` arithmetic (wrapping; divisions by zero are left
//! out: the result is undefined in HLSL).
#![cfg(target_vendor = "apple")]

mod common_dxop;
mod common_msc;

use common_dxop::*;

fn operands() -> Vec<(u64, u64)> {
    let edge = [
        0u64,
        1,
        2,
        3,
        0xffff_ffff,
        0x1_0000_0000,
        0x1_0000_0001,
        0x7fff_ffff,
        0x8000_0000,
        0x7fff_ffff_ffff_ffff,
        0x8000_0000_0000_0000,
        0xffff_ffff_ffff_ffff,
        0xffff_ffff_ffff_fffe,
        0x0123_4567_89ab_cdef,
        0xfedc_ba98_7654_3210,
        63,
        64,
    ];
    let mut v = Vec::new();
    for &a in &edge {
        for &b in &edge {
            v.push((a, b));
        }
    }
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    while v.len() < 4096 {
        let a = next();
        // vary magnitude so that small operands (division, shifts) are common too
        let b = next() >> (next() % 64);
        v.push((a >> (next() % 40), b));
    }
    v
}

fn words(pairs: &[(u64, u64)]) -> Vec<u32> {
    pairs
        .iter()
        .flat_map(|&(a, b)| [a as u32, (a >> 32) as u32, b as u32, (b >> 32) as u32])
        .collect()
}

const LOAD: &str = "uint i = t.x; uint64_t a = ((uint64_t)I.Load(i*16+4) << 32) | I.Load(i*16);\n\
  uint64_t b = ((uint64_t)I.Load(i*16+12) << 32) | I.Load(i*16+8);\n\
  int64_t sa = (int64_t)a, sb = (int64_t)b;\n";

fn put(slot: usize, expr: &str) -> String {
    format!(
        "{{ uint64_t r = {expr}; O.Store2((i*16+{slot})*8, uint2((uint)r, (uint)(r >> 32))); }}\n"
    )
}

type Ran = (Vec<(u64, u64)>, Vec<u32>);

/// Runs one program over every pair and returns the words per pair.
fn run(rig: &Rig, name: &str, body: &str, n_out: usize, ops: &[&str]) -> Option<Ran> {
    let pairs = operands();
    let src = format!(
        "[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) {{\n{LOAD}{body}}}\n"
    );
    let p = rig.program(name, &src, ops)?;
    let got = rig.run(
        &p,
        [(pairs.len() / 64) as u32, 1, 1],
        [64, 1, 1],
        &[],
        &words(&pairs),
        pairs.len() * n_out * 2,
    );
    Some((pairs, got))
}

fn check(
    name: &str,
    pairs: &[(u64, u64)],
    got: &[u32],
    n_out: usize,
    want: impl Fn(u64, u64) -> Vec<Option<u64>>,
) {
    let mut bad = Vec::new();
    for (i, &(a, b)) in pairs.iter().enumerate() {
        let w = want(a, b);
        for (k, w) in w.iter().enumerate() {
            let Some(w) = w else { continue };
            let g = u64::from(got[(i * n_out + k) * 2])
                | (u64::from(got[(i * n_out + k) * 2 + 1]) << 32);
            if g != *w {
                bad.push(format!(
                    "result {k} of ({a:#x}, {b:#x}): got {g:#x} want {w:#x}"
                ));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "{name}: {} of {} results differ; first: {:?}",
        bad.len(),
        pairs.len() * n_out,
        &bad[..bad.len().min(4)]
    );
}

/// add, sub, mul, and, or, xor, not, neg: wrapping.
#[test]
fn i64_add_sub_mul_and_bitwise() {
    let rig = rig();
    let body = [
        put(0, "a + b"),
        put(1, "a - b"),
        put(2, "a * b"),
        put(3, "a & b"),
        put(4, "a | b"),
        put(5, "a ^ b"),
        put(6, "~a"),
        put(7, "(uint64_t)(-sa)"),
    ]
    .concat();
    // `put` writes slot k at word offset (i*16 + k) * 8 bytes: n_out is 16 records of 64 bits per pair
    let Some((pairs, got)) = run(&rig, "i64_arith", &body, 16, &[]) else {
        return;
    };
    check("arith", &pairs, &got, 16, |a, b| {
        vec![
            Some(a.wrapping_add(b)),
            Some(a.wrapping_sub(b)),
            Some(a.wrapping_mul(b)),
            Some(a & b),
            Some(a | b),
            Some(a ^ b),
            Some(!a),
            Some((a as i64).wrapping_neg() as u64),
        ]
    });
}

/// shl, lshr, ashr by (b & 63); and by the full 64-bit count (HLSL masks the
/// count to 6 bits, D3D11.3 spec 22.x: only the low 6 bits of the shift count
/// are used).
#[test]
fn i64_shifts_use_the_low_six_bits_of_the_count() {
    let rig = rig();
    let body = [
        put(0, "a << (b & 63)"),
        put(1, "a >> (b & 63)"),
        put(2, "(uint64_t)(sa >> (b & 63))"),
        put(3, "a << (uint)b"),
        put(4, "a >> (uint)b"),
        put(5, "(uint64_t)(sa >> (uint)b)"),
    ]
    .concat();
    let Some((pairs, got)) = run(&rig, "i64_shift", &body, 16, &[]) else {
        return;
    };
    check("shift", &pairs, &got, 16, |a, b| {
        let c = (b & 63) as u32;
        let c32 = (b as u32) & 63;
        vec![
            Some(a << c),
            Some(a >> c),
            Some(((a as i64) >> c) as u64),
            Some(a << c32),
            Some(a >> c32),
            Some(((a as i64) >> c32) as u64),
        ]
    });
}

/// Unsigned and signed compare, min, max, select, and the conversions:
/// 32 -> 64 zero and sign extension, 64 -> 32 truncation, 64-bit to and
/// from float.
#[test]
fn i64_compare_select_and_conversions() {
    let rig = rig();
    let body = [
        put(0, "(a < b) ? 1ull : 0ull"),
        put(1, "(sa < sb) ? 1ull : 0ull"),
        put(2, "(a == b) ? 1ull : 0ull"),
        put(3, "(a >= b) ? a : b"),
        put(4, "(sa >= sb) ? a : b"),
        put(5, "(uint64_t)(uint)a"),
        put(6, "(uint64_t)(int64_t)(int)(uint)a"),
        put(7, "(uint64_t)(uint)(a >> 17)"),
    ]
    .concat();
    let Some((pairs, got)) = run(&rig, "i64_cmp", &body, 16, &[]) else {
        return;
    };
    check("compare", &pairs, &got, 16, |a, b| {
        vec![
            Some(u64::from(a < b)),
            Some(u64::from((a as i64) < (b as i64))),
            Some(u64::from(a == b)),
            Some(a.max(b)),
            Some(if (a as i64) >= (b as i64) { a } else { b }),
            Some(u64::from(a as u32)),
            Some(i64::from(a as u32 as i32) as u64),
            Some(u64::from((a >> 17) as u32)),
        ]
    });
}

/// Division and remainder, unsigned and signed (not by zero; INT64_MIN / -1
/// is left out too).
#[test]
fn i64_division_and_remainder() {
    let rig = rig();
    let body = [
        "if (b != 0) {\n".to_string(),
        put(0, "a / b"),
        put(1, "a % b"),
        "}\nif (sb != 0 && !(sa == (int64_t)0x8000000000000000ll && sb == -1)) {\n".to_string(),
        put(2, "(uint64_t)(sa / sb)"),
        put(3, "(uint64_t)(sa % sb)"),
        "}\n".to_string(),
    ]
    .concat();
    let Some((pairs, got)) = run(&rig, "i64_div", &body, 16, &[]) else {
        return;
    };
    check("division", &pairs, &got, 16, |a, b| {
        let (sa, sb) = (a as i64, b as i64);
        let u = b != 0;
        let s = sb != 0 && !(sa == i64::MIN && sb == -1);
        vec![
            u.then(|| a / b),
            u.then(|| a % b),
            s.then(|| (sa / sb) as u64),
            s.then(|| (sa % sb) as u64),
        ]
    });
}

/// `countbits` and `reversebits` of a 64-bit value (`dx.op.unaryBits.i64`,
/// `dx.op.unary.i64`) compute right. The `firstbitlow` / `firstbithigh`
/// forms (`dx.op.unaryBits.i64` FirstbitLo / FirstbitHi / FirstbitSHi) are
/// not usable: MSC 4.0.3 converts them to a module Metal's compiler cannot
/// read ("Failed to materializeAll"). The census has no `unaryBits.i64`
/// (every `unaryBits` of the 13,058 programs is `.i32`), the translator
/// splits 64-bit counts into halves, so it never reaches this.
#[test]
fn i64_bit_counting_and_the_find_first_bit_defect() {
    let rig = rig();
    let body = [put(0, "countbits(a)"), put(1, "reversebits(a)")].concat();
    let Some((pairs, got)) = run(
        &rig,
        "i64_bits",
        &body,
        16,
        &["dx.op.unaryBits.i64", "dx.op.unary.i64"],
    ) else {
        return;
    };
    check("bits", &pairs, &got, 16, |a, _| {
        vec![Some(u64::from(a.count_ones())), Some(a.reverse_bits())]
    });
    for (n, e) in [
        ("firstbitlow", "firstbitlow(a)"),
        ("firstbithigh", "firstbithigh(a)"),
        ("sfirstbithigh", "firstbithigh(sa)"),
    ] {
        let src = format!(
            "[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) {{\n{LOAD}{}}}\n",
            put(0, e)
        );
        let Some(dxil) = dxil_checked(
            &format!("i64b_{n}"),
            "cs_6_6",
            &format!("{PRELUDE}{src}"),
            &["dx.op.unaryBits.i64"],
        ) else {
            return;
        };
        let r = rig.try_build(&dxil, 0);
        assert!(
            r.as_ref().is_err_and(|e| e.contains("materializeAll")),
            "{n}: {r:?}"
        );
    }
}
