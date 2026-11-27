//! Recording: one front-end command list is one `MTL4CommandBuffer`.
//!
//! Copies, fills and dispatches share one `MTL4ComputeCommandEncoder` (there
//! is no blit encoder in Metal 4). Barriers are **deferred**: a barrier is
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
    MTL4CommandAllocator, MTL4CommandBuffer, MTL4CommandEncoder, MTL4ComputeCommandEncoder, MTLSize,
};
use std::collections::HashMap;
use std::ptr::NonNull;

use crate::barrier::{Action, EncoderKind, Tracker};
use crate::descriptors::{ArgumentTable, DescriptorHeap, RingAlloc, bind_point};
use crate::error::{Error, Result};
use crate::memory::Buffer;
use crate::pipeline::ComputePipeline;
use crate::queue::{Queue, Slot};
use crate::stages::{self, BLIT, DISPATCH};

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
}

/// A command list being recorded. Made by [`Queue::begin_list`], finished
/// with [`end`](Self::end), handed to [`Queue::submit`].
pub struct CommandList {
    queue: Queue,
    slot: Option<Slot>,
    tracker: Tracker,
    encoder: Option<ComputeEncoder>,
    table: ArgumentTable,
    trailing_barrier: bool,
    ended: bool,
    keep_buffers: Vec<Buffer>,
    keep_pipelines: Vec<ComputePipeline>,
    last_pipeline: Option<*const ()>,
    splits: HashMap<u64, (u16, u8)>,
    stats: ListStats,
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
        let trailing_barrier = self.inter_submit_barrier();
        Ok(CommandList {
            queue: self.clone(),
            table: slot.table.clone(),
            slot: Some(slot),
            tracker: Tracker::new(trailing_barrier),
            encoder: None,
            trailing_barrier,
            ended: false,
            keep_buffers: Vec::new(),
            keep_pipelines: Vec::new(),
            last_pipeline: None,
            splits: HashMap::new(),
            stats: ListStats::default(),
        })
    }
}

impl CommandList {
    fn cb(&self) -> &ProtocolObject<dyn MTL4CommandBuffer> {
        &self.slot.as_ref().expect("list is live").cb
    }

    pub(crate) fn queue(&self) -> &Queue {
        &self.queue
    }

    pub(crate) fn is_ended(&self) -> bool {
        self.ended
    }

    pub(crate) fn raw_command_buffer(&self) -> NonNull<ProtocolObject<dyn MTL4CommandBuffer>> {
        NonNull::from(self.cb())
    }

    /// The list was committed with timeline value `value`: its allocator
    /// is busy until then.
    pub(crate) fn into_submitted(mut self, value: u64) {
        if let Some(slot) = self.slot.take() {
            self.queue.return_slot(slot, value);
        }
        // `keep_*` drop here, after the commit: anything retired now waits
        // for `value`.
    }

    /// What the list has encoded so far.
    pub fn stats(&self) -> ListStats {
        self.stats
    }

    /// Space in the constant ring (see [`Queue::alloc_constants`]).
    pub fn alloc_constants(&self, size: u64, align: u64) -> Result<RingAlloc> {
        self.queue.alloc_constants(size, align)
    }

    fn live(&self) -> Result<()> {
        if self.ended {
            Err(Error::BadArgument("command list already ended".into()))
        } else {
            Ok(())
        }
    }

    fn perform(&mut self, actions: Vec<Action>) -> Result<()> {
        for action in actions {
            match action {
                Action::Open(EncoderKind::Compute) => {
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
                    return Err(Error::Unsupported("render encoders arrive with M3".into()));
                }
                Action::Close => {
                    if let Some(encoder) = self.encoder.take() {
                        encoder.endEncoding();
                    }
                }
                Action::Encoder { after, before, vis } => {
                    self.encoder_ref()?
                        .barrierAfterEncoderStages_beforeEncoderStages_visibilityOptions(
                            stages::to_mtl_stages(after)?,
                            stages::to_mtl_stages(before)?,
                            stages::to_mtl_visibility(vis)?,
                        );
                    self.stats.encoder_barriers += 1;
                }
                Action::Consumer { after, before, vis } => {
                    self.encoder_ref()?
                        .barrierAfterQueueStages_beforeStages_visibilityOptions(
                            stages::to_mtl_stages(after)?,
                            stages::to_mtl_stages(before)?,
                            stages::to_mtl_visibility(vis)?,
                        );
                    self.stats.consumer_barriers += 1;
                }
                Action::Producer { after, before, vis } => {
                    self.encoder_ref()?
                        .barrierAfterStages_beforeQueueStages_visibilityOptions(
                            stages::to_mtl_stages(after)?,
                            stages::to_mtl_stages(before)?,
                            stages::to_mtl_visibility(vis)?,
                        );
                    self.stats.producer_barriers += 1;
                }
            }
        }
        Ok(())
    }

    fn encoder_ref(&self) -> Result<&ComputeEncoder> {
        self.encoder
            .as_ref()
            .ok_or_else(|| Error::Metal("no open encoder".into()))
    }

    /// Makes the encoder for a command of `stage` current and encodes the
    /// barriers it needs.
    fn prepare(&mut self, stage: u16) -> Result<&ComputeEncoder> {
        let actions = self
            .tracker
            .command(stage)
            .ok_or_else(|| Error::BadArgument(format!("no encoder for stage {stage:#x}")))?;
        self.perform(actions)?;
        self.encoder_ref()
    }

    fn check_buffer(&self, buffer: &Buffer, offset: u64, size: u64, what: &str) -> Result<()> {
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

    /// Binds the top-level argument buffer (bind point 2) to a ring
    /// allocation holding root constants, root descriptors and table
    /// pointers.
    pub fn set_root_arguments(&mut self, tlab: &RingAlloc) -> Result<()> {
        self.live()?;
        self.table
            .set_address(bind_point::ARGUMENT_BUFFER as u32, tlab.gpu_va)
    }

    /// Binds a GPU address at an arbitrary buffer slot.
    pub fn set_address(&mut self, index: u32, gpu_va: u64) -> Result<()> {
        self.live()?;
        self.table.set_address(index, gpu_va)
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
        let new_pipeline = self.last_pipeline != Some(key);
        let encoder = self.prepare(DISPATCH)?;
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
    /// form, the trailing all-stage barrier is added when the queue asks for
    /// it, and the command buffer is ended.
    pub fn end(&mut self) -> Result<()> {
        self.live()?;
        let actions = self.tracker.finish(self.trailing_barrier);
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
            if !self.ended {
                slot.cb.endCommandBuffer();
            }
            self.queue.return_slot(slot, 0);
        }
    }
}
