//! The D3D12 objects, as plain Rust: what the COM shell calls.
//!
//! Everything here talks to the back only through `victory_metal_proto`:
//! creation and destruction are sync calls (`Gpu::call`), and a command list
//! is a `Vec<Item>` that [`encode_block`] turns into one proto block per
//! `ExecuteCommandLists`. Handles are the back's; only `ListId` is ours.
//!
//! Objects hold an `Arc<Device>` and destroy their back-side object on drop
//! (the back waits for the GPU, rule 11), so the shell's reference counting
//! is enough lifetime management.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use victory_metal_proto::cmd::{
    self, Barrier, BeginList, EndList, QueueSignal, QueueWait, SetDescriptorHeaps,
};
use victory_metal_proto::cmd::{
    CopyBufferRegion, Dispatch, SetPipeline, SetRootDescriptor, SetRootSignature, SetRootTable,
};
use victory_metal_proto::sync::*;
use victory_metal_proto::types::{self as pt, BindPoint, RootParamKind, ViewDesc, ViewKind};
use victory_metal_proto::{
    Caps as PCaps, Command, Encoder, FenceId, HeapId, Link, PROTO_VERSION, PsoId, QueueId, ResId,
    RootSigId, Status,
};

use crate::adapter::AdapterInfo;
use crate::caps::{self, Caps};
use crate::convert;
use crate::descriptor::{self, DescriptorSpace, HeapDesc, HeapRange, SlotHeap};
use crate::gpu::{Gpu, status_to_hr};
use crate::hresult::{Hr, Result};
use crate::resource::{self, ResourceDesc};
use crate::rootsig;
use crate::state::{self, StateTracker};

/// What the front needs from the back, fixed at connect time.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_buffer_len: u64,
    pub working_set: u64,
    pub page_size: u32,
    pub descriptor_stride: u32,
    pub sampler_stride: u32,
    pub max_threads_per_group: u32,
    pub simd_width: u32,
    pub max_queues: u32,
}

/// The connection to the back: `HELLO` done, adapter and limits known.
/// `SHUTDOWN` when the last reference goes.
pub struct System {
    pub gpu: Gpu,
    pub adapter: AdapterInfo,
    pub caps: Caps,
    pub proto_caps: PCaps,
    pub limits: Limits,
}

/// Capabilities the front insists on.
pub const REQUIRED_CAPS: PCaps = PCaps::METAL4;

impl System {
    /// `HELLO` and `QUERY_CAPS`. A back that refuses is "no usable adapter":
    /// the caller reports `DXGI_ERROR_UNSUPPORTED`.
    pub fn open(link: Arc<dyn Link>) -> Result<Arc<System>> {
        let gpu = Gpu::new(link);
        let mut hello = HelloArgs::new();
        hello.client_proto_version = PROTO_VERSION;
        hello.required_caps = REQUIRED_CAPS;
        gpu.call(&mut hello)?;
        let mut q = QueryCapsArgs::new();
        gpu.call(&mut q)?;
        let limits = Limits {
            max_buffer_len: q.max_buffer_len,
            working_set: q.working_set,
            page_size: q.page_size,
            descriptor_stride: q.descriptor_stride,
            sampler_stride: q.sampler_stride,
            max_threads_per_group: q.max_threads_per_group,
            simd_width: q.simd_width,
            max_queues: q.max_queues,
        };
        if limits.descriptor_stride == 0 || limits.sampler_stride == 0 {
            return Err(Hr::E_FAIL);
        }
        let name_len = hello
            .device_name
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(hello.device_name.len());
        let mut adapter = AdapterInfo::placeholder();
        adapter.name = String::from_utf8_lossy(&hello.device_name[..name_len]).into_owned();
        adapter.dedicated_video_memory = limits.working_set;
        adapter.shared_system_memory = limits.working_set;
        let mut c = Caps::apple();
        c.wave_lane_min = limits.simd_width;
        c.wave_lane_max = limits.simd_width;
        c.unified_memory = q.caps.contains(PCaps::UNIFIED_MEMORY);
        c.mesh_shader_tier = if q.caps.contains(PCaps::MESH_SHADERS) {
            caps::MESH_SHADER_TIER_1
        } else {
            0
        };
        Ok(Arc::new(System {
            gpu,
            adapter,
            caps: c,
            proto_caps: q.caps,
            limits,
        }))
    }
}

