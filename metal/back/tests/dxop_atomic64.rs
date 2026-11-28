//! 64-bit atomics (`atomicBinOp.i64`): the translator's `image_atomic_*` with
//! a 64-bit data mask on a typed UAV (an `R32G32_UINT` view read as one
//! `uint64_t`; SFI0 bits 22 and 28). See docs/metal/dxop.md for the census
//! (679 programs: 677 pixel, 2 compute) and the result.
#![cfg(target_vendor = "apple")]

mod common_dxop;
mod common_msc;

use common_dxop::*;
use victory_metal_back::format::dxgi;
use victory_metal_back::msc::flags;
use victory_metal_back::texture::{TextureDesc, ViewDim, ViewSpec};

const W: usize = 4;

fn operand(x: u32, y: u32) -> u64 {
    // the high word differs more than the low one, and is not monotonic in x, y
    (u64::from((x * 7 + y * 5) % 13) << 32) | u64::from((y * 3 + x * 11) % 17)
}

fn run_op(rig: &Rig, op: &str, init: u64, ulong_texture: bool) -> Option<Vec<u64>> {
    let body = format!(
        "[numthreads(8,8,1)] void main(uint3 id : SV_DispatchThreadID) {{\n\
           RWTexture2D<uint64_t> t = ResourceDescriptorHeap[0];\n\
           uint64_t v = ((uint64_t)((id.x * 7 + id.y * 5) % 13) << 32) | (uint64_t)((id.y * 3 + id.x * 11) % 17);\n\
           {op}(t[uint2(id.x & 3, id.y & 3)], v); }}\n"
    );
    let _ = ulong_texture;
    let dxil = dxil_checked(
        &format!("a64_{op}"),
        "cs_6_6",
        &format!("{PRELUDE}{body}"),
        &["dx.op.atomicBinOp.i64"],
    )?;
    let p = rig.prog_with(&dxil, flags::HEAP_INDEXED | flags::SAMPLER_HEAP_INDEXED);
    let t = rig
        .d
        .create_texture(&TextureDesc::tex2d(
            dxgi::R32G32_UINT,
            W as u32,
            W as u32,
            SRV | UAV,
        ))
        .unwrap();
    let bytes: Vec<u8> = (0..W * W).flat_map(|_| init.to_le_bytes()).collect();
    upload(&rig.d, &rig.q, &t, 0, &bytes, (W * 8) as u32);
    let view = t.view(&ViewSpec::whole(ViewDim::D2)).unwrap();
    let hs = heaps(&rig.d, &[view], &[]);
    rig.run_heaps(&p, [1, 1, 1], [8, 8, 1], &[], &[], 1, Some((&hs.0, &hs.1)));
    let out = download(&rig.d, &rig.q, &t, (W * 8) as u32);
    Some(
        out.chunks(8)
            .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
            .collect(),
    )
}

/// What a 64-bit `InterlockedMax` / `InterlockedMin` leaves in each of the
/// 16 texels hit by 4 threads each.
#[test]
fn texture_atomic_max_and_min_on_64_bits() {
    let rig = rig();
    for (op, init) in [
        ("InterlockedMax", 0u64),
        ("InterlockedMin", u64::MAX),
        ("InterlockedMax", 0x0000_0005_ffff_ffff),
    ] {
        let Some(got) = run_op(&rig, op, init, true) else {
            return;
        };
        let mut want = vec![init; W * W];
        for y in 0..8u32 {
            for x in 0..8u32 {
                let i = ((y & 3) as usize) * W + (x & 3) as usize;
                let v = operand(x, y);
                want[i] = if op == "InterlockedMax" {
                    want[i].max(v)
                } else {
                    want[i].min(v)
                };
            }
        }
        assert_eq!(got, want, "{op} from {init:#x}");
    }
}

const HEAPS: u32 = flags::HEAP_INDEXED | flags::SAMPLER_HEAP_INDEXED;

