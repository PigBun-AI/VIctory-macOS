//! Wave and quad dx.op nodes of the DXIL -> MSC hop (map-plan/inventory.md:
//! the 21 dx.op without a GPU case and the SFI0 feature `WaveOps`, 18,912
//! programs): `waveReadLaneFirst`, `waveGetLaneIndex`, `waveReadLaneAt`,
//! `waveActiveOp`, `waveActiveBit`, `waveActiveBallot`, `waveAnyTrue`,
//! `wavePrefixOp`, `quadOp`, `quadReadLaneAt`.
//!
//! Wave width: Apple GPUs run 32-lane SIMD groups (`threadExecutionWidth` 32)
//! and MSC 4.0.3 refuses `[WaveSize(64)]` (msc_features.rs). The translator
//! lowers a console wave64 program for exactly such a host: with
//! `set_host_max_wave(32)` (victory-shader emit/lower.rs, "wave64 programs in
//! host waves of 32") the DXIL it emits assumes 32-lane waves, so these tests
//! take 32 as the wave size, and check that `WaveGetLaneCount()` really says
//! 32 and that threads of a group are dealt to waves in order (lane =
//! flattened thread index % 32, wave = index / 32).
//!
//! Sources: DirectX-Specs "HLSL Derivative Operations in Compute, Mesh and
//! Amplification Shaders" (SM 6.6, v1.00 2021-04-20: quad layouts 1D/2D);
//! DXIL spec (DXIL.rst) WaveReadLaneFirst 118, WaveReadLaneAt 117,
//! WaveActiveOp 119, WaveActiveBit 120, WaveActiveAllTrue/AnyTrue 114/113,
//! WaveActiveBallot 116, WavePrefixOp 121, QuadReadLaneAt 122, QuadOp 123;
//! the HLSL "Wave Intrinsics" page of the DirectXShaderCompiler wiki
//! (inactive lanes take no part; results for inactive lanes are undefined).
#![cfg(target_vendor = "apple")]

mod common_dxop;
mod common_msc;

use common_dxop::*;

const FULL: u32 = 0xffff_ffff;

/// Active-lane masks used by the divergence tests (bit = lane of the wave).
const MASKS: [u32; 8] = [
    FULL,
    0x0000_0001,
    0x8000_0000,
    0xaaaa_aaaa,
    0x5555_5555,
    0x00ff_0f00,
    0x0000_8000,
    0x7fff_fffe,
];

fn lcg(n: usize, seed: u32) -> Vec<u32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            s
        })
        .collect()
}

/// Lanes of wave `w` in a 1D group, as thread indices.
fn lanes(w: usize) -> impl Iterator<Item = usize> {
    (0..32).map(move |l| w * 32 + l)
}

fn active(mask: u32, l: usize) -> bool {
    (mask >> l) & 1 == 1
}

#[test]
fn wave_size_is_32_and_lanes_follow_the_thread_index() {
    let rig = rig();
    let one = |nx: u32, ny: u32, nz: u32| {
        format!(
            "[numthreads({nx},{ny},{nz})] void main(uint gi : SV_GroupIndex) {{\n\
               O.Store4(gi*16, uint4(WaveGetLaneIndex(), WaveGetLaneCount(), WaveIsFirstLane() ? 1u : 0u, gi)); }}\n"
        )
    };
    for (nx, ny, nz) in [
        (128, 1, 1),
        (64, 1, 1),
        (32, 1, 1),
        (16, 8, 1),
        (8, 8, 1),
        (8, 4, 4),
    ] {
        let name = format!("lane_index_{nx}_{ny}_{nz}");
        let Some(p) = rig.program(
            &name,
            &one(nx, ny, nz),
            &["dx.op.waveGetLaneIndex", "dx.op.waveGetLaneCount"],
        ) else {
            return;
        };
        assert_eq!(p.pipeline.thread_execution_width(), 32, "the SIMD width");
        let n = (nx * ny * nz) as usize;
        let got = rig.run(&p, [1, 1, 1], [nx, ny, nz], &[], &[], n * 4);
        let mut want = Vec::new();
        for gi in 0..n as u32 {
            want.extend_from_slice(&[gi % 32, 32, u32::from(gi % 32 == 0), gi]);
        }
        assert_words(&name, &got, &want);
    }
}

