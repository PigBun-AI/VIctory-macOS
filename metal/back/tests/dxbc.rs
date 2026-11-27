//! DXBC in: shaders `D3DCompile` made (shader model 5.1) go through the
//! DXBC→DXIL converter (`libdxilconv`, x86_64 only) into MSC and run on the
//! GPU. The DXBC is made by `tools/metal/dxbc-compile.sh` (named by
//! `VICTORY_DXBC_TOOL`; Wine's compiler), nothing is stored. The converter
//! library is D3DMetal's: `VICTORY_DXILCONV_DYLIB` or its default path. On
//! arm64 the converter cannot load, so those tests print SKIPPED: run them
//! under Rosetta (`--target x86_64-apple-darwin`).
#![cfg(target_vendor = "apple")]

mod common_msc;

use victory_metal_back::device::Device;
use victory_metal_back::memory::BufferKind;
use victory_metal_back::msc::dxbc::DxbcConverter;
use victory_metal_back::msc::{
    CompileOptions, Msc, MscError, Param, ParamKind, RootSignatureDesc, Visibility, dxil,
};
use victory_metal_back::queue::Queue;

use common_msc::dxbc_or_skip;

const UAV_AND_CONSTANTS: &str = "RWByteAddressBuffer dst : register(u0);\n\
cbuffer C : register(b0) { uint a; uint b; };\n\
[numthreads(8,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
  dst.Store(t.x * 4, t.x * a + b); }\n";

const SPACES: &str = "cbuffer K : register(b0, space0) { uint scale; uint bias; uint count; uint pad; };\n\
cbuffer R : register(b1, space3) { uint4 rootv; };\n\
ByteAddressBuffer src : register(t2, space1);\n\
RWByteAddressBuffer dst : register(u1, space1);\n\
[numthreads(4,2,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
  uint i = t.y * 4 + t.x;\n\
  if (i < count) dst.Store(i * 4, src.Load(i * 4) * scale + bias + rootv.x); }\n";

/// The converter, or a SKIPPED line (not installed or the wrong architecture).
fn converter(test: &str) -> Option<DxbcConverter> {
    match DxbcConverter::get() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("SKIPPED {test}: {e}");
            None
        }
    }
}

fn param(kind: ParamKind) -> Param {
    Param {
        kind,
        visibility: Visibility::All,
    }
}

#[test]
fn dxbc_is_told_from_dxil_by_its_parts() {
    // A container: magic, digest, version, size, one part at offset 36.
    let container = |fourcc: &[u8; 4]| {
        let mut c = Vec::new();
        c.extend(b"DXBC");
        c.extend([0u8; 16]);
        c.extend(1u32.to_le_bytes());
        c.extend(48u32.to_le_bytes());
        c.extend(1u32.to_le_bytes());
        c.extend(36u32.to_le_bytes());
        c.extend(fourcc);
        c.extend(4u32.to_le_bytes());
        c.extend(0x0006_0066u32.to_le_bytes());
        c
    };
    assert!(dxil::is_dxbc(&container(b"SHEX")));
    assert!(dxil::is_dxbc(&container(b"SHDR")));
    assert!(
        !dxil::is_dxbc(&container(b"DXIL")),
        "a DXIL part makes it DXIL"
    );
    assert!(!dxil::is_dxbc(&container(b"RDEF")));
    assert!(!dxil::is_dxbc(b"not a container at all, really"));
}

#[test]
fn the_library_that_is_not_there_says_so_through_compile() {
    // DXBC bytes with the converter pointed nowhere would race with the other
    // tests' environment: the loader is tested with explicit paths instead.
    let e = DxbcConverter::load_from(&["/nonexistent/libdxilconv.dylib".into()])
        .err()
        .unwrap();
    assert!(matches!(e, MscError::Library { .. }), "{e}");
}

