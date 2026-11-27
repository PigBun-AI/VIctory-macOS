//! Metal Shader Converter on the GPU: DXIL from HLSL, converted with a root
//! signature, run through [`Device::compute_pipeline_from_dxil`], and
//! compared with a CPU reference. Native on arm64 and under Rosetta
//! (`--target x86_64-apple-darwin`).
//!
//! Needs `libmetalirconverter.dylib` (fails without it: MSC is required) and,
//! for the DXIL tests, a DXC (see `common_msc.rs`; they skip without one).
#![cfg(target_vendor = "apple")]

mod common_msc;

use std::sync::{Mutex, MutexGuard, OnceLock};

use victory_metal_back::descriptors::DescriptorHeap;
use victory_metal_back::device::Device;
use victory_metal_back::memory::BufferKind;
use victory_metal_back::msc::{
    ArgKind, CompileOptions, Msc, MscError, Param, ParamKind, RootSignatureDesc, ShaderStage,
    Visibility, flags,
};
use victory_metal_back::queue::Queue;

use common_msc::dxil_or_skip;

fn setup() -> (MutexGuard<'static, ()>, Device) {
    static LOCK: Mutex<()> = Mutex::new(());
    static D: OnceLock<Device> = OnceLock::new();
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let d = D
        .get_or_init(|| Device::new().expect("Metal 4 device"))
        .clone();
    (guard, d)
}

fn run(q: &Queue, mut l: victory_metal_back::record::CommandList) {
    l.end().unwrap();
    let v = q.submit(vec![l]).expect("submit");
    q.wait_for(v, 20_000).expect("completes");
}

fn constants(n: u32) -> Param {
    Param {
        kind: ParamKind::Constants {
            register: 0,
            space: 0,
            num_32bit: n,
        },
        visibility: Visibility::All,
    }
}

fn root_uav(register: u32) -> Param {
    Param {
        kind: ParamKind::Uav { register, space: 0 },
        visibility: Visibility::All,
    }
}

#[test]
fn the_library_loads_and_reports_its_path() {
    let m = Msc::get().expect("MSC 4.0.3 installed");
    eprintln!("MSC loaded from {}", m.path());
    assert!(m.path().ends_with("libmetalirconverter.dylib"));
}

#[test]
fn a_missing_library_is_a_clear_error() {
    let e = Msc::load_from(&["/nonexistent/libmetalirconverter.dylib".to_owned()]).unwrap_err();
    let text = e.to_string();
    assert!(matches!(e, MscError::Library { .. }), "{text}");
    assert!(
        text.contains("/nonexistent/libmetalirconverter.dylib"),
        "{text}"
    );
    assert!(text.contains("Install Metal Shader Converter"), "{text}");
    // A file that is not a library, and a library without MSC's symbols.
    let e = Msc::load_from(&["/usr/lib/libSystem.B.dylib".to_owned()]).unwrap_err();
    assert!(e.to_string().contains("lacks `IR"), "{e}");
}

/// The top-level argument buffer is laid out by MSC, with padding: three
/// root constants (12 bytes) put a root descriptor at 16, not 12.
#[test]
fn the_top_level_layout_comes_from_msc_with_its_padding() {
    let m = Msc::get().unwrap();
    let desc = RootSignatureDesc {
        flags: 0,
        params: vec![
            constants(3),
            root_uav(0),
            Param {
                kind: ParamKind::Table(vec![victory_metal_back::msc::Range {
                    kind: victory_metal_back::msc::RangeKind::Srv,
                    count: 4,
                    base_register: 0,
                    space: 0,
                    offset: victory_metal_back::msc::OFFSET_APPEND,
                    flags: 0,
                }]),
                visibility: Visibility::All,
            },
            constants(1),
        ],
        static_samplers: vec![],
    };
    let l = m.layout(&desc).unwrap();
    let got: Vec<_> = l
        .entries
        .iter()
        .map(|e| (e.kind, e.offset, e.size))
        .collect();
    assert_eq!(
        got,
        vec![
            (ArgKind::Constants, 0, 12),
            (ArgKind::Uav, 16, 8),
            (ArgKind::Table, 24, 8),
            (ArgKind::Constants, 32, 4),
        ]
    );
    assert_eq!(l.size, 40);
}