fn read_first_prog(rig: &Rig, name: &str, float: bool) -> Option<Prog> {
    let (ty, wrap, unwrap) = if float {
        ("float", "asfloat", "asuint")
    } else {
        ("uint", "", "")
    };
    rig.program(
        name,
        &format!(
            "[numthreads(128,1,1)] void main(uint3 t : SV_DispatchThreadID) {{\n\
               uint lane = WaveGetLaneIndex();\n\
               {ty} v = {wrap}(I.Load(t.x*4));\n\
               uint r = 0xFFFFFFFFu;\n\
               if ((k0 >> lane) & 1) {{ r = {unwrap}(WaveReadLaneFirst(v)); }}\n\
               O.Store(t.x*4, r); }}\n"
        ),
        &[if float {
            "dx.op.waveReadLaneFirst.f32"
        } else {
            "dx.op.waveReadLaneFirst.i32"
        }],
    )
}

/// `waveReadLaneFirst`: the value of the first *active* lane, per wave, with
/// lanes switched off by a divergent branch; inactive lanes keep their marker.
#[test]
fn wave_read_lane_first_takes_the_first_active_lane_of_each_wave() {
    let rig = rig();
    let Some(p) = read_first_prog(&rig, "read_first_u32", false) else {
        return;
    };
    let input = lcg(128, 11);
    for mask in MASKS {
        let got = rig.run(&p, [1, 1, 1], [128, 1, 1], &[mask], &input, 128);
        let mut want = vec![FULL; 128];
        for w in 0..4 {
            let first = (0..32).find(|&l| active(mask, l)).unwrap();
            for l in 0..32 {
                if active(mask, l) {
                    want[w * 32 + l] = input[w * 32 + first];
                }
            }
        }
        assert_words(&format!("mask {mask:#010x}"), &got, &want);
    }
}

/// The same for float values whose bit patterns must survive the move:
/// NaN payloads, a signalling NaN, -0, +-Inf, denormals.
#[test]
fn wave_read_lane_first_keeps_float_bit_patterns() {
    let rig = rig();
    let Some(p) = read_first_prog(&rig, "read_first_f32", true) else {
        return;
    };
    let specials = [
        0x7fc0_0001u32, // quiet NaN with payload
        0x7f80_0001,    // signalling NaN
        0xffc0_1234,    // negative NaN
        0x8000_0000,    // -0
        0x0000_0000,    // +0
        0x7f80_0000,    // +Inf
        0xff80_0000,    // -Inf
        0x0000_0001,    // smallest denormal
        0x807f_ffff,    // largest negative denormal
        0x0080_0000,    // smallest normal
    ];
    for (i, &sp) in specials.iter().enumerate() {
        // each wave's first lane (lane 0 under FULL, lane 3 under 0x8) holds `sp`
        let mut input = lcg(128, 5 + i as u32);
        for w in 0..4 {
            input[w * 32] = sp;
            input[w * 32 + 3] = sp;
        }
        for mask in [FULL, 0x0000_0008] {
            let got = rig.run(&p, [1, 1, 1], [128, 1, 1], &[mask], &input, 128);
            for w in 0..4 {
                for l in 0..32 {
                    if active(mask, l) {
                        assert_eq!(
                            got[w * 32 + l],
                            sp,
                            "wave {w} lane {l} mask {mask:#x}: first-lane value {sp:#010x} came out as {:#010x}",
                            got[w * 32 + l]
                        );
                    }
                }
            }
        }
    }
}

