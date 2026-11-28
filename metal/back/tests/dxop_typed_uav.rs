//! SFI0 bit 11 `TypedUAVLoadAdditionalFormats` (640 programs: 190 pixel, 403
//! compute, 47 mesh): loads from a typed UAV (`RWTexture2D<T>::Load`,
//! `dx.op.textureLoad`) whose view format has more than one channel or is
//! not `R32_*`. D3D12 lists the formats a device must load when
//! `TypedUAVLoadAdditionalFormats` is true (D3D12_FEATURE_DATA_D3D12_OPTIONS;
//! "Typed unordered access view loads", D3D12 docs fetched 2026-11-28):
//! R32G32B32A32_{FLOAT,UINT,SINT}, R16G16B16A16_{FLOAT,UNORM,UINT,SNORM,SINT},
//! R8G8B8A8_{UNORM,UINT,SNORM,SINT}, R16_{FLOAT,UNORM,UINT,SNORM,SINT},
//! R8_{UNORM,UINT,SNORM,SINT}. Each format is loaded through MSC and Metal
//! from a texture the CPU filled with its raw encoding, and every channel is
//! compared with the CPU's decoding (D3D11.3 spec 19.x format conversion:
//! UNORM = raw / max, SNORM = max(raw / max, -1), missing channels read
//! (0, 0, 0, 1)).
#![cfg(target_vendor = "apple")]

mod common_dxop;
mod common_msc;

use common_dxop::*;
use victory_metal_back::msc::flags;
use victory_metal_back::texture::{TextureDesc, ViewDim, ViewSpec};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Float,
    Unorm,
    Snorm,
    Uint,
    Sint,
}

struct F {
    name: &'static str,
    dxgi: u32,
    kind: Kind,
    chans: usize,
    bits: u32,
}

const fn f(name: &'static str, dxgi: u32, kind: Kind, chans: usize, bits: u32) -> F {
    F {
        name,
        dxgi,
        kind,
        chans,
        bits,
    }
}

use Kind::*;
/// DXGI numbers: dxgiformat.h.
const FORMATS: [F; 26] = [
    f("R32G32B32A32_FLOAT", 2, Float, 4, 32),
    f("R32G32B32A32_UINT", 3, Uint, 4, 32),
    f("R32G32B32A32_SINT", 4, Sint, 4, 32),
    f("R16G16B16A16_FLOAT", 10, Float, 4, 16),
    f("R16G16B16A16_UNORM", 11, Unorm, 4, 16),
    f("R16G16B16A16_UINT", 12, Uint, 4, 16),
    f("R16G16B16A16_SNORM", 13, Snorm, 4, 16),
    f("R16G16B16A16_SINT", 14, Sint, 4, 16),
    f("R8G8B8A8_UNORM", 28, Unorm, 4, 8),
    f("R8G8B8A8_UINT", 30, Uint, 4, 8),
    f("R8G8B8A8_SNORM", 31, Snorm, 4, 8),
    f("R8G8B8A8_SINT", 32, Sint, 4, 8),
    f("R16_FLOAT", 54, Float, 1, 16),
    f("R16_UNORM", 56, Unorm, 1, 16),
    f("R16_UINT", 57, Uint, 1, 16),
    f("R16_SNORM", 58, Snorm, 1, 16),
    f("R16_SINT", 59, Sint, 1, 16),
    f("R8_UNORM", 61, Unorm, 1, 8),
    f("R8_UINT", 62, Uint, 1, 8),
    f("R8_SNORM", 63, Snorm, 1, 8),
    f("R8_SINT", 64, Sint, 1, 8),
    // the two-channel and packed ones the translator's census also meets (optional in D3D12)
    f("R32G32_FLOAT", 16, Float, 2, 32),
    f("R16G16_FLOAT", 34, Float, 2, 16),
    f("R16G16_UNORM", 35, Unorm, 2, 16),
    f("R8G8_UNORM", 49, Unorm, 2, 8),
    f("R8G8_UINT", 50, Uint, 2, 8),
];