impl Drop for System {
    fn drop(&mut self) {
        let mut a = ShutdownArgs::new();
        let _ = self.gpu.call_status(&mut a);
    }
}

#[derive(Default)]
struct Descriptors {
    space: DescriptorSpace,
    back: std::collections::HashMap<SlotHeap, HeapId>,
}

pub struct Device {
    pub sys: Arc<System>,
    desc: Mutex<Descriptors>,
    tracker: Mutex<StateTracker>,
    next_list: AtomicU32,
}

impl Device {
    pub fn new(sys: Arc<System>) -> Arc<Device> {
        Arc::new(Device {
            sys,
            desc: Mutex::new(Descriptors {
                space: DescriptorSpace::new(),
                ..Default::default()
            }),
            tracker: Mutex::new(StateTracker::default()),
            next_list: AtomicU32::new(1),
        })
    }

    pub fn gpu(&self) -> &Gpu {
        &self.sys.gpu
    }

    pub fn descriptor_increment(&self, ty: u32) -> u32 {
        match ty {
            descriptor::TYPE_SAMPLER => self.sys.limits.sampler_stride,
            // RTV/DSV heaps are M3; give them the CBV/SRV/UAV size.
            _ => self.sys.limits.descriptor_stride,
        }
    }

    /// `ID3D12Device::GetDeviceRemovedReason`.
    pub fn removed_reason(&self) -> Hr {
        let mut a = DeviceStatusArgs::new();
        match self.gpu().call_status(&mut a) {
            Status::OK => match a.state {
                0 => Hr::S_OK,
                1 => Hr::DXGI_ERROR_DEVICE_HUNG,
                _ => Hr::DXGI_ERROR_DEVICE_REMOVED,
            },
            s => status_to_hr(s),
        }
    }

    // ---- buffers --------------------------------------------------------

    /// `CreateCommittedResource`, buffers only (textures are M3).
    pub fn create_committed_buffer(
        self: &Arc<Self>,
        heap_type: u32,
        desc: &ResourceDesc,
        initial_state: u32,
    ) -> Result<Buffer> {
        let desc = resource::validate_committed(heap_type, desc, initial_state)?;
        if !desc.is_buffer() || heap_type == resource::HEAP_CUSTOM {
            return Err(Hr::E_NOTIMPL);
        }
        let mut a = CreateBufferArgs::new();
        a.size = desc.width;
        a.kind = match heap_type {
            resource::HEAP_UPLOAD => pt::BufferKind::UPLOAD,
            resource::HEAP_READBACK => pt::BufferKind::READBACK,
            _ => pt::BufferKind::DEFAULT,
        }
        .0;
        self.gpu().call(&mut a)?;
        self.tracker.lock().unwrap().add(a.res.0, 1, initial_state);
        Ok(Buffer {
            dev: self.clone(),
            res: a.res,
            gpu_va: a.gpu_va,
            cpu_addr: a.cpu_addr,
            heap_type,
            desc,
        })
    }

    // ---- queues, fences -------------------------------------------------

    pub fn create_queue(self: &Arc<Self>, kind: u32, priority: u32) -> Result<Queue> {
        let kind = match kind {
            0 => pt::QueueKind::DIRECT,
            1 => pt::QueueKind::COMPUTE,
            2 => pt::QueueKind::COPY,
            _ => return Err(Hr::E_INVALIDARG),
        };
        let mut a = CreateQueueArgs::new();
        a.kind = kind.0;
        a.priority = priority;
        self.gpu().call(&mut a)?;
        Ok(Queue {
            dev: self.clone(),
            id: a.queue,
        })
    }

    pub fn create_fence(self: &Arc<Self>, initial: u64) -> Result<Fence> {
        let mut a = CreateFenceArgs::new();
        a.initial = initial;
        self.gpu().call(&mut a)?;
        Ok(Fence {
            dev: self.clone(),
            id: a.fence,
        })
    }

    // ---- root signatures and pipelines ---------------------------------

