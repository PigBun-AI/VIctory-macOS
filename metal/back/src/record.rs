//! Recording: one front-end command list is one `MTL4CommandBuffer`.
//!
//! Copies, fills and dispatches share one `MTL4ComputeCommandEncoder` (there
//! is no blit encoder in Metal 4). When the queue orders lists after one
//! another (the default), every encoder starts with a consumer-form
//! queue barrier from all earlier work; a list that opens no encoder costs
//! nothing. Barriers are **deferred**: a barrier is
//! held until the next command shows where it has to be encoded (see
//! [`crate::barrier`]); split barriers are only encoded at their end.
//!
//! The argument table is the shader's view of the world: bind point 0 is the
//! resource descriptor heap, 1 the sampler heap, 2 the top-level argument
//! buffer ([`crate::descriptors::bind_point`]). Metal snapshots the table at
//! every dispatch, so one table serves a whole list; an address set after a
//! dispatch affects only the later ones.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSRange, NSString};
use objc2_metal::{
    MTL4CommandAllocator, MTL4CommandBuffer, MTL4CommandEncoder, MTL4ComputeCommandEncoder,
    MTL4RenderCommandEncoder, MTLSize,
};
use std::collections::HashMap;

use crate::barrier::{Action, EncoderKind, Tracker};
use crate::descriptors::{ArgumentTable, DescriptorHeap, RingAlloc, bind_point};
use crate::error::{Error, Result};
use crate::memory::Buffer;
use crate::pipeline::ComputePipeline;
use crate::queue::{Queue, Slot};
use crate::stages::{self, BLIT, DISPATCH, VIS_DEVICE};

type ComputeEncoder = Retained<ProtocolObject<dyn MTL4ComputeCommandEncoder>>;

/// What a list encoded (for tests and tuning).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListStats {
    /// Compute encoders begun.
    pub encoders: u32,
    /// Copies encoded.
    pub copies: u32,
    /// Fills encoded.
    pub fills: u32,
    /// Dispatches encoded.
    pub dispatches: u32,
    /// Barriers encoded inside an encoder.
    pub encoder_barriers: u32,
    /// Consumer-form barriers encoded.
    pub consumer_barriers: u32,
    /// Producer-form barriers encoded.
    pub producer_barriers: u32,
    /// Render encoders begun (a pass split by a barrier counts again).
    pub render_encoders: u32,
    /// Render passes begun and ended.
    pub passes: u32,
    /// Draws encoded (indexed ones included).
    pub draws: u32,
    /// Mesh dispatches encoded.
    pub mesh_draws: u32,
    /// Clears of part of an attachment inside a pass.
    pub attachment_clears: u32,
    /// Mesh draws emulated as compute plus a vertex-fetch draw (the mesh
    /// emulation, [`crate::mesh_emu`]); `emu_splits` of them split their pass
    /// (counted in both), `emu_dropped` were dropped.
    pub emu_draws: u32,
    pub emu_splits: u32,
    pub emu_dropped: u32,
}

/// A command list being recorded. Made by [`Queue::begin_list`], finished
/// with [`end`](Self::end), handed to [`Queue::submit`].
pub struct CommandList {
    queue: Queue,
    slot: Option<Slot>,
    pub(crate) tracker: Tracker,
    encoder: Option<ComputeEncoder>,
    /// The open render encoder (at most one of `encoder` and `render`).
    pub(crate) render: Option<crate::draw::RenderEncoder>,
    /// The render pass between `begin_pass` and `end_pass`.
    pub(crate) pass: Option<crate::draw::Pass>,
    /// Render state the stream has set (it outlives passes).
    pub(crate) rstate: crate::draw::RenderState,
    pub(crate) keep_gfx: Vec<crate::render::GraphicsPipeline>,
    pub(crate) table: ArgumentTable,
    ended: bool,
    pub(crate) keep_buffers: Vec<Buffer>,
    /// Texture copies: what they keep alive and touch ([`crate::copy`]).
    pub(crate) copies: crate::copy::CopyState,
    keep_pipelines: Vec<ComputePipeline>,
    last_pipeline: Option<*const ()>,
    splits: HashMap<u64, (u16, u8)>,
    /// Buffer ranges written by copies and fills since the last barrier that
    /// orders blits (see [`CommandList::blit_hazard`]).
    blit_writes: Vec<(Buffer, u64, u64)>,
    pub(crate) stats: ListStats,
    /// GPU time samples (`VICTORY_METAL_GPUPROF`, [`crate::prof`]).
    pub(crate) prof: Option<crate::prof::ListProf>,
    /// What the executor put into the list, for the log of a failed submission ([`crate::fault`]).
    pub(crate) journal: Option<crate::fault::Journal>,
    /// The ring entry the last indirect dispatch's arguments were copied to ([`crate::prof::Snap`]).
    pub(crate) last_snap: Option<u32>,
}

