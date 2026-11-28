//! Two more dx.op nodes of the DXIL -> MSC hop that games use and no GPU test covered:
//!
//! * `dx.op.dot4AddPacked` (SM 6.4, DXIL opcodes 163 `Dot4AddI8Packed` and 164
//!   `Dot4AddU8Packed`; HLSL `dot4add_i8packed` / `dot4add_u8packed`): 13 compute programs of
//!   the census. DXIL.rst (github.com/microsoft/DirectXShaderCompiler, docs/DXIL.rst, fetched
//!   2026-11-28): "signed / unsigned dot product of 4 x i8 / u8 vectors packed into i32, with
//!   accumulate to i32". The four bytes of each operand are the elements (byte 0 the lowest);
//!   the accumulator is added with the dword's wraparound.
//! * `dx.op.textureStore.i32` (opcode 67; DXIL.rst "TextureStore": the write mask must cover all
//!   components of the resource): stores of integer values to typed UAVs
//!   (`RWTexture2D<uint/uint2/uint4/int...>`) of every integer format the census meets. D3D11.3
//!   Functional Spec 3.2.3 / 22.4.9 `store_uav_typed`: the value is converted to the view's
//!   format; for an out-of-range value the spec lets hardware clamp (17.7.1 says so of output
//!   merger UINT; the store path leaves it open), so the test asserts in-range values exactly and
//!   only reports what an out-of-range value becomes.
//!
//! HLSL -> DXC -> DXIL -> MSC -> Metal compute -> read back -> CPU reference (common_dxop.rs).
#![cfg(target_vendor = "apple")]

mod common_dxop;
mod common_msc;

use common_dxop::*;
use victory_metal_back::msc::flags;
use victory_metal_back::texture::{TextureDesc, ViewDim, ViewSpec};

fn dot4_u(a: u32, b: u32, acc: u32) -> u32 {
    (0..4).fold(acc, |s, i| {
        s.wrapping_add(((a >> (8 * i)) & 255).wrapping_mul((b >> (8 * i)) & 255))
    })
}

fn dot4_i(a: u32, b: u32, acc: u32) -> u32 {
    (0..4).fold(acc as i32, |s, i| {
        let x = i32::from((a >> (8 * i)) as u8 as i8);
        let y = i32::from((b >> (8 * i)) as u8 as i8);
        s.wrapping_add(x * y)
    }) as u32
}

#[test]
fn dot4_add_packed_signed_and_unsigned() {
    let rig = rig();
    let Some(p) = rig.program(
        "dot4add",
        "[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
           uint a = I.Load(t.x*12); uint b = I.Load(t.x*12+4); uint c = I.Load(t.x*12+8);\n\
           O.Store2(t.x*8, uint2(dot4add_u8packed(a, b, c), (uint)dot4add_i8packed(a, b, (int)c))); }\n",
        &["dx.op.dot4AddPacked.i32"],
    ) else {
        return;
    };
    let mut cases: Vec<[u32; 3]> = vec![
        [0, 0, 0],
        [0x0101_0101, 0x0101_0101, 0],
        [0xffff_ffff, 0xffff_ffff, 0],
        [0xffff_ffff, 0x0101_0101, 0],
        [0x8080_8080, 0x8080_8080, 0],
        [0x7f7f_7f7f, 0x7f7f_7f7f, 5],
        [0x8080_8080, 0x7f7f_7f7f, 0],
        [0x0403_0201, 0x0807_0605, 100],
        [0xffff_ffff, 0xffff_ffff, 0xffff_ffff], // accumulator wraps
        [0x7f7f_7f7f, 0x7f7f_7f7f, 0x7fff_ffff], // signed accumulator wraps
        [0x8080_8080, 0x8080_8080, 0x8000_0000],
        [0x0000_0080, 0x0000_0080, 0],
        [0x8000_0000, 0x8000_0000, 0],
        [0x0000_0001, 0x0000_00ff, 0],
    ];
    let mut s = 99u32;
    for _ in 0..2000 {
        let mut next = || {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            s
        };
        cases.push([next(), next(), next()]);
    }
    while !cases.len().is_multiple_of(64) {
        cases.push([0, 0, 0]);
    }
    let n = cases.len();
    let input: Vec<u32> = cases.iter().flatten().copied().collect();
    let got = rig.run(&p, [(n / 64) as u32, 1, 1], [64, 1, 1], &[], &input, n * 2);
    let want: Vec<u32> = cases
        .iter()
        .flat_map(|&[a, b, c]| [dot4_u(a, b, c), dot4_i(a, b, c)])
        .collect();
    assert_words("dot4AddPacked (u8 then i8 per case)", &got, &want);
}

