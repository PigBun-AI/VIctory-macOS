//! The proto interpreter: the one place in this crate that knows
//! `victory-metal-proto`. It turns synchronous calls and command-stream
//! blocks into calls on the executor core ([`crate::device`],
//! [`crate::queue`], [`crate::record`], ...), which know nothing of proto.
//!
//! * [`Executor`] has the shape of `victory_metal_proto::Link`:
//!   [`Executor::call`] (one sync call) and [`Executor::submit`] (one block =
//!   one batch of lists). It owns the [`Device`] (made by `HELLO`) and the
//!   handle tables: `ResId`, `HeapId`, `RootSigId`, `PsoId`, `FenceId` and
//!   `QueueId` map to core objects, and the back allocates the handles.
//! * [`InProcess`] is a `Link` that calls an `Executor` directly: for native
//!   tests, for the replayer and for a Route B front in the same process. The
//!   Wine bridge's Mach-O half calls the same two entry points.
//! * [`tlab`] lays root signatures out as top-level argument buffers.
//!
//! # Rules the interpreter keeps
//!
//! * **Never a panic, always a status.** Bad handles are `NOT_FOUND`, values
//!   out of range `INVALID_ARG`, malformed blocks `BAD_STREAM`, unknown
//!   sync ops `UNKNOWN_OP`, things proto defines and this back does not do
//!   yet `UNSUPPORTED`.
//! * **A block is checked whole before anything runs**: a truncated or
//!   dishonest block has no effect. A *semantic* error in the middle (a dead
//!   handle, say) abandons the block: lists not yet handed to a queue are
//!   dropped, those already submitted stay; the status of the error is
//!   returned.
//! * **Every `submit` ends with `Device::maintain`** (commit residency
//!   additions, free what retired work no longer needs).
//! * Handles are never reused (per-type counters from 1, see `handles.rs`).
//! * Addresses in sync calls (`CREATE_ROOT_SIGNATURE` bytes, `WRAP_MEMORY`)
//!   are used as they are: front and back share one address space, which is
//!   the `Link` contract. `WRAP_MEMORY` still checks that the range is mapped.
//!
//! `CREATE_COMPUTE_PIPELINE` converts the DXIL with Metal Shader Converter
//! ([`crate::msc`], loaded on first use) against the root signature, takes the
//! thread-group size from its reflection and checks that the signature's
//! argument-buffer layout is the one the shader reads. `CREATE_ROOT_SIGNATURE`
//! already asks MSC for that layout (including the static-sampler table
//! pointer MSC appends); without MSC it falls back to the rule-based layout of
//! [`tlab`], and `CREATE_COMPUTE_PIPELINE` answers `UNSUPPORTED`. Tests can
//! also build pipelines from MSL with
//! [`Executor::create_compute_pipeline_from_msl`], which does not touch the
//! wire protocol. [`Executor::last_error`] keeps the text of the last refused
//! conversion (and `VICTORY_METAL_LOG=1` prints it to stderr).

mod dxil;
mod handles;
mod stream;
mod sync;
pub mod tlab;

use std::sync::{Arc, Mutex, MutexGuard};

use victory_metal_proto::ids::{PsoId, RootSigId};
use victory_metal_proto::link::Link;
use victory_metal_proto::status::Status;
use victory_metal_proto::sync::{SyncArgs, SyncOp, args_from_raw};

use crate::device::Device;
use crate::error::Error;
use crate::pipeline::ComputePipeline;
use crate::queue::{DeviceStatus, QueueOptions};

use handles::Table;
use tlab::TlabLayout;

/// Knobs of an [`Executor`].
#[derive(Clone, Copy, Debug)]
pub struct ExecOptions {
    /// Options of every queue `CREATE_QUEUE` makes. The default has the
    /// inter-submit barrier on and room for a frame's worth of root
    /// arguments (4 segments of 4 MiB) and for many lists in one block.
    pub queue: QueueOptions,
    /// What MSC adds to converted shaders to behave like the original API
    /// (the default checks buffer bounds: an out-of-range read gives 0).
    pub compat: crate::msc::Compat,
}

impl Default for ExecOptions {
    fn default() -> Self {
        ExecOptions {
            queue: QueueOptions {
                max_lists: 256,
                ring_segment_bytes: 4 << 20,
                ..QueueOptions::default()
            },
            compat: crate::msc::Compat::BOUNDS_CHECK,
        }
    }
}

/// A root signature: the description proto carried and the TLAB layout
/// made from it (or from reflection).
pub(crate) struct RootSig {
    pub(crate) desc: victory_metal_proto::root::RootSigDesc,
    /// The same description for the converter.
    pub(crate) msc_desc: crate::msc::RootSignatureDesc,
    pub(crate) layout: TlabLayout,
    /// The table of static samplers MSC makes the shader read through a
    /// pointer at the end of the argument buffer (24 bytes per sampler, in
    /// the heap residency set). Its entries stay zero until the back has
    /// samplers (textures, M2): no compute shader without textures can use them.
    pub(crate) static_table: Option<crate::descriptors::DescriptorHeap>,
}

