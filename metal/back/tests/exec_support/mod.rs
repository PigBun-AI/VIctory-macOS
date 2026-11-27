//! Helpers shared by the interpreter tests (`tests/exec*.rs`).
#![cfg(target_vendor = "apple")]
#![allow(dead_code)]

use std::sync::{Mutex, MutexGuard};

use victory_metal_back::exec::{ExecOptions, Executor, InProcess};
use victory_metal_back::queue::QueueOptions;
use victory_metal_proto::cmd::*;
use victory_metal_proto::ids::*;
use victory_metal_proto::link::{Link, LinkExt};
use victory_metal_proto::root::*;
use victory_metal_proto::status::Status;
use victory_metal_proto::stream::Encoder;
use victory_metal_proto::sync::*;
use victory_metal_proto::types::*;
use victory_metal_proto::{Caps, PROTO_VERSION};

pub use victory_metal_proto::sync::{
    CopyDescriptorsArgs, CreateBufferArgs, CreateDescHeapArgs, WriteDescriptorArgs,
};

/// The tests share the GPU and some of them are slow: one at a time.
static LOCK: Mutex<()> = Mutex::new(());

/// A started executor behind an `InProcess` link.
pub struct Fx {
    pub link: InProcess,
    _guard: MutexGuard<'static, ()>,
}

pub fn small_options() -> ExecOptions {
    ExecOptions {
        queue: QueueOptions {
            max_lists: 16,
            ring_segment_bytes: 1 << 16,
            ..QueueOptions::default()
        },
        ..ExecOptions::default()
    }
}

impl Fx {
    /// `HELLO` done, small queues.
    pub fn new() -> Fx {
        Fx::with_options(small_options())
    }

    pub fn with_options(options: ExecOptions) -> Fx {
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let link = InProcess::with_options(options);
        let fx = Fx {
            link,
            _guard: guard,
        };
        let mut h = HelloArgs::new();
        h.client_proto_version = PROTO_VERSION;
        h.required_caps = Caps::METAL4;
        assert_eq!(fx.link.call_args(&mut h), Status::OK);
        fx
    }

    /// A fixture whose `HELLO` has not been done.
    pub fn unstarted() -> Fx {
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        Fx {
            link: InProcess::with_options(small_options()),
            _guard: guard,
        }
    }

    pub fn executor(&self) -> &Executor {
        self.link.executor()
    }

    pub fn call<A: SyncArgs>(&self, mut a: A) -> (Status, A) {
        let s = self.link.call_args(&mut a);
        (s, a)
    }

    pub fn ok<A: SyncArgs>(&self, a: A) -> A {
        let (s, a) = self.call(a);
        assert_eq!(s, Status::OK, "{}", std::any::type_name::<A>());
        a
    }

    pub fn status<A: SyncArgs>(&self, a: A) -> Status {
        self.call(a).0
    }

    pub fn queue(&self) -> QueueId {
        let mut a = CreateQueueArgs::new();
        a.kind = QueueKind::DIRECT.0;
        self.ok(a).queue
    }

    pub fn fence(&self, initial: u64) -> FenceId {
        let mut a = CreateFenceArgs::new();
        a.initial = initial;
        self.ok(a).fence
    }

    pub fn fence_value(&self, fence: FenceId) -> u64 {
        let mut a = FenceValueArgs::new();
        a.fence = fence;
        self.ok(a).value
    }

    pub fn signal_fence(&self, fence: FenceId, value: u64) {
        let mut a = FenceSignalArgs::new();
        a.fence = fence;
        a.value = value;
        self.ok(a);
    }

    /// `FENCE_WAIT` with a 20 s limit.
    pub fn wait(&self, fence: FenceId, value: u64) -> Status {
        self.wait_for(fence, value, 20_000)
    }