/// What MSC 4.0.3 takes of 64-bit atomics (checked with the 64-bit unsigned
/// operand on a heap resource, 2026-11-28): `InterlockedMax` / `InterlockedMin`
/// without the original value, on a texture of every dimension, a typed
/// buffer, a structured buffer and a byte-address buffer. Refused with
/// `UnsupportedInstruction`: the original value (`InterlockedMax(.., v, orig)`),
/// And, Or, Xor, Add, Exchange; CompareExchange converts and is refused by
/// Metal's compiler ("unlowered function call to
/// air.atomic_compare_exchange_weak_explicit_texture_2d"). This is Metal's
/// own limit: MSL 3.x 64-bit atomics are `min` and `max` of `ulong`, no
/// original value (Metal Shading Language Specification 6.15.4.6; Metal
/// Feature Set Tables, "64-bit atomics", Apple9 and later).
#[test]
fn msc_takes_64_bit_atomic_min_max_without_the_original_and_nothing_else() {
    let rig = rig();
    let takes = |name: &str, decl: &str, stmt: &str| -> Option<Result<(), String>> {
        let hlsl = format!(
            "{PRELUDE}[numthreads(8,8,1)] void main(uint3 id : SV_DispatchThreadID) {{ {decl} uint64_t v = (uint64_t)id.x << 20; {stmt} }}\n"
        );
        let dxil = common_msc::dxil_or_skip(&format!("a64_{name}"), "cs_6_6", &hlsl)?;
        Some(rig.try_build(&dxil, HEAPS))
    };
    let tex2d = "RWTexture2D<uint64_t> t = ResourceDescriptorHeap[0];";
    for (name, decl, stmt) in [
        (
            "tex1d_max",
            "RWTexture1D<uint64_t> t = ResourceDescriptorHeap[0];",
            "InterlockedMax(t[id.x], v);",
        ),
        ("tex2d_max", tex2d, "InterlockedMax(t[id.xy], v);"),
        ("tex2d_min", tex2d, "InterlockedMin(t[id.xy], v);"),
        (
            "tex2darr_max",
            "RWTexture2DArray<uint64_t> t = ResourceDescriptorHeap[0];",
            "InterlockedMax(t[id.xyz], v);",
        ),
        (
            "tex3d_max",
            "RWTexture3D<uint64_t> t = ResourceDescriptorHeap[0];",
            "InterlockedMax(t[id.xyz], v);",
        ),
        (
            "typedbuf_max",
            "RWBuffer<uint64_t> t = ResourceDescriptorHeap[0];",
            "InterlockedMax(t[id.x], v);",
        ),
        (
            "structbuf_max",
            "RWStructuredBuffer<uint64_t> t = ResourceDescriptorHeap[0];",
            "InterlockedMax(t[id.x], v);",
        ),
        (
            "rawbuf_max",
            "RWByteAddressBuffer t = ResourceDescriptorHeap[0];",
            "uint64_t r; t.InterlockedMax64(id.x * 8, v, r);",
        ),
    ] {
        let Some(r) = takes(name, decl, stmt) else {
            return;
        };
        assert_eq!(r, Ok(()), "{name} is converted and built");
    }
    for (name, stmt) in [
        (
            "tex2d_max_original",
            "uint64_t r; InterlockedMax(t[id.xy], v, r); O.Store2(0, uint2((uint)r, (uint)(r >> 32)));",
        ),
        ("tex2d_and", "InterlockedAnd(t[id.xy], v);"),
        ("tex2d_or", "InterlockedOr(t[id.xy], v);"),
        ("tex2d_xor", "InterlockedXor(t[id.xy], v);"),
        ("tex2d_add", "InterlockedAdd(t[id.xy], v);"),
        (
            "tex2d_exchange",
            "uint64_t r; InterlockedExchange(t[id.xy], v, r);",
        ),
    ] {
        let Some(r) = takes(name, tex2d, stmt) else {
            return;
        };
        let e = r.expect_err(name);
        assert!(
            e.contains("unsupported instruction") && e.contains("atomicBinOp.i64"),
            "{name}: {e}"
        );
    }
    let Some(r) = takes(
        "tex2d_cas",
        tex2d,
        "uint64_t r; InterlockedCompareExchange(t[id.xy], 1ull, v, r);",
    ) else {
        return;
    };
    let e = r.expect_err("compare-exchange");
    assert!(
        e.contains("atomic_compare_exchange_weak_explicit_texture_2d"),
        "{e}"
    );
}