// A command buffer and its encoder may move between threads as long as one
// thread uses them at a time, which `&mut self` guarantees.
unsafe impl Send for CommandList {}

impl Queue {
    /// Starts recording a list. Takes an allocator and command buffer from
    /// the pool (waiting for a busy one if the pool is at its limit).
    ///
    /// Record lists one after the other and submit them in that order.
    pub fn begin_list(&self) -> Result<CommandList> {
        let slot = self.acquire_slot()?;
        slot.allocator.reset();
        slot.cb.beginCommandBufferWithAllocator(&slot.allocator);
        slot.cb
            .setLabel(Some(&NSString::from_str("victory command list")));
        let ordered_after_prior = self.inter_submit_barrier();
        let mut list = CommandList {
            queue: self.clone(),
            table: slot.table.clone(),
            slot: Some(slot),
            tracker: Tracker::new(ordered_after_prior),
            encoder: None,
            render: None,
            pass: None,
            rstate: Default::default(),
            keep_gfx: Vec::new(),
            ended: false,
            keep_buffers: Vec::new(),
            copies: Default::default(),
            keep_pipelines: Vec::new(),
            last_pipeline: None,
            splits: HashMap::new(),
            blit_writes: Vec::new(),
            stats: ListStats::default(),
            prof: crate::prof::ListProf::new(self.device().raw()),
            journal: crate::fault::Journal::new(),
            last_snap: None,
        };
        list.refresh_mirrors()?;
        Ok(list)
    }
}

impl CommandList {
    pub(crate) fn cb(&self) -> &ProtocolObject<dyn MTL4CommandBuffer> {
        &self.slot.as_ref().expect("list is live").cb
    }

    pub(crate) fn queue(&self) -> &Queue {
        &self.queue
    }

    pub(crate) fn is_ended(&self) -> bool {
        self.ended
    }

    pub(crate) fn retained_command_buffer(
        &self,
    ) -> Retained<ProtocolObject<dyn MTL4CommandBuffer>> {
        self.slot.as_ref().expect("list is live").cb.clone()
    }

    /// The list was committed with timeline value `value`: its allocator
    /// is busy until then.
    pub(crate) fn into_submitted(mut self, value: u64) {
        if let Some(slot) = self.slot.take() {
            self.queue.return_slot(slot, value);
        }
        if let Some(journal) = self.journal.take() {
            crate::fault::queue(value, journal);
        }
        if let Some(prof) = self.prof.take() {
            crate::prof::queue_list(
                self.queue.device().raw(),
                &self.queue.inner.timeline,
                value,
                prof,
            );
        }
        // `keep_*` drop here, after the commit: anything retired now waits
        // for `value`.
    }

    /// What the list has encoded so far.
    pub fn stats(&self) -> ListStats {
        self.stats
    }

    /// Space in the scratch ring of the mesh emulation (see [`Queue::alloc_scratch`]).
    pub fn alloc_scratch(&self, size: u64, align: u64) -> Result<RingAlloc> {
        self.queue.alloc_scratch(size, align)
    }

    /// Space in the constant ring (see [`Queue::alloc_constants`]).
    pub fn alloc_constants(&self, size: u64, align: u64) -> Result<RingAlloc> {
        self.queue.alloc_constants(size, align)
    }