#[test]
fn an_empty_root_signature_lays_out_nothing() {
    let l = Msc::get()
        .unwrap()
        .layout(&RootSignatureDesc::default())
        .unwrap();
    assert!(l.entries.is_empty());
    assert_eq!(l.size, 0);
}

#[test]
fn garbage_dxil_is_rejected_with_a_code() {
    let m = Msc::get().unwrap();
    let e = m
        .compile(b"this is not DXIL at all", None, &CompileOptions::default())
        .unwrap_err();
    eprintln!("{e}");
    assert!(
        matches!(e, MscError::Rejected { .. } | MscError::NoOutput(_)),
        "{e}"
    );
    let e = m
        .compile(&[], None, &CompileOptions::default())
        .unwrap_err();
    assert!(matches!(e, MscError::BadInput(_)));
}

const ROOT_UAV: &str = "RWByteAddressBuffer o : register(u0);\n\
[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) { o.Store(t.x*4, t.x*3u+1u); }\n";

#[test]
fn compute_with_a_root_uav() {
    let Some(dxil) = dxil_or_skip("root_uav", "cs_6_6", ROOT_UAV) else {
        return;
    };
    let (_g, d) = setup();
    let q = Queue::new(&d).unwrap();
    let root = RootSignatureDesc {
        params: vec![root_uav(0)],
        ..Default::default()
    };
    let sp = d
        .compute_pipeline_from_dxil(&dxil, Some(&root), &CompileOptions::default())
        .unwrap();
    let r = &sp.reflection;
    eprintln!("{r:#?} metallib {} bytes", sp.metallib_bytes);
    assert_eq!(r.stage, ShaderStage::Compute);
    assert_eq!(r.threadgroup_size, [64, 1, 1]);
    assert!(!r.uses_resource_heap && !r.uses_sampler_heap);
    assert_eq!(r.layout.entries.len(), 1);
    assert_eq!(r.layout.entries[0].kind, ArgKind::Uav);
    assert!(sp.pipeline.max_threads_per_threadgroup() >= 64);

    let n = 64 * 8;
    let out = d.create_buffer(BufferKind::Shared, 4 * n).unwrap();
    out.write_u32s(0, &vec![0xDEAD_BEEF; n as usize]).unwrap();
    let tlab = q.alloc_constants(u64::from(r.layout.size), 8).unwrap();
    tlab.zero();
    tlab.write_u64(u64::from(r.layout.entries[0].offset), out.gpu_va())
        .unwrap();
    let mut l = q.begin_list().unwrap();
    l.set_root_arguments(&tlab).unwrap();
    l.dispatch(&sp.pipeline, [8, 1, 1], [64, 1, 1]).unwrap();
    run(&q, l);
    let got = out.read_u32s(0, n as usize).unwrap();
    let want: Vec<u32> = (0..n as u32).map(|i| i * 3 + 1).collect();
    assert_eq!(got, want);
}

const ROOT_CONSTANTS: &str = "cbuffer c : register(b0) { uint a; uint b; uint c; uint d; }\n\
RWByteAddressBuffer o : register(u0);\n\
[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
  o.Store(t.x*4, ((t.x * a + b) ^ c) + d); }\n";

