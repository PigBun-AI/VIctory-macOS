//! mac F1: the rounding of 16-bit float results on the hardware host, over every input.
//!
//! The console rounds the result of `v_cvt_f16_f32`, `v_add_f16`, `v_fma_mixlo_f16` ... to
//! nearest, ties to even (the MODE default). The host's `f32tof16` may round toward zero
//! (NVIDIA's does; Metal Shader Converter's does, D3D11.3 spec 3.2.2 allows it), so the
//! translator rounds by itself ([`super::lower::f16_nearest`]). These tests run the translated
//! programs of three instructions over the whole input space and compare each result with the
//! exact nearest-even rounding computed on the CPU:
//!
//! * `v_cvt_f16_f32`: all 2^32 f32 inputs (`VICTORY_F16_CHUNKS=0-255`, in segments; without it a
//!   1/32 sample: 8 of the 256 chunks of 2^24, 134 million inputs).
//! * `v_add_f16`: all 2^32 pairs of halves (same switch).
//! * `v_fma_mixlo_f16` with f32 operands, `fma(x, 1, 0)`: all f32 inputs.
//!
//! With the switch off the same programs must differ from the reference (they round toward
//! zero where the host does): that is what makes the passing run mean something.
//! Both forms of the nearest rounding are run: the 12-operation sequence and the one `fptrunc`
//! (`VICTORY_F16_NATIVE=1`, or `VICTORY_METAL_OUT` set; the tests force the form themselves through
//! `lower::TEST_F16_NATIVE`, so neither variable is needed to run them).
//! A Mac without the executor has no device: the tests skip (see `host_exec.rs`).

use super::host::{Dispatch, Host, Root, View};
use super::{HostShader, lower};
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{io, ir, resource, structure};

const S_ENDPGM: u32 = 0xBF81_0000;
/// Threads per group, groups per chunk: a chunk is 2^24 inputs.
const THREADS: u32 = 1024;
const GROUPS: u32 = 1 << 14;
const CHUNK: u64 = 1 << 24;

const fn sop2(op: u8, sdst: u8, src0: u8, src1: u8) -> u32 {
    0x8000_0000 | (op as u32 & 0x7F) << 23 | (sdst as u32 & 0x7F) << 16 | (src1 as u32) << 8 | src0 as u32
}

const fn s_mov_b32(sdst: u8, src0: u8) -> u32 {
    0xBE80_0000 | (sdst as u32) << 16 | 0x03 << 8 | src0 as u32
}

const fn vop1(op: u16, vdst: u8, src0: u16) -> u32 {
    0x7E00_0000 | (vdst as u32) << 17 | (op as u32) << 9 | (src0 as u32 & 0x1FF)
}

const fn vop2(op: u8, vdst: u8, src0: u16, vsrc1: u8) -> u32 {
    (op as u32) << 25 | (vdst as u32) << 17 | (vsrc1 as u32) << 9 | (src0 as u32 & 0x1FF)
}

const fn vgpr(n: u8) -> u16 {
    256 + n as u16
}

/// The program: lane `i` of the dispatch holds `x = s0 + i` (s0: the chunk's first input) in
/// v1, `body` turns v1 into the result in v2, and the result is stored at dword `i`.
/// `body` may use v3.
fn build(body: &[u32]) -> NativeProgram {
    let mut words = vec![
        s_mov_b32(5, 0x80), // s5 = 0: the table address's high half
        0x3D << 26 | 0x02 << 18 | 8 << 6 | (4 >> 1),
        125 << 25, // s_load_dwordx4 s[8:11], s[4:5], 0
        sop2(0x1e, 7, 6, 128 + 10), // s_lshl_b32 s7, s6, 10   (s6: the group's x)
        vop2(0x25, 5, 7, 0),        // v_add_nc_u32 v5, s7, v0   (v5: the lane's index)
        vop2(0x25, 1, 0, 5),        // v_add_nc_u32 v1, s0, v5   (v1: its input)
        vop1(1, 2, 128),            // v_mov_b32 v2, 0
        vop2(0x1a, 4, 130, 5),      // v_lshlrev_b32 v4, 2, v5
    ];
    words.extend_from_slice(body);
    // buffer_store_dword v2, v4, s[8:11], 0 offen
    words.extend_from_slice(&[0x38 << 26 | 0x1C << 18 | 1 << 12, 0x80 << 24 | (8 >> 2) << 16 | 2 << 8 | 4]);
    words.push(S_ENDPGM);
    NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
        vgprs: 8,
        layout: vec![
            RootParam {
                root: RootType::Constants,
                table_offset_dwords: 0,
                sgprs: 4,
                base_register: 0,
                user_data: 0,
                slot: 0,
            },
            RootParam {
                root: RootType::Table,
                table_offset_dwords: 0,
                sgprs: 1,
                base_register: 0,
                user_data: 4,
                slot: 1,
            },
        ],
        used: vec![UsedResource {
            kind: ResourceType::Uav,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 0,
            dwords: 4,
            user_data: 4,
            slot: 1,
        }],
        io: StageIo::Cs(ComputeIo {
            // 6 user SGPRs (s0..s5), then the group's x in s6
            rsrc2: 6 << 1 | 1 << 7,
            threads: [THREADS, 1, 1],
        }),
        code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
    }
}

