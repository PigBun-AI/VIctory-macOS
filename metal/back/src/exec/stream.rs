//! The command stream (`Link::submit`): checking a block, then running it.
//!
//! **Pass 1, [`validate`]**, looks at the block as a whole and touches no
//! state: the framing (the decoder), the nesting of lists, duplicate list
//! ids, commands in the wrong place, and the opcodes and variants this back
//! does not implement. A block that fails has no effect at all.
//!
//! **Pass 2, `State::run_block`**, executes. What it can still find wrong
//! is about state (dead handles, values out of range, a descriptor that
//! names no live buffer): it stops at the first such command, drops the
//! list being recorded and the lists not yet handed to a queue, and returns
//! the status; lists already submitted stay submitted.
//!
//! Lists are recorded one after another (no nesting). Ended lists are
//! gathered per queue and committed together (one [`Queue::submit`]) before
//! a `QUEUE_SIGNAL` / `QUEUE_WAIT` on that queue and at the end of the block.
//! That is one `ExecuteCommandLists`.

use std::collections::HashSet;
use std::sync::Arc;

use victory_metal_proto::cmd::{
    BeginList, Command, CopyBufferRegion, CopyTextureRegion, Dispatch, FillBuffer, QueueSignal,
    QueueWait, SetDescriptorHeaps, SetPipeline, SetRootConstants, SetRootDescriptor,
    SetRootSignature, SetRootTable,
};
use victory_metal_proto::ids::{HeapId, QueueId};
use victory_metal_proto::status::Status;
use victory_metal_proto::stream::Decoder;
use victory_metal_proto::types::{BarrierClass, BindPoint, RootParamKind, format};

use super::tlab::SlotKind;
use super::{Pso, PsoBody, RootSig, State, status_of};
use crate::queue::Queue;
use crate::record::CommandList;

/// Checks everything about a block that does not need the object tables.
/// Returns the stream's capability bits.
pub(super) fn validate(block: &[u8]) -> Result<victory_metal_proto::Caps, Status> {
    let decoder = Decoder::new(block).map_err(|e| e.status())?;
    let caps = decoder.header().caps;
    let mut open: Option<u32> = None;
    let mut seen = HashSet::new();
    for c in decoder {
        let c = c.map_err(|e| e.status())?;
        match &c {
            Command::BeginList(b) => {
                if open.is_some() || b.list.is_null() || !seen.insert(b.list.0) {
                    return Err(Status::BAD_STREAM);
                }
                open = Some(b.list.0);
            }
            Command::EndList(e) => {
                if open != Some(e.list.0) {
                    return Err(Status::BAD_STREAM);
                }
                open = None;
            }
            Command::QueueSignal(_) | Command::QueueWait(_) => {
                if open.is_some() {
                    return Err(Status::BAD_STREAM);
                }
            }
            Command::DebugLabel(_) => {}
            // Not implemented by this back (the status is the same wherever
            // they stand, so they are refused before anything runs).
            Command::SyncRecord(_) | Command::BufferData(_) | Command::DispatchIndirect(_) => {
                return Err(Status::UNSUPPORTED);
            }
            other => {
                if open.is_none() {
                    return Err(Status::BAD_STREAM);
                }
                match other {
                    Command::SetRootSignature(c) if c.bind_point != BindPoint::COMPUTE => {
                        return Err(Status::UNSUPPORTED);
                    }
                    Command::SetRootConstants(c) if c.bind_point != BindPoint::COMPUTE => {
                        return Err(Status::UNSUPPORTED);
                    }
                    Command::SetRootDescriptor(c) if c.bind_point != BindPoint::COMPUTE => {
                        return Err(Status::UNSUPPORTED);
                    }
                    Command::SetRootTable(c) if c.bind_point != BindPoint::COMPUTE => {
                        return Err(Status::UNSUPPORTED);
                    }
                    // Draws, mesh dispatch, vertex and index buffers: waiting for the render encoder.
                    Command::ExecuteIndirect(c)
                        if c.arguments()
                            .any(|a| !super::indirect::is_compute_arg(a.kind)) =>
                    {
                        return Err(Status::UNSUPPORTED);
                    }
                    Command::Barrier(b) if b.class == BarrierClass::LAYOUT => {
                        return Err(Status::UNSUPPORTED);
                    }
                    _ => {}
                }
            }
        }
    }
    if open.is_some() {
        return Err(Status::BAD_STREAM);
    }
    Ok(caps)
}

