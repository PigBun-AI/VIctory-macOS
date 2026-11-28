//! Texture dx.op nodes of the DXIL -> MSC hop (map-plan/inventory.md, the 21
//! dx.op without a GPU case): `sample`, `sampleBias`, `sampleGrad`,
//! `sampleCmpLevelZero`, `textureGather`, `calculateLOD`, `sampleLevel`.
//!
//! The shaders run as compute (SM 6.6 quad derivatives: 8x8 groups, the 2D
//! quad rule of the "HLSL Derivative Operations in Compute, Mesh and
//! Amplification Shaders" spec) because that reads results back without a
//! render pass; the pixel-stage versions of the same operations are in
//! `dxop_pixel.rs`.
//!
//! Method: a four-level mip chain whose every level is one constant
//! (`1 + level`), sampled trilinearly, returns `1 + LOD` (D3D11.3 spec
//! 7.18.x, "Texture sampling": trilinear = lerp of the two nearest levels by
//! the fraction of the LOD), so one float tells what LOD the GPU used. The
//! LOD of a quad is `log2(max(|ddx|, |ddy|) * size)` (isotropic, no
//! anisotropy); hardware computes it with limited fraction bits, hence the
//! tolerance of 1/16 LOD. Gather and the comparison sample are compared with
//! the D3D11.3 definitions (gather4: x=(i0,j1) y=(i1,j1) z=(i1,j0) w=(i0,j0);
//! comparison filtering = bilinear weights over the four comparison results).
#![cfg(target_vendor = "apple")]

mod common_dxop;
mod common_msc;

use common_dxop::*;
use victory_metal_back::format::dxgi;
use victory_metal_back::sampler::{Address, Compare, Filter, SamplerSpec};
use victory_metal_back::texture::{TextureDesc, ViewDim, ViewSpec};

const LOD_BODY: &str = "\
[numthreads(8,8,1)] void main(uint3 tg : SV_GroupThreadID, uint gi : SV_GroupIndex) {\n\
   Texture2D<float4> tx = ResourceDescriptorHeap[0];\n\
   SamplerState s = SamplerDescriptorHeap[0];\n\
   float2 uv = float2(tg.x * asfloat(k0), tg.y * asfloat(k1)) / 8.0;\n\
   float a = tx.Sample(s, uv).x;\n\
   float b = tx.SampleBias(s, uv, asfloat(k2)).x;\n\
   O.Store2(gi*8, asuint(float2(a, b))); }\n";

/// `sample` (implicit derivatives) in a compute quad, and `sampleBias`, whose bias MSC drops there (`calculateLOD`
/// is not converted in compute stages, see `dxop_pixel.rs`): the LOD follows the per-pixel step of the
/// coordinates, the bias adds to it, the result is clamped to the mip chain.
#[test]
fn sample_sample_bias_and_calculate_lod_follow_the_quad_derivatives() {
    let rig = rig();
    let Some(p) = rig.program_heaps(
        "tex_lod",
        LOD_BODY,
        &["dx.op.sample.f32", "dx.op.sampleBias.f32"],
    ) else {
        return;
    };
    let t = lod_texture(&rig.d, &rig.q);
    let view = t.view(&ViewSpec::whole(ViewDim::D2)).unwrap();
    let hs = heaps(&rig.d, &[view], &[trilinear()]);
    // (texels per pixel in x, in y, bias)
    for (sx, sy, bias) in [
        (1.0f32, 1.0f32, 0.0f32),
        (2.0, 1.0, 0.0),
        (1.0, 4.0, 0.0),
        (2.828_427, 2.828_427, 0.0),
        (1.5, 1.0, 0.0),
        (2.0, 2.0, 1.0),
        (2.0, 2.0, -0.5),
        (2.0, 2.0, 5.0),
        (0.5, 0.5, 0.0),
        (0.5, 0.5, 1.0),
        (8.0, 1.0, 0.0),
    ] {
        let got = rig.run_heaps(
            &p,
            [1, 1, 1],
            [8, 8, 1],
            &[sx.to_bits(), sy.to_bits(), bias.to_bits()],
            &[],
            64 * 2,
            Some((&hs.0, &hs.1)),
        );
        let lod = sx.max(sy).log2();
        let clamped = lod.clamp(0.0, 3.0);
        for gi in 0..64 {
            let g = |i: usize| f32::from_bits(got[gi * 2 + i]);
            let what = format!("s=({sx},{sy}) bias {bias} thread {gi}");
            assert!(
                close(g(0), 1.0 + clamped, 0.0625),
                "{what}: Sample {} want {}",
                g(0),
                1.0 + clamped
            );
            // MSC 4.0.3 (measured 2026-11-28): in a compute stage the bias of SampleBias is
            // dropped, the result is Sample's (the stage-by-op census: sampleBias is used by
            // 906 pixel programs and no compute one; the pixel stage is exact, dxop_pixel.rs).
            assert!(
                close(g(1), 1.0 + clamped, 0.0625),
                "{what}: SampleBias {} (Sample {}); the compute stage ignores the bias",
                g(1),
                g(0)
            );
        }
    }
}