fn lowered(native: &NativeProgram, nearest: bool) -> HostShader {
    lowered_as(native, nearest, false)
}

/// `native`: the nearest rounding as one `fptrunc` (native 16-bit module), else the dozen operations.
fn lowered_as(native: &NativeProgram, nearest: bool, fptrunc: bool) -> HostShader {
    let p = ir::build(&native.code, native.wave64, native.vgprs).expect("ir::build");
    let st = structure::structure(&p).expect("structure");
    let res = resource::track(&p, &native.layout, native.stage, Some(&st)).expect("resource::track");
    let io = io::raise(&p, native).expect("io::raise");
    let was = lower::TEST_F16_NEAREST.replace(Some(nearest));
    let was_native = lower::TEST_F16_NATIVE.replace(Some(fptrunc));
    let lowered = lower::lower(&lower::Parts {
        native,
        p: &p,
        st: &st,
        io: &io,
        res: &res,
        inputs: Vec::new(),
        outputs: Vec::new(),
        prims: Vec::new(),
        mesh_stride: 0,
        cs: None, // mac S5
    });
    lower::TEST_F16_NEAREST.set(was);
    lower::TEST_F16_NATIVE.set(was_native);
    let lowered = lowered.expect("lower");
    if let Some(dir) = std::env::var_os("VICTORY_F16_DUMP") {
        let _ = std::fs::write(std::path::Path::new(&dir).join(format!("cvt-{nearest}-{fptrunc}.dxil")), &lowered.dxil);
    }
    HostShader {
        stage: HwStage::Cs,
        dxil: lowered.dxil,
        user_data: lowered.user_data,
        samplers: lowered.samplers,
        static_samplers: Vec::new(),
        root_cbvs: lowered.root_cbvs,
        root_srvs: lowered.root_srvs,
        root_uavs: lowered.root_uavs,
        root_indexed_views: res.root_indexed_views,
        vertex_reads: Vec::new(),
    }
}

/// `v`, a finite or infinite number of at most 53 significant bits, as the nearest half, ties to
/// even; the sign of zero kept. NaN is the caller's.
fn half_nearest(v: f64) -> u16 {
    let sign = if v.is_sign_negative() { 0x8000u16 } else { 0 };
    let a = v.abs();
    if a == 0.0 {
        return sign;
    }
    if a.is_infinite() {
        return sign | 0x7C00;
    }
    let b = a.to_bits();
    let e = ((b >> 52) & 0x7FF) as i32 - 1023;
    let m = (1u64 << 52) | (b & ((1 << 52) - 1));
    let (q, shift) = if e >= -14 {
        (m >> 42, 42)
    } else {
        let shift = 42 + (-14 - e);
        (if shift >= 64 { 0 } else { m >> shift }, shift)
    };
    let (rest, half) = if shift >= 64 { (m, u64::MAX) } else { (m & ((1u64 << shift) - 1), 1u64 << (shift - 1)) };
    let up = u64::from(rest > half || (rest == half && q & 1 == 1));
    let h = if e >= -14 { (((e + 15) as u64) << 10) + (q - 1024) + up } else { q + up };
    sign | if h >= 0x7C00 { 0x7C00 } else { h as u16 }
}

fn half_value(h: u16) -> f64 {
    let s = if h >> 15 == 1 { -1.0 } else { 1.0 };
    let e = i32::from((h >> 10) & 31);
    let m = f64::from(h & 1023);
    if e == 0 {
        s * m * 2f64.powi(-24)
    } else if e == 31 {
        if m == 0.0 { s * f64::INFINITY } else { f64::NAN }
    } else {
        s * (1.0 + m / 1024.0) * 2f64.powi(e - 15)
    }
}

