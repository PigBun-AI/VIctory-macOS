//! The synchronous calls (`Link::call`): one handler per `SyncOp`.

use std::sync::Arc;
use std::time::Instant;

use victory_metal_proto::ids::{FenceId, HeapId, PsoId, QueueId, ResId, RootSigId};
use victory_metal_proto::root::{RootSigDesc, root_flags};
use victory_metal_proto::status::Status;
use victory_metal_proto::sync::*;
use victory_metal_proto::types::{BufferForm, BufferKind, HeapKind, QueueKind, ViewKind};
use victory_metal_proto::{Caps, PROTO_VERSION};

use super::tlab::TlabLayout;
use super::{ExecOptions, Executor, Heap, Pso, PsoBody, RootSig, State, lock, status_of, typed};
use crate::descriptors::{DescriptorHeap, ENTRY_SIZE, IrDescriptorTableEntry};
use crate::device::Device;
use crate::memory::{Buffer, BufferKind as CoreKind};
use crate::queue::{DeviceStatus, Queue};

/// Queues one executor makes. Each has a constant ring and command
/// buffer pool; the number is a guard, not a measured limit.
const MAX_QUEUES: u32 = 16;
/// Largest descriptor heap: 2^21 slots (the API promises 1,000,000; D3DMetal takes 2^21, and
/// victory-shader's `ResourceDescriptorHeap[]` tests use that size).
const MAX_HEAP_SLOTS: u32 = 1 << 21;
/// Largest root signature description accepted by `CREATE_ROOT_SIGNATURE`.
const MAX_ROOT_SIG_BYTES: u64 = 1 << 20;
/// How long `DESTROY_QUEUE` and `SHUTDOWN` wait for a queue to drain.
const DRAIN_MS: u64 = 10_000;

pub(super) fn exhausted<T>(_: T) -> Status {
    Status::OUT_OF_MEMORY
}

impl State {
    pub(super) fn new(device: Device, options: ExecOptions) -> State {
        State {
            device,
            options,
            resources: Default::default(),
            heaps: Default::default(),
            root_sigs: Default::default(),
            psos: Default::default(),
            fences: Default::default(),
            queues: Default::default(),
        }
    }

    /// What the device offers, as proto's capability bits.
    pub(super) fn caps_bits(&self) -> Caps {
        let c = self.device.caps();
        let mut bits = Caps::METAL4 | Caps::ARGUMENT_BUFFERS_TIER2 | Caps::WRAP_MEMORY;
        if c.unified_memory {
            bits = bits | Caps::UNIFIED_MEMORY;
        }
        if c.mesh_shaders {
            bits = bits | Caps::MESH_SHADERS;
        }
        if c.atomics64 {
            bits = bits | Caps::ATOMICS64_HEAP;
        }
        if c.translated {
            bits = bits | Caps::TRANSLATED;
        }
        bits
    }

