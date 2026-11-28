//! dx.op nodes of the DXIL -> MSC hop that are plain per-thread arithmetic and
//! thread ids (map-plan/inventory.md, "DXIL -> MSC", the 21 dx.op without a
//! GPU case): `legacyF16ToF32`, `legacyF32ToF16`, `unaryBits.i32`, `groupId`,
//! `threadIdInGroup`, `flattenedThreadIdInGroup` and `threadId`.
//!
//! HLSL -> DXC -> DXIL -> MSC -> Metal compute -> read back -> CPU reference
//! (common_dxop.rs). Each test names the dx.op functions the DXIL has to
//! declare. Sources of the semantics:
//! - D3D11.3 Functional Spec (microsoft.github.io/DirectX-Specs, 2026-11-28):
//!   3.1.5 (16-bit float: NaN, Inf, denorms preserved), 3.2.2 (Floating Point
//!   Conversion: to a narrower float "Round-to-zero must be used"; values past
//!   the range give the largest finite value, not Inf; NaN stays NaN, Inf stays
//!   Inf, a denorm of the wider format that the narrow one cannot hold gives a
//!   signed zero), 22.13.1 f16tof32, 22.13.2 f32tof16, 22.11.3 bfrev,
//!   22.11.4 countbits, 22.11.5 firstbit.
//! - DXIL spec (github.com/microsoft/DirectXShaderCompiler docs/DXIL.rst):
//!   LegacyF16ToF32 (opcode 131), LegacyF32ToF16 (130), Bfrev (30),
//!   Countbits (31), FirstbitLo (32), FirstbitHi (33), FirstbitSHi (34).
#![cfg(target_vendor = "apple")]

mod common_dxop;
mod common_msc;

use common_dxop::*;

fn f16_to_f32_ref(h: u16) -> u32 {
    let s = u32::from(h >> 15) << 31;
    let e = u32::from((h >> 10) & 31);
    let f = u32::from(h & 0x3ff);
    match (e, f) {
        (0, 0) => s,
        (0, _) => {
            // denormal half: normalised in f32
            let mut m = f;
            let mut e32 = 127 - 15 + 1;
            while m & 0x400 == 0 {
                m <<= 1;
                e32 -= 1;
            }
            s | (e32 << 23) | ((m & 0x3ff) << 13)
        }
        (31, 0) => s | 0x7f80_0000,
        (31, _) => s | 0x7f80_0000 | (f << 13),
        _ => s | ((e + 127 - 15) << 23) | (f << 13),
    }
}

/// Round-toward-zero f32 -> f16 (D3D11.3 spec 3.2.2): the result of an
/// overflow is the largest finite half, NaN stays NaN, Inf stays Inf.
fn f32_to_f16_rtz(x: u32) -> u16 {
    let s = ((x >> 16) & 0x8000) as u16;
    let e = ((x >> 23) & 0xff) as i32;
    let f = x & 0x7f_ffff;
    if e == 255 {
        return s | 0x7c00 | if f != 0 { 0x200 | (f >> 13) as u16 } else { 0 };
    }
    let he = e - 127 + 15;
    if he >= 31 {
        return s | 0x7bff;
    }
    if he <= 0 {
        // half denormal: value = (1.f) * 2^(e-127) = m * 2^-24
        if he < -10 || e == 0 {
            return s;
        }
        let m = f | 0x80_0000;
        return s | (m >> (14 - he)) as u16;
    }
    s | ((he as u16) << 10) | (f >> 13) as u16
}

