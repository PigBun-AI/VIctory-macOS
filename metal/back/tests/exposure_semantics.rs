//! The float conversions and guards the title's auto-exposure shaders rely on
//! (`white` branch: the exposure chain 309:73 → 309:96 of rec-w24), run
//! through DXC → MSC → Metal and compared with the IEEE/HLSL result computed on
//! the CPU. Skips without a DXC (see `common_msc.rs`).
#![cfg(target_vendor = "apple")]

mod common_msc;

use std::sync::{Mutex, MutexGuard, OnceLock};

use victory_metal_back::device::Device;
use victory_metal_back::memory::BufferKind;
use victory_metal_back::msc::{
    Compat, CompileOptions, Param, ParamKind, RootSignatureDesc, Visibility,
};
use victory_metal_back::queue::Queue;

use common_msc::dxil_or_skip;

fn setup() -> (MutexGuard<'static, ()>, Device) {
    static LOCK: Mutex<()> = Mutex::new(());
    static D: OnceLock<Device> = OnceLock::new();
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let d = D
        .get_or_init(|| Device::new().expect("Metal 4 device"))
        .clone();
    (guard, d)
}

fn root_uav(register: u32) -> Param {
    Param {
        kind: ParamKind::Uav { register, space: 0 },
        visibility: Visibility::All,
    }
}

/// Runs `main` (64 threads, one group, `u0` = input words, `u1` = output
/// words) and returns the 64 output words.
fn run(name: &str, hlsl: &str, input: &[u32], compat: Compat) -> Option<Vec<u32>> {
    let dxil = dxil_or_skip(name, "cs_6_6", hlsl)?;
    let (_g, d) = setup();
    let q = Queue::new(&d).unwrap();
    let root = RootSignatureDesc {
        params: vec![root_uav(0), root_uav(1)],
        ..Default::default()
    };
    let opts = CompileOptions {
        compat,
        ..Default::default()
    };
    let sp = d
        .compute_pipeline_from_dxil(&dxil, Some(&root), &opts)
        .unwrap();
    let r = &sp.reflection;
    let inb = d.create_buffer(BufferKind::Shared, 4 * 64).unwrap();
    let mut padded = input.to_vec();
    padded.resize(64, 0);
    inb.write_u32s(0, &padded).unwrap();
    let out = d.create_buffer(BufferKind::Shared, 4 * 64).unwrap();
    out.write_u32s(0, &vec![0xDEAD_BEEF; 64]).unwrap();
    let tlab = q.alloc_constants(u64::from(r.layout.size), 8).unwrap();
    tlab.zero();
    tlab.write_u64(u64::from(r.layout.entries[0].offset), inb.gpu_va())
        .unwrap();
    tlab.write_u64(u64::from(r.layout.entries[1].offset), out.gpu_va())
        .unwrap();
    let mut l = q.begin_list().unwrap();
    l.set_root_arguments(&tlab).unwrap();
    l.dispatch(&sp.pipeline, [1, 1, 1], [64, 1, 1]).unwrap();
    l.end().unwrap();
    let v = q.submit(vec![l]).expect("submit");
    q.wait_for(v, 20_000).expect("completes");
    Some(out.read_u32s(0, 64).unwrap())
}

fn f(x: f32) -> u32 {
    x.to_bits()
}

const HEAD: &str = "RWByteAddressBuffer i : register(u0);\nRWByteAddressBuffer o : register(u1);\n";

/// `1 / y` with y = 0, then `eps <= y ? z * r : 0` (the history range guard
/// of the histogram pass).
const RECIPROCAL_GUARD: &str = "[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
  float y = asfloat(i.Load(t.x*4)); float z = 1234.5;\n\
  float r = 1.0 / y; bool ok = 0.000000011920929 <= y;\n\
  o.Store(t.x*4, asuint(ok ? z * r : 0.0)); }\n";

fn guard_ref(y: f32) -> f32 {
    let r = 1.0 / y;
    if 0.000000011920929 <= y {
        1234.5 * r
    } else {
        0.0
    }
}

#[test]
fn a_reciprocal_guard_with_a_zero_denominator_is_zero() {
    let ys = [
        0.0f32,
        -0.0,
        1.0,
        15.33,
        0.0001,
        1e-30,
        5853.9,
        f32::INFINITY,
    ];
    let input: Vec<u32> = ys.iter().map(|y| f(*y)).collect();
    for compat in [
        Compat::BOUNDS_CHECK,
        Compat::BOUNDS_CHECK | Compat::DISABLE_NAN_INF_OPTIMIZATION,
    ] {
        let Some(got) = run(
            "recip_guard",
            &format!("{HEAD}{RECIPROCAL_GUARD}"),
            &input,
            compat,
        ) else {
            return;
        };
        for (k, y) in ys.iter().enumerate() {
            let want = guard_ref(*y);
            let g = f32::from_bits(got[k]);
            // `1 / y` is not correctly rounded on the GPU: a few ulps.
            assert!(
                (g - want).abs() <= want.abs() * 1e-5,
                "y={y} compat={compat:?}: {g} vs {want}"
            );
        }
    }
}

/// `uint(x)` and the log-bin of the histogram pass:
/// `bin = uint(max(0, 255 * ln(lum / y) / max(eps, ln(z / y))))`.
const BIN: &str = "[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
  float lum = asfloat(i.Load(t.x*4)); float y = 15.33, z = 5853.9;\n\
  float num = log2(lum / y) * 0.6931472; float den = max(0.00000011920929, log2(z / y) * 0.6931472);\n\
  float b = 255.0 * (num / den);\n\
  o.Store(t.x*4, uint(max(0.0, b))); }\n";

fn bin_ref(lum: f32) -> u32 {
    let num = (lum / 15.33).log2() * std::f32::consts::LN_2;
    let den = f32::max(
        0.000_000_119_209_29,
        (5853.9f32 / 15.33).log2() * std::f32::consts::LN_2,
    );
    let b = 255.0 * (num / den);
    // `max` of a NaN gives the other operand; conversions saturate in HLSL on every GPU the title ran on.
    let b = if b.is_nan() || b < 0.0 { 0.0 } else { b };
    if b >= 4294967296.0 {
        u32::MAX
    } else {
        b as u32
    }
}

#[test]
fn the_log_bin_of_the_histogram_pass_matches_the_cpu() {
    let ls = [
        0.0f32,
        1e-10,
        0.00056,
        1.0,
        15.33,
        16.0,
        100.0,
        1359.0,
        5853.9,
        13088.0,
        1e10,
        f32::INFINITY,
    ];
    let input: Vec<u32> = ls.iter().map(|l| f(*l)).collect();
    let Some(got) = run(
        "log_bin",
        &format!("{HEAD}{BIN}"),
        &input,
        Compat::BOUNDS_CHECK,
    ) else {
        return;
    };
    for (k, l) in ls.iter().enumerate() {
        let want = bin_ref(*l);
        let diff = (i64::from(got[k]) - i64::from(want)).abs();
        assert!(diff <= 1, "lum={l}: got {} want {want}", got[k]);
    }
}