const GRAD_BODY: &str = "[numthreads(8,8,1)] void main(uint3 tg : SV_GroupThreadID, uint gi : SV_GroupIndex) {\n\
   Texture2D<float4> tx = ResourceDescriptorHeap[0];\n\
   SamplerState s = SamplerDescriptorHeap[0];\n\
   float2 uv = float2(0.4, 0.6);\n\
   float4 a = tx.SampleGrad(s, uv, float2(asfloat(k0), asfloat(k1)), float2(asfloat(k2), asfloat(k3)));\n\
   O.Store4(gi*16, asuint(a)); }\n";

/// `sampleGrad`: explicit gradients, the LOD from the larger of the two.
#[test]
fn sample_grad_uses_the_given_gradients() {
    let rig = rig();
    let Some(p) = rig.program_heaps("tex_grad", GRAD_BODY, &["dx.op.sampleGrad.f32"]) else {
        return;
    };
    let t = lod_texture(&rig.d, &rig.q);
    let view = t.view(&ViewSpec::whole(ViewDim::D2)).unwrap();
    let hs = heaps(&rig.d, &[view], &[trilinear()]);
    for (dx, dy) in [
        ([0.125f32, 0.0f32], [0.0f32, 0.125f32]), // one texel -> LOD 0
        ([0.25, 0.0], [0.0, 0.25]),               // two texels -> 1
        ([0.125, 0.0], [0.0, 0.5]),               // four texels in y -> 2
        ([0.5, 0.5], [0.0, 0.0]),                 // 5.66 texels -> 2.5
        ([0.0, 0.0], [0.0, 0.0]),                 // none: LOD 0
        ([-0.25, 0.0], [0.0, -0.25]),             // signs do not matter
        ([4.0, 0.0], [0.0, 0.0]),                 // past the chain: clamps to 3
    ] {
        let got = rig.run_heaps(
            &p,
            [1, 1, 1],
            [8, 8, 1],
            &[
                dx[0].to_bits(),
                dx[1].to_bits(),
                dy[0].to_bits(),
                dy[1].to_bits(),
            ],
            &[],
            64 * 4,
            Some((&hs.0, &hs.1)),
        );
        let texels = [dx[0], dx[1], dy[0], dy[1]]
            .chunks(2)
            .map(|v| (v[0] * v[0] + v[1] * v[1]).sqrt() * 8.0)
            .fold(0.0f32, f32::max)
            .max(1.0);
        let want = 1.0 + texels.log2().clamp(0.0, 3.0);
        for gi in 0..64 {
            let r = f32::from_bits(got[gi * 4]);
            assert!(
                close(r, want, 0.0625),
                "grad {dx:?} {dy:?} thread {gi}: {r} want {want}"
            );
        }
    }
}