    pub(crate) fn live(&self) -> Result<()> {
        if self.ended {
            Err(Error::BadArgument("command list already ended".into()))
        } else {
            Ok(())
        }
    }

    /// An op of the title's put into the list, for the journal of a failed submission (does
    /// nothing unless `VICTORY_METAL_LOG` is on).
    pub fn journal_note(&mut self, label: impl FnOnce() -> String) {
        if let Some(j) = self.journal.as_mut() {
            j.note(&label());
        }
    }

    /// `VICTORY_METAL_GPUPROF`: a timestamp into the open encoder, named `label()` (the scope
    /// that ends here). Does nothing when the profiler is off or no encoder is open.
    pub fn prof_sample(&mut self, label: impl FnOnce() -> String) {
        let Some(cfg) = crate::prof::config() else {
            return;
        };
        let Some(p) = self.prof.as_mut() else {
            return;
        };
        if let Some(enc) = &self.encoder {
            if let Some(i) = p.next(label()) {
                // SAFETY: `i` is below the heap's entry count (`ListProf::next`).
                unsafe {
                    enc.writeTimestampWithGranularity_intoHeap_atIndex(
                        cfg.granularity(),
                        p.heap(),
                        i,
                    )
                };
            }
        } else if let Some(enc) = &self.render
            && let Some(i) = p.next(label())
        {
            // SAFETY: as above.
            unsafe {
                enc.writeTimestampWithGranularity_afterStage_intoHeap_atIndex(
                    if cfg.draws {
                        cfg.granularity()
                    } else {
                        objc2_metal::MTL4TimestampGranularity::Relaxed
                    },
                    objc2_metal::MTLRenderStages::Fragment,
                    p.heap(),
                    i,
                )
            };
        }
    }

    /// A render encoder's first sample: before its first draw (the stage `Vertex` is where the
    /// pass starts), named `gap`: the time since the last scope ended is the wait for earlier work.
    fn prof_sample_render_start(&mut self) {
        let Some(cfg) = crate::prof::config() else {
            return;
        };
        let (Some(p), Some(enc)) = (self.prof.as_mut(), &self.render) else {
            return;
        };
        if let Some(i) = p.next("gap before an encoder".into()) {
            // SAFETY: `i` is below the heap's entry count.
            unsafe {
                enc.writeTimestampWithGranularity_afterStage_intoHeap_atIndex(
                    objc2_metal::MTL4TimestampGranularity::Relaxed,
                    objc2_metal::MTLRenderStages::Vertex,
                    p.heap(),
                    i,
                )
            };
            let _ = cfg;
        }
    }

    /// What a render encoder's scope is called: its size, attachments and draws.
    fn prof_pass_label(&mut self) -> String {
        let draws = self
            .prof
            .as_mut()
            .map_or(0, |p| std::mem::take(&mut p.draws));
        match &self.pass {
            Some(p) => p.describe(draws),
            None => format!("render encoder, {draws} draws"),
        }
    }