/// What a list has bound so far: the shadow of its argument buffer and the
/// objects a dispatch needs.
#[derive(Default)]
struct Bound {
    sig: Option<Arc<RootSig>>,
    pso: Option<Arc<Pso>>,
    /// The argument buffer as the stream has set it (root constants,
    /// pointers), `layout.size` bytes.
    shadow: Vec<u8>,
    /// Shadow changed since the last dispatch wrote it to the ring.
    dirty: bool,
}

/// A queue with the lists ended for it and not yet committed.
struct Batch {
    queue_id: u32,
    queue: Queue,
    lists: Vec<CommandList>,
}

struct Run<'a> {
    st: &'a mut State,
    batches: Vec<Batch>,
    /// The list being recorded.
    open: Option<(CommandList, QueueId, Bound)>,
}

impl State {
    /// Runs a block that [`validate`] accepted.
    pub(super) fn run_block(&mut self, block: &[u8]) -> Status {
        let Ok(decoder) = Decoder::new(block) else {
            return Status::BAD_STREAM;
        };
        let mut run = Run {
            st: self,
            batches: Vec::new(),
            open: None,
        };
        let mut failed: Option<(Status, String)> = None;
        for c in decoder {
            let Ok(c) = c else {
                return Status::BAD_STREAM;
            };
            if let Err(s) = run.command(&c) {
                // Dropping `run` drops the open list and the unflushed ones.
                failed = Some((s, format!("{c:?}")));
                break;
            }
        }
        let result = match failed {
            Some((s, what)) => Err((s, what)),
            None => run.flush_all().map_err(|s| (s, "commit".to_string())),
        };
        drop(run);
        match result {
            Ok(()) => Status::OK,
            Err((s, what)) => {
                // The wire carries a status only; `ExecuteCommandLists` has no
                // result at all, so the command that stopped the block is
                // kept for `VICTORY_METAL_LOG` and `Executor::last_error`.
                self.note = Some(format!("block stopped with {s:?} at {what}"));
                s
            }
        }
    }
}

type R = Result<(), Status>;