fn f16_bits(x: f32) -> u16 {
    // exact for the multiples of 1/8 below 16 used here
    if x == 0.0 {
        return 0;
    }
    let sign = u16::from(x < 0.0) << 15;
    let a = x.abs();
    let e = a.log2().floor() as i32;
    let m = ((a / 2f32.powi(e) - 1.0) * 1024.0).round() as u16;
    sign | (((e + 15) as u16) << 10) | m
}

fn f16_value(h: u16) -> f32 {
    let s = if h >> 15 == 1 { -1.0 } else { 1.0 };
    let e = i32::from((h >> 10) & 31);
    let m = f32::from(h & 1023);
    if e == 0 {
        s * m * 2f32.powi(-24)
    } else {
        s * (1.0 + m / 1024.0) * 2f32.powi(e - 15)
    }
}

/// Raw channel value `c` of pixel `i`: kept in range of the format.
fn raw(fm: &F, i: usize, c: usize) -> i64 {
    let span: i64 = 1 << fm.bits.min(31);
    let v = ((i as i64 * 977 + c as i64 * 6151 + 13) * 40503) % span;
    match fm.kind {
        Snorm | Sint => v - span / 2,
        _ => v,
    }
}

fn float_value(i: usize, c: usize) -> f32 {
    ((i * 4 + c) as f32) * 0.125 - 1.0
}

/// The texel's bytes and the four words `Load` should return.
fn texel(fm: &F, i: usize) -> (Vec<u8>, [u32; 4]) {
    let mut bytes = Vec::new();
    let mut want = [0u32, 0, 0, 0];
    let float_out = matches!(fm.kind, Float | Unorm | Snorm);
    if float_out {
        want[3] = 1.0f32.to_bits();
    } else {
        want[3] = 1;
    }
    for (c, slot) in want.iter_mut().enumerate().take(fm.chans) {
        let (b, w): (Vec<u8>, u32) = match (fm.kind, fm.bits) {
            (Float, 32) => {
                let v = float_value(i, c);
                (v.to_le_bytes().to_vec(), v.to_bits())
            }
            (Float, 16) => {
                let v = float_value(i, c);
                (
                    f16_bits(v).to_le_bytes().to_vec(),
                    f16_value(f16_bits(v)).to_bits(),
                )
            }
            (Unorm, bits) => {
                let r = raw(fm, i, c);
                let max = ((1u64 << bits) - 1) as f32;
                let b = if bits == 16 {
                    (r as u16).to_le_bytes().to_vec()
                } else {
                    vec![r as u8]
                };
                (b, (r as f32 / max).to_bits())
            }
            (Snorm, bits) => {
                let r = raw(fm, i, c);
                let max = ((1u64 << (bits - 1)) - 1) as f32;
                let b = if bits == 16 {
                    (r as i16).to_le_bytes().to_vec()
                } else {
                    vec![r as i8 as u8]
                };
                (b, (r as f32 / max).max(-1.0).to_bits())
            }
            (Uint, bits) => {
                let r = raw(fm, i, c);
                let b = match bits {
                    32 => (r as u32).to_le_bytes().to_vec(),
                    16 => (r as u16).to_le_bytes().to_vec(),
                    _ => vec![r as u8],
                };
                (b, r as u32)
            }
            (Sint, bits) => {
                let r = raw(fm, i, c);
                let b = match bits {
                    32 => (r as i32).to_le_bytes().to_vec(),
                    16 => (r as i16).to_le_bytes().to_vec(),
                    _ => vec![r as i8 as u8],
                };
                (b, r as i32 as u32)
            }
            (Float, _) => unreachable!(),
        };
        bytes.extend(b);
        *slot = w;
    }
    if !float_out && fm.chans < 4 {
        // (0, 0, 0, 1) for the channels the format lacks
        for w in &mut want[fm.chans..3] {
            *w = 0;
        }
    }
    (bytes, want)
}