/// Round-to-nearest-even f32 -> f16 (Metal's `half(x)`, IEEE 754).
fn f32_to_f16_rne(x: u32) -> u16 {
    let s = ((x >> 16) & 0x8000) as u16;
    let e = ((x >> 23) & 0xff) as i32;
    let f = x & 0x7f_ffff;
    if e == 255 {
        return s | 0x7c00 | if f != 0 { 0x200 | (f >> 13) as u16 } else { 0 };
    }
    let he = e - 127 + 15;
    // the magnitude as an integer count of the target's last places
    let (mant, shift) = if he >= 1 {
        (u64::from(f | 0x80_0000), 13)
    } else if he < -10 {
        return s;
    } else {
        (u64::from(f | 0x80_0000), 13 + (1 - he))
    };
    let mut q = mant >> shift;
    let rem = mant & ((1u64 << shift) - 1);
    let half = 1u64 << (shift - 1);
    if rem > half || (rem == half && q & 1 == 1) {
        q += 1;
    }
    let base = if he >= 1 { ((he as u64) - 1) << 10 } else { 0 };
    let bits = base + q; // carries into the exponent as it should
    if bits >= 0x7c00 {
        return s | 0x7c00;
    }
    s | bits as u16
}

#[test]
fn legacy_f16_to_f32_matches_the_spec_for_all_65536_halves() {
    let rig = rig();
    let Some(p) = rig.program(
        "f16_to_f32_all",
        "[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) { O.Store(t.x*4, asuint(f16tof32(t.x))); }\n",
        &["dx.op.legacyF16ToF32"],
    ) else {
        return;
    };
    let got = rig.run(&p, [1024, 1, 1], [64, 1, 1], &[], &[], 65536);
    let mut bad = Vec::new();
    let mut nan_payload_kept = 0;
    for h in 0..=65535u32 {
        let want = f16_to_f32_ref(h as u16);
        let g = got[h as usize];
        let nan = (h & 0x7c00) == 0x7c00 && (h & 0x3ff) != 0;
        if nan {
            // 3.2.2: NaN must stay NaN (any payload)
            if !f32::from_bits(g).is_nan() {
                bad.push((h, g, want));
            }
            nan_payload_kept += u32::from(g == want);
        } else if g != want {
            bad.push((h, g, want));
        }
    }
    eprintln!("NaN halves whose payload comes through exactly: {nan_payload_kept} of 2046");
    assert!(
        bad.is_empty(),
        "{} halves differ, first (half, got, want): {:x?}",
        bad.len(),
        &bad[..bad.len().min(8)]
    );
}

fn f32_inputs() -> Vec<u32> {
    let mut v: Vec<u32> = vec![
        0x0000_0000, // +0
        0x8000_0000, // -0
        0x0000_0001, // smallest f32 denormal
        0x8000_0001,
        0x007f_ffff,         // largest f32 denormal
        0x0080_0000,         // smallest f32 normal
        0x3380_0000,         // 2^-24 = smallest half denormal
        0x3300_0000,         // 2^-25: half of it, a tie to zero
        0x3300_0001,         // just above the tie
        0x3380_0001,         // just above the smallest half denormal
        0x33c0_0000,         // 1.5 * 2^-24: a tie between two half denormals
        0x3800_0000,         // 2^-15 half denormal
        0x387f_c000,         // largest half denormal (1023 * 2^-24)
        0x387f_e000,         // tie between the largest denormal and 2^-14
        0x387f_f000,         // above that tie
        0x3880_0000,         // 2^-14 smallest half normal
        0x3f80_0000,         // 1
        0x3f80_1000,         // 1 + 2^-11: a tie (even below)
        0x3f80_3000,         // 1 + 3*2^-11: a tie (odd below)
        0x3f80_1001,         // above a tie
        0x3f80_0fff,         // below a tie
        0x477f_e000,         // 65504, largest half
        0x477f_efff,         // just below the tie to 65520
        0x477f_f000,         // 65520: a tie that rounds to Inf when nearest
        0x477f_ffff,         // just below 65536
        0x4780_0000,         // 65536
        0x7f7f_ffff,         // largest f32
        0x7f80_0000,         // +Inf
        0xff80_0000,         // -Inf
        0x7fc0_0000,         // quiet NaN
        0xffc0_0000,
        0x7f80_0001,         // signalling NaN, payload 1
        0x7fa0_0000,
        0x7fff_ffff,
        0xdead_beef,         // a negative NaN? (0xdeadbeef is a large negative normal)
        0xc47f_c000,         // -1023
    ];
    // a sweep: every exponent, a few mantissas, both signs
    for e in (0u32..=254).step_by(3) {
        for m in [0u32, 1, 0x1000, 0x1fff, 0x2000, 0x3000, 0x7f_e000, 0x7f_ffff] {
            v.push((e << 23) | m);
            v.push(0x8000_0000 | (e << 23) | m);
        }
    }
    // pseudo-random
    let mut s = 0x1234_5678u32;
    for _ in 0..4096 {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        v.push(s);
    }
    while v.len() % 64 != 0 {
        v.push(0x3f80_0000);
    }
    v
}

