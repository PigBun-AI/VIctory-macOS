//! A mock `victory_metal_proto::Link`: no GPU, no Metal.
//!
//! It answers the sync calls the way a back would (fresh ids and addresses,
//! device name, capabilities), records every call and every block `submit`
//! receives, checks each block with proto's `Decoder`, and performs the one
//! thing a test can see end to end: `COPY_BUFFER_REGION` between host-memory
//! buffers, and fence signals from `QUEUE_SIGNAL`. Tests assert on the bytes
//! of the recorded blocks (golden bytes) and on the recorded calls.
//!
//! It is also what the dlls link to until the bridge carries a real `Link`
//! (`front::connect()`), so the Wine ABI smoke test works without a back.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use victory_metal_proto::pso::GraphicsPsoDesc;
use victory_metal_proto::sync::*;
use victory_metal_proto::types::{Caps, SamplerDesc, ViewDesc, ViewKind};
use victory_metal_proto::{
    Command, Decoder, FenceId, HeapId, Link, PROTO_VERSION, PsoId, QueueId, ResId, RootSigDesc,
    RootSigId, Status, SurfaceId,
};

const VA_START: u64 = 0x1_0000_0000;
const PAGE: u64 = 16 * 1024;
const MAX_BUFFER: u64 = 1 << 30;
/// Slot sizes of the mock's descriptor heaps (a real back reports its own).
pub const DESCRIPTOR_STRIDE: u32 = 32;
pub const SAMPLER_STRIDE: u32 = 16;
pub const SIMD_WIDTH: u32 = 32;
pub const DEVICE_NAME: &str = "Mock Metal GPU (victory-metal-front)";
pub const WORKING_SET: u64 = 24 << 30;

struct Buffer {
    ptr: *mut u8,
    size: usize,
    gpu_va: u64,
    cpu_visible: bool,
}

// SAFETY: the pointer is a private heap allocation owned by this record.
unsafe impl Send for Buffer {}

impl Drop for Buffer {
    fn drop(&mut self) {
        // SAFETY: allocated in `create_buffer` as a boxed slice of this length.
        unsafe {
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                self.ptr, self.size,
            )))
        };
    }
}

/// A mock texture takes `w * h * d` texels of 4 bytes and a third more for mips,
/// in whole pages.
fn mock_texture_size(w: u32, h: u32, d: u32) -> u64 {
    let bytes = w as u64 * h as u64 * d as u64 * 4 * 4 / 3;
    bytes.div_ceil(PAGE) * PAGE
}

/// What a `WRITE_DESCRIPTOR` stored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Descriptor {
    Empty,
    View(ViewDesc),
    Sampler(SamplerDesc),
}

struct Heap {
    kind: u32,
    slots: Vec<Descriptor>,
}

/// What `CREATE_TEXTURE` was asked for (a mock texture has no contents).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureRecord {
    pub dimension: u32,
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub depth_or_array: u32,
    pub mips: u32,
    pub samples: u32,
    pub usage: u32,
    /// Placement, if any: `(heap, offset)`.
    pub placed: Option<(u32, u64)>,
}

/// A `PRESENT` the mock accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentRecord {
    pub queue: QueueId,
    pub swapchain: SurfaceId,
    pub buffer: ResId,
    pub sync_interval: u32,
    pub flags: u32,
}

struct SwapRecord {
    window: u64,
    width: u32,
    height: u32,
    latency: u32,
}

/// A memory heap (`CREATE_HEAP`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemHeapRecord {
    pub kind: u32,
    pub size: u64,
}

/// One entry of the call log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallRecord {
    pub op: SyncOp,
    pub status: Status,
}

#[derive(Default)]
struct State {
    hello: bool,
    next_id: u32,
    next_va: u64,
    buffers: HashMap<u32, Buffer>,
    by_va: BTreeMap<u64, u32>,
    heaps: HashMap<u32, Heap>,
    textures: HashMap<u32, TextureRecord>,
    mem_heaps: HashMap<u32, MemHeapRecord>,
    root_sigs: HashMap<u32, RootSigDesc>,
    pipelines: HashMap<u32, (u32, usize)>,
    /// The description of every graphics pipeline made (by id).
    graphics: HashMap<u32, GraphicsPsoDesc>,
    fences: HashMap<u32, u64>,
    queues: HashMap<u32, u32>,
    swapchains: HashMap<u32, SwapRecord>,
    presents: Vec<PresentRecord>,
    calls: Vec<CallRecord>,
    blocks: Vec<Vec<u8>>,
    /// Commands that could not be carried out (bad ids, out of range).
    rejected: Vec<String>,
    /// `op -> status`: the next call of `op` fails with it.
    fail_next: HashMap<u32, Status>,
    caps: Caps,
    shutdown_calls: u32,
}

