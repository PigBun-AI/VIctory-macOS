//! Shared test data: one of every command, a synthetic compute stream, helpers
//! to build deliberately broken blocks.
#![allow(dead_code)]

use victory_metal_proto::cmd::*;
use victory_metal_proto::file::FileWriter;
use victory_metal_proto::ids::*;
use victory_metal_proto::indirect::ExecuteIndirect;
use victory_metal_proto::render::*;
use victory_metal_proto::root::*;
use victory_metal_proto::status::Status;
use victory_metal_proto::stream::{Encoder, STREAM_HEADER_SIZE, STREAM_MAGIC};
use victory_metal_proto::sync::*;
use victory_metal_proto::types::*;
use victory_metal_proto::wire::Wire;
use victory_metal_proto::{Caps, PROTO_VERSION};

pub const LABEL: &[u8] = b"frame 0: compute";
pub const CONSTANTS: [u8; 12] = [1, 0, 0, 0, 2, 0, 0, 0, 0xFE, 0xFF, 0xFF, 0xFF];
/// `[CONSTANT root 1 offset 0 x2, UAV root 3, DISPATCH]` encoded: 32 bytes of record.
pub const INDIRECT_SIGNATURE: [u8; 48] = {
    let mut v = [0u8; 48];
    let words: [u32; 12] = [5, 1, 0, 2, 8, 3, 0, 0, 2, 0, 0, 0];
    let mut i = 0;
    while i < 12 {
        let b = words[i].to_le_bytes();
        v[4 * i] = b[0];
        v[4 * i + 1] = b[1];
        v[4 * i + 2] = b[2];
        v[4 * i + 3] = b[3];
        i += 1;
    }
    v
};
/// `[DISPATCH]`.
pub const INDIRECT_PLAIN: [u8; 16] = [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
pub const BLOB: &[u8] = b"DXBC-not-really";
pub const DATA: &[u8] = &[9, 8, 7, 6, 5];

/// One of every command, with values that make fields distinguishable.
pub fn one_of_each() -> Vec<Command<'static>> {
    vec![
        Command::BeginList(BeginList {
            list: ListId(7),
            queue: QueueId(2),
            flags: 0,
            _pad: 0,
        }),
        Command::EndList(EndList {
            list: ListId(7),
            _pad: 0,
        }),
        Command::QueueSignal(QueueSignal {
            queue: QueueId(2),
            fence: FenceId(3),
            value: 0x1_0000_0001,
        }),
        Command::QueueWait(QueueWait {
            queue: QueueId(2),
            fence: FenceId(3),
            value: u64::MAX,
        }),
        Command::DebugLabel(LABEL),
        Command::DebugLabel(b""),
        Command::SetPipeline(SetPipeline {
            pso: PsoId(11),
            _pad: 0,
        }),
        Command::SetRootSignature(SetRootSignature {
            bind_point: BindPoint::COMPUTE,
            root_sig: RootSigId(5),
        }),
        Command::SetDescriptorHeaps(SetDescriptorHeaps {
            cbv_srv_uav: HeapId(1),
            sampler: HeapId(0),
        }),
        Command::SetRootConstants(SetRootConstants {
            bind_point: BindPoint::COMPUTE,
            root_index: 1,
            dest_offset: 4,
            data: &CONSTANTS,
        }),
        Command::SetRootConstants(SetRootConstants {
            bind_point: BindPoint::COMPUTE,
            root_index: 0,
            dest_offset: 0,
            data: &[],
        }),
        Command::SetRootDescriptor(SetRootDescriptor {
            bind_point: BindPoint::COMPUTE,
            root_index: 2,
            kind: RootParamKind::UAV,
            _pad: 0,
            gpu_va: 0x0000_7000_dead_beef,
        }),
        Command::SetRootTable(SetRootTable {
            bind_point: BindPoint::COMPUTE,
            root_index: 3,
            heap: HeapId(1),
            index: 16,
        }),
        Command::Dispatch(Dispatch {
            x: 64,
            y: 2,
            z: 1,
            _pad: 0,
        }),
        Command::DispatchIndirect(DispatchIndirect { args_va: 0x1000 }),
        Command::CopyBufferRegion(CopyBufferRegion {
            dst: ResId(4),
            src: ResId(3),
            dst_offset: 256,
            src_offset: 0,
            size: 4096,
        }),
        Command::FillBuffer(FillBuffer {
            dst: ResId(4),
            value: 0xAABBCCDD,
            offset: 8,
            size: 64,
        }),
        Command::ClearUav(ClearUav {
            res: ResId(4),
            format: format::R32_UINT,
            values: [1, 2, 3, 4],
            offset: 0,
            size: 16,
        }),
        Command::Barrier(Barrier::memory(stage::BLIT, stage::DISPATCH)),
        Command::Barrier(Barrier::aliasing(ResId(3), ResId(4))),
        Command::CopyTextureRegion(CopyTextureRegion {
            dst: TextureLocation {
                res: ResId(9),
                kind: LocKind::SUBRESOURCE,
                plane: 1,
                mip: 3,
                slice: 5,
                format: 0,
                offset: 0,
                row_pitch: 0,
                width: 0,
                height: 0,
                depth: 0,
            },
            src: TextureLocation {
                res: ResId(4),
                kind: LocKind::FOOTPRINT,
                plane: 0,
                mip: 0,
                slice: 0,
                format: format::R8G8B8A8_UNORM,
                offset: 0x1200,
                row_pitch: 512,
                width: 100,
                height: 37,
                depth: 1,
            },
            dst_x: 4,
            dst_y: 8,
            dst_z: 0,
            flags: CopyTextureRegion::HAS_BOX,
            box_left: 1,
            box_top: 2,
            box_front: 0,
            box_right: 50,
            box_bottom: 20,
            box_back: 1,
        }),
        Command::CopyResource(CopyResource {
            dst: ResId(9),
            src: ResId(8),
        }),
        Command::WriteImmediate(WriteImmediate {
            dst: ResId(9),
            size: 8,
            offset: 24,
            value: 0x0123_4567_89ab_cdef,
            mode: WriteImmediate::MARKER_OUT,
            _pad: 0,
        }),
        Command::ClearUavTexture(ClearUavTexture {
            res: ResId(9),
            format: format::R32_FLOAT,
            dimension: TexViewDim::TEX2D_ARRAY,
            float_values: 1,
            flags: ClearUavTexture::HAS_RECT,
            mip: 2,
            first_slice: 1,
            slice_count: 3,
            values: [0x3f80_0000, 0, 0, 0x3f80_0000],
            rect_left: 0,
            rect_top: 4,
            rect_right: 16,
            rect_bottom: 12,
            _pad: 0,
            _pad2: 0,
        }),
        Command::SetViewports(SetViewports::from_slice(&[
            Viewport::new(0.0, 1.0, 640.0, 360.0),
            Viewport {
                min_depth: 0.25,
                max_depth: 0.75,
                ..Viewport::new(2.0, 3.0, 4.0, 5.0)
            },
        ])),
        Command::SetScissors(SetScissors::from_slice(&[Rect::new(1, 2, 300, 200)])),
        Command::SetBlendFactor(SetBlendFactor {
            color: [0.1, 0.2, 0.3, 0.4],
        }),
        Command::SetStencilRef(SetStencilRef {
            value: 0x80,
            _pad: 0,
        }),
        Command::SetPrimitiveTopology(SetPrimitiveTopology {
            topology: Topology::TRIANGLE_STRIP,
            _pad: 0,
        }),
        Command::SetIndexBuffer(SetIndexBuffer {
            va: 0x7000_0000_1000,
            size: 4096,
            format: IndexFormat::U32,
        }),
        Command::SetVertexBuffers(SetVertexBuffers::from_slice(
            1,
            &[
                VertexBufferView {
                    va: 0x1000,
                    size: 96,
                    stride: 32,
                },
                VertexBufferView {
                    va: 0x2000,
                    size: 48,
                    stride: 16,
                },
            ],
        )),
        Command::Draw(Draw {
            vertex_count: 3,
            instance_count: 2,
            start_vertex: 1,
            start_instance: 5,
        }),
        Command::DrawIndexed(DrawIndexed {
            index_count: 6,
            instance_count: 1,
            start_index: 3,
            base_vertex: -2,
            start_instance: 0,
            _pad: 0,
        }),
        Command::DispatchMesh(DispatchMesh {
            x: 4,
            y: 3,
            z: 2,
            _pad: 0,
        }),
        Command::ClearAttachment(sample_clear_attachment()),
        Command::BeginRenderPass(sample_begin_pass()),
        Command::EndRenderPass(sample_end_pass()),
        Command::ExecuteIndirect(ExecuteIndirect {
            args: ResId(5),
            args_offset: 64,
            count: ResId(6),
            count_offset: 4,
            max_count: 128,
            stride: 32,
            root_sig: RootSigId(2),
            signature: &INDIRECT_SIGNATURE,
        }),
        Command::ExecuteIndirect(ExecuteIndirect {
            args: ResId(5),
            args_offset: 0,
            count: ResId::NULL,
            count_offset: 0,
            max_count: 1,
            stride: 12,
            root_sig: RootSigId::NULL,
            signature: &INDIRECT_PLAIN,
        }),
        Command::SyncRecord(SyncRecord {
            op: SyncOp::CREATE_COMPUTE_PIPELINE,
            status: Status::OK,
            args: &[0xAB; 24],
            blob: BLOB,
        }),
        Command::SyncRecord(SyncRecord {
            op: SyncOp::NOP,
            status: Status::UNSUPPORTED,
            args: &[],
            blob: &[],
        }),
        Command::BufferData(BufferData {
            res: ResId(3),
            offset: 12,
            data: DATA,
        }),
    ]
}