/// One load per format and pixel of a 4x4 UAV texture.
#[test]
fn typed_uav_loads_of_the_additional_formats() {
    let rig = rig();
    let mut report = Vec::new();
    let mut failed = Vec::new();
    for fm in &FORMATS {
        let (ty, op) = match fm.kind {
            Float | Unorm | Snorm => ("float4", "dx.op.textureLoad.f32"),
            Uint => ("uint4", "dx.op.textureLoad.i32"),
            Sint => ("int4", "dx.op.textureLoad.i32"),
        };
        let src = format!(
            "[numthreads(4,4,1)] void main(uint3 id : SV_GroupThreadID) {{\n\
               RWTexture2D<{ty}> t = ResourceDescriptorHeap[0];\n\
               {ty} v = t.Load(int2(id.xy));\n\
               O.Store4((id.y*4+id.x)*16, uint4(asuint(v.x), asuint(v.y), asuint(v.z), asuint(v.w))); }}\n"
        );
        // the shader's load format comes from the view (typed UAV): `RWTexture2D<float4>` over any of them
        let Some(dxil) = dxil_checked(
            &format!("tuav_{}", fm.name),
            "cs_6_6",
            &format!("{PRELUDE}{src}"),
            &[op],
        ) else {
            return;
        };
        let built = rig.try_build(&dxil, flags::HEAP_INDEXED | flags::SAMPLER_HEAP_INDEXED);
        if let Err(e) = built {
            report.push(format!("{}: pipeline refused: {e}", fm.name));
            failed.push(fm.name);
            continue;
        }
        let p = rig.prog_with(&dxil, flags::HEAP_INDEXED | flags::SAMPLER_HEAP_INDEXED);
        let t = match rig
            .d
            .create_texture(&TextureDesc::tex2d(fm.dxgi, 4, 4, SRV | UAV))
        {
            Ok(t) => t,
            Err(e) => {
                report.push(format!("{}: texture refused: {e}", fm.name));
                failed.push(fm.name);
                continue;
            }
        };
        let texels: Vec<(Vec<u8>, [u32; 4])> = (0..16).map(|i| texel(fm, i)).collect();
        let bytes: Vec<u8> = texels.iter().flat_map(|t| t.0.clone()).collect();
        let pitch = (bytes.len() / 4) as u32;
        upload(&rig.d, &rig.q, &t, 0, &bytes, pitch);
        let view = match t.view(&ViewSpec::whole(ViewDim::D2)) {
            Ok(v) => v,
            Err(e) => {
                report.push(format!("{}: view refused: {e}", fm.name));
                failed.push(fm.name);
                continue;
            }
        };
        let hs = heaps(&rig.d, &[view], &[]);
        let got = rig.run_heaps(&p, [1, 1, 1], [4, 4, 1], &[], &[], 64, Some((&hs.0, &hs.1)));
        let mut bad = 0;
        let mut first = String::new();
        for (i, (_, want)) in texels.iter().enumerate() {
            let g = &got[i * 4..i * 4 + 4];
            let ok = (0..4).all(|c| {
                if matches!(fm.kind, Float | Unorm | Snorm) {
                    close(
                        f32::from_bits(g[c]),
                        f32::from_bits(want[c]),
                        1.5e-5 * f32::from_bits(want[c]).abs().max(1.0),
                    )
                } else {
                    g[c] == want[c]
                }
            });
            if !ok {
                bad += 1;
                if first.is_empty() {
                    first = format!("pixel {i}: got {g:x?} want {want:x?}");
                }
            }
        }
        if bad > 0 {
            report.push(format!("{}: {bad} of 16 texels differ; {first}", fm.name));
            failed.push(fm.name);
        } else {
            report.push(format!("{}: ok", fm.name));
        }
    }
    eprintln!("{}", report.join("\n"));
    assert!(
        failed.is_empty(),
        "typed UAV loads that fail: {failed:?}\n{}",
        report.join("\n")
    );
}