    /// The handler of every call that needs the state and does not wait.
    pub(super) fn call(&mut self, op: SyncOp, args: *mut u8, len: usize) -> Status {
        match op {
            SyncOp::QUERY_CAPS => typed(args, len, |a| self.query_caps(a)),
            SyncOp::CREATE_QUEUE => typed(args, len, |a| self.create_queue(a)),
            SyncOp::CREATE_BUFFER => typed(args, len, |a| self.create_buffer(a)),
            SyncOp::DESTROY_BUFFER => typed::<DestroyBufferArgs>(args, len, |a| {
                self.resources
                    .remove(a.res.0)
                    .map(drop)
                    .ok_or(Status::NOT_FOUND)
            }),
            SyncOp::WRAP_MEMORY => typed(args, len, |a| self.wrap_memory(a)),
            SyncOp::CREATE_DESC_HEAP => typed(args, len, |a| self.create_desc_heap(a)),
            SyncOp::DESTROY_DESC_HEAP => typed::<DestroyDescHeapArgs>(args, len, |a| {
                self.heaps
                    .remove(a.heap.0)
                    .map(drop)
                    .ok_or(Status::NOT_FOUND)
            }),
            SyncOp::WRITE_DESCRIPTOR => typed(args, len, |a| self.write_descriptor(a)),
            SyncOp::COPY_DESCRIPTORS => typed(args, len, |a| self.copy_descriptors(a)),
            SyncOp::CREATE_ROOT_SIGNATURE => typed(args, len, |a| self.create_root_signature(a)),
            SyncOp::DESTROY_ROOT_SIGNATURE => typed::<DestroyRootSignatureArgs>(args, len, |a| {
                self.root_sigs
                    .remove(a.root_sig.0)
                    .map(drop)
                    .ok_or(Status::NOT_FOUND)
            }),
            SyncOp::DESTROY_PIPELINE => typed::<DestroyPipelineArgs>(args, len, |a| {
                self.psos.remove(a.pso.0).map(drop).ok_or(Status::NOT_FOUND)
            }),
            SyncOp::CREATE_FENCE => typed(args, len, |a| self.create_fence(a)),
            SyncOp::DESTROY_FENCE => typed::<DestroyFenceArgs>(args, len, |a| {
                self.fences
                    .remove(a.fence.0)
                    .map(drop)
                    .ok_or(Status::NOT_FOUND)
            }),
            SyncOp::FENCE_VALUE => typed::<FenceValueArgs>(args, len, |a| {
                a.value = self
                    .fences
                    .get(a.fence.0)
                    .ok_or(Status::NOT_FOUND)?
                    .completed_value();
                Ok(())
            }),
            SyncOp::FENCE_SIGNAL => typed::<FenceSignalArgs>(args, len, |a| {
                let f = self.fences.get(a.fence.0).ok_or(Status::NOT_FOUND)?;
                // Never lowers the value.
                if a.value > f.completed_value() {
                    f.signal_cpu(a.value);
                }
                Ok(())
            }),
            SyncOp::DEVICE_STATUS => typed::<DeviceStatusArgs>(args, len, |a| {
                a.state = self.device_state();
                Ok(())
            }),
            // The surface comes with M3.
            SyncOp::GET_SURFACE_LAYER => {
                typed::<GetSurfaceLayerArgs>(args, len, |_| Err(Status::UNSUPPORTED))
            }
            // HELLO, SHUTDOWN, NOP, FENCE_WAIT and DESTROY_QUEUE are handled
            // before the state is taken; anything else proto knows of is
            // not a sync call this version defines.
            _ => Status::UNKNOWN_OP,
        }
    }

    fn query_caps(&mut self, a: &mut QueryCapsArgs) -> Result<(), Status> {
        let c = self.device.caps();
        a.caps = self.caps_bits();
        a.max_buffer_len = c.max_buffer_len;
        a.working_set = c.recommended_working_set;
        a.page_size = c.page_size as u32;
        a.descriptor_stride = ENTRY_SIZE as u32;
        a.sampler_stride = ENTRY_SIZE as u32;
        a.max_threads_per_group = c.max_threads_per_threadgroup[0].min(u64::from(u32::MAX)) as u32;
        a.simd_width = c.simd_width;
        a.max_queues = MAX_QUEUES;
        Ok(())
    }

    fn create_queue(&mut self, a: &mut CreateQueueArgs) -> Result<(), Status> {
        if !QueueKind(a.kind).is_known() {
            return Err(Status::INVALID_ARG);
        }
        // Priority is a hint the Metal 4 queue has no field for.
        if self.queues.len() as u32 >= MAX_QUEUES {
            return Err(Status::OUT_OF_MEMORY);
        }
        let q = Queue::with_options(&self.device, self.options.queue).map_err(|e| status_of(&e))?;
        a.queue = QueueId(self.queues.insert(q).map_err(exhausted)?);
        Ok(())
    }