pub fn encode(cmds: &[Command<'_>]) -> Vec<u8> {
    let mut e = Encoder::new(Caps::NONE);
    for c in cmds {
        e.push(c).unwrap();
    }
    e.finish()
}

/// The synthetic compute workload used for the golden files:
/// copy upload -> default, barrier, dispatch with a root table, barrier, copy back.
pub fn synthetic_compute() -> Vec<u8> {
    let mut e = Encoder::new(Caps::METAL4 | Caps::UNIFIED_MEMORY);
    let q = QueueId(1);
    let list = ListId(1);
    let cp = BindPoint::COMPUTE;
    let push = |e: &mut Encoder, c: Command<'_>| e.push(&c).unwrap();
    push(
        &mut e,
        Command::QueueWait(QueueWait {
            queue: q,
            fence: FenceId(1),
            value: 1,
        }),
    );
    push(
        &mut e,
        Command::BeginList(BeginList {
            list,
            queue: q,
            flags: 0,
            _pad: 0,
        }),
    );
    push(&mut e, Command::DebugLabel(b"synthetic compute"));
    push(
        &mut e,
        Command::CopyBufferRegion(CopyBufferRegion {
            dst: ResId(2),
            src: ResId(1),
            dst_offset: 0,
            src_offset: 0,
            size: 4096,
        }),
    );
    push(
        &mut e,
        Command::Barrier(Barrier::memory(stage::BLIT, stage::DISPATCH)),
    );
    push(
        &mut e,
        Command::SetRootSignature(SetRootSignature {
            bind_point: cp,
            root_sig: RootSigId(1),
        }),
    );
    push(
        &mut e,
        Command::SetPipeline(SetPipeline {
            pso: PsoId(1),
            _pad: 0,
        }),
    );
    push(
        &mut e,
        Command::SetDescriptorHeaps(SetDescriptorHeaps {
            cbv_srv_uav: HeapId(1),
            sampler: HeapId(0),
        }),
    );
    e.set_root_constants(cp, 0, 0, &[1024, 0x3f80_0000])
        .unwrap();
    push(
        &mut e,
        Command::SetRootDescriptor(SetRootDescriptor {
            bind_point: cp,
            root_index: 1,
            kind: RootParamKind::CBV,
            _pad: 0,
            gpu_va: 0x0000_0001_0000_4000,
        }),
    );
    push(
        &mut e,
        Command::SetRootTable(SetRootTable {
            bind_point: cp,
            root_index: 2,
            heap: HeapId(1),
            index: 0,
        }),
    );
    push(
        &mut e,
        Command::Dispatch(Dispatch {
            x: 16,
            y: 1,
            z: 1,
            _pad: 0,
        }),
    );
    push(
        &mut e,
        Command::Barrier(Barrier::memory(stage::DISPATCH, stage::BLIT)),
    );
    push(
        &mut e,
        Command::CopyBufferRegion(CopyBufferRegion {
            dst: ResId(3),
            src: ResId(2),
            dst_offset: 0,
            src_offset: 0,
            size: 4096,
        }),
    );
    push(&mut e, Command::EndList(EndList { list, _pad: 0 }));
    push(
        &mut e,
        Command::QueueSignal(QueueSignal {
            queue: q,
            fence: FenceId(1),
            value: 2,
        }),
    );
    e.finish()
}

/// A root signature: one constant pair, one root CBV, a table with an SRV
/// range and a UAV range, one static sampler.
pub fn synthetic_rootsig() -> RootSigDesc {
    RootSigDesc {
        flags: root_flags::CBV_SRV_UAV_HEAP_INDEXED,
        params: vec![
            RootParam {
                kind: RootParamKind::CONSTANTS,
                visibility: ShaderVisibility::ALL,
                shader_register: 0,
                register_space: 0,
                num_32bit_values: 2,
                ..RootParam::default()
            },
            RootParam {
                kind: RootParamKind::CBV,
                visibility: ShaderVisibility::ALL,
                shader_register: 1,
                register_space: 0,
                ..RootParam::default()
            },
            RootParam {
                kind: RootParamKind::TABLE,
                visibility: ShaderVisibility::ALL,
                range_start: 0,
                range_count: 2,
                ..RootParam::default()
            },
        ],
        ranges: vec![
            DescriptorRange {
                kind: RangeKind::SRV,
                num_descriptors: 4,
                base_register: 0,
                register_space: 0,
                offset_in_table: 0,
                flags: 0,
            },
            DescriptorRange {
                kind: RangeKind::UAV,
                num_descriptors: UNBOUNDED,
                base_register: 0,
                register_space: 1,
                offset_in_table: APPEND,
                flags: 0,
            },
        ],
        static_samplers: vec![StaticSampler {
            sampler: SamplerDesc {
                min_filter: Filter::LINEAR,
                mag_filter: Filter::LINEAR,
                mip_filter: Filter::LINEAR,
                address_u: AddressMode::CLAMP,
                address_v: AddressMode::CLAMP,
                address_w: AddressMode::CLAMP,
                compare: CompareFunc::NEVER,
                max_lod: 1000.0,
                max_anisotropy: 1,
                ..SamplerDesc::default()
            },
            shader_register: 0,
            register_space: 0,
            visibility: ShaderVisibility::ALL,
            _pad: 0,
        }],
    }
}

/// A sync call with a recorded argument block: what a recorder writes.
pub fn recorded_create_buffer() -> CreateBufferArgs {
    let mut a = CreateBufferArgs::new();
    a.size = 4096;
    a.kind = BufferKind::UPLOAD.0;
    a.res = ResId(1);
    a.gpu_va = 0x0000_0001_0000_0000;
    a.cpu_addr = 0x0000_0002_0000_0000;
    a.alloc_size = 16384;
    a
}

pub fn wire_bytes<T: Wire>(v: &T) -> Vec<u8> {
    let mut out = Vec::new();
    v.put(&mut out);
    out
}

/// The synthetic dump file: a recorded sync call and a buffer fill in a first
/// block, the compute stream in a second.
pub fn synthetic_dump() -> Vec<u8> {
    let mut w = FileWriter::new("proto tests");
    let create = recorded_create_buffer();
    let create_bytes = wire_bytes(&create);
    let rs = synthetic_rootsig().encode().unwrap();
    let mut setup = Encoder::new(Caps::NONE);
    setup
        .push(&Command::SyncRecord(SyncRecord {
            op: SyncOp::CREATE_BUFFER,
            status: Status::OK,
            args: &create_bytes,
            blob: &[],
        }))
        .unwrap();
    setup
        .push(&Command::BufferData(BufferData {
            res: ResId(1),
            offset: 0,
            data: &[1, 2, 3, 4, 5, 6, 7, 8],
        }))
        .unwrap();
    setup
        .push(&Command::SyncRecord(SyncRecord {
            op: SyncOp::CREATE_ROOT_SIGNATURE,
            status: Status::OK,
            args: &[0u8; 32],
            blob: &rs,
        }))
        .unwrap();
    w.push_block(&setup.finish()).unwrap();
    w.push_block(&synthetic_compute()).unwrap();
    w.finish()
}

// ---- broken blocks -------------------------------------------------------

/// A block header for `count` commands over `body` bytes, honest otherwise.
pub fn block_with(count: u32, body: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&STREAM_MAGIC);
    v.extend_from_slice(&PROTO_VERSION.to_le_bytes());
    v.extend_from_slice(&(STREAM_HEADER_SIZE as u16).to_le_bytes());
    v.extend_from_slice(&0u16.to_le_bytes());
    v.extend_from_slice(&0u64.to_le_bytes());
    v.extend_from_slice(&((STREAM_HEADER_SIZE + body.len()) as u32).to_le_bytes());
    v.extend_from_slice(&count.to_le_bytes());
    v.extend_from_slice(body);
    v
}

