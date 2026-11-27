//! The command stream's commands: opcodes, payloads, and the codec of one command.
//!
//! A command is an 8-byte header (`opcode: u16`, `version: u8`, `flags: u8`
//! (0), `len: u32`) and `len` bytes of payload; `len` is a multiple of 8.
//! A fixed payload is a `repr(C)` POD struct; a variable one is a fixed head
//! followed by bytes, zero-padded to a multiple of 8. Offsets inside a stream
//! are multiples of 8, so a reader with an 8-aligned buffer may also view a
//! payload in place; this decoder does not need that and works on any slice.

use crate::ids::*;
use crate::render::*;
use crate::status::{Error, Status};
use crate::sync::SyncOp;
use crate::types::*;
use crate::wire::{Reader, Wire, align8, open_enum, pad_to_8, wire_struct};
use alloc::vec::Vec;

open_enum! {
    /// Opcode of a command. Ranges: 0x0001 framing and queue ops, 0x0100 state,
    /// 0x0200 compute/copy work, 0x0300 barriers, 0x0400-0x04FF texture work;
    /// 0x0501-0x0601 render state, draws and passes (m4-render); the rest of 0x0500-0x09FF reserved for M2/M3 (see [`reserved_range`]); 0x0F00
    /// replay-only records; 0x8000 and up are never assigned.
    pub struct Opcode(u16) {
        BEGIN_LIST = 0x0001,
        END_LIST = 0x0002,
        QUEUE_SIGNAL = 0x0003,
        QUEUE_WAIT = 0x0004,
        DEBUG_LABEL = 0x0005,

        SET_PIPELINE = 0x0100,
        SET_ROOT_SIGNATURE = 0x0101,
        SET_DESCRIPTOR_HEAPS = 0x0102,
        SET_ROOT_CONSTANTS = 0x0103,
        SET_ROOT_DESCRIPTOR = 0x0104,
        SET_ROOT_TABLE = 0x0105,

        DISPATCH = 0x0200,
        DISPATCH_INDIRECT = 0x0201,
        COPY_BUFFER_REGION = 0x0210,
        FILL_BUFFER = 0x0211,
        CLEAR_UAV = 0x0212,

        BARRIER = 0x0300,

        COPY_TEXTURE_REGION = 0x0400,
        COPY_RESOURCE = 0x0401,
        CLEAR_UAV_TEXTURE = 0x0402,

        SET_VIEWPORTS = 0x0501,
        SET_SCISSORS = 0x0502,
        SET_BLEND_FACTOR = 0x0503,
        SET_STENCIL_REF = 0x0504,
        SET_PRIMITIVE_TOPOLOGY = 0x0505,
        SET_INDEX_BUFFER = 0x0506,
        SET_VERTEX_BUFFERS = 0x0507,
        DRAW = 0x0510,
        DRAW_INDEXED = 0x0511,
        DISPATCH_MESH = 0x0512,
        CLEAR_ATTACHMENT = 0x0520,
        BEGIN_RENDER_PASS = 0x0600,
        END_RENDER_PASS = 0x0601,

        SYNC_RECORD = 0x0F00,
        BUFFER_DATA = 0x0F01,
    }
}

/// Opcode ranges reserved for later milestones: `(first, last, what)`.
/// Opcodes in them are refused with `Error::ReservedOpcode`.
pub const RESERVED_RANGES: &[(u16, u16, &str)] = &[
    (0x0403, 0x04FF, "textures: numbers not yet used"),
    (
        0x0500,
        0x05FF,
        "render state, draws, mesh dispatch, clears in a pass (m4-render; numbers not yet used)",
    ),
    (
        0x0602,
        0x06FF,
        "render passes and render-encoder bindings (m4-render; numbers not yet used)",
    ),
    (0x0700, 0x07FF, "present (M3)"),
    (0x0800, 0x08FF, "queries and timestamps (M2)"),
    (0x0900, 0x09FF, "ExecuteIndirect (M2)"),
];

/// The reserved range an opcode number falls in, if any.
pub fn reserved_range(code: u16) -> Option<&'static str> {
    RESERVED_RANGES
        .iter()
        .find(|(a, b, _)| (*a..=*b).contains(&code))
        .map(|r| r.2)
}

/// Bytes of a command header.
pub const CMD_HEADER_SIZE: usize = 8;
/// Largest payload of any command.
pub const MAX_PAYLOAD: usize = 1 << 24;
/// Largest `SET_ROOT_CONSTANTS` run, in 32-bit values.
pub const MAX_ROOT_CONSTANTS: usize = 64;
/// Largest `DEBUG_LABEL`, in bytes.
pub const MAX_LABEL: usize = 1024;