#[test]
fn a_cs_5_1_shader_runs_through_the_converter_and_msc() {
    let Some(conv) = converter("a_cs_5_1_shader_runs") else {
        return;
    };
    let Some(dxbc) = dxbc_or_skip("uav_constants", "cs_5_1", UAV_AND_CONSTANTS) else {
        return;
    };
    assert!(dxil::is_dxbc(&dxbc));
    let converted = conv.convert(&dxbc).unwrap();
    let info = dxil::inspect(&converted).unwrap();
    assert!(info.kind.is_some(), "the output has a DXIL part: {info:?}");
    eprintln!("converter {}: {info:?}", conv.path());

    let d = Device::new().expect("Metal 4 device");
    let q = Queue::new(&d).unwrap();
    let root = RootSignatureDesc {
        params: vec![
            param(ParamKind::Constants {
                register: 0,
                space: 0,
                num_32bit: 2,
            }),
            param(ParamKind::Uav {
                register: 0,
                space: 0,
            }),
        ],
        ..Default::default()
    };
    // The DXBC goes in as it is: `compile` converts it.
    let sp = d
        .compute_pipeline_from_dxil(&dxbc, Some(&root), &CompileOptions::default())
        .unwrap();
    let lay = &sp.reflection.layout;
    assert_eq!(lay.entries.len(), 2, "{lay:?}");
    let (a, b) = (5u32, 1000u32);
    let out = d.create_buffer(BufferKind::Shared, 4 * 32).unwrap();
    out.write_u32s(0, &[0; 32]).unwrap();
    let tlab = q.alloc_constants(u64::from(lay.size), 8).unwrap();
    tlab.zero();
    tlab.write_u32s(u64::from(lay.entries[0].offset), &[a, b])
        .unwrap();
    tlab.write_u64(u64::from(lay.entries[1].offset), out.gpu_va())
        .unwrap();
    let mut l = q.begin_list().unwrap();
    l.set_root_arguments(&tlab).unwrap();
    l.dispatch(&sp.pipeline, [4, 1, 1], [8, 1, 1]).unwrap();
    l.end().unwrap();
    let v = q.submit(vec![l]).unwrap();
    q.wait_for(v, 20_000).unwrap();
    let want: Vec<u32> = (0..32u32).map(|t| t * a + b).collect();
    assert_eq!(out.read_u32s(0, 32).unwrap(), want);
}

#[test]
fn register_spaces_of_shader_model_5_1_land_where_the_root_signature_puts_them() {
    let Some(conv) = converter("register_spaces") else {
        return;
    };
    let Some(dxbc) = dxbc_or_skip("spaces", "cs_5_1", SPACES) else {
        return;
    };
    let converted = conv.convert(&dxbc).unwrap();
    assert!(dxil::inspect(&converted).unwrap().kind.is_some());
    let d = Device::new().expect("Metal 4 device");
    let q = Queue::new(&d).unwrap();
    // b0 space0 as root constants, b1 space3 root CBV, t2 space1 root SRV,
    // u1 space1 root UAV.
    let root = RootSignatureDesc {
        params: vec![
            param(ParamKind::Constants {
                register: 0,
                space: 0,
                num_32bit: 4,
            }),
            param(ParamKind::Cbv {
                register: 1,
                space: 3,
            }),
            param(ParamKind::Srv {
                register: 2,
                space: 1,
            }),
            param(ParamKind::Uav {
                register: 1,
                space: 1,
            }),
        ],
        ..Default::default()
    };
    let sp = d
        .compute_pipeline_from_dxil(&dxbc, Some(&root), &CompileOptions::default())
        .unwrap();
    let lay = &sp.reflection.layout;
    assert_eq!(lay.entries.len(), 4, "{lay:?}");
    let n = 8usize;
    let src = d.create_buffer(BufferKind::Shared, 4 * n as u64).unwrap();
    src.write_u32s(0, &(0..n as u32).map(|i| i + 10).collect::<Vec<_>>())
        .unwrap();
    let cb = d.create_buffer(BufferKind::Shared, 16).unwrap();
    cb.write_u32s(0, &[3, 0, 0, 0]).unwrap();
    let out = d.create_buffer(BufferKind::Shared, 4 * 16).unwrap();
    out.write_u32s(0, &[0xAAAA_AAAA; 16]).unwrap();
    let (scale, bias, count) = (7u32, 100u32, n as u32);
    let tlab = q.alloc_constants(u64::from(lay.size), 8).unwrap();
    tlab.zero();
    tlab.write_u32s(u64::from(lay.entries[0].offset), &[scale, bias, count, 0])
        .unwrap();
    tlab.write_u64(u64::from(lay.entries[1].offset), cb.gpu_va())
        .unwrap();
    tlab.write_u64(u64::from(lay.entries[2].offset), src.gpu_va())
        .unwrap();
    tlab.write_u64(u64::from(lay.entries[3].offset), out.gpu_va())
        .unwrap();
    let mut l = q.begin_list().unwrap();
    l.set_root_arguments(&tlab).unwrap();
    l.dispatch(&sp.pipeline, [1, 1, 1], [4, 2, 1]).unwrap();
    l.end().unwrap();
    let v = q.submit(vec![l]).unwrap();
    q.wait_for(v, 20_000).unwrap();
    let got = out.read_u32s(0, 16).unwrap();
    for (i, &g) in got.iter().enumerate().take(n) {
        assert_eq!(g, (i as u32 + 10) * scale + bias + 3, "element {i}");
    }
    assert!(
        got[n..].iter().all(|&v| v == 0xAAAA_AAAA),
        "no write past count"
    );
}

#[test]
fn msc_alone_still_refuses_dxbc_it_was_not_given_the_converter_for() {
    // The refusal of the bare header (the failure the game layer saw).
    let Ok(msc) = Msc::get() else {
        return;
    };
    let mut junk = b"DXBC".to_vec();
    junk.extend([0u8; 60]);
    // No parts: not DXBC bytecode, so it goes to MSC unchanged and is refused there.
    let e = msc.compile(&junk, None, &CompileOptions::default());
    assert!(e.is_err());
}