    pub fn wait_for(&self, fence: FenceId, value: u64, timeout_ms: u64) -> Status {
        let mut a = FenceWaitArgs::new();
        a.fence = fence;
        a.value = value;
        a.timeout_ms = timeout_ms;
        self.status(a)
    }

    pub fn buffer(&self, kind: BufferKind, size: u64) -> CreateBufferArgs {
        let mut a = CreateBufferArgs::new();
        a.size = size;
        a.kind = kind.0;
        self.ok(a)
    }

    pub fn heap(&self, kind: HeapKind, capacity: u32, visible: bool) -> CreateDescHeapArgs {
        let mut a = CreateDescHeapArgs::new();
        a.kind = kind.0;
        a.capacity = capacity;
        a.flags = u32::from(visible);
        self.ok(a)
    }

    pub fn write_view_status(&self, heap: HeapId, index: u32, view: ViewDesc) -> Status {
        let mut a = WriteDescriptorArgs::new();
        a.heap = heap;
        a.index = index;
        a.view = view;
        self.status(a)
    }

    pub fn write_view(&self, heap: HeapId, index: u32, view: ViewDesc) {
        assert_eq!(self.write_view_status(heap, index, view), Status::OK);
    }

    pub fn copy_descriptors(
        &self,
        dst: HeapId,
        dst_index: u32,
        src: HeapId,
        src_index: u32,
        count: u32,
    ) {
        assert_eq!(
            self.copy_descriptors_status(dst, dst_index, src, src_index, count),
            Status::OK
        );
    }

    pub fn copy_descriptors_status(
        &self,
        dst: HeapId,
        dst_index: u32,
        src: HeapId,
        src_index: u32,
        count: u32,
    ) -> Status {
        let mut a = CopyDescriptorsArgs::new();
        a.dst_heap = dst;
        a.dst_index = dst_index;
        a.src_heap = src;
        a.src_index = src_index;
        a.count = count;
        self.status(a)
    }

    pub fn root_sig_status(&self, bytes: &[u8]) -> (Status, RootSigId) {
        let mut a = CreateRootSignatureArgs::new();
        a.data_addr = bytes.as_ptr() as u64;
        a.data_len = bytes.len() as u64;
        let (s, a) = self.call(a);
        (s, a.root_sig)
    }

    pub fn root_sig(&self, desc: &RootSigDesc) -> RootSigId {
        let bytes = desc.encode().unwrap();
        let (s, id) = self.root_sig_status(&bytes);
        assert_eq!(s, Status::OK);
        id
    }

    pub fn submit(&self, cmds: &[Command<'_>]) -> Status {
        self.link.submit(&block(cmds))
    }
}

pub fn block(cmds: &[Command<'_>]) -> Vec<u8> {
    let mut e = Encoder::new(Caps::NONE);
    for c in cmds {
        e.push(c).unwrap();
    }
    e.finish()
}

pub fn begin(list: u32, queue: QueueId) -> Command<'static> {
    Command::BeginList(BeginList {
        list: ListId(list),
        queue,
        flags: 0,
        _pad: 0,
    })
}

pub fn end(list: u32) -> Command<'static> {
    Command::EndList(EndList {
        list: ListId(list),
        _pad: 0,
    })
}

pub fn signal_cmd(queue: QueueId, fence: FenceId, value: u64) -> Command<'static> {
    Command::QueueSignal(QueueSignal {
        queue,
        fence,
        value,
    })
}

pub fn wait_cmd(queue: QueueId, fence: FenceId, value: u64) -> Command<'static> {
    Command::QueueWait(QueueWait {
        queue,
        fence,
        value,
    })
}

pub fn dispatch(x: u32, y: u32, z: u32) -> Command<'static> {
    Command::Dispatch(Dispatch { x, y, z, _pad: 0 })
}

pub fn copy(dst: ResId, src: ResId, size: u64) -> Command<'static> {
    Command::CopyBufferRegion(CopyBufferRegion {
        dst,
        src,
        dst_offset: 0,
        src_offset: 0,
        size,
    })
}