/// `textureGather`: the four texels a bilinear fetch would use, in the order
/// (i0,j1) (i1,j1) (i1,j0) (i0,j0), for each channel, with clamped edges and
/// an offset.
#[test]
fn texture_gather_returns_the_four_texels_in_d3d_order() {
    let rig = rig();
    let Some(p) = rig.program_heaps(
        "tex_gather",
        "[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
           Texture2D<float4> tx = ResourceDescriptorHeap[0];\n\
           SamplerState s = SamplerDescriptorHeap[0];\n\
           float2 uv = float2(asfloat(k0), asfloat(k1));\n\
           float4 r = tx.GatherRed(s, uv);\n\
           float4 g = tx.GatherGreen(s, uv);\n\
           float4 b = tx.GatherBlue(s, uv);\n\
           float4 a = tx.GatherAlpha(s, uv);\n\
           float4 o = tx.GatherRed(s, uv, int2(1, -1));\n\
           O.Store4(t.x*80, asuint(r)); O.Store4(t.x*80+16, asuint(g)); O.Store4(t.x*80+32, asuint(b));\n\
           O.Store4(t.x*80+48, asuint(a)); O.Store4(t.x*80+64, asuint(o)); }\n",
        &["dx.op.textureGather.f32"],
    ) else {
        return;
    };
    let t = gather_texture(&rig.d, &rig.q);
    let view = t.view(&ViewSpec::whole(ViewDim::D2)).unwrap();
    let nearest = SamplerSpec {
        min: Filter::Nearest,
        mag: Filter::Nearest,
        mip: None,
        address: [Address::ClampToEdge; 3],
        ..Default::default()
    };
    let hs = heaps(&rig.d, &[view], &[nearest]);
    // uv in the middle of texel groups, near edges, and exactly on a texel boundary
    for uv in [
        [0.30f32, 0.40f32],
        [0.60, 0.80],
        [0.05, 0.05],
        [0.97, 0.97],
        [0.50, 0.50],
        [0.26, 0.74],
    ] {
        let got = rig.run_heaps(
            &p,
            [1, 1, 1],
            [32, 1, 1],
            &[uv[0].to_bits(), uv[1].to_bits()],
            &[],
            32 * 20,
            Some((&hs.0, &hs.1)),
        );
        let (fx, fy) = (uv[0] * 4.0 - 0.5, uv[1] * 4.0 - 0.5);
        let (i0, j0) = (fx.floor() as i32, fy.floor() as i32);
        let order = |c: usize, dx: i32, dy: i32| {
            [
                texel(i0 + dx, j0 + 1 + dy, c),
                texel(i0 + 1 + dx, j0 + 1 + dy, c),
                texel(i0 + 1 + dx, j0 + dy, c),
                texel(i0 + dx, j0 + dy, c),
            ]
        };
        let mut want = Vec::new();
        for c in 0..4 {
            want.extend(order(c, 0, 0));
        }
        want.extend(order(0, 1, -1));
        for t in 0..32 {
            let g: Vec<f32> = got[t * 20..t * 20 + 20]
                .iter()
                .map(|&w| f32::from_bits(w))
                .collect();
            assert_eq!(g, want, "uv {uv:?} thread {t}");
        }
    }
}