/// One raw command: header fields given explicitly, payload as is.
pub fn raw_cmd(opcode: u16, version: u8, flags: u8, len: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&opcode.to_le_bytes());
    v.push(version);
    v.push(flags);
    v.extend_from_slice(&len.to_le_bytes());
    v.extend_from_slice(payload);
    v
}

/// A pass with two colour targets (one cleared, one loaded) and a depth-stencil.
pub fn sample_begin_pass() -> BeginRenderPass {
    let mut b = <BeginRenderPass as Wire>::ZERO;
    b.rt_count = 2;
    b.rt[0] = RenderAttachment {
        res: ResId(4),
        format: format::R8G8B8A8_UNORM,
        dimension: TexViewDim::TEX2D,
        mip: 0,
        load: LoadAction::CLEAR,
        _pad: 0,
        first_slice: 0,
        slice_count: 1,
        clear: [0.0, 0.5, 1.0, 1.0],
    };
    b.rt[1] = RenderAttachment {
        res: ResId(5),
        format: format::R32_FLOAT,
        dimension: TexViewDim::TEX2D_ARRAY,
        mip: 1,
        load: LoadAction::LOAD,
        _pad: 0,
        first_slice: 2,
        slice_count: 3,
        clear: [0.0; 4],
    };
    b.ds = DepthAttachment {
        res: ResId(6),
        format: format::D32_FLOAT,
        dimension: TexViewDim::TEX2D,
        mip: 0,
        depth_load: LoadAction::CLEAR,
        stencil_load: LoadAction::DONT_CARE,
        first_slice: 0,
        slice_count: 1,
        clear_depth: 1.0,
        clear_stencil: 0,
        _pad: 0,
    };
    b
}

pub fn sample_end_pass() -> EndRenderPass {
    let mut e = <EndRenderPass as Wire>::ZERO;
    e.rt_store[1] = StoreAction::DONT_CARE;
    e.depth_store = StoreAction::DONT_CARE;
    e
}

pub fn sample_clear_attachment() -> ClearAttachment {
    let mut c = <ClearAttachment as Wire>::ZERO;
    c.target = CLEAR_TARGET_DEPTH_STENCIL;
    c.flags = CLEAR_DEPTH | CLEAR_STENCIL;
    c.rect_count = 2;
    c.rect[0] = Rect::new(0, 0, 16, 16);
    c.rect[1] = Rect::new(32, 8, 64, 24);
    c.clear_depth = 0.5;
    c.clear_stencil = 7;
    c
}