pub struct MockLink {
    st: Mutex<State>,
}

impl Default for MockLink {
    fn default() -> Self {
        Self::new()
    }
}

impl MockLink {
    pub fn new() -> MockLink {
        MockLink::with_caps(Caps::KNOWN)
    }

    pub fn with_caps(caps: Caps) -> MockLink {
        MockLink {
            st: Mutex::new(State {
                next_id: 1,
                next_va: VA_START,
                caps,
                ..Default::default()
            }),
        }
    }

    /// Every block `submit` accepted, in order.
    pub fn blocks(&self) -> Vec<Vec<u8>> {
        self.st.lock().unwrap().blocks.clone()
    }

    pub fn calls(&self) -> Vec<CallRecord> {
        self.st.lock().unwrap().calls.clone()
    }

    pub fn calls_of(&self, op: SyncOp) -> usize {
        self.st
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|c| c.op == op)
            .count()
    }

    pub fn rejected(&self) -> Vec<String> {
        self.st.lock().unwrap().rejected.clone()
    }

    pub fn live_buffers(&self) -> usize {
        self.st.lock().unwrap().buffers.len()
    }

    pub fn live_heaps(&self) -> usize {
        self.st.lock().unwrap().heaps.len()
    }

    pub fn live_textures(&self) -> usize {
        self.st.lock().unwrap().textures.len()
    }

    pub fn live_mem_heaps(&self) -> usize {
        self.st.lock().unwrap().mem_heaps.len()
    }

    pub fn texture(&self, res: ResId) -> Option<TextureRecord> {
        self.st.lock().unwrap().textures.get(&res.0).copied()
    }

    pub fn mem_heap(&self, id: victory_metal_proto::MemHeapId) -> Option<MemHeapRecord> {
        self.st.lock().unwrap().mem_heaps.get(&id.0).copied()
    }

    pub fn live_pipelines(&self) -> usize {
        self.st.lock().unwrap().pipelines.len()
    }

    /// The description a graphics pipeline was made from.
    pub fn graphics_desc(&self, id: PsoId) -> Option<GraphicsPsoDesc> {
        self.st.lock().unwrap().graphics.get(&id.0).copied()
    }

    pub fn live_root_signatures(&self) -> usize {
        self.st.lock().unwrap().root_sigs.len()
    }

    pub fn live_fences(&self) -> usize {
        self.st.lock().unwrap().fences.len()
    }

    pub fn live_queues(&self) -> usize {
        self.st.lock().unwrap().queues.len()
    }

    pub fn live_swapchains(&self) -> usize {
        self.st.lock().unwrap().swapchains.len()
    }

    pub fn swapchain_window(&self, id: SurfaceId) -> Option<u64> {
        self.st
            .lock()
            .unwrap()
            .swapchains
            .get(&id.0)
            .map(|s| s.window)
    }

    pub fn swapchain_size(&self, id: SurfaceId) -> Option<(u32, u32)> {
        self.st
            .lock()
            .unwrap()
            .swapchains
            .get(&id.0)
            .map(|s| (s.width, s.height))
    }

    pub fn swapchain_latency(&self, id: SurfaceId) -> Option<u32> {
        self.st
            .lock()
            .unwrap()
            .swapchains
            .get(&id.0)
            .map(|s| s.latency)
    }

    /// Every `PRESENT` the mock accepted, in order.
    pub fn presents(&self) -> Vec<PresentRecord> {
        self.st.lock().unwrap().presents.clone()
    }

    /// The next call of `op` fails with `status` (once).
    pub fn fail_next(&self, op: SyncOp, status: Status) {
        self.st.lock().unwrap().fail_next.insert(op.0, status);
    }

    pub fn descriptor(&self, heap: HeapId, index: u32) -> Option<Descriptor> {
        self.st
            .lock()
            .unwrap()
            .heaps
            .get(&heap.0)?
            .slots
            .get(index as usize)
            .copied()
    }

    pub fn root_signature(&self, id: RootSigId) -> Option<RootSigDesc> {
        self.st.lock().unwrap().root_sigs.get(&id.0).cloned()
    }

    /// Bytes of a buffer (for assertions).
    pub fn buffer_bytes(&self, res: ResId) -> Option<Vec<u8>> {
        let g = self.st.lock().unwrap();
        let b = g.buffers.get(&res.0)?;
        // SAFETY: the allocation is `size` bytes and alive while locked.
        Some(unsafe { std::slice::from_raw_parts(b.ptr, b.size) }.to_vec())
    }

    pub fn fence_value(&self, fence: FenceId) -> Option<u64> {
        self.st.lock().unwrap().fences.get(&fence.0).copied()
    }
}

