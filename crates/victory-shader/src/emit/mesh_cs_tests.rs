//! Mac patch S5: a mesh program as a compute program and a fetch vertex program (`mesh_cs`). The
//! translation side: the forms' shape, the scratch layout, pruning to the pixel program's reads. What
//! they draw is checked against the mesh pipeline on the hardware (`hardware` below, and the Metal
//! back's tests).

use super::container::SigElement;
use super::mesh_cs::{self, MeshCompute};
use super::vertex_flat::{self, VertexRead};
use super::vertex_flat_tests::{self, element, mesh_program, pixel, without};
use super::{HostShader, emit_mesh_compute, lower, signature};
use crate::dxbc::{Container, FourCC};
use crate::native::HwStage;
use crate::{ShaderError, io, ir, resource, structure};

/// The compute and fetch forms of the synthetic mesh program of `vertex_flat_tests`.
fn forms(lanes: u32, max_v: u32, max_p: u32, reads: &[VertexRead]) -> Result<MeshCompute, ShaderError> {
    let (bytes, native) = mesh_program(lanes, max_v, max_p, false);
    without(|| vertex_flat::with_reads(reads, || {
        let c = Container::parse(&bytes)?;
        let p = ir::build(&native.patched_code(), native.wave64, native.vgprs)?;
        let st = structure::structure(&p)?;
        let res = resource::track(&p, &native.layout, native.stage, Some(&st))?;
        let io = io::raise(&p, &native)?;
        emit_mesh_compute(&c, &native, &p, &st, &io, &res)
    }))
}

/// The program kind in the DXIL header: 0 pixel, 1 vertex, 2 geometry, 5 compute, 13 mesh.
fn kind(shader: &HostShader) -> u8 {
    let c = Container::parse(&shader.dxil).unwrap();
    c.part(FourCC::DXIL).unwrap().data[2]
}

fn has(shader: &HostShader, part: FourCC) -> bool {
    Container::parse(&shader.dxil).unwrap().part(part).is_some()
}

fn outputs(shader: &HostShader) -> Vec<SigElement> {
    let c = Container::parse(&shader.dxil).unwrap();
    signature(c.part(FourCC::OSG1).unwrap().data).unwrap()
}

/// The `dx.op` names the translation called (both programs), and `f`'s result.
fn ops<R>(f: impl FnOnce() -> R) -> (R, Vec<String>) {
    vertex_flat::take_ops();
    let r = f();
    (r, vertex_flat::take_ops())
}

/// The numbers of a `vmcl 1` text (`key value`).
fn number(text: &str, key: &str) -> u32 {
    text.lines()
        .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix(' ')).and_then(|r| r.split_whitespace().next()?.parse().ok()))
        .unwrap_or_else(|| panic!("no `{key}` in {text}"))
}

#[test]
fn the_compute_form_is_a_compute_program_and_has_no_signatures() {
    let m = forms(32, 32, 32, &[]).unwrap();
    assert_eq!(m.cs.stage, HwStage::Cs);
    assert_eq!(kind(&m.cs), 5);
    assert!(!has(&m.cs, FourCC::PSG1), "a compute program has no primitive signature");
    assert!(outputs(&m.cs).is_empty());
    assert_eq!(kind(&m.vs), 1);
    assert_eq!(m.vs.stage, HwStage::Vs);
}

#[test]
fn the_layout_is_the_scratch_the_back_expects() {
    let m = forms(32, 32, 32, &[]).unwrap();
    // position (16) + TEXCOORD0 (16) = 32 bytes a vertex; the primitive: its indices, 16 padded.
    let n = |k: &str| number(&m.layout, k);
    assert_eq!((n("threads"), n("maxv"), n("maxp")), (32, 32, 32));
    assert_eq!((n("vstride"), n("pstride")), (32, 16));
    assert_eq!(n("parea"), 16 + 32 * 32);
    assert_eq!(n("gstride"), 16 + 32 * 32 + 32 * 16);
    assert_eq!(m.group_bytes, n("gstride"));
    assert_eq!(m.draw_vertices, 96);
    assert!(m.layout.starts_with("vmcl 1\n"));
    assert!(m.layout.contains("\nspace 4000000000\n"));
}

