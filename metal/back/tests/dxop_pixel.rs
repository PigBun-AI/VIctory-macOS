//! Pixel-stage dx.op nodes (map-plan/inventory.md, the 21 dx.op without a GPU
//! case): `discard`, `isHelperLane`, `quadReadLaneAt` and `quadOp` in a pixel
//! shader, `sample`, `sampleBias`, `sampleGrad`, `calculateLOD`,
//! `textureGather`, `sampleCmpLevelZero`. The pass has no render target; the
//! shader writes its results through the root UAV `O`, one record per pixel
//! (`common_dxop::PixProg`), over a full-screen triangle, so every pixel quad
//! is whole.
//!
//! Sources: D3D11.3 Functional Spec (fetched 2026-11-28) 3.3.x "Helper pixels
//! / derivatives" and the HLSL `discard` / `IsHelperLane` documentation
//! (SM 6.6 "Quad-wide derivatives and helper lanes": a lane that discarded
//! stays a helper lane in its quad, quad reads still see its values;
//! IsHelperLane() is true there).
#![cfg(target_vendor = "apple")]

mod common_dxop;
mod common_msc;

use common_dxop::*;
use victory_metal_back::format::dxgi;
use victory_metal_back::sampler::{Address, Compare, Filter, SamplerSpec};
use victory_metal_back::texture::TextureDesc;
use victory_metal_back::texture::{ViewDim, ViewSpec};

const N: usize = 8;

fn px(i: usize) -> (usize, usize) {
    (i % N, i / N)
}

/// `discard`: the pixels it is taken on keep what was written before it and
/// lose what comes after (stores after a discard do not happen; D3D12 spec:
/// the pixel is discarded at the end of the shader, UAV writes already done
/// stay, later ones are not performed).
#[test]
fn discard_stops_the_pixel_writes_that_follow() {
    let rig = rig();
    let Some(p) = rig.pixel_program(
        "px_discard",
        "void main(float4 pos : SV_Position) {\n\
           uint x = (uint)pos.x, y = (uint)pos.y; uint i = y*8+x;\n\
           O.Store(i*12, 1);\n\
           if (((x + y * 3) % 4) == k0) discard;\n\
           O.Store(i*12+4, 2);\n\
           O.Store(i*12+8, 3); }\n",
        &["dx.op.discard"],
        false,
    ) else {
        return;
    };
    for k0 in 0..5u32 {
        let got = rig.run_pixels(&p, [8, 8], &[k0], &[], N * N * 3, None);
        for i in 0..N * N {
            let (x, y) = px(i);
            let disc = ((x + y * 3) % 4) as u32 == k0;
            let want = [
                1,
                if disc { 0xDEAD_BEEF } else { 2 },
                if disc { 0xDEAD_BEEF } else { 3 },
            ];
            assert_eq!(
                &got[i * 3..i * 3 + 3],
                &want,
                "k0={k0} pixel ({x},{y}) discard={disc}"
            );
        }
    }
}

/// Pixels a primitive does not cover are helper lanes of the quads it
/// touches: they run the shader (so the covered neighbours can read their
/// values with quad reads and take derivatives from them), their stores are
/// not performed, and `IsHelperLane()` is true in them. One triangle over the
/// lower-left half of the target (`HALF_VS`): the covered pixel (i, j) has
/// i <= j; the diagonal quads have their upper-right pixel uncovered.
#[test]
fn uncovered_pixels_are_helper_lanes_the_quad_can_read() {
    let rig = rig();
    let Some(p) = rig.pixel_program_vs(
        "px_helper_uncovered",
        HALF_VS,
        "void main(float4 pos : SV_Position) {\n\
           uint x = (uint)pos.x, y = (uint)pos.y; uint i = y*8+x;\n\
           uint v = 100 + y*8 + x;\n\
           uint h = IsHelperLane() ? 1u : 0u;\n\
           uint a = IsHelperLane() ? 77u : v;\n\
           O.Store4(i*32, uint4(h, QuadReadAcrossX(v), QuadReadAcrossY(v), QuadReadAcrossDiagonal(v)));\n\
           O.Store4(i*32+16, uint4(QuadReadAcrossX(h), QuadReadAcrossY(h), QuadReadAcrossDiagonal(h), QuadReadAcrossX(a))); }\n",
        &["dx.op.isHelperLane.i1", "dx.op.quadOp.i32"],
        false,
    ) else {
        return;
    };
    let got = rig.run_pixels(&p, [8, 8], &[], &[], N * N * 8, None);
    let covered = |x: usize, y: usize| x <= y;
    let mut bad = Vec::new();
    for i in 0..N * N {
        let (x, y) = px(i);
        let r = &got[i * 8..i * 8 + 8];
        if !covered(x, y) {
            if r.iter().any(|&w| w != 0xDEAD_BEEF) {
                bad.push(format!("({x},{y}) uncovered lane stored {r:x?}"));
            }
            continue;
        }
        let v = |xx: usize, yy: usize| 100 + (yy * 8 + xx) as u32;
        let helper = |xx: usize, yy: usize| u32::from(!covered(xx, yy));
        let want = [
            0,
            v(x ^ 1, y),
            v(x, y ^ 1),
            v(x ^ 1, y ^ 1),
            helper(x ^ 1, y),
            helper(x, y ^ 1),
            helper(x ^ 1, y ^ 1),
            if covered(x ^ 1, y) { v(x ^ 1, y) } else { 77 },
        ];
        if r != want {
            bad.push(format!("({x},{y}) got {r:x?} want {want:x?}"));
        }
    }
    assert!(
        bad.is_empty(),
        "{} pixels differ, first: {:?}",
        bad.len(),
        &bad[..bad.len().min(4)]
    );
}