pub fn fill(dst: ResId, value: u32, offset: u64, size: u64) -> Command<'static> {
    Command::FillBuffer(FillBuffer {
        dst,
        value,
        offset,
        size,
    })
}

pub fn root_va(root_index: u32, kind: RootParamKind, gpu_va: u64) -> Command<'static> {
    Command::SetRootDescriptor(SetRootDescriptor {
        bind_point: BindPoint::COMPUTE,
        root_index,
        kind,
        _pad: 0,
        gpu_va,
    })
}

pub fn set_sig(sig: RootSigId) -> Command<'static> {
    Command::SetRootSignature(SetRootSignature {
        bind_point: BindPoint::COMPUTE,
        root_sig: sig,
    })
}

pub fn constants_param(n: u32) -> RootParam {
    RootParam {
        kind: RootParamKind::CONSTANTS,
        num_32bit_values: n,
        ..RootParam::default()
    }
}

pub fn descriptor_param(kind: RootParamKind) -> RootParam {
    RootParam {
        kind,
        ..RootParam::default()
    }
}

pub fn table_param(range_start: u32, range_count: u32) -> RootParam {
    RootParam {
        kind: RootParamKind::TABLE,
        range_start,
        range_count,
        ..RootParam::default()
    }
}

pub fn range(kind: RangeKind) -> DescriptorRange {
    DescriptorRange {
        kind,
        num_descriptors: UNBOUNDED,
        offset_in_table: APPEND,
        ..DescriptorRange::default()
    }
}

/// Writes words to CPU-visible buffer memory (a `cpu_addr` the back returned).
pub fn cpu_write(addr: u64, words: &[u32]) {
    assert_ne!(addr, 0);
    // SAFETY: `addr` is the CPU address of a live buffer of at least this size
    // (every caller sizes its buffers for it); the GPU is not using it.
    unsafe { std::ptr::copy_nonoverlapping(words.as_ptr(), addr as *mut u32, words.len()) };
}

pub fn cpu_read(addr: u64, count: usize) -> Vec<u32> {
    assert_ne!(addr, 0);
    let mut v = vec![0u32; count];
    // SAFETY: as in `cpu_write`.
    unsafe { std::ptr::copy_nonoverlapping(addr as *const u32, v.as_mut_ptr(), count) };
    v
}

pub fn cpu_read_u64s(addr: u64, count: usize) -> Vec<u64> {
    assert_ne!(addr, 0);
    let mut v = vec![0u64; count];
    // SAFETY: as in `cpu_write`.
    unsafe { std::ptr::copy_nonoverlapping(addr as *const u64, v.as_mut_ptr(), count) };
    v
}

/// A block header for `count` commands over `body` bytes, honest otherwise.
pub fn block_with(count: u32, body: &[u8]) -> Vec<u8> {
    use victory_metal_proto::stream::{STREAM_HEADER_SIZE, STREAM_MAGIC};
    let mut v = Vec::new();
    v.extend_from_slice(&STREAM_MAGIC);
    v.extend_from_slice(&PROTO_VERSION.to_le_bytes());
    v.extend_from_slice(&(STREAM_HEADER_SIZE as u16).to_le_bytes());
    v.extend_from_slice(&0u16.to_le_bytes());
    v.extend_from_slice(&0u64.to_le_bytes());
    v.extend_from_slice(&((STREAM_HEADER_SIZE + body.len()) as u32).to_le_bytes());
    v.extend_from_slice(&count.to_le_bytes());
    v.extend_from_slice(body);
    v
}

/// One raw command: header fields given explicitly, payload as is.
pub fn raw_cmd(opcode: u16, version: u8, flags: u8, len: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&opcode.to_le_bytes());
    v.push(version);
    v.push(flags);
    v.extend_from_slice(&len.to_le_bytes());
    v.extend_from_slice(payload);
    v
}