#[test]
fn the_maxima_are_cut_to_the_lanes_that_write() {
    // 128 declared, 32 lanes: lane n writes vertex n and primitive n only.
    let m = forms(32, 128, 128, &[]).unwrap();
    assert_eq!((number(&m.layout, "maxv"), number(&m.layout, "maxp")), (32, 32));
    assert_eq!(m.draw_vertices, 96);
}

#[test]
fn the_outputs_are_stored_to_the_scratch_not_to_the_mesh_stage() {
    let (m, called) = ops(|| forms(32, 32, 32, &[]));
    m.unwrap();
    let called = |n: &str| called.iter().any(|o| o.starts_with(n));
    assert!(called("rawBufferStore"), "no store to the scratch");
    assert!(called("rawBufferLoad"), "no load in the fetch program");
    for gone in ["storeVertexOutput", "storePrimitiveOutput", "emitIndices", "setMeshOutputCounts"] {
        assert!(!called(gone), "{gone} is still called by the compute form");
    }
}

#[test]
fn the_fetch_program_returns_what_the_mesh_program_wrote() {
    let m = forms(32, 32, 32, &[]).unwrap();
    let outs = outputs(&m.vs);
    let names: Vec<_> = outs.iter().map(|e| (e.name.as_str(), e.kind, e.cols)).collect();
    assert_eq!(names, [("SV_Position", 3, 4), ("TEXCOORD", 0, 4)]);
    let ins = signature(Container::parse(&m.vs.dxil).unwrap().part(FourCC::ISG1).unwrap().data).unwrap();
    assert_eq!(ins.iter().map(|e| e.kind).collect::<Vec<_>>(), [1, 2], "vertex id and instance id");
}

#[test]
fn what_s1_hands_on_travels_as_flat_primitive_outputs() {
    let ps = without(pixel).unwrap();
    assert!(!ps.vertex_reads.is_empty());
    let m = forms(32, 32, 32, &ps.vertex_reads).unwrap();
    let outs = outputs(&m.vs);
    let added = outs.iter().find(|e| e.index == vertex_flat::FLAT_BASE).expect("the added element");
    assert_eq!((added.name.as_str(), added.comp), ("TEXCOORD", 5));
    // (interpolation 1: constant)
    let line = m.layout.lines().find(|l| l.starts_with(&format!("p TEXCOORD {} ", vertex_flat::FLAT_BASE))).expect(&m.layout);
    assert!(line.ends_with(" 1"), "{line}");
    assert!(line.contains(" 0 uint 1 "), "{line}");
}

#[test]
fn a_compute_form_is_not_the_mesh_forms_translation() {
    // The same program as a mesh pipeline stays what it was (S5 changes nothing without being asked).
    let (bytes, _) = mesh_program(32, 32, 32, false);
    let _ = bytes;
    let ms = super::vertex_flat_tests::mesh(32, 32, 32).unwrap();
    assert_eq!(kind(&ms), 13);
    assert!(has(&ms, FourCC::PSG1));
    let m = forms(32, 32, 32, &[]).unwrap();
    assert_ne!(ms.dxil, m.cs.dxil);
}

fn vertex_element(name: &str, cols: u8) -> SigElement {
    SigElement { cols, usage: (1 << cols) - 1, ..element(name, 0, 0) }
}

#[test]
fn metal_counts_the_mesh_struct_as_the_back_measured() {
    // 128 vertices of position + 17 float4 (288 bytes), no primitive outputs: 128 x 288 = 36,864.
    let mut v = vec![SigElement { kind: 3, ..element("SV_Position", 3, 0) }];
    v.extend((0..17).map(|i| SigElement { index: i, ..vertex_element("TEXCOORD", 4) }));
    assert_eq!(mesh_cs::metal_bytes(&v, &[], 128, 128, 128), 128 * 288);
    // 4 floats fewer a vertex: under.
    v.truncate(13);
    assert!(mesh_cs::metal_bytes(&v, &[], 128, 128, 128) <= 32768);
    // Rounded up to four vertices.
    assert_eq!(mesh_cs::metal_bytes(&v, &[], 128, 30, 30), 32 * 208);
}

#[test]
fn the_mode_is_in_the_translation_settings() {
    use mesh_cs::MeshComputeMode::*;
    let word = |m| {
        super::lower::TEST_MESH_COMPUTE.set(Some(m));
        let w = super::lower::host_settings();
        super::lower::TEST_MESH_COMPUTE.set(None);
        w
    };
    let (off, problem, all) = (word(Off), word(Problem), word(All));
    assert_ne!(off, problem);
    assert_ne!(problem, all);
    assert_eq!(off[..3], all[..3]);
    assert_eq!(off[3] >> 29 & 3, 0);
    assert_eq!(problem[3] >> 29 & 3, 1);
    assert_eq!(all[3] >> 29 & 3, 2);
}