/// `waveReadLaneAt` with a lane index that is the same everywhere, and with
/// one that differs per lane (SM 6.7 allows it; MSC lowers both to a shuffle).
#[test]
fn wave_read_lane_at_uniform_and_per_lane_indices() {
    let rig = rig();
    let Some(p) = rig.program(
        "read_lane_at",
        "[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
           uint lane = WaveGetLaneIndex();\n\
           uint v = I.Load(t.x*4);\n\
           uint a = WaveReadLaneAt(v, k0);\n\
           uint b = WaveReadLaneAt(v, (lane * 7 + k1) & 31);\n\
           O.Store2(t.x*8, uint2(a, b)); }\n",
        &["dx.op.waveReadLaneAt.i32"],
    ) else {
        return;
    };
    let input = lcg(64, 99);
    for (k0, k1) in [(0u32, 0u32), (5, 3), (31, 17), (16, 31)] {
        let got = rig.run(&p, [1, 1, 1], [64, 1, 1], &[k0, k1], &input, 128);
        let mut want = vec![0u32; 128];
        for w in 0..2 {
            for l in 0..32 {
                let t = w * 32 + l;
                want[t * 2] = input[w * 32 + k0 as usize];
                want[t * 2 + 1] = input[w * 32 + ((l * 7 + k1 as usize) & 31)];
            }
        }
        assert_words(&format!("k0={k0} k1={k1}"), &got, &want);
    }
}

fn reduce_u(op: &str, vals: &[u32]) -> u32 {
    match op {
        "sum" => vals.iter().fold(0u32, |a, &b| a.wrapping_add(b)),
        "prod" => vals.iter().fold(1u32, |a, &b| a.wrapping_mul(b)),
        "min" => *vals.iter().min().unwrap(),
        "max" => *vals.iter().max().unwrap(),
        "and" => vals.iter().fold(!0u32, |a, &b| a & b),
        "or" => vals.iter().fold(0u32, |a, &b| a | b),
        "xor" => vals.iter().fold(0u32, |a, &b| a ^ b),
        _ => unreachable!(),
    }
}

fn reduce_i(op: &str, vals: &[u32]) -> u32 {
    let v: Vec<i32> = vals.iter().map(|&x| x as i32).collect();
    (match op {
        "sum" => v.iter().fold(0i32, |a, &b| a.wrapping_add(b)),
        "prod" => v.iter().fold(1i32, |a, &b| a.wrapping_mul(b)),
        "min" => *v.iter().min().unwrap(),
        "max" => *v.iter().max().unwrap(),
        _ => unreachable!(),
    }) as u32
}

/// `waveActiveOp.i32` (sum, product, min, max; unsigned and signed) and
/// `waveActiveBit.i32` (and, or, xor) over the active lanes only.
#[test]
fn wave_active_ops_over_active_lanes() {
    let rig = rig();
    let Some(p) = rig.program(
        "active_ops",
        "[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
           uint lane = WaveGetLaneIndex();\n\
           uint v = I.Load(t.x*4);\n\
           if ((k0 >> lane) & 1) {\n\
             O.Store4(t.x*64, uint4(WaveActiveSum(v), WaveActiveProduct(v), WaveActiveMin(v), WaveActiveMax(v)));\n\
             int s = (int)v;\n\
             O.Store4(t.x*64+16, uint4((uint)WaveActiveSum(s), (uint)WaveActiveProduct(s), (uint)WaveActiveMin(s), (uint)WaveActiveMax(s)));\n\
             O.Store4(t.x*64+32, uint4(WaveActiveBitAnd(v), WaveActiveBitOr(v), WaveActiveBitXor(v), 0)); } }\n",
        &[
            "dx.op.waveActiveOp.i32",
            "dx.op.waveActiveBit.i32",
        ],
    ) else {
        return;
    };
    let mut input = lcg(64, 321);
    // a few wave-wide extremes
    input[3] = 0;
    input[40] = 0x8000_0000;
    input[41] = 0x7fff_ffff;
    input[10] = 0xffff_ffff;
    for mask in MASKS {
        let got = rig.run(&p, [1, 1, 1], [64, 1, 1], &[mask], &input, 64 * 16);
        let mut want = vec![0xDEAD_BEEFu32; 64 * 16];
        for w in 0..2 {
            let vals: Vec<u32> = lanes(w)
                .filter(|t| active(mask, t % 32))
                .map(|t| input[t])
                .collect();
            for t in lanes(w).filter(|t| active(mask, t % 32)) {
                want[t * 16..t * 16 + 12].copy_from_slice(&[
                    reduce_u("sum", &vals),
                    reduce_u("prod", &vals),
                    reduce_u("min", &vals),
                    reduce_u("max", &vals),
                    reduce_i("sum", &vals),
                    reduce_i("prod", &vals),
                    reduce_i("min", &vals),
                    reduce_i("max", &vals),
                    reduce_u("and", &vals),
                    reduce_u("or", &vals),
                    reduce_u("xor", &vals),
                    0,
                ]);
            }
        }
        assert_words(&format!("mask {mask:#010x}"), &got, &want);
    }
}

