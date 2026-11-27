//! Tables of everything proto defines: used by the docs generator, by the
//! tests that keep the docs and the code in step, and by the layout
//! fingerprint that makes "changed the wire format" impossible to miss.

use crate::cmd::{self, Opcode, opcode_version};
use crate::sync::*;
use crate::wire::{FieldInfo, Layout};
use alloc::string::String;
use core::fmt::Write;

/// How far the back is expected to go with a command or call in M1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// Codec done and the back implements it in M1.
    Implemented,
    /// Codec done; the back may answer `UNSUPPORTED` until M2/M3.
    Placeholder,
    /// Only in recorded streams; the replayer handles it, a live back never sees it.
    ReplayOnly,
}

/// One sync call.
#[derive(Clone, Copy, Debug)]
pub struct SyncInfo {
    pub op: SyncOp,
    pub name: &'static str,
    pub version: u16,
    pub size: usize,
    pub fields: &'static [FieldInfo],
    pub support: Support,
    pub summary: &'static str,
}

/// One stream command. `size` is the fixed head; `var` says bytes follow it.
#[derive(Clone, Copy, Debug)]
pub struct CommandInfo {
    pub opcode: Opcode,
    pub name: &'static str,
    pub version: u8,
    pub size: usize,
    pub var: bool,
    pub fields: &'static [FieldInfo],
    pub support: Support,
    pub summary: &'static str,
}

const fn sync<A: SyncArgs>(support: Support, summary: &'static str) -> SyncInfo {
    SyncInfo {
        op: A::OP,
        name: A::NAME,
        version: A::VERSION,
        size: A::SIZE,
        fields: A::FIELDS,
        support,
        summary,
    }
}

const fn command<P: Layout>(
    opcode: Opcode,
    var: bool,
    support: Support,
    summary: &'static str,
) -> CommandInfo {
    CommandInfo {
        opcode,
        name: P::NAME,
        version: opcode_version(opcode),
        size: P::SIZE,
        var,
        fields: P::FIELDS,
        support,
        summary,
    }
}

use Support::{Implemented as I, Placeholder as P, ReplayOnly as R};

/// Every sync call, in number order.
pub static SYNC_OPS: &[SyncInfo] = &[
    sync::<NopArgs>(I, "does nothing; measures the call"),
    sync::<HelloArgs>(
        I,
        "first call: creates the device. in: client_proto_version, required_caps; out: proto_version, caps, device_name",
    ),
    sync::<QueryCapsArgs>(
        I,
        "out: everything but head (limits, slot sizes for handle arithmetic)",
    ),
    sync::<ShutdownArgs>(I, "destroys everything; HELLO again to restart"),
    sync::<CreateQueueArgs>(I, "in: kind (QueueKind), priority; out: queue"),
    sync::<DestroyQueueArgs>(I, "in: queue (drained first)"),
    sync::<CreateBufferArgs>(
        I,
        "in: size, kind (BufferKind, not WRAPPED), flags=0; out: res, gpu_va, cpu_addr (0 for DEFAULT), alloc_size",
    ),
    sync::<DestroyBufferArgs>(I, "in: res (freed after the GPU is done)"),
    sync::<WrapMemoryArgs>(
        I,
        "in: addr, len (page aligned), flags=0; out: res, gpu_va; CONFLICT if a page is already in another buffer",
    ),
    sync::<CreateDescHeapArgs>(
        I,
        "in: kind (HeapKind), capacity, flags (bit0 shader visible); out: heap, stride, gpu_va, cpu_addr",
    ),
    sync::<DestroyDescHeapArgs>(I, "in: heap"),
    sync::<WriteDescriptorArgs>(
        I,
        "in: heap, index, view (or sampler when view.kind = SAMPLER)",
    ),
    sync::<CopyDescriptorsArgs>(
        I,
        "in: dst_heap, dst_index, src_heap, src_index, count (same heap kind)",
    ),
    sync::<CreateRootSignatureArgs>(
        I,
        "in: data_addr, data_len (RootSigDesc::encode bytes); out: root_sig",
    ),
    sync::<DestroyRootSignatureArgs>(I, "in: root_sig"),
    sync::<CreateComputePipelineArgs>(
        I,
        "in: root_sig, flags=0, dxil_addr, dxil_len, reserved=0; out: pso (may be UNSUPPORTED until the shader converter is wired)",
    ),
    sync::<DestroyPipelineArgs>(I, "in: pso"),
    sync::<CreateFenceArgs>(I, "in: initial, flags=0; out: fence"),
    sync::<DestroyFenceArgs>(I, "in: fence"),
    sync::<FenceValueArgs>(I, "in: fence; out: value (highest completed)"),
    sync::<FenceWaitArgs>(
        I,
        "in: fence, value, timeout_ms (u64::MAX = forever); TIMEOUT when it runs out",
    ),
    sync::<FenceSignalArgs>(I, "in: fence, value (never lowers it)"),
    sync::<DeviceStatusArgs>(I, "out: state (0 fine, 1 hung, 2 removed)"),
    sync::<GetSurfaceLayerArgs>(
        P,
        "in: window (WindowRef); out: surface, layer_addr (M3; back may answer UNSUPPORTED)",
    ),
    sync::<CreateTextureArgs>(
        I,
        "in: dimension (TexDim), format (DXGI), width, height, depth_or_array, mips, samples, usage (tex_usage bits), flags=0, heap (null = committed), heap_offset; out: res, alloc_size",
    ),
    sync::<DestroyTextureArgs>(I, "in: res (freed after the GPU is done)"),
    sync::<CreateHeapArgs>(
        I,
        "in: kind (MemKind), flags=0, size; out: heap, alloc_size (placement memory for buffers and textures)",
    ),
    sync::<DestroyHeapArgs>(I, "in: heap"),
    sync::<CreatePlacedBufferArgs>(
        I,
        "in: heap, offset, size; out: res, gpu_va, cpu_addr (0 in a DEFAULT heap)",
    ),
    sync::<GetResourceAllocationInfoArgs>(
        I,
        "in: kind (ResKind), the texture fields or buffer_size, heap_kind; out: size, alignment (what Metal's heap size-and-align says)",
    ),
    sync::<CreateGraphicsPipelineArgs>(
        I,
        "in: root_sig, flags=0, desc (GraphicsPsoDesc: DXIL addr+len per stage, inputs, targets, blend, raster, depth); out: pso (UNSUPPORTED for a shader or state the back cannot make)",
    ),
];