/// What `discard` does to the quad (SM 6.x: a discarded lane becomes a
/// helper lane; its neighbours still read the values it computed before
/// the discard). Pinned to what Metal does: the values computed before the
/// discard can be read, `IsHelperLane()` after the discard is true in the
/// discarded lane (not observable by its own stores); reads of a value
/// computed *after* the discard from the discarded lane are what the notes of
/// S-D-0xx call unreliable (the lane no longer runs those instructions).
#[test]
fn a_discarded_lane_keeps_the_values_it_made_before_the_discard() {
    let rig = rig();
    let Some(p) = rig.pixel_program(
        "px_discard_quad",
        "void main(float4 pos : SV_Position) {\n\
           uint x = (uint)pos.x, y = (uint)pos.y; uint i = y*8+x;\n\
           uint v = 100 + y*8 + x;\n\
           bool dis = (x & 1) == k0 && (y & 1) == k1;\n\
           uint ax = QuadReadAcrossX(v), ay = QuadReadAcrossY(v), ad = QuadReadAcrossDiagonal(v);\n\
           if (dis) discard;\n\
           uint h = IsHelperLane() ? 1u : 0u;\n\
           O.Store4(i*16, uint4(h, ax, ay, ad));\n\
           O.Store(1024 + i*4, QuadReadAcrossX(h) | (QuadReadAcrossY(h) << 1) | (QuadReadAcrossDiagonal(h) << 2)); }\n",
        &["dx.op.discard", "dx.op.isHelperLane.i1", "dx.op.quadOp.i32"],
        false,
    ) else {
        return;
    };
    for (k0, k1) in [(0u32, 0u32), (1, 0), (0, 1), (1, 1)] {
        let got = rig.run_pixels(&p, [8, 8], &[k0, k1], &[], 256 + N * N, None);
        let mut odd = 0;
        for i in 0..N * N {
            let (x, y) = px(i);
            let dis = (x & 1) as u32 == k0 && (y & 1) as u32 == k1;
            if !dis {
                // The helper flags the neighbours see (bit 0 across X, 1 across Y, 2 diagonal):
                // only the discarded lane is a helper.
                let d = |xx: usize, yy: usize| {
                    u32::from((xx & 1) as u32 == k0 && (yy & 1) as u32 == k1)
                };
                let want = d(x ^ 1, y) | (d(x, y ^ 1) << 1) | (d(x ^ 1, y ^ 1) << 2);
                odd += usize::from(got[256 + i] != want);
            }
            let r = &got[i * 4..i * 4 + 4];
            if dis {
                assert!(
                    r.iter().all(|&w| w == 0xDEAD_BEEF),
                    "({x},{y}) discarded lane stored {r:x?}"
                );
                continue;
            }
            let v = |xx: usize, yy: usize| 100 + (yy * 8 + xx) as u32;
            assert_eq!(
                r,
                &[0, v(x ^ 1, y), v(x, y ^ 1), v(x ^ 1, y ^ 1)],
                "discard lane ({k0},{k1}) pixel ({x},{y})"
            );
        }
        // Measured, not asserted: a quad read of `IsHelperLane()` made after the discard
        // does not always show the discarded lane alone (on the M5 Max some quads show
        // one more helper); see the S-D-0xx note.
        eprintln!(
            "discard lane ({k0},{k1}): {odd} of {} live pixels read a different helper set than the discarded lane alone",
            N * N * 3 / 4
        );
    }
}