/// The version of an opcode this proto writes and reads.
pub const fn opcode_version(op: Opcode) -> u8 {
    match op.0 {
        0x0001..=0x0005 | 0x0100..=0x0105 | 0x0200 | 0x0201 | 0x0210..=0x0212 | 0x0300 => 1,
        0x0400..=0x0402 => 1,
        0x0501..=0x0507 | 0x0510..=0x0512 | 0x0520 | 0x0600 | 0x0601 => 1,
        0x0F00 | 0x0F01 => 1,
        _ => 0,
    }
}

wire_struct! {
    /// Fixed head of `DEBUG_LABEL`; `len` bytes of text follow.
    pub struct DebugLabelHead {
        len: u32,
        _pad: u32,
    }
}

wire_struct! {
    /// Starts a command list on a queue. Lists in one block run in order.
    pub struct BeginList {
        /// Front-chosen; unique within the block.
        list: ListId,
        queue: QueueId,
        flags: u32,
        _pad: u32,
    }
}

wire_struct! {
    pub struct EndList {
        list: ListId,
        _pad: u32,
    }
}

wire_struct! {
    /// The queue signals `value` on `fence` once everything before this point has completed.
    pub struct QueueSignal {
        queue: QueueId,
        fence: FenceId,
        value: u64,
    }
}

wire_struct! {
    /// The queue waits for `fence` to reach `value` before what follows.
    pub struct QueueWait {
        queue: QueueId,
        fence: FenceId,
        value: u64,
    }
}

wire_struct! {
    pub struct SetPipeline {
        pso: PsoId,
        _pad: u32,
    }
}

wire_struct! {
    pub struct SetRootSignature {
        bind_point: BindPoint,
        root_sig: RootSigId,
    }
}

wire_struct! {
    /// Which heaps shaders reach through root tables; null = none.
    pub struct SetDescriptorHeaps {
        cbv_srv_uav: HeapId,
        sampler: HeapId,
    }
}

wire_struct! {
    /// Fixed head of `SET_ROOT_CONSTANTS`; `count` 32-bit values follow.
    pub struct RootConstantsHead {
        bind_point: BindPoint,
        root_index: u32,
        /// Offset in 32-bit values inside the root parameter.
        dest_offset: u32,
        count: u32,
    }
}

wire_struct! {
    /// A root CBV/SRV/UAV: the GPU address, as the back returned it.
    pub struct SetRootDescriptor {
        bind_point: BindPoint,
        root_index: u32,
        /// `RootParamKind::CBV`, `SRV` or `UAV`.
        kind: RootParamKind,
        _pad: u32,
        gpu_va: u64,
    }
}

wire_struct! {
    /// A root descriptor table: slot `index` of `heap` is the table's first descriptor.
    pub struct SetRootTable {
        bind_point: BindPoint,
        root_index: u32,
        heap: HeapId,
        index: u32,
    }
}

wire_struct! {
    pub struct Dispatch {
        x: u32,
        y: u32,
        z: u32,
        _pad: u32,
    }
}

wire_struct! {
    /// Placeholder (M2): three u32 thread-group counts at `args_va`.
    pub struct DispatchIndirect {
        args_va: u64,
    }
}

wire_struct! {
    pub struct CopyBufferRegion {
        dst: ResId,
        src: ResId,
        dst_offset: u64,
        src_offset: u64,
        size: u64,
    }
}

wire_struct! {
    /// Placeholder (M2): fill `size` bytes at `offset` with the 32-bit `value`.
    pub struct FillBuffer {
        dst: ResId,
        value: u32,
        offset: u64,
        size: u64,
    }
}

wire_struct! {
    /// Placeholder (M2): clear a buffer UAV with four 32-bit values.
    pub struct ClearUav {
        res: ResId,
        /// Format id of the view being cleared.
        format: u32,
        values: [u32; 4],
        offset: u64,
        size: u64,
    }
}