    /// `CreateRootSignature`: a serialized blob (any version).
    pub fn create_root_signature(self: &Arc<Self>, blob: &[u8]) -> Result<RootSignature> {
        let d = rootsig::deserialize(blob)?;
        let proto = convert::root_signature(&d)?;
        let bytes = proto.encode().map_err(|_| Hr::E_INVALIDARG)?;
        let mut a = CreateRootSignatureArgs::new();
        a.data_addr = bytes.as_ptr() as u64;
        a.data_len = bytes.len() as u64;
        self.gpu().call(&mut a)?;
        Ok(RootSignature {
            dev: self.clone(),
            id: a.root_sig,
            blob: blob.to_vec(),
            desc: d,
        })
    }

    pub fn create_compute_pipeline(
        self: &Arc<Self>,
        rs: &RootSignature,
        dxil: &[u8],
    ) -> Result<Pipeline> {
        let mut a = CreateComputePipelineArgs::new();
        a.root_sig = rs.id;
        a.dxil_addr = dxil.as_ptr() as u64;
        a.dxil_len = dxil.len() as u64;
        self.gpu().call(&mut a)?;
        Ok(Pipeline {
            dev: self.clone(),
            id: a.pso,
        })
    }

    // ---- descriptors ----------------------------------------------------

    pub fn create_descriptor_heap(self: &Arc<Self>, d: &HeapDesc) -> Result<DescriptorHeap> {
        descriptor::validate_heap_desc(d)?;
        let kind = match d.ty {
            descriptor::TYPE_CBV_SRV_UAV => pt::HeapKind::CBV_SRV_UAV,
            descriptor::TYPE_SAMPLER => pt::HeapKind::SAMPLER,
            // RTV and DSV: M3. The heap exists (applications create them
            // alongside the others) but holds nothing the back needs.
            _ => return self.create_unbacked_heap(d),
        };
        let mut a = CreateDescHeapArgs::new();
        a.kind = kind.0;
        a.capacity = d.count;
        a.flags = d.flags & descriptor::FLAG_SHADER_VISIBLE;
        self.gpu().call(&mut a)?;
        let range = {
            let mut g = self.desc.lock().unwrap();
            let r = g.space.alloc(d.count, a.stride, d.shader_visible());
            g.back.insert(r.id, a.heap);
            r
        };
        Ok(DescriptorHeap {
            dev: self.clone(),
            desc: *d,
            range,
            back: a.heap,
        })
    }

    fn create_unbacked_heap(self: &Arc<Self>, d: &HeapDesc) -> Result<DescriptorHeap> {
        let stride = self.sys.limits.descriptor_stride;
        let range = self
            .desc
            .lock()
            .unwrap()
            .space
            .alloc(d.count, stride, false);
        Ok(DescriptorHeap {
            dev: self.clone(),
            desc: *d,
            range,
            back: HeapId::NULL,
        })
    }

    fn resolve_cpu(&self, handle: u64) -> Result<(HeapId, u32)> {
        let g = self.desc.lock().unwrap();
        let (slot, index) = g.space.resolve_cpu(handle).ok_or(Hr::E_INVALIDARG)?;
        let back = *g.back.get(&slot).ok_or(Hr::E_NOTIMPL)?;
        Ok((back, index))
    }

    /// The `(heap, slot)` a shader-visible GPU handle names.
    pub fn resolve_gpu(&self, handle: u64) -> Result<(HeapId, u32)> {
        let g = self.desc.lock().unwrap();
        let (slot, index) = g.space.resolve_gpu(handle).ok_or(Hr::E_INVALIDARG)?;
        Ok((*g.back.get(&slot).ok_or(Hr::E_INVALIDARG)?, index))
    }

    fn write_descriptor(
        &self,
        cpu_handle: u64,
        view: ViewDesc,
        sampler: Option<pt::SamplerDesc>,
    ) -> Result<()> {
        let (heap, index) = self.resolve_cpu(cpu_handle)?;
        let mut a = WriteDescriptorArgs::new();
        a.heap = heap;
        a.index = index;
        a.view = view;
        if let Some(s) = sampler {
            a.sampler = s;
            a.view.kind = ViewKind::SAMPLER;
        }
        self.gpu().call(&mut a)
    }