fn is_nan16(h: u16) -> bool {
    h & 0x7FFF > 0x7C00
}

#[test]
fn the_reference_rounds_the_known_cases() {
    // 0x3C03 + 0x4000 = 3 + 1.5 steps of 2^-9: a tie, to the even 0x4202 (the console's value); toward zero 0x4201
    assert_eq!(half_nearest(3.0029296875), 0x4202);
    assert_eq!(half_nearest(65519.0), 0x7BFF);
    assert_eq!(half_nearest(65520.0), 0x7C00);
    assert_eq!(half_nearest(2f64.powi(-25)), 0); // a tie between 0 and the least denormal: even
    assert_eq!(half_nearest(1.5 * 2f64.powi(-24)), 2); // a tie between 1 and 2: even
    assert_eq!(half_nearest(1.25 * 2f64.powi(-24)), 1);
    assert_eq!(half_nearest(-1e-30), 0x8000);
    assert_eq!(half_nearest(f64::from(f32::from_bits(0x33002000))), 1);
    for h in 0..=0xFFFFu16 {
        if !is_nan16(h) {
            assert_eq!(half_nearest(half_value(h)), h, "{h:#x}");
        }
    }
}

/// Which chunks to run: `VICTORY_F16_CHUNKS=a-b,c,d-e` (ranges and single chunks of the 256 chunks of
/// 2^24 inputs; `0-255` is every input, run in segments), else every 32nd.
fn chunks() -> Vec<u64> {
    let Some(list) = std::env::var("VICTORY_F16_CHUNKS").ok() else {
        return (0..256).step_by(32).collect();
    };
    let mut out = Vec::new();
    for part in list.split(',') {
        match part.split_once('-') {
            Some((a, b)) => out.extend(a.parse::<u64>().unwrap()..=b.parse::<u64>().unwrap().min(255)),
            None => out.push(part.parse().unwrap()),
        }
    }
    out
}

/// The run of a range: the switch-off runs that show the proof can fail are made on the sample only.
fn sample_run() -> bool {
    std::env::var_os("VICTORY_F16_CHUNKS").is_none()
}

/// Runs `shader` over the chunks; `check(x, got)` returns whether the result `got` (a dword) of
/// input `x` is right. Returns `(inputs checked, mismatches, first few mismatches)`, or `None`
/// without a device.
fn sweep(shader: &HostShader, mut check: impl FnMut(u32, u32) -> bool) -> Option<(u64, u64, Vec<(u32, u32)>)> {
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let host = match Host::new(false) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("no SM 6.6 hardware device ({e}); not run");
            return None;
        }
    };
    let (root, pso) = match host.compute_pipeline(shader) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("the hardware device does not take this program: {e}; not run");
            return None;
        }
    };
    let (mut seen, mut bad, mut first) = (0u64, 0u64, Vec::new());
    for chunk in chunks() {
        let base = (chunk * CHUNK) as u32;
        let mut ud = vec![0u32; shader.user_data_dwords().max(1) as usize];
        for u in &shader.user_data {
            ud[u.index as usize] = match (u.root, u.slot) {
                (RootType::Constants, 0) => {
                    if u.dword == 0 {
                        base
                    } else {
                        0
                    }
                }
                (RootType::Table, 1) => 0,
                other => panic!("unexpected user data {other:?}"),
            };
        }
        let out = host
            .dispatch(
                &root,
                &pso,
                &Dispatch {
                    roots: vec![(0, Root::Constants(ud))],
                    views: &[View::RawUav(CHUNK as u32)],
                    samplers: &[],
                    groups: [GROUPS, 1, 1],
                },
            )
            .unwrap_or_else(|e| panic!("dispatch failed: {e}"));
        for (i, &got) in out[0].iter().enumerate() {
            let x = base.wrapping_add(i as u32);
            seen += 1;
            if !check(x, got) {
                bad += 1;
                if first.len() < 6 {
                    first.push((x, got));
                }
            }
        }
    }
    Some((seen, bad, first))
}

/// Whether `got` (the low 16 bits) is the nearest half of the f32 `x`. A NaN input gives some
/// NaN; a negative f32 denormal may give either zero (hosts flush it).
fn cvt_ok(x: u32, got: u32) -> bool {
    let f = f32::from_bits(x);
    let h = got as u16;
    if f.is_nan() {
        return is_nan16(h);
    }
    let want = half_nearest(f64::from(f));
    if x & 0x7F80_0000 == 0 {
        return h & 0x7FFF == 0;
    }
    h == want
}

