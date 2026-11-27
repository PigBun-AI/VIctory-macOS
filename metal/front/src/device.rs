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
use victory_metal_proto::wire::Wire;
use victory_metal_proto::{
    Caps as PCaps, Command, Encoder, FenceId, HeapId, Link, MemHeapId, PROTO_VERSION, PsoId,
    QueueId, ResId, RootSigId, Status,
};

use crate::adapter::AdapterInfo;
use crate::caps::{self, Caps};
use crate::convert;
use crate::descriptor::{self, DescriptorSpace, HeapDesc, HeapRange, SlotHeap};
use crate::footprint;
use crate::gpu::{Gpu, status_to_hr};
use crate::hresult::{Hr, Result};
use crate::resource::{self, HeapProps, Memory, ResourceDesc};
use crate::rootsig;
use crate::state::{self, StateTracker};
use crate::view::{Record, TargetView, TextureView};

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
    /// What the front keeps about a slot: render-target and depth-stencil
    /// views (the back has no memory for them) and texture UAVs (clears name a
    /// descriptor). Keyed by the front's heap number and the slot.
    records: std::collections::HashMap<(SlotHeap, u32), Record>,
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

    // ---- resources ------------------------------------------------------

    /// `CreateCommittedResource`: a buffer or a texture with its own memory.
    pub fn create_committed(
        self: &Arc<Self>,
        props: HeapProps,
        desc: &ResourceDesc,
        initial_state: u32,
    ) -> Result<Resource> {
        let mem = props.memory()?;
        let desc = resource::validate_committed(props.ty, desc, initial_state)?;
        if desc.is_buffer() {
            let mut a = CreateBufferArgs::new();
            a.size = desc.width;
            a.kind = memory_buffer_kind(mem).0;
            self.gpu().call(&mut a)?;
            Ok(self.track(
                a.res,
                a.gpu_va,
                a.cpu_addr,
                props,
                desc,
                initial_state,
                None,
            ))
        } else {
            // A texture in CPU-visible custom memory is made like any other
            // (private storage): there is no `Map` for it (the original
            // API's row-major textures are not built).
            let res = self.make_texture(&desc, None)?;
            Ok(self.track(res, 0, 0, props, desc, initial_state, None))
        }
    }

    /// `CreateCommittedResource` of a buffer on one of the plain heap types.
    pub fn create_committed_buffer(
        self: &Arc<Self>,
        heap_type: u32,
        desc: &ResourceDesc,
        initial_state: u32,
    ) -> Result<Buffer> {
        if !desc.is_buffer() {
            return Err(Hr::E_NOTIMPL);
        }
        self.create_committed(HeapProps::of_type(heap_type), desc, initial_state)
    }

    /// `CreateHeap`. `size` must be a multiple of `alignment` (0 = 64 KiB;
    /// 4 MiB for multisampled textures).
    pub fn create_heap(
        self: &Arc<Self>,
        props: HeapProps,
        size: u64,
        alignment: u64,
        flags: u32,
    ) -> Result<Heap> {
        let mem = props.memory()?;
        let alignment = if alignment == 0 {
            resource::DEFAULT_ALIGNMENT
        } else {
            alignment
        };
        if size == 0
            || !(alignment == resource::DEFAULT_ALIGNMENT || alignment == resource::MSAA_ALIGNMENT)
            || !size.is_multiple_of(alignment)
        {
            return Err(Hr::E_INVALIDARG);
        }
        let mut a = CreateHeapArgs::new();
        a.kind = memory_heap_kind(mem);
        a.size = size;
        self.gpu().call(&mut a)?;
        Ok(Heap {
            dev: self.clone(),
            id: a.heap,
            props,
            flags,
            size,
            alignment,
        })
    }

    /// `CreatePlacedResource`: the resource lives at `offset` of the heap and
    /// shares its bytes with whatever else is placed there (ordered by
    /// aliasing barriers).
    pub fn create_placed(
        self: &Arc<Self>,
        heap: &Arc<Heap>,
        offset: u64,
        desc: &ResourceDesc,
        initial_state: u32,
    ) -> Result<Resource> {
        let desc = resource::validate_committed(heap.props.ty, desc, initial_state)?;
        if !offset.is_multiple_of(heap.alignment) || offset >= heap.size {
            return Err(Hr::E_INVALIDARG);
        }
        if desc.is_buffer() {
            let mut a = CreatePlacedBufferArgs::new();
            a.heap = heap.id;
            a.offset = offset;
            a.size = desc.width;
            self.gpu().call(&mut a)?;
            Ok(self.track(
                a.res,
                a.gpu_va,
                a.cpu_addr,
                heap.props,
                desc,
                initial_state,
                Some(heap.clone()),
            ))
        } else {
            let res = self.make_texture(&desc, Some((heap, offset)))?;
            Ok(self.track(
                res,
                0,
                0,
                heap.props,
                desc,
                initial_state,
                Some(heap.clone()),
            ))
        }
    }

    fn make_texture(&self, d: &ResourceDesc, at: Option<(&Heap, u64)>) -> Result<ResId> {
        use victory_metal_proto::types::TexDim;
        if d.layout == resource::LAYOUT_ROW_MAJOR {
            return Err(Hr::E_NOTIMPL);
        }
        let mut a = CreateTextureArgs::new();
        a.dimension = match d.dimension {
            resource::DIM_TEXTURE1D => TexDim::TEX1D,
            resource::DIM_TEXTURE2D => TexDim::TEX2D,
            _ => TexDim::TEX3D,
        };
        a.format = d.format;
        a.width = d.width as u32;
        a.height = d.height;
        a.depth_or_array = d.depth_or_array as u32;
        a.mips = d.mips as u32;
        a.samples = d.sample_count;
        a.usage = d.texture_usage();
        if let Some((h, off)) = at {
            a.heap = h.id;
            a.heap_offset = off;
        }
        self.gpu().call(&mut a)?;
        Ok(a.res)
    }

    #[allow(clippy::too_many_arguments)]
    fn track(
        self: &Arc<Self>,
        res: ResId,
        gpu_va: u64,
        cpu_addr: u64,
        props: HeapProps,
        desc: ResourceDesc,
        initial_state: u32,
        heap: Option<Arc<Heap>>,
    ) -> Resource {
        self.tracker
            .lock()
            .unwrap()
            .add(res.0, desc.subresource_count(), initial_state);
        Resource {
            dev: self.clone(),
            res,
            gpu_va,
            cpu_addr,
            heap_type: props.ty,
            props,
            desc,
            _heap: heap,
        }
    }

    /// `GetResourceAllocationInfo` for resources placed one after the other
    /// in a default heap: total size and the alignment to place them at.
    /// `None` for a description that cannot be made (the application gets
    /// `UINT64_MAX`, as D3D12 does).
    pub fn allocation_info(&self, descs: &[ResourceDesc]) -> Option<(u64, u64)> {
        let mut at = 0u64;
        let mut align = resource::DEFAULT_ALIGNMENT;
        for d in descs {
            let d = resource::validate_committed(resource::HEAP_DEFAULT, d, state::COMMON).ok()?;
            let (size, a) = if d.is_buffer() {
                (d.width, resource::DEFAULT_ALIGNMENT)
            } else {
                let mut q = GetResourceAllocationInfoArgs::new();
                q.kind = pt::ResKind::TEXTURE;
                q.dimension = match d.dimension {
                    resource::DIM_TEXTURE1D => pt::TexDim::TEX1D,
                    resource::DIM_TEXTURE2D => pt::TexDim::TEX2D,
                    _ => pt::TexDim::TEX3D,
                };
                q.format = d.format;
                q.width = d.width as u32;
                q.height = d.height;
                q.depth_or_array = d.depth_or_array as u32;
                q.mips = d.mips as u32;
                q.samples = d.sample_count;
                q.usage = d.texture_usage();
                q.heap_kind = pt::MemKind::DEFAULT;
                self.gpu().call(&mut q).ok()?;
                (q.size, q.alignment.max(d.alignment))
            };
            let a = a.max(resource::DEFAULT_ALIGNMENT);
            align = align.max(a);
            at = at.div_ceil(a) * a + size.div_ceil(a) * a;
        }
        Some((at.div_ceil(align) * align, align))
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

    /// `CreateGraphicsPipelineState` / `ID3D12Device2::CreatePipelineState`
    /// for a vertex or mesh pipeline. `desc` comes from
    /// [`crate::pso::to_proto`]; the stages' DXIL it points at is only read
    /// for the call.
    pub fn create_graphics_pipeline(
        self: &Arc<Self>,
        rs: &RootSignature,
        desc: &victory_metal_proto::pso::GraphicsPsoDesc,
    ) -> Result<Pipeline> {
        let mut a = CreateGraphicsPipelineArgs::new();
        a.root_sig = rs.id;
        a.desc = *desc;
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

    /// The front's `(heap, slot)` of a CPU handle.
    fn slot_of(&self, handle: u64) -> Result<(SlotHeap, u32)> {
        self.desc
            .lock()
            .unwrap()
            .space
            .resolve_cpu(handle)
            .ok_or(Hr::E_INVALIDARG)
    }

    fn write_descriptor(
        &self,
        cpu_handle: u64,
        view: ViewDesc,
        sampler: Option<pt::SamplerDesc>,
    ) -> Result<()> {
        self.write_descriptor_rec(cpu_handle, view, sampler, None)
    }

    fn write_descriptor_rec(
        &self,
        cpu_handle: u64,
        view: ViewDesc,
        sampler: Option<pt::SamplerDesc>,
        record: Option<Record>,
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
        self.gpu().call(&mut a)?;
        let (slot, index) = self.slot_of(cpu_handle)?;
        self.set_record(slot, index, record);
        Ok(())
    }

    fn set_record(&self, slot: SlotHeap, index: u32, record: Option<Record>) {
        let mut g = self.desc.lock().unwrap();
        match record {
            Some(r) => g.records.insert((slot, index), r),
            None => g.records.remove(&(slot, index)),
        };
    }

    /// `CreateShaderResourceView` / `CreateUnorderedAccessView` on a texture.
    pub fn create_texture_view(&self, cpu_handle: u64, v: &TextureView) -> Result<()> {
        let rec = (v.uav && v.resource.is_some()).then_some(Record::Uav(*v));
        self.write_descriptor_rec(cpu_handle, v.to_proto(), None, rec)
    }

    /// `CreateRenderTargetView` / `CreateDepthStencilView`: the heap has no
    /// memory on the back; the record is the descriptor. `None` is a null view.
    pub fn create_target_view(&self, cpu_handle: u64, v: Option<TargetView>) -> Result<()> {
        let (slot, index) = self.slot_of(cpu_handle)?;
        self.set_record(slot, index, v.map(Record::Target));
        Ok(())
    }

    /// The render-target or depth-stencil view a CPU handle names.
    pub fn target_view(&self, cpu_handle: u64) -> Option<TargetView> {
        let (slot, index) = self.slot_of(cpu_handle).ok()?;
        match self.desc.lock().unwrap().records.get(&(slot, index)) {
            Some(Record::Target(t)) => Some(*t),
            _ => None,
        }
    }

    /// The texture UAV recorded in a descriptor, by CPU handle, else by the
    /// GPU handle of a shader-visible heap.
    pub fn texture_uav(&self, cpu_handle: u64, gpu_handle: u64) -> Option<TextureView> {
        let g = self.desc.lock().unwrap();
        let key = g
            .space
            .resolve_cpu(cpu_handle)
            .filter(|k| g.records.contains_key(k))
            .or_else(|| g.space.resolve_gpu(gpu_handle))?;
        match g.records.get(&key) {
            Some(Record::Uav(v)) => Some(*v),
            _ => None,
        }
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

    /// `CopyDescriptorsSimple`. Heaps the back knows copy there; the front's
    /// own records (render targets, texture UAVs) go along either way.
    pub fn copy_descriptors(&self, dst_cpu: u64, src_cpu: u64, count: u32) -> Result<()> {
        let (ds, di) = self.slot_of(dst_cpu)?;
        let (ss, si) = self.slot_of(src_cpu)?;
        let (db, sb) = {
            let g = self.desc.lock().unwrap();
            (g.back.get(&ds).copied(), g.back.get(&ss).copied())
        };
        match (db, sb) {
            (Some(dh), Some(sh)) => {
                let mut a = CopyDescriptorsArgs::new();
                a.dst_heap = dh;
                a.dst_index = di;
                a.src_heap = sh;
                a.src_index = si;
                a.count = count;
                self.gpu().call(&mut a)?;
            }
            (None, None) => {}
            _ => return Err(Hr::E_INVALIDARG),
        }
        let mut g = self.desc.lock().unwrap();
        // Overlapping ranges within one heap: read all first.
        let moved: Vec<Option<Record>> = (0..count)
            .map(|i| g.records.get(&(ss, si + i)).copied())
            .collect();
        for (i, r) in moved.into_iter().enumerate() {
            match r {
                Some(r) => g.records.insert((ds, di + i as u32), r),
                None => g.records.remove(&(ds, di + i as u32)),
            };
        }
        Ok(())
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

/// A heap of memory resources can be placed in (`ID3D12Heap`).
pub struct Heap {
    dev: Arc<Device>,
    pub id: MemHeapId,
    pub props: HeapProps,
    pub flags: u32,
    pub size: u64,
    pub alignment: u64,
}

impl Drop for Heap {
    fn drop(&mut self) {
        let mut a = DestroyHeapArgs::new();
        a.heap = self.id;
        let _ = self.dev.gpu().call_status(&mut a);
    }
}

/// A buffer or a texture, committed or placed.
pub struct Resource {
    dev: Arc<Device>,
    pub res: ResId,
    pub gpu_va: u64,
    pub cpu_addr: u64,
    pub heap_type: u32,
    pub props: HeapProps,
    pub desc: ResourceDesc,
    /// A placed resource keeps its heap alive.
    _heap: Option<Arc<Heap>>,
}

/// Where buffers were all there was.
pub type Buffer = Resource;

impl Resource {
    /// `Map`: the CPU address of the contents; only buffers in CPU-visible
    /// memory have one.
    pub fn map(&self, subresource: u32) -> Result<u64> {
        if subresource != 0 || self.cpu_addr == 0 {
            return Err(Hr::E_INVALIDARG);
        }
        Ok(self.cpu_addr)
    }

    pub fn is_texture(&self) -> bool {
        !self.desc.is_buffer()
    }
}

impl Drop for Resource {
    fn drop(&mut self) {
        self.dev.tracker.lock().unwrap().remove(self.res.0);
        if self.desc.is_buffer() {
            let mut a = DestroyBufferArgs::new();
            a.res = self.res;
            let _ = self.dev.gpu().call_status(&mut a);
        } else {
            let mut a = DestroyTextureArgs::new();
            a.res = self.res;
            let _ = self.dev.gpu().call_status(&mut a);
        }
    }
}

fn memory_buffer_kind(m: Memory) -> pt::BufferKind {
    match m {
        Memory::Gpu => pt::BufferKind::DEFAULT,
        Memory::Upload => pt::BufferKind::UPLOAD,
        Memory::Readback => pt::BufferKind::READBACK,
        Memory::Shared => pt::BufferKind::SHARED,
    }
}

fn memory_heap_kind(m: Memory) -> pt::MemKind {
    match m {
        Memory::Gpu => pt::MemKind::DEFAULT,
        Memory::Upload => pt::MemKind::UPLOAD,
        Memory::Readback => pt::MemKind::READBACK,
        Memory::Shared => pt::MemKind::SHARED,
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
        let slot = self.range.id;
        g.records.retain(|k, _| k.0 != slot);
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
/// `BEGIN_RENDER_PASS` makes `Cmd` the big variant (a few hundred bytes); a
/// list holds few of them next to the draws, which are small, so it is kept
/// inline rather than boxed.
#[allow(clippy::large_enum_variant)]
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
    let bad = |e: victory_metal_proto::Error| {
        if std::env::var_os("VICTORY_METAL_LOG").is_some_and(|v| v != "0") {
            eprintln!("victory-metal front: a command was refused when encoding: {e}");
        }
        Hr::E_INVALIDARG
    };
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

/// `D3D12_RESOURCE_BARRIER`, reduced to what the back needs.
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
    pub(crate) dev: Arc<Device>,
    pub kind: u32,
    pub(crate) items: Vec<Item>,
    pub state: ListState,
    /// First error found while recording; reported by `close`, as D3D12's
    /// debug layer and drivers do.
    error: Option<Hr>,
    /// Render pass derivation (`render.rs`).
    pub(crate) render: crate::render::RenderRec,
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
            render: Default::default(),
        }
    }

    /// A command of the stream. Anything but state setting and barriers is
    /// not a render command, so it ends the render pass first (a render
    /// encoder cannot copy or dispatch).
    fn push(&mut self, c: Command<'static>) {
        if self.state == ListState::Recording {
            if !crate::render::is_state_command(&c) {
                self.leave_render();
            }
            self.items.push(Item::Cmd(c));
        }
    }

    pub(crate) fn fail(&mut self, e: Hr) {
        self.error.get_or_insert(e);
    }

    /// `Close`. A second `Close` returns S_OK, as D3DMetal does.
    pub fn close(&mut self) -> Result<Arc<[Item]>> {
        if self.state == ListState::Recording {
            self.leave_render();
        }
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
        self.render = Default::default();
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

    /// `CopyResource`: buffers of one size, or textures of one shape.
    pub fn copy_resource(&mut self, dst: &Resource, src: &Resource) {
        if dst.desc.is_buffer() && src.desc.is_buffer() {
            if dst.desc.width != src.desc.width {
                return self.fail(Hr::E_INVALIDARG);
            }
            return self.copy_buffer_region(dst, 0, src, 0, src.desc.width);
        }
        let (d, s) = (&dst.desc, &src.desc);
        if d.is_buffer() != s.is_buffer()
            || d.dimension != s.dimension
            || (d.width, d.height, d.depth_or_array, d.mips, d.sample_count)
                != (s.width, s.height, s.depth_or_array, s.mips, s.sample_count)
        {
            return self.fail(Hr::E_INVALIDARG);
        }
        self.push(Command::CopyResource(pt_copy_resource(dst.res, src.res)));
    }

    /// `CopyTextureRegion`. `src_box` is `[left, top, front, right, bottom,
    /// back]` in texels (blocks' texels for compressed formats).
    pub fn copy_texture_region(
        &mut self,
        dst: &CopyLoc,
        at: [u32; 3],
        src: &CopyLoc,
        src_box: Option<[u32; 6]>,
    ) {
        let (Some(d), Some(s)) = (dst.to_proto(), src.to_proto()) else {
            return self.fail(Hr::E_INVALIDARG);
        };
        // A buffer cannot copy to a buffer through here.
        if d.kind == pt::LocKind::FOOTPRINT && s.kind == pt::LocKind::FOOTPRINT {
            return self.fail(Hr::E_INVALIDARG);
        }
        let mut c = <cmd::CopyTextureRegion as Wire>::ZERO;
        c.dst = d;
        c.src = s;
        [c.dst_x, c.dst_y, c.dst_z] = at;
        if let Some(b) = src_box {
            c.flags = cmd::CopyTextureRegion::HAS_BOX;
            [
                c.box_left,
                c.box_top,
                c.box_front,
                c.box_right,
                c.box_bottom,
                c.box_back,
            ] = b;
        }
        self.push(Command::CopyTextureRegion(c));
    }

    /// `ClearUnorderedAccessViewUint/Float` on a texture: `view` is what the
    /// descriptor holds, `rects` are `[left, top, right, bottom]` (none = all).
    pub fn clear_uav_texture(
        &mut self,
        tex: &Resource,
        view: &TextureView,
        values: [u32; 4],
        float_values: bool,
        rects: &[[i32; 4]],
    ) {
        if tex.desc.is_buffer() || view.resource != Some(tex.res) {
            return self.fail(Hr::E_INVALIDARG);
        }
        let mut c = <cmd::ClearUavTexture as Wire>::ZERO;
        c.res = tex.res;
        c.format = view.format;
        c.dimension = view.dim;
        c.float_values = float_values as u8;
        c.mip = view.mip_first;
        c.first_slice = view.slice_first;
        c.slice_count = view.slice_count;
        c.values = values;
        if rects.is_empty() {
            return self.push(Command::ClearUavTexture(c));
        }
        for r in rects {
            if r.iter().any(|v| *v < 0) || r[2] <= r[0] || r[3] <= r[1] {
                return self.fail(Hr::E_INVALIDARG);
            }
            let mut c = c;
            c.flags = cmd::ClearUavTexture::HAS_RECT;
            [c.rect_left, c.rect_top, c.rect_right, c.rect_bottom] =
                [r[0] as u32, r[1] as u32, r[2] as u32, r[3] as u32];
            self.push(Command::ClearUavTexture(c));
        }
    }

    pub fn resource_barrier(&mut self, barriers: &[ResourceBarrier]) {
        for b in barriers {
            if let ResourceBarrier::Transition { res, .. } = *b {
                self.barrier_on_attachment(res);
            }
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
        self.set_root_signature(BindPoint::COMPUTE, rs);
    }

    pub fn set_root_signature(&mut self, bind_point: BindPoint, rs: &RootSignature) {
        self.push(Command::SetRootSignature(SetRootSignature {
            bind_point,
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
        self.set_root_constants(BindPoint::COMPUTE, root_index, dest_offset, values);
    }

    pub fn set_root_constants(
        &mut self,
        bind_point: BindPoint,
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
                bind_point,
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
        self.set_root_descriptor(BindPoint::COMPUTE, root_index, kind, gpu_va);
    }

    pub fn set_root_descriptor(
        &mut self,
        bind_point: BindPoint,
        root_index: u32,
        kind: RootParamKind,
        gpu_va: u64,
    ) {
        self.push(Command::SetRootDescriptor(SetRootDescriptor {
            bind_point,
            root_index,
            kind,
            _pad: 0,
            gpu_va,
        }));
    }

    /// `SetComputeRootDescriptorTable`: `gpu_handle` is a handle of a shader-visible heap.
    pub fn set_compute_root_table(&mut self, root_index: u32, gpu_handle: u64) {
        self.set_root_table(BindPoint::COMPUTE, root_index, gpu_handle);
    }

    pub fn set_root_table(&mut self, bind_point: BindPoint, root_index: u32, gpu_handle: u64) {
        match self.dev.resolve_gpu(gpu_handle) {
            Ok((heap, index)) => self.push(Command::SetRootTable(SetRootTable {
                bind_point,
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

fn pt_copy_resource(dst: ResId, src: ResId) -> cmd::CopyResource {
    cmd::CopyResource { dst, src }
}

/// One side of `CopyTextureRegion` (`D3D12_TEXTURE_COPY_LOCATION`).
pub enum CopyLoc<'a> {
    /// A subresource of a texture, by D3D12 subresource index.
    Subresource { res: &'a Resource, index: u32 },
    /// Bytes of a buffer laid out as a texture region.
    Footprint {
        res: &'a Resource,
        offset: u64,
        format: u32,
        width: u32,
        height: u32,
        depth: u32,
        row_pitch: u32,
    },
}

impl CopyLoc<'_> {
    fn to_proto(&self) -> Option<cmd::TextureLocation> {
        let mut l = <cmd::TextureLocation as Wire>::ZERO;
        match *self {
            CopyLoc::Subresource { res, index } => {
                if res.desc.is_buffer() || index >= res.desc.subresource_count() {
                    return None;
                }
                let (mip, slice, plane) = footprint::decompose(&res.desc, index);
                l.res = res.res;
                l.kind = pt::LocKind::SUBRESOURCE;
                l.plane = plane as u8;
                l.mip = mip as u16;
                l.slice = slice;
            }
            CopyLoc::Footprint {
                res,
                offset,
                format,
                width,
                height,
                depth,
                row_pitch,
            } => {
                if !res.desc.is_buffer() {
                    return None;
                }
                l.res = res.res;
                l.kind = pt::LocKind::FOOTPRINT;
                l.offset = offset;
                l.format = format;
                l.width = width;
                l.height = height;
                l.depth = depth;
                l.row_pitch = row_pitch;
            }
        }
        Some(l)
    }
}

fn stage_barrier(sb: state::StageBarrier, res: ResId) -> Command<'static> {
    let mut b = Barrier::memory(sb.before, sb.after);
    b.visibility = sb.visibility;
    b.res_before = res;
    b.res_after = res;
    Command::Barrier(b)
}
