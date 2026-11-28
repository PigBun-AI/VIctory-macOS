//! Emulated mesh draws in a stream (docs/metal/mesh-compute.md, section 8).
//!
//! A mesh pipeline that `CREATE_GRAPHICS_PIPELINE` made as an emulation
//! ([`PsoBody::Emulated`]: a compute form and a vertex-fetch pipeline, see
//! [`crate::mesh_emu`]) runs as two steps: the compute work, then a draw that
//! reads what it wrote. Metal cannot take the compute work inside a render
//! encoder, so it is **hoisted**:
//!
//! * **Pre-scan.** When a render pass begins, the commands up to its end are
//!   looked at (the front ends a pass at every dispatch, copy or clear, so a
//!   pass holds render state, draws and barriers: the look-ahead is
//!   self-contained). The root arguments are followed on a copy of the
//!   graphics side's shadow, and for each emulated `DispatchMesh` or
//!   mesh `ExecuteIndirect` the compute work is encoded now, into **one
//!   compute encoder before the pass's render encoder**; the draw's
//!   ingredients wait in [`Bound::emu`] for the draw command.
//! * **The pass-split rule.** A kernel that runs before the pass cannot see
//!   what an earlier draw of the same pass writes through a UAV. If the draw's
//!   kernel may read a resource (by identity: root descriptors and the
//!   ranges of descriptor tables) that an earlier draw of the pass may have
//!   written, the draw is not hoisted: its kernel is encoded right before the
//!   draw, which ends the render encoder and starts a second one that
//!   continues the pass. Accesses the back cannot resolve (a heap-indexed
//!   root signature on either side) are counted as `unproven` and do not split
//!   unless `VICTORY_METAL_MESH_COMPUTE_SPLIT=unproven`.
//! * **Whatever the pre-scan could not see** (a command it does not look past,
//!   a descriptor heap change in the pass) is done in place, as a split.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use victory_metal_proto::cmd::Command;
use victory_metal_proto::indirect::{ExecuteIndirect, IndirectArgKind};
use victory_metal_proto::render::DispatchMesh;
use victory_metal_proto::root::{APPEND, RootSigDesc, UNBOUNDED, root_flags};
use victory_metal_proto::status::Status;
use victory_metal_proto::stream::Decoder;
use victory_metal_proto::types::{BindPoint, RangeKind, RootParamKind};

use super::stream::{R, Run, Side};
use super::tlab::SlotKind;
use super::{Pso, PsoBody, RootSig, State, status_of};
use crate::descriptors::ENTRY_SIZE;
use crate::mesh_emu::{self, Direct, Emulated, Event, Fetch};
use crate::stages::{COMPUTE_ENCODER_STAGES, DISPATCH, RENDER_ENCODER_STAGES, VIS_DEVICE};

/// What the pre-scan decided for one emulated draw (one per draw, in order).
pub(super) enum Prepared {
    /// The kernel ran before the render encoder; the draw reads this.
    Direct(Direct),
    /// The same for an `ExecuteIndirect`: one draw per record.
    Indirect(Vec<Fetch>),
    /// Not hoisted: the kernel is encoded at the draw (`split`: the rule said
    /// so; otherwise the pre-scan did not get there).
    Inline { split: bool },
    /// Nothing to draw (no groups, or the dispatch was refused: the reason,
    /// empty for "no groups").
    Dropped(String),
}

/// Most entries of one descriptor range looked at by the pass-split rule.
const RANGE_LOOK: u32 = 512;

/// What the draws of a pass may touch, by identity.
#[derive(Default)]
struct Touch {
    srv: HashSet<u64>,
    uav: HashSet<u64>,
    /// The root signature indexes the heap directly: anything may be read /
    /// written.
    heap_srv: bool,
    heap_uav: bool,
}

/// The pre-scan's copy of the graphics side and what the pass wrote so far.
struct Sim {
    gfx: Side,
    gpso: Option<Arc<Pso>>,
    written: HashSet<u64>,
    heap_written: bool,
    /// The heaps bound at this point of the pass, and when the pre-scan began
    /// (the heaps the kernels are encoded with).
    heaps: (u32, u32),
    start_heaps: (u32, u32),
}