    /// `CreateConstantBufferView`; a zero `size` or address is a null view.
    pub fn create_cbv(&self, cpu_handle: u64, gpu_va: u64, size: u32) -> Result<()> {
        let v = if gpu_va == 0 {
            ViewDesc::null()
        } else {
            ViewDesc::cbv(gpu_va, size)
        };
        self.write_descriptor(cpu_handle, v, None)
    }

    /// `CreateShaderResourceView` / `CreateUnorderedAccessView` on a buffer.
    pub fn create_buffer_view(&self, cpu_handle: u64, v: &BufferView) -> Result<()> {
        self.write_descriptor(cpu_handle, v.to_proto()?, None)
    }

    pub fn create_sampler(&self, cpu_handle: u64, s: &convert::SamplerParams) -> Result<()> {
        self.write_descriptor(cpu_handle, ViewDesc::null(), Some(convert::sampler(s)?))
    }

    /// `CopyDescriptorsSimple`.
    pub fn copy_descriptors(&self, dst_cpu: u64, src_cpu: u64, count: u32) -> Result<()> {
        let (dh, di) = self.resolve_cpu(dst_cpu)?;
        let (sh, si) = self.resolve_cpu(src_cpu)?;
        let mut a = CopyDescriptorsArgs::new();
        a.dst_heap = dh;
        a.dst_index = di;
        a.src_heap = sh;
        a.src_index = si;
        a.count = count;
        self.gpu().call(&mut a)
    }

    // ---- recording and execution ---------------------------------------

    fn next_list_id(&self) -> u32 {
        self.next_list.fetch_add(1, Ordering::Relaxed)
    }
}

/// A buffer view's description, in D3D12 terms already reduced to what a
/// buffer can have.
#[derive(Clone, Copy, Debug)]
pub struct BufferView {
    pub uav: bool,
    /// `None` for a null view.
    pub resource: Option<ResId>,
    pub first_element: u64,
    pub num_elements: u32,
    pub structure_stride: u32,
    pub format: u32,
    pub raw: bool,
    pub counter: Option<(ResId, u64)>,
}

impl BufferView {
    pub fn to_proto(&self) -> Result<ViewDesc> {
        let kind = if self.uav {
            ViewKind::BUFFER_UAV
        } else {
            ViewKind::BUFFER_SRV
        };
        let Some(res) = self.resource else {
            return Ok(ViewDesc::null());
        };
        let mut v = if self.raw {
            ViewDesc::raw(kind, res, self.first_element, self.num_elements)
        } else if self.structure_stride != 0 {
            ViewDesc::structured(
                kind,
                res,
                self.structure_stride,
                self.first_element,
                self.num_elements,
            )
        } else if self.format != 0 {
            ViewDesc::typed(
                kind,
                res,
                self.format,
                self.first_element,
                self.num_elements,
            )
        } else {
            return Err(Hr::E_INVALIDARG);
        };
        if let Some((c, off)) = self.counter {
            v.flags |= ViewDesc::HAS_COUNTER;
            v.counter_resource = c;
            v.counter_offset = off;
        }
        Ok(v)
    }
}

// ---- objects ---------------------------------------------------------------

pub struct Buffer {
    dev: Arc<Device>,
    pub res: ResId,
    pub gpu_va: u64,
    pub cpu_addr: u64,
    pub heap_type: u32,
    pub desc: ResourceDesc,
}

impl Buffer {
    /// `Map`: the CPU address of the contents; only upload and readback
    /// buffers have one.
    pub fn map(&self, subresource: u32) -> Result<u64> {
        if subresource != 0 || self.cpu_addr == 0 {
            return Err(Hr::E_INVALIDARG);
        }
        Ok(self.cpu_addr)
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        self.dev.tracker.lock().unwrap().remove(self.res.0);
        let mut a = DestroyBufferArgs::new();
        a.res = self.res;
        let _ = self.dev.gpu().call_status(&mut a);
    }
}

pub struct RootSignature {
    dev: Arc<Device>,
    pub id: RootSigId,
    /// The blob as the application gave it (`GetRootSignature`-style queries).
    pub blob: Vec<u8>,
    pub desc: rootsig::RootSignatureDesc,
}