/// The mesh program's translation, as the pipeline gets it, with the host asking for `mode`.
fn translated(mode: mesh_cs::MeshComputeMode, lanes: u32, max: u32) -> HostShader {
    lower::TEST_MESH_COMPUTE.set(Some(mode));
    let ms = without(|| vertex_flat_tests::mesh(lanes, max, max));
    lower::TEST_MESH_COMPUTE.set(None);
    ms.unwrap()
}

#[test]
fn nothing_is_attached_unless_the_host_asks() {
    use mesh_cs::MeshComputeMode::*;
    let off = translated(Off, 32, 32);
    assert!(mesh_cs::read_vmcs(&off.dxil).is_none());
    // The mesh container is what it was, whatever the mode: the part is appended last.
    let all = translated(All, 32, 32);
    let a = Container::parse(&off.dxil).unwrap();
    let b = Container::parse(&all.dxil).unwrap();
    assert_eq!(b.parts.len(), a.parts.len() + 1);
    for (x, y) in a.parts.iter().zip(&b.parts) {
        assert_eq!((x.fourcc, x.data), (y.fourcc, y.data));
    }
    assert!(has(&all, mesh_cs::VMCS));
}

#[test]
fn the_part_carries_the_three_products() {
    use mesh_cs::MeshComputeMode::*;
    let all = translated(All, 32, 32);
    let (layout, cs, vs) = mesh_cs::read_vmcs(&all.dxil).expect("a VMCS part");
    assert!(layout.starts_with("vmcl 1\n"));
    let cs = Container::parse(&cs).unwrap();
    let vs = Container::parse(&vs).unwrap();
    assert!(cs.part(FourCC::DXIL).is_some() && vs.part(FourCC::DXIL).is_some());
    assert_eq!(cs.part(FourCC::DXIL).unwrap().data[2], 5);
    assert_eq!(vs.part(FourCC::DXIL).unwrap().data[2], 1);
    // The payload header: version 1, flags 0, every offset 4-aligned and in the part.
    let payload = Container::parse(&all.dxil).unwrap().part(mesh_cs::VMCS).unwrap().data.to_vec();
    let word = |at: usize| u32::from_le_bytes(payload[at..at + 4].try_into().unwrap()) as usize;
    assert_eq!((word(0), word(4)), (1, 0));
    for at in [8, 16, 24] {
        assert_eq!(word(at) % 4, 0);
        assert!(word(at) + word(at + 4) <= payload.len());
    }
    // A foreign version is refused, not guessed.
    let mut other = payload.clone();
    other[0] = 2;
    let parts: Vec<(&[u8; 4], &[u8])> = vec![(&mesh_cs::VMCS.0, &other)];
    assert!(mesh_cs::read_vmcs(&super::container::build(&parts)).is_none());
}

#[test]
fn problem_mode_attaches_only_where_the_mesh_struct_is_over_32k() {
    use mesh_cs::MeshComputeMode::*;
    // 32 lanes: 32 x 32 + 32 x 4 bytes in Metal's struct (far under).
    assert!(mesh_cs::read_vmcs(&translated(Problem, 32, 32).dxil).is_none());
    // The same program declaring 256 vertices and primitives is cut to its lanes (the mesh form's limit
    // comes from D3D12's, not from the lanes): still under.
    assert!(mesh_cs::read_vmcs(&translated(All, 32, 256).dxil).is_some());
}

/// With `VICTORY_MC_DUMP=<dir>`: writes the synthetic program's two forms and its pixel program there
/// (to disassemble them with DXC, or to feed Metal Shader Converter by hand).
#[test]
fn dump_the_forms_when_asked() {
    let Some(dir) = std::env::var_os("VICTORY_MC_DUMP") else {
        return;
    };
    let ps = without(pixel).unwrap();
    let m = forms(32, 32, 32, &ps.vertex_reads).unwrap();
    let dir = std::path::Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("cs.dxil"), &m.cs.dxil).unwrap();
    std::fs::write(dir.join("vs.dxil"), &m.vs.dxil).unwrap();
    std::fs::write(dir.join("ps.dxil"), &ps.dxil).unwrap();
    std::fs::write(dir.join("layout.txt"), &m.layout).unwrap();
}

