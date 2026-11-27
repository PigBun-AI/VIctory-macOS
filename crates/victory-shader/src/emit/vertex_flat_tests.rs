//! Mac patch S1: a host without `attributeAtVertex` gets the three vertices of an attribute as flat
//! inputs (`vertex_flat`). The pixel half, the mesh half (the exchange), and their agreement.

use super::container::{self, SigElement};
use super::vertex_flat::{self, VertexRead};
use super::{HostShader, emit, lower, signature};
use crate::dxbc::{Container, FourCC};
use crate::native::{HwStage, MeshIo, NativeProgram, PsInput, StageIo, Usage, VsOutput};
use crate::{ShaderError, io, ir, resource, structure};

const S_ENDPGM: u32 = 0xBF81_0000;

fn element(name: &str, kind: u8, row: u32) -> SigElement {
    SigElement {
        name: name.into(),
        index: 0,
        kind,
        comp: 9,
        interp: 0,
        row: Some(row),
        col: 0,
        cols: 4,
        usage: 0,
        half: false,
    }
}

/// Runs `f` as a host that has no `attributeAtVertex`.
fn without<R>(f: impl FnOnce() -> R) -> R {
    lower::TEST_ATTRIBUTE_AT_VERTEX.set(Some(false));
    let r = f();
    lower::TEST_ATTRIBUTE_AT_VERTEX.set(None);
    r
}

/// Whether the translation `f` made called `dx.op.<name>`.
fn calls<R>(name: &str, f: impl FnOnce() -> R) -> (R, bool) {
    vertex_flat::take_ops();
    let r = f();
    let called = vertex_flat::take_ops().iter().any(|o| o.starts_with(name));
    (r, called)
}

/// A pixel program: `v_mul_f32 v3, 0.5, v0` (a computed i), `v_interp_p1_f32 v2, v3, attr0.x` and
/// `v_interp_p2_f32 v2, v1, attr0.x`, then `exp mrt0`: TEXCOORD0.x from its three vertices.
fn pixel() -> Result<HostShader, ShaderError> {
    let words = [
        0x1006_00F0u32, // v_mul_f32 v3, 0.5, v0
        0xC808_0003,    // v_interp_p1_f32 v2, v3, attr0.x
        0xC809_0001,    // v_interp_p2_f32 v2, v1, attr0.x
        0xF800_180F,    // exp mrt0 v2 v2 v2 v2 done vm
        2 * 0x0101_0101,
        S_ENDPGM,
    ];
    let native = NativeProgram {
        stage: HwStage::Ps,
        wave64: false,
        vgprs: 8,
        layout: vec![],
        used: vec![],
        io: StageIo::Ps {
            inputs: vec![PsInput {
                usage: Usage::Generic,
                row: 0,
                mask: 0b1111,
                attr: 0,
                flat: false,
                fp16: false,
                pack2: false,
                row2: 0,
            }],
            input_addr: 1 << 1, // PERSP_CENTER
            db_shader_control: 0,
            patches: vec![],
        },
        code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
    };
    let mut target = element("SV_Target", 16, 0);
    target.usage = 15;
    let isg1 = container::signature(&[element("TEXCOORD", 0, 0)], true);
    let osg1 = container::signature(&[target], false);
    let bytes = crate::dxbc::build(&[(FourCC::ISG1, &isg1), (FourCC::OSG1, &osg1)]);
    let c = Container::parse(&bytes).unwrap();
    let p = ir::build(&native.patched_code(), native.wave64, native.vgprs)?;
    let res = resource::track(&p, &native.layout, native.stage, None)?;
    let io = io::raise(&p, &native)?;
    let st = structure::structure(&p)?;
    emit(&c, &native, &p, &st, &io, &res)
}

