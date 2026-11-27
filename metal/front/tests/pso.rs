//! Pipeline state parsing: the subobject stream of
//! `ID3D12Device2::CreatePipelineState` and the conversion to proto.
//!
//! Golden bytes (`UPDATE_GOLDEN=1 cargo test -p victory-metal-front --test pso`):
//!
//! * `pso-stream-vs-ps.txt`: a stream as the D3D12 helper types lay it out
//!   (type words at 8-byte boundaries, pointer-holding data at 8, the rest
//!   at 4), the offsets of which are also checked by hand below;
//! * `pso-desc-vs-ps.vmsa.txt`: the proto description it becomes.

#[path = "../../proto/tests/common/hexfile.rs"]
mod hexfile;

use victory_metal_front::hresult::Hr;
use victory_metal_front::mock::MockLink;
use victory_metal_front::pso::{self, subobject as t};
use victory_metal_proto::pso::*;
use victory_metal_proto::types::{CompareFunc, format};
use victory_metal_proto::wire::Wire;

fn golden_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

fn golden(name: &str, what: &str, bytes: &[u8]) -> Vec<u8> {
    let path = golden_path(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        hexfile::write(&path, what, bytes);
    }
    hexfile::read(&path)
}

/// Builds a stream the way the helper structs of `d3dx12` lay it out.
#[derive(Default)]
struct Stream(Vec<u8>);