/// Every command, in opcode order.
pub static COMMANDS: &[CommandInfo] = &[
    command::<cmd::BeginList>(
        Opcode::BEGIN_LIST,
        false,
        I,
        "start a command list on a queue",
    ),
    command::<cmd::EndList>(Opcode::END_LIST, false, I, "end the list"),
    command::<cmd::QueueSignal>(
        Opcode::QUEUE_SIGNAL,
        false,
        I,
        "queue signals a fence value",
    ),
    command::<cmd::QueueWait>(
        Opcode::QUEUE_WAIT,
        false,
        I,
        "queue waits for a fence value",
    ),
    command::<cmd::DebugLabelHead>(
        Opcode::DEBUG_LABEL,
        true,
        I,
        "text marker (len bytes follow)",
    ),
    command::<cmd::SetPipeline>(Opcode::SET_PIPELINE, false, I, "bind a pipeline"),
    command::<cmd::SetRootSignature>(
        Opcode::SET_ROOT_SIGNATURE,
        false,
        I,
        "bind a root signature",
    ),
    command::<cmd::SetDescriptorHeaps>(
        Opcode::SET_DESCRIPTOR_HEAPS,
        false,
        I,
        "choose the two heaps",
    ),
    command::<cmd::RootConstantsHead>(
        Opcode::SET_ROOT_CONSTANTS,
        true,
        I,
        "32-bit root constants (count u32 follow)",
    ),
    command::<cmd::SetRootDescriptor>(
        Opcode::SET_ROOT_DESCRIPTOR,
        false,
        I,
        "root CBV/SRV/UAV by GPU address",
    ),
    command::<cmd::SetRootTable>(Opcode::SET_ROOT_TABLE, false, I, "root table = heap slot"),
    command::<cmd::Dispatch>(Opcode::DISPATCH, false, I, "dispatch thread groups"),
    command::<cmd::DispatchIndirect>(
        Opcode::DISPATCH_INDIRECT,
        false,
        P,
        "dispatch from GPU arguments (M2)",
    ),
    command::<cmd::CopyBufferRegion>(
        Opcode::COPY_BUFFER_REGION,
        false,
        I,
        "buffer to buffer copy",
    ),
    command::<cmd::FillBuffer>(
        Opcode::FILL_BUFFER,
        false,
        P,
        "fill with a 32-bit value (M2)",
    ),
    command::<cmd::ClearUav>(Opcode::CLEAR_UAV, false, P, "clear a buffer UAV (M2)"),
    command::<cmd::Barrier>(
        Opcode::BARRIER,
        false,
        I,
        "stage-to-stage / aliasing barrier",
    ),
    command::<cmd::CopyTextureRegion>(
        Opcode::COPY_TEXTURE_REGION,
        false,
        I,
        "buffer footprint to texture, texture to buffer footprint, or texture to texture (optional source box)",
    ),
    command::<cmd::CopyResource>(
        Opcode::COPY_RESOURCE,
        false,
        I,
        "whole resource to a resource of the same description (buffer or texture)",
    ),
    command::<cmd::ClearUavTexture>(
        Opcode::CLEAR_UAV_TEXTURE,
        false,
        I,
        "clear a texture UAV (mip, slices, optional rectangle) with float or uint values",
    ),
    command::<crate::render::SetViewports>(
        Opcode::SET_VIEWPORTS,
        false,
        I,
        "RSSetViewports: first count of 16 (applied at the next draw)",
    ),
    command::<crate::render::SetScissors>(
        Opcode::SET_SCISSORS,
        false,
        I,
        "RSSetScissorRects: first count of 16 edge rectangles",
    ),
    command::<crate::render::SetBlendFactor>(
        Opcode::SET_BLEND_FACTOR,
        false,
        I,
        "OMSetBlendFactor",
    ),
    command::<crate::render::SetStencilRef>(Opcode::SET_STENCIL_REF, false, I, "OMSetStencilRef"),
    command::<crate::render::SetPrimitiveTopology>(
        Opcode::SET_PRIMITIVE_TOPOLOGY,
        false,
        I,
        "IASetPrimitiveTopology (point/line/triangle lists and strips)",
    ),
    command::<crate::render::SetIndexBuffer>(
        Opcode::SET_INDEX_BUFFER,
        false,
        I,
        "IASetIndexBuffer: GPU address, size, 16 or 32 bit",
    ),
    command::<crate::render::SetVertexBuffers>(
        Opcode::SET_VERTEX_BUFFERS,
        false,
        I,
        "IASetVertexBuffers: start slot and first count of 16 views",
    ),
    command::<crate::render::Draw>(Opcode::DRAW, false, I, "DrawInstanced"),
    command::<crate::render::DrawIndexed>(Opcode::DRAW_INDEXED, false, I, "DrawIndexedInstanced"),
    command::<crate::render::DispatchMesh>(
        Opcode::DISPATCH_MESH,
        false,
        I,
        "DispatchMesh (object or mesh threadgroup counts)",
    ),
    command::<crate::render::ClearAttachment>(
        Opcode::CLEAR_ATTACHMENT,
        false,
        I,
        "clear an attachment, or rectangles of it, inside the open pass",
    ),
    command::<crate::render::BeginRenderPass>(
        Opcode::BEGIN_RENDER_PASS,
        false,
        I,
        "start a render pass: colour and depth attachments with their load actions and clear values",
    ),
    command::<crate::render::EndRenderPass>(
        Opcode::END_RENDER_PASS,
        false,
        I,
        "end the render pass: store action of every attachment",
    ),
    command::<cmd::SyncRecordHead>(
        Opcode::SYNC_RECORD,
        true,
        R,
        "recorded sync call (args, blob follow)",
    ),
    command::<cmd::BufferDataHead>(
        Opcode::BUFFER_DATA,
        true,
        R,
        "recorded CPU write to a buffer (len bytes follow)",
    ),
];