/// `quadReadLaneAt` and `quadOp` in a pixel shader: pixel (x, y) of a quad is
/// lane `(x & 1) + 2 * (y & 1)` (0 upper left, 1 upper right, 2 lower left,
/// 3 lower right, D3D11.3 spec: derivative quads).
#[test]
fn quad_reads_in_a_pixel_shader() {
    let rig = rig();
    let Some(p) = rig.pixel_program(
        "px_quad",
        "void main(float4 pos : SV_Position) {\n\
           uint x = (uint)pos.x, y = (uint)pos.y; uint i = y*8+x;\n\
           uint v = 1000 + y*8 + x;\n\
           O.Store4(i*32, uint4(QuadReadLaneAt(v, k0), QuadReadAcrossX(v), QuadReadAcrossY(v), QuadReadAcrossDiagonal(v)));\n\
           O.Store4(i*32+16, uint4(QuadReadLaneAt(v, (x + k1) & 3), asuint(QuadReadAcrossX(asfloat(v))), QuadReadLaneAt(v, y & 3), 0)); }\n",
        &["dx.op.quadReadLaneAt.i32", "dx.op.quadOp.i32"],
        false,
    ) else {
        return;
    };
    for (k0, k1) in [(0u32, 0u32), (1, 1), (2, 2), (3, 3)] {
        let got = rig.run_pixels(&p, [8, 8], &[k0, k1], &[], N * N * 8, None);
        let mut want = vec![0u32; N * N * 8];
        for i in 0..N * N {
            let (x, y) = px(i);
            let v = |xx: usize, yy: usize| 1000 + (yy * 8 + xx) as u32;
            let (qx, qy) = (x & !1, y & !1);
            let lane = |l: usize| v(qx + (l & 1), qy + (l >> 1));
            want[i * 8..i * 8 + 8].copy_from_slice(&[
                lane(k0 as usize),
                v(x ^ 1, y),
                v(x, y ^ 1),
                v(x ^ 1, y ^ 1),
                lane((x + k1 as usize) & 3),
                v(x ^ 1, y),
                lane(y & 3),
                0,
            ]);
        }
        assert_words(&format!("k0={k0} k1={k1}"), &got, &want);
    }
}

const LOD_PS: &str = "void main(float4 pos : SV_Position) {\n\
   uint x = (uint)pos.x, y = (uint)pos.y; uint i = y*8+x;\n\
   Texture2D<float4> tx = ResourceDescriptorHeap[0];\n\
   SamplerState s = SamplerDescriptorHeap[0];\n\
   float2 uv = float2(x * asfloat(k0), y * asfloat(k1)) / 8.0;\n\
   float a = tx.Sample(s, uv).x;\n\
   float b = tx.SampleBias(s, uv, asfloat(k2)).x;\n\
   float l = tx.CalculateLevelOfDetail(s, uv);\n\
   float lu = tx.CalculateLevelOfDetailUnclamped(s, uv);\n\
   O.Store4(i*16, asuint(float4(a, b, l, lu))); }\n";