/// A 32-lane triangle mesh program: lane n is vertex n at (lane & 1 ? 3 : -1, lane & 2 ? 3 : -1, 0.5, 1) with
/// TEXCOORD0 = the same four floats, and primitive n = triangle (0, 1, 2). `outputs`: the mesh's
/// vertex output signature beside the position.
fn mesh(lanes: u32, max_vertices: u32, max_primitives: u32) -> Result<HostShader, ShaderError> {
    let words = [
        0xD765_0001u32, // v_mbcnt_lo_u32_b32 v1, -1, 0
        0x0001_00C1,
        0x3604_0281, // v_and_b32 v2, 1, v1
        0x3606_0282, // v_and_b32 v3, 2, v1
        0x7E04_0D02, // v_cvt_f32_u32 v2, v2
        0x7E06_0D03, // v_cvt_f32_u32 v3, v3
        0x1008_04FF, // v_mul_f32 v4, 4.0, v2
        4.0f32.to_bits(),
        0x0608_08F3, // v_add_f32 v4, -1.0, v4
        0x100A_06FF, // v_mul_f32 v5, 2.0, v3
        2.0f32.to_bits(),
        0x060A_0AF3, // v_add_f32 v5, -1.0, v5
        0x7E0C_02F0, // v_mov_b32 v6, 0.5
        0x7E0E_02F2, // v_mov_b32 v7, 1.0
        0x7E12_02FF, // v_mov_b32 v9, prim
        1 << 10 | 2 << 20,
        0xF800_0941, // exp prim v9, off, off, off
        0x0000_0009,
        0xF800_020F, // exp param0 v4, v5, v6, v7
        0x0706_0504,
        0xF800_08CF, // exp pos0 v4, v5, v6, v7 done
        0x0706_0504,
        S_ENDPGM,
    ];
    let native = NativeProgram {
        stage: HwStage::Ms,
        wave64: false,
        vgprs: 10,
        layout: vec![],
        used: vec![],
        io: StageIo::Ms(MeshIo {
            outputs: vec![VsOutput { usage: Usage::Generic, row: 1, param: 0, fp16: false, pack2: false }],
            clip_cntl: 0,
            threads: lanes,
            max_vertices,
            max_primitives,
            topology: 2,
            lds_bytes: 0,
        }),
        code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
    };
    let position = SigElement { kind: 3, usage: 0xF, ..element("SV_Position", 3, 0) };
    let uv = element("TEXCOORD", 0, 1);
    let osg1 = container::signature(&[position, uv], false);
    let bytes = crate::dxbc::build(&[(FourCC::OSG1, &osg1)]);
    let c = Container::parse(&bytes)?;
    let p = ir::build(&native.patched_code(), native.wave64, native.vgprs)?;
    let st = structure::structure(&p)?;
    let res = resource::track(&p, &native.layout, native.stage, Some(&st))?;
    let io = io::raise(&p, &native)?;
    emit(&c, &native, &p, &st, &io, &res)
}

fn read(name: &str, index: u32, channel: u8, vertices: u8) -> VertexRead {
    VertexRead { name: name.into(), index, channel, vertices }
}

#[test]
fn a_host_with_attribute_at_vertex_is_translated_as_before() {
    let (ps, called) = calls("attributeAtVertex", pixel);
    assert!(called);
    assert!(ps.unwrap().vertex_reads.is_empty());
}

#[test]
fn a_pixel_program_reads_vertices_one_and_two_from_added_flat_inputs() {
    let (ps, called) = calls("attributeAtVertex", || without(pixel));
    assert!(!called, "still asks the host for an attribute at a vertex");
    let ps = ps.unwrap();
    assert_eq!(ps.vertex_reads, vec![read("TEXCOORD", 0, 0, 3)]);
    let c = Container::parse(&ps.dxil).unwrap();
    let inputs = signature(c.part(FourCC::ISG1).unwrap().data).unwrap();
    // The added element (vertices 1 and 2 of the one channel; vertex 0 is the element's own input).
    let added = inputs.iter().find(|e| e.index == vertex_flat::FLAT_BASE).unwrap();
    assert_eq!((added.name.as_str(), added.cols, added.comp), ("TEXCOORD", 2, 5));
    // i and j still come from SV_Barycentrics.
    assert_eq!(inputs.iter().filter(|e| e.kind == 28).count(), 1);
}

#[test]
fn the_capability_is_part_of_the_translation_settings() {
    let with = lower::host_settings();
    let without = without(lower::host_settings);
    assert_ne!(with, without);
    assert_eq!(with[..3], without[..3]);
    assert_eq!(without[3] >> 31, 1);
}