impl State {
    fn new_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn hello(&mut self, a: &mut HelloArgs) -> Status {
        if a.client_proto_version != PROTO_VERSION {
            return Status::BAD_VERSION;
        }
        if a.required_caps.0 & !self.caps.0 != 0 {
            return Status::UNSUPPORTED;
        }
        self.hello = true;
        a.proto_version = PROTO_VERSION;
        a.caps = self.caps;
        a.device_name = [0; 96];
        a.device_name[..DEVICE_NAME.len()].copy_from_slice(DEVICE_NAME.as_bytes());
        Status::OK
    }

    fn query_caps(&mut self, a: &mut QueryCapsArgs) -> Status {
        a.caps = self.caps;
        a.max_buffer_len = MAX_BUFFER;
        a.working_set = WORKING_SET;
        a.page_size = PAGE as u32;
        a.descriptor_stride = DESCRIPTOR_STRIDE;
        a.sampler_stride = SAMPLER_STRIDE;
        a.max_threads_per_group = 1024;
        a.simd_width = SIMD_WIDTH;
        a.max_queues = 64;
        Status::OK
    }

    fn create_buffer(&mut self, a: &mut CreateBufferArgs) -> Status {
        use victory_metal_proto::types::BufferKind;
        let kind = BufferKind(a.kind);
        if a.size == 0 || a.flags != 0 || !kind.is_known() || kind == BufferKind::WRAPPED {
            return Status::INVALID_ARG;
        }
        if a.size > MAX_BUFFER {
            return Status::OUT_OF_MEMORY;
        }
        let id = self.new_id();
        let alloc = a.size.div_ceil(PAGE) * PAGE;
        let gpu_va = self.next_va;
        self.next_va += alloc;
        let ptr = Box::into_raw(vec![0u8; a.size as usize].into_boxed_slice()).cast::<u8>();
        let cpu_visible = kind != BufferKind::DEFAULT;
        self.buffers.insert(
            id,
            Buffer {
                ptr,
                size: a.size as usize,
                gpu_va,
                cpu_visible,
            },
        );
        self.by_va.insert(gpu_va, id);
        a.res = ResId(id);
        a.gpu_va = gpu_va;
        a.cpu_addr = if cpu_visible { ptr as u64 } else { 0 };
        a.alloc_size = alloc;
        Status::OK
    }

    fn create_texture(&mut self, a: &mut CreateTextureArgs) -> Status {
        if a.width == 0 || a.height == 0 || a.depth_or_array == 0 || a.mips == 0 || a.samples == 0 {
            return Status::INVALID_ARG;
        }
        let placed = if a.heap.0 == 0 {
            None
        } else if let Some(h) = self.mem_heaps.get(&a.heap.0) {
            if h.kind != 0 || a.heap_offset >= h.size || !a.heap_offset.is_multiple_of(PAGE) {
                return Status::INVALID_ARG;
            }
            Some((a.heap.0, a.heap_offset))
        } else {
            return Status::NOT_FOUND;
        };
        let id = self.new_id();
        self.textures.insert(
            id,
            TextureRecord {
                dimension: a.dimension.0,
                format: a.format,
                width: a.width,
                height: a.height,
                depth_or_array: a.depth_or_array,
                mips: a.mips,
                samples: a.samples,
                usage: a.usage,
                placed,
            },
        );
        a.res = ResId(id);
        a.alloc_size = mock_texture_size(a.width, a.height, a.depth_or_array);
        Status::OK
    }