impl Run<'_> {
    fn command(&mut self, c: &Command<'_>) -> R {
        match c {
            Command::BeginList(c) => self.begin_list(c),
            Command::EndList(_) => self.end_list(),
            Command::QueueSignal(c) => self.queue_signal(c),
            Command::QueueWait(c) => self.queue_wait(c),
            // Labels have no Metal 4 counterpart wired up yet: accepted, dropped.
            Command::DebugLabel(_) => Ok(()),
            Command::SetPipeline(c) => self.set_pipeline(c),
            Command::SetRootSignature(c) => self.set_root_signature(c),
            Command::SetDescriptorHeaps(c) => self.set_descriptor_heaps(c),
            Command::SetRootConstants(c) => self.set_root_constants(c),
            Command::SetRootDescriptor(c) => self.set_root_descriptor(c),
            Command::SetRootTable(c) => self.set_root_table(c),
            Command::Dispatch(c) => self.dispatch(c),
            Command::ExecuteIndirect(c) => self.execute_indirect(c),
            Command::CopyBufferRegion(c) => self.copy_buffer_region(c),
            Command::FillBuffer(c) => self.fill_buffer(c),
            Command::ClearUav(c) => self.clear_uav(c),
            Command::Barrier(c) => self.barrier(c),
            // Refused by `validate`; kept as a status rather than a panic.
            Command::CopyTextureRegion(c) => self.copy_texture_region(c),
            Command::CopyResource(c) => self.copy_resource(c),
            Command::ClearUavTexture(c) => self.clear_uav_texture(c),
            Command::SyncRecord(_) | Command::BufferData(_) | Command::DispatchIndirect(_) => {
                Err(Status::UNSUPPORTED)
            }
        }
    }

    fn list(&mut self) -> Result<(&mut CommandList, &mut Bound), Status> {
        match self.open.as_mut() {
            Some((l, _, b)) => Ok((l, b)),
            None => Err(Status::BAD_STREAM),
        }
    }

    /// Commits the lists ended for one queue.
    fn flush(&mut self, queue_id: u32) -> R {
        let Some(i) = self.batches.iter().position(|b| b.queue_id == queue_id) else {
            return Ok(());
        };
        let batch = self.batches.remove(i);
        self.commit(batch)
    }

    fn commit(&mut self, batch: Batch) -> R {
        let result = if batch.lists.is_empty() {
            Ok(())
        } else {
            batch
                .queue
                .submit(batch.lists)
                .map(|_| ())
                .map_err(|e| status_of(&e))
        };
        // Every submit ends with maintain.
        self.st.device.maintain();
        result
    }

    fn flush_all(&mut self) -> R {
        let batches = std::mem::take(&mut self.batches);
        let mut first = Ok(());
        for b in batches {
            let r = self.commit(b);
            if first.is_ok() {
                first = r;
            }
        }
        first
    }

    fn begin_list(&mut self, c: &BeginList) -> R {
        if c.flags != 0 {
            return Err(Status::INVALID_ARG);
        }
        let queue = self
            .st
            .queues
            .get(c.queue.0)
            .ok_or(Status::NOT_FOUND)?
            .clone();
        // A queue's pool of command buffers is finite, and `begin_list` waits
        // when it is used up: commit what is waiting before that can happen.
        let limit = (self.st.options.queue.max_lists / 2).max(1) as usize;
        if self
            .batches
            .iter()
            .find(|b| b.queue_id == c.queue.0)
            .is_some_and(|b| b.lists.len() >= limit)
        {
            self.flush(c.queue.0)?;
        }
        let mut list = queue.begin_list().map_err(|e| status_of(&e))?;
        // Argument-table slots keep the addresses of the list that used them
        // last: clear the three binding points the stream sets.
        for point in [
            crate::descriptors::bind_point::DESCRIPTOR_HEAP,
            crate::descriptors::bind_point::SAMPLER_HEAP,
            crate::descriptors::bind_point::ARGUMENT_BUFFER,
        ] {
            list.set_address(point as u32, 0)
                .map_err(|e| status_of(&e))?;
        }
        self.open = Some((list, c.queue, Bound::default()));
        Ok(())
    }

    fn end_list(&mut self) -> R {
        let (mut list, queue_id, _) = self.open.take().ok_or(Status::BAD_STREAM)?;
        list.end().map_err(|e| status_of(&e))?;
        let queue = self
            .st
            .queues
            .get(queue_id.0)
            .ok_or(Status::NOT_FOUND)?
            .clone();
        match self.batches.iter_mut().find(|b| b.queue_id == queue_id.0) {
            Some(b) => b.lists.push(list),
            None => self.batches.push(Batch {
                queue_id: queue_id.0,
                queue,
                lists: vec![list],
            }),
        }
        Ok(())
    }

    fn queue_signal(&mut self, c: &QueueSignal) -> R {
        let queue = self
            .st
            .queues
            .get(c.queue.0)
            .ok_or(Status::NOT_FOUND)?
            .clone();
        let fence = self
            .st
            .fences
            .get(c.fence.0)
            .ok_or(Status::NOT_FOUND)?
            .clone();
        self.flush(c.queue.0)?;
        queue.signal(&fence, c.value);
        Ok(())
    }

    fn queue_wait(&mut self, c: &QueueWait) -> R {
        let queue = self
            .st
            .queues
            .get(c.queue.0)
            .ok_or(Status::NOT_FOUND)?
            .clone();
        let fence = self
            .st
            .fences
            .get(c.fence.0)
            .ok_or(Status::NOT_FOUND)?
            .clone();
        self.flush(c.queue.0)?;
        queue.wait_fence(&fence, c.value);
        Ok(())
    }

    fn set_pipeline(&mut self, c: &SetPipeline) -> R {
        let pso = self.st.psos.get(c.pso.0).ok_or(Status::NOT_FOUND)?.clone();
        let (_, bound) = self.list()?;
        bound.pso = Some(pso);
        Ok(())
    }

    fn set_root_signature(&mut self, c: &SetRootSignature) -> R {
        let sig = self
            .st
            .root_sigs
            .get(c.root_sig.0)
            .ok_or(Status::NOT_FOUND)?
            .clone();
        let (_, bound) = self.list()?;
        // Setting the signature that is already bound changes nothing.
        if bound.sig.as_ref().is_some_and(|s| Arc::ptr_eq(s, &sig)) {
            return Ok(());
        }
        bound.shadow = vec![0; sig.layout.size() as usize];
        // The pointer to the static-sampler table MSC appends is the one
        // entry no root parameter sets.
        if let (Some(at), Some(table)) = (sig.layout.static_table_offset(), &sig.static_table) {
            let at = at as usize;
            bound.shadow[at..at + 8].copy_from_slice(&table.gpu_va().to_le_bytes());
        }
        bound.sig = Some(sig);
        bound.dirty = true;
        Ok(())
    }

    fn set_descriptor_heaps(&mut self, c: &SetDescriptorHeaps) -> R {
        let find = |st: &State,
                    id: HeapId,
                    sampler: bool|
         -> Result<Option<crate::descriptors::DescriptorHeap>, Status> {
            if id.is_null() {
                return Ok(None);
            }
            let h = st.heaps.get(id.0).ok_or(Status::NOT_FOUND)?;
            // A staging heap (not shader visible) cannot be bound.
            if h.sampler != sampler || !h.shader_visible {
                return Err(Status::INVALID_ARG);
            }
            Ok(Some(h.heap.clone()))
        };
        let resources = find(self.st, c.cbv_srv_uav, false)?;
        let samplers = find(self.st, c.sampler, true)?;
        let (list, _) = self.list()?;
        let result = match (&resources, &samplers) {
            (Some(r), s) => list.set_heaps(r, s.as_ref()),
            (None, Some(s)) => list.set_sampler_heap(s),
            (None, None) => Ok(()),
        };
        result.map_err(|e| status_of(&e))
    }

    fn set_root_constants(&mut self, c: &SetRootConstants<'_>) -> R {
        let (_, bound) = self.list()?;
        let sig = bound.sig.clone().ok_or(Status::INVALID_ARG)?;
        let slot = sig.layout.slot(c.root_index).ok_or(Status::INVALID_ARG)?;
        let SlotKind::Constants { count } = slot.kind else {
            return Err(Status::INVALID_ARG);
        };
        let end = u64::from(c.dest_offset) + c.count() as u64;
        if end > u64::from(count) {
            return Err(Status::INVALID_ARG);
        }
        let at = (slot.offset + 4 * c.dest_offset) as usize;
        bound.shadow[at..at + c.data.len()].copy_from_slice(c.data);
        bound.dirty = true;
        Ok(())
    }

    fn set_root_descriptor(&mut self, c: &SetRootDescriptor) -> R {
        // A root descriptor is an address: 0 means "none", anything else
        // must lie in a live buffer that is resident (the GPU would read
        // memory Metal does not know it must keep).
        if c.gpu_va != 0 {
            match self.st.device.resolve_va(c.gpu_va, 1) {
                Some((_, e)) if e.resident => {}
                _ => return Err(Status::INVALID_ARG),
            }
        }
        let (_, bound) = self.list()?;
        let sig = bound.sig.clone().ok_or(Status::INVALID_ARG)?;
        let slot = sig.layout.slot(c.root_index).ok_or(Status::INVALID_ARG)?;
        match (slot.kind, c.kind) {
            (SlotKind::Descriptor(k), kind) if k == kind => {}
            _ => return Err(Status::INVALID_ARG),
        }
        debug_assert!(matches!(
            c.kind,
            RootParamKind::CBV | RootParamKind::SRV | RootParamKind::UAV
        ));
        let at = slot.offset as usize;
        bound.shadow[at..at + 8].copy_from_slice(&c.gpu_va.to_le_bytes());
        bound.dirty = true;
        Ok(())
    }

    fn set_root_table(&mut self, c: &SetRootTable) -> R {
        let heap = self.st.heaps.get(c.heap.0).ok_or(Status::NOT_FOUND)?;
        if c.index >= heap.heap.capacity() {
            return Err(Status::INVALID_ARG);
        }
        let va = heap.heap.entry_va(c.index);
        let heap_is_sampler = heap.sampler;
        let (_, bound) = self.list()?;
        let sig = bound.sig.clone().ok_or(Status::INVALID_ARG)?;
        let slot = sig.layout.slot(c.root_index).ok_or(Status::INVALID_ARG)?;
        match slot.kind {
            SlotKind::Table { sampler } if sampler == heap_is_sampler => {}
            _ => return Err(Status::INVALID_ARG),
        }
        let at = slot.offset as usize;
        bound.shadow[at..at + 8].copy_from_slice(&va.to_le_bytes());
        bound.dirty = true;
        Ok(())
    }

    fn dispatch(&mut self, c: &Dispatch) -> R {
        let (list, bound) = self.list()?;
        let pso = bound.pso.clone().ok_or(Status::INVALID_ARG)?;
        let sig = bound.sig.clone().ok_or(Status::INVALID_ARG)?;
        let PsoBody::Compute { pipeline, threads } = &pso.body else {
            // A dispatch needs a compute pipeline.
            return Err(Status::INVALID_ARG);
        };
        // The pipeline was made for one root signature (or an identical
        // one); its argument buffer layout is what the shader reads.
        if !pso.root_sig.compatible_with(&sig) {
            return Err(Status::INVALID_ARG);
        }
        if bound.dirty && !bound.shadow.is_empty() {
            let tlab = list
                .alloc_constants(bound.shadow.len() as u64, 8)
                .map_err(|e| status_of(&e))?;
            tlab.write_bytes(0, &bound.shadow)
                .map_err(|e| status_of(&e))?;
            list.set_root_arguments(&tlab).map_err(|e| status_of(&e))?;
        }
        bound.dirty = false;
        list.dispatch(pipeline, [c.x, c.y, c.z], *threads)
            .map_err(|e| status_of(&e))
    }

    fn execute_indirect(&mut self, c: &victory_metal_proto::indirect::ExecuteIndirect<'_>) -> R {
        let args = self.st.buffer(c.args)?.clone();
        let count = if c.count.is_null() {
            None
        } else {
            Some((self.st.buffer(c.count)?.clone(), c.count_offset))
        };
        let signature_root = if c.root_sig.is_null() {
            None
        } else {
            Some(
                self.st
                    .root_sigs
                    .get(c.root_sig.0)
                    .ok_or(Status::NOT_FOUND)?
                    .clone(),
            )
        };
        let kernels = &self.st.indirect_kernels;
        let (list, bound) = match self.open.as_mut() {
            Some((l, _, b)) => (l, b),
            None => return Err(Status::BAD_STREAM),
        };
        let pso = bound.pso.clone().ok_or(Status::INVALID_ARG)?;
        let sig = bound.sig.clone().ok_or(Status::INVALID_ARG)?;
        let PsoBody::Compute { pipeline, threads } = &pso.body else {
            return Err(Status::INVALID_ARG);
        };
        if !pso.root_sig.compatible_with(&sig) {
            return Err(Status::INVALID_ARG);
        }
        // The signature was made for a root signature; the list's must be
        // the same one (or an identical one).
        if let Some(made_for) = &signature_root {
            if !made_for.compatible_with(&sig) {
                return Err(Status::INVALID_ARG);
            }
        }
        let plan = super::indirect::plan(c, Some(&sig))?;
        let spec = crate::indirect::IndirectDispatch {
            args: &args,
            args_offset: c.args_offset,
            count: count.as_ref().map(|(b, off)| (b, *off)),
            max_count: c.max_count,
            stride: c.stride,
            dispatch_offset: plan.dispatch_offset,
            record_bytes: plan.record_bytes,
            roots: &plan.roots,
            base: &bound.shadow,
        };
        // The kernel makes its own copies of the argument buffer; otherwise
        // the dispatches read the one the list has, written as for a dispatch.
        if !spec.replaces_root_arguments() && bound.dirty && !bound.shadow.is_empty() {
            let tlab = list
                .alloc_constants(bound.shadow.len() as u64, 8)
                .map_err(|e| status_of(&e))?;
            tlab.write_bytes(0, &bound.shadow)
                .map_err(|e| status_of(&e))?;
            list.set_root_arguments(&tlab).map_err(|e| status_of(&e))?;
            bound.dirty = false;
        }
        let replaced = spec.replaces_root_arguments();
        let r = list
            .execute_indirect(kernels, pipeline, *threads, &spec)
            .map_err(|e| {
                self.st.note = Some(format!("EXECUTE_INDIRECT: {e}"));
                status_of(&e)
            });
        // The argument table now points at a record's copy: the next dispatch writes its own.
        if replaced {
            bound.dirty = true;
        }
        r
    }

    fn copy_buffer_region(&mut self, c: &CopyBufferRegion) -> R {
        let dst = self.st.buffer(c.dst)?.clone();
        let src = self.st.buffer(c.src)?.clone();
        let (list, _) = self.list()?;
        list.copy_buffer(&dst, c.dst_offset, &src, c.src_offset, c.size)
            .map_err(|e| status_of(&e))
    }

    fn fill_buffer(&mut self, c: &FillBuffer) -> R {
        let byte = repeated_byte(c.value).ok_or(Status::UNSUPPORTED)?;
        let dst = self.st.buffer(c.dst)?.clone();
        let (list, _) = self.list()?;
        list.fill_buffer(&dst, c.offset, c.size, byte)
            .map_err(|e| status_of(&e))
    }

    fn clear_uav(&mut self, c: &victory_metal_proto::cmd::ClearUav) -> R {
        // Raw (format UNKNOWN) or one 32-bit channel: the value is the first
        // word. Only values whose four bytes agree are done (a blit fill
        // writes one byte); others are refused, not approximated.
        match c.format {
            format::UNKNOWN | format::R32_UINT | format::R32_SINT | format::R32_FLOAT => {}
            _ => return Err(Status::UNSUPPORTED),
        }
        let byte = repeated_byte(c.values[0]).ok_or(Status::UNSUPPORTED)?;
        let dst = self.st.buffer(c.res)?.clone();
        let (list, _) = self.list()?;
        list.fill_buffer(&dst, c.offset, c.size, byte)
            .map_err(|e| status_of(&e))
    }

    fn copy_texture_region(&mut self, c: &CopyTextureRegion) -> R {
        use crate::copy::{BufLoc, TexLoc};
        use victory_metal_proto::types::LocKind;
        let (dst, src) = (&c.dst, &c.src);
        let boxed = c.flags & CopyTextureRegion::HAS_BOX != 0;
        let [left, top, front] = if boxed {
            [c.box_left, c.box_top, c.box_front]
        } else {
            [0; 3]
        };
        let box_size = [
            c.box_right.saturating_sub(c.box_left),
            c.box_bottom.saturating_sub(c.box_top),
            c.box_back.saturating_sub(c.box_front),
        ];
        match (dst.kind, src.kind) {
            (LocKind::SUBRESOURCE, LocKind::FOOTPRINT) => {
                let t = self.st.texture(dst.res)?.clone();
                let b = self.st.buffer(src.res)?.clone();
                let size = if boxed {
                    box_size
                } else {
                    [src.width, src.height, src.depth]
                };
                let offset = footprint_offset(src, [left, top, front])?;
                let dl = TexLoc {
                    texture: &t,
                    mip: u32::from(dst.mip),
                    slice: dst.slice,
                    plane: u32::from(dst.plane),
                    origin: [c.dst_x, c.dst_y, c.dst_z],
                };
                let bl = BufLoc {
                    buffer: &b,
                    offset,
                    format: src.format,
                    row_pitch: src.row_pitch,
                    rows: src.height.saturating_sub(top),
                };
                let (list, _) = self.list()?;
                list.copy_buffer_to_texture(&dl, &bl, size)
            }
            (LocKind::FOOTPRINT, LocKind::SUBRESOURCE) => {
                let t = self.st.texture(src.res)?.clone();
                let b = self.st.buffer(dst.res)?.clone();
                let mip = u32::from(src.mip);
                let whole = t.desc().mip_size(mip.min(t.desc().mips - 1));
                let size = if boxed { box_size } else { whole };
                let offset = footprint_offset(dst, [c.dst_x, c.dst_y, c.dst_z])?;
                let sl = TexLoc {
                    texture: &t,
                    mip,
                    slice: src.slice,
                    plane: u32::from(src.plane),
                    origin: [left, top, front],
                };
                let bl = BufLoc {
                    buffer: &b,
                    offset,
                    format: dst.format,
                    row_pitch: dst.row_pitch,
                    rows: dst.height.saturating_sub(c.dst_y),
                };
                let (list, _) = self.list()?;
                list.copy_texture_to_buffer(&bl, &sl, size)
            }
            (LocKind::SUBRESOURCE, LocKind::SUBRESOURCE) => {
                let d = self.st.texture(dst.res)?.clone();
                let s = self.st.texture(src.res)?.clone();
                let mip = u32::from(src.mip);
                let whole = s.desc().mip_size(mip.min(s.desc().mips - 1));
                let size = if boxed { box_size } else { whole };
                let dl = TexLoc {
                    texture: &d,
                    mip: u32::from(dst.mip),
                    slice: dst.slice,
                    plane: u32::from(dst.plane),
                    origin: [c.dst_x, c.dst_y, c.dst_z],
                };
                let sl = TexLoc {
                    texture: &s,
                    mip,
                    slice: src.slice,
                    plane: u32::from(src.plane),
                    origin: [left, top, front],
                };
                let (list, _) = self.list()?;
                list.copy_texture(&dl, &sl, size)
            }
            // Buffer to buffer is `CopyBufferRegion`.
            _ => return Err(Status::INVALID_ARG),
        }
        .map_err(|e| self.st.refuse("COPY_TEXTURE_REGION", &e))
    }

    fn copy_resource(&mut self, c: &victory_metal_proto::cmd::CopyResource) -> R {
        use super::Res;
        let both = match (
            self.st.resources.get(c.dst.0).ok_or(Status::NOT_FOUND)?,
            self.st.resources.get(c.src.0).ok_or(Status::NOT_FOUND)?,
        ) {
            (Res::Buffer(d), Res::Buffer(s)) => Ok((d.clone(), s.clone())),
            (Res::Texture(d), Res::Texture(s)) => Err((d.clone(), s.clone())),
            _ => return Err(Status::INVALID_ARG),
        };
        let r = match both {
            Ok((d, s)) => {
                if d.len() != s.len() {
                    return Err(Status::INVALID_ARG);
                }
                let (list, _) = self.list()?;
                list.copy_buffer(&d, 0, &s, 0, s.len())
            }
            Err((d, s)) => {
                let (list, _) = self.list()?;
                list.copy_texture_resource(&d, &s)
            }
        };
        r.map_err(|e| self.st.refuse("COPY_RESOURCE", &e))
    }

    fn clear_uav_texture(&mut self, c: &victory_metal_proto::cmd::ClearUavTexture) -> R {
        use victory_metal_proto::cmd::ClearUavTexture;
        let t = self.st.texture(c.res)?.clone();
        let req = crate::clear::ClearRequest {
            format: c.format,
            mip: c.mip,
            first_slice: c.first_slice,
            slice_count: c.slice_count,
            values: c.values,
            float_values: c.float_values != 0,
            rect: (c.flags & ClearUavTexture::HAS_RECT != 0).then_some([
                c.rect_left,
                c.rect_top,
                c.rect_right,
                c.rect_bottom,
            ]),
        };
        let kernels = &self.st.clear_kernels;
        let (list, _) = match self.open.as_mut() {
            Some((l, _, b)) => (l, b),
            None => return Err(Status::BAD_STREAM),
        };
        list.clear_texture(kernels, &t, &req).map_err(|e| {
            self.st.note = Some(format!("CLEAR_UAV_TEXTURE: {e}"));
            status_of(&e)
        })
    }

    fn barrier(&mut self, c: &victory_metal_proto::cmd::Barrier) -> R {
        let (list, _) = self.list()?;
        let r = match c.class {
            BarrierClass::MEMORY => list.barrier(c.before, c.after, c.visibility),
            BarrierClass::ALIASING => list.aliasing_barrier(),
            _ => return Err(Status::UNSUPPORTED),
        };
        r.map_err(|e| status_of(&e))
    }
}