/// `sampleCmpLevelZero`: a comparison sampler on a depth texture, point
/// (exact) and bilinear (percentage-closer: the weights of the four
/// comparisons).
#[test]
fn sample_cmp_level_zero_on_a_depth_texture() {
    let rig = rig();
    let Some(p) = rig.program_heaps(
        "tex_cmp",
        "[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
           Texture2D<float> tx = ResourceDescriptorHeap[0];\n\
           SamplerComparisonState sp = SamplerDescriptorHeap[0];\n\
           SamplerComparisonState sl = SamplerDescriptorHeap[1];\n\
           float2 uv = float2(asfloat(k0), asfloat(k1));\n\
           float r = asfloat(k2);\n\
           O.Store2(t.x*8, asuint(float2(tx.SampleCmpLevelZero(sp, uv, r), tx.SampleCmpLevelZero(sl, uv, r)))); }\n",
        &["dx.op.sampleCmpLevelZero.f32"],
    ) else {
        return;
    };
    // 4x4 depth, d(x, y) = 0.05 + 0.05 * (x + 4 y)
    let dt = rig
        .d
        .create_texture(&TextureDesc::tex2d(dxgi::D32_FLOAT, 4, 4, DS | SRV))
        .unwrap();
    let depth = |x: i32, y: i32| 0.05f32 + 0.05 * (x.clamp(0, 3) + 4 * y.clamp(0, 3)) as f32;
    let mut v = Vec::new();
    for y in 0..4 {
        for x in 0..4 {
            v.push(depth(x, y));
        }
    }
    upload(&rig.d, &rig.q, &dt, 0, &f32s(&v), 16);
    let mut vs = ViewSpec::whole(ViewDim::D2);
    vs.format = dxgi::R32_FLOAT;
    let view = dt.view(&vs).unwrap();
    let base = SamplerSpec {
        mip: None,
        address: [Address::ClampToEdge; 3],
        compare: Some(Compare::LessEqual),
        ..Default::default()
    };
    let point = SamplerSpec {
        min: Filter::Nearest,
        mag: Filter::Nearest,
        ..base
    };
    let lin = SamplerSpec {
        min: Filter::Linear,
        mag: Filter::Linear,
        ..base
    };
    let hs = heaps(&rig.d, &[view], &[point, lin]);
    for (uv, r) in [
        ([0.30f32, 0.40f32], 0.40f32),
        ([0.30, 0.40], 0.05),
        ([0.30, 0.40], 1.0),
        ([0.55, 0.55], 0.5),
        ([0.375, 0.375], 0.35), // the centre of texel (1,1): depth 0.30
        ([0.125, 0.125], 0.05), // texel (0,0): depth 0.05: equal passes (<=)
        ([0.97, 0.03], 0.25),
        ([0.5, 0.5], 0.5),
    ] {
        let got = rig.run_heaps(
            &p,
            [1, 1, 1],
            [32, 1, 1],
            &[uv[0].to_bits(), uv[1].to_bits(), r.to_bits()],
            &[],
            64,
            Some((&hs.0, &hs.1)),
        );
        // point: the texel the coordinate falls in
        let (px, py) = ((uv[0] * 4.0).floor() as i32, (uv[1] * 4.0).floor() as i32);
        let want_point = f32::from(u8::from(r <= depth(px, py)));
        // linear: bilinear weights over the four comparisons
        let (fx, fy) = (uv[0] * 4.0 - 0.5, uv[1] * 4.0 - 0.5);
        let (i0, j0) = (fx.floor() as i32, fy.floor() as i32);
        let (wx, wy) = (fx - i0 as f32, fy - j0 as f32);
        let cmp = |x, y| f32::from(u8::from(r <= depth(x, y)));
        let want_lin = (1.0 - wx) * (1.0 - wy) * cmp(i0, j0)
            + wx * (1.0 - wy) * cmp(i0 + 1, j0)
            + (1.0 - wx) * wy * cmp(i0, j0 + 1)
            + wx * wy * cmp(i0 + 1, j0 + 1);
        eprintln!(
            "cmp uv {uv:?} ref {r}: got point {} linear {}; want point {want_point} linear {want_lin}",
            f32::from_bits(got[0]),
            f32::from_bits(got[1])
        );
        for t in 0..32 {
            let (gp, gl) = (f32::from_bits(got[t * 2]), f32::from_bits(got[t * 2 + 1]));
            assert_eq!(
                gp, want_point,
                "uv {uv:?} ref {r}: point comparison, thread {t}"
            );
            assert!(
                close(gl, want_lin, 0.02),
                "uv {uv:?} ref {r}: linear comparison {gl} want {want_lin}"
            );
        }
    }
}

/// `sampleLevel` for completeness of the family: an explicit LOD (also with
/// a fraction) picks the mip.
#[test]
fn sample_level_picks_the_mip_it_is_given() {
    let rig = rig();
    let Some(p) = rig.program_heaps(
        "tex_level",
        "[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
           Texture2D<float4> tx = ResourceDescriptorHeap[0];\n\
           SamplerState s = SamplerDescriptorHeap[0];\n\
           O.Store(t.x*4, asuint(tx.SampleLevel(s, float2(0.3, 0.6), asfloat(k0)).x)); }\n",
        &["dx.op.sampleLevel.f32"],
    ) else {
        return;
    };
    let t = lod_texture(&rig.d, &rig.q);
    let view = t.view(&ViewSpec::whole(ViewDim::D2)).unwrap();
    let hs = heaps(&rig.d, &[view], &[trilinear()]);
    for lod in [0.0f32, 1.0, 2.0, 3.0, 0.5, 1.75, 2.5, -2.0, 7.0] {
        let got = rig.run_heaps(
            &p,
            [1, 1, 1],
            [32, 1, 1],
            &[lod.to_bits()],
            &[],
            32,
            Some((&hs.0, &hs.1)),
        );
        let want = 1.0 + lod.clamp(0.0, 3.0);
        for (i, w) in got.iter().enumerate() {
            let r = f32::from_bits(*w);
            assert!(
                close(r, want, 0.0625),
                "lod {lod} thread {i}: {r} want {want}"
            );
        }
    }
}