const fn fnv(mut h: u64, bytes: &[u8]) -> u64 {
    let mut i = 0;
    while i < bytes.len() {
        h ^= bytes[i] as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    h
}

const fn fnv_u64(h: u64, v: u64) -> u64 {
    fnv(h, &v.to_le_bytes())
}

const fn fnv_fields(mut h: u64, fields: &[FieldInfo]) -> u64 {
    let mut i = 0;
    while i < fields.len() {
        h = fnv(h, fields[i].name.as_bytes());
        h = fnv_u64(h, fields[i].offset as u64);
        h = fnv_u64(h, fields[i].size as u64);
        i += 1;
    }
    h
}

/// A hash of every wire-visible fact (names, offsets, sizes; not type spellings): header layouts, each command's number,
/// version, fixed layout, and each sync call's number, version, layout, plus
/// the descriptor records. Any change to the format changes it. The tests pin
/// it per `PROTO_VERSION`, so changing the format without bumping the version
/// (rule 4) fails them.
pub const fn layout_fingerprint() -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    h = fnv_u64(h, crate::PROTO_VERSION as u64);
    h = fnv_fields(h, <crate::stream::StreamHeader as Layout>::FIELDS);
    h = fnv_fields(h, <crate::file::FileHeader as Layout>::FIELDS);
    h = fnv_fields(h, <crate::sync::CallHead as Layout>::FIELDS);
    h = fnv_fields(h, <crate::root::RootSigHeader as Layout>::FIELDS);
    h = fnv_fields(h, <crate::root::RootParam as Layout>::FIELDS);
    h = fnv_fields(h, <crate::root::DescriptorRange as Layout>::FIELDS);
    h = fnv_fields(h, <crate::root::StaticSampler as Layout>::FIELDS);
    h = fnv_fields(h, <crate::types::SamplerDesc as Layout>::FIELDS);
    h = fnv_fields(h, <crate::types::ViewDesc as Layout>::FIELDS);
    {
        h = fnv_fields(h, <crate::pso::StageCode as Layout>::FIELDS);
        h = fnv_fields(h, <crate::pso::InputElement as Layout>::FIELDS);
        h = fnv_fields(h, <crate::pso::RtBlend as Layout>::FIELDS);
        h = fnv_fields(h, <crate::pso::BlendDesc as Layout>::FIELDS);
        h = fnv_fields(h, <crate::pso::RasterDesc as Layout>::FIELDS);
        h = fnv_fields(h, <crate::pso::StencilFace as Layout>::FIELDS);
        h = fnv_fields(h, <crate::pso::DepthStencilDesc as Layout>::FIELDS);
        h = fnv_fields(h, <crate::pso::GraphicsPsoDesc as Layout>::FIELDS);
    }
    let mut i = 0;
    while i < COMMANDS.len() {
        let c = &COMMANDS[i];
        h = fnv_u64(h, c.opcode.0 as u64);
        h = fnv_u64(h, c.version as u64);
        h = fnv_u64(h, c.size as u64);
        h = fnv_u64(h, c.var as u64);
        h = fnv_fields(h, c.fields);
        i += 1;
    }
    let mut i = 0;
    while i < SYNC_OPS.len() {
        let s = &SYNC_OPS[i];
        h = fnv_u64(h, s.op.0 as u64);
        h = fnv_u64(h, s.version as u64);
        h = fnv_u64(h, s.size as u64);
        h = fnv_fields(h, s.fields);
        i += 1;
    }
    h
}