/// `sample`, `sampleBias`, `calculateLOD` (clamped and unclamped): the LOD of
/// a quad follows the per-pixel step of the coordinates in texels, the bias
/// adds to it (D3D11.3 spec 7.18.x: LOD = log2(max step) + bias, clamped to
/// the sampler's and the texture's range).
#[test]
fn sample_sample_bias_and_calculate_lod_in_a_pixel_shader() {
    let rig = rig();
    let Some(p) = rig.pixel_program(
        "px_lod",
        LOD_PS,
        &[
            "dx.op.sample.f32",
            "dx.op.sampleBias.f32",
            "dx.op.calculateLOD.f32",
        ],
        true,
    ) else {
        return;
    };
    let t = lod_texture(&rig.d, &rig.q);
    let view = t.view(&ViewSpec::whole(ViewDim::D2)).unwrap();
    let hs = heaps(&rig.d, &[view], &[trilinear()]);
    let mut report = Vec::new();
    for (sx, sy, bias) in [
        (1.0f32, 1.0f32, 0.0f32),
        (2.0, 1.0, 0.0),
        (1.0, 4.0, 0.0),
        (2.828_427, 2.828_427, 0.0),
        (2.0, 2.0, 1.0),
        (2.0, 2.0, -0.5),
        (2.0, 2.0, 5.0),
        (0.5, 0.5, 0.0),
        (0.5, 0.5, 1.0),
        (8.0, 1.0, 0.0),
    ] {
        let got = rig.run_pixels(
            &p,
            [8, 8],
            &[sx.to_bits(), sy.to_bits(), bias.to_bits()],
            &[],
            64 * 4,
            Some((&hs.0, &hs.1)),
        );
        let lod = sx.max(sy).log2();
        let clamped = lod.clamp(0.0, 3.0);
        let biased = (lod + bias).clamp(0.0, 3.0);
        let g = |i: usize, c: usize| f32::from_bits(got[i * 4 + c]);
        report.push(format!(
            "step ({sx},{sy}) bias {bias}: Sample {:.3}/{:.3}, SampleBias {:.3}/{:.3}, LOD {:.3}/{:.3}, unclamped {:.3}/{:.3} (got/want)",
            g(9, 0), 1.0 + clamped, g(9, 1), 1.0 + biased, g(9, 2), clamped, g(9, 3), lod
        ));
        for i in 0..64 {
            let (x, y) = px(i);
            let what = format!("step ({sx},{sy}) bias {bias} pixel ({x},{y})");
            assert!(
                close(g(i, 0), 1.0 + clamped, 0.0625),
                "{what}: Sample {} want {}",
                g(i, 0),
                1.0 + clamped
            );
            assert!(
                close(g(i, 1), 1.0 + biased, 0.0625),
                "{what}: SampleBias {} want {}",
                g(i, 1),
                1.0 + biased
            );
            assert!(
                close(g(i, 2), clamped, 0.0625),
                "{what}: CalculateLOD {} want {clamped}",
                g(i, 2)
            );
            assert!(
                close(g(i, 3), lod, 0.0625),
                "{what}: unclamped LOD {} want {lod}",
                g(i, 3)
            );
        }
    }
    eprintln!("{}", report.join("\n"));
}

/// `sampleGrad` in a pixel shader: the explicit gradients decide the LOD.
#[test]
fn sample_grad_in_a_pixel_shader() {
    let rig = rig();
    let Some(p) = rig.pixel_program(
        "px_grad",
        "void main(float4 pos : SV_Position) {\n\
           uint x = (uint)pos.x, y = (uint)pos.y; uint i = y*8+x;\n\
           Texture2D<float4> tx = ResourceDescriptorHeap[0];\n\
           SamplerState s = SamplerDescriptorHeap[0];\n\
           float4 a = tx.SampleGrad(s, float2(0.4, 0.6), float2(asfloat(k0), asfloat(k1)), float2(asfloat(k2), asfloat(k3)));\n\
           O.Store(i*4, asuint(a.x)); }\n",
        &["dx.op.sampleGrad.f32"],
        true,
    ) else {
        return;
    };
    let t = lod_texture(&rig.d, &rig.q);
    let view = t.view(&ViewSpec::whole(ViewDim::D2)).unwrap();
    let hs = heaps(&rig.d, &[view], &[trilinear()]);
    for (dx, dy) in [
        ([0.125f32, 0.0f32], [0.0f32, 0.125f32]),
        ([0.25, 0.0], [0.0, 0.25]),
        ([0.125, 0.0], [0.0, 0.5]),
        ([0.5, 0.5], [0.0, 0.0]),
        ([0.0, 0.0], [0.0, 0.0]),
        ([-0.25, 0.0], [0.0, -0.25]),
        ([4.0, 0.0], [0.0, 0.0]),
    ] {
        let got = rig.run_pixels(
            &p,
            [8, 8],
            &[
                dx[0].to_bits(),
                dx[1].to_bits(),
                dy[0].to_bits(),
                dy[1].to_bits(),
            ],
            &[],
            64,
            Some((&hs.0, &hs.1)),
        );
        let texels = [dx, dy]
            .iter()
            .map(|v| (v[0] * v[0] + v[1] * v[1]).sqrt() * 8.0)
            .fold(0.0f32, f32::max)
            .max(1.0);
        let want = 1.0 + texels.log2().clamp(0.0, 3.0);
        for (i, w) in got.iter().enumerate() {
            let r = f32::from_bits(*w);
            assert!(
                close(r, want, 0.0625),
                "grad {dx:?} {dy:?} pixel {i}: {r} want {want}"
            );
        }
    }
}

