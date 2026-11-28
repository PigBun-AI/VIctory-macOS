//! `fptrunc float to half` (native 16-bit types, SM 6.2+): the cheaper rounding F1 could use
//! instead of the translator's 12-operation `f16_nearest`. LLVM's `fptrunc` is round-to-nearest-even
//! (LLVM LangRef, `fptrunc`: "rounded using the default rounding mode"); the DXIL spec's
//! `legacyF32ToF16` is the round-toward-zero one (D3D11.3 spec 3.2.2). This runs the HLSL cast
//! `(half)f` (DXC `-enable-16bit-types`, which needs a helper built with that flag:
//! `VICTORY_HELPER_DXC16`) through MSC and Metal and compares every result with the exact
//! nearest-even half. Without that variable the test prints SKIPPED and passes.
#![cfg(target_vendor = "apple")]

mod common_dxop;
mod common_msc;

use std::process::Command;

use common_dxop::*;

fn dxil16(name: &str, profile: &str, hlsl: &str) -> Option<Vec<u8>> {
    let helper = std::env::var_os("VICTORY_HELPER_DXC16")?;
    std::env::var_os("DXC_LIB")?;
    let dir = std::env::temp_dir().join(format!("victory-fpt-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (src, out) = (dir.join("p.hlsl"), dir.join("p.dxil"));
    std::fs::write(&src, hlsl).unwrap();
    let run = Command::new("/usr/bin/arch")
        .arg("-x86_64")
        .arg(helper)
        .args(["-T", profile, "-E", "main", "-Fo"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("run the DXC front end");
    assert!(
        run.status.success(),
        "DXC refused {name}: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let d = std::fs::read(&out).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    Some(d)
}

/// The exact nearest-even half of a finite or infinite f32 (NaN handled by the caller).
fn rne(x: u32) -> u16 {
    let s = ((x >> 16) & 0x8000) as u16;
    let e = ((x >> 23) & 0xff) as i32;
    let f = x & 0x7f_ffff;
    if e == 255 {
        return s | 0x7c00;
    }
    let he = e - 127 + 15;
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
    let bits = base + q;
    if bits >= 0x7c00 {
        return s | 0x7c00;
    }
    s | bits as u16
}

#[test]
fn fptrunc_to_half_is_nearest_even_for_a_wide_input_set() {
    let rig = rig();
    let Some(dxil) = dxil16(
        "fptrunc",
        "cs_6_2",
        "ByteAddressBuffer I : register(t0);\n\
         RWByteAddressBuffer O : register(u0);\n\
         [numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
           float f = asfloat(I.Load(t.x*4));\n\
           half h = (half)f;\n\
           O.Store(t.x*4, (uint)asuint16(h)); }\n",
    ) else {
        eprintln!("SKIPPED fptrunc: set VICTORY_HELPER_DXC16 (a DXC helper with -enable-16bit-types) and DXC_LIB");
        return;
    };
    let ops = declared_dxops(&dxil);
    assert!(!ops.contains("dx.op.legacyF32ToF16"), "the cast went through the legacy op: {ops:?}");
    let p = rig.prog_from(&dxil);
    // every 4096th f32 pattern (a sweep of the 2^32 space), the neighbours of every half
    // rounding boundary of its exponent, and edge values
    let mut input: Vec<u32> = (0..(1u32 << 20)).map(|i| i << 12 | (i.wrapping_mul(2654435761) & 0xfff)).collect();
    for h in 0..=0xffffu32 {
        // the exact midpoint between half h and h+1 (a tie), and one f32 step each side
        let lo = (h & 0x7fff) as u16;
        if lo >= 0x7bff {
            continue;
        }
        let wide = |hh: u32| {
            let m = (hh & 0x3ff) as f64;
            let e = ((hh >> 10) & 31) as i32;
            let v = if e == 0 { m * 2f64.powi(-24) } else { (1.0 + m / 1024.0) * 2f64.powi(e - 15) };
            if hh >> 15 == 1 { -v } else { v }
        };
        let mid = ((wide(h) + wide(h + 1)) / 2.0) as f32;
        let b = mid.to_bits();
        input.extend_from_slice(&[b.wrapping_sub(1), b, b.wrapping_add(1)]);
    }
    input.extend_from_slice(&[0x477f_f000, 0x477f_efff, 0x7f7f_ffff, 0x7f80_0000, 0xff80_0000, 0x8000_0000, 0, 0x3300_0000, 0x3380_0000, 0x33c0_0000]);
    while !input.len().is_multiple_of(64) {
        input.push(0);
    }
    let n = input.len();
    let got = rig.run(&p, [(n / 64) as u32, 1, 1], [64, 1, 1], &[], &input, n);
    let mut bad = Vec::new();
    for (i, (&x, &g)) in input.iter().zip(&got).enumerate() {
        let f = f32::from_bits(x);
        let ok = if f.is_nan() { (g as u16 & 0x7fff) > 0x7c00 } else if x & 0x7f80_0000 == 0 { g as u16 & 0x7fff == 0 } else { g == u32::from(rne(x)) };
        if !ok {
            bad.push((i, x, g));
        }
    }
    eprintln!("fptrunc: {n} inputs, {} differ from nearest-even; first {:x?}", bad.len(), &bad[..bad.len().min(6)]);
    assert!(bad.is_empty(), "{} differ: {:x?}", bad.len(), &bad[..bad.len().min(6)]);
}