    fn create_buffer(&mut self, a: &mut CreateBufferArgs) -> Result<(), Status> {
        if a.flags != 0 {
            return Err(Status::INVALID_ARG);
        }
        let kind = match BufferKind(a.kind) {
            BufferKind::DEFAULT => CoreKind::Default,
            BufferKind::UPLOAD => CoreKind::Upload,
            BufferKind::READBACK => CoreKind::Readback,
            BufferKind::SHARED => CoreKind::Shared,
            _ => return Err(Status::INVALID_ARG),
        };
        let buffer = self
            .device
            .create_buffer(kind, a.size)
            .map_err(|e| match e {
                // Metal said no to an allocation that passed our checks.
                crate::Error::Metal(_) => Status::OUT_OF_MEMORY,
                e => status_of(&e),
            })?;
        a.gpu_va = buffer.gpu_va();
        a.cpu_addr = buffer.cpu_addr();
        a.alloc_size = buffer.alloc_size();
        a.res = ResId(self.resources.insert(buffer).map_err(exhausted)?);
        Ok(())
    }

    fn wrap_memory(&mut self, a: &mut WrapMemoryArgs) -> Result<(), Status> {
        if a.flags != 0 {
            return Err(Status::INVALID_ARG);
        }
        let (addr, len) = (a.addr as usize, a.len as usize);
        if u64::try_from(len).ok() != Some(a.len) || addr.checked_add(len).is_none() {
            return Err(Status::INVALID_ARG);
        }
        let page = self.device.caps().page_size as usize;
        if addr == 0 || !addr.is_multiple_of(page) || len == 0 || !len.is_multiple_of(page) {
            return Err(Status::INVALID_ARG);
        }
        if !range_is_mapped(addr, len) {
            return Err(Status::INVALID_ARG);
        }
        // SAFETY: the range was checked to be mapped; staying mapped for the
        // buffer's life (and the GPU work after it) is the contract of
        // `WRAP_MEMORY`, which the front keeps (it destroys the buffer
        // before it unmaps, and destruction waits for the GPU).
        let buffer = unsafe { self.device.wrap(addr as *mut u8, len) }.map_err(|e| match e {
            crate::Error::Metal(_) => Status::OUT_OF_MEMORY,
            e => status_of(&e),
        })?;
        a.gpu_va = buffer.gpu_va();
        a.res = ResId(self.resources.insert(buffer).map_err(exhausted)?);
        Ok(())
    }

    fn create_desc_heap(&mut self, a: &mut CreateDescHeapArgs) -> Result<(), Status> {
        let sampler = match HeapKind(a.kind) {
            HeapKind::CBV_SRV_UAV => false,
            HeapKind::SAMPLER => true,
            // Render-target and depth-stencil heaps come with M3.
            HeapKind::RTV | HeapKind::DSV => return Err(Status::UNSUPPORTED),
            _ => return Err(Status::INVALID_ARG),
        };
        if a.flags & !1 != 0 || a.capacity == 0 || a.capacity > MAX_HEAP_SLOTS {
            return Err(Status::INVALID_ARG);
        }
        let heap = DescriptorHeap::new(&self.device, a.capacity).map_err(|e| status_of(&e))?;
        a.stride = heap.stride();
        a.gpu_va = heap.gpu_va();
        a.cpu_addr = heap.cpu_addr();
        let entry = Heap {
            heap,
            sampler,
            shader_visible: a.flags & 1 != 0,
        };
        a.heap = HeapId(self.heaps.insert(entry).map_err(exhausted)?);
        Ok(())
    }