/// `f32tof16`: what Metal does, pinned, against both roundings. The spec
/// (D3D11.3 3.2.2) says round-toward-zero with overflow to the largest finite
/// value; this reports which of the two the GPU follows.
#[test]
fn legacy_f32_to_f16_edge_values_and_rounding() {
    let rig = rig();
    let Some(p) = rig.program(
        "f32_to_f16",
        "[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) { O.Store(t.x*4, f32tof16(asfloat(I.Load(t.x*4)))); }\n",
        &["dx.op.legacyF32ToF16"],
    ) else {
        return;
    };
    let input = f32_inputs();
    let n = input.len();
    let got = rig.run(&p, [(n / 64) as u32, 1, 1], [64, 1, 1], &[], &input, n);
    let (mut spec_ok, mut deviates, mut bad) = (0, Vec::new(), Vec::new());
    for (i, &x) in input.iter().enumerate() {
        let g = got[i];
        if f32::from_bits(x).is_nan() {
            // NaN must stay NaN; the upper 16 bits are zero (22.13.2)
            assert!(
                g <= 0xffff && (g & 0x7c00) == 0x7c00 && (g & 0x3ff) != 0,
                "NaN {x:#010x} -> {g:#x}"
            );
            continue;
        }
        assert_eq!(g >> 16, 0, "the upper 16 bits are zero (22.13.2): {x:#010x} -> {g:#x}");
        let rtz = u32::from(f32_to_f16_rtz(x));
        let model = u32::from(f32_to_f16_msc(x));
        if g != model {
            bad.push((x, g, model, rtz));
        }
        if g == rtz {
            spec_ok += 1;
        } else {
            deviates.push(x);
        }
    }
    let at = |x: u32| got[input.iter().position(|&v| v == x).unwrap()];
    eprintln!(
        "f32tof16 over {} inputs: {spec_ok} as the spec says (round-to-zero), {} differ from it \
         (all of them with |x| < 2^-14: {})",
        input.len(),
        deviates.len(),
        deviates.iter().all(|x| (x & 0x7fff_ffff) < 0x3880_0000)
    );
    eprintln!(
        "1+3*2^-11 -> {:#06x} (RTZ 0x3c01, RNE 0x3c02); 65520 -> {:#06x} (RTZ 0x7bff, RNE 0x7c00); 1.5*2^-24 -> {:#06x} (RTZ 1, RNE 2)",
        at(0x3f80_3000),
        at(0x477f_f000),
        at(0x33c0_0000)
    );
    assert!(bad.is_empty(), "(input, got, model, rtz): {:x?}", &bad[..bad.len().min(8)]);
    // Everything that gives a half normal is exactly the spec's.
    assert!(
        deviates.iter().all(|x| (x & 0x7fff_ffff) < 0x3880_0000),
        "a half-normal result that is not round-to-zero"
    );
    assert!(!deviates.is_empty(), "the denormal-range deviation is gone: update docs and this test");
}