    fn make_back_buffers(
        &mut self,
        count: u32,
        w: u32,
        h: u32,
        format: u32,
        usage: u32,
    ) -> [u32; 16] {
        let mut out = [0u32; 16];
        for slot in out.iter_mut().take(count as usize) {
            let id = self.new_id();
            self.textures.insert(
                id,
                TextureRecord {
                    dimension: 3,
                    format,
                    width: w,
                    height: h,
                    depth_or_array: 1,
                    mips: 1,
                    samples: 1,
                    usage: usage | 5,
                    placed: None,
                },
            );
            *slot = id;
        }
        out
    }

    fn create_swapchain(&mut self, a: &mut CreateSwapchainArgs) -> Status {
        if a.buffer_count == 0
            || a.buffer_count as usize > MAX_SWAPCHAIN_BUFFERS
            || a.width == 0
            || a.height == 0
        {
            return Status::INVALID_ARG;
        }
        a.buffers = self.make_back_buffers(a.buffer_count, a.width, a.height, a.format, a.usage);
        let id = self.new_id();
        self.swapchains.insert(
            id,
            SwapRecord {
                window: a.window,
                width: a.width,
                height: a.height,
                latency: if a.max_latency == 0 { 3 } else { a.max_latency },
            },
        );
        a.swapchain = SurfaceId(id);
        a.surface_format = 80;
        Status::OK
    }

    fn resize_swapchain(&mut self, a: &mut ResizeSwapchainArgs) -> Status {
        let Some(s) = self.swapchains.get_mut(&a.swapchain.0) else {
            return Status::NOT_FOUND;
        };
        (s.width, s.height) = (a.width, a.height);
        a.buffers = self.make_back_buffers(a.buffer_count, a.width, a.height, a.format, a.usage);
        Status::OK
    }

    fn present(&mut self, a: &mut PresentArgs) -> Status {
        if !self.swapchains.contains_key(&a.swapchain.0)
            || !self.textures.contains_key(&a.buffer.0)
            || !self.queues.contains_key(&a.queue.0)
        {
            return Status::NOT_FOUND;
        }
        self.presents.push(PresentRecord {
            queue: a.queue,
            swapchain: a.swapchain,
            buffer: a.buffer,
            sync_interval: a.sync_interval,
            flags: a.flags,
        });
        Status::OK
    }

    fn create_mem_heap(&mut self, a: &mut CreateHeapArgs) -> Status {
        if a.size == 0 || a.flags != 0 || !a.kind.is_known() {
            return Status::INVALID_ARG;
        }
        let id = self.new_id();
        let size = a.size.div_ceil(PAGE) * PAGE;
        self.mem_heaps.insert(
            id,
            MemHeapRecord {
                kind: a.kind.0,
                size,
            },
        );
        a.heap = victory_metal_proto::MemHeapId(id);
        a.alloc_size = size;
        Status::OK
    }

    fn create_placed_buffer(&mut self, a: &mut CreatePlacedBufferArgs) -> Status {
        let Some(h) = self.mem_heaps.get(&a.heap.0).copied() else {
            return Status::NOT_FOUND;
        };
        if a.size == 0
            || !a.offset.is_multiple_of(PAGE)
            || a.offset.checked_add(a.size).is_none_or(|e| e > h.size)
        {
            return Status::INVALID_ARG;
        }
        let kind = match h.kind {
            1 => victory_metal_proto::types::BufferKind::UPLOAD,
            2 => victory_metal_proto::types::BufferKind::READBACK,
            3 => victory_metal_proto::types::BufferKind::SHARED,
            _ => victory_metal_proto::types::BufferKind::DEFAULT,
        };
        let mut b = CreateBufferArgs::new();
        b.size = a.size;
        b.kind = kind.0;
        let st = self.create_buffer(&mut b);
        a.res = b.res;
        a.gpu_va = b.gpu_va;
        a.cpu_addr = b.cpu_addr;
        st
    }