/// `waveActiveOp.f32`: sums, products, min and max in float, with the values
/// IEEE gives special meaning. Sums are exact (small integers); the special
/// cases are checked in classes (NaN stays NaN, Inf+(-Inf) is NaN, Inf
/// wins a sum and a max).
#[test]
fn wave_active_float_ops_and_special_values() {
    let rig = rig();
    let Some(p) = rig.program(
        "active_ops_f32",
        "[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
           float v = asfloat(I.Load(t.x*4));\n\
           O.Store4(t.x*16, uint4(asuint(WaveActiveSum(v)), asuint(WaveActiveProduct(v)), asuint(WaveActiveMin(v)), asuint(WaveActiveMax(v)))); }\n",
        &["dx.op.waveActiveOp.f32"],
    ) else {
        return;
    };
    let f = |x: f32| x.to_bits();
    let run = |input: &[u32]| {
        let got = rig.run(&p, [1, 1, 1], [32, 1, 1], &[], input, 32 * 4);
        // every lane of the wave gets the same four words
        for l in 1..32 {
            assert_eq!(
                &got[l * 4..l * 4 + 4],
                &got[0..4],
                "lane {l} differs from lane 0"
            );
        }
        [got[0], got[1], got[2], got[3]]
    };
    // small integers: exact in any order
    let ints: Vec<u32> = (0..32).map(|i| f((i as f32 - 10.0) * 0.5)).collect();
    let r = run(&ints);
    let vals: Vec<f32> = (0..32).map(|i| (i as f32 - 10.0) * 0.5).collect();
    assert_eq!(r[0], f(vals.iter().sum::<f32>()), "sum");
    assert_eq!(r[2], f(-5.0), "min");
    assert_eq!(r[3], f(10.5), "max");
    // product of ones and one 3.0 and one -2.0
    let mut prod = vec![f(1.0); 32];
    prod[5] = f(3.0);
    prod[9] = f(-2.0);
    assert_eq!(run(&prod)[1], f(-6.0), "product");
    // +Inf wins a sum and a max; -Inf wins a min
    let mut inf = ints.clone();
    inf[7] = 0x7f80_0000;
    inf[20] = 0xff80_0000;
    let r = run(&inf);
    assert!(
        f32::from_bits(r[0]).is_nan(),
        "Inf + -Inf is NaN: {:#x}",
        r[0]
    );
    assert_eq!(r[2], 0xff80_0000, "min with -Inf");
    assert_eq!(r[3], 0x7f80_0000, "max with +Inf");
    let mut one_inf = ints.clone();
    one_inf[7] = 0x7f80_0000;
    assert_eq!(run(&one_inf)[0], 0x7f80_0000, "sum with +Inf");
    // Denormals: D3D11.3 spec 3.2.2 counts 32-bit floats as a format without a
    // denorm representation (arithmetic on them flushes to a signed zero), so
    // the sum of 32 smallest denormals (exactly 0x210) may be 0; min and max
    // only select an operand and may pass a denormal through. Measured on the
    // M5 Max: the sum is 0, min and max keep the denormal.
    let den: Vec<u32> = (0..32).map(|i| 1 + i as u32).collect();
    let r = run(&den);
    eprintln!(
        "denormal wave sum {:#x} (exact 0x210), min {:#x} (exact 1), max {:#x} (exact 0x20)",
        r[0], r[2], r[3]
    );
    assert!(r[0] == 0 || r[0] == 0x210, "denormal sum {:#x}", r[0]);
    assert!(r[2] == 0 || r[2] == 1, "denormal min {:#x}", r[2]);
    assert!(
        r[3] == 0 || r[3] == 0x20 || r[3] == 1,
        "denormal max {:#x}",
        r[3]
    );
    // A NaN lane: the sum is NaN. Min and max ignore the NaN, as DXIL's FMin
    // and FMax do (DXIL.rst: "if one operand is NaN, the other is returned");
    // measured: the wave's min is -5.0 and its max 10.5, the NaN lane absent.
    let mut nan = ints.clone();
    nan[12] = 0x7fc0_0000;
    let r = run(&nan);
    assert!(f32::from_bits(r[0]).is_nan(), "sum with NaN is NaN");
    assert_eq!(
        (r[2], r[3]),
        (f(-5.0), f(10.5)),
        "min/max skip the NaN lane"
    );
}