    pub(crate) fn perform(&mut self, actions: Vec<Action>) -> Result<()> {
        let mut opened = None;
        for action in actions {
            match action {
                Action::Open(EncoderKind::Compute) => {
                    opened = Some(EncoderKind::Compute);
                    let encoder = self
                        .cb()
                        .computeCommandEncoder()
                        .ok_or_else(|| Error::Metal("computeCommandEncoder returned nil".into()))?;
                    encoder.setArgumentTable(Some(self.table.raw()));
                    self.encoder = Some(encoder);
                    self.last_pipeline = None;
                    self.stats.encoders += 1;
                }
                Action::Open(EncoderKind::Render) => {
                    opened = Some(EncoderKind::Render);
                    self.open_render()?
                }
                Action::Close => {
                    if self.render.is_some() && self.prof.is_some() {
                        let label = self.prof_pass_label();
                        self.prof_sample(|| label);
                    }
                    if let Some(encoder) = self.encoder.take() {
                        encoder.endEncoding();
                    }
                    if let Some(encoder) = self.render.take() {
                        encoder.endEncoding();
                    }
                }
                Action::Encoder { after, before, vis } => {
                    self.any_encoder()?
                        .barrierAfterEncoderStages_beforeEncoderStages_visibilityOptions(
                            stages::to_mtl_stages(after)?,
                            stages::to_mtl_stages(before)?,
                            stages::to_mtl_visibility(vis)?,
                        );
                    self.stats.encoder_barriers += 1;
                }
                Action::Consumer { after, before, vis } => {
                    self.any_encoder()?
                        .barrierAfterQueueStages_beforeStages_visibilityOptions(
                            stages::to_mtl_stages(after)?,
                            stages::to_mtl_stages(before)?,
                            stages::to_mtl_visibility(vis)?,
                        );
                    self.stats.consumer_barriers += 1;
                }
                Action::Producer { after, before, vis } => {
                    self.any_encoder()?
                        .barrierAfterStages_beforeQueueStages_visibilityOptions(
                            stages::to_mtl_stages(after)?,
                            stages::to_mtl_stages(before)?,
                            stages::to_mtl_visibility(vis)?,
                        );
                    self.stats.producer_barriers += 1;
                }
            }
        }
        // The barriers that open an encoder wait for earlier work: a sample after them
        // separates that wait from the first op's own time.
        match opened {
            Some(EncoderKind::Compute) => self.prof_sample(|| "gap before an encoder".into()),
            Some(EncoderKind::Render) => self.prof_sample_render_start(),
            None => {}
        }
        Ok(())
    }

    /// The open encoder, whichever kind, for the calls every encoder has.
    fn any_encoder(&self) -> Result<&ProtocolObject<dyn MTL4CommandEncoder>> {
        if let Some(e) = &self.render {
            return Ok(ProtocolObject::from_ref(&**e));
        }
        let e = self.encoder_ref()?;
        Ok(ProtocolObject::from_ref(&**e))
    }

    fn encoder_ref(&self) -> Result<&ComputeEncoder> {
        self.encoder
            .as_ref()
            .ok_or_else(|| Error::Metal("no open encoder".into()))
    }

    /// Makes the encoder for a command of `stage` current and encodes the
    /// barriers it needs.
    pub(crate) fn prepare(&mut self, stage: u16) -> Result<&ComputeEncoder> {
        let actions = self
            .tracker
            .command(stage)
            .ok_or_else(|| Error::BadArgument(format!("no encoder for stage {stage:#x}")))?;
        self.perform(actions)?;
        self.encoder_ref()
    }

    pub(crate) fn check_buffer(
        &self,
        buffer: &Buffer,
        offset: u64,
        size: u64,
        what: &str,
    ) -> Result<()> {
        if !buffer.belongs_to(self.queue.device()) {
            return Err(Error::BadArgument(format!(
                "{what}: buffer of another device"
            )));
        }
        if offset
            .checked_add(size)
            .is_none_or(|end| end > buffer.len())
        {
            return Err(Error::BadArgument(format!(
                "{what}: range {offset}+{size} outside a buffer of {} bytes",
                buffer.len()
            )));
        }
        if !buffer.is_resident() {
            return Err(Error::NotResident(format!(
                "{what}: buffer {:#x}",
                buffer.gpu_va()
            )));
        }
        Ok(())
    }

    /// Copies and fills of one list that touch the same bytes (write after
    /// write, read after write) must run in the order they were recorded, as they
    /// do on the original API's queues and as D3DMetal gives. Metal 4 may run
    /// blits of one encoder concurrently, so this records the ranges written since
    /// the last barrier and, when a new blit overlaps one, asks for a barrier
    /// between blits (the tracker puts it before this command, in the encoder).
    /// Blits that touch different bytes still overlap.
    fn blit_hazard(&mut self, dst: &Buffer, dst_off: u64, src: Option<(&Buffer, u64)>, size: u64) {
        let overlaps = |b: &Buffer, off: u64, w: &(Buffer, u64, u64)| {
            w.0.same(b) && off < w.2 && w.1 < off + size
        };
        let hazard = self
            .blit_writes
            .iter()
            .any(|w| overlaps(dst, dst_off, w) || src.is_some_and(|(sb, so)| overlaps(sb, so, w)));
        // A very long list of distinct writes: stop tracking, order everything.
        if hazard || self.blit_writes.len() >= 256 {
            self.order_blits();
        }
        self.blit_writes
            .push((dst.clone(), dst_off, dst_off + size));
    }