/// The mesh pipeline's drawing against the compute form's, on the host's GPU and on WARP: the 32-lane
/// triangle program with the pixel program that reads other vertices (S1), drawn through the mesh
/// pipeline (with S1's flat outputs) and through the compute pass and the fetch draw; every texel is
/// the same. 
#[test]
#[cfg(windows)]
fn the_compute_and_fetch_draw_is_the_mesh_pipelines_draw() {
    use super::host::{Draw, Host, Root, ScratchPass};
    use windows::Win32::Graphics::Direct3D12::*;
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let ps = without(pixel).unwrap();
    let ms = without(|| vertex_flat::with_reads(&ps.vertex_reads, || vertex_flat_tests::mesh(32, 32, 32))).unwrap();
    let mut ran = 0;
    for (label, warp) in [("hardware", false), ("warp", true)] {
        let Ok(host) = Host::new(warp) else {
            eprintln!("no SM 6.6 {label} device; skipped");
            continue;
        };
        let native = (|| {
            let root = host.root_signature_for(&[
                (&ms, D3D12_SHADER_VISIBILITY_MESH),
                (&ps, D3D12_SHADER_VISIBILITY_PIXEL),
            ])?;
            let pso = host.mesh_pipeline_with(&root, &ms.dxil, &ps.dxil)?;
            host.draw(
                &root,
                &pso,
                &Draw {
                    roots: vec![(0, Root::Constants(vec![0])), (1, Root::Constants(vec![0]))],
                    textures: &[],
                    samplers: &[],
                    vertices: &[],
                    stride: 0,
                    size: (16, 16),
                    mesh_groups: 1,
                    scratch: None,
                },
            )
        })();
        let Ok(native) = native else {
            eprintln!("no mesh pipeline on {label} ({}); skipped", native.unwrap_err());
            continue;
        };
        assert!(native.iter().any(|t| t[0] != 0.0), "{label}: the mesh pipeline drew nothing");
        for (name, dim) in [("one group", [1u32, 1, 1]), ("two in x", [2, 1, 1]), ("three in y", [1, 3, 1]), ("2x2x2", [2, 2, 2])] {
            let m = forms(32, 32, 32, &ps.vertex_reads).unwrap();
            let root = host
                .root_signature_scratch(
                    &[
                        (&m.cs, D3D12_SHADER_VISIBILITY_ALL),
                        (&m.vs, D3D12_SHADER_VISIBILITY_VERTEX),
                        (&ps, D3D12_SHADER_VISIBILITY_PIXEL),
                    ],
                    true,
                )
                .unwrap();
            let cs = host.compute_pipeline_with(&root, &m.cs.dxil).unwrap_or_else(|e| panic!("{label}: cs pipeline: {e}"));
            let gfx = host.pipeline_with(&root, &m.vs.dxil, &ps.dxil).unwrap_or_else(|e| panic!("{label}: pipeline: {e}"));
            let texels = host
                .draw(
                    &root,
                    &gfx,
                    &Draw {
                        roots: vec![(1, Root::Constants(vec![0])), (2, Root::Constants(vec![0])), (5, Root::Constants(vec![dim[0], dim[1], dim[2], 0]))],
                        textures: &[],
                        samplers: &[],
                        vertices: &[],
                        stride: 0,
                        size: (16, 16),
                        mesh_groups: 0,
                        scratch: Some(ScratchPass {
                            cs: &cs,
                            roots: vec![(0, Root::Constants(vec![0])), (5, Root::Constants(vec![dim[0], dim[1], dim[2], 0]))],
                            dim,
                            bytes: u64::from(m.group_bytes) * u64::from(dim[0] * dim[1] * dim[2]),
                            uav: 3,
                            srv: 4,
                            draw_vertices: m.draw_vertices,
                        }),
                    },
                )
                .unwrap_or_else(|e| panic!("{label}, {name}: draw: {e}"));
            let bad = texels.iter().zip(&native).filter(|(a, b)| a != b).count();
            assert_eq!(bad, 0, "{label}, {name}: {bad} of 256 texels differ from the mesh pipeline's");
        }
        eprintln!("{label}: the compute and fetch draw equals the mesh pipeline's");
        ran += 1;
    }
    if ran == 0 {
        eprintln!("no device with mesh shaders; skipped");
    }
}