    fn write_descriptor(&mut self, a: &mut WriteDescriptorArgs) -> Result<(), Status> {
        let h = self.heaps.get(a.heap.0).ok_or(Status::NOT_FOUND)?;
        let v = a.view;
        if !v.is_valid() {
            return Err(Status::INVALID_ARG);
        }
        if a.index >= h.heap.capacity() {
            return Err(Status::INVALID_ARG);
        }
        let result = match v.kind {
            ViewKind::NONE => h.heap.write(a.index, IrDescriptorTableEntry::default()),
            // Sampler states are not made yet (they come with the texture
            // work); the heap itself exists and descriptors can be copied.
            ViewKind::SAMPLER => return Err(Status::UNSUPPORTED),
            ViewKind::CBV => {
                if h.sampler {
                    return Err(Status::INVALID_ARG);
                }
                // The address goes through the live-buffer check.
                h.heap
                    .write_buffer_view(a.index, v.gpu_va, u64::from(v.size_bytes), false)
            }
            ViewKind::BUFFER_SRV | ViewKind::BUFFER_UAV => {
                if h.sampler {
                    return Err(Status::INVALID_ARG);
                }
                if v.flags & victory_metal_proto::ViewDesc::HAS_COUNTER != 0 {
                    // The counter is a second buffer MSC reads through its own
                    // convention; not wired.
                    return Err(Status::UNSUPPORTED);
                }
                let buffer = self.resources.get(v.resource.0).ok_or(Status::NOT_FOUND)?;
                let unit = match v.form {
                    BufferForm::RAW => 4,
                    BufferForm::STRUCTURED => u64::from(v.structure_stride),
                    // Typed views need a texture view of the buffer.
                    _ => return Err(Status::UNSUPPORTED),
                };
                if unit == 0 {
                    return Err(Status::INVALID_ARG);
                }
                let first = v.first_element.checked_mul(unit);
                let size = u64::from(v.num_elements).checked_mul(unit);
                let (Some(first), Some(size)) = (first, size) else {
                    return Err(Status::INVALID_ARG);
                };
                if first.checked_add(size).is_none_or(|end| end > buffer.len()) {
                    return Err(Status::INVALID_ARG);
                }
                if size == 0 {
                    h.heap.write(a.index, IrDescriptorTableEntry::default())
                } else {
                    h.heap
                        .write_buffer_view(a.index, buffer.gpu_va() + first, size, false)
                }
            }
            _ => return Err(Status::INVALID_ARG),
        };
        result.map_err(|e| status_of(&e))
    }