    /// A copy that only reads `[off, off + size)` of `buf` (a texture upload
    /// reads its source buffer): asks for a barrier when an earlier copy
    /// wrote those bytes since the last one.
    pub(crate) fn blit_read_hazard(&mut self, buf: &Buffer, off: u64, size: u64) {
        let hazard = self
            .blit_writes
            .iter()
            .any(|w| w.0.same(buf) && off < w.2 && w.1 < off + size);
        if hazard {
            self.order_blits();
        }
    }

    /// A copy that writes `[off, off + size)` of `buf` (a texture download).
    pub(crate) fn blit_write_hazard(&mut self, buf: &Buffer, off: u64, size: u64) {
        self.blit_hazard(buf, off, None, size);
    }

    /// Orders every copy and fill before this point against the ones after.
    pub(crate) fn order_blits(&mut self) {
        self.tracker.barrier(BLIT, BLIT, VIS_DEVICE);
        self.blit_writes.clear();
        self.copies.accesses.clear();
    }

    /// `CopyBufferRegion`: `size` bytes from `src[src_offset..]` to
    /// `dst[dst_offset..]`. Overlap inside one buffer is refused (undefined in the original API).
    pub fn copy_buffer(
        &mut self,
        dst: &Buffer,
        dst_offset: u64,
        src: &Buffer,
        src_offset: u64,
        size: u64,
    ) -> Result<()> {
        self.live()?;
        self.check_buffer(dst, dst_offset, size, "copy destination")?;
        self.check_buffer(src, src_offset, size, "copy source")?;
        if size == 0 {
            return Ok(());
        }
        if dst.same(src) && dst_offset < src_offset + size && src_offset < dst_offset + size {
            return Err(Error::BadArgument("copy ranges overlap".into()));
        }
        let (d, s) = (dst.clone(), src.clone());
        self.blit_hazard(dst, dst_offset, Some((src, src_offset)), size);
        let encoder = self.prepare(BLIT)?;
        // SAFETY: both buffers are alive (kept by this list), the ranges were
        // checked above.
        unsafe {
            encoder.copyFromBuffer_sourceOffset_toBuffer_destinationOffset_size(
                s.raw(),
                src_offset as usize,
                d.raw(),
                dst_offset as usize,
                size as usize,
            );
        }
        self.stats.copies += 1;
        self.keep_buffers.push(d);
        self.keep_buffers.push(s);
        Ok(())
    }

    /// Fills `size` bytes at `offset` with `value`.
    pub fn fill_buffer(
        &mut self,
        buffer: &Buffer,
        offset: u64,
        size: u64,
        value: u8,
    ) -> Result<()> {
        self.live()?;
        self.check_buffer(buffer, offset, size, "fill")?;
        if size == 0 {
            return Ok(());
        }
        let b = buffer.clone();
        self.blit_hazard(buffer, offset, None, size);
        let encoder = self.prepare(BLIT)?;
        // SAFETY: the buffer is alive (kept by this list); range checked.
        unsafe {
            encoder.fillBuffer_range_value(
                b.raw(),
                NSRange::new(offset as usize, size as usize),
                value,
            );
        }
        self.stats.fills += 1;
        self.keep_buffers.push(b);
        Ok(())
    }

    /// Binds the descriptor heap (bind point 0) and, if given, the sampler
    /// heap (bind point 1).
    pub fn set_heaps(
        &mut self,
        resources: &DescriptorHeap,
        samplers: Option<&DescriptorHeap>,
    ) -> Result<()> {
        self.live()?;
        crate::prof::LAST_HEAP_VA.store(resources.gpu_va(), std::sync::atomic::Ordering::Relaxed);
        self.table
            .set_address(bind_point::DESCRIPTOR_HEAP as u32, resources.gpu_va())?;
        self.keep_buffers.push(resources.buffer().clone());
        if let Some(s) = samplers {
            self.table
                .set_address(bind_point::SAMPLER_HEAP as u32, s.gpu_va())?;
            self.keep_buffers.push(s.buffer().clone());
        }
        Ok(())
    }