/// In a pixel shader (677 of the 679 programs): a 64-bit `InterlockedMax`
/// into a texel per pixel. The uncovered pixels of the quads on the diagonal
/// (helper lanes, `HALF_VS`) perform no atomic: their texels keep the
/// initial value.
#[test]
fn pixel_shader_64_bit_atomic_max_skips_helper_lanes() {
    let rig = rig();
    let Some(p) = rig.pixel_program_vs(
        "a64_px",
        HALF_VS,
        "void main(float4 pos : SV_Position) {\n\
           uint x = (uint)pos.x, y = (uint)pos.y;\n\
           RWTexture2D<uint64_t> t = ResourceDescriptorHeap[0];\n\
           InterlockedMax(t[uint2(x, y)], ((uint64_t)(x + 1) << 33) | (uint64_t)(y * 8 + x + 1));\n\
           InterlockedMax(t[uint2(x, y)], ((uint64_t)x << 33) | 0x1ffffffffull);\n\
           InterlockedMin(t[uint2(0, 0)], 3ull + y); }\n",
        &["dx.op.atomicBinOp.i64"],
        true,
    ) else {
        return;
    };
    let t = rig
        .d
        .create_texture(&TextureDesc::tex2d(dxgi::R32G32_UINT, 8, 8, SRV | UAV))
        .unwrap();
    let init = 0x0000_0001_0000_0000u64;
    let bytes: Vec<u8> = (0..64).flat_map(|_| init.to_le_bytes()).collect();
    upload(&rig.d, &rig.q, &t, 0, &bytes, 64);
    let view = t.view(&ViewSpec::whole(ViewDim::D2)).unwrap();
    let hs = heaps(&rig.d, &[view], &[]);
    rig.run_pixels(&p, [8, 8], &[], &[], 1, Some((&hs.0, &hs.1)));
    let out = download(&rig.d, &rig.q, &t, 64);
    let got: Vec<u64> = out
        .chunks(8)
        .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
        .collect();
    for y in 0..8usize {
        for x in 0..8usize {
            let g = got[y * 8 + x];
            if x > y {
                assert_eq!(g, init, "uncovered ({x},{y}) was written");
                continue;
            }
            let a = (((x + 1) as u64) << 33) | ((y * 8 + x + 1) as u64);
            let b = ((x as u64) << 33) | 0x1_ffff_ffff;
            let mut want = init.max(a).max(b);
            if (x, y) == (0, 0) {
                // the texel (0,0) also takes the minimum of 3 + y over the covered pixels
                want = want.min(3);
            }
            assert_eq!(g, want, "pixel ({x},{y}): got {g:#x} want {want:#x}");
        }
    }
}

/// The plan for the one program that uses the original value
/// (`PS_Weather_Particle_Prepass_74a7a22c`: `image_atomic_umax` with glc, the high word of
/// the original compared with the pixel's depth, discard if the pixel is behind): read the texel,
/// then the 64-bit `InterlockedMax` without the original. A max only grows, so the value read before
/// is never above the one the atomic would have returned, and "my depth < the old depth" can only be
/// missed (the pixel is drawn that would have been discarded), never wrongly taken. Checked here
/// without a race: one thread per texel; the read gives the initial value and the texel ends as the max.
#[test]
fn a_64_bit_texel_read_then_max_gives_the_value_before_and_the_max() {
    let rig = rig();
    let Some(dxil) = dxil_checked(
        "a64_read_then_max",
        "cs_6_6",
        &format!(
            "{PRELUDE}[numthreads(4,4,1)] void main(uint3 id : SV_DispatchThreadID) {{\n\
               RWTexture2D<uint64_t> t = ResourceDescriptorHeap[0];\n\
               uint64_t v = ((uint64_t)(id.x * 7 + id.y * 5 + 1) << 32) | (uint64_t)(id.y * 3 + id.x);\n\
               uint64_t old = t[id.xy];\n\
               InterlockedMax(t[id.xy], v);\n\
               O.Store2((id.y * 4 + id.x) * 8, uint2((uint)old, (uint)(old >> 32))); }}\n"
        ),
        &["dx.op.atomicBinOp.i64"],
    ) else {
        return;
    };
    if let Err(e) = rig.try_build(&dxil, HEAPS) {
        eprintln!("MSC/Metal refuse the 64-bit texel read: {e}");
        panic!("a 64-bit typed read is refused: {e}");
    }
    let p = rig.prog_with(&dxil, HEAPS);
    let t = rig
        .d
        .create_texture(&TextureDesc::tex2d(dxgi::R32G32_UINT, 4, 4, SRV | UAV))
        .unwrap();
    // texel i starts at (i % 5) << 32 | 0x10: the pixel's value is above it in some texels, below in others
    let init = |i: u64| ((i % 5) << 32) | 0x10;
    let bytes: Vec<u8> = (0..16u64).flat_map(|i| init(i).to_le_bytes()).collect();
    upload(&rig.d, &rig.q, &t, 0, &bytes, 32);
    let view = t.view(&ViewSpec::whole(ViewDim::D2)).unwrap();
    let hs = heaps(&rig.d, &[view], &[]);
    let got = rig.run_heaps(&p, [1, 1, 1], [4, 4, 1], &[], &[], 32, Some((&hs.0, &hs.1)));
    let out = download(&rig.d, &rig.q, &t, 32);
    for y in 0..4u64 {
        for x in 0..4u64 {
            let i = y * 4 + x;
            let v = ((x * 7 + y * 5 + 1) << 32) | (y * 3 + x);
            let old = u64::from(got[(i * 2) as usize]) | u64::from(got[(i * 2 + 1) as usize]) << 32;
            assert_eq!(old, init(i), "the value read at ({x},{y})");
            let now = u64::from_le_bytes(
                out[(i * 8) as usize..(i * 8 + 8) as usize]
                    .try_into()
                    .unwrap(),
            );
            assert_eq!(now, init(i).max(v), "the texel at ({x},{y}) after the max");
        }
    }
}