wire_struct! {
    /// One side of a texture copy: a subresource of a texture, or a footprint
    /// in a buffer. Fields of the other kind are zero.
    pub struct TextureLocation {
        /// A texture (`SUBRESOURCE`) or a buffer (`FOOTPRINT`).
        res: ResId,
        kind: LocKind,
        /// `SUBRESOURCE`: plane of a depth-stencil texture (0 depth or colour, 1 stencil).
        plane: u8,
        /// `SUBRESOURCE`: mip level.
        mip: u16,
        /// `SUBRESOURCE`: array slice.
        slice: u32,
        /// `FOOTPRINT`: format of the texels in the buffer (DXGI number).
        format: u32,
        /// `FOOTPRINT`: byte offset of the first texel.
        offset: u64,
        /// `FOOTPRINT`: bytes between rows; the back checks it against the
        /// format's row size and refuses (`INVALID_ARG`) a pitch Metal cannot use.
        row_pitch: u32,
        /// `FOOTPRINT`: texels per row.
        width: u32,
        /// `FOOTPRINT`
        height: u32,
        /// `FOOTPRINT`
        depth: u32,
    }
}

wire_struct! {
    /// Copies a region between a texture and a buffer, or between two
    /// textures. With `flags` bit 0 clear the source is the whole of its
    /// subresource (or footprint). Coordinates are texels (a block compressed format: texels, a multiple of the block size).
    pub struct CopyTextureRegion {
        dst: TextureLocation,
        src: TextureLocation,
        dst_x: u32,
        dst_y: u32,
        dst_z: u32,
        /// Bit 0: the source box below is given.
        flags: u32,
        box_left: u32,
        box_top: u32,
        box_front: u32,
        box_right: u32,
        box_bottom: u32,
        box_back: u32,
    }
}

impl CopyTextureRegion {
    pub const HAS_BOX: u32 = 1;
}

wire_struct! {
    /// Copies the whole of one resource to another of the same description
    /// (two buffers of one size, or two textures with the same format,
    /// dimensions, mips and array size): every subresource.
    pub struct CopyResource {
        dst: ResId,
        src: ResId,
    }
}

wire_struct! {
    /// Clears a texture UAV: mip `mip`, slices `first_slice ..
    /// first_slice + slice_count` (`slice_count` 0 = to the end), in `format`
    /// (`UNKNOWN` = the texture's own). `float_values` says the four values
    /// are `f32` bit patterns, else `u32`.
    pub struct ClearUavTexture {
        res: ResId,
        format: u32,
        dimension: TexViewDim,
        float_values: u8,
        /// Bit 0: the rectangle below is given, else the whole mip.
        flags: u16,
        mip: u32,
        first_slice: u32,
        slice_count: u32,
        values: [u32; 4],
        rect_left: u32,
        rect_top: u32,
        rect_right: u32,
        rect_bottom: u32,
        _pad: u32,
        _pad2: u32,
    }
}

impl ClearUavTexture {
    pub const HAS_RECT: u16 = 1;
}

wire_struct! {
    /// A barrier: `before` stages finish what they were doing to memory before
    /// `after` stages start. Resources are informational for `MEMORY`
    /// barriers (the Metal barrier is stage to stage) and name the pair for `ALIASING`.
    pub struct Barrier {
        class: BarrierClass,
        /// `visibility::*` bits.
        visibility: u8,
        /// `stage::*` bits.
        before: u16,
        /// `stage::*` bits.
        after: u16,
        _pad: u16,
        /// 0 = any.
        res_before: ResId,
        /// 0 = any.
        res_after: ResId,
    }
}

impl Barrier {
    /// A stage-to-stage memory barrier with device visibility.
    pub const fn memory(before: u16, after: u16) -> Self {
        let mut b = <Self as Wire>::ZERO;
        b.class = BarrierClass::MEMORY;
        b.visibility = visibility::DEVICE;
        b.before = before;
        b.after = after;
        b
    }

    /// An aliasing barrier between two resources sharing memory.
    pub const fn aliasing(before: ResId, after: ResId) -> Self {
        let mut b = <Self as Wire>::ZERO;
        b.class = BarrierClass::ALIASING;
        b.visibility = visibility::DEVICE | visibility::RESOURCE_ALIAS;
        b.before = stage::ALL;
        b.after = stage::ALL;
        b.res_before = before;
        b.res_after = after;
        b
    }
}

wire_struct! {
    /// Fixed head of `SYNC_RECORD`; `args_len` bytes of block and `blob_len`
    /// bytes of blob follow, each padded to 8.
    pub struct SyncRecordHead {
        op: SyncOp,
        status: Status,
        args_len: u32,
        blob_len: u32,
    }
}

wire_struct! {
    /// Fixed head of `BUFFER_DATA`; `len` bytes follow.
    pub struct BufferDataHead {
        res: ResId,
        _pad: u32,
        offset: u64,
        len: u64,
    }
}