fn run_cvt(name: &str, body: &[u32], nearest: bool) -> Option<(u64, u64, Vec<(u32, u32)>)> {
    run_cvt_as(name, body, nearest, false)
}

fn run_cvt_as(name: &str, body: &[u32], nearest: bool, fptrunc: bool) -> Option<(u64, u64, Vec<(u32, u32)>)> {
    let shader = lowered_as(&build(body), nearest, fptrunc);
    let r = sweep(&shader, cvt_ok)?;
    eprintln!("{name}, nearest {nearest}, fptrunc {fptrunc}: {} inputs, {} differ from the nearest half; first {:x?}", r.0, r.1, r.2);
    Some(r)
}

#[test]
fn v_cvt_f16_f32_rounds_to_nearest_even_for_every_f32() {
    let body = [vop1(0x0a, 2, vgpr(1))]; // v_cvt_f16_f32 v2, v1
    let Some(on) = run_cvt("v_cvt_f16_f32", &body, true) else { return };
    assert_eq!(on.1, 0, "differences from the nearest half: {:x?}", on.2);
    let fp = run_cvt_as("v_cvt_f16_f32", &body, true, true).expect("the same device");
    assert_eq!(fp.1, 0, "fptrunc: differences from the nearest half: {:x?}", fp.2);
    // the host's own conversion differs from it (toward zero), or this proves nothing
    if !sample_run() {
        return;
    }
    let off = run_cvt("v_cvt_f16_f32", &body, false).expect("the same device");
    assert!(off.1 > 0, "the host's conversion already rounds to nearest: the translator's rounding is not needed");
}

#[test]
fn v_fma_mixlo_f16_rounds_to_nearest_even_for_every_f32() {
    // v_fma_mixlo_f16 v2, v1, 1.0, 0: three f32 operands (op_sel_hi 0), the result's low half
    let body = [0x33 << 26 | 0x21 << 16 | 2, 256 + 1 | 242 << 9 | 128 << 18];
    let ok = |x: u32, got: u32| {
        // fma(-0, 1, +0) is +0
        if x == 0x8000_0000 { got as u16 == 0 } else { cvt_ok(x, got) }
    };
    let shader = lowered(&build(&body), true);
    let Some((seen, bad, first)) = sweep(&shader, ok) else { return };
    eprintln!("v_fma_mixlo_f16, nearest true: {seen} inputs, {bad} differ; first {first:x?}");
    assert_eq!(bad, 0, "differences: {first:x?}");
    let shader = lowered_as(&build(&body), true, true);
    let (_, fp, first) = sweep(&shader, ok).expect("the same device");
    eprintln!("v_fma_mixlo_f16, fptrunc: {fp} differ; first {first:x?}");
    assert_eq!(fp, 0, "fptrunc: differences: {first:x?}");
    if !sample_run() {
        return;
    }
    let shader = lowered(&build(&body), false);
    let (_, off, _) = sweep(&shader, ok).expect("the same device");
    assert!(off > 0, "the host's conversion already rounds to nearest");
}

#[test]
fn v_add_f16_rounds_to_nearest_even_for_every_pair_of_halves() {
    // v3 = x >> 16; v_add_f16 v2, v1, v3: the low halves of x and x >> 16
    let body = [vop2(0x16, 3, 128 + 16, 1), vop2(0x32, 2, vgpr(1), 3)];
    let ok = |x: u32, got: u32| {
        let (a, b) = (x as u16, (x >> 16) as u16);
        let (fa, fb) = (half_value(a), half_value(b));
        let h = got as u16;
        let sum = fa + fb;
        if sum.is_nan() {
            return is_nan16(h);
        }
        h == half_nearest(sum)
    };
    let shader = lowered(&build(&body), true);
    let Some((seen, bad, first)) = sweep(&shader, ok) else { return };
    eprintln!("v_add_f16, nearest true: {seen} pairs, {bad} differ; first {first:x?}");
    assert_eq!(bad, 0, "differences: {first:x?}");
    let shader = lowered_as(&build(&body), true, true);
    let (_, fp, first) = sweep(&shader, ok).expect("the same device");
    eprintln!("v_add_f16, fptrunc: {fp} differ; first {first:x?}");
    assert_eq!(fp, 0, "fptrunc: differences: {first:x?}");
    if !sample_run() {
        return;
    }
    let shader = lowered(&build(&body), false);
    let (_, off, _) = sweep(&shader, ok).expect("the same device");
    assert!(off > 0, "the host's conversion already rounds to nearest");
}