/// MSC's `f32tof16` as measured (docs/metal/translation-matrix.csv.txt,
/// S-D-0xx legacyF32ToF16): the mantissa is cut to 10 bits (round toward
/// zero, which is the spec for half normals and for overflow, where the
/// result saturates to 65504), and a result that is a half denormal is then
/// rounded to nearest even from that cut value: a double rounding. The spec
/// rounds toward zero once, so the two differ by one half-denormal step
/// (2^-24) for some |x| < 2^-14.
fn f32_to_f16_msc(x: u32) -> u16 {
    let r = f32_to_f16_rne(x & !0x1fff);
    if r & 0x7fff == 0x7c00 && (x >> 23) & 0xff != 255 {
        (r & 0x8000) | 0x7bff
    } else {
        r
    }
}

/// Where the rounding comes from: Metal's own `half(x)` is round-to-nearest-even
/// for every input (IEEE 754), so the round-toward-zero of `f32tof16` is
/// MSC's doing (it emulates the D3D rule), and its denormal-range
/// double rounding is MSC's too, not the back's and not the GPU's.
#[test]
fn metal_half_conversion_is_nearest_even_so_the_f32tof16_rule_is_mscs() {
    use victory_metal_back::memory::BufferKind;
    let rig = rig();
    let msl = "#include <metal_stdlib>\nusing namespace metal;\n\
kernel void cvt(device const uint* i [[buffer(0)]], device uint* o [[buffer(1)]], uint t [[thread_position_in_grid]]) {\n\
  o[t] = as_type<ushort>(half(as_type<float>(i[t]))); }\n";
    let pso = rig.d.compute_pipeline_from_msl(msl, "cvt").unwrap();
    let input = f32_inputs();
    let n = input.len();
    let ib = rig.d.create_buffer(BufferKind::Shared, 4 * n as u64).unwrap();
    let ob = rig.d.create_buffer(BufferKind::Shared, 4 * n as u64).unwrap();
    ib.write_u32s(0, &input).unwrap();
    let mut l = rig.q.begin_list().unwrap();
    l.set_address(0, ib.gpu_va()).unwrap();
    l.set_address(1, ob.gpu_va()).unwrap();
    l.dispatch(&pso, [(n / 64) as u32, 1, 1], [64, 1, 1]).unwrap();
    l.end().unwrap();
    let v = rig.q.submit(vec![l]).unwrap();
    rig.q.wait_for(v, 20_000).unwrap();
    let got = ob.read_u32s(0, n).unwrap();
    let mut bad = Vec::new();
    for (i, &x) in input.iter().enumerate() {
        if f32::from_bits(x).is_nan() {
            continue;
        }
        let rne = u32::from(f32_to_f16_rne(x));
        if got[i] != rne {
            bad.push((x, got[i], rne));
        }
    }
    assert!(bad.is_empty(), "(input, got, rne): {:x?}", &bad[..bad.len().min(8)]);
}

fn firstbit_hi_u(x: u32) -> u32 {
    if x == 0 { u32::MAX } else { 31 - x.leading_zeros() }
}
fn firstbit_hi_s(x: i32) -> u32 {
    if x == 0 || x == -1 {
        u32::MAX
    } else if x < 0 {
        firstbit_hi_u(!x as u32)
    } else {
        firstbit_hi_u(x as u32)
    }
}
fn firstbit_lo(x: u32) -> u32 {
    if x == 0 { u32::MAX } else { x.trailing_zeros() }
}