/// `waveActiveBallot` (all four dwords: lanes past the wave width are zero),
/// `waveAnyTrue`, `waveAllTrue`, `WaveActiveCountBits` and the prefix ops
/// `wavePrefixOp` over active lanes.
#[test]
fn ballot_any_all_count_and_prefix() {
    let rig = rig();
    let Some(p) = rig.program(
        "ballot_prefix",
        "[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
           uint lane = WaveGetLaneIndex();\n\
           uint v = I.Load(t.x*4);\n\
           if ((k0 >> lane) & 1) {\n\
             uint4 b = WaveActiveBallot((v & 3) == 1);\n\
             O.Store4(t.x*64, b);\n\
             O.Store4(t.x*64+16, uint4(WaveActiveAnyTrue((v & 7) == 5) ? 1u : 0u, WaveActiveAllTrue((v & 1) == 1) ? 1u : 0u, WaveActiveCountBits((v & 3) == 1), WaveActiveAllEqual(v & 0xf0) ? 1u : 0u));\n\
             O.Store4(t.x*64+32, uint4(WavePrefixSum(v), WavePrefixProduct(v | 1), WavePrefixCountBits((v & 3) == 1), 0)); } }\n",
        &[
            "dx.op.waveActiveBallot",
            "dx.op.waveAnyTrue",
            "dx.op.waveAllTrue",
            "dx.op.wavePrefixOp.i32",
        ],
    ) else {
        return;
    };
    let mut input = lcg(64, 8);
    // wave 1: all lanes odd (AllTrue), same high nibble (AllEqual)
    for x in &mut input[32..64] {
        *x = (*x & 0xf) | 0x131;
        *x = (*x & !0xf0) | 0x30;
        *x |= 1;
    }
    for mask in MASKS {
        let got = rig.run(&p, [1, 1, 1], [64, 1, 1], &[mask], &input, 64 * 16);
        let mut want = vec![0xDEAD_BEEFu32; 64 * 16];
        for w in 0..2 {
            let act: Vec<usize> = lanes(w).filter(|t| active(mask, t % 32)).collect();
            let ballot = act
                .iter()
                .filter(|&&t| input[t] & 3 == 1)
                .fold(0u32, |m, &t| m | (1 << (t % 32)));
            let any = act.iter().any(|&t| input[t] & 7 == 5);
            let all = act.iter().all(|&t| input[t] & 1 == 1);
            let alleq = act.iter().all(|&t| input[t] & 0xf0 == input[act[0]] & 0xf0);
            for &t in &act {
                let before: Vec<usize> = act.iter().copied().filter(|&u| u < t).collect();
                want[t * 16..t * 16 + 12].copy_from_slice(&[
                    ballot,
                    0,
                    0,
                    0,
                    u32::from(any),
                    u32::from(all),
                    ballot.count_ones(),
                    u32::from(alleq),
                    before.iter().fold(0u32, |a, &u| a.wrapping_add(input[u])),
                    before
                        .iter()
                        .fold(1u32, |a, &u| a.wrapping_mul(input[u] | 1)),
                    before.iter().filter(|&&u| input[u] & 3 == 1).count() as u32,
                    0,
                ]);
            }
        }
        assert_words(&format!("mask {mask:#010x}"), &got, &want);
    }
}

