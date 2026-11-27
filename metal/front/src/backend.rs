//! The seam between the front and whatever executes its work.
//!
//! **Placeholder.** Until `victory-metal-proto` exists, the front records
//! into a plain command list (POD, ids, `gpu_va`) and hands it to a
//! [`Backend`]. This is deliberately *not* a protocol: no encoding, no
//! versioning, nothing that crosses the Wine/unixlib boundary. When proto
//! lands, `Command` is replaced by (or mapped one to one onto) proto's
//! opcodes and `Backend` by `Link`; the front code that calls
//! [`Recorder`] does not change. Nothing here names a Metal or a D3D12
//! type: D3D12 states have already become stage masks.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::adapter::AdapterInfo;
use crate::caps::Caps;
use crate::hresult::Result;
use crate::state::StageBarrier;

pub type ResourceId = u32;
pub type PipelineId = u32;
pub type RootSigId = u32;
pub type QueueId = u32;
pub type HeapId = u32;

/// Where a buffer lives: the front's three D3D12 heap kinds, all that M1 needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeapKind {
    Default,
    Upload,
    Readback,
}

/// What the back returns for a new buffer: its id, the GPU virtual address
/// applications will see, and, for CPU-visible heaps, the CPU address of the
/// contents (0 for `Default`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferAlloc {
    pub id: ResourceId,
    pub gpu_va: u64,
    pub cpu_ptr: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewKind {
    Cbv,
    Srv,
    Uav,
}

/// One recorded call. `Unsupported` keeps the place of a call outside the
/// current milestone so that a list stays well formed.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    CopyBuffer { dst: ResourceId, dst_offset: u64, src: ResourceId, src_offset: u64, size: u64 },
    /// `resource` is `None` for global (aliasing) barriers.
    Barrier { resource: Option<ResourceId>, barrier: StageBarrier },
    SetPipeline { pipeline: PipelineId },
    SetComputeRootSignature { root_signature: RootSigId },
    SetDescriptorHeaps { heaps: Vec<HeapId> },
    SetComputeRootConstants { index: u32, dest_offset: u32, values: Vec<u32> },
    SetComputeRootTable { index: u32, heap: HeapId, first: u32 },
    SetComputeRootView { index: u32, kind: ViewKind, gpu_va: u64 },
    Dispatch { x: u32, y: u32, z: u32 },
    Unsupported { call: &'static str },
}

/// One command list's worth of work: one list becomes one stream.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandStream {
    pub commands: Vec<Command>,
}

/// Accumulates the commands of the list being recorded.
#[derive(Default)]
pub struct Recorder {
    commands: Vec<Command>,
}

impl Recorder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&mut self, c: Command) {
        self.commands.push(c);
    }
    pub fn len(&self) -> usize {
        self.commands.len()
    }
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
    pub fn clear(&mut self) {
        self.commands.clear();
    }
    /// The stream of what was recorded so far (the recorder is kept: a
    /// closed D3D12 list can be executed again).
    pub fn stream(&self) -> CommandStream {
        CommandStream { commands: self.commands.clone() }
    }
}

/// A queue's unit of submission: `ExecuteCommandLists` is one `Execute`
/// (one `Link::submit`), `Signal` / `Wait` are the queue's fence operations.
pub enum Submission {
    Execute(Vec<CommandStream>),
    Signal { fence: Arc<Fence>, value: u64 },
    Wait { fence: Arc<Fence>, value: u64 },
}

/// The back, as seen from the front.
pub trait Backend: Send + Sync {
    fn adapter(&self) -> AdapterInfo;
    fn caps(&self) -> Caps {
        Caps::apple()
    }
    fn create_buffer(&self, size: u64, heap: HeapKind) -> Result<BufferAlloc>;
    fn destroy_buffer(&self, id: ResourceId);
    /// `root_signature` is the serialized blob, `shader` the compute shader
    /// bytecode exactly as the application gave it.
    fn create_compute_pipeline(&self, root_signature: &[u8], shader: &[u8]) -> Result<PipelineId>;
    fn destroy_pipeline(&self, id: PipelineId);
    fn submit(&self, queue: QueueId, work: Submission) -> Result<()>;
}

type Callback = Box<dyn FnOnce() + Send>;

/// A fence's completed value, shared by the front (reads, `SetEventOnCompletion`)
/// and the back (`signal` when the GPU gets there; `MTLSharedEvent` later).
pub struct Fence {
    inner: Mutex<FenceInner>,
    cv: Condvar,
}

struct FenceInner {
    completed: u64,
    waiters: Vec<(u64, Callback)>,
}

impl Fence {
    pub fn new(initial: u64) -> Arc<Fence> {
        Arc::new(Fence { inner: Mutex::new(FenceInner { completed: initial, waiters: Vec::new() }), cv: Condvar::new() })
    }

    pub fn completed(&self) -> u64 {
        self.inner.lock().unwrap().completed
    }

    /// Raise the completed value (never lowers it, as `ID3D12Fence::Signal`
    /// can: that is the CPU path, [`Fence::set`]) and run what was waiting.
    pub fn signal(&self, value: u64) {
        let ready = {
            let mut g = self.inner.lock().unwrap();
            if value > g.completed {
                g.completed = value;
            }
            let done = g.completed;
            let (ready, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut g.waiters).into_iter().partition(|(v, _)| *v <= done);
            g.waiters = rest;
            ready
        };
        self.cv.notify_all();
        for (_, cb) in ready {
            cb();
        }
    }

    /// `ID3D12Fence::Signal`: the CPU sets the value (it may go down).
    pub fn set(&self, value: u64) {
        let mut g = self.inner.lock().unwrap();
        g.completed = value;
        drop(g);
        self.signal(value);
    }

    /// Run `cb` once the fence reaches `value`: now, if it already has.
    pub fn on_completion(&self, value: u64, cb: Callback) {
        let mut g = self.inner.lock().unwrap();
        if g.completed >= value {
            drop(g);
            cb();
        } else {
            g.waiters.push((value, cb));
        }
    }

    /// Wait up to `timeout` for `value`; whether it was reached.
    pub fn wait(&self, value: u64, timeout: Duration) -> bool {
        let g = self.inner.lock().unwrap();
        let (g, _) = self.cv.wait_timeout_while(g, timeout, |i| i.completed < value).unwrap();
        g.completed >= value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn fence_runs_waiters_in_order_of_value() {
        let f = Fence::new(0);
        let hits = Arc::new(AtomicU32::new(0));
        for v in [3u64, 1, 2] {
            let h = hits.clone();
            f.on_completion(v, Box::new(move || {
                h.fetch_add(1, Ordering::SeqCst);
            }));
        }
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        f.signal(2);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert_eq!(f.completed(), 2);
        f.signal(1); // never goes down
        assert_eq!(f.completed(), 2);
        f.signal(3);
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        // Already complete: runs at once.
        let h = hits.clone();
        f.on_completion(1, Box::new(move || {
            h.fetch_add(1, Ordering::SeqCst);
        }));
        assert_eq!(hits.load(Ordering::SeqCst), 4);
        assert!(f.wait(3, Duration::from_millis(1)));
        assert!(!f.wait(4, Duration::from_millis(1)));
    }

    #[test]
    fn cpu_set_can_lower_the_value() {
        let f = Fence::new(5);
        f.set(2);
        assert_eq!(f.completed(), 2);
    }
}