#[derive(Clone, Copy)]
struct F {
    name: &'static str,
    dxgi: u32,
    chans: usize,
    bits: u32,
    signed: bool,
}

const fn f(name: &'static str, dxgi: u32, chans: usize, bits: u32, signed: bool) -> F {
    F {
        name,
        dxgi,
        chans,
        bits,
        signed,
    }
}

/// DXGI numbers: dxgiformat.h.
const FORMATS: [F; 15] = [
    f("R32G32B32A32_UINT", 3, 4, 32, false),
    f("R32G32B32A32_SINT", 4, 4, 32, true),
    f("R32G32_UINT", 17, 2, 32, false),
    f("R32G32_SINT", 18, 2, 32, true),
    f("R16G16B16A16_UINT", 12, 4, 16, false),
    f("R16G16B16A16_SINT", 14, 4, 16, true),
    f("R8G8B8A8_UINT", 30, 4, 8, false),
    f("R8G8B8A8_SINT", 32, 4, 8, true),
    f("R32_UINT", 42, 1, 32, false),
    f("R32_SINT", 43, 1, 32, true),
    f("R16_UINT", 57, 1, 16, false),
    f("R16_SINT", 59, 1, 16, true),
    f("R8_UINT", 62, 1, 8, false),
    f("R8_SINT", 64, 1, 8, true),
    f("R8G8_UINT", 50, 2, 8, false),
];