    /// Binds only the sampler heap (bind point 1), leaving the descriptor
    /// heap as it is.
    pub fn set_sampler_heap(&mut self, samplers: &DescriptorHeap) -> Result<()> {
        self.live()?;
        self.table
            .set_address(bind_point::SAMPLER_HEAP as u32, samplers.gpu_va())?;
        self.keep_buffers.push(samplers.buffer().clone());
        Ok(())
    }

    /// Binds the top-level argument buffer (bind point 2) to a ring
    /// allocation holding root constants, root descriptors and table
    /// pointers.
    pub fn set_root_arguments(&mut self, tlab: &RingAlloc) -> Result<()> {
        self.live()?;
        // The object stage of a geometry pipeline (a vertex shader made
        // for the emulation) reads the same arguments at the hull/domain
        // point, too.
        self.table
            .set_address(bind_point::ARGUMENT_BUFFER_HULL_DOMAIN as u32, tlab.gpu_va)?;
        self.table
            .set_address(bind_point::ARGUMENT_BUFFER as u32, tlab.gpu_va)
    }

    /// Binds a GPU address at an arbitrary buffer slot.
    pub fn set_address(&mut self, index: u32, gpu_va: u64) -> Result<()> {
        self.live()?;
        self.table.set_address(index, gpu_va)
    }

    /// Binds the argument buffer at `gpu_va` as the root arguments (a ring
    /// allocation, or a copy of one made on the GPU).
    pub(crate) fn set_root_arguments_va(&mut self, gpu_va: u64) -> Result<()> {
        self.live()?;
        self.table
            .set_address(bind_point::ARGUMENT_BUFFER as u32, gpu_va)
    }

    /// One dispatch whose group counts the GPU reads at `args_va` (`{x, y, z}`
    /// as `u32`, 4-byte aligned). The buffer must be resident and its writers
    /// ordered before, as for any read.
    pub(crate) fn encode_dispatch_indirect(
        &mut self,
        pipeline: &ComputePipeline,
        args_va: u64,
        threads: [u32; 3],
    ) -> Result<()> {
        self.live()?;
        if !args_va.is_multiple_of(4) {
            return Err(Error::BadArgument(format!(
                "indirect dispatch arguments at {args_va:#x} are not 4-byte aligned"
            )));
        }
        if self.prof.is_some()
            && std::env::var_os("VICTORY_METAL_GPUPROF_ARGS").is_some_and(|v| v != "0")
            && let Some(sn) = crate::prof::snap(&self.queue.device().clone())
        {
            let (n, dst) = sn.take();
            let params = self.alloc_constants(16, 16)?;
            params.write_u32s(
                0,
                &[
                    args_va as u32,
                    (args_va >> 32) as u32,
                    dst as u32,
                    (dst >> 32) as u32,
                ],
            )?;
            self.set_address(30, params.gpu_va)?;
            self.dispatch(&sn.pipeline, [1, 1, 1], [1, 1, 1])?;
            self.keep_buffers.push(sn.ring.clone());
            self.last_snap = Some(n);
        }
        let per_group: u64 = threads.iter().map(|&t| u64::from(t)).product();
        if per_group == 0 || per_group > u64::from(pipeline.max_threads_per_threadgroup()) {
            return Err(Error::BadArgument(format!(
                "{threads:?} threads per group; the pipeline allows {}",
                pipeline.max_threads_per_threadgroup()
            )));
        }
        let p = pipeline.clone();
        let key = p.identity();
        // After `prepare`: it may open a new encoder, which has no pipeline
        // set (and `last_pipeline` is forgotten with the old encoder).
        self.prepare(DISPATCH)?;
        let new_pipeline = self.last_pipeline != Some(key);
        let encoder = self.encoder_ref()?;
        if new_pipeline {
            encoder.setComputePipelineState(p.raw());
        }
        // SAFETY: the address is the caller's: a resident buffer (or ring
        // allocation) holding three u32, 4-byte aligned (checked above).
        unsafe {
            encoder.dispatchThreadgroupsWithIndirectBuffer_threadsPerThreadgroup(
                args_va,
                MTLSize {
                    width: threads[0] as usize,
                    height: threads[1] as usize,
                    depth: threads[2] as usize,
                },
            );
        }
        self.last_pipeline = Some(key);
        self.stats.dispatches += 1;
        if new_pipeline {
            self.keep_pipelines.push(p);
        }
        Ok(())
    }

