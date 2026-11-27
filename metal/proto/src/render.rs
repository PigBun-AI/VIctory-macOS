//! Render commands (proto version 3): passes, render state, draws, mesh
//! dispatch, and the clear of a rectangle inside a pass.
//!
//! The design is `docs/metal/proto-render-draft.md`; what was built and why it
//! differs from the draft is in `docs/metal/m4-render.md`. Summary:
//!
//! * **The front derives the pass, the back runs it.** `BEGIN_RENDER_PASS`
//!   carries the resolved attachments and their *load* actions (and clear
//!   values); `END_RENDER_PASS` carries the *store* actions, because those are
//!   only known when the pass ends (a `DiscardResource` that follows the pass
//!   patches them in the front's list before the block is sent).
//! * **State commands are order-free.** The back keeps a shadow of viewports,
//!   scissors, blend factor, stencil reference, topology, index and vertex
//!   buffers and applies it before the draw that needs it, so a front may emit
//!   them before `BEGIN_RENDER_PASS`.
//! * Fixed-size commands like every other one.
//!
//! Opcode numbers (this branch's range `0x0500..=0x06FF`): `0x0501..=0x0507`
//! state, `0x0510..=0x0512` draws, `0x0520` clear in a pass, `0x0600`/`0x0601`
//! pass begin/end.

use crate::ids::ResId;
use crate::types::TexViewDim;
use crate::wire::{Wire, array_wire, open_enum, wire_struct};

/// Most render targets of a pass (D3D12 and Metal agree).
pub const MAX_RENDER_TARGETS: usize = 8;
/// Most viewports and scissor rectangles (Metal's limit; D3D12 has 16 too).
pub const MAX_VIEWPORTS: usize = 16;
/// Most vertex buffer slots one `SET_VERTEX_BUFFERS` carries (D3D12's 16).
pub const MAX_VERTEX_BUFFERS: usize = 16;
/// Most rectangles of one `CLEAR_ATTACHMENT`.
pub const MAX_CLEAR_RECTS: usize = 8;
/// `CLEAR_ATTACHMENT.target` of the depth-stencil attachment.
pub const CLEAR_TARGET_DEPTH_STENCIL: u32 = 8;

open_enum! {
    /// What a pass does with an attachment's contents at its start.
    pub struct LoadAction(u8) {
        /// Keep what is in memory (the common case).
        LOAD = 0,
        /// Fill with the attachment's clear value.
        CLEAR = 1,
        /// The contents are not needed (a discarded or never-read target).
        DONT_CARE = 2,
    }
}

open_enum! {
    /// What a pass does with an attachment's contents at its end.
    pub struct StoreAction(u8) {
        /// Write the tile back to memory.
        STORE = 0,
        /// The contents are not needed afterwards (`DiscardResource`, a
        /// read-only depth view).
        DONT_CARE = 1,
    }
}

open_enum! {
    /// Primitive topology of a draw: the list and strip variants. Adjacency,
    /// patch and rectangle lists are not in the set.
    pub struct Topology(u32) {
        POINT_LIST = 1,
        LINE_LIST = 2,
        LINE_STRIP = 3,
        TRIANGLE_LIST = 4,
        TRIANGLE_STRIP = 5,
    }
}

open_enum! {
    /// Index size of an index buffer.
    pub struct IndexFormat(u32) {
        U16 = 0,
        U32 = 1,
    }
}

/// `CLEAR_ATTACHMENT.flags`: clear the depth plane.
pub const CLEAR_DEPTH: u32 = 1;
/// `CLEAR_ATTACHMENT.flags`: clear the stencil plane.
pub const CLEAR_STENCIL: u32 = 2;

