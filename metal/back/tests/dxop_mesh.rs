//! Mesh-stage dx.op nodes of the DXIL -> MSC hop (8,765 programs each,
//! map-plan/dxop-usage.tsv): `setMeshOutputCounts`, `emitIndices`,
//! `storeVertexOutput.f32` and `.i32` (and the `groupId`, `flattenedThreadId`
//! of `dxop_alu.rs`, which every mesh program also uses).
//!
//! Two triangles over an 8x8 target, drawn by a mesh shader: vertices carry
//! a float attribute (an affine function of the position, so interpolation
//! is exact apart from rounding), a flat integer attribute, and the pixel
//! shader writes what it receives to the root UAV, one record per pixel. The
//! expectation is a CPU rasterisation: pixel (i, j) (column, row from the top)
//! is in triangle A when i <= j, else in triangle B (the hypotenuse
//! x + y = 0.1 has no pixel centre on it); a flat attribute comes from the
//! provoking vertex, the first index of the triangle (D3D12 spec, "Flat
//! interpolation": the provoking vertex is the first of the primitive).
#![cfg(target_vendor = "apple")]

mod common_dxop;
mod common_msc;

use common_dxop::*;

const N: usize = 8;

const MS: &str = "struct V { float4 pos : SV_Position; float f : TEXCOORD0; nointerpolation int id : TEXCOORD1; nointerpolation uint bits : TEXCOORD2; };\n\
[numthreads(4,1,1)] [outputtopology(\"triangle\")]\n\
void main(uint tid : SV_GroupIndex, out vertices V v[4], out indices uint3 t[2]) {\n\
  uint tris = k0;\n\
  SetMeshOutputCounts(4, tris);\n\
  float2 p[4] = { float2(-1, -1), float2(1.1, -1), float2(-1, 1.1), float2(1.1, 1.1) };\n\
  v[tid].pos = float4(p[tid], 0.5, 1);\n\
  v[tid].f = 3.0 * p[tid].x - 2.0 * p[tid].y + 1.0;\n\
  v[tid].id = -(int)(10 + tid);\n\
  v[tid].bits = asuint(float(tid)) ^ 0x80000000u ^ k1;\n\
  if (tid == 0) { t[0] = uint3(0, 1, 2); }\n\
  if (tid == 1 && tris > 1) { t[1] = uint3(3, 2, 1); } }\n";

const PS: &str = "struct V { float4 pos : SV_Position; float f : TEXCOORD0; nointerpolation int id : TEXCOORD1; nointerpolation uint bits : TEXCOORD2; };\n\
void main(V v) {\n\
  uint x = (uint)v.pos.x, y = (uint)v.pos.y; uint i = y*8+x;\n\
  O.Store4(i*16, uint4(asuint(v.f), (uint)v.id, v.bits, 0)); }\n";