/// `SET_ROOT_CONSTANTS`: `data` is `4 * count` bytes, little-endian u32 values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetRootConstants<'a> {
    pub bind_point: BindPoint,
    pub root_index: u32,
    pub dest_offset: u32,
    pub data: &'a [u8],
}

impl SetRootConstants<'_> {
    /// Number of 32-bit values.
    pub fn count(&self) -> usize {
        self.data.len() / 4
    }

    /// The values, decoded.
    pub fn values(&self) -> impl Iterator<Item = u32> + '_ {
        self.data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
    }
}

/// `SYNC_RECORD` (replay-only): one recorded sync call. `args` is the whole
/// argument block as it was after the call (inputs and outputs); `blob` holds
/// the bytes the block pointed to (root signature bytes, DXIL), which the
/// replayer must re-point the address fields at. See `docs/metal/proto.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncRecord<'a> {
    pub op: SyncOp,
    pub status: Status,
    pub args: &'a [u8],
    pub blob: &'a [u8],
}

/// `BUFFER_DATA` (replay-only): the contents the CPU put in a mapped buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferData<'a> {
    pub res: ResId,
    pub offset: u64,
    pub data: &'a [u8],
}

/// One decoded (or to-be-encoded) command. Variable parts borrow the stream.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command<'a> {
    BeginList(BeginList),
    EndList(EndList),
    QueueSignal(QueueSignal),
    QueueWait(QueueWait),
    DebugLabel(&'a [u8]),
    SetPipeline(SetPipeline),
    SetRootSignature(SetRootSignature),
    SetDescriptorHeaps(SetDescriptorHeaps),
    SetRootConstants(SetRootConstants<'a>),
    SetRootDescriptor(SetRootDescriptor),
    SetRootTable(SetRootTable),
    Dispatch(Dispatch),
    DispatchIndirect(DispatchIndirect),
    CopyBufferRegion(CopyBufferRegion),
    FillBuffer(FillBuffer),
    ClearUav(ClearUav),
    Barrier(Barrier),
    CopyTextureRegion(CopyTextureRegion),
    CopyResource(CopyResource),
    ClearUavTexture(ClearUavTexture),
    BeginRenderPass(BeginRenderPass),
    EndRenderPass(EndRenderPass),
    SetViewports(SetViewports),
    SetScissors(SetScissors),
    SetBlendFactor(SetBlendFactor),
    SetStencilRef(SetStencilRef),
    SetPrimitiveTopology(SetPrimitiveTopology),
    SetIndexBuffer(SetIndexBuffer),
    SetVertexBuffers(SetVertexBuffers),
    Draw(Draw),
    DrawIndexed(DrawIndexed),
    DispatchMesh(DispatchMesh),
    ClearAttachment(ClearAttachment),
    SyncRecord(SyncRecord<'a>),
    BufferData(BufferData<'a>),
}

fn bad(op: Opcode, field: &'static str) -> Error {
    Error::BadValue {
        opcode: op.0,
        field,
    }
}

fn check_bind_point(op: Opcode, b: BindPoint) -> Result<(), Error> {
    if b.is_known() {
        Ok(())
    } else {
        Err(bad(op, "bind_point"))
    }
}

fn check_begin_pass(op: Opcode, c: &BeginRenderPass) -> Result<(), Error> {
    if c.rt_count as usize > MAX_RENDER_TARGETS || c.flags != 0 {
        return Err(bad(op, "rt_count"));
    }
    for (i, a) in c.rt.iter().enumerate() {
        let used = i < c.rt_count as usize;
        if !used {
            if *a != <RenderAttachment as Wire>::ZERO {
                return Err(bad(op, "attachment past the count"));
            }
            continue;
        }
        if a.clear.iter().any(|f| f.is_nan()) {
            return Err(bad(op, "clear value"));
        }
        if a.res.is_null() {
            continue;
        }
        if !a.load.is_known() || !a.dimension.is_known() || a.dimension == TexViewDim::NONE {
            return Err(bad(op, "attachment"));
        }
        if a.slice_count == 0 {
            return Err(bad(op, "slice_count"));
        }
    }
    let d = &c.ds;
    if d.clear_depth.is_nan() {
        return Err(bad(op, "clear depth"));
    }
    if !d.res.is_null()
        && (!d.depth_load.is_known()
            || !d.stencil_load.is_known()
            || !d.dimension.is_known()
            || d.dimension == TexViewDim::NONE
            || d.slice_count == 0)
    {
        return Err(bad(op, "depth attachment"));
    }
    Ok(())
}

impl Command<'_> {
    pub fn opcode(&self) -> Opcode {
        match self {
            Command::BeginList(_) => Opcode::BEGIN_LIST,
            Command::EndList(_) => Opcode::END_LIST,
            Command::QueueSignal(_) => Opcode::QUEUE_SIGNAL,
            Command::QueueWait(_) => Opcode::QUEUE_WAIT,
            Command::DebugLabel(_) => Opcode::DEBUG_LABEL,
            Command::SetPipeline(_) => Opcode::SET_PIPELINE,
            Command::SetRootSignature(_) => Opcode::SET_ROOT_SIGNATURE,
            Command::SetDescriptorHeaps(_) => Opcode::SET_DESCRIPTOR_HEAPS,
            Command::SetRootConstants(_) => Opcode::SET_ROOT_CONSTANTS,
            Command::SetRootDescriptor(_) => Opcode::SET_ROOT_DESCRIPTOR,
            Command::SetRootTable(_) => Opcode::SET_ROOT_TABLE,
            Command::Dispatch(_) => Opcode::DISPATCH,
            Command::DispatchIndirect(_) => Opcode::DISPATCH_INDIRECT,
            Command::CopyBufferRegion(_) => Opcode::COPY_BUFFER_REGION,
            Command::FillBuffer(_) => Opcode::FILL_BUFFER,
            Command::ClearUav(_) => Opcode::CLEAR_UAV,
            Command::Barrier(_) => Opcode::BARRIER,
            Command::CopyTextureRegion(_) => Opcode::COPY_TEXTURE_REGION,
            Command::CopyResource(_) => Opcode::COPY_RESOURCE,
            Command::ClearUavTexture(_) => Opcode::CLEAR_UAV_TEXTURE,
            Command::BeginRenderPass(_) => Opcode::BEGIN_RENDER_PASS,
            Command::EndRenderPass(_) => Opcode::END_RENDER_PASS,
            Command::SetViewports(_) => Opcode::SET_VIEWPORTS,
            Command::SetScissors(_) => Opcode::SET_SCISSORS,
            Command::SetBlendFactor(_) => Opcode::SET_BLEND_FACTOR,
            Command::SetStencilRef(_) => Opcode::SET_STENCIL_REF,
            Command::SetPrimitiveTopology(_) => Opcode::SET_PRIMITIVE_TOPOLOGY,
            Command::SetIndexBuffer(_) => Opcode::SET_INDEX_BUFFER,
            Command::SetVertexBuffers(_) => Opcode::SET_VERTEX_BUFFERS,
            Command::Draw(_) => Opcode::DRAW,
            Command::DrawIndexed(_) => Opcode::DRAW_INDEXED,
            Command::DispatchMesh(_) => Opcode::DISPATCH_MESH,
            Command::ClearAttachment(_) => Opcode::CLEAR_ATTACHMENT,
            Command::SyncRecord(_) => Opcode::SYNC_RECORD,
            Command::BufferData(_) => Opcode::BUFFER_DATA,
        }
    }

    /// Checks the values the type system cannot: known enums, mask bits,
    /// size limits. The decoder runs it on every command; the encoder too.
    pub fn check(&self) -> Result<(), Error> {
        let op = self.opcode();
        match self {
            Command::SetRootSignature(c) => check_bind_point(op, c.bind_point),
            Command::SetRootConstants(c) => {
                check_bind_point(op, c.bind_point)?;
                if !c.data.len().is_multiple_of(4) || c.data.len() > MAX_ROOT_CONSTANTS * 4 {
                    return Err(bad(op, "data"));
                }
                Ok(())
            }
            Command::SetRootDescriptor(c) => {
                check_bind_point(op, c.bind_point)?;
                match c.kind {
                    RootParamKind::CBV | RootParamKind::SRV | RootParamKind::UAV => Ok(()),
                    _ => Err(bad(op, "kind")),
                }
            }
            Command::SetRootTable(c) => check_bind_point(op, c.bind_point),
            Command::DebugLabel(l) if l.len() > MAX_LABEL => Err(bad(op, "label")),
            Command::CopyTextureRegion(c) => {
                for (l, name) in [(&c.dst, "dst"), (&c.src, "src")] {
                    if !l.kind.is_known() {
                        return Err(bad(op, name));
                    }
                }
                if c.flags & !CopyTextureRegion::HAS_BOX != 0 {
                    return Err(bad(op, "flags"));
                }
                Ok(())
            }
            Command::ClearUavTexture(c) => {
                if !c.dimension.is_known() || c.dimension == TexViewDim::NONE {
                    return Err(bad(op, "dimension"));
                }
                if c.float_values > 1 || c.flags & !ClearUavTexture::HAS_RECT != 0 {
                    return Err(bad(op, "flags"));
                }
                Ok(())
            }
            Command::Barrier(b) => {
                if !b.class.is_known() {
                    return Err(bad(op, "class"));
                }
                if b.visibility & !visibility::ALL_BITS != 0 {
                    return Err(bad(op, "visibility"));
                }
                if b.before & !stage::ALL != 0 || b.after & !stage::ALL != 0 {
                    return Err(bad(op, "stages"));
                }
                if b.class == BarrierClass::MEMORY && (b.before == 0 || b.after == 0) {
                    return Err(bad(op, "stages"));
                }
                Ok(())
            }
            Command::BeginRenderPass(c) => check_begin_pass(op, c),
            Command::EndRenderPass(c) => {
                let known = c.rt_store.iter().all(|s| s.is_known())
                    && c.depth_store.is_known()
                    && c.stencil_store.is_known();
                if known { Ok(()) } else { Err(bad(op, "store")) }
            }
            Command::SetViewports(c) => {
                let nan = c.vp.iter().any(|v| {
                    [v.x, v.y, v.width, v.height, v.min_depth, v.max_depth]
                        .iter()
                        .any(|f| f.is_nan())
                });
                if c.count as usize > MAX_VIEWPORTS || nan {
                    return Err(bad(op, "viewport"));
                }
                Ok(())
            }
            Command::SetBlendFactor(c) if c.color.iter().any(|f| f.is_nan()) => {
                Err(bad(op, "color"))
            }
            Command::SetScissors(c) if c.count as usize > MAX_VIEWPORTS => Err(bad(op, "count")),
            Command::SetPrimitiveTopology(c) if !c.topology.is_known() => Err(bad(op, "topology")),
            Command::SetIndexBuffer(c) if !c.format.is_known() => Err(bad(op, "format")),
            Command::SetVertexBuffers(c) => {
                if c.count as usize > MAX_VERTEX_BUFFERS
                    || u64::from(c.start_slot) + u64::from(c.count) > MAX_VERTEX_BUFFERS as u64
                {
                    return Err(bad(op, "count"));
                }
                Ok(())
            }
            Command::ClearAttachment(c) => {
                if c.target > CLEAR_TARGET_DEPTH_STENCIL {
                    return Err(bad(op, "target"));
                }
                if c.rect_count as usize > MAX_CLEAR_RECTS {
                    return Err(bad(op, "rect_count"));
                }
                if c.color.iter().any(|f| f.is_nan()) || c.clear_depth.is_nan() {
                    return Err(bad(op, "clear value"));
                }
                if c.target == CLEAR_TARGET_DEPTH_STENCIL
                    && (c.flags == 0 || c.flags & !(CLEAR_DEPTH | CLEAR_STENCIL) != 0)
                {
                    return Err(bad(op, "flags"));
                }
                Ok(())
            }
            Command::SyncRecord(r) if r.args.len() > MAX_PAYLOAD || r.blob.len() > MAX_PAYLOAD => {
                Err(Error::TooLarge)
            }
            Command::BufferData(d) if d.data.len() > MAX_PAYLOAD => Err(Error::TooLarge),
            _ => Ok(()),
        }
    }

    /// Appends the command (header and payload) to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<(), Error> {
        self.check()?;
        let op = self.opcode();
        let start = out.len();
        // Header with a length to patch.
        op.0.put(out);
        opcode_version(op).put(out);
        0u8.put(out);
        0u32.put(out);
        let body = out.len();
        match self {
            Command::BeginList(c) => c.put(out),
            Command::EndList(c) => c.put(out),
            Command::QueueSignal(c) => c.put(out),
            Command::QueueWait(c) => c.put(out),
            Command::DebugLabel(l) => {
                DebugLabelHead {
                    len: l.len() as u32,
                    _pad: 0,
                }
                .put(out);
                out.extend_from_slice(l);
            }
            Command::SetPipeline(c) => c.put(out),
            Command::SetRootSignature(c) => c.put(out),
            Command::SetDescriptorHeaps(c) => c.put(out),
            Command::SetRootConstants(c) => {
                RootConstantsHead {
                    bind_point: c.bind_point,
                    root_index: c.root_index,
                    dest_offset: c.dest_offset,
                    count: c.count() as u32,
                }
                .put(out);
                out.extend_from_slice(c.data);
            }
            Command::SetRootDescriptor(c) => c.put(out),
            Command::SetRootTable(c) => c.put(out),
            Command::Dispatch(c) => c.put(out),
            Command::DispatchIndirect(c) => c.put(out),
            Command::CopyBufferRegion(c) => c.put(out),
            Command::FillBuffer(c) => c.put(out),
            Command::ClearUav(c) => c.put(out),
            Command::Barrier(c) => c.put(out),
            Command::CopyTextureRegion(c) => c.put(out),
            Command::CopyResource(c) => c.put(out),
            Command::ClearUavTexture(c) => c.put(out),
            Command::BeginRenderPass(c) => c.put(out),
            Command::EndRenderPass(c) => c.put(out),
            Command::SetViewports(c) => c.put(out),
            Command::SetScissors(c) => c.put(out),
            Command::SetBlendFactor(c) => c.put(out),
            Command::SetStencilRef(c) => c.put(out),
            Command::SetPrimitiveTopology(c) => c.put(out),
            Command::SetIndexBuffer(c) => c.put(out),
            Command::SetVertexBuffers(c) => c.put(out),
            Command::Draw(c) => c.put(out),
            Command::DrawIndexed(c) => c.put(out),
            Command::DispatchMesh(c) => c.put(out),
            Command::ClearAttachment(c) => c.put(out),
            Command::SyncRecord(c) => {
                SyncRecordHead {
                    op: c.op,
                    status: c.status,
                    args_len: c.args.len() as u32,
                    blob_len: c.blob.len() as u32,
                }
                .put(out);
                out.extend_from_slice(c.args);
                pad_to_8(out);
                out.extend_from_slice(c.blob);
            }
            Command::BufferData(c) => {
                BufferDataHead {
                    res: c.res,
                    _pad: 0,
                    offset: c.offset,
                    len: c.data.len() as u64,
                }
                .put(out);
                out.extend_from_slice(c.data);
            }
        }
        pad_to_8(out);
        let len = out.len() - body;
        if len > MAX_PAYLOAD {
            out.truncate(start);
            return Err(Error::TooLarge);
        }
        out[start + 4..start + 8].copy_from_slice(&(len as u32).to_le_bytes());
        Ok(())
    }
}

/// Decodes one command at the start of `buf`; returns it and the bytes it took.
pub fn decode_one<'a>(buf: &'a [u8]) -> Result<(Command<'a>, usize), Error> {
    let mut h = Reader::new(buf);
    let code = u16::get(&mut h)?;
    let version = u8::get(&mut h)?;
    let flags = u8::get(&mut h)?;
    let len = u32::get(&mut h)?;
    let op = Opcode(code);
    if !op.is_known() {
        return Err(match reserved_range(code) {
            Some(_) => Error::ReservedOpcode(code),
            None => Error::UnknownOpcode(code),
        });
    }
    if version != opcode_version(op) {
        return Err(Error::UnknownOpcodeVersion {
            opcode: code,
            version,
        });
    }
    if flags != 0 {
        return Err(bad(op, "flags"));
    }
    let bad_len = Error::BadPayloadLength { opcode: code, len };
    let payload_len = len as usize;
    if payload_len > MAX_PAYLOAD || !payload_len.is_multiple_of(8) || payload_len > h.remaining() {
        return Err(bad_len);
    }
    let payload = h.take(payload_len)?;
    let mut r = Reader::new(payload);

    macro_rules! fixed {
        ($variant:ident, $ty:ty) => {{
            if payload_len != <$ty as Wire>::SIZE {
                return Err(bad_len);
            }
            Command::$variant(<$ty>::get(&mut r)?)
        }};
    }
    // After a variable head of `head` bytes: `n` bytes of data, padded to the length.
    fn tail<'a>(
        r: &mut Reader<'a>,
        head: usize,
        n: usize,
        payload_len: usize,
        err: Error,
    ) -> Result<&'a [u8], Error> {
        if n > payload_len || align8(head + n) != payload_len {
            return Err(err);
        }
        r.take(n)
    }

    let cmd = match op {
        Opcode::BEGIN_LIST => fixed!(BeginList, BeginList),
        Opcode::END_LIST => fixed!(EndList, EndList),
        Opcode::QUEUE_SIGNAL => fixed!(QueueSignal, QueueSignal),
        Opcode::QUEUE_WAIT => fixed!(QueueWait, QueueWait),
        Opcode::DEBUG_LABEL => {
            let head = DebugLabelHead::get(&mut r)?;
            let n = head.len as usize;
            Command::DebugLabel(tail(&mut r, DebugLabelHead::SIZE, n, payload_len, bad_len)?)
        }
        Opcode::SET_PIPELINE => fixed!(SetPipeline, SetPipeline),
        Opcode::SET_ROOT_SIGNATURE => fixed!(SetRootSignature, SetRootSignature),
        Opcode::SET_DESCRIPTOR_HEAPS => fixed!(SetDescriptorHeaps, SetDescriptorHeaps),
        Opcode::SET_ROOT_CONSTANTS => {
            let head = RootConstantsHead::get(&mut r)?;
            let n = (head.count as usize).checked_mul(4).ok_or(bad_len)?;
            Command::SetRootConstants(SetRootConstants {
                bind_point: head.bind_point,
                root_index: head.root_index,
                dest_offset: head.dest_offset,
                data: tail(&mut r, RootConstantsHead::SIZE, n, payload_len, bad_len)?,
            })
        }
        Opcode::SET_ROOT_DESCRIPTOR => fixed!(SetRootDescriptor, SetRootDescriptor),
        Opcode::SET_ROOT_TABLE => fixed!(SetRootTable, SetRootTable),
        Opcode::DISPATCH => fixed!(Dispatch, Dispatch),
        Opcode::DISPATCH_INDIRECT => fixed!(DispatchIndirect, DispatchIndirect),
        Opcode::COPY_BUFFER_REGION => fixed!(CopyBufferRegion, CopyBufferRegion),
        Opcode::FILL_BUFFER => fixed!(FillBuffer, FillBuffer),
        Opcode::CLEAR_UAV => fixed!(ClearUav, ClearUav),
        Opcode::BARRIER => fixed!(Barrier, Barrier),
        Opcode::COPY_TEXTURE_REGION => fixed!(CopyTextureRegion, CopyTextureRegion),
        Opcode::COPY_RESOURCE => fixed!(CopyResource, CopyResource),
        Opcode::CLEAR_UAV_TEXTURE => fixed!(ClearUavTexture, ClearUavTexture),
        Opcode::BEGIN_RENDER_PASS => fixed!(BeginRenderPass, BeginRenderPass),
        Opcode::END_RENDER_PASS => fixed!(EndRenderPass, EndRenderPass),
        Opcode::SET_VIEWPORTS => fixed!(SetViewports, SetViewports),
        Opcode::SET_SCISSORS => fixed!(SetScissors, SetScissors),
        Opcode::SET_BLEND_FACTOR => fixed!(SetBlendFactor, SetBlendFactor),
        Opcode::SET_STENCIL_REF => fixed!(SetStencilRef, SetStencilRef),
        Opcode::SET_PRIMITIVE_TOPOLOGY => fixed!(SetPrimitiveTopology, SetPrimitiveTopology),
        Opcode::SET_INDEX_BUFFER => fixed!(SetIndexBuffer, SetIndexBuffer),
        Opcode::SET_VERTEX_BUFFERS => fixed!(SetVertexBuffers, SetVertexBuffers),
        Opcode::DRAW => fixed!(Draw, Draw),
        Opcode::DRAW_INDEXED => fixed!(DrawIndexed, DrawIndexed),
        Opcode::DISPATCH_MESH => fixed!(DispatchMesh, DispatchMesh),
        Opcode::CLEAR_ATTACHMENT => fixed!(ClearAttachment, ClearAttachment),
        Opcode::SYNC_RECORD => {
            let head = SyncRecordHead::get(&mut r)?;
            let (a, b) = (head.args_len as usize, head.blob_len as usize);
            // Head, args padded, blob padded: both parts must fit exactly.
            let total = SyncRecordHead::SIZE
                .checked_add(align8(a))
                .and_then(|x| x.checked_add(align8(b)))
                .ok_or(bad_len)?;
            if total != payload_len {
                return Err(bad_len);
            }
            let args = r.take(a)?;
            r.take(align8(a) - a)?;
            let blob = r.take(b)?;
            Command::SyncRecord(SyncRecord {
                op: head.op,
                status: head.status,
                args,
                blob,
            })
        }
        Opcode::BUFFER_DATA => {
            let head = BufferDataHead::get(&mut r)?;
            let n = usize::try_from(head.len).map_err(|_| bad_len)?;
            Command::BufferData(BufferData {
                res: head.res,
                offset: head.offset,
                data: tail(&mut r, BufferDataHead::SIZE, n, payload_len, bad_len)?,
            })
        }
        _ => return Err(Error::UnknownOpcode(code)),
    };
    cmd.check()?;
    Ok((cmd, CMD_HEADER_SIZE + payload_len))
}