wire_struct! {
    /// A colour attachment of a pass. `res` null = the slot is unused.
    /// A view as a render target sees the texture: format, dimension, one
    /// mip, a range of array slices (`slice_count` 1 for a plain 2D view;
    /// more renders to several slices of an array: layered rendering), or one
    /// W slice of a 3D texture.
    pub struct RenderAttachment {
        res: ResId,
        /// The view's format id (DXGI number): may differ from the
        /// texture's (sRGB views of a typeless texture).
        format: u32,
        dimension: TexViewDim,
        mip: u8,
        load: LoadAction,
        _pad: u8,
        first_slice: u32,
        slice_count: u32,
        /// The clear colour when `load` is `CLEAR` (four floats; the back
        /// converts to the format).
        clear: [f32; 4],
    }
}

wire_struct! {
    /// The depth-stencil attachment of a pass. `res` null = none. The back
    /// attaches the stencil plane when the texture's format has one; its load
    /// is `stencil_load`.
    pub struct DepthAttachment {
        res: ResId,
        format: u32,
        dimension: TexViewDim,
        mip: u8,
        depth_load: LoadAction,
        stencil_load: LoadAction,
        first_slice: u32,
        slice_count: u32,
        clear_depth: f32,
        clear_stencil: u32,
        _pad: u32,
    }
}

wire_struct! {
    /// the D3D viewport.
    pub struct Viewport {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        min_depth: f32,
        max_depth: f32,
    }
}

wire_struct! {
    /// A rectangle as edges (a D3D rect): Metal wants origin and size, the
    /// back converts and clamps to the attachments.
    pub struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
}

wire_struct! {
    /// One vertex buffer binding (a D3D vertex buffer view, address resolved).
    pub struct VertexBufferView {
        va: u64,
        size: u32,
        stride: u32,
    }
}

array_wire!(
    RenderAttachment,
    LoadAction,
    StoreAction,
    Viewport,
    Rect,
    VertexBufferView
);

wire_struct! {
    /// Starts a render pass (an encoder). `rt_count` is the highest used
    /// slot + 1. All attachments must agree on sample count and size
    /// (`INVALID_ARG`). Only the load actions are here; stores come with the end.
    ///
    /// A pass with no attachment at all (draws that only write through
    /// resource views) gives its size in `width` and `height` (1..=16384);
    /// with attachments both are 0 and the attachments' size is the pass's.
    pub struct BeginRenderPass {
        rt_count: u32,
        flags: u32,
        rt: [RenderAttachment; 8],
        ds: DepthAttachment,
        width: u32,
        height: u32,
    }
}

wire_struct! {
    /// Ends the pass: the store action of every attachment.
    pub struct EndRenderPass {
        rt_store: [StoreAction; 8],
        depth_store: StoreAction,
        stencil_store: StoreAction,
        _pad: [u8; 6],
    }
}

wire_struct! {
    /// `RSSetViewports`: the first `count` of `vp`.
    pub struct SetViewports {
        count: u32,
        _pad: u32,
        vp: [Viewport; 16],
    }
}

wire_struct! {
    /// `RSSetScissorRects`: the first `count` of `rect`.
    pub struct SetScissors {
        count: u32,
        _pad: u32,
        rect: [Rect; 16],
    }
}

wire_struct! {
    /// `OMSetBlendFactor`.
    pub struct SetBlendFactor {
        color: [f32; 4],
    }
}

wire_struct! {
    /// `OMSetStencilRef`.
    pub struct SetStencilRef {
        value: u32,
        _pad: u32,
    }
}

wire_struct! {
    /// `IASetPrimitiveTopology`.
    pub struct SetPrimitiveTopology {
        topology: Topology,
        _pad: u32,
    }
}

wire_struct! {
    /// `IASetIndexBuffer`: the view's GPU address, size in bytes and format.
    pub struct SetIndexBuffer {
        va: u64,
        size: u32,
        format: IndexFormat,
    }
}

wire_struct! {
    /// `IASetVertexBuffers`: slots `start_slot ..` get the first `count` views.
    pub struct SetVertexBuffers {
        start_slot: u32,
        count: u32,
        vb: [VertexBufferView; 16],
    }
}

