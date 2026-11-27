//! The experiments behind `docs/metal/mesh32k.md`: what Metal's 32768-byte
//! "total mesh size" counts. Ignored (they print a table; run with
//! `--ignored --nocapture`). The pinned facts are in `mesh_limit.rs`.
#![cfg(target_vendor = "apple")]

mod common_mesh;
use common_mesh::*;
use objc2_metal::MTLDevice;

/// The largest `n` in lo..=hi for which `f(n)` builds, and the refusal at n+1.
fn max_ok(p: &Probe, lo: u32, hi: u32, f: impl Fn(u32) -> Shape) -> (u32, Option<Refused>) {
    let (mut a, mut b) = (lo, hi); // a builds (assumed), find the last that builds
    if f(hi).build(p).is_ok() {
        return (hi, None);
    }
    while b - a > 1 {
        let m = (a + b) / 2;
        if f(m).build(p).is_ok() { a = m } else { b = m }
    }
    (a, f(b).build(p).err())
}

#[test]
#[ignore]
fn probe_vertex_only() {
    let p = Probe::new().unwrap();
    println!("device {}", p.device.name());
    for k in [0usize, 1, 3, 7, 8, 11, 15] {
        let vertex = vec!["float4"; k];
        let (ok, fail) = max_ok(&p, 1, 256, |n| Shape::new(vertex.clone(), n, 1));
        let per = 16 + 16 * k as u32;
        println!(
            "pos + {k} float4 ({per} B/vertex): max vertices {ok} = {} B; first refusal {:?}",
            ok * per,
            fail.map(|e| (
                e.total_mesh_size(),
                e.text().lines().next().unwrap_or("").to_string()
            ))
        );
    }
}

fn reported(p: &Probe, s: &Shape) -> String {
    match s.build(p) {
        Ok(()) => "OK (under the limit)".into(),
        Err(e) => match e.total_mesh_size() {
            Some(n) => format!("{n}"),
            None => format!("OTHER: {}", e.text().lines().next().unwrap_or("")),
        },
    }
}

#[test]
#[ignore]
fn probe_what_is_counted() {
    let p = Probe::new().unwrap();
    // Every shape is over the limit on purpose: Metal then names its total.
    let big = vec!["float4"; 8]; // pos + 8 float4 = 144 B / vertex
    println!("-- vertex count rounding (pos + 8 float4 = 144 B)");
    for nv in [225u32, 226, 227, 228, 229, 232, 240, 255, 256] {
        let s = Shape::new(big.clone(), nv, 1);
        println!("NV={nv} NP=1: {}  (NV*144={})", reported(&p, &s), nv * 144);
    }
    println!("-- primitive maximum (pos + 8 float4, NV=256)");
    for np in [1u32, 2, 4, 8, 64, 128, 256, 512] {
        let s = Shape::new(big.clone(), 256, np);
        println!("NP={np}: {}  (256*144={})", reported(&p, &s), 256 * 144);
    }
    println!("-- topology (pos + 8 float4, NV=256, NP=64)");
    for t in ["triangle", "line", "point"] {
        let mut s = Shape::new(big.clone(), 256, 64);
        s.topology = t;
        println!("{t}: {}", reported(&p, &s));
    }
    println!("-- primitive attributes (pos + 8 float4, NV=256)");
    for (np, prim) in [
        (1u32, vec!["float4"]),
        (64, vec!["float4"]),
        (64, vec!["float"]),
        (64, vec!["float", "float4"]),
        (128, vec!["float4", "float4"]),
        (512, vec!["uint"]),
    ] {
        let mut s = Shape::new(big.clone(), 256, np);
        s.primitive = prim.clone();
        println!("NP={np} prim={prim:?}: {}", reported(&p, &s));
    }
    println!("-- vertex attribute packing (NV=256)");
    for v in [
        vec!["float4", "float", "float4"],
        vec!["float", "float4", "float4"],
        vec!["float", "float", "float", "float4"],
        vec!["float2", "float4", "float2"],
        vec!["half4", "half4", "float4"],
        vec![
            "half2", "half2", "half2", "half2", "float4", "float4", "float4",
        ],
        vec![
            "float3", "float3", "float3", "float3", "float3", "float3", "float3", "float3",
            "float3",
        ],
        vec![
            "uint", "uint", "uint", "uint", "uint", "uint", "uint", "uint",
        ],
    ] {
        let s = Shape::new(v.clone(), 256, 1);
        println!("{v:?}: {}", reported(&p, &s));
    }
    println!(
        "-- extra threadgroup memory (pos + 7 float4 = 128 B, NV=256: at the limit by itself)"
    );
    for tg in [0u32, 16, 256, 1024, 4096, 16384] {
        let mut s = Shape::new(vec!["float4"; 7], 256, 1);
        s.threadgroup_bytes = tg;
        println!("tg={tg}: {}", reported(&p, &s));
    }
    println!("-- threadgroup memory only, small mesh (pos + float4, NV=16)");
    for tg in [0u32, 16384, 32768, 32784, 40000] {
        let mut s = Shape::new(vec!["float4"], 16, 1);
        s.threadgroup_bytes = tg;
        println!("tg={tg}: {}", reported(&p, &s));
    }
}