impl Drop for RootSignature {
    fn drop(&mut self) {
        let mut a = DestroyRootSignatureArgs::new();
        a.root_sig = self.id;
        let _ = self.dev.gpu().call_status(&mut a);
    }
}

pub struct Pipeline {
    dev: Arc<Device>,
    pub id: PsoId,
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        let mut a = DestroyPipelineArgs::new();
        a.pso = self.id;
        let _ = self.dev.gpu().call_status(&mut a);
    }
}

pub struct DescriptorHeap {
    dev: Arc<Device>,
    pub desc: HeapDesc,
    pub range: HeapRange,
    pub back: HeapId,
}

impl DescriptorHeap {
    pub fn cpu_start(&self) -> u64 {
        self.range.cpu_base
    }
    pub fn gpu_start(&self) -> u64 {
        self.range.gpu_base
    }
}

impl Drop for DescriptorHeap {
    fn drop(&mut self) {
        let mut g = self.dev.desc.lock().unwrap();
        g.space.free(self.range.id);
        g.back.remove(&self.range.id);
        drop(g);
        if !self.back.is_null() {
            let mut a = DestroyDescHeapArgs::new();
            a.heap = self.back;
            let _ = self.dev.gpu().call_status(&mut a);
        }
    }
}

pub struct Fence {
    dev: Arc<Device>,
    pub id: FenceId,
}

impl Fence {
    pub fn completed(&self) -> u64 {
        let mut a = FenceValueArgs::new();
        a.fence = self.id;
        match self.dev.gpu().call_status(&mut a) {
            Status::OK => a.value,
            // A fence the back lost reads as "everything done", like a removed device.
            _ => u64::MAX,
        }
    }

    /// `ID3D12Fence::Signal`.
    pub fn signal(&self, value: u64) -> Result<()> {
        let mut a = FenceSignalArgs::new();
        a.fence = self.id;
        a.value = value;
        self.dev.gpu().call(&mut a)
    }

    /// Wait for `value`; `Ok(false)` when the time ran out.
    pub fn wait(&self, value: u64, timeout_ms: u64) -> Result<bool> {
        let mut a = FenceWaitArgs::new();
        a.fence = self.id;
        a.value = value;
        a.timeout_ms = timeout_ms;
        match self.dev.gpu().call_status(&mut a) {
            Status::OK => Ok(true),
            Status::TIMEOUT => Ok(false),
            s => Err(status_to_hr(s)),
        }
    }
}

impl Drop for Fence {
    fn drop(&mut self) {
        let mut a = DestroyFenceArgs::new();
        a.fence = self.id;
        let _ = self.dev.gpu().call_status(&mut a);
    }
}

pub struct Queue {
    dev: Arc<Device>,
    pub id: QueueId,
}

impl Queue {
    /// `ExecuteCommandLists`: one block, the lists in order.
    pub fn execute(&self, lists: &[Arc<[Item]>]) -> Result<()> {
        let block = encode_block(self.dev.sys.proto_caps, self.id, lists, || {
            self.dev.next_list_id()
        })?;
        self.dev.gpu().submit(&block)
    }

    /// `ID3D12CommandQueue::Signal`.
    pub fn signal(&self, fence: &Fence, value: u64) -> Result<()> {
        let mut e = Encoder::new(self.dev.sys.proto_caps);
        e.push(&Command::QueueSignal(QueueSignal {
            queue: self.id,
            fence: fence.id,
            value,
        }))
        .map_err(|_| Hr::E_FAIL)?;
        self.dev.gpu().submit(&e.finish())
    }

    /// `ID3D12CommandQueue::Wait`.
    pub fn wait(&self, fence: &Fence, value: u64) -> Result<()> {
        let mut e = Encoder::new(self.dev.sys.proto_caps);
        e.push(&Command::QueueWait(QueueWait {
            queue: self.id,
            fence: fence.id,
            value,
        }))
        .map_err(|_| Hr::E_FAIL)?;
        self.dev.gpu().submit(&e.finish())
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        let mut a = DestroyQueueArgs::new();
        a.queue = self.id;
        let _ = self.dev.gpu().call_status(&mut a);
    }
}

