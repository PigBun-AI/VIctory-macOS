//! S-P-001 / S-P-002: a pipeline made from a metallib behaves the same
//! whether it is compiled cold or found in an MTL4Archive. Own test binary:
//! the capture and lookup switches are process-wide.
//!
//! Needs a DXC (see `common_msc.rs`); without one the test prints SKIPPED.
#![cfg(target_vendor = "apple")]

mod common_msc;

use victory_metal_back::archive;
use victory_metal_back::device::Device;
use victory_metal_back::memory::BufferKind;
use victory_metal_back::msc::{CompileOptions, Param, ParamKind, RootSignatureDesc, Visibility};
use victory_metal_back::queue::Queue;

use common_msc::dxil_or_skip;

const SRC: &str = "cbuffer c : register(b0) { uint a; uint b; }\n\
RWByteAddressBuffer o : register(u0);\n\
[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
  o.Store(t.x*4, (t.x * a) ^ b); }\n";

fn root() -> RootSignatureDesc {
    let p = |kind| Param {
        kind,
        visibility: Visibility::All,
    };
    RootSignatureDesc {
        params: vec![
            p(ParamKind::Constants {
                register: 0,
                space: 0,
                num_32bit: 2,
            }),
            p(ParamKind::Uav {
                register: 0,
                space: 0,
            }),
        ],
        ..Default::default()
    }
}

fn run_once(d: &Device, dxil: &[u8], a: u32, b: u32) -> Vec<u32> {
    let q = Queue::new(d).unwrap();
    let sp = d
        .compute_pipeline_from_dxil(dxil, Some(&root()), &CompileOptions::default())
        .unwrap();
    let r = &sp.reflection;
    let n = 32 * 4;
    let out = d.create_buffer(BufferKind::Shared, 4 * n).unwrap();
    out.write_u32s(0, &vec![0xDEAD_BEEF; n as usize]).unwrap();
    let tlab = q.alloc_constants(u64::from(r.layout.size), 8).unwrap();
    tlab.zero();
    for e in &r.layout.entries {
        use victory_metal_back::msc::ArgKind;
        match e.kind {
            ArgKind::Uav => tlab.write_u64(u64::from(e.offset), out.gpu_va()).unwrap(),
            _ => {
                tlab.write_u32s(u64::from(e.offset), &[a, b]).unwrap();
            }
        }
    }
    let mut l = q.begin_list().unwrap();
    l.set_root_arguments(&tlab).unwrap();
    l.dispatch(&sp.pipeline, [4, 1, 1], [32, 1, 1]).unwrap();
    l.end().unwrap();
    let v = q.submit(vec![l]).expect("submit");
    q.wait_for(v, 20_000).expect("completes");
    out.read_u32s(0, n as usize).unwrap()
}

#[test]
fn an_archived_pipeline_computes_what_a_cold_compile_computes() {
    let Some(dxil) = dxil_or_skip("archive_cs", "cs_6_6", SRC) else {
        return;
    };
    let path = std::env::temp_dir().join(format!("victory-archive-{}.bin", std::process::id()));
    let _ = std::fs::remove_file(&path);
    archive::arm_capture();
    let d = Device::new().expect("Metal 4 device");
    let want: Vec<u32> = (0..128u32).map(|i| (i * 7) ^ 0x55).collect();
    // cold, captured
    assert_eq!(run_once(&d, &dxil, 7, 0x55), want);
    archive::flush(&path).unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > 0);
    // from the archive: same shader, other constants, same code path
    let a = archive::open(&d, &path).unwrap();
    archive::set_lookup(&[&a]);
    assert_eq!(run_once(&d, &dxil, 7, 0x55), want);
    let other: Vec<u32> = (0..128u32).map(|i| (i * 3) ^ 0x0F).collect();
    assert_eq!(run_once(&d, &dxil, 3, 0x0F), other);
    // lookup off again: a cold compile still agrees
    archive::set_lookup(&[]);
    assert_eq!(run_once(&d, &dxil, 7, 0x55), want);
    let _ = std::fs::remove_file(&path);
}