    fn copy_descriptors(&mut self, a: &mut CopyDescriptorsArgs) -> Result<(), Status> {
        let dst = self.heaps.get(a.dst_heap.0).ok_or(Status::NOT_FOUND)?;
        let src = self.heaps.get(a.src_heap.0).ok_or(Status::NOT_FOUND)?;
        if dst.sampler != src.sampler {
            return Err(Status::INVALID_ARG);
        }
        let end = |index: u32, h: &Heap| {
            index
                .checked_add(a.count)
                .is_some_and(|e| e <= h.heap.capacity())
        };
        if !end(a.src_index, src) || !end(a.dst_index, dst) {
            return Err(Status::INVALID_ARG);
        }
        // Read everything first: overlap inside one heap behaves like memmove.
        let entries = (0..a.count)
            .map(|i| src.heap.read(a.src_index + i))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| status_of(&e))?;
        for (i, e) in entries.into_iter().enumerate() {
            dst.heap
                .write(a.dst_index + i as u32, e)
                .map_err(|e| status_of(&e))?;
        }
        Ok(())
    }

    fn create_root_signature(&mut self, a: &mut CreateRootSignatureArgs) -> Result<(), Status> {
        if a.data_addr == 0 || a.data_len == 0 || a.data_len > MAX_ROOT_SIG_BYTES {
            return Err(Status::INVALID_ARG);
        }
        // SAFETY: the `Link` contract: the bytes are the caller's, valid for
        // the call (`CreateRootSignatureArgs::data_addr`).
        let bytes =
            unsafe { std::slice::from_raw_parts(a.data_addr as *const u8, a.data_len as usize) };
        let desc = RootSigDesc::decode(bytes).map_err(|_| Status::INVALID_ARG)?;
        debug_assert_eq!(desc.flags & !root_flags::ALL_BITS, 0);
        let msc_desc = super::dxil::to_msc(&desc);
        // MSC decides where each parameter sits in the argument buffer: ask it
        // (reflection). Without MSC the rule-based layout stands in; a
        // pipeline cannot be made then anyway.
        let layout = match crate::msc::Msc::get().and_then(|m| m.layout(&msc_desc)) {
            Ok(l) => super::dxil::layout_from_msc(&desc, &l).map_err(|_| Status::INVALID_ARG)?,
            Err(_) => TlabLayout::from_desc(&desc).map_err(|_| Status::INVALID_ARG)?,
        };
        let (static_table, static_samplers) =
            if desc.static_samplers.is_empty() || layout.static_table_offset().is_none() {
                (None, Vec::new())
            } else {
                let table = DescriptorHeap::new(&self.device, desc.static_samplers.len() as u32)
                    .map_err(|e| status_of(&e))?;
                let mut keep = Vec::new();
                for (i, s) in desc.static_samplers.iter().enumerate() {
                    let sampler = self
                        .device
                        .create_sampler(&super::dxil::sampler_spec(&s.sampler))
                        .map_err(|e| status_of(&e))?;
                    table
                        .write(i as u32, sampler.entry())
                        .map_err(|e| status_of(&e))?;
                    keep.push(sampler);
                }
                (Some(table), keep)
            };
        let sig = Arc::new(RootSig {
            desc,
            msc_desc,
            layout,
            static_table,
            static_samplers,
        });
        a.root_sig = RootSigId(self.root_sigs.insert(sig).map_err(exhausted)?);
        Ok(())
    }

    fn create_fence(&mut self, a: &mut CreateFenceArgs) -> Result<(), Status> {
        if a.flags != 0 {
            return Err(Status::INVALID_ARG);
        }
        let fence = self.device.create_fence().map_err(|e| status_of(&e))?;
        if a.initial != 0 {
            fence.signal_cpu(a.initial);
        }
        a.fence = FenceId(self.fences.insert(fence).map_err(exhausted)?);
        Ok(())
    }

    /// 0 fine, 1 hung, 2 removed: the worst any queue reports.
    fn device_state(&self) -> u32 {
        self.queues
            .values()
            .map(|q| match q.status() {
                DeviceStatus::Healthy => 0,
                DeviceStatus::Hung => 1,
                DeviceStatus::Removed => 2,
            })
            .max()
            .unwrap_or(0)
    }

    pub(super) fn pipeline_from_msl(
        &mut self,
        root_sig: RootSigId,
        source: &str,
        entry: &str,
        threads: [u32; 3],
    ) -> Result<PsoId, Status> {
        let sig = self
            .root_sigs
            .get(root_sig.0)
            .ok_or(Status::NOT_FOUND)?
            .clone();
        if threads.contains(&0) {
            return Err(Status::INVALID_ARG);
        }
        let pipeline = self
            .device
            .compute_pipeline_from_msl(source, entry)
            .map_err(|e| status_of(&e))?;
        let pso = Arc::new(Pso {
            body: PsoBody::Compute { pipeline, threads },
            root_sig: sig,
        });
        Ok(PsoId(self.psos.insert(pso).map_err(exhausted)?))
    }

    pub(super) fn set_tlab_layout(
        &mut self,
        root_sig: RootSigId,
        offsets: &[u32],
        size: u32,
    ) -> Result<(), Status> {
        let old = self
            .root_sigs
            .get(root_sig.0)
            .ok_or(Status::NOT_FOUND)?
            .clone();
        let layout = TlabLayout::from_reflection(&old.desc, offsets, size)
            .map_err(|_| Status::INVALID_ARG)?;
        // Same handle, new object: pipelines made earlier keep the old layout
        // and a dispatch with both refuses.
        let new = Arc::new(RootSig {
            desc: old.desc.clone(),
            msc_desc: old.msc_desc.clone(),
            layout,
            static_table: old.static_table.clone(),
            static_samplers: old.static_samplers.clone(),
        });
        self.root_sigs.replace(root_sig.0, new);
        Ok(())
    }

    pub(super) fn buffer(&self, id: ResId) -> Result<&Buffer, Status> {
        self.resources.get(id.0).ok_or(Status::NOT_FOUND)
    }
}