/// `Some(b)` when all four bytes of `v` are `b`.
fn repeated_byte(v: u32) -> Option<u8> {
    let b = (v & 0xff) as u8;
    (v == u32::from_ne_bytes([b; 4])).then_some(b)
}

/// Byte offset of texel `at` (`[x, y, z]` from the footprint's corner) in a
/// buffer footprint: the footprint's offset plus whole images, rows and
/// texels. A block compressed format moves in blocks, so `at` must be on one.
fn footprint_offset(
    loc: &victory_metal_proto::cmd::TextureLocation,
    at: [u32; 3],
) -> Result<u64, Status> {
    let f = crate::format::lookup(loc.format).ok_or(Status::INVALID_ARG)?;
    let (bw, bh) = (u32::from(f.block.0), u32::from(f.block.1));
    if !at[0].is_multiple_of(bw) || !at[1].is_multiple_of(bh) {
        return Err(Status::INVALID_ARG);
    }
    let rows_per_image = u64::from(f.block_rows(loc.height));
    let image = u64::from(loc.row_pitch) * rows_per_image;
    Ok(loc.offset
        + u64::from(at[2]) * image
        + u64::from(at[1] / bh) * u64::from(loc.row_pitch)
        + u64::from(at[0] / bw) * u64::from(f.bytes))
}