/// `quadOp` (QuadReadAcrossX/Y/Diagonal) and `quadReadLaneAt` in a 1D group
/// of 64: every four consecutive lanes are a quad, lane 0 upper left, 1 upper
/// right, 2 lower left, 3 lower right (SM 6.6 derivatives spec, "1D Quads").
#[test]
fn quad_ops_in_a_1d_group() {
    let rig = rig();
    let Some(p) = rig.program(
        "quad_1d",
        "[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
           uint v = I.Load(t.x*4);\n\
           O.Store4(t.x*32, uint4(QuadReadAcrossX(v), QuadReadAcrossY(v), QuadReadAcrossDiagonal(v), QuadReadLaneAt(v, k0)));\n\
           O.Store4(t.x*32+16, uint4(QuadReadLaneAt(v, (t.x + k1) & 3), asuint(QuadReadAcrossX(asfloat(v))), 0, 0)); }\n",
        &["dx.op.quadOp.i32", "dx.op.quadReadLaneAt.i32"],
    ) else {
        return;
    };
    let input = lcg(64, 77);
    for (k0, k1) in [(0u32, 0u32), (1, 1), (2, 2), (3, 3)] {
        let got = rig.run(&p, [1, 1, 1], [64, 1, 1], &[k0, k1], &input, 64 * 8);
        let mut want = vec![0u32; 64 * 8];
        for t in 0..64usize {
            let q = t & !3;
            let i = t & 3;
            want[t * 8..t * 8 + 8].copy_from_slice(&[
                input[q + (i ^ 1)],
                input[q + (i ^ 2)],
                input[q + (i ^ 3)],
                input[q + k0 as usize],
                input[q + ((t + k1 as usize) & 3)],
                input[q + (i ^ 1)],
                0,
                0,
            ]);
        }
        assert_words(&format!("k0={k0} k1={k1}"), &got, &want);
    }
}

/// The 2D quad rule of the SM 6.6 derivatives spec: in an 8x8 group the quad
/// of thread (x, y) is the 2x2 block of (x & ~1, y & ~1), whatever lane
/// indices the threads get. What Metal's quads are is reported; the test
/// pins the outcome (see S-D-0xx `quadOp` in the matrix).
#[test]
fn quad_ops_in_a_2d_group() {
    let rig = rig();
    let Some(p) = rig.program(
        "quad_2d",
        "[numthreads(8,8,1)] void main(uint3 tg : SV_GroupThreadID, uint gi : SV_GroupIndex) {\n\
           uint v = I.Load(gi*4);\n\
           O.Store4(gi*16, uint4(QuadReadAcrossX(v), QuadReadAcrossY(v), QuadReadAcrossDiagonal(v), WaveGetLaneIndex())); }\n",
        &["dx.op.quadOp.i32"],
    ) else {
        return;
    };
    // The value of thread (x, y) names its place: 1000 + y*8 + x.
    let input: Vec<u32> = (0..64u32).map(|gi| 1000 + gi).collect();
    let got = rig.run(&p, [1, 1, 1], [8, 8, 1], &[], &input, 64 * 4);
    let (mut spec_ok, mut lane_pair_ok) = (0, 0);
    let mut sample = Vec::new();
    for gi in 0..64usize {
        let (x, y) = (gi % 8, gi / 8);
        let at = |xx: usize, yy: usize| 1000 + (yy * 8 + xx) as u32;
        let spec = [at(x ^ 1, y), at(x, y ^ 1), at(x ^ 1, y ^ 1)];
        let g = &got[gi * 4..gi * 4 + 3];
        if g == spec {
            spec_ok += 1;
        }
        // the 1D rule on the flattened index (lane quads of 4 consecutive lanes)
        let q = gi & !3;
        let i = gi & 3;
        let lin = [
            1000 + (q + (i ^ 1)) as u32,
            1000 + (q + (i ^ 2)) as u32,
            1000 + (q + (i ^ 3)) as u32,
        ];
        if g == lin {
            lane_pair_ok += 1;
        }
        if gi < 10 {
            sample.push((gi, g.to_vec(), got[gi * 4 + 3]));
        }
    }
    eprintln!(
        "2D 8x8 group, QuadReadAcross: {spec_ok}/64 threads follow the 2D rule of the spec, \
         {lane_pair_ok}/64 follow four consecutive lanes; first threads (gi, X/Y/Diag, lane): {sample:?}"
    );
    // Measured: MSC deals the threads to lanes in quad (Z) order when a 2D group uses quad ops, so the
    // 2D rule of the spec holds; lane indices there are not the flattened index (the spec: "no relation ... should be assumed").
    assert_eq!(spec_ok, 64, "the 2D quad rule of the spec");
}