    /// `VICTORY_METAL_GPUPROF_ARGS` for a geometry-shader `ExecuteIndirect` record: two snapshots
    /// (consecutive ring entries) of what the expansion wrote at `action_va`: the draw's first
    /// three words (vertex or index count, instance count, start) and the mesh threadgroup
    /// counts the draw reads (`GS_GROUPS_AT`). The table sums them per pipeline.
    pub(crate) fn snap_gs_record(&mut self, action_va: u64) -> Result<()> {
        if self.prof.is_none()
            || !std::env::var_os("VICTORY_METAL_GPUPROF_ARGS").is_some_and(|v| v != "0")
        {
            return Ok(());
        }
        let Some(sn) = crate::prof::snap(&self.queue.device().clone()) else {
            return Ok(());
        };
        // The kernel reads what the expansion wrote.
        self.barrier(DISPATCH, DISPATCH, VIS_DEVICE)?;
        let (first, d0) = sn.take();
        let (_, d1) = sn.take();
        for (src, dst) in [
            (action_va, d0),
            (action_va + crate::indirect::GS_GROUPS_AT, d1),
        ] {
            let params = self.alloc_constants(16, 16)?;
            params.write_u32s(
                0,
                &[
                    src as u32,
                    (src >> 32) as u32,
                    dst as u32,
                    (dst >> 32) as u32,
                ],
            )?;
            self.set_address(30, params.gpu_va)?;
            self.dispatch(&sn.pipeline, [1, 1, 1], [1, 1, 1])?;
        }
        self.keep_buffers.push(sn.ring.clone());
        self.last_snap = Some(first);
        Ok(())
    }

    /// `VICTORY_METAL_GS_GROUP_CAP`: clamps the mesh threadgroup counts (x, y, z) the expansion wrote at
    /// `action_va` (`GS_GROUPS_AT`) to the cap the executor set, if any.
    pub(crate) fn cap_gs_groups(&mut self, action_va: u64) -> Result<()> {
        let Some(cap) = crate::prof::GS_CAP.with(|c| c.get()) else {
            return Ok(());
        };
        let Some(k) = crate::prof::cap_kernel(&self.queue.device().clone()) else {
            return Ok(());
        };
        self.barrier(DISPATCH, DISPATCH, VIS_DEVICE)?;
        let params = self.alloc_constants(16, 16)?;
        let at = action_va;
        params.write_u32s(0, &[at as u32, (at >> 32) as u32, cap, 0])?;
        self.set_address(30, params.gpu_va)?;
        self.dispatch(k, [1, 1, 1], [1, 1, 1])?;
        // The draw reads the threadgroup counts next.
        self.barrier(
            DISPATCH,
            stages::OBJECT | stages::MESH | stages::VERTEX,
            VIS_DEVICE,
        )?;
        Ok(())
    }

    /// `Dispatch`: `groups` threadgroups of `threads` threads.
    pub fn dispatch(
        &mut self,
        pipeline: &ComputePipeline,
        groups: [u32; 3],
        threads: [u32; 3],
    ) -> Result<()> {
        self.live()?;
        if groups.contains(&0) {
            return Ok(());
        }
        let per_group: u64 = threads.iter().map(|&t| u64::from(t)).product();
        if per_group == 0 || per_group > u64::from(pipeline.max_threads_per_threadgroup()) {
            return Err(Error::BadArgument(format!(
                "{threads:?} threads per group; the pipeline allows {}",
                pipeline.max_threads_per_threadgroup()
            )));
        }
        let p = pipeline.clone();
        let key = p.identity();
        // After `prepare`: it may open a new encoder, which has no pipeline
        // set (and `last_pipeline` is forgotten with the old encoder).
        self.prepare(DISPATCH)?;
        let new_pipeline = self.last_pipeline != Some(key);
        let encoder = self.encoder_ref()?;
        if new_pipeline {
            encoder.setComputePipelineState(p.raw());
        }
        encoder.dispatchThreadgroups_threadsPerThreadgroup(
            MTLSize {
                width: groups[0] as usize,
                height: groups[1] as usize,
                depth: groups[2] as usize,
            },
            MTLSize {
                width: threads[0] as usize,
                height: threads[1] as usize,
                depth: threads[2] as usize,
            },
        );
        self.last_pipeline = Some(key);
        self.stats.dispatches += 1;
        if new_pipeline {
            self.keep_pipelines.push(p);
        }
        Ok(())
    }

