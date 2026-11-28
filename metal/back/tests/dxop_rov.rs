//! SFI0 bit 12 `ROVs` (rasterizer-ordered views; 10 pixel programs): D3D12
//! guarantees that pixel shader invocations that touch the same pixel access a
//! `RasterizerOrdered*` resource in primitive order, and that one invocation's
//! accesses finish before the next one starts (D3D11.3 Functional Spec 18.x
//! "Rasterizer Ordered Views"). MSC 4.0.3 "accepts" ROVs (docs/metal/msc.md:
//! accepted-but-unsupported: "MSC ignores or approximates them").
//!
//! The probe: `N` identical full-screen triangles drawn in one draw, the
//! pixel shader makes a non-atomic read-modify-write of a texel of an
//! R32_UINT texture, `r = r * 1664525 + prim + 1` (order dependent), and the
//! result is compared with the CPU applying the updates in primitive order.
#![cfg(target_vendor = "apple")]

mod common_dxop;
mod common_msc;

use common_dxop::*;
use victory_metal_back::format::dxgi;
use victory_metal_back::texture::{TextureDesc, ViewDim, ViewSpec};

const N: u32 = 64;

/// `N` full-screen triangles in one draw: corner `id % 3` of triangle `id / 3`.
const VS_MANY: &str = "float4 main(uint id : SV_VertexID) : SV_Position {\n\
  float2 p[3] = { float2(-1, -1), float2(3, -1), float2(-1, 3) };\n\
  return float4(p[id % 3], 0.5, 1); }\n";

fn ps(resource: &str) -> String {
    format!(
        "void main(float4 pos : SV_Position, uint prim : SV_PrimitiveID) {{\n\
           uint2 xy = uint2(pos.xy);\n\
           {resource}<uint> r = ResourceDescriptorHeap[0];\n\
           uint old = r[xy];\n\
           r[xy] = old * 1664525u + prim + 1u; }}\n"
    )
}

fn run(rig: &Rig, name: &str, resource: &str) -> Option<Vec<u32>> {
    let p = rig.pixel_program_vs(name, VS_MANY, &ps(resource), &[], true)?;
    p.vertices.set(3 * N);
    let t = rig.d.create_texture(&TextureDesc::tex2d(dxgi::R32_UINT, 8, 8, SRV | UAV)).unwrap();
    upload(&rig.d, &rig.q, &t, 0, &vec![0u8; 8 * 8 * 4], 32);
    let view = t.view(&ViewSpec::whole(ViewDim::D2)).unwrap();
    let hs = heaps(&rig.d, &[view], &[]);
    rig.run_pixels(&p, [8, 8], &[], &[], 1, Some((&hs.0, &hs.1)));
    let out = download(&rig.d, &rig.q, &t, 32);
    Some(out.chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect())
}

fn expected() -> u32 {
    (0..N).fold(0u32, |r, prim| r.wrapping_mul(1_664_525).wrapping_add(prim + 1))
}

#[test]
fn a_rasterizer_ordered_view_sees_the_primitives_in_order() {
    let rig = rig();
    let want = expected();
    let mut bad: Vec<(usize, u32)> = Vec::new();
    // ten runs: an ordering that holds by luck of the scheduler would break at some
    for rep in 0..10 {
        let Some(got) = run(&rig, "rov_ordered", "RasterizerOrderedTexture2D") else { return };
        bad.extend(got.iter().enumerate().filter(|&(_, &g)| g != want).map(|(i, &g)| (i + 64 * rep, g)));
    }
    // The plain UAV version of the same shader shows what ordering does.
    let plain = run(&rig, "rov_plain", "RWTexture2D").unwrap();
    let plain_bad = plain.iter().filter(|&&g| g != want).count();
    eprintln!("ROV: {} of 640 pixel results differ from the in-order result {want:#x}; plain UAV: {plain_bad} of 64", bad.len());
    assert!(bad.is_empty(), "ROV pixels out of order: {} of 640, first {:?}", bad.len(), &bad[..bad.len().min(4)]);
}