    fn allocation_info(&mut self, a: &mut GetResourceAllocationInfoArgs) -> Status {
        use victory_metal_proto::types::ResKind;
        a.alignment = PAGE;
        a.size = match a.kind {
            ResKind::BUFFER if a.buffer_size > 0 => a.buffer_size.div_ceil(PAGE) * PAGE,
            ResKind::TEXTURE if a.width > 0 && a.height > 0 && a.depth_or_array > 0 => {
                mock_texture_size(a.width, a.height, a.depth_or_array)
            }
            _ => return Status::INVALID_ARG,
        };
        Status::OK
    }

    fn destroy_buffer(&mut self, a: &mut DestroyBufferArgs) -> Status {
        match self.buffers.remove(&a.res.0) {
            Some(b) => {
                self.by_va.remove(&b.gpu_va);
                Status::OK
            }
            None => Status::NOT_FOUND,
        }
    }

    fn create_desc_heap(&mut self, a: &mut CreateDescHeapArgs) -> Status {
        use victory_metal_proto::types::HeapKind;
        let kind = HeapKind(a.kind);
        if !kind.is_known() || a.capacity == 0 || a.flags > 1 {
            return Status::INVALID_ARG;
        }
        let stride = if kind == HeapKind::SAMPLER {
            SAMPLER_STRIDE
        } else {
            DESCRIPTOR_STRIDE
        };
        let id = self.new_id();
        let size = (a.capacity as u64 * stride as u64).div_ceil(PAGE) * PAGE;
        a.heap = HeapId(id);
        a.stride = stride;
        a.gpu_va = self.next_va;
        self.next_va += size;
        a.cpu_addr = 0;
        self.heaps.insert(
            id,
            Heap {
                kind: a.kind,
                slots: vec![Descriptor::Empty; a.capacity as usize],
            },
        );
        Status::OK
    }

    fn write_descriptor(&mut self, a: &mut WriteDescriptorArgs) -> Status {
        let Some(h) = self.heaps.get_mut(&a.heap.0) else {
            return Status::NOT_FOUND;
        };
        let Some(slot) = h.slots.get_mut(a.index as usize) else {
            return Status::INVALID_ARG;
        };
        if !a.view.is_valid() {
            return Status::INVALID_ARG;
        }
        *slot = if a.view.kind == ViewKind::SAMPLER {
            Descriptor::Sampler(a.sampler)
        } else {
            Descriptor::View(a.view)
        };
        Status::OK
    }

    fn copy_descriptors(&mut self, a: &mut CopyDescriptorsArgs) -> Status {
        let (Some(d), Some(s)) = (self.heaps.get(&a.dst_heap.0), self.heaps.get(&a.src_heap.0))
        else {
            return Status::NOT_FOUND;
        };
        if d.kind != s.kind {
            return Status::INVALID_ARG;
        }
        let (di, si, n) = (a.dst_index as usize, a.src_index as usize, a.count as usize);
        if di.checked_add(n).is_none_or(|e| e > d.slots.len())
            || si.checked_add(n).is_none_or(|e| e > s.slots.len())
        {
            return Status::INVALID_ARG;
        }
        let copied: Vec<Descriptor> = s.slots[si..si + n].to_vec();
        self.heaps.get_mut(&a.dst_heap.0).unwrap().slots[di..di + n].copy_from_slice(&copied);
        Status::OK
    }

    fn create_root_signature(&mut self, a: &mut CreateRootSignatureArgs) -> Status {
        if a.data_addr == 0 || a.data_len == 0 || a.data_len > (1 << 20) {
            return Status::INVALID_ARG;
        }
        // SAFETY: the caller promises `[data_addr, +data_len)` is readable for the call.
        let bytes =
            unsafe { std::slice::from_raw_parts(a.data_addr as *const u8, a.data_len as usize) };
        match RootSigDesc::decode(bytes) {
            Ok(d) => {
                let id = self.new_id();
                self.root_sigs.insert(id, d);
                a.root_sig = RootSigId(id);
                Status::OK
            }
            Err(e) => e.status(),
        }
    }

    fn create_compute_pipeline(&mut self, a: &mut CreateComputePipelineArgs) -> Status {
        if !self.root_sigs.contains_key(&a.root_sig.0) {
            return Status::NOT_FOUND;
        }
        if a.flags != 0 || a.dxil_addr == 0 || a.dxil_len < 4 || a.reserved != [0; 4] {
            return Status::INVALID_ARG;
        }
        // SAFETY: valid for the call, as above.
        let bytes = unsafe { std::slice::from_raw_parts(a.dxil_addr as *const u8, 4) };
        if bytes != b"DXBC" {
            return Status::INVALID_ARG;
        }
        let id = self.new_id();
        self.pipelines
            .insert(id, (a.root_sig.0, a.dxil_len as usize));
        a.pso = PsoId(id);
        Status::OK
    }