const _: () = {
    // The registry and the version function must agree on what exists.
    let mut i = 0;
    while i < COMMANDS.len() {
        assert!(COMMANDS[i].version >= 1);
        assert!(COMMANDS[i].size.is_multiple_of(8));
        i += 1;
    }
};

fn fields_text(fields: &[FieldInfo]) -> String {
    let mut out = String::new();
    for (i, f) in fields.iter().enumerate() {
        if f.name.starts_with('_') {
            continue;
        }
        if i > 0 && !out.is_empty() {
            out.push_str(", ");
        }
        let _ = write!(out, "{}: {} @{}", f.name, f.ty, f.offset);
    }
    out
}

/// The API tables of `docs/metal/proto.md` as Markdown, generated from the
/// registry so that the document cannot drift from the code. Padding fields
/// are left out; offsets are bytes from the start of the block or payload.
pub fn markdown_tables() -> String {
    let mut out = String::new();
    let _ = writeln!(out, "### Sync calls (`Link::call`)\n");
    let _ = writeln!(
        out,
        "| num | name | ver | size | support | fields (name: type @offset) | what |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|---|");
    for s in SYNC_OPS {
        let _ = writeln!(
            out,
            "| {} | `{}` (`{}`) | {} | {} | {:?} | {} | {} |",
            s.op.0,
            s.op.name().unwrap_or("?"),
            s.name,
            s.version,
            s.size,
            s.support,
            fields_text(s.fields),
            s.summary
        );
    }
    let _ = writeln!(out, "\n### Stream commands (`Link::submit`)\n");
    let _ = writeln!(
        out,
        "| opcode | name | ver | head size | support | fields (name: type @offset) | what |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|---|");
    for c in COMMANDS {
        let _ = writeln!(
            out,
            "| 0x{:04X} | `{}` | {} | {}{} | {:?} | {} | {} |",
            c.opcode.0,
            c.opcode.name().unwrap_or("?"),
            c.version,
            c.size,
            if c.var { " + data" } else { "" },
            c.support,
            fields_text(c.fields),
            c.summary
        );
    }
    let _ = writeln!(out, "\n### Reserved opcode ranges\n");
    for (a, b, what) in cmd::RESERVED_RANGES {
        let _ = writeln!(out, "- 0x{a:04X}-0x{b:04X}: {what}");
    }
    out
}