/// Whether every page of `[addr, addr + len)` is mapped in this process.
/// `msync` with `MS_ASYNC` answers `ENOMEM` for a range with a hole in it
/// and does nothing to a mapped one (`mincore` does not notice holes on
/// macOS, tested).
pub(super) fn range_is_mapped(addr: usize, len: usize) -> bool {
    // SAFETY: `msync` only inspects the mappings of the range; it reads or
    // writes none of the memory.
    let rc = unsafe { libc::msync(addr as *mut libc::c_void, len, libc::MS_ASYNC) };
    rc == 0
}

impl Executor {
    pub(super) fn hello(&self, a: &mut HelloArgs) -> Result<(), Status> {
        if a.client_proto_version != PROTO_VERSION {
            return Err(Status::BAD_VERSION);
        }
        if a.required_caps.0 & !Caps::KNOWN.0 != 0 {
            return Err(Status::UNSUPPORTED);
        }
        let mut g = lock(&self.state);
        // A second HELLO shares the device the first one made.
        if g.is_none() {
            let device = Device::new().map_err(|e| status_of(&e))?;
            *g = Some(State::new(device, self.options));
        }
        let st = g.as_ref().expect("made above");
        let caps = st.caps_bits();
        if !caps.contains(a.required_caps) {
            return Err(Status::UNSUPPORTED);
        }
        a.proto_version = PROTO_VERSION;
        a.caps = caps;
        a.device_name = [0; 96];
        let name = st.device.caps().name.as_bytes();
        let n = name.len().min(a.device_name.len() - 1);
        a.device_name[..n].copy_from_slice(&name[..n]);
        Ok(())
    }

    /// Drains every queue, then drops everything, the device last.
    pub(super) fn shutdown(&self) -> Result<(), Status> {
        let queues: Vec<Queue> = {
            let g = lock(&self.state);
            let st = g.as_ref().ok_or(Status::NOT_INITIALIZED)?;
            st.queues.values().cloned().collect()
        };
        // A queue that never drains (a wait nobody signals) is dropped anyway.
        for q in &queues {
            let _ = q.wait_idle(DRAIN_MS);
        }
        drop(queues);
        let taken = lock(&self.state).take();
        drop(taken);
        Ok(())
    }

    pub(super) fn destroy_queue(&self, a: &mut DestroyQueueArgs) -> Result<(), Status> {
        let queue = self.with_state(|st| st.queues.get(a.queue.0).cloned())?;
        let queue = queue.ok_or(Status::NOT_FOUND)?;
        // Drained first, without holding the state: other calls go on.
        queue.wait_idle(DRAIN_MS).map_err(|e| status_of(&e))?;
        self.with_state(|st| st.queues.remove(a.queue.0))?
            .map(drop)
            .ok_or(Status::NOT_FOUND)
    }

    pub(super) fn fence_wait(&self, a: &mut FenceWaitArgs) -> Result<(), Status> {
        let fence = self.with_state(|st| st.fences.get(a.fence.0).cloned())?;
        let fence = fence.ok_or(Status::NOT_FOUND)?;
        // In slices, without the state held: submits and other waits go on,
        // and a `SHUTDOWN` is noticed.
        let started = Instant::now();
        loop {
            let slice = if a.timeout_ms == u64::MAX {
                250
            } else {
                let spent = started.elapsed().as_millis() as u64;
                if spent >= a.timeout_ms {
                    return if fence.completed_value() >= a.value {
                        Ok(())
                    } else {
                        Err(Status::TIMEOUT)
                    };
                }
                (a.timeout_ms - spent).min(250)
            };
            if fence.wait(a.value, slice) {
                return Ok(());
            }
            if lock(&self.state).is_none() {
                return Err(Status::NOT_INITIALIZED);
            }
        }
    }
}