/// (Interpolation is checked to 2e-3: the triangle reaches 0.1 NDC past the target and the GPU's barycentrics are fixed point; measured error up to 5e-4.)
/// `setMeshOutputCounts` (4 vertices, 1 or 2 triangles), `emitIndices`, and
/// `storeVertexOutput` for a float and for two integer attributes, with the
/// primitive count chosen at run time.
#[test]
fn a_mesh_shader_emits_vertices_indices_and_attributes() {
    let rig = rig();
    let Some(p) = rig.mesh_program(
        "mesh_attrs",
        MS,
        PS,
        &[
            "dx.op.setMeshOutputCounts",
            "dx.op.emitIndices",
            "dx.op.storeVertexOutput.f32",
            "dx.op.storeVertexOutput.i32",
        ],
        &[],
    ) else {
        return;
    };
    for (tris, k1) in [(2u32, 0u32), (1, 0), (2, 0x0000_ffff)] {
        let got = rig.run_mesh(&p, 1, [8, 8], &[tris, k1], &[], N * N * 4);
        let mut bad = Vec::new();
        for j in 0..N {
            for i in 0..N {
                let r = &got[(j * N + i) * 4..(j * N + i) * 4 + 4];
                let in_a = i <= j;
                if !in_a && tris == 1 {
                    if r.iter().any(|&w| w != 0xDEAD_BEEF) {
                        bad.push(format!("({i},{j}) written with only triangle A: {r:x?}"));
                    }
                    continue;
                }
                let (nx, ny) = ((i as f32 + 0.5) / 4.0 - 1.0, 1.0 - (j as f32 + 0.5) / 4.0);
                let f = 3.0 * nx - 2.0 * ny + 1.0;
                let first = if in_a { 0u32 } else { 3 };
                let id = (-(10 + first as i32)) as u32;
                let bits = (first as f32).to_bits() ^ 0x8000_0000 ^ k1;
                if (f32::from_bits(r[0]) - f).abs() > 2e-3 || r[1] != id || r[2] != bits {
                    bad.push(format!("({i},{j}) got f={} id={:#x} bits={:#x}; want f={f} id={id:#x} bits={bits:#x}", f32::from_bits(r[0]), r[1], r[2]));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "tris={tris} k1={k1:#x}: {} pixels differ, first: {:?}",
            bad.len(),
            &bad[..bad.len().min(4)]
        );
    }
}

/// Several groups, each its own thread and group ids: group g draws the
/// triangle pair with the vertex `id` offset by 100 g into pixels of the
/// column band `g` (so groups are told apart by what they wrote).
#[test]
fn mesh_groups_have_their_own_ids_and_counts() {
    let rig = rig();
    let Some(p) = rig.mesh_program(
        "mesh_groups",
        "struct V { float4 pos : SV_Position; nointerpolation uint g : TEXCOORD0; };\n\
         [numthreads(3,1,1)] [outputtopology(\"triangle\")]\n\
         void main(uint tid : SV_GroupIndex, uint3 gid : SV_GroupID, out vertices V v[3], out indices uint3 t[1]) {\n\
           SetMeshOutputCounts(3, 1);\n\
           float x0 = -1.0 + 0.5 * gid.x;\n\
           float2 p[3] = { float2(x0, -1), float2(x0 + 0.5, -1), float2(x0, 1) };\n\
           v[tid].pos = float4(p[tid], 0.5, 1);\n\
           v[tid].g = gid.x * 100 + tid;\n\
           if (tid == 0) t[0] = uint3(0, 1, 2); }\n",
        "struct V { float4 pos : SV_Position; nointerpolation uint g : TEXCOORD0; };\n\
         void main(V v) { uint x = (uint)v.pos.x, y = (uint)v.pos.y; O.Store((y*8+x)*4, v.g); }\n",
        &["dx.op.groupId.i32", "dx.op.setMeshOutputCounts", "dx.op.emitIndices"],
        &[],
    ) else {
        return;
    };
    let got = rig.run_mesh(&p, 4, [8, 8], &[], &[], N * N);
    // group g covers the right-angled triangle with corners at x0, x0+0.5 (bottom) and x0 (top): 2 pixels wide
    let mut wrote = 0;
    for j in 0..N {
        for i in 0..N {
            let w = got[j * N + i];
            if w == 0xDEAD_BEEF {
                continue;
            }
            wrote += 1;
            let g = w / 100;
            assert_eq!(
                w % 100,
                0,
                "pixel ({i},{j}): flat value is the provoking vertex's (tid 0): {w}"
            );
            assert_eq!(
                i / 2,
                g as usize,
                "pixel ({i},{j}) belongs to the band of group {g}"
            );
        }
    }
    assert!(wrote >= 4, "every group drew pixels: {wrote}");
    for g in 0..4 {
        assert!(
            (0..N).any(|j| got[j * N + 2 * g] == g as u32 * 100),
            "group {g} drew"
        );
    }
}
