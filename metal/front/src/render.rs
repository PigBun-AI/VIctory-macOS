//! Render pass derivation: from the render calls of a D3D12 command list to
//! proto's `BEGIN_RENDER_PASS` / `END_RENDER_PASS`, the draws and the
//! clears (`docs/metal/proto-render-draft.md` section 2, `m4-render.md`).
//!
//! D3D12 has no passes: targets are bound, state is set, draws happen. Metal
//! has passes whose attachments and load actions are fixed at the start, so
//! the recorder here derives them:
//!
//! * **Begin** is lazy: the first draw, mesh dispatch or clear that needs the
//!   pass, never `OMSetRenderTargets` (applications set targets, set state,
//!   change their minds).
//! * A **full clear of a bound view** before the first draw becomes the load
//!   action `CLEAR` of that attachment (`pending` below). With rectangles, or
//!   after the first draw, it is `CLEAR_ATTACHMENT` inside the pass.
//! * A **clear of a view that is not bound** is a pass of its own (begin with
//!   `CLEAR`, end).
//! * **End** is: `OMSetRenderTargets` with another set (the same set again
//!   is nothing), a transition barrier of an attachment, any command that is
//!   not render state or a barrier (copy, dispatch, UAV clear), `Close`.
//! * **Store** is `STORE`. `DiscardResource` of an attachment of the pass
//!   that just ended turns that attachment's store into `DONT_CARE` by
//!   patching the already recorded `END_RENDER_PASS` (the window stays open
//!   until the next command that is not state, a barrier or a discard);
//!   discarded resources also load with `DONT_CARE` in the next pass.
//!
//! Known differences: `OMSetDepthBounds` is accepted and has no effect (Metal
//! has no depth bounds test); a read-only depth view still stores; clears of
//! integer render targets by rectangle are refused by the back.

use std::collections::HashSet;

use victory_metal_proto::render::*;
use victory_metal_proto::wire::Wire;
use victory_metal_proto::{Command, ResId};

use crate::device::{CommandList, Item, ListState};
use crate::hresult::Hr;
use crate::view::TargetView;

/// Commands that neither need nor end a render pass.
pub fn is_state_command(c: &Command<'_>) -> bool {
    matches!(
        c,
        Command::SetPipeline(_)
            | Command::SetRootSignature(_)
            | Command::SetDescriptorHeaps(_)
            | Command::SetRootDescriptor(_)
            | Command::SetRootTable(_)
            | Command::Barrier(_)
    )
}

#[derive(Default)]
struct OpenPass {
    rt_res: [ResId; MAX_RENDER_TARGETS],
    ds_res: ResId,
}

/// Where the last `END_RENDER_PASS` is, while a `DiscardResource` may still
/// patch it.
struct EndWindow {
    item: usize,
    rt_res: [ResId; MAX_RENDER_TARGETS],
    ds_res: ResId,
}

#[derive(Default)]
pub struct RenderRec {
    rt: [Option<TargetView>; MAX_RENDER_TARGETS],
    ds: Option<TargetView>,
    pass: Option<OpenPass>,
    pending_rt: [Option<[f32; 4]>; MAX_RENDER_TARGETS],
    pending_depth: Option<f32>,
    pending_stencil: Option<u32>,
    window: Option<EndWindow>,
    discarded: HashSet<u32>,
}

impl RenderRec {
    fn has_pending(&self) -> bool {
        self.pending_rt.iter().any(Option::is_some)
            || self.pending_depth.is_some()
            || self.pending_stencil.is_some()
    }

    fn rt_count(&self) -> usize {
        self.rt
            .iter()
            .rposition(Option::is_some)
            .map_or(0, |i| i + 1)
    }

    /// Whether the open pass, or a clear waiting for one, uses `res`.
    fn uses(&self, res: ResId) -> bool {
        if let Some(p) = &self.pass
            && (p.ds_res == res || p.rt_res.contains(&res))
        {
            return true;
        }
        (0..MAX_RENDER_TARGETS).any(|i| self.pending_rt[i].is_some() && self.rt_res(i) == res)
            || ((self.pending_depth.is_some() || self.pending_stencil.is_some())
                && self.ds.is_some_and(|d| d.res == res))
    }

    fn rt_res(&self, i: usize) -> ResId {
        self.rt[i].map_or(ResId::NULL, |t| t.res)
    }
}

fn attachment(tv: &TargetView, load: LoadAction, clear: [f32; 4]) -> RenderAttachment {
    RenderAttachment {
        res: tv.res,
        format: tv.format,
        dimension: tv.dim,
        mip: tv.mip as u8,
        load,
        _pad: 0,
        first_slice: tv.first_slice,
        slice_count: tv.slice_count.max(1),
        clear,
    }
}

