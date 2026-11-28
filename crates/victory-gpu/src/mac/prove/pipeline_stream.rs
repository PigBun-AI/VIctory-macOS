//! C-V-009 (docs/mac/PATCHES.md G12): the graphics pipeline state stream the title hands to the device, as the host
//! pipeline description reads it.
//!
//! Reference: `d3d12.h` (Windows SDK 10.0.26100): `D3D12_PIPELINE_STATE_SUBOBJECT_TYPE` (ROOT_SIGNATURE 0, VS 1, PS 2, DS 3,
//! HS 4, GS 5, CS 6, STREAM_OUTPUT 7, BLEND 8, SAMPLE_MASK 9, RASTERIZER 10, DEPTH_STENCIL 11, INPUT_LAYOUT 12,
//! IB_STRIP_CUT_VALUE 13, PRIMITIVE_TOPOLOGY 14, RENDER_TARGET_FORMATS 15, DEPTH_STENCIL_FORMAT 16, SAMPLE_DESC 17,
//! NODE_MASK 18, CACHED_PSO 19, FLAGS 20, DEPTH_STENCIL1 21, VIEW_INSTANCING 22; the console numbers AS/MS 23/24, the PC
//! 24/25) and the layout `CD3DX12_PIPELINE_STATE_STREAM_SUBOBJECT<T>` gives a subobject: the type (4 bytes), padding to
//! T's alignment, T, the whole padded to 8 (the alignment of a pointer).
use std::ffi::CString;
use std::mem::{align_of, size_of};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::PCSTR;

use super::{subobject, walk_stream};

/// Size of a subobject carrying a `T`, by the C++ layout.
fn spec_size<T>() -> usize {
    (4usize.next_multiple_of(align_of::<T>()) + size_of::<T>()).next_multiple_of(8)
}

#[test]
fn the_size_of_every_subobject_is_the_one_the_headers_lay_out() {
    let sizes: [(u32, usize); 22] = [
        (0, spec_size::<usize>()),                                  // root signature
        (1, spec_size::<D3D12_SHADER_BYTECODE>()),                  // VS
        (2, spec_size::<D3D12_SHADER_BYTECODE>()),                  // PS
        (3, spec_size::<D3D12_SHADER_BYTECODE>()),                  // DS
        (4, spec_size::<D3D12_SHADER_BYTECODE>()),                  // HS
        (5, spec_size::<D3D12_SHADER_BYTECODE>()),                  // GS
        (6, spec_size::<D3D12_SHADER_BYTECODE>()),                  // CS
        (7, spec_size::<D3D12_STREAM_OUTPUT_DESC>()),               // stream output
        (8, spec_size::<D3D12_BLEND_DESC>()),                       // blend
        (9, spec_size::<u32>()),                                    // sample mask
        (10, spec_size::<D3D12_RASTERIZER_DESC>()),                 // rasterizer
        (11, spec_size::<D3D12_DEPTH_STENCIL_DESC>()),              // depth stencil
        (12, spec_size::<D3D12_INPUT_LAYOUT_DESC>()),               // input layout
        (13, spec_size::<D3D12_INDEX_BUFFER_STRIP_CUT_VALUE>()),    // strip cut
        (14, spec_size::<D3D12_PRIMITIVE_TOPOLOGY_TYPE>()),         // topology
        (15, spec_size::<D3D12_RT_FORMAT_ARRAY>()),                 // render target formats
        (16, spec_size::<DXGI_FORMAT>()),                           // depth stencil format
        (17, spec_size::<DXGI_SAMPLE_DESC>()),                      // sample desc
        (18, spec_size::<u32>()),                                   // node mask
        (19, spec_size::<D3D12_CACHED_PIPELINE_STATE>()),           // cached PSO
        (21, spec_size::<D3D12_DEPTH_STENCIL_DESC1>()),             // depth stencil 1
        (22, spec_size::<D3D12_VIEW_INSTANCING_DESC>()),            // view instancing
    ];
    for (kind, want) in sizes {
        assert_eq!(subobject(kind).map(|s| s.0), Some(want), "subobject type {kind}");
    }
    assert_eq!(subobject(20).map(|s| s.0), Some(spec_size::<D3D12_PIPELINE_STATE_FLAGS>()), "flags");
    // the shaders by type: the stage names, the console's numbers for amplification and mesh
    let names: Vec<_> = (0..=24).map(|k| subobject(k).and_then(|s| s.1)).collect();
    assert_eq!(&names[1..=6], &[Some("vertex"), Some("pixel"), Some("domain"), Some("hull"), Some("geometry"), Some("compute")]);
    assert_eq!((names[23], names[24]), (Some("amplification"), Some("mesh")));
    assert_eq!(subobject(25), None, "a type of unknown size stops the walk");
}