/// The idiom the translator builds around `waveReadLaneFirst` for a console
/// wave64 program in host waves of 32 (victory-shader emit/lower.rs,
/// `wave_base` and `console_thread`): every thread zeroes an LDS counter, the
/// group meets, each wave's first lane adds 32 to it, and the wave takes the
/// value that add returned (read from the first lane) as the base of its 32
/// console threads; the console thread is base + `waveGetLaneIndex`. The
/// other form (no LDS counter) is `(WaveReadLaneFirst(flattened id) & ~31) +
/// lane`. Each wave must get a run of 32 consecutive console threads, and
/// across the group the numbers 0..N must each appear once.
#[test]
fn the_translators_wave_base_idiom_numbers_every_thread_once() {
    let rig = rig();
    let Some(p) = rig.program(
        "wave_base_idiom",
        "groupshared uint cnt;\n\
         [numthreads(128,1,1)] void main(uint gi : SV_GroupIndex, uint3 g : SV_GroupID) {\n\
           cnt = 0;\n\
           GroupMemoryBarrierWithGroupSync();\n\
           uint lane = WaveGetLaneIndex();\n\
           uint add = (lane == 0) ? 32u : 0u;\n\
           uint old; InterlockedAdd(cnt, add, old);\n\
           uint base = WaveReadLaneFirst(old);\n\
           uint ct = base + lane;\n\
           uint first = WaveReadLaneFirst(gi) & ~31u;\n\
           uint ct2 = first + lane;\n\
           O.Store2((g.x*128 + gi)*8, uint2(ct, ct2)); }\n",
        &[
            "dx.op.waveReadLaneFirst.i32",
            "dx.op.waveGetLaneIndex",
            "dx.op.barrier", // the LDS add is an LLVM `atomicrmw`, not a dx.op
        ],
    ) else {
        return;
    };
    let groups = 16usize;
    let got = rig.run(
        &p,
        [groups as u32, 1, 1],
        [128, 1, 1],
        &[],
        &[],
        groups * 128 * 2,
    );
    for g in 0..groups {
        let mut seen = [false; 128];
        for gi in 0..128usize {
            let ct = got[(g * 128 + gi) * 2] as usize;
            let ct2 = got[(g * 128 + gi) * 2 + 1] as usize;
            assert!(
                ct < 128 && !seen[ct],
                "group {g}: console thread {ct} of thread {gi} out of range or twice"
            );
            seen[ct] = true;
            assert_eq!(
                ct % 32,
                gi % 32,
                "group {g} thread {gi}: the lane inside the run"
            );
            assert_eq!(ct / 32 == gi / 32, ct / 32 == gi / 32);
            assert_eq!(
                ct2, gi,
                "group {g}: the LDS-free numbering is the thread index"
            );
        }
        // each wave's lanes share one run of 32
        for w in 0..4 {
            let run = got[(g * 128 + w * 32) * 2] as usize / 32;
            for l in 0..32 {
                assert_eq!(
                    got[(g * 128 + w * 32 + l) * 2] as usize / 32,
                    run,
                    "group {g} wave {w}"
                );
            }
        }
    }
}