impl Stream {
    fn add(&mut self, ty: u32, data_align: usize, data: &[u8]) -> usize {
        self.0.resize(self.0.len().next_multiple_of(8), 0);
        let at = self.0.len();
        self.0.extend_from_slice(&ty.to_le_bytes());
        self.0.resize(self.0.len().next_multiple_of(data_align), 0);
        self.0.extend_from_slice(data);
        at
    }
    fn u32s(vals: &[u32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn code(addr: u64, len: u64) -> Vec<u8> {
        [addr.to_le_bytes(), len.to_le_bytes()].concat()
    }
}

fn blend_bytes(alpha_to_coverage: u32, independent: u32, rt0: [u32; 9]) -> Vec<u8> {
    let mut v = Stream::u32s(&[alpha_to_coverage, independent]);
    for i in 0..8 {
        // BlendEnable, LogicOpEnable, Src, Dst, Op, SrcA, DstA, OpA, LogicOp
        let w = if i == 0 {
            rt0
        } else {
            [0, 0, 2, 1, 1, 2, 1, 1, 4]
        };
        v.extend(Stream::u32s(&w));
        v.extend([0xF, 0, 0, 0]); // RenderTargetWriteMask + padding
    }
    v
}

fn raster_bytes() -> Vec<u8> {
    let mut v = Stream::u32s(&[3, 1, 1]); // solid, none, front is CCW
    v.extend((-3i32).to_le_bytes()); // DepthBias
    v.extend(0.5f32.to_le_bytes());
    v.extend(1.25f32.to_le_bytes());
    v.extend(Stream::u32s(&[1, 0, 0, 0, 0])); // clip, multisample, aa line, forced, conservative
    v
}

fn depth_bytes(bounds: Option<u32>) -> Vec<u8> {
    // Enable, WriteMask all, GREATER_EQUAL, StencilEnable, masks, then faces.
    let mut v = Stream::u32s(&[1, 1, 7, 1]);
    v.extend([0x0F, 0xF0, 0, 0]);
    v.extend(Stream::u32s(&[1, 1, 3, 8])); // front: keep, keep, replace, always
    v.extend(Stream::u32s(&[1, 1, 4, 3])); // back: keep, keep, incr_sat, equal-ish (3 = EQUAL)
    if let Some(b) = bounds {
        v.extend(b.to_le_bytes());
    }
    v
}

const FAKE_ROOT: u64 = 0x1111_0000;
const FAKE_VS: (u64, u64) = (0x2000, 0x100);
const FAKE_PS: (u64, u64) = (0x3000, 0x80);
const FAKE_IL: u64 = 0x4000;

/// The vertex+pixel stream with fake pointers, and the offsets every
/// subobject is expected at (written out by hand from the layouts of the
/// structures: 8-byte type slots, data alignment 8 or 4, sizes 16, 24, 24,
/// 336, 8, 48, 64, 24, 8, 8, 40, 8, 16).
#[allow(clippy::vec_init_then_push)]
fn vs_ps_stream() -> (Vec<u8>, Vec<usize>) {
    let mut s = Stream::default();
    let mut at = Vec::new();
    at.push(s.add(t::ROOT_SIGNATURE, 8, &FAKE_ROOT.to_le_bytes()));
    at.push(s.add(t::VS, 8, &Stream::code(FAKE_VS.0, FAKE_VS.1)));
    at.push(s.add(t::PS, 8, &Stream::code(FAKE_PS.0, FAKE_PS.1)));
    // Blend: alpha blending on target 0.
    at.push(s.add(t::BLEND, 4, &blend_bytes(0, 0, [1, 0, 5, 6, 1, 2, 6, 1, 4])));
    at.push(s.add(t::SAMPLE_MASK, 4, &Stream::u32s(&[0xFFFF_FFFF])));
    at.push(s.add(t::RASTERIZER, 4, &raster_bytes()));
    at.push(s.add(t::DEPTH_STENCIL1, 4, &depth_bytes(Some(1))));
    let mut il = FAKE_IL.to_le_bytes().to_vec();
    il.extend(Stream::u32s(&[3, 0]));
    at.push(s.add(t::INPUT_LAYOUT, 8, &il));
    at.push(s.add(t::IB_STRIP_CUT_VALUE, 4, &Stream::u32s(&[1])));
    at.push(s.add(t::PRIMITIVE_TOPOLOGY, 4, &Stream::u32s(&[3])));
    let mut rt = [0u32; 9];
    rt[0] = 28; // R8G8B8A8_UNORM
    rt[1] = 42; // R32_UINT
    rt[8] = 2; // NumRenderTargets
    at.push(s.add(t::RENDER_TARGET_FORMATS, 4, &Stream::u32s(&rt)));
    at.push(s.add(t::DEPTH_STENCIL_FORMAT, 4, &Stream::u32s(&[40])));
    at.push(s.add(t::SAMPLE_DESC, 4, &Stream::u32s(&[4, 0])));
    // The stream's size is its last subobject rounded to the type alignment.
    s.0.resize(s.0.len().next_multiple_of(8), 0);
    (s.0, at)
}

#[test]
fn subobjects_sit_where_the_d3d12_structures_put_them() {
    let (bytes, at) = vs_ps_stream();
    assert_eq!(
        at,
        [0, 16, 40, 64, 400, 408, 456, 520, 544, 552, 560, 600, 608],
        "offsets of the type words"
    );
    assert_eq!(bytes.len(), 624);
    let g = golden(
        "pso-stream-vs-ps.txt",
        "the vertex+pixel pipeline state stream",
        &bytes,
    );
    assert_eq!(bytes, g, "the stream layout the test builds changed");
}

#[test]
fn a_vertex_pixel_stream_parses_to_the_state() {
    let (bytes, _) = vs_ps_stream();
    let s = pso::parse_stream(&bytes).unwrap();
    assert_eq!(s.root_sig, FAKE_ROOT);
    assert_eq!((s.vs.addr, s.vs.len), FAKE_VS);
    assert_eq!((s.ps.addr, s.ps.len), FAKE_PS);
    assert_eq!(s.gs, pso::Bytecode::default());
    let b = s.blend.rt[0];
    assert!(b.enable && !b.logic_enable);
    assert_eq!(
        (b.src, b.dst, b.op, b.src_alpha, b.dst_alpha),
        (5, 6, 1, 2, 6)
    );
    assert_eq!(b.write_mask, 0xF);
    assert_eq!(s.sample_mask, u32::MAX);
    assert_eq!((s.raster.fill, s.raster.cull), (3, 1));
    assert!(s.raster.front_ccw && s.raster.depth_clip);
    assert_eq!(
        (
            s.raster.depth_bias,
            s.raster.depth_bias_clamp,
            s.raster.slope_scaled_bias
        ),
        (-3, 0.5, 1.25)
    );
    assert!(s.depth.depth_enable && s.depth.stencil_enable && s.depth.depth_bounds);
    assert_eq!(
        (
            s.depth.depth_func,
            s.depth.stencil_read_mask,
            s.depth.stencil_write_mask
        ),
        (7, 0x0F, 0xF0)
    );
    assert_eq!(
        (s.depth.front.pass, s.depth.back.pass, s.depth.back.func),
        (3, 4, 3)
    );
    assert_eq!(
        (s.input_layout.elements, s.input_layout.count),
        (FAKE_IL, 3)
    );
    assert_eq!((s.strip_cut, s.topology), (1, 3));
    assert_eq!((s.rt_formats[0], s.rt_formats[1], s.rt_count), (28, 42, 2));
    assert_eq!(s.ds_format, 40);
    assert_eq!((s.sample_count, s.sample_quality), (4, 0));
}

#[test]
fn absent_subobjects_take_the_d3d12_defaults() {
    let mut s = Stream::default();
    s.add(t::ROOT_SIGNATURE, 8, &FAKE_ROOT.to_le_bytes());
    s.add(t::VS, 8, &Stream::code(0x2000, 8));
    let st = pso::parse_stream(&s.0).unwrap();
    assert_eq!(
        st,
        pso::PipelineState {
            root_sig: FAKE_ROOT,
            vs: pso::Bytecode {
                addr: 0x2000,
                len: 8
            },
            ..Default::default()
        }
    );
    let d = pso::to_proto(&st, &[]).unwrap();
    let mut want = GraphicsPsoDesc::new();
    want.vs = StageCode {
        addr: 0x2000,
        len: 8,
    };
    assert_eq!(d, want);
}

#[test]
fn a_mesh_stream_has_amplification_and_mesh_and_no_input_layout() {
    let mut s = Stream::default();
    s.add(t::ROOT_SIGNATURE, 8, &FAKE_ROOT.to_le_bytes());
    s.add(t::AS, 8, &Stream::code(0x2000, 8));
    s.add(t::MS, 8, &Stream::code(0x3000, 8));
    s.add(t::PS, 8, &Stream::code(0x4000, 8));
    s.add(t::RASTERIZER, 4, &raster_bytes());
    s.add(t::DEPTH_STENCIL, 4, &depth_bytes(None));
    let mut rt = [0u32; 9];
    rt[0] = 28;
    rt[8] = 1;
    s.add(t::RENDER_TARGET_FORMATS, 4, &Stream::u32s(&rt));
    s.add(t::DEPTH_STENCIL_FORMAT, 4, &Stream::u32s(&[40]));
    let st = pso::parse_stream(&s.0).unwrap();
    let d = pso::to_proto(&st, &[]).unwrap();
    assert_eq!(d.kind(), Some(PsoKind::Mesh));
    assert_eq!((d.amp.addr, d.ms.addr, d.ps.addr), (0x2000, 0x3000, 0x4000));
    assert_eq!(d.input_count, 0);
    assert!(!st.depth.depth_bounds);
}

#[test]
fn malformed_and_unsupported_streams_say_which() {
    let (bytes, _) = vs_ps_stream();
    // Cut inside a subobject.
    let e = pso::parse_stream(&bytes[..30]).unwrap_err();
    assert_eq!(e.hr, Hr::E_INVALIDARG);
    // A type nobody knows.
    let mut s = Stream::default();
    s.add(99, 4, &Stream::u32s(&[0]));
    assert_eq!(pso::parse_stream(&s.0).unwrap_err().hr, Hr::E_INVALIDARG);
    // The same subobject twice.
    let mut s = Stream::default();
    s.add(t::SAMPLE_MASK, 4, &Stream::u32s(&[1]));
    s.add(t::SAMPLE_MASK, 4, &Stream::u32s(&[2]));
    assert_eq!(pso::parse_stream(&s.0).unwrap_err().hr, Hr::E_INVALIDARG);
    // A compute shader.
    let mut s = Stream::default();
    s.add(t::CS, 8, &Stream::code(8, 8));
    assert_eq!(pso::parse_stream(&s.0).unwrap_err().hr, Hr::E_INVALIDARG);
    // Newer rasterizer/depth structures are not carried.
    let mut s = Stream::default();
    s.add(t::RASTERIZER2, 4, &Stream::u32s(&[0; 10]));
    assert_eq!(pso::parse_stream(&s.0).unwrap_err().hr, Hr::E_NOTIMPL);
    // A geometry shader is carried; a hull shader is refused.
    let mut s = Stream::default();
    s.add(t::VS, 8, &Stream::code(8, 8));
    s.add(t::GS, 8, &Stream::code(16, 8));
    let st = pso::parse_stream(&s.0).unwrap();
    assert_eq!(pso::to_proto(&st, &[]).unwrap().gs.addr, 16);
    let mut s = Stream::default();
    s.add(t::VS, 8, &Stream::code(8, 8));
    s.add(t::HS, 8, &Stream::code(16, 8));
    let st = pso::parse_stream(&s.0).unwrap();
    assert_eq!(pso::to_proto(&st, &[]).unwrap_err().hr, Hr::E_NOTIMPL);
    // Empty stream: parses (to nothing), no pipeline.
    let st = pso::parse_stream(&[]).unwrap();
    assert_eq!(pso::to_proto(&st, &[]).unwrap_err().hr, Hr::E_INVALIDARG);
}

/// `D3D12_INPUT_ELEMENT_DESC` array in memory, names alive as long as the vectors.
struct Layout {
    _names: Vec<std::ffi::CString>,
    elements: Vec<u8>,
}

fn layout(elements: &[(&str, u32, u32, u32, u32, u32, u32)]) -> Layout {
    let names: Vec<_> = elements
        .iter()
        .map(|e| std::ffi::CString::new(e.0).unwrap())
        .collect();
    let mut bytes = Vec::new();
    for (n, e) in names.iter().zip(elements) {
        bytes.extend((n.as_ptr() as u64).to_le_bytes());
        bytes.extend(Stream::u32s(&[e.1, e.2, e.3, e.4, e.5, e.6]));
    }
    Layout {
        _names: names,
        elements: bytes,
    }
}

#[test]
fn the_stream_becomes_the_proto_description() {
    let il = layout(&[
        ("POSITION", 0, 6, 0, 0, 0, 0),
        ("TEXCOORD", 0, 16, 0, 0xFFFF_FFFF, 0, 0),
        ("COLOR", 1, 28, 1, 0, 1, 1),
    ]);
    let (bytes, _) = vs_ps_stream();
    let mut st = pso::parse_stream(&bytes).unwrap();
    st.input_layout.elements = il.elements.as_ptr() as u64;
    // SAFETY: the layout's vectors are alive.
    let inputs = unsafe { pso::read_input_layout(st.input_layout) }.unwrap();
    assert_eq!(inputs.len(), 3);
    assert_eq!(inputs[1].semantic, "TEXCOORD");
    assert_eq!(inputs[2].class, 1);
    let d = pso::to_proto(&st, &inputs).unwrap();

    assert_eq!(d.kind(), Some(PsoKind::Vertex));
    let e = d.input_elements();
    assert_eq!(e.len(), 3);
    assert_eq!(
        (e[0].name(), e[0].format, e[0].slot, e[0].offset),
        ("POSITION", 6, 0, 0)
    );
    assert_eq!((e[1].name(), e[1].offset), ("TEXCOORD", APPEND_ALIGNED));
    assert_eq!(
        (e[2].name(), e[2].semantic_index, e[2].class, e[2].step_rate),
        ("COLOR", 1, InputClass::PER_INSTANCE, 1)
    );
    let b = d.blend.rt[0];
    assert_eq!(
        (
            b.enable,
            b.src,
            b.dst,
            b.op,
            b.src_alpha,
            b.dst_alpha,
            b.op_alpha
        ),
        (
            1,
            BlendFactor::SRC_ALPHA,
            BlendFactor::INV_SRC_ALPHA,
            BlendOp::ADD,
            BlendFactor::ONE,
            BlendFactor::INV_SRC_ALPHA,
            BlendOp::ADD
        )
    );
    assert_eq!(b.logic_op, LogicOp::NOOP);
    assert_eq!(
        (d.raster.fill, d.raster.cull, d.raster.front_ccw),
        (FillMode::SOLID, CullMode::NONE, 1)
    );
    assert_eq!(
        (
            d.raster.depth_bias,
            d.raster.depth_bias_clamp,
            d.raster.slope_scaled_bias
        ),
        (-3, 0.5, 1.25)
    );
    assert_eq!(d.depth.depth_func, CompareFunc::GREATER_EQUAL);
    assert_eq!(
        (
            d.depth.stencil_enable,
            d.depth.stencil_read_mask,
            d.depth.stencil_write_mask,
            d.depth.depth_bounds
        ),
        (1, 0x0F, 0xF0, 1)
    );
    assert_eq!(d.depth.front.pass, StencilOp::REPLACE);
    assert_eq!(
        (d.depth.back.pass, d.depth.back.func),
        (StencilOp::INCR_SAT, CompareFunc::EQUAL)
    );
    assert_eq!(
        (
            d.rt_count,
            d.rt_formats[0],
            d.rt_formats[1],
            d.rt_formats[2]
        ),
        (2, format::R8G8B8A8_UNORM, format::R32_UINT, 0)
    );
    assert_eq!(
        (d.ds_format, d.sample_count, d.sample_mask),
        (format::D32_FLOAT, 4, u32::MAX)
    );
    assert_eq!(
        (d.topology, d.strip_cut),
        (TopologyType::TRIANGLE, StripCut::INDEX_0XFFFF)
    );

    // Golden: the bytes that go to the back.
    let mut wire = Vec::new();
    d.put(&mut wire);
    let g = golden(
        "pso-desc-vs-ps.vmsa.txt",
        "the GraphicsPsoDesc of the vertex+pixel stream",
        &wire,
    );
    assert_eq!(wire, g, "the description the front sends changed");
}

#[test]
fn input_layouts_are_read_with_checks() {
    // SAFETY (all): the memory is alive; the failures are about the contents.
    assert!(
        unsafe { pso::read_input_layout(pso::InputLayoutRef::default()) }
            .unwrap()
            .is_empty()
    );
    let bad = pso::InputLayoutRef {
        elements: 0,
        count: 2,
    };
    assert_eq!(
        unsafe { pso::read_input_layout(bad) }.unwrap_err().hr,
        Hr::E_INVALIDARG
    );
    let il = layout(&[("A", 0, 6, 0, 0, 0, 0)]);
    let too_many = pso::InputLayoutRef {
        elements: il.elements.as_ptr() as u64,
        count: 33,
    };
    assert!(unsafe { pso::read_input_layout(too_many) }.is_err());
    let long = layout(&[("ABCDEFGHIJKLMNOPQRSTUVWXYZABCDEFGHI", 0, 6, 0, 0, 0, 0)]);
    let r = pso::InputLayoutRef {
        elements: long.elements.as_ptr() as u64,
        count: 1,
    };
    assert!(unsafe { pso::read_input_layout(r) }.is_err());
}

#[test]
fn the_front_sends_the_description_to_the_back() {
    use std::sync::Arc;
    use victory_metal_front::device::{Device, System};
    use victory_metal_front::rootsig::{self, RootSignatureDesc, Version};
    use victory_metal_proto::sync::SyncOp;

    let link = Arc::new(MockLink::new());
    let dev = Device::new(System::open(link.clone()).unwrap());
    let rs = dev
        .create_root_signature(
            &rootsig::serialize(&RootSignatureDesc {
                version: Version::V1_1,
                params: vec![],
                samplers: vec![],
                flags: 0,
            })
            .unwrap(),
        )
        .unwrap();
    let code = b"DXBC\0\0\0\0".to_vec();
    let mut d = GraphicsPsoDesc::new();
    d.vs = StageCode::of(&code);
    let p = dev.create_graphics_pipeline(&rs, &d).unwrap();
    assert_eq!(link.calls_of(SyncOp::CREATE_GRAPHICS_PIPELINE), 1);
    assert_eq!(link.live_pipelines(), 1);
    assert_eq!(link.graphics_desc(p.id).unwrap().vs, d.vs);
    // The back refuses what is not DXIL.
    let mut bad = d;
    bad.vs = StageCode::of(b"nope....");
    assert!(dev.create_graphics_pipeline(&rs, &bad).is_err());
    drop(p);
    assert_eq!(
        link.live_pipelines(),
        0,
        "dropping the pipeline destroys it"
    );
}
