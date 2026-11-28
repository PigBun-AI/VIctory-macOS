//! Mac patch S8: a 64-bit image atomic max that returns the original is lowered as a 64-bit
//! load and an atomic whose result is unused (MSC refuses the other form).

use super::lower;
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use crate::{ShaderError, io, ir, resource, structure};

/// Lowers a wave32 compute program of one thread with the table of views at
/// s0 and the view heap at s1; `srv`: the dwords and the stride of the used
/// resource at the table's first entry (the third is a raw UAV).
fn build(words: &[u32], srv: (u8, u32)) -> Result<Vec<u8>, ShaderError> {
    let param = |root, user_data| RootParam {
        root,
        table_offset_dwords: 0,
        sgprs: 1,
        base_register: 0,
        user_data,
        slot: 0,
    };
    let range = |kind, table_offset_dwords, dwords, stride| UsedResource {
        kind,
        root: RootType::Table,
        table_offset_dwords,
        lower_bound: 0,
        stride,
        dwords,
        user_data: 0,
        slot: 0,
    };
    let native = NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
        vgprs: 32,
        layout: vec![param(RootType::Table, 0), param(RootType::ResourceHeap, 1)],
        used: vec![
            range(ResourceType::Srv, 0, srv.0, srv.1),
            range(ResourceType::Uav, 16, 4, 0),
        ],
        io: StageIo::Cs(ComputeIo {
            rsrc2: 2 << 1,
            threads: [1, 1, 1],
        }),
        code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
    };
    let p = ir::build(&native.code, native.wave64, native.vgprs)?;
    let st = structure::structure(&p)?;
    let res = resource::track(&p, &native.layout, native.stage, Some(&st))?;
    let io = io::raise(&p, &native)?;
    let lowered = lower::lower(&lower::Parts {
        native: &native,
        p: &p,
        st: &st,
        io: &io,
        res: &res,
        inputs: Vec::new(),
        outputs: Vec::new(),
        prims: Vec::new(),
        mesh_stride: 0,
        cs: None, // mac S5
    })?;
    Ok(lowered.dxil)
}

/// The table at s[2:3], its raw UAV's descriptor in s[20:23].
const TABLE: &[u32] = &[
    0xBE82_0300, // s_mov_b32 s2, s0
    0xBE83_0380, // s_mov_b32 s3, 0
    0xF408_0501, // s_load_dwordx4 s[20:23], s[2:3], 64
    0xFA00_0040,
];

/// Stores s4 and s5 at bytes 0 and 4 of the UAV.
const STORE: &[u32] = &[
    0x7E08_0204, // v_mov_b32 v4, s4
    0x7E0A_0205, // v_mov_b32 v5, s5
    0xE070_0000, // buffer_store_dword v4, off, s[20:23], 0
    0x8005_0400,
    0xE070_0004, // buffer_store_dword v5, off, s[20:23], 0 offset:4
    0x8005_0500,
    0xBF81_0000, // s_endpgm
];

/// mac S8: a 64-bit image atomic max that returns the original (`image_atomic_umax` of two
/// channels with GLC) reads the texel first and leaves the atomic's result unused, so MSC takes it;
/// without GLC, or with `VICTORY_F2_READ_FIRST=0`, nothing is read first.
#[test]
fn a_64_bit_image_atomic_max_with_its_original_reads_the_texel_first() {
    let program = |atomic: [u32; 2], switch: Option<bool>| {
        lower::TEST_F2_READ_FIRST.set(switch);
        let words = [
            0xF40C_0201u32, // s_load_dwordx8 s[8:15], s[2:3], 0
            0xFA00_0000,
            0x7E00_0280, // v_mov_b32 v0, 0
            0x7E02_0280, // v_mov_b32 v1, 0
            0x7E04_0285, // v_mov_b32 v2, 5
            0x7E06_0280, // v_mov_b32 v3, 0
            atomic[0], // image_atomic_umax v[2:3], v[0:1], s[8:15] dmask:0x3 2D (glc)
            atomic[1],
            0xBE84_0380, // s_mov_b32 s4, 0
            0xBE85_0380, // s_mov_b32 s5, 0
        ];
        super::vertex_flat::take_ops();
        build(&[TABLE, &words, STORE].concat(), (8, 0)).expect("translates");
        super::vertex_flat::take_ops()
    };
    let with = program([0xF05C_3308, 0x0002_0200], None);
    assert!(with.iter().any(|o| o == "textureLoad.i32"), "{with:?}");
    assert!(with.iter().any(|o| o == "atomicBinOp.i64"), "{with:?}");
    let without = program([0xF05C_1308, 0x0002_0200], None);
    let off = program([0xF05C_3308, 0x0002_0200], Some(false));
    assert!(!off.iter().any(|o| o == "textureLoad.i32"), "{off:?}");
    lower::TEST_F2_READ_FIRST.set(None);
    assert!(!without.iter().any(|o| o == "textureLoad.i32"), "{without:?}");
    assert!(without.iter().any(|o| o == "atomicBinOp.i64"), "{without:?}");
}