/// Bytes per vertex Metal counts for `vertex` (after the position), read as
/// the refusal's total at 256 vertices minus a base that is over the limit.
fn per_vertex(p: &Probe, vertex: &[&'static str]) -> String {
    let mut v = vec!["float4"; 8];
    v.extend_from_slice(vertex);
    let s = Shape::new(v, 256, 1);
    match s.build(p).err().and_then(|e| e.total_mesh_size()) {
        Some(n) => format!("{} B/vertex (total {n})", n as f64 / 256.0 - 144.0),
        None => "?".into(),
    }
}

#[test]
#[ignore]
fn probe_vertex_layout() {
    let p = Probe::new().unwrap();
    println!("-- bytes added per vertex after pos + 8 float4 (144 B)");
    for v in [
        vec!["float"],
        vec!["float2"],
        vec!["float3"],
        vec!["float4"],
        vec!["half"],
        vec!["half2"],
        vec!["half3"],
        vec!["half4"],
        vec!["uint"],
        vec!["uchar"],
        vec!["ushort"],
        vec!["short2"],
        vec!["float3", "float3"],
        vec!["float3", "float3", "float3"],
        vec!["float3", "float"],
        vec!["float", "float3"],
        vec!["float", "float4"],
        vec!["float2", "float4"],
        vec!["float3", "float4"],
        vec!["half", "float4"],
        vec!["half", "float"],
        vec!["half", "float2"],
        vec!["half", "half", "half", "half"],
        vec!["float", "float", "float"],
        vec!["float", "float", "float", "float"],
        vec!["float", "float", "float3"],
        vec!["float2", "float3"],
        vec!["half", "float3"],
        vec!["float", "float", "float", "float3"],
        vec!["float3", "float2"],
        vec!["float3", "half"],
        vec!["float3", "float3", "float"],
        vec!["float", "float3", "float3"],
        vec!["float", "float2"],
        vec!["float", "float", "float2"],
        vec!["float2", "float"],
        vec!["float2", "float2"],
        vec!["half3", "half3"],
        vec!["float", "half3"],
        vec!["float", "uchar", "float2"],
    ] {
        println!("{v:?}: {}", per_vertex(&p, &v));
    }
    println!("-- first attribute small, then 8 float4");
    for first in ["float", "float2", "float3", "half", "uchar"] {
        let mut v = vec![first];
        v.extend(vec!["float4"; 8]);
        let s = Shape::new(v, 256, 1);
        let n = s.build(&p).err().and_then(|e| e.total_mesh_size());
        println!(
            "[{first}, float4 x8]: total {n:?} = {} B/vertex",
            n.unwrap_or(0) as f64 / 256.0
        );
    }
}

#[test]
#[ignore]
fn probe_np_rounding_and_payload() {
    let p = Probe::new().unwrap();
    println!("-- NP rounding, pos + 8 float4 + prim float, NV=256");
    for np in [1u32, 2, 3, 5, 7, 9, 100, 101] {
        let mut s = Shape::new(vec!["float4"; 8], 256, np);
        s.primitive = vec!["float"];
        let n = s.build(&p).err().and_then(|e| e.total_mesh_size()).unwrap();
        println!(
            "NP={np}: total {n} = 36864 + {} (NP*4={})",
            n - 36864,
            np * 4
        );
    }
    println!("-- payload with an object stage (mesh pos + 7 float4, NV=256 = exactly 32768)");
    for payload in [16u32, 1024, 4096, 8192, 16000, 16368] {
        let ms = Shape::new(vec!["float4"; 7], 256, 1).msl().replace(
            "void ms(M m,",
            "void ms(M m, const object_data Pl& pl [[payload]],",
        );
        let _src = format!(
            "{}\nstruct Pl {{ float v[{}]; }};\n[[object, max_total_threads_per_threadgroup(32)]] void os(object_data Pl& pl [[payload]], mesh_grid_properties g, uint tid [[thread_index_in_threadgroup]]) {{ for (uint i = tid; i < {}; i += 32) pl.v[i] = float(i); g.set_threadgroups_per_grid(uint3(1,1,1)); }}\n",
            "#include <metal_stdlib>\n#include <metal_mesh>\nusing namespace metal;\nstruct Pl { float v[1]; };",
            payload / 4,
            payload / 4
        );
        // The payload type must be declared before the mesh function: rebuild.
        let body = ms.replace(
            "#include <metal_stdlib>\n#include <metal_mesh>\nusing namespace metal;\n",
            "",
        );
        let src = format!(
            "#include <metal_stdlib>\n#include <metal_mesh>\nusing namespace metal;\nstruct Pl {{ float v[{}]; }};\n{}\n[[object, max_total_threads_per_threadgroup(32)]] void os(object_data Pl& pl [[payload]], mesh_grid_properties g, uint tid [[thread_index_in_threadgroup]]) {{ for (uint i = tid; i < {}; i += 32) pl.v[i] = float(i); g.set_threadgroups_per_grid(uint3(1,1,1)); }}\n",
            payload / 4,
            body,
            payload / 4
        );
        let _ = src.len();
        let r = p.mesh_pipeline(&src, "ms", Some(("os", 32, payload as usize)), 32);
        println!(
            "payload={payload}: {:?}",
            r.map_err(|e| (
                e.total_mesh_size(),
                e.text().lines().next().unwrap_or("").to_string()
            ))
        );
    }
}

#[test]
#[ignore]
fn probe_fragment_pruning() {
    let p = Probe::new().unwrap();
    // pos + 8 float4 = 144 B x 256 = 36864 B, over the limit by itself.
    let s = Shape::new(vec!["float4"; 8], 256, 1);
    let ms = s.msl();
    let cases: [(&str, &str); 3] = [
        (
            "reads only a0",
            "struct F { float4 pos [[position]]; float4 a0; };\nfragment float4 fs(F in [[stage_in]]) { return in.a0; }\n",
        ),
        (
            "reads nothing but position",
            "struct F { float4 pos [[position]]; };\nfragment float4 fs(F in [[stage_in]]) { return float4(1); }\n",
        ),
        (
            "reads a0..a7",
            "struct F { float4 pos [[position]]; float4 a0; float4 a1; float4 a2; float4 a3; float4 a4; float4 a5; float4 a6; float4 a7; };\nfragment float4 fs(F in [[stage_in]]) { return in.a0+in.a1+in.a2+in.a3+in.a4+in.a5+in.a6+in.a7; }\n",
        ),
    ];
    for (label, fs) in cases {
        let src = format!("{ms}\n{fs}");
        let r = p.mesh_pipeline_with_fragment(&src, "ms", None, 32, Some("fs"));
        println!(
            "fragment {label}: {:?}",
            r.map_err(|e| (
                e.total_mesh_size(),
                e.text().lines().next().unwrap_or("").to_string()
            ))
        );
    }
}