    /// A stage-to-stage barrier: the work of the `producer` stages must be
    /// finished and visible (`vis`, [`stages`] bits) before the `consumer`
    /// stages start. The wire call `BARRIER{MEMORY}` carries the same masks as
    /// `before` = producer and `after` = consumer. Nothing is encoded until a
    /// command shows where the barrier belongs.
    pub fn barrier(&mut self, producer: u16, consumer: u16, vis: u8) -> Result<()> {
        self.live()?;
        stages::to_mtl_stages(producer)?;
        stages::to_mtl_stages(consumer)?;
        stages::to_mtl_visibility(vis)?;
        // The tracker speaks Metal's words: `after` the producer stages,
        // `before` the consumer stages.
        self.tracker.barrier(producer, consumer, vis);
        if producer & BLIT != 0 && consumer & BLIT != 0 {
            self.blit_writes.clear();
            self.copies.accesses.clear();
        }
        Ok(())
    }

    /// `BARRIER{ALIASING}`: memory that two resources share is reused; every
    /// stage before is ordered against every stage after, with the alias
    /// visibility.
    pub fn aliasing_barrier(&mut self) -> Result<()> {
        self.barrier(stages::ALL, stages::ALL, stages::VIS_RESOURCE_ALIAS)
    }

    /// Start of a split barrier: only recorded (`BEGIN_ONLY` of the original API; the
    /// wire protocol sends the barrier at the end point alone, so the
    /// interpreter normally never calls this).
    pub fn split_barrier_begin(&mut self, id: u64, producer: u16, vis: u8) -> Result<()> {
        self.live()?;
        stages::to_mtl_stages(producer)?;
        stages::to_mtl_visibility(vis)?;
        self.splits.insert(id, (producer, vis));
        Ok(())
    }

    /// End of a split barrier: this is where it is issued, as a normal
    /// deferred barrier with the producer stages and visibility given at the
    /// begin.
    pub fn split_barrier_end(&mut self, id: u64, consumer: u16) -> Result<()> {
        let (producer, vis) = self
            .splits
            .remove(&id)
            .ok_or_else(|| Error::BadArgument(format!("split barrier {id} was never begun")))?;
        self.barrier(producer, consumer, vis)
    }

    /// Ends the current encoder: the next command starts a new one (barriers
    /// across the boundary become queue-level barriers).
    pub fn split_encoder(&mut self) -> Result<()> {
        self.live()?;
        let actions = self.tracker.split();
        self.perform(actions)
    }

    /// Finishes the list: barriers nobody consumed are encoded in producer
    /// form (device-visibility ones are left out when the queue orders lists
    /// itself, see [`crate::barrier`]), and the command buffer is ended.
    pub fn end(&mut self) -> Result<()> {
        self.live()?;
        let actions = self.tracker.finish();
        self.perform(actions)?;
        self.cb().endCommandBuffer();
        self.ended = true;
        Ok(())
    }
}

impl Drop for CommandList {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.take() {
            // Abandoned without being submitted: close what is open so the
            // command buffer can be reused, and give the slot back.
            if let Some(encoder) = self.encoder.take() {
                encoder.endEncoding();
            }
            if let Some(encoder) = self.render.take() {
                encoder.endEncoding();
            }
            if !self.ended {
                slot.cb.endCommandBuffer();
            }
            self.queue.return_slot(slot, 0);
        }
    }
}