#[test]
fn compute_with_root_constants() {
    let Some(dxil) = dxil_or_skip("root_constants", "cs_6_6", ROOT_CONSTANTS) else {
        return;
    };
    let (_g, d) = setup();
    let q = Queue::new(&d).unwrap();
    // Constants first, then the UAV: the UAV lands at offset 16 (8-aligned).
    let root = RootSignatureDesc {
        params: vec![constants(4), root_uav(0)],
        ..Default::default()
    };
    let sp = d
        .compute_pipeline_from_dxil(&dxil, Some(&root), &CompileOptions::default())
        .unwrap();
    let lay = &sp.reflection.layout;
    eprintln!("{lay:?}");
    assert_eq!(lay.entries[0].kind, ArgKind::Constants);
    assert_eq!((lay.entries[0].offset, lay.entries[0].size), (0, 16));
    assert_eq!(lay.entries[1].offset, 16);
    let (a, b, c, dd) = (7u32, 0x1234_5678, 0x0F0F_0F0F, 99);
    let n = 32 * 4;
    let out = d.create_buffer(BufferKind::Shared, 4 * n).unwrap();
    out.write_u32s(0, &vec![0; n as usize]).unwrap();
    let tlab = q.alloc_constants(u64::from(lay.size), 8).unwrap();
    tlab.zero();
    tlab.write_u32s(u64::from(lay.entries[0].offset), &[a, b, c, dd])
        .unwrap();
    tlab.write_u64(u64::from(lay.entries[1].offset), out.gpu_va())
        .unwrap();
    let mut l = q.begin_list().unwrap();
    l.set_root_arguments(&tlab).unwrap();
    l.dispatch(&sp.pipeline, [4, 1, 1], [32, 1, 1]).unwrap();
    run(&q, l);
    let got = out.read_u32s(0, n as usize).unwrap();
    let want: Vec<u32> = (0..n as u32)
        .map(|t| (t.wrapping_mul(a).wrapping_add(b) ^ c).wrapping_add(dd))
        .collect();
    assert_eq!(got, want);
}

const HEAP_INDEXED: &str = "cbuffer c : register(b0) { uint si; uint di; uint k; uint pad; }\n\
[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
  ByteAddressBuffer src = ResourceDescriptorHeap[si];\n\
  RWByteAddressBuffer dst = ResourceDescriptorHeap[di];\n\
  dst.Store(t.x*4, src.Load(t.x*4) * k + 7u); }\n";

#[test]
fn compute_through_the_resource_descriptor_heap() {
    let Some(dxil) = dxil_or_skip("heap_indexed", "cs_6_6", HEAP_INDEXED) else {
        return;
    };
    let (_g, d) = setup();
    let q = Queue::new(&d).unwrap();
    let root = RootSignatureDesc {
        flags: flags::HEAP_INDEXED | flags::SAMPLER_HEAP_INDEXED,
        params: vec![constants(4)],
        ..Default::default()
    };
    let sp = d
        .compute_pipeline_from_dxil(&dxil, Some(&root), &CompileOptions::default())
        .unwrap();
    let r = &sp.reflection;
    eprintln!("{r:#?}");
    assert!(r.uses_resource_heap, "SM 6.6 heap indexing is reported");
    assert!(!r.uses_sampler_heap);
    assert_eq!(r.layout.entries[0].kind, ArgKind::Constants);

    let n = 32 * 2;
    let src = d.create_buffer(BufferKind::Shared, 4 * n).unwrap();
    let dst = d.create_buffer(BufferKind::Shared, 4 * n).unwrap();
    let words: Vec<u32> = (0..n as u32).map(|i| i * 5 + 2).collect();
    src.write_u32s(0, &words).unwrap();
    dst.write_u32s(0, &vec![0; n as usize]).unwrap();
    let heap = DescriptorHeap::new(&d, 64).unwrap();
    let (si, di, k) = (17u32, 40u32, 3u32);
    heap.write_buffer(si, &src, false).unwrap();
    heap.write_buffer(di, &dst, false).unwrap();
    let tlab = q.alloc_constants(u64::from(r.layout.size), 8).unwrap();
    tlab.zero();
    tlab.write_u32s(u64::from(r.layout.entries[0].offset), &[si, di, k, 0])
        .unwrap();
    let mut l = q.begin_list().unwrap();
    l.set_heaps(&heap, None).unwrap();
    l.set_root_arguments(&tlab).unwrap();
    l.dispatch(&sp.pipeline, [2, 1, 1], [32, 1, 1]).unwrap();
    run(&q, l);
    let got = dst.read_u32s(0, n as usize).unwrap();
    let want: Vec<u32> = words.iter().map(|w| w * k + 7).collect();
    assert_eq!(got, want);
}