fn depth_attachment(
    tv: &TargetView,
    depth_load: LoadAction,
    stencil_load: LoadAction,
    depth: f32,
    stencil: u32,
) -> DepthAttachment {
    DepthAttachment {
        res: tv.res,
        format: tv.format,
        dimension: tv.dim,
        mip: tv.mip as u8,
        depth_load,
        stencil_load,
        first_slice: tv.first_slice,
        slice_count: tv.slice_count.max(1),
        clear_depth: depth,
        clear_stencil: stencil,
        _pad: 0,
    }
}

impl CommandList {
    /// A command that is part of the render stream: recorded as is.
    fn push_raw(&mut self, c: Command<'static>) {
        if self.state == ListState::Recording {
            self.items.push(Item::Cmd(c));
        }
    }

    /// Ends what a non-render command must not share an encoder with, and
    /// forgets what only holds across state commands.
    pub(crate) fn leave_render(&mut self) {
        self.end_render();
        self.render.window = None;
        self.render.discarded.clear();
    }

    /// Ends the open pass (or the clear-only pass of waiting clears).
    fn end_render(&mut self) {
        if self.render.pass.is_none() && self.render.has_pending() {
            self.ensure_pass();
        }
        let Some(p) = self.render.pass.take() else {
            return;
        };
        let item = self.items.len();
        self.push_raw(Command::EndRenderPass(<EndRenderPass as Wire>::ZERO));
        self.render.window = Some(EndWindow {
            item,
            rt_res: p.rt_res,
            ds_res: p.ds_res,
        });
    }

    /// [`Self::ensure_pass`] for the indirect draws of `indirect.rs`.
    pub(crate) fn ensure_render_pass(&mut self) -> bool {
        self.ensure_pass()
    }

    /// Begins the pass of the bound targets if none is open; false when
    /// there is nothing to render to.
    fn ensure_pass(&mut self) -> bool {
        if self.render.pass.is_some() {
            return true;
        }
        let n = self.render.rt_count();
        if n == 0 && self.render.ds.is_none() {
            self.unsupported();
            return false;
        }
        let r = &mut self.render;
        let mut b = <BeginRenderPass as Wire>::ZERO;
        b.rt_count = n as u32;
        let mut open = OpenPass::default();
        for i in 0..n {
            let Some(tv) = r.rt[i] else { continue };
            let (load, clear) = match r.pending_rt[i].take() {
                Some(c) => (LoadAction::CLEAR, c),
                None if r.discarded.remove(&tv.res.0) => (LoadAction::DONT_CARE, [0.0; 4]),
                None => (LoadAction::LOAD, [0.0; 4]),
            };
            b.rt[i] = attachment(&tv, load, clear);
            open.rt_res[i] = tv.res;
        }
        if let Some(tv) = r.ds {
            let discarded = r.discarded.remove(&tv.res.0);
            let load = |pending: bool| match (pending, discarded) {
                (true, _) => LoadAction::CLEAR,
                (false, true) => LoadAction::DONT_CARE,
                (false, false) => LoadAction::LOAD,
            };
            b.ds = depth_attachment(
                &tv,
                load(r.pending_depth.is_some()),
                load(r.pending_stencil.is_some()),
                r.pending_depth.take().unwrap_or(1.0),
                r.pending_stencil.take().unwrap_or(0),
            );
            open.ds_res = tv.res;
        }
        r.pending_rt = Default::default();
        r.pending_depth = None;
        r.pending_stencil = None;
        r.window = None;
        r.pass = Some(open);
        self.push_raw(Command::BeginRenderPass(b));
        true
    }

    /// A transition barrier of a resource the pass (or a clear waiting for
    /// one) uses ends the pass: its contents must reach memory first.
    pub(crate) fn barrier_on_attachment(&mut self, res: ResId) {
        if self.render.uses(res) {
            self.end_render();
        }
    }

    /// A call with a bad argument.
    pub fn fail_invalid(&mut self) {
        self.fail(Hr::E_INVALIDARG);
    }

    /// `OMSetRenderTargets`. `rts[i]` is the view in slot `i` (None = null).
    pub fn set_render_targets(&mut self, rts: [Option<TargetView>; 8], ds: Option<TargetView>) {
        if self.render.rt == rts && self.render.ds == ds {
            return;
        }
        self.end_render();
        self.render.rt = rts;
        self.render.ds = ds;
    }