#[derive(PartialEq, Eq)]
enum Conflict {
    None,
    Definite,
    Unproven,
}

fn key_of(st: &State, va: u64) -> u64 {
    st.device.resolve_va(va, 1).map_or(va, |(base, _)| base)
}

fn shadow_u64(shadow: &[u8], at: u32) -> u64 {
    shadow
        .get(at as usize..at as usize + 8)
        .map_or(0, |b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
}

/// The resources the root arguments of `side` name: root descriptors, and the
/// entries of the ranges of the descriptor tables.
fn touched(st: &State, side: &Side) -> Touch {
    let mut t = Touch::default();
    let Some(sig) = &side.sig else { return t };
    let desc: &RootSigDesc = &sig.desc;
    // Entries the scan could not look at (counted as `silent.emu.touched.*`:
    // the touch set it hands back is short by these).
    let mut no_key = 0u64;
    if desc.flags & root_flags::CBV_SRV_UAV_HEAP_INDEXED != 0 {
        t.heap_srv = true;
        t.heap_uav = true;
    }
    for (i, p) in desc.params.iter().enumerate() {
        let Some(slot) = sig.layout.slot(i as u32) else {
            crate::dropstats::silent("emu.touched", "slot_unknown");
            continue;
        };
        match (p.kind, slot.kind) {
            (RootParamKind::CBV | RootParamKind::SRV, SlotKind::Descriptor(_)) => {
                let va = shadow_u64(&side.shadow, slot.offset);
                if va != 0 {
                    t.srv.insert(key_of(st, va));
                }
            }
            (RootParamKind::UAV, SlotKind::Descriptor(_)) => {
                let va = shadow_u64(&side.shadow, slot.offset);
                if va != 0 {
                    t.uav.insert(key_of(st, va));
                }
            }
            (RootParamKind::TABLE, SlotKind::Table { sampler: false }) => {
                let va = shadow_u64(&side.shadow, slot.offset);
                let Some(h) = st.heaps.values().find(|h| {
                    !h.sampler
                        && va >= h.heap.gpu_va()
                        && va < h.heap.gpu_va() + u64::from(h.heap.capacity()) * ENTRY_SIZE
                }) else {
                    crate::dropstats::silent("emu.touched", "heap_unknown");
                    continue;
                };
                let start = ((va - h.heap.gpu_va()) / ENTRY_SIZE) as u32;
                let cap = h.heap.capacity();
                let from = p.range_start as usize;
                let ranges = desc
                    .ranges
                    .get(from..from + p.range_count as usize)
                    .unwrap_or(&[]);
                let mut next = 0u32;
                for r in ranges {
                    let at = if r.offset_in_table == APPEND {
                        next
                    } else {
                        r.offset_in_table
                    };
                    let n = if r.num_descriptors == UNBOUNDED {
                        cap.saturating_sub(start.saturating_add(at))
                    } else {
                        r.num_descriptors
                    };
                    next = at.saturating_add(n);
                    if r.kind == RangeKind::SAMPLER {
                        continue;
                    }
                    for k in 0..n.min(RANGE_LOOK) {
                        let idx = start.saturating_add(at).saturating_add(k);
                        if idx >= cap {
                            break;
                        }
                        // What the view looks at: recorded when it was written
                        // (a texture's entry holds a view id, not the texture).
                        let Some(&key) = h.keys.get(&idx) else {
                            no_key += 1;
                            continue;
                        };
                        if r.kind == RangeKind::UAV {
                            t.uav.insert(key);
                        } else {
                            t.srv.insert(key);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    crate::dropstats::silent_n("emu.touched", "key_unknown", no_key);
    t
}

impl Sim {
    /// Whether the heaps bound at this draw differ from the ones the kernels
    /// of the pre-scan are encoded with in a way the draw's kernel can see:
    /// tables are addresses, so only a root signature that indexes the heap
    /// directly (the heap's address is bound, not passed) does.
    fn heaps_matter(&self) -> bool {
        let Some(sig) = &self.gfx.sig else {
            return false;
        };
        let flags = sig.desc.flags;
        (flags & root_flags::CBV_SRV_UAV_HEAP_INDEXED != 0 && self.heaps.0 != self.start_heaps.0)
            || (flags & root_flags::SAMPLER_HEAP_INDEXED != 0 && self.heaps.1 != self.start_heaps.1)
    }

    /// A draw of the pass: what it may write joins the pass's writes.
    fn note_draw(&mut self, st: &State) {
        let t = touched(st, &self.gfx);
        self.written.extend(t.uav);
        self.heap_written |= t.heap_uav;
    }

    /// Whether the kernel of an emulated draw (reading what the graphics
    /// side binds now) may read what the pass wrote before.
    fn conflict(&self, st: &State, em: &Emulated) -> Conflict {
        let t = touched(st, &self.gfx);
        if t.srv.iter().any(|k| self.written.contains(k)) {
            return Conflict::Definite;
        }
        let heap_reads = t.heap_srv && em.layout.reads != 0;
        let any_write = !self.written.is_empty() || self.heap_written;
        if (heap_reads && any_write) || (self.heap_written && !t.srv.is_empty()) {
            return Conflict::Unproven;
        }
        Conflict::None
    }
}

fn graphics(b: BindPoint) -> bool {
    b == BindPoint::GRAPHICS
}

impl Run<'_> {
    /// The emulated pipeline the open list has bound, with its `Pso`.
    fn emulated_bound(&mut self) -> Option<(Arc<Pso>, Arc<Emulated>)> {
        let (_, _, bound) = self.open.as_ref()?;
        let pso = bound.gpso.clone()?;
        match &pso.body {
            PsoBody::Emulated(em) => {
                let em = em.clone();
                Some((pso, em))
            }
            _ => None,
        }
    }

    /// The compute work of `groups` groups, encoded now (into the compute
    /// encoder the list has, or a new one).
    fn encode_direct(
        &mut self,
        side: &Side,
        em: &Emulated,
        groups: [u32; 3],
    ) -> Result<Direct, Status> {
        let r = self.encode_direct_inner(side, em, groups);
        if let Err((_, scratch)) = &r {
            self.ev(if *scratch {
                Event::DroppedScratch
            } else {
                Event::DroppedOther
            });
        }
        r.map_err(|(s, _)| s)
    }

    /// An event of the emulation: counters, the list's stats, drop statistics.
    fn ev(&mut self, e: Event) {
        let list = self.open.as_mut().map(|(l, _, _)| l);
        mesh_emu::event(list, e);
    }

    fn encode_direct_inner(
        &mut self,
        side: &Side,
        em: &Emulated,
        groups: [u32; 3],
    ) -> Result<Direct, (Status, bool)> {
        let static_va = side
            .sig
            .as_ref()
            .and_then(|s| s.static_table.as_ref())
            .map(|t| t.gpu_va());
        let Some((list, _, _)) = self.open.as_mut() else {
            return Err((Status::BAD_STREAM, false));
        };
        list.emu_direct(em, &side.shadow, static_va, groups)
            .map_err(|e| {
                self.st.note = Some(format!("emulated DISPATCH_MESH: {e}"));
                (status_of(&e), mesh_emu::is_scratch_error(&e))
            })
    }

    /// The compute work of an indirect mesh dispatch, encoded now.
    fn encode_indirect(
        &mut self,
        side: &Side,
        em: &Emulated,
        c: &ExecuteIndirect<'_>,
    ) -> Result<Vec<Fetch>, Status> {
        let r = self.encode_indirect_inner(side, em, c);
        match &r {
            Ok(_) => self.ev(Event::Indirect),
            Err(_) => self.ev(Event::DroppedOther),
        }
        r
    }

    fn encode_indirect_inner(
        &mut self,
        side: &Side,
        em: &Emulated,
        c: &ExecuteIndirect<'_>,
    ) -> Result<Vec<Fetch>, Status> {
        let args = self.st.buffer(c.args)?.clone();
        let count = if c.count.is_null() {
            None
        } else {
            Some((self.st.buffer(c.count)?.clone(), c.count_offset))
        };
        let sig = side.sig.clone().ok_or(Status::INVALID_ARG)?;
        if !c.root_sig.is_null() {
            let made_for = self
                .st
                .root_sigs
                .get(c.root_sig.0)
                .ok_or(Status::NOT_FOUND)?
                .clone();
            if !made_for.compatible_with(&sig) {
                return Err(Status::INVALID_ARG);
            }
        }
        let plan = super::indirect::plan(c, Some(&sig))?;
        if plan.action != super::indirect::Action::Mesh {
            return Err(Status::INVALID_ARG);
        }
        let static_va = sig.static_table.as_ref().map(|t| t.gpu_va());
        let spec = crate::indirect::IndirectDispatch {
            args: &args,
            args_offset: c.args_offset,
            count: count.as_ref().map(|(b, off)| (b, *off)),
            max_count: c.max_count,
            stride: c.stride,
            dispatch_offset: plan.dispatch_offset,
            record_bytes: plan.record_bytes,
            roots: &plan.roots,
            base: &side.shadow,
            action_words: 3,
            draws: false,
            index_type: 0,
            consumer: DISPATCH,
            gs: None,
            always_expand: true,
        };
        let kernels = &self.st.indirect_kernels;
        let emu_kernels = &self.st.emu_kernels;
        let Some((list, _, _)) = self.open.as_mut() else {
            return Err(Status::BAD_STREAM);
        };
        list.emu_indirect(kernels, emu_kernels, em, &spec, static_va)
            .map_err(|e| {
                self.st.note = Some(format!("emulated EXECUTE_INDIRECT: {e}"));
                status_of(&e)
            })
    }

    /// The pre-scan of the render pass that has just begun: `dec` is the
    /// decoder right after its `BEGIN_RENDER_PASS`.
    pub(super) fn prescan(&mut self, mut dec: Decoder<'_>) {
        let Some((_, _, bound)) = self.open.as_ref() else {
            return;
        };
        let mut sim = Sim {
            gfx: bound.graphics.clone(),
            gpso: bound.gpso.clone(),
            written: HashSet::new(),
            heap_written: false,
            heaps: bound.heaps,
            start_heaps: bound.heaps,
        };
        let mut queue: VecDeque<Prepared> = VecDeque::new();
        let mut hoisted = false;
        let mut ordered = false;
        while let Some(Ok(cmd)) = dec.next() {
            match cmd {
                Command::EndRenderPass(_) => break,
                Command::SetPipeline(c) => {
                    if let Some(p) = self.st.psos.get(c.pso.0)
                        && matches!(p.body, PsoBody::Graphics(_) | PsoBody::Emulated(_))
                    {
                        sim.gpso = Some(p.clone());
                    }
                }
                Command::SetRootSignature(c) if graphics(c.bind_point) => {
                    if let Some(sig) = self.st.root_sigs.get(c.root_sig.0) {
                        sim.gfx.set_signature(sig.clone());
                    }
                }
                Command::SetRootConstants(c) if graphics(c.bind_point) => {
                    let _ = sim.gfx.set_constants(&c);
                }
                Command::SetRootDescriptor(c) if graphics(c.bind_point) => {
                    let known = c.gpu_va == 0
                        || matches!(self.st.device.resolve_va(c.gpu_va, 1), Some((_, e)) if e.resident);
                    if known {
                        let _ = sim.gfx.set_descriptor(&c);
                    }
                }
                Command::SetRootTable(c) if graphics(c.bind_point) => {
                    if let Some(h) = self.st.heaps.get(c.heap.0)
                        && c.index < h.heap.capacity()
                    {
                        let (va, sampler) = (h.heap.entry_va(c.index), h.sampler);
                        let _ = sim.gfx.set_table(&c, va, sampler);
                    }
                }
                // Another bind point's arguments do not matter to a draw.
                Command::SetRootSignature(_)
                | Command::SetRootConstants(_)
                | Command::SetRootDescriptor(_)
                | Command::SetRootTable(_) => {}
                Command::SetDescriptorHeaps(c) => {
                    if !c.cbv_srv_uav.is_null() {
                        sim.heaps.0 = c.cbv_srv_uav.0;
                    }
                    if !c.sampler.is_null() {
                        sim.heaps.1 = c.sampler.0;
                    }
                }
                Command::Draw(_) | Command::DrawIndexed(_) => sim.note_draw(self.st),
                Command::DispatchMesh(c) => {
                    let emulated = sim.gpso.as_ref().and_then(|p| match &p.body {
                        PsoBody::Emulated(em) => Some((p.clone(), em.clone())),
                        _ => None,
                    });
                    let Some((pso, em)) = emulated else {
                        sim.note_draw(self.st);
                        continue;
                    };
                    let heaps_changed = sim.heaps_matter();
                    if !ordered && !heaps_changed {
                        ordered = self.order_before_kernel().is_ok();
                    }
                    let prepared = self.prepare_direct(&sim, &pso, &em, &c, heaps_changed);
                    hoisted |= matches!(prepared, Prepared::Direct(_));
                    queue.push_back(prepared);
                    sim.note_draw(self.st);
                }
                Command::ExecuteIndirect(c) => {
                    let mesh = c
                        .arguments()
                        .any(|a| a.kind == IndirectArgKind::DISPATCH_MESH);
                    let emulated = sim.gpso.as_ref().and_then(|p| match &p.body {
                        PsoBody::Emulated(em) => Some((p.clone(), em.clone())),
                        _ => None,
                    });
                    match (mesh, emulated) {
                        (true, Some((pso, em))) => {
                            let heaps_changed = sim.heaps_matter();
                            if !ordered && !heaps_changed {
                                ordered = self.order_before_kernel().is_ok();
                            }
                            let prepared =
                                self.prepare_indirect(&sim, &pso, &em, &c, heaps_changed);
                            hoisted |= matches!(prepared, Prepared::Indirect(_));
                            queue.push_back(prepared);
                            sim.note_draw(self.st);
                        }
                        (true, None) => sim.note_draw(self.st),
                        // A compute record in a pass is not something the
                        // front makes: stop looking.
                        (false, _)
                            if !c.arguments().any(|a| {
                                a.kind.is_action() && a.kind != IndirectArgKind::DISPATCH
                            }) =>
                        {
                            break;
                        }
                        (false, _) => sim.note_draw(self.st),
                    }
                }
                Command::Barrier(_)
                | Command::SetViewports(_)
                | Command::SetScissors(_)
                | Command::SetBlendFactor(_)
                | Command::SetStencilRef(_)
                | Command::SetPrimitiveTopology(_)
                | Command::SetIndexBuffer(_)
                | Command::SetVertexBuffers(_)
                | Command::ClearAttachment(_)
                | Command::DebugLabel(_) => {}
                // Anything else is not in a pass: what follows is done in place.
                _ => break,
            }
        }
        if let Some((list, _, bound)) = self.open.as_mut() {
            if hoisted {
                // What the kernels wrote is visible to the draws.
                if let Err(e) = list.emu_publish() {
                    self.st.note = Some(format!("emulated mesh draws: {e}"));
                }
            }
            // The argument table now points at the last kernel's arguments.
            bound.owner = None;
            bound.emu = queue;
        }
    }

    fn prepare_direct(
        &mut self,
        sim: &Sim,
        pso: &Arc<Pso>,
        em: &Arc<Emulated>,
        c: &DispatchMesh,
        heaps_changed: bool,
    ) -> Prepared {
        if c.x == 0 || c.y == 0 || c.z == 0 {
            return Prepared::Dropped(String::new());
        }
        let Some(sig) = sim.gfx.sig.clone() else {
            return Prepared::Dropped("no root signature bound".into());
        };
        if !pso.root_sig.compatible_with(&sig) {
            return Prepared::Dropped("the pipeline was made for another root signature".into());
        }
        if heaps_changed {
            return Prepared::Inline { split: false };
        }
        match sim.conflict(self.st, em) {
            Conflict::Definite => return Prepared::Inline { split: true },
            Conflict::Unproven => {
                self.ev(Event::Unproven);
                if mesh_emu::split_on_unproven() {
                    return Prepared::Inline { split: true };
                }
            }
            Conflict::None => {}
        }
        match self.encode_direct(&sim.gfx, em, [c.x, c.y, c.z]) {
            Ok(d) => {
                self.ev(Event::Hoisted);
                Prepared::Direct(d)
            }
            Err(s) => Prepared::Dropped(format!("emulated DISPATCH_MESH: {s:?}")),
        }
    }

    fn prepare_indirect(
        &mut self,
        sim: &Sim,
        pso: &Arc<Pso>,
        em: &Arc<Emulated>,
        c: &ExecuteIndirect<'_>,
        heaps_changed: bool,
    ) -> Prepared {
        let Some(sig) = sim.gfx.sig.clone() else {
            return Prepared::Dropped("no root signature bound".into());
        };
        if !pso.root_sig.compatible_with(&sig) {
            return Prepared::Dropped("the pipeline was made for another root signature".into());
        }
        if heaps_changed {
            return Prepared::Inline { split: false };
        }
        match sim.conflict(self.st, em) {
            Conflict::Definite => return Prepared::Inline { split: true },
            Conflict::Unproven => {
                self.ev(Event::Unproven);
                if mesh_emu::split_on_unproven() {
                    return Prepared::Inline { split: true };
                }
            }
            Conflict::None => {}
        }
        match self.encode_indirect(&sim.gfx, em, c) {
            Ok(f) => {
                self.ev(Event::Hoisted);
                Prepared::Indirect(f)
            }
            Err(s) => Prepared::Dropped(format!("emulated EXECUTE_INDIRECT: {s:?}")),
        }
    }

    /// Before an in-place kernel: what the pass drew so far is finished and
    /// visible to it.
    fn order_before_kernel(&mut self) -> R {
        let (list, _, _) = self.open.as_mut().ok_or(Status::BAD_STREAM)?;
        // The application's barriers name the stages of its own draw; the
        // kernel is a dispatch, so everything before it is ordered before it.
        list.barrier(
            RENDER_ENCODER_STAGES | COMPUTE_ENCODER_STAGES,
            DISPATCH,
            VIS_DEVICE,
        )
        .map_err(|e| status_of(&e))
    }

    /// `DISPATCH_MESH` with an emulated pipeline bound: `None` when the
    /// pipeline is not one.
    pub(super) fn emu_mesh_command(&mut self, c: &DispatchMesh) -> Option<R> {
        let (pso, em) = self.emulated_bound()?;
        Some(self.emu_mesh(&pso, &em, c))
    }

    fn emu_mesh(&mut self, pso: &Arc<Pso>, em: &Arc<Emulated>, c: &DispatchMesh) -> R {
        let (_, bound) = self.list()?;
        let prepared = bound.emu.pop_front();
        let side = bound.graphics.clone();
        let sig = side.sig.clone().ok_or(Status::INVALID_ARG)?;
        if !pso.root_sig.compatible_with(&sig) {
            return Err(Status::INVALID_ARG);
        }
        if c.x == 0 || c.y == 0 || c.z == 0 {
            return Ok(());
        }
        let r = match prepared {
            Some(Prepared::Direct(d)) => {
                let (list, bound) = self.list()?;
                let r = list.emu_draw(em, &d);
                bound.owner = None;
                r.map_err(|e| self.st.refuse("DISPATCH_MESH", &e))
            }
            Some(Prepared::Dropped(why)) => {
                if !why.is_empty() {
                    crate::dropstats::skip(&why);
                }
                Ok(())
            }
            Some(Prepared::Indirect(_)) => Err(Status::INVALID_ARG),
            Some(Prepared::Inline { split }) => self.emu_mesh_in_place(em, &side, c, split),
            None => self.emu_mesh_in_place(em, &side, c, false),
        };
        if let Some((_, _, bound)) = self.open.as_mut() {
            bound.owner = None;
            bound.graphics.dirty = true;
        }
        r
    }

    /// The kernel right before its draw: the render encoder ends and the pass
    /// continues in a second one.
    fn emu_mesh_in_place(
        &mut self,
        em: &Arc<Emulated>,
        side: &Side,
        c: &DispatchMesh,
        split: bool,
    ) -> R {
        if split {
            self.ev(Event::Split);
        } else {
            self.ev(Event::InPlace);
        }
        self.order_before_kernel()?;
        let d = self.encode_direct(side, em, [c.x, c.y, c.z])?;
        let (list, _, _) = self.open.as_mut().ok_or(Status::BAD_STREAM)?;
        list.emu_publish().map_err(|e| status_of(&e))?;
        list.emu_draw(em, &d)
            .map_err(|e| self.st.refuse("DISPATCH_MESH", &e))
    }

    /// `EXECUTE_INDIRECT` whose record ends with a mesh dispatch, with an
    /// emulated pipeline bound: `None` when it is not one.
    pub(super) fn emu_indirect_command(&mut self, c: &ExecuteIndirect<'_>) -> Option<R> {
        if !c
            .arguments()
            .any(|a| a.kind == IndirectArgKind::DISPATCH_MESH)
        {
            return None;
        }
        let (pso, em) = self.emulated_bound()?;
        Some(self.emu_indirect_run(&pso, &em, c))
    }

    fn emu_indirect_run(
        &mut self,
        pso: &Arc<Pso>,
        em: &Arc<Emulated>,
        c: &ExecuteIndirect<'_>,
    ) -> R {
        let (_, bound) = self.list()?;
        let prepared = bound.emu.pop_front();
        let side = bound.graphics.clone();
        let sig = side.sig.clone().ok_or(Status::INVALID_ARG)?;
        if !pso.root_sig.compatible_with(&sig) {
            return Err(Status::INVALID_ARG);
        }
        let r = match prepared {
            Some(Prepared::Indirect(f)) => self.emu_draw_records(em, &f),
            Some(Prepared::Dropped(why)) => {
                if !why.is_empty() {
                    crate::dropstats::skip(&why);
                }
                Ok(())
            }
            Some(Prepared::Direct(_)) => Err(Status::INVALID_ARG),
            other => {
                if matches!(other, Some(Prepared::Inline { split: true })) {
                    self.ev(Event::Split);
                } else {
                    self.ev(Event::InPlace);
                }
                self.order_before_kernel()?;
                let f = self.encode_indirect(&side, em, c)?;
                let (list, _, _) = self.open.as_mut().ok_or(Status::BAD_STREAM)?;
                list.emu_publish().map_err(|e| status_of(&e))?;
                self.emu_draw_records(em, &f)
            }
        };
        if let Some((_, _, bound)) = self.open.as_mut() {
            bound.owner = None;
            bound.graphics.dirty = true;
        }
        r
    }

    fn emu_draw_records(&mut self, em: &Arc<Emulated>, f: &[Fetch]) -> R {
        let (list, _, _) = self.open.as_mut().ok_or(Status::BAD_STREAM)?;
        for r in f {
            list.emu_draw_record(em, r)
                .map_err(|e| self.st.refuse("EXECUTE_INDIRECT", &e))?;
        }
        Ok(())
    }
}

/// The root signature the extension is made over: its argument buffer.
pub(super) fn app_layout(sig: &RootSig) -> mesh_emu::AppLayout {
    mesh_emu::AppLayout {
        offsets: (0..sig.desc.params.len() as u32)
            .map(|i| sig.layout.slot(i).map_or(0, |s| s.offset))
            .collect(),
        size: sig.layout.size(),
        static_table: sig.layout.static_table_offset(),
    }
}

/// Whether a refusal of a mesh pipeline is the kind the emulation answers:
/// over Metal's output size after the repack, or Apple's GPU compiler crashes
/// on it (seen in this process, or on the census's denylist).
pub(super) fn is_problem_pair(e: &crate::msc::MscError) -> bool {
    mesh_emu::is_problem_refusal(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msc::MscError;

    #[test]
    fn the_three_problems_are_told_apart_from_other_refusals() {
        assert!(is_problem_pair(&MscError::Limit(
            "mesh pipeline over a Metal limit: Total mesh size 40000".into()
        )));
        assert!(is_problem_pair(&MscError::Unsupported(
            "mesh pipeline: this mesh and pixel shader crash Apple's GPU compiler".into()
        )));
        assert!(is_problem_pair(&MscError::Unsupported(
            "mesh pipeline: Apple's GPU compiler crashed on this mesh and pixel shader".into()
        )));
        assert!(!is_problem_pair(&MscError::Unsupported(
            "logic operations on render targets".into()
        )));
        assert!(!is_problem_pair(&MscError::Limit(
            "too many threads".into()
        )));
        assert!(!is_problem_pair(&MscError::BadInput("x".into())));
    }
}