// ---- command lists --------------------------------------------------------

/// One recorded call. `Command` borrows its variable part, so root
/// constants are held by value until the block is encoded.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Cmd(Command<'static>),
    RootConstants {
        bind_point: BindPoint,
        root_index: u32,
        dest_offset: u32,
        values: Vec<u32>,
    },
}

/// Lists → one block. `new_list_id` hands out ids unique within the block.
pub fn encode_block(
    caps: PCaps,
    queue: QueueId,
    lists: &[Arc<[Item]>],
    mut new_list_id: impl FnMut() -> u32,
) -> Result<Vec<u8>> {
    let mut e = Encoder::new(caps);
    let bad = |_| Hr::E_INVALIDARG;
    for items in lists {
        let list = victory_metal_proto::ListId(new_list_id());
        e.push(&Command::BeginList(BeginList {
            list,
            queue,
            flags: 0,
            _pad: 0,
        }))
        .map_err(bad)?;
        for it in items.iter() {
            match it {
                Item::Cmd(c) => e.push(c).map_err(bad)?,
                Item::RootConstants {
                    bind_point,
                    root_index,
                    dest_offset,
                    values,
                } => e
                    .set_root_constants(*bind_point, *root_index, *dest_offset, values)
                    .map_err(bad)?,
            }
        }
        e.push(&Command::EndList(EndList { list, _pad: 0 }))
            .map_err(bad)?;
    }
    Ok(e.finish())
}