/// countbits / reversebits / firstbitlow / firstbithigh (unsigned and signed):
/// `unaryBits.i32`. 22.11.3-22.11.5 of the D3D11.3 spec: `firstbit` returns
/// -1 for no set bit; HLSL's `firstbithigh` counts from the LSB.
#[test]
fn unary_bits_count_reverse_and_find() {
    let rig = rig();
    let Some(p) = rig.program(
        "unary_bits",
        "[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
           uint x = I.Load(t.x*4);\n\
           O.Store4(t.x*32, uint4(countbits(x), reversebits(x), firstbitlow(x), firstbithigh(x)));\n\
           O.Store4(t.x*32+16, uint4(firstbithigh((int)x), countbits((int)x), 0, 0)); }\n",
        &["dx.op.unaryBits.i32"],
    ) else {
        return;
    };
    let mut input: Vec<u32> = vec![
        0, 1, 2, 3, 0x8000_0000, 0xffff_ffff, 0xffff_fffe, 0x7fff_ffff, 0x8000_0001, 0x4000_0000,
        0xc000_0000, 0x0000_ffff, 0xffff_0000, 0x5555_5555, 0xaaaa_aaaa, 0xdead_beef, 0x0f0f_0f0f,
    ];
    for b in 0..32 {
        input.push(1 << b);
        input.push(!(1u32 << b));
        input.push((1u32 << b).wrapping_neg());
    }
    let mut s = 7u32;
    for _ in 0..512 {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        // sparse and dense words
        input.push(s);
        input.push(s & s.rotate_left(7) & s.rotate_left(19));
        input.push(s | s.rotate_left(11) | s.rotate_left(23));
    }
    while input.len() % 64 != 0 {
        input.push(0);
    }
    let n = input.len();
    let got = rig.run(&p, [(n / 64) as u32, 1, 1], [64, 1, 1], &[], &input, n * 8);
    let mut want = Vec::new();
    for &x in &input {
        want.extend_from_slice(&[
            x.count_ones(),
            x.reverse_bits(),
            firstbit_lo(x),
            firstbit_hi_u(x),
            firstbit_hi_s(x as i32),
            (x as i32).count_ones(),
            0,
            0,
        ]);
    }
    assert_words("unaryBits", &got, &want);
}

/// `groupId`, `threadIdInGroup`, `flattenedThreadIdInGroup`, `threadId`:
/// a 3D grid of 3D groups. SV_GroupIndex is x + 8y + 32z (the flattened id of
/// the DXIL spec, opcodes 94, 95, 96, 93).
#[test]
fn group_and_thread_ids_in_a_3d_dispatch() {
    let rig = rig();
    let Some(p) = rig.program(
        "ids_3d",
        "[numthreads(8,4,2)] void main(uint3 g : SV_GroupID, uint3 tg : SV_GroupThreadID, uint gi : SV_GroupIndex, uint3 dt : SV_DispatchThreadID) {\n\
           uint lin = (((g.z * 2 + g.y) * 3 + g.x) * 64) + gi;\n\
           O.Store4(lin*32, uint4(g.x, g.y, g.z, gi));\n\
           O.Store4(lin*32+16, uint4(tg.x + 100*tg.y + 10000*tg.z, dt.x, dt.y, dt.z)); }\n",
        &[
            "dx.op.groupId.i32",
            "dx.op.threadIdInGroup.i32",
            "dx.op.flattenedThreadIdInGroup.i32",
            "dx.op.threadId.i32",
        ],
    ) else {
        return;
    };
    let (gx, gy, gz) = (3u32, 2u32, 2u32);
    let total = (gx * gy * gz * 64) as usize;
    let got = rig.run(&p, [gx, gy, gz], [8, 4, 2], &[], &[], total * 8);
    let mut want = vec![0u32; total * 8];
    for z in 0..gz {
        for y in 0..gy {
            for x in 0..gx {
                for gi in 0..64u32 {
                    let (tx, ty, tz) = (gi % 8, (gi / 8) % 4, gi / 32);
                    let lin = (((z * 2 + y) * 3 + x) * 64 + gi) as usize;
                    want[lin * 8..lin * 8 + 8].copy_from_slice(&[
                        x,
                        y,
                        z,
                        gi,
                        tx + 100 * ty + 10000 * tz,
                        x * 8 + tx,
                        y * 4 + ty,
                        z * 2 + tz,
                    ]);
                }
            }
        }
    }
    assert_words("ids", &got, &want);
}