/// A compute pipeline with what a dispatch needs besides the Metal object.
pub(crate) struct Pso {
    pub(crate) pipeline: ComputePipeline,
    pub(crate) root_sig: Arc<RootSig>,
    /// Threads per threadgroup: from the shader (`numthreads`, which MSC
    /// reflection reports) for converted pipelines, given for MSL tests.
    pub(crate) threads: [u32; 3],
}

/// A descriptor heap and what proto said about it.
pub(crate) struct Heap {
    pub(crate) heap: crate::descriptors::DescriptorHeap,
    pub(crate) sampler: bool,
    pub(crate) shader_visible: bool,
}

/// Everything `HELLO` made.
pub(crate) struct State {
    pub(crate) device: Device,
    pub(crate) options: ExecOptions,
    pub(crate) resources: Table<crate::memory::Buffer>,
    pub(crate) heaps: Table<Heap>,
    pub(crate) root_sigs: Table<Arc<RootSig>>,
    pub(crate) psos: Table<Arc<Pso>>,
    pub(crate) fences: Table<crate::queue::Fence>,
    pub(crate) queues: Table<crate::queue::Queue>,
}

/// `Error` to the status a `Link` call answers with.
pub(crate) fn status_of(e: &Error) -> Status {
    match e {
        Error::NoDevice | Error::Unsupported(_) => Status::UNSUPPORTED,
        Error::Metal(_) => Status::INTERNAL,
        Error::PageConflict { .. } => Status::CONFLICT,
        Error::BadArgument(_) | Error::NotResident(_) => Status::INVALID_ARG,
        Error::Timeout => Status::TIMEOUT,
        Error::DeviceLost(DeviceStatus::Hung) => Status::DEVICE_HUNG,
        Error::DeviceLost(_) => Status::DEVICE_LOST,
    }
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Reads an argument block from a raw pointer, runs `f`, answers.
pub(crate) fn typed<A: SyncArgs>(
    args: *mut u8,
    len: usize,
    f: impl FnOnce(&mut A) -> Result<(), Status>,
) -> Status {
    // SAFETY: the `Link::call` contract: `args` is null or points at `len`
    // bytes the caller owns for the duration of the call; `args_from_raw`
    // checks null, alignment, length and version.
    match unsafe { args_from_raw::<A>(args, len) } {
        Ok(a) => match f(a) {
            Ok(()) => Status::OK,
            Err(s) => s,
        },
        Err(s) => s,
    }
}

/// The interpreter. Cheap to make; the device comes with `HELLO`.
pub struct Executor {
    pub(crate) options: ExecOptions,
    pub(crate) state: Mutex<Option<State>>,
    last_error: Mutex<Option<String>>,
}

impl Default for Executor {
    fn default() -> Self {
        Executor::new()
    }
}

impl Executor {
    /// An executor with the default options; it has no device until `HELLO`.
    pub fn new() -> Executor {
        Executor::with_options(ExecOptions::default())
    }

    /// An executor with these options.
    pub fn with_options(options: ExecOptions) -> Executor {
        Executor {
            options,
            state: Mutex::new(None),
            last_error: Mutex::new(None),
        }
    }

    /// The text of the last failed `CREATE_COMPUTE_PIPELINE` (why the
    /// converter or Metal refused), for diagnostics: the wire only has a status.
    pub fn last_error(&self) -> Option<String> {
        lock(&self.last_error).clone()
    }

    pub(crate) fn note_error(&self, what: String) {
        if std::env::var_os("VICTORY_METAL_LOG").is_some_and(|v| v != "0") {
            eprintln!("victory-metal back: {what}");
        }
        *lock(&self.last_error) = Some(what);
    }

    /// Runs `f` on the state, or `NOT_INITIALIZED` before `HELLO`.
    pub(crate) fn with_state<R>(&self, f: impl FnOnce(&mut State) -> R) -> Result<R, Status> {
        let mut g = lock(&self.state);
        match g.as_mut() {
            Some(st) => Ok(f(st)),
            None => Err(Status::NOT_INITIALIZED),
        }
    }

    /// One synchronous call (`Link::call`).
    pub fn call(&self, op: SyncOp, args: *mut u8, len: usize) -> Status {
        match op {
            SyncOp::NOP => typed::<victory_metal_proto::sync::NopArgs>(args, len, |_| Ok(())),
            SyncOp::HELLO => typed(args, len, |a| self.hello(a)),
            SyncOp::SHUTDOWN => {
                typed::<victory_metal_proto::sync::ShutdownArgs>(args, len, |_| self.shutdown())
            }
            SyncOp::DESTROY_QUEUE => typed(args, len, |a| self.destroy_queue(a)),
            SyncOp::FENCE_WAIT => typed(args, len, |a| self.fence_wait(a)),
            SyncOp::CREATE_COMPUTE_PIPELINE => typed(args, len, |a| self.create_compute_pipeline(a)),
            op if op.is_known() => match self.with_state(|st| st.call(op, args, len)) {
                Ok(s) => s,
                Err(s) => s,
            },
            op if (victory_metal_proto::sync::SYNC_RESERVED_START
                ..=victory_metal_proto::sync::SYNC_RESERVED_END)
                .contains(&op.0) =>
            {
                Status::UNSUPPORTED
            }
            _ => Status::UNKNOWN_OP,
        }
    }

    /// One block of commands (`Link::submit`): one `ExecuteCommandLists`.
    pub fn submit(&self, block: &[u8]) -> Status {
        // The whole block is checked before anything runs.
        let header = match stream::validate(block) {
            Ok(caps) => caps,
            Err(s) => return s,
        };
        let mut g = lock(&self.state);
        let Some(st) = g.as_mut() else {
            return Status::NOT_INITIALIZED;
        };
        let status = if header.0 & !victory_metal_proto::Caps::KNOWN.0 != 0 {
            Status::BAD_STREAM
        } else if !st.caps_bits().contains(header) {
            Status::UNSUPPORTED
        } else {
            st.run_block(block)
        };
        st.device.maintain();
        status
    }

    /// Test hook: a compute pipeline built from MSL source, for a root
    /// signature that already exists. `threads` is the threadgroup size a
    /// dispatch uses (a converted shader brings it from its `numthreads`;
    /// proto's `DISPATCH` carries thread-group counts only). Not part of
    /// proto: nothing on the wire names MSL.
    pub fn create_compute_pipeline_from_msl(
        &self,
        root_sig: RootSigId,
        source: &str,
        entry: &str,
        threads: [u32; 3],
    ) -> Result<PsoId, Status> {
        self.with_state(|st| st.pipeline_from_msl(root_sig, source, entry, threads))?
    }

    /// Replaces the layout of a root signature's argument buffer with the
    /// one MSC reflection reported (see [`tlab`]). Do it before creating
    /// pipelines from the signature: a dispatch checks that the bound
    /// signature and the pipeline's agree.
    pub fn set_tlab_layout(
        &self,
        root_sig: RootSigId,
        offsets: &[u32],
        size: u32,
    ) -> Result<(), Status> {
        self.with_state(|st| st.set_tlab_layout(root_sig, offsets, size))?
    }

    /// The argument buffer layout of a root signature (tests, tools).
    pub fn tlab_layout(&self, root_sig: RootSigId) -> Result<TlabLayout, Status> {
        self.with_state(|st| {
            st.root_sigs
                .get(root_sig.0)
                .map(|r| r.layout.clone())
                .ok_or(Status::NOT_FOUND)
        })?
    }

    /// The device, once `HELLO` has made it (tests, tools).
    pub fn device(&self) -> Option<Device> {
        lock(&self.state).as_ref().map(|st| st.device.clone())
    }

    /// The queue behind a handle (tests, tools).
    pub fn queue(&self, queue: victory_metal_proto::ids::QueueId) -> Option<crate::queue::Queue> {
        lock(&self.state)
            .as_ref()
            .and_then(|st| st.queues.get(queue.0).cloned())
    }
}

impl Link for Executor {
    fn submit(&self, block: &[u8]) -> Status {
        Executor::submit(self, block)
    }

    fn call(&self, op: SyncOp, args: *mut u8, len: usize) -> Status {
        Executor::call(self, op, args, len)
    }
}

/// A `Link` that calls an [`Executor`] in the same process.
#[derive(Clone, Default)]
pub struct InProcess {
    executor: Arc<Executor>,
}

impl InProcess {
    /// A link to a new executor with the default options.
    pub fn new() -> InProcess {
        InProcess::default()
    }

    /// A link to a new executor with these options.
    pub fn with_options(options: ExecOptions) -> InProcess {
        InProcess {
            executor: Arc::new(Executor::with_options(options)),
        }
    }

    /// A link to an executor someone else keeps too.
    pub fn from_executor(executor: Arc<Executor>) -> InProcess {
        InProcess { executor }
    }

    /// The executor behind the link (the test hooks live there).
    pub fn executor(&self) -> &Arc<Executor> {
        &self.executor
    }
}

impl Link for InProcess {
    fn submit(&self, block: &[u8]) -> Status {
        self.executor.submit(block)
    }

    fn call(&self, op: SyncOp, args: *mut u8, len: usize) -> Status {
        self.executor.call(op, args, len)
    }
}