/// One subobject: the type, T at its alignment, padded to 8.
fn push<T: Copy>(stream: &mut Vec<u8>, kind: u32, value: T) {
    let start = stream.len();
    assert_eq!(start % 8, 0);
    stream.extend(kind.to_le_bytes());
    stream.resize(start + 4usize.next_multiple_of(align_of::<T>()), 0);
    // SAFETY: a plain C struct, copied as bytes.
    stream.extend(unsafe { std::slice::from_raw_parts((&raw const value).cast::<u8>(), size_of::<T>()) });
    stream.resize(start + spec_size::<T>(), 0);
}

#[test]
fn a_stream_is_read_into_the_graphics_state_field_by_field() {
    let mut blend = D3D12_BLEND_DESC { AlphaToCoverageEnable: true.into(), IndependentBlendEnable: true.into(), ..Default::default() };
    blend.RenderTarget[0] = D3D12_RENDER_TARGET_BLEND_DESC {
        BlendEnable: true.into(),
        SrcBlend: D3D12_BLEND_SRC_ALPHA,
        DestBlend: D3D12_BLEND_INV_SRC_ALPHA,
        BlendOp: D3D12_BLEND_OP_ADD,
        SrcBlendAlpha: D3D12_BLEND_ONE,
        DestBlendAlpha: D3D12_BLEND_ZERO,
        BlendOpAlpha: D3D12_BLEND_OP_MAX,
        RenderTargetWriteMask: 0b0111,
        ..Default::default()
    };
    let raster = D3D12_RASTERIZER_DESC {
        FillMode: D3D12_FILL_MODE_WIREFRAME,
        CullMode: D3D12_CULL_MODE_FRONT,
        FrontCounterClockwise: true.into(),
        DepthBias: -5,
        DepthBiasClamp: 0.25,
        SlopeScaledDepthBias: 1.5,
        DepthClipEnable: false.into(),
        ForcedSampleCount: 4,
        ..Default::default()
    };
    let op = D3D12_DEPTH_STENCILOP_DESC {
        StencilFailOp: D3D12_STENCIL_OP_REPLACE,
        StencilDepthFailOp: D3D12_STENCIL_OP_INCR,
        StencilPassOp: D3D12_STENCIL_OP_DECR_SAT,
        StencilFunc: D3D12_COMPARISON_FUNC_NOT_EQUAL,
    };
    let depth1 = D3D12_DEPTH_STENCIL_DESC1 {
        DepthEnable: true.into(),
        DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ZERO,
        DepthFunc: D3D12_COMPARISON_FUNC_GREATER_EQUAL,
        StencilEnable: true.into(),
        StencilReadMask: 0x0f,
        StencilWriteMask: 0xf0,
        FrontFace: op,
        BackFace: op,
        DepthBoundsTestEnable: true.into(),
    };
    let names = [CString::new("POSITION").unwrap(), CString::new("TEXCOORD").unwrap()];
    let elements = [
        D3D12_INPUT_ELEMENT_DESC { SemanticName: PCSTR(names[0].as_ptr().cast()), SemanticIndex: 0, Format: DXGI_FORMAT_R32G32B32_FLOAT, InputSlot: 0, AlignedByteOffset: 0, InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, InstanceDataStepRate: 0 },
        D3D12_INPUT_ELEMENT_DESC { SemanticName: PCSTR(names[1].as_ptr().cast()), SemanticIndex: 1, Format: DXGI_FORMAT_R16G16_FLOAT, InputSlot: 1, AlignedByteOffset: D3D12_APPEND_ALIGNED_ELEMENT, InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA, InstanceDataStepRate: 1 },
    ];
    let layout = D3D12_INPUT_LAYOUT_DESC { pInputElementDescs: elements.as_ptr(), NumElements: 2 };
    let mut targets = D3D12_RT_FORMAT_ARRAY { NumRenderTargets: 2, ..Default::default() };
    targets.RTFormats[0] = DXGI_FORMAT_R8G8B8A8_UNORM;
    targets.RTFormats[1] = DXGI_FORMAT_R16G16B16A16_FLOAT;
    let code = [1u8, 2, 3, 4];
    let bytecode = D3D12_SHADER_BYTECODE { pShaderBytecode: code.as_ptr().cast(), BytecodeLength: 4 };

    let mut stream = Vec::new();
    push(&mut stream, 0, 0usize); // root signature: skipped
    push(&mut stream, 1, bytecode); // VS
    push(&mut stream, 8, blend);
    push(&mut stream, 9, 0xA5A5_0F0Fu32);
    push(&mut stream, 10, raster);
    push(&mut stream, 21, depth1);
    push(&mut stream, 12, layout);
    push(&mut stream, 13, D3D12_INDEX_BUFFER_STRIP_CUT_VALUE_0xFFFF);
    push(&mut stream, 14, D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE);
    push(&mut stream, 18, 1u32); // node mask: skipped
    push(&mut stream, 15, targets);
    push(&mut stream, 16, DXGI_FORMAT_D32_FLOAT_S8X24_UINT);
    push(&mut stream, 20, 0u32); // flags: skipped
    push(&mut stream, 17, DXGI_SAMPLE_DESC { Count: 4, Quality: 0 });
    push(&mut stream, 2, bytecode); // PS, after everything
    let mut stages = Vec::new();
    let mut g = super::host::GraphicsState::default();
    walk_stream(stream.len(), stream.as_ptr(), |kind, whole, stage| {
        stages.extend(stage);
        // SAFETY: a whole subobject of this type, built above.
        unsafe { g.read(kind, whole) };
    });
    assert_eq!(stages, ["vertex", "pixel"], "both shaders are found, with the state between them walked over");
    assert_eq!(g.blend.AlphaToCoverageEnable.as_bool(), true);
    assert_eq!(g.blend.IndependentBlendEnable.as_bool(), true);
    let rt = &g.blend.RenderTarget[0];
    assert_eq!((rt.BlendEnable.as_bool(), rt.SrcBlend, rt.DestBlend, rt.BlendOpAlpha, rt.RenderTargetWriteMask), (true, D3D12_BLEND_SRC_ALPHA, D3D12_BLEND_INV_SRC_ALPHA, D3D12_BLEND_OP_MAX, 7));
    assert_eq!(g.sample_mask, 0xA5A5_0F0F);
    let r = &g.rasterizer;
    assert_eq!((r.FillMode, r.CullMode, r.FrontCounterClockwise.as_bool(), r.DepthBias, r.DepthBiasClamp, r.SlopeScaledDepthBias, r.DepthClipEnable.as_bool(), r.ForcedSampleCount), (D3D12_FILL_MODE_WIREFRAME, D3D12_CULL_MODE_FRONT, true, -5, 0.25, 1.5, false, 4));
    let d = &g.depth;
    assert_eq!((d.DepthEnable.as_bool(), d.DepthWriteMask, d.DepthFunc, d.StencilEnable.as_bool(), d.StencilReadMask, d.StencilWriteMask), (true, D3D12_DEPTH_WRITE_MASK_ZERO, D3D12_COMPARISON_FUNC_GREATER_EQUAL, true, 0x0f, 0xf0));
    assert_eq!((d.FrontFace.StencilFailOp, d.FrontFace.StencilDepthFailOp, d.FrontFace.StencilPassOp, d.FrontFace.StencilFunc), (op.StencilFailOp, op.StencilDepthFailOp, op.StencilPassOp, op.StencilFunc));
    assert_eq!(d.BackFace.StencilFunc, op.StencilFunc);
    assert!(g.depth_bounds, "DEPTH_STENCIL1 carries the depth bounds test");
    assert_eq!(g.inputs.len(), 2);
    assert_eq!(g.names[0].to_str().unwrap(), "POSITION");
    assert_eq!(g.inputs[1].SemanticIndex, 1);
    assert_eq!((g.inputs[1].InputSlot, g.inputs[1].InputSlotClass, g.inputs[1].InstanceDataStepRate), (1, D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA, 1));
    assert_eq!(g.strip_cut, D3D12_INDEX_BUFFER_STRIP_CUT_VALUE_0xFFFF);
    assert_eq!(g.topology, D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE);
    assert_eq!(g.targets.NumRenderTargets, 2);
    assert_eq!(g.targets.RTFormats[0], DXGI_FORMAT_R8G8B8A8_UNORM);
    assert_eq!(g.targets.RTFormats[1], DXGI_FORMAT_R16G16B16A16_FLOAT);
    assert_eq!(g.depth_format, DXGI_FORMAT_D32_FLOAT_S8X24_UINT);
    assert_eq!((g.samples.Count, g.samples.Quality), (4, 0));
}