/// `textureGather` and `sampleCmpLevelZero` in a pixel shader: as in compute
/// (`dxop_tex.rs`), gather in the D3D order, comparison filtering the
/// weighted comparison results.
#[test]
fn gather_and_comparison_sampling_in_a_pixel_shader() {
    let rig = rig();
    let Some(p) = rig.pixel_program(
        "px_gather_cmp",
        "void main(float4 pos : SV_Position) {\n\
           uint x = (uint)pos.x, y = (uint)pos.y; uint i = y*8+x;\n\
           Texture2D<float4> tx = ResourceDescriptorHeap[0];\n\
           Texture2D<float> dp = ResourceDescriptorHeap[1];\n\
           SamplerState s = SamplerDescriptorHeap[0];\n\
           SamplerComparisonState sc = SamplerDescriptorHeap[1];\n\
           float2 uv = float2(asfloat(k0), asfloat(k1));\n\
           O.Store4(i*32, asuint(tx.GatherRed(s, uv)));\n\
           O.Store4(i*32+16, asuint(tx.GatherAlpha(s, uv, int2(1, -1))));\n\
           O.Store(i*32+8+16+4, asuint(dp.SampleCmpLevelZero(sc, uv, asfloat(k2)))); }\n",
        &["dx.op.textureGather.f32", "dx.op.sampleCmpLevelZero.f32"],
        true,
    ) else {
        return;
    };
    let t = gather_texture(&rig.d, &rig.q);
    let tv = t.view(&ViewSpec::whole(ViewDim::D2)).unwrap();
    let dt = rig
        .d
        .create_texture(&TextureDesc::tex2d(40, 4, 4, DS | SRV))
        .unwrap();
    let depth = |x: i32, y: i32| 0.05f32 + 0.05 * (x.clamp(0, 3) + 4 * y.clamp(0, 3)) as f32;
    let mut dv = Vec::new();
    for y in 0..4 {
        for x in 0..4 {
            dv.push(depth(x, y));
        }
    }
    upload(&rig.d, &rig.q, &dt, 0, &f32s(&dv), 16);
    let mut vs = ViewSpec::whole(ViewDim::D2);
    vs.format = dxgi::R32_FLOAT;
    let dview = dt.view(&vs).unwrap();
    let point = SamplerSpec {
        min: Filter::Nearest,
        mag: Filter::Nearest,
        mip: None,
        address: [Address::ClampToEdge; 3],
        ..Default::default()
    };
    let cmp = SamplerSpec {
        min: Filter::Linear,
        mag: Filter::Linear,
        mip: None,
        address: [Address::ClampToEdge; 3],
        compare: Some(Compare::LessEqual),
        ..Default::default()
    };
    let hs = heaps(&rig.d, &[tv, dview], &[point, cmp]);
    for (uv, r) in [
        ([0.30f32, 0.40f32], 0.40f32),
        ([0.60, 0.80], 0.5),
        ([0.05, 0.05], 0.1),
        ([0.97, 0.97], 0.9),
    ] {
        let got = rig.run_pixels(
            &p,
            [8, 8],
            &[uv[0].to_bits(), uv[1].to_bits(), r.to_bits()],
            &[],
            64 * 8,
            Some((&hs.0, &hs.1)),
        );
        let (fx, fy) = (uv[0] * 4.0 - 0.5, uv[1] * 4.0 - 0.5);
        let (i0, j0) = (fx.floor() as i32, fy.floor() as i32);
        let g4 = |c: usize, dx: i32, dy: i32| {
            [
                texel(i0 + dx, j0 + 1 + dy, c),
                texel(i0 + 1 + dx, j0 + 1 + dy, c),
                texel(i0 + 1 + dx, j0 + dy, c),
                texel(i0 + dx, j0 + dy, c),
            ]
        };
        let (wx, wy) = (fx - i0 as f32, fy - j0 as f32);
        let c = |x, y| f32::from(u8::from(r <= depth(x, y)));
        let pcf = (1.0 - wx) * (1.0 - wy) * c(i0, j0)
            + wx * (1.0 - wy) * c(i0 + 1, j0)
            + (1.0 - wx) * wy * c(i0, j0 + 1)
            + wx * wy * c(i0 + 1, j0 + 1);
        for i in 0..64 {
            let w: Vec<f32> = got[i * 8..i * 8 + 8]
                .iter()
                .map(|&w| f32::from_bits(w))
                .collect();
            assert_eq!(&w[0..4], &g4(0, 0, 0), "uv {uv:?} pixel {i}: GatherRed");
            // the comparison result overlaps the last gather word (offset 28): the gather alpha is first four words at +16
            assert_eq!(
                &w[4..7],
                &g4(3, 1, -1)[0..3],
                "uv {uv:?} pixel {i}: GatherAlpha with offset"
            );
            assert!(
                close(w[7], pcf, 0.02),
                "uv {uv:?} ref {r} pixel {i}: SampleCmp {} want {pcf}",
                w[7]
            );
        }
    }
}