    fn push_clear(
        &mut self,
        target: u32,
        flags: u32,
        rects: &[[i32; 4]],
        color: [f32; 4],
        depth: f32,
        stencil: u32,
    ) {
        let mut chunks: Vec<&[[i32; 4]]> = rects.chunks(MAX_CLEAR_RECTS).collect();
        if chunks.is_empty() {
            chunks.push(&[]);
        }
        for chunk in chunks {
            let mut c = <ClearAttachment as Wire>::ZERO;
            c.target = target;
            c.flags = flags;
            c.rect_count = chunk.len() as u32;
            c.color = color;
            c.clear_depth = depth;
            c.clear_stencil = stencil;
            for (i, r) in chunk.iter().enumerate() {
                c.rect[i] = Rect::new(r[0], r[1], r[2], r[3]);
            }
            self.push_raw(Command::ClearAttachment(c));
        }
    }

    /// `ClearRenderTargetView`.
    pub fn clear_render_target(
        &mut self,
        view: Option<TargetView>,
        color: [f32; 4],
        rects: &[[i32; 4]],
    ) {
        // A null or unknown descriptor: nothing to clear.
        let Some(tv) = view else { return };
        if tv.depth_stencil {
            return self.fail(Hr::E_INVALIDARG);
        }
        if let Some(i) = self.render.rt.iter().position(|b| *b == Some(tv)) {
            if self.render.pass.is_none() && rects.is_empty() {
                self.render.pending_rt[i] = Some(color);
                return;
            }
            if !self.ensure_pass() {
                return;
            }
            return self.push_clear(i as u32, 0, rects, color, 0.0, 0);
        }
        // Not bound: a pass of its own.
        self.end_render();
        self.clear_only_pass(&tv, 0, rects, color, 0.0, 0);
    }

    /// `ClearDepthStencilView`; `flags` bit 0 depth, bit 1 stencil.
    pub fn clear_depth_stencil(
        &mut self,
        view: Option<TargetView>,
        flags: u32,
        depth: f32,
        stencil: u32,
        rects: &[[i32; 4]],
    ) {
        let Some(tv) = view else { return };
        if !tv.depth_stencil || flags & 3 == 0 || flags & !3 != 0 {
            return self.fail(Hr::E_INVALIDARG);
        }
        if self.render.ds == Some(tv) {
            if self.render.pass.is_none() && rects.is_empty() {
                if flags & 1 != 0 {
                    self.render.pending_depth = Some(depth);
                }
                if flags & 2 != 0 {
                    self.render.pending_stencil = Some(stencil);
                }
                return;
            }
            if !self.ensure_pass() {
                return;
            }
            return self.push_clear(
                CLEAR_TARGET_DEPTH_STENCIL,
                flags,
                rects,
                [0.0; 4],
                depth,
                stencil,
            );
        }
        self.end_render();
        self.clear_only_pass(&tv, flags, rects, [0.0; 4], depth, stencil);
    }

    fn clear_only_pass(
        &mut self,
        tv: &TargetView,
        flags: u32,
        rects: &[[i32; 4]],
        color: [f32; 4],
        depth: f32,
        stencil: u32,
    ) {
        let whole = rects.is_empty();
        let load = |on: bool| {
            if on && whole {
                LoadAction::CLEAR
            } else {
                LoadAction::LOAD
            }
        };
        let mut b = <BeginRenderPass as Wire>::ZERO;
        if tv.depth_stencil {
            b.ds = depth_attachment(
                tv,
                load(flags & 1 != 0),
                load(flags & 2 != 0),
                depth,
                stencil,
            );
        } else {
            b.rt_count = 1;
            b.rt[0] = attachment(tv, load(true), color);
        }
        self.push_raw(Command::BeginRenderPass(b));
        if !whole {
            let target = if tv.depth_stencil {
                CLEAR_TARGET_DEPTH_STENCIL
            } else {
                0
            };
            self.push_clear(target, flags, rects, color, depth, stencil);
        }
        self.push_raw(Command::EndRenderPass(<EndRenderPass as Wire>::ZERO));
    }

    /// `DiscardResource` of a whole resource.
    pub fn discard_resource(&mut self, res: ResId) {
        if self
            .render
            .pass
            .as_ref()
            .is_some_and(|p| p.ds_res == res || p.rt_res.contains(&res))
        {
            self.end_render();
        }
        if let Some(w) = &self.render.window {
            let (item, rt_res, ds_res) = (w.item, w.rt_res, w.ds_res);
            if let Some(Item::Cmd(Command::EndRenderPass(e))) = self.items.get_mut(item) {
                for (i, r) in rt_res.iter().enumerate() {
                    if *r == res {
                        e.rt_store[i] = StoreAction::DONT_CARE;
                    }
                }
                if ds_res == res {
                    e.depth_store = StoreAction::DONT_CARE;
                    e.stencil_store = StoreAction::DONT_CARE;
                }
            }
        }
        self.render.discarded.insert(res.0);
    }