/// What a stream leaves out is D3D12's default state (the spec's table of defaults: no blending, all channels written,
/// solid fill, back-face culling, depth test LESS with writes, sample mask all ones, one sample).
#[test]
fn what_the_stream_leaves_out_is_the_d3d12_default() {
    let g = super::host::GraphicsState::default();
    assert_eq!(g.sample_mask, u32::MAX);
    assert_eq!((g.rasterizer.FillMode, g.rasterizer.CullMode, g.rasterizer.DepthClipEnable.as_bool()), (D3D12_FILL_MODE_SOLID, D3D12_CULL_MODE_BACK, true));
    assert!(!g.blend.RenderTarget[0].BlendEnable.as_bool());
    assert_eq!(g.blend.RenderTarget[0].RenderTargetWriteMask, 0xf);
    assert_eq!((g.depth.DepthEnable.as_bool(), g.depth.DepthFunc, g.depth.DepthWriteMask), (true, D3D12_COMPARISON_FUNC_LESS, D3D12_DEPTH_WRITE_MASK_ALL));
    assert!(!g.depth.StencilEnable.as_bool());
    assert_eq!((g.samples.Count, g.samples.Quality), (1, 0));
    assert_eq!(g.topology, D3D12_PRIMITIVE_TOPOLOGY_TYPE_UNDEFINED);
}