/// `D3D12_RESOURCE_BARRIER`, reduced to buffers.
#[derive(Clone, Copy, Debug)]
pub enum ResourceBarrier {
    Transition {
        res: ResId,
        subresource: u32,
        before: u32,
        after: u32,
        flags: u32,
    },
    Uav {
        res: Option<ResId>,
    },
    Aliasing {
        before: Option<ResId>,
        after: Option<ResId>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListState {
    Recording,
    Closed,
}

/// `ID3D12GraphicsCommandList` (compute subset).
pub struct CommandList {
    dev: Arc<Device>,
    pub kind: u32,
    items: Vec<Item>,
    pub state: ListState,
    /// First error found while recording; reported by `close`, as D3D12's
    /// debug layer and drivers do.
    error: Option<Hr>,
}

impl CommandList {
    /// Created in the recording state.
    pub fn new(dev: &Arc<Device>, kind: u32) -> CommandList {
        CommandList {
            dev: dev.clone(),
            kind,
            items: Vec::new(),
            state: ListState::Recording,
            error: None,
        }
    }

    fn push(&mut self, c: Command<'static>) {
        if self.state == ListState::Recording {
            self.items.push(Item::Cmd(c));
        }
    }

    fn fail(&mut self, e: Hr) {
        self.error.get_or_insert(e);
    }

    /// `Close`. A second `Close` returns S_OK, as D3DMetal does.
    pub fn close(&mut self) -> Result<Arc<[Item]>> {
        self.state = ListState::Closed;
        match self.error {
            Some(e) => Err(e),
            None => Ok(Arc::from(self.items.clone())),
        }
    }

    /// `Reset`: records again from empty.
    pub fn reset(&mut self) {
        self.items.clear();
        self.error = None;
        self.state = ListState::Recording;
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn copy_buffer_region(
        &mut self,
        dst: &Buffer,
        dst_offset: u64,
        src: &Buffer,
        src_offset: u64,
        size: u64,
    ) {
        let fits = |off: u64, b: &Buffer| off.checked_add(size).is_some_and(|e| e <= b.desc.width);
        if !fits(dst_offset, dst) || !fits(src_offset, src) {
            self.fail(Hr::E_INVALIDARG);
            return;
        }
        self.push(Command::CopyBufferRegion(CopyBufferRegion {
            dst: dst.res,
            src: src.res,
            dst_offset,
            src_offset,
            size,
        }));
    }

    pub fn resource_barrier(&mut self, barriers: &[ResourceBarrier]) {
        for b in barriers {
            match *b {
                ResourceBarrier::Transition {
                    res,
                    subresource,
                    before,
                    after,
                    flags,
                } => {
                    let r = self.dev.tracker.lock().unwrap().transition(
                        res.0,
                        subresource,
                        before,
                        after,
                        flags,
                    );
                    match r {
                        Ok(Some(sb)) => self.push(stage_barrier(sb, res)),
                        Ok(None) => {}
                        Err(_) => self.fail(Hr::E_INVALIDARG),
                    }
                }
                ResourceBarrier::Uav { res } => {
                    self.push(stage_barrier(
                        state::uav_barrier(),
                        res.unwrap_or(ResId::NULL),
                    ));
                }
                ResourceBarrier::Aliasing { before, after } => {
                    self.push(Command::Barrier(Barrier::aliasing(
                        before.unwrap_or(ResId::NULL),
                        after.unwrap_or(ResId::NULL),
                    )));
                }
            }
        }
    }

    pub fn set_pipeline(&mut self, p: &Pipeline) {
        self.push(Command::SetPipeline(SetPipeline { pso: p.id, _pad: 0 }));
    }

    pub fn set_compute_root_signature(&mut self, rs: &RootSignature) {
        self.push(Command::SetRootSignature(SetRootSignature {
            bind_point: BindPoint::COMPUTE,
            root_sig: rs.id,
        }));
    }

    /// `SetDescriptorHeaps`: at most one CBV/SRV/UAV and one sampler heap.
    pub fn set_descriptor_heaps(&mut self, heaps: &[&DescriptorHeap]) {
        let (mut cbv, mut smp) = (HeapId::NULL, HeapId::NULL);
        for h in heaps {
            let slot = match h.desc.ty {
                descriptor::TYPE_CBV_SRV_UAV => &mut cbv,
                descriptor::TYPE_SAMPLER => &mut smp,
                _ => {
                    self.fail(Hr::E_INVALIDARG);
                    return;
                }
            };
            if !slot.is_null() || !h.desc.shader_visible() {
                self.fail(Hr::E_INVALIDARG);
                return;
            }
            *slot = h.back;
        }
        self.push(Command::SetDescriptorHeaps(SetDescriptorHeaps {
            cbv_srv_uav: cbv,
            sampler: smp,
        }));
    }

    pub fn set_compute_root_constants(
        &mut self,
        root_index: u32,
        dest_offset: u32,
        values: &[u32],
    ) {
        if values.len() > cmd::MAX_ROOT_CONSTANTS {
            self.fail(Hr::E_INVALIDARG);
            return;
        }
        if self.state == ListState::Recording {
            self.items.push(Item::RootConstants {
                bind_point: BindPoint::COMPUTE,
                root_index,
                dest_offset,
                values: values.to_vec(),
            });
        }
    }

    /// `SetComputeRootConstantBufferView` / `ShaderResourceView` / `UnorderedAccessView`.
    pub fn set_compute_root_descriptor(
        &mut self,
        root_index: u32,
        kind: RootParamKind,
        gpu_va: u64,
    ) {
        self.push(Command::SetRootDescriptor(SetRootDescriptor {
            bind_point: BindPoint::COMPUTE,
            root_index,
            kind,
            _pad: 0,
            gpu_va,
        }));
    }

    /// `SetComputeRootDescriptorTable`: `gpu_handle` is a handle of a shader-visible heap.
    pub fn set_compute_root_table(&mut self, root_index: u32, gpu_handle: u64) {
        match self.dev.resolve_gpu(gpu_handle) {
            Ok((heap, index)) => self.push(Command::SetRootTable(SetRootTable {
                bind_point: BindPoint::COMPUTE,
                root_index,
                heap,
                index,
            })),
            Err(e) => self.fail(e),
        }
    }

    pub fn dispatch(&mut self, x: u32, y: u32, z: u32) {
        self.push(Command::Dispatch(Dispatch { x, y, z, _pad: 0 }));
    }

    /// A call outside this milestone: remembered, reported at `close`.
    pub fn unsupported(&mut self) {
        self.fail(Hr::E_NOTIMPL);
    }
}

fn stage_barrier(sb: state::StageBarrier, res: ResId) -> Command<'static> {
    let mut b = Barrier::memory(sb.before, sb.after);
    b.visibility = sb.visibility;
    b.res_before = res;
    b.res_after = res;
    Command::Barrier(b)
}