wire_struct! {
    /// `DrawInstanced`.
    pub struct Draw {
        vertex_count: u32,
        instance_count: u32,
        start_vertex: u32,
        start_instance: u32,
    }
}

wire_struct! {
    /// `DrawIndexedInstanced`; needs `SET_INDEX_BUFFER` first.
    pub struct DrawIndexed {
        index_count: u32,
        instance_count: u32,
        start_index: u32,
        base_vertex: i32,
        start_instance: u32,
        _pad: u32,
    }
}

wire_struct! {
    /// `DispatchMesh`: object-threadgroup counts when the pipeline has an
    /// amplification stage, mesh-threadgroup counts otherwise.
    pub struct DispatchMesh {
        x: u32,
        y: u32,
        z: u32,
        _pad: u32,
    }
}

wire_struct! {
    /// Clears part of an attachment *inside the open pass* (the back draws a
    /// quad with the value; every other attachment and plane keeps its tile
    /// contents). `target` is a colour slot 0..7 or [`CLEAR_TARGET_DEPTH_STENCIL`];
    /// `flags` are [`CLEAR_DEPTH`] / [`CLEAR_STENCIL`] for the latter.
    /// `rect_count` 0 = the whole attachment.
    pub struct ClearAttachment {
        target: u32,
        flags: u32,
        rect_count: u32,
        clear_stencil: u32,
        color: [f32; 4],
        clear_depth: f32,
        _pad: u32,
        rect: [Rect; 8],
    }
}

impl BeginRenderPass {
    /// The colour attachments in use (up to `rt_count`).
    pub fn attachments(&self) -> &[RenderAttachment] {
        &self.rt[..(self.rt_count as usize).min(MAX_RENDER_TARGETS)]
    }
}

impl Viewport {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Viewport {
            x,
            y,
            width,
            height,
            min_depth: 0.0,
            max_depth: 1.0,
        }
    }
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Rect {
            left,
            top,
            right,
            bottom,
        }
    }
}

impl SetViewports {
    pub fn from_slice(v: &[Viewport]) -> Self {
        let mut s = <Self as Wire>::ZERO;
        let n = v.len().min(MAX_VIEWPORTS);
        s.count = n as u32;
        s.vp[..n].copy_from_slice(&v[..n]);
        s
    }

    /// The viewports in use.
    pub fn viewports(&self) -> &[Viewport] {
        &self.vp[..(self.count as usize).min(MAX_VIEWPORTS)]
    }
}

impl SetScissors {
    pub fn from_slice(v: &[Rect]) -> Self {
        let mut s = <Self as Wire>::ZERO;
        let n = v.len().min(MAX_VIEWPORTS);
        s.count = n as u32;
        s.rect[..n].copy_from_slice(&v[..n]);
        s
    }

    /// The rectangles in use.
    pub fn rects(&self) -> &[Rect] {
        &self.rect[..(self.count as usize).min(MAX_VIEWPORTS)]
    }
}

impl SetVertexBuffers {
    pub fn from_slice(start_slot: u32, v: &[VertexBufferView]) -> Self {
        let mut s = <Self as Wire>::ZERO;
        let n = v.len().min(MAX_VERTEX_BUFFERS);
        s.start_slot = start_slot;
        s.count = n as u32;
        s.vb[..n].copy_from_slice(&v[..n]);
        s
    }

    /// The views in use.
    pub fn views(&self) -> &[VertexBufferView] {
        &self.vb[..(self.count as usize).min(MAX_VERTEX_BUFFERS)]
    }
}

impl ClearAttachment {
    /// The rectangles in use (empty = the whole attachment).
    pub fn rects(&self) -> &[Rect] {
        &self.rect[..(self.rect_count as usize).min(MAX_CLEAR_RECTS)]
    }
}