/// The in-range value stored for texel `i`, channel `c`: both ends of the range, then spread.
fn value(fm: &F, i: usize, c: usize) -> i64 {
    let (lo, hi) = if fm.signed {
        (-(1i64 << (fm.bits - 1)), (1i64 << (fm.bits - 1)) - 1)
    } else {
        (0, (1i64 << fm.bits) - 1)
    };
    match (i + c) % 5 {
        0 => lo,
        1 => hi,
        _ => {
            let span = (hi - lo) as u64 + 1;
            let h = ((i * 16 + c) as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 11;
            lo + (h % span) as i64
        }
    }
}

fn encode(fm: &F, v: i64) -> Vec<u8> {
    match fm.bits {
        32 => (v as u32).to_le_bytes().to_vec(),
        16 => (v as u16).to_le_bytes().to_vec(),
        _ => vec![v as u8],
    }
}

fn hlsl_type(fm: &F) -> String {
    let base = if fm.signed { "int" } else { "uint" };
    if fm.chans == 1 {
        base.to_string()
    } else {
        format!("{base}{}", fm.chans)
    }
}

/// Stores one integer texel per thread to a 4x4 typed UAV of `fm` and reads the texture back.
/// `values(i, c)` is stored; returns the texels' bytes (row pitch `4 * texel bytes`).
fn store_and_read(
    rig: &Rig,
    fm: &F,
    values: &dyn Fn(usize, usize) -> i64,
) -> Option<Result<Vec<u8>, String>> {
    let ty = hlsl_type(fm);
    // the value of texel i, channel c, comes from the input buffer (the raw 32-bit pattern)
    let comps: Vec<String> = (0..fm.chans)
        .map(|c| {
            let load = format!("I.Load(((id.y*4+id.x)*4+{c})*4)");
            if fm.signed {
                format!("asint({load})")
            } else {
                load
            }
        })
        .collect();
    let value = if fm.chans == 1 {
        comps[0].clone()
    } else {
        format!("{ty}({})", comps.join(", "))
    };
    let src = format!(
        "[numthreads(4,4,1)] void main(uint3 id : SV_GroupThreadID) {{\n\
           RWTexture2D<{ty}> t = ResourceDescriptorHeap[0];\n\
           t[int2(id.xy)] = {value}; }}\n"
    );
    let dxil = dxil_checked(
        &format!("tstore_{}", fm.name),
        "cs_6_6",
        &format!("{PRELUDE}{src}"),
        &["dx.op.textureStore.i32"],
    )?;
    let heaps_flags = flags::HEAP_INDEXED | flags::SAMPLER_HEAP_INDEXED;
    if let Err(e) = rig.try_build(&dxil, heaps_flags) {
        return Some(Err(format!("pipeline refused: {e}")));
    }
    let p = rig.prog_with(&dxil, heaps_flags);
    let t = match rig
        .d
        .create_texture(&TextureDesc::tex2d(fm.dxgi, 4, 4, SRV | UAV))
    {
        Ok(t) => t,
        Err(e) => return Some(Err(format!("texture refused: {e}"))),
    };
    let view = match t.view(&ViewSpec::whole(ViewDim::D2)) {
        Ok(v) => v,
        Err(e) => return Some(Err(format!("view refused: {e}"))),
    };
    let hs = heaps(&rig.d, &[view], &[]);
    let input: Vec<u32> = (0..16)
        .flat_map(|i| (0..4).map(move |c| (i, c)))
        .map(|(i, c)| values(i, c) as u32)
        .collect();
    let _ = rig.run_heaps(
        &p,
        [1, 1, 1],
        [4, 4, 1],
        &[],
        &input,
        1,
        Some((&hs.0, &hs.1)),
    );
    let pitch = (4 * fm.chans as u32 * fm.bits / 8).max(4);
    Some(Ok(download(&rig.d, &rig.q, &t, pitch)))
}

/// `textureStore.i32` of in-range values to each integer format reads back exactly.
#[test]
fn texture_store_i32_writes_each_integer_format_exactly() {
    let rig = rig();
    let mut report = Vec::new();
    let mut failed = Vec::new();
    for fm in &FORMATS {
        let vals = |i: usize, c: usize| value(fm, i, c);
        let Some(r) = store_and_read(&rig, fm, &vals) else {
            return;
        };
        match r {
            Err(e) => {
                report.push(format!("{}: {e}", fm.name));
                failed.push(fm.name);
            }
            Ok(bytes) => {
                let texel_bytes = fm.chans * fm.bits as usize / 8;
                let mut want = Vec::new();
                for i in 0..16 {
                    for c in 0..fm.chans {
                        want.extend(encode(fm, vals(i, c)));
                    }
                }
                let pitch = (4 * texel_bytes).max(4);
                let mut got = Vec::new();
                for row in 0..4 {
                    got.extend_from_slice(&bytes[row * pitch..row * pitch + 4 * texel_bytes]);
                }
                if got == want {
                    report.push(format!("{}: ok", fm.name));
                } else {
                    let at = got.iter().zip(&want).position(|(a, b)| a != b).unwrap();
                    report.push(format!(
                        "{}: differs at byte {at} (texel {}): got {:02x?} want {:02x?}",
                        fm.name,
                        at / texel_bytes,
                        &got[at / texel_bytes * texel_bytes..(at / texel_bytes + 1) * texel_bytes],
                        &want[at / texel_bytes * texel_bytes..(at / texel_bytes + 1) * texel_bytes],
                    ));
                    failed.push(fm.name);
                }
            }
        }
    }
    eprintln!("{}", report.join("\n"));
    assert!(
        failed.is_empty(),
        "integer typed UAV stores that fail: {failed:?}\n{}",
        report.join("\n")
    );
}

/// Out of range: reported, not asserted (D3D leaves clamp or truncation to the hardware).
#[test]
fn texture_store_i32_out_of_range_values_are_clamped_or_truncated() {
    let rig = rig();
    let cases: [(F, i64); 4] = [
        (FORMATS[12], 0x1ff),  // R8_UINT
        (FORMATS[10], 70_000), // R16_UINT
        (FORMATS[11], 40_000), // R16_SINT
        (FORMATS[13], 300),    // R8_SINT
    ];
    let mut lines = Vec::new();
    for (fm, v) in cases {
        let vals = |_: usize, _: usize| v;
        let Some(r) = store_and_read(&rig, &fm, &vals) else {
            return;
        };
        match r {
            Err(e) => lines.push(format!("{}: {e}", fm.name)),
            Ok(bytes) => {
                let b = fm.bits as usize / 8;
                let raw: i64 = match (fm.bits, fm.signed) {
                    (32, false) => i64::from(u32::from_le_bytes(bytes[..4].try_into().unwrap())),
                    (32, true) => i64::from(i32::from_le_bytes(bytes[..4].try_into().unwrap())),
                    (16, false) => i64::from(u16::from_le_bytes(bytes[..2].try_into().unwrap())),
                    (16, true) => i64::from(i16::from_le_bytes(bytes[..2].try_into().unwrap())),
                    (_, false) => i64::from(bytes[0]),
                    (_, true) => i64::from(bytes[0] as i8),
                };
                let what = if raw == v {
                    "kept"
                } else {
                    let (lo, hi) = if fm.signed {
                        (-(1i64 << (fm.bits - 1)), (1i64 << (fm.bits - 1)) - 1)
                    } else {
                        (0, (1i64 << fm.bits) - 1)
                    };
                    if raw == v.clamp(lo, hi) {
                        "clamped"
                    } else if raw == (v & ((1i64 << (8 * b)) - 1)) || fm.signed {
                        "truncated"
                    } else {
                        "other"
                    }
                };
                lines.push(format!("{}: stored {v}, read {raw} ({what})", fm.name));
            }
        }
    }
    eprintln!("textureStore.i32 out of range:\n{}", lines.join("\n"));
}