    /// `RSSetViewports`.
    pub fn set_viewports(&mut self, v: &[Viewport]) {
        if v.len() > MAX_VIEWPORTS {
            return self.fail(Hr::E_INVALIDARG);
        }
        self.push_raw(Command::SetViewports(SetViewports::from_slice(v)));
    }

    /// `RSSetScissorRects` (`[left, top, right, bottom]`).
    pub fn set_scissors(&mut self, r: &[[i32; 4]]) {
        if r.len() > MAX_VIEWPORTS {
            return self.fail(Hr::E_INVALIDARG);
        }
        let rs: Vec<Rect> = r
            .iter()
            .map(|r| Rect::new(r[0], r[1], r[2], r[3]))
            .collect();
        self.push_raw(Command::SetScissors(SetScissors::from_slice(&rs)));
    }

    pub fn set_blend_factor(&mut self, color: [f32; 4]) {
        self.push_raw(Command::SetBlendFactor(SetBlendFactor { color }));
    }

    pub fn set_stencil_ref(&mut self, value: u32) {
        self.push_raw(Command::SetStencilRef(SetStencilRef { value, _pad: 0 }));
    }

    /// `IASetPrimitiveTopology` with a `D3D_PRIMITIVE_TOPOLOGY` number; the
    /// five list and strip kinds exist, the rest is not supported.
    pub fn set_topology(&mut self, d3d: i32) {
        let topology = match d3d {
            1 => Topology::POINT_LIST,
            2 => Topology::LINE_LIST,
            3 => Topology::LINE_STRIP,
            4 => Topology::TRIANGLE_LIST,
            5 => Topology::TRIANGLE_STRIP,
            _ => return self.unsupported(),
        };
        self.push_raw(Command::SetPrimitiveTopology(SetPrimitiveTopology {
            topology,
            _pad: 0,
        }));
    }

    /// `IASetIndexBuffer`; a null view unbinds. `dxgi_format` 57 (R16_UINT)
    /// or 42 (R32_UINT).
    pub fn set_index_buffer(&mut self, va: u64, size: u32, dxgi_format: u32) {
        let format = match dxgi_format {
            57 => IndexFormat::U16,
            42 => IndexFormat::U32,
            _ if va == 0 => IndexFormat::U16,
            _ => return self.fail(Hr::E_INVALIDARG),
        };
        self.push_raw(Command::SetIndexBuffer(SetIndexBuffer { va, size, format }));
    }

    /// `IASetVertexBuffers`: `(address, size, stride)` per slot from `start`.
    pub fn set_vertex_buffers(&mut self, start: u32, views: &[(u64, u32, u32)]) {
        if start as usize + views.len() > MAX_VERTEX_BUFFERS {
            return self.fail(Hr::E_INVALIDARG);
        }
        let vb: Vec<VertexBufferView> = views
            .iter()
            .map(|&(va, size, stride)| VertexBufferView { va, size, stride })
            .collect();
        self.push_raw(Command::SetVertexBuffers(SetVertexBuffers::from_slice(
            start, &vb,
        )));
    }

    /// `DrawInstanced`.
    pub fn draw(
        &mut self,
        vertex_count: u32,
        instance_count: u32,
        start_vertex: u32,
        start_instance: u32,
    ) {
        if self.ensure_pass() {
            self.push_raw(Command::Draw(Draw {
                vertex_count,
                instance_count,
                start_vertex,
                start_instance,
            }));
        }
    }

    /// `DrawIndexedInstanced`.
    pub fn draw_indexed(
        &mut self,
        index_count: u32,
        instance_count: u32,
        start_index: u32,
        base_vertex: i32,
        start_instance: u32,
    ) {
        if self.ensure_pass() {
            self.push_raw(Command::DrawIndexed(DrawIndexed {
                index_count,
                instance_count,
                start_index,
                base_vertex,
                start_instance,
                _pad: 0,
            }));
        }
    }

    /// `DispatchMesh`.
    pub fn dispatch_mesh(&mut self, x: u32, y: u32, z: u32) {
        if self.ensure_pass() {
            self.push_raw(Command::DispatchMesh(DispatchMesh { x, y, z, _pad: 0 }));
        }
    }
}