    fn create_graphics_pipeline(&mut self, a: &mut CreateGraphicsPipelineArgs) -> Status {
        if !self.root_sigs.contains_key(&a.root_sig.0) {
            return Status::NOT_FOUND;
        }
        if a.flags != 0 || a._pad != 0 || a.desc.check().is_err() {
            return Status::INVALID_ARG;
        }
        let d = a.desc;
        for s in [&d.vs, &d.ps, &d.amp, &d.ms, &d.gs] {
            if s.is_present() {
                // SAFETY: the stage's bytes are valid for the call (the front's contract).
                let magic = unsafe {
                    std::slice::from_raw_parts(s.addr as *const u8, 4.min(s.len as usize))
                };
                if magic != b"DXBC" {
                    return Status::INVALID_ARG;
                }
            }
        }
        let id = self.new_id();
        self.pipelines.insert(id, (a.root_sig.0, 0));
        self.graphics.insert(id, d);
        a.pso = PsoId(id);
        Status::OK
    }

    fn submit(&mut self, block: &[u8]) -> Status {
        let dec = match Decoder::new(block) {
            Ok(d) => d,
            Err(e) => return e.status(),
        };
        let cmds = match dec.collect_all() {
            Ok(c) => c,
            Err(e) => return e.status(),
        };
        for c in &cmds {
            match c {
                Command::CopyBufferRegion(c) => {
                    self.copy(c.dst, c.dst_offset, c.src, c.src_offset, c.size)
                }
                Command::CopyTextureRegion(t) => {
                    for l in [&t.dst, &t.src] {
                        if !self.textures.contains_key(&l.res.0)
                            && !self.buffers.contains_key(&l.res.0)
                        {
                            self.rejected.push(format!("{c:?}"));
                        }
                    }
                }
                Command::CopyResource(r) => {
                    for id in [r.dst, r.src] {
                        if !self.textures.contains_key(&id.0) && !self.buffers.contains_key(&id.0) {
                            self.rejected.push(format!("{c:?}"));
                        }
                    }
                }
                Command::WriteImmediate(w) => {
                    let fits = self.buffers.get(&w.dst.0).is_some_and(|d| {
                        w.offset
                            .checked_add(u64::from(w.size))
                            .is_some_and(|e| e <= d.size as u64)
                    });
                    if fits {
                        let d = &self.buffers[&w.dst.0];
                        // SAFETY: the range was checked against the live allocation above.
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                w.value.to_le_bytes().as_ptr(),
                                d.ptr.add(w.offset as usize),
                                w.size as usize,
                            )
                        };
                    } else {
                        self.rejected.push(format!("{c:?}"));
                    }
                }
                Command::ClearUavTexture(t) if !self.textures.contains_key(&t.res.0) => {
                    self.rejected.push(format!("{c:?}"));
                }
                Command::QueueSignal(s) => match self.fences.get_mut(&s.fence.0) {
                    Some(v) => *v = (*v).max(s.value),
                    None => self.rejected.push(format!("{c:?}")),
                },
                Command::BeginList(b) if !self.queues.contains_key(&b.queue.0) => {
                    self.rejected.push(format!("{c:?}"));
                }
                _ => {}
            }
        }
        self.blocks.push(block.to_vec());
        Status::OK
    }

    fn copy(&mut self, dst: ResId, dst_off: u64, src: ResId, src_off: u64, size: u64) {
        let ok = match (self.buffers.get(&dst.0), self.buffers.get(&src.0)) {
            (Some(d), Some(s)) => {
                let fits =
                    |off: u64, len: usize| off.checked_add(size).is_some_and(|e| e <= len as u64);
                if fits(dst_off, d.size) && fits(src_off, s.size) {
                    // SAFETY: both ranges are inside live allocations (checked above); `copy`
                    // allows dst and src to be the same buffer.
                    unsafe {
                        std::ptr::copy(
                            s.ptr.add(src_off as usize),
                            d.ptr.add(dst_off as usize),
                            size as usize,
                        )
                    };
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        if !ok {
            self.rejected.push(format!(
                "copy {dst:?}+{dst_off} <- {src:?}+{src_off} x{size}"
            ));
        }
    }
}

impl Link for MockLink {
    fn submit(&self, block: &[u8]) -> Status {
        let mut g = self.st.lock().unwrap();
        if !g.hello {
            return Status::NOT_INITIALIZED;
        }
        g.submit(block)
    }

    // The trait method is safe by proto's contract (see `Link::call`).
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    fn call(&self, op: SyncOp, args: *mut u8, len: usize) -> Status {
        let mut g = self.st.lock().unwrap();
        let s = &mut *g;
        macro_rules! args {
            ($t:ty) => {
                // SAFETY: `Link::call`'s contract: `args` is valid for `len` bytes.
                match unsafe { args_from_raw::<$t>(args, len) } {
                    Ok(a) => a,
                    Err(e) => {
                        s.calls.push(CallRecord { op, status: e });
                        return e;
                    }
                }
            };
        }
        if let Some(st) = s.fail_next.remove(&op.0) {
            s.calls.push(CallRecord { op, status: st });
            return st;
        }
        if !s.hello && !matches!(op, SyncOp::NOP | SyncOp::HELLO) {
            s.calls.push(CallRecord {
                op,
                status: Status::NOT_INITIALIZED,
            });
            return Status::NOT_INITIALIZED;
        }
        let status = match op {
            SyncOp::NOP => {
                let _ = args!(NopArgs);
                Status::OK
            }
            SyncOp::HELLO => s.hello(args!(HelloArgs)),
            SyncOp::QUERY_CAPS => s.query_caps(args!(QueryCapsArgs)),
            SyncOp::SHUTDOWN => {
                let _ = args!(ShutdownArgs);
                s.shutdown_calls += 1;
                s.hello = false;
                Status::OK
            }
            SyncOp::CREATE_QUEUE => {
                let a = args!(CreateQueueArgs);
                let id = s.new_id();
                s.queues.insert(id, a.kind);
                a.queue = QueueId(id);
                Status::OK
            }
            SyncOp::DESTROY_QUEUE => {
                let a = args!(DestroyQueueArgs);
                if s.queues.remove(&a.queue.0).is_some() {
                    Status::OK
                } else {
                    Status::NOT_FOUND
                }
            }
            SyncOp::CREATE_BUFFER => s.create_buffer(args!(CreateBufferArgs)),
            SyncOp::DESTROY_BUFFER => s.destroy_buffer(args!(DestroyBufferArgs)),
            SyncOp::CREATE_TEXTURE => s.create_texture(args!(CreateTextureArgs)),
            SyncOp::DESTROY_TEXTURE => {
                let a = args!(DestroyTextureArgs);
                if s.textures.remove(&a.res.0).is_some() {
                    Status::OK
                } else {
                    Status::NOT_FOUND
                }
            }
            SyncOp::CREATE_HEAP => s.create_mem_heap(args!(CreateHeapArgs)),
            SyncOp::DESTROY_HEAP => {
                let a = args!(DestroyHeapArgs);
                if s.mem_heaps.remove(&a.heap.0).is_some() {
                    Status::OK
                } else {
                    Status::NOT_FOUND
                }
            }
            SyncOp::CREATE_PLACED_BUFFER => s.create_placed_buffer(args!(CreatePlacedBufferArgs)),
            SyncOp::GET_RESOURCE_ALLOCATION_INFO => {
                s.allocation_info(args!(GetResourceAllocationInfoArgs))
            }
            SyncOp::CREATE_DESC_HEAP => s.create_desc_heap(args!(CreateDescHeapArgs)),
            SyncOp::DESTROY_DESC_HEAP => {
                let a = args!(DestroyDescHeapArgs);
                if s.heaps.remove(&a.heap.0).is_some() {
                    Status::OK
                } else {
                    Status::NOT_FOUND
                }
            }
            SyncOp::WRITE_DESCRIPTOR => s.write_descriptor(args!(WriteDescriptorArgs)),
            SyncOp::COPY_DESCRIPTORS => s.copy_descriptors(args!(CopyDescriptorsArgs)),
            SyncOp::CREATE_ROOT_SIGNATURE => {
                s.create_root_signature(args!(CreateRootSignatureArgs))
            }
            SyncOp::DESTROY_ROOT_SIGNATURE => {
                let a = args!(DestroyRootSignatureArgs);
                if s.root_sigs.remove(&a.root_sig.0).is_some() {
                    Status::OK
                } else {
                    Status::NOT_FOUND
                }
            }
            SyncOp::CREATE_COMPUTE_PIPELINE => {
                s.create_compute_pipeline(args!(CreateComputePipelineArgs))
            }
            SyncOp::CREATE_GRAPHICS_PIPELINE => {
                s.create_graphics_pipeline(args!(CreateGraphicsPipelineArgs))
            }
            SyncOp::DESTROY_PIPELINE => {
                let a = args!(DestroyPipelineArgs);
                s.graphics.remove(&a.pso.0);
                if s.pipelines.remove(&a.pso.0).is_some() {
                    Status::OK
                } else {
                    Status::NOT_FOUND
                }
            }
            SyncOp::CREATE_FENCE => {
                let a = args!(CreateFenceArgs);
                let id = s.new_id();
                s.fences.insert(id, a.initial);
                a.fence = FenceId(id);
                Status::OK
            }
            SyncOp::DESTROY_FENCE => {
                let a = args!(DestroyFenceArgs);
                if s.fences.remove(&a.fence.0).is_some() {
                    Status::OK
                } else {
                    Status::NOT_FOUND
                }
            }
            SyncOp::FENCE_VALUE => {
                let a = args!(FenceValueArgs);
                match s.fences.get(&a.fence.0) {
                    Some(v) => {
                        a.value = *v;
                        Status::OK
                    }
                    None => Status::NOT_FOUND,
                }
            }
            SyncOp::FENCE_WAIT => {
                let a = args!(FenceWaitArgs);
                match s.fences.get(&a.fence.0) {
                    // Everything submitted has already run.
                    Some(v) if *v >= a.value => Status::OK,
                    Some(_) => Status::TIMEOUT,
                    None => Status::NOT_FOUND,
                }
            }
            SyncOp::FENCE_SIGNAL => {
                let a = args!(FenceSignalArgs);
                match s.fences.get_mut(&a.fence.0) {
                    Some(v) => {
                        *v = (*v).max(a.value);
                        Status::OK
                    }
                    None => Status::NOT_FOUND,
                }
            }
            SyncOp::DEVICE_STATUS => {
                let a = args!(DeviceStatusArgs);
                a.state = 0;
                Status::OK
            }
            SyncOp::CREATE_SWAPCHAIN => s.create_swapchain(args!(CreateSwapchainArgs)),
            SyncOp::RESIZE_SWAPCHAIN => s.resize_swapchain(args!(ResizeSwapchainArgs)),
            SyncOp::DESTROY_SWAPCHAIN => {
                let a = args!(DestroySwapchainArgs);
                if s.swapchains.remove(&a.swapchain.0).is_some() {
                    Status::OK
                } else {
                    Status::NOT_FOUND
                }
            }
            SyncOp::PRESENT => s.present(args!(PresentArgs)),
            SyncOp::SET_SWAPCHAIN_LATENCY => {
                let a = args!(SetSwapchainLatencyArgs);
                match s.swapchains.get_mut(&a.swapchain.0) {
                    Some(sc) => {
                        sc.latency = a.max_latency;
                        Status::OK
                    }
                    None => Status::NOT_FOUND,
                }
            }
            SyncOp::WRAP_MEMORY | SyncOp::GET_SURFACE_LAYER => Status::UNSUPPORTED,
            _ => Status::UNKNOWN_OP,
        };
        s.calls.push(CallRecord { op, status });
        status
    }
}

impl MockLink {
    pub fn gpu_va(&self, res: ResId) -> Option<u64> {
        self.st
            .lock()
            .unwrap()
            .buffers
            .get(&res.0)
            .map(|b| b.gpu_va)
    }
}

// Buffers' CPU visibility is part of the mock's contract with tests.
impl MockLink {
    pub fn is_cpu_visible(&self, res: ResId) -> Option<bool> {
        self.st
            .lock()
            .unwrap()
            .buffers
            .get(&res.0)
            .map(|b| b.cpu_visible)
    }
}