/// Without a root signature MSC lays every resource the shader declares
/// out as an inline descriptor-table entry (24 bytes, `IRDescriptorTableEntry`)
/// in declaration order: a different TLAB than any root signature gives.
#[test]
fn a_shader_without_a_root_signature_gets_inline_descriptor_entries() {
    let Some(dxil) = dxil_or_skip("no_rs", "cs_6_6", ROOT_UAV) else {
        return;
    };
    let (_g, d) = setup();
    let sp = d
        .compute_pipeline_from_dxil(&dxil, None, &CompileOptions::default())
        .unwrap();
    let e = &sp.reflection.layout.entries;
    assert_eq!(e.len(), 1);
    assert_eq!((e[0].kind, e[0].offset, e[0].size), (ArgKind::Uav, 0, 24));
}

const SAMPLER_HEAP: &str = "cbuffer c : register(b0) { uint si; uint di; uint k; uint pad; }\n\
[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
  Texture2D<float4> tx = ResourceDescriptorHeap[si];\n\
  SamplerState s = SamplerDescriptorHeap[k];\n\
  RWByteAddressBuffer dst = ResourceDescriptorHeap[di];\n\
  dst.Store(t.x*4, asuint(tx.SampleLevel(s, float2(0.5, 0.5), 0).x)); }\n";

#[test]
fn reflection_reports_the_sampler_heap() {
    let Some(dxil) = dxil_or_skip("sampler_heap", "cs_6_6", SAMPLER_HEAP) else {
        return;
    };
    let (_g, d) = setup();
    let root = RootSignatureDesc {
        flags: flags::HEAP_INDEXED | flags::SAMPLER_HEAP_INDEXED,
        params: vec![constants(4)],
        ..Default::default()
    };
    let sp = d
        .compute_pipeline_from_dxil(&dxil, Some(&root), &CompileOptions::default())
        .unwrap();
    assert!(sp.reflection.uses_resource_heap);
    assert!(sp.reflection.uses_sampler_heap);
}

const TABLE_AND_STATIC_SAMPLER: &str = "Texture2D<float4> tx : register(t0);\n\
SamplerState s : register(s0);\n\
RWByteAddressBuffer o : register(u0);\n\
[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
  o.Store(t.x*4, asuint(tx.SampleLevel(s, float2(0.5, 0.5), 0).x)); }\n";

/// A descriptor table, a root UAV and a static sampler: the table is a
/// pointer in the TLAB, and the static sampler is one more pointer, to a
/// table of sampler descriptors the host fills in.
#[test]
fn a_table_a_root_uav_and_a_static_sampler() {
    use victory_metal_back::msc::{OFFSET_APPEND, Range, RangeKind, StaticSampler};
    let Some(dxil) = dxil_or_skip("table_sampler", "cs_6_6", TABLE_AND_STATIC_SAMPLER) else {
        return;
    };
    let (_g, d) = setup();
    let root = RootSignatureDesc {
        flags: 0,
        params: vec![
            Param {
                kind: ParamKind::Table(vec![Range {
                    kind: RangeKind::Srv,
                    count: 1,
                    base_register: 0,
                    space: 0,
                    offset: OFFSET_APPEND,
                    flags: 0,
                }]),
                visibility: Visibility::All,
            },
            root_uav(0),
        ],
        static_samplers: vec![StaticSampler {
            filter: 0x15, // D3D12_FILTER_MIN_MAG_MIP_LINEAR
            address: [1, 1, 1],
            mip_lod_bias: 0.0,
            max_anisotropy: 0,
            comparison: 1, // never
            border: 0,
            min_lod: 0.0,
            max_lod: f32::MAX,
            register: 0,
            space: 0,
            visibility: Visibility::All,
        }],
    };
    let sp = d
        .compute_pipeline_from_dxil(&dxil, Some(&root), &CompileOptions::default())
        .unwrap();
    let e = &sp.reflection.layout.entries;
    eprintln!("{e:?}");
    // Table, root UAV, then the table of the static samplers.
    assert_eq!(e.len(), 3);
    assert_eq!((e[0].kind, e[0].offset, e[0].size), (ArgKind::Table, 0, 8));
    assert_eq!((e[1].kind, e[1].offset, e[1].size), (ArgKind::Uav, 8, 8));
    assert_eq!((e[2].kind, e[2].offset, e[2].size), (ArgKind::Table, 16, 8));
    assert_eq!(sp.reflection.layout.static_sampler_table, Some(2));
    assert!(!sp.reflection.uses_resource_heap && !sp.reflection.uses_sampler_heap);
}
