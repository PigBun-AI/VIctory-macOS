//! What Metal counts against its 32768-byte "total mesh size", pinned
//! (docs/metal/mesh32k.md): measured on an Apple M5 Max, macOS 27.2.1, SDK
//! 27.2, 2026-11-27, with MSL mesh pipelines made by `MTL4Compiler` (no
//! DXC, no converter, so these run everywhere there is a mesh-capable GPU).
//! `mesh_limit_probe.rs` holds the experiments; `back/src/mesh_size.rs` the
//! model these tests hold to the measurements.
#![cfg(target_vendor = "apple")]

mod common_mesh;
use common_mesh::*;
use victory_metal_back::mesh_size::{self, LIMIT, Output};

/// Bytes of an MSL scalar or vector type.
fn bytes(t: &str) -> (u32, u32) {
    // (components, bytes per component)
    match t {
        "float" => (1, 4),
        "float2" => (2, 4),
        "float3" => (3, 4),
        "float4" => (4, 4),
        "half" => (1, 2),
        "half2" => (2, 2),
        "half3" => (3, 2),
        "half4" => (4, 2),
        "uint" => (1, 4),
        other => panic!("no size for {other}"),
    }
}

fn outputs(types: &[&str], position: bool) -> Vec<Output> {
    let mut v = Vec::new();
    if position {
        v.push(out(1, 4, 4));
    }
    v.extend(types.iter().map(|t| {
        let (c, b) = bytes(t);
        out(0, c, b)
    }));
    v
}

fn out(system_value: u32, components: u32, bytes_per_component: u32) -> Output {
    Output {
        name: String::new(),
        system_value,
        components,
        bytes_per_component,
        allocated: true,
    }
}

/// The model's total for a shape.
fn model(s: &Shape) -> u64 {
    mesh_size::total(
        s.max_vertices,
        mesh_size::stride(&outputs(&s.vertex, true), true),
        s.max_primitives,
        mesh_size::stride(&outputs(&s.primitive, false), false),
    )
}

/// The number Metal names when it refuses, or `None` when it builds.
fn metal_total(p: &Probe, s: &Shape) -> Option<u64> {
    match s.build(p) {
        Ok(()) => None,
        Err(e) => Some(
            e.total_mesh_size()
                .unwrap_or_else(|| panic!("refused for another reason: {}", e.text())),
        ),
    }
}

fn probe() -> Option<Probe> {
    let p = Probe::new();
    if p.is_none() {
        eprintln!("SKIPPED: no Metal 4 device with mesh shaders");
    }
    p
}

#[test]
fn the_limit_is_32768_bytes_inclusive() {
    let Some(p) = probe() else { return };
    // 128 vertices of a position and 15 float4: 256 bytes each.
    let at = Shape::new(vec!["float4"; 15], 128, 1);
    assert_eq!(model(&at), LIMIT);
    assert_eq!(metal_total(&p, &at), None, "exactly 32768 builds");
    let over = Shape::new(vec!["float4"; 16], 128, 1);
    assert_eq!(metal_total(&p, &over), Some(128 * 272));
}

#[test]
fn the_declared_maxima_count_not_what_is_written_and_the_vertex_count_rounds_to_four() {
    let Some(p) = probe() else { return };
    for (nv, rounded) in [
        (225u32, 228u32),
        (228, 228),
        (229, 232),
        (232, 232),
        (256, 256),
    ] {
        let s = Shape::new(vec!["float4"; 31], nv, 1);
        let want = u64::from(rounded) * 512;
        assert_eq!(model(&s), want, "{nv} vertices");
        assert_eq!(metal_total(&p, &s), Some(want), "{nv} vertices");
    }
}

/// The member order decides the padding: each member is aligned to its own
/// size (a `float3` to 12), nothing is added at the end.
#[test]
fn members_are_aligned_to_their_own_size() {
    let Some(p) = probe() else { return };
    let cases: [Vec<&'static str>; 7] = [
        vec!["float", "float3", "float", "float3"],
        vec!["float3", "float3", "float", "float"],
        vec!["float", "float", "float3", "float3"],
        vec!["half3", "half", "half3", "half"],
        vec!["half4", "float3", "half", "float2"],
        vec!["float2", "float3", "float2", "float3", "float"],
        vec!["float3"; 9],
    ];
    for vertex in cases {
        // Many vertices so that the total is over the limit and named.
        let s = Shape::new(vertex.clone(), 256, 256);
        let want = model(&s);
        let got = metal_total(&p, &s);
        if want > LIMIT {
            assert_eq!(got, Some(want), "{vertex:?}");
        } else {
            assert_eq!(got, None, "{vertex:?}");
        }
    }
    // The same members in two orders: 4 + 12 + 4 + 12 against 12 + 12 + 4 + 4.
    let alternating = Shape::new(
        ["float", "float3"].into_iter().cycle().take(40).collect(),
        128,
        1,
    );
    let packed = {
        let mut v = vec!["float3"; 20];
        v.extend(vec!["float"; 20]);
        Shape::new(v, 128, 1)
    };
    let (a, b) = (metal_total(&p, &alternating), metal_total(&p, &packed));
    assert_eq!(a, Some(model(&alternating)));
    assert_eq!(b.unwrap_or(model(&packed)), model(&packed));
    assert!(model(&alternating) > model(&packed));
}

#[test]
fn the_primitive_struct_is_counted_per_primitive_and_indices_and_topology_are_not() {
    let Some(p) = probe() else { return };
    for topology in ["triangle", "line", "point"] {
        let mut s = Shape::new(vec!["float4"; 3], 64, 200);
        s.primitive = vec!["float4", "uint"];
        s.topology = topology;
        // 64 x 64 + 200 x 20 = 8096: under, for any topology.
        assert_eq!(model(&s), 64 * 64 + 200 * 20);
        assert_eq!(metal_total(&p, &s), None, "{topology}");
    }
    let mut s = Shape::new(vec!["float4"; 3], 64, 512);
    s.primitive = vec!["float4"; 32];
    // 64 x 64 + 512 x 512 is far over.
    assert_eq!(metal_total(&p, &s), Some(model(&s)));
}

#[test]
fn threadgroup_memory_is_not_part_of_it() {
    let Some(p) = probe() else { return };
    // 32768 bytes of mesh output and 16384 of threadgroup memory.
    let mut s = Shape::new(vec!["float4"; 15], 128, 1);
    s.threadgroup_bytes = 16384;
    s.threads = 128;
    assert_eq!(metal_total(&p, &s), None);
    // Its own limit, named differently.
    s.threadgroup_bytes = 32784;
    let e = s.build(&p).expect_err("over the threadgroup memory limit");
    assert!(e.text().contains("Threadgroup memory size"), "{}", e.text());
    assert_eq!(e.total_mesh_size(), None);
}

/// The refusal is the GPU driver's at pipeline creation: the library of the
/// same source compiles, so neither the front end nor the converter says it.
#[test]
fn it_is_the_driver_at_pipeline_creation_that_refuses() {
    let Some(p) = probe() else { return };
    let s = Shape::new(vec!["float4"; 24], 128, 1);
    match s.build(&p) {
        Err(Refused::Pipeline { text, .. }) => {
            assert!(
                text.contains("exceeds the maximum mesh size allowed (32768)"),
                "{text}"
            );
        }
        other => panic!(
            "a pipeline refusal was expected: {:?}",
            other.err().map(|e| e.text().to_owned())
        ),
    }
}