#[test]
fn a_mesh_program_hands_the_pixel_program_its_vertices() {
    let reads = [read("TEXCOORD", 0, 0, 3), read("TEXCOORD", 0, 3, 1)];
    let (ms, stored) = calls("storePrimitiveOutput", || vertex_flat::with_reads(&reads, || mesh(32, 32, 32)));
    let ms = ms.unwrap();
    assert!(stored);
    assert_eq!(ms.vertex_reads.len(), 2);
    let c = Container::parse(&ms.dxil).unwrap();
    // Three scalars (v1 and v2 of .x, v1 of .w) in one element, as the pixel stage lays them out.
    let prims = signature(c.part(FourCC::PSG1).unwrap().data).unwrap();
    let added = prims.iter().find(|e| e.index == vertex_flat::FLAT_BASE).unwrap();
    assert_eq!((added.name.as_str(), added.cols, added.comp), ("TEXCOORD", 3, 5));
    // Without a demand the program is what it was.
    let (plain, stored) = calls("storePrimitiveOutput", || mesh(32, 32, 32));
    assert!(plain.unwrap().vertex_reads.is_empty());
    assert!(!stored);
}

#[test]
fn the_two_stages_agree_on_the_layout() {
    // What the pixel program says it reads is what the mesh program is given, and the added elements of
    // both stages carry the same names, indices and widths.
    let ps = without(pixel).unwrap();
    let ms = vertex_flat::with_reads(&ps.vertex_reads, || mesh(32, 32, 32)).unwrap();
    let pc = Container::parse(&ps.dxil).unwrap();
    let mc = Container::parse(&ms.dxil).unwrap();
    let inputs = signature(pc.part(FourCC::ISG1).unwrap().data).unwrap();
    let prims = signature(mc.part(FourCC::PSG1).unwrap().data).unwrap();
    let added = |els: &[SigElement]| -> Vec<(String, u32, u8)> {
        els.iter()
            .filter(|e| e.index >= vertex_flat::FLAT_BASE)
            .map(|e| (e.name.clone(), e.index, e.cols))
            .collect()
    };
    assert_eq!(added(&inputs), added(&prims));
    assert!(!added(&prims).is_empty());
}

#[test]
fn an_exchange_past_group_memory_is_refused() {
    // 76 x 4 channels per vertex, 32 lanes: 9,728 dwords, past the 8,192 of 32 KiB.
    let reads: Vec<VertexRead> = (0..76u32).flat_map(|i| (0..4u8).map(move |c| read("TEXCOORD", i, c, 3))).collect();
    let r = vertex_flat::with_reads(&reads, || mesh(32, 32, 32));
    assert!(
        matches!(r, Err(ShaderError::Unsupported { what, .. }) if what.contains("group memory")),
        "{r:?}"
    );
}

#[test]
fn a_vertex_pipeline_gets_a_geometry_stage_that_carries_the_other_vertices() {
    let position = SigElement { kind: 3, usage: 0xF, ..element("SV_Position", 3, 0) };
    let uv = element("TEXCOORD", 0, 1);
    let vs = crate::dxbc::build(&[(FourCC::OSG1, &container::signature(&[position, uv.clone()], false))]);
    let flat = SigElement { interp: 1, ..uv };
    let ps = crate::dxbc::build(&[(FourCC::ISG1, &container::signature(&[flat], true))]);
    let reads = [read("TEXCOORD", 0, 0, 3), read("TEXCOORD", 0, 2, 2)];
    let gs = vertex_flat::vertex_pipeline_gs(&vs, &ps, &reads).unwrap();
    let c = Container::parse(&gs).unwrap();
    // Geometry (2), shader model 6.6.
    assert_eq!(c.part(FourCC::DXIL).unwrap().data[..4], [0x66, 0, 2, 0]);
    let ins = signature(c.part(FourCC::ISG1).unwrap().data).unwrap();
    let outs = signature(c.part(FourCC::OSG1).unwrap().data).unwrap();
    assert_eq!(ins.len(), 2);
    // The vertex's own outputs, then the added element: v1.x, v2.x, v1.z.
    assert_eq!(outs.len(), 3);
    let added = outs.iter().find(|e| e.index == vertex_flat::FLAT_BASE).unwrap();
    assert_eq!((added.cols, added.comp), (3, 5));
    // A read of a channel no vertex output has is refused.
    let r = vertex_flat::vertex_pipeline_gs(&vs, &ps, &[read("COLOR", 0, 0, 3)]);
    assert!(r.is_err());
}
