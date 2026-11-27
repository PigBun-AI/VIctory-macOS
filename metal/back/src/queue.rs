//! Queues, fences, submission and error mapping.
//!
//! * a [`Queue`] is an `MTL4CommandQueue` with the backend's residency sets
//!   attached (at most 32 per queue), an allocator/command-buffer pool
//!   sized by the number of submissions in flight, a constant ring, and a
//!   private timeline (a `MTLSharedEvent` the queue signals after every
//!   submission; retirement and allocator reuse wait on it);
//! * a [`Fence`] is a `MTLSharedEvent` the front end creates for the
//!   game's fences; the queue signals it with `signalEvent:value:`, waits for it
//!   with `waitForEvent:value:`, the CPU with
//!   [`Fence::wait`] (`waitUntilSignaledValue:timeoutMS:`);
//! * completion news arrives through `MTL4CommitOptions
//!   addFeedbackHandler:` and is mapped as in architecture note 2.6, item 9
//!   ([`status_for_error_code`]).

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTL4CommandAllocator, MTL4CommandBuffer, MTL4CommandQueue, MTL4CommandQueueError,
    MTL4CommandQueueErrorDomain, MTL4CommitFeedback, MTL4CommitOptions, MTLDevice, MTLEvent,
    MTLResidencySet, MTLSharedEvent,
};
use std::ptr::NonNull;

use crate::descriptors::{ArgumentTable, ConstantRing, RingAlloc, RingFull};
use crate::device::Device;
use crate::error::{Error, Result};
use crate::record::CommandList;

/// Header `MTL4CommandQueue.h`: a queue takes at most this many residency
/// sets.
pub const MAX_RESIDENCY_SETS_PER_QUEUE: u32 = 32;

/// What a failed command buffer says about the device (architecture note
/// 2.6, item 9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceStatus {
    /// Nothing went wrong.
    Healthy,
    /// A submission timed out (`MTL4CommandQueueErrorTimeout`): the front
    /// end reports a hung device.
    Hung,
    /// Access revoked, internal error, out of memory, ...: the front end
    /// reports a removed device.
    Removed,
}

impl DeviceStatus {
    fn from_u8(v: u8) -> DeviceStatus {
        match v {
            0 => DeviceStatus::Healthy,
            1 => DeviceStatus::Hung,
            _ => DeviceStatus::Removed,
        }
    }
}

/// `MTL4CommandQueueError` code to [`DeviceStatus`]. `Timeout` is a hang;
/// `AccessRevoked`, `Internal` and `OutOfMemory` (the three the note names)
/// remove the device, and so do `NotPermitted` and `DeviceRemoved` and any
/// code we do not know (an unknown failure is not a safe thing to continue
/// after).
pub fn status_for_error_code(code: isize) -> DeviceStatus {
    match MTL4CommandQueueError(code) {
        MTL4CommandQueueError::None => DeviceStatus::Healthy,
        MTL4CommandQueueError::Timeout => DeviceStatus::Hung,
        _ => DeviceStatus::Removed,
    }
}

/// The last failure a queue reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// `NSError.code`.
    pub code: isize,
    /// `NSError.domain`.
    pub domain: String,
    /// `NSError.localizedDescription`.
    pub message: String,
}

#[derive(Default)]
struct Sink {
    state: AtomicU8,
    last: Mutex<Option<Failure>>,
    completed: AtomicU64,
    gpu_ns: AtomicU64,
}

impl Sink {
    fn record(&self, failure: Option<Failure>, gpu_ns: u64) {
        self.completed.fetch_add(1, Ordering::AcqRel);
        self.gpu_ns.fetch_add(gpu_ns, Ordering::Relaxed);
        let Some(failure) = failure else {
            return;
        };
        let status = status_for_error_code(failure.code);
        // Healthy < Hung < Removed: the worst status seen stays.
        self.state.fetch_max(status as u8, Ordering::AcqRel);
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(failure);
    }
}

/// A queue's private timeline.
pub struct Timeline {
    event: Retained<ProtocolObject<dyn MTLSharedEvent>>,
    submitted: AtomicU64,
}

// MTLSharedEvent is thread-safe.
unsafe impl Send for Timeline {}
unsafe impl Sync for Timeline {}

impl Timeline {
    /// The highest value handed out (signalled or about to be).
    pub fn submitted(&self) -> u64 {
        self.submitted.load(Ordering::Acquire)
    }

    /// The highest value the GPU has signalled.
    pub fn completed(&self) -> u64 {
        self.event.signaledValue()
    }

    /// Waits until the timeline reaches `value`.
    pub fn wait(&self, value: u64, timeout_ms: u64) -> bool {
        self.completed() >= value
            || self
                .event
                .waitUntilSignaledValue_timeoutMS(value, timeout_ms)
    }
}

/// A `MTLSharedEvent`: the backend's fence.
#[derive(Clone)]
pub struct Fence {
    event: Retained<ProtocolObject<dyn MTLSharedEvent>>,
}

unsafe impl Send for Fence {}
unsafe impl Sync for Fence {}

impl Fence {
    /// The highest signalled value (the front end's `GetCompletedValue`).
    pub fn completed_value(&self) -> u64 {
        self.event.signaledValue()
    }

    /// CPU signal (the front end's fence `Signal`).
    pub fn signal_cpu(&self, value: u64) {
        self.event.setSignaledValue(value);
    }

    /// CPU wait (`waitUntilSignaledValue:timeoutMS:`); false on timeout.
    pub fn wait(&self, value: u64, timeout_ms: u64) -> bool {
        self.completed_value() >= value
            || self
                .event
                .waitUntilSignaledValue_timeoutMS(value, timeout_ms)
    }
}

impl Device {
    /// A new fence with value 0.
    pub fn create_fence(&self) -> Result<Fence> {
        let event = self
            .raw()
            .newSharedEvent()
            .ok_or_else(|| Error::Metal("newSharedEvent returned nil".into()))?;
        Ok(Fence { event })
    }
}

/// How a queue is set up.
#[derive(Clone, Copy, Debug)]
pub struct QueueOptions {
    /// Allocators/command buffers kept for submissions in flight; the pool
    /// grows past this to `max_lists` before a recorder has to wait.
    pub in_flight: u32,
    /// Upper bound of pooled command buffers (lists recorded at once plus
    /// lists in flight).
    pub max_lists: u32,
    /// Size of one constant-ring segment in bytes.
    pub ring_segment_bytes: u64,
    /// Number of constant-ring segments.
    pub ring_segments: u32,
    /// Start every encoder with a queue-level all-stage barrier from
    /// everything committed before it (consumer form, no extra encoder),
    /// which orders each list after the earlier ones (the game's API
    /// executes lists of a queue in order; Metal 4 would overlap them).
    pub inter_submit_barrier: bool,
}

impl Default for QueueOptions {
    fn default() -> Self {
        QueueOptions {
            in_flight: 3,
            max_lists: 64,
            ring_segment_bytes: 1 << 20,
            ring_segments: 4,
            inter_submit_barrier: true,
        }
    }
}

pub(crate) struct Slot {
    pub(crate) allocator: Retained<ProtocolObject<dyn MTL4CommandAllocator>>,
    pub(crate) cb: Retained<ProtocolObject<dyn MTL4CommandBuffer>>,
    pub(crate) table: ArgumentTable,
    pub(crate) busy_until: u64,
}

pub(crate) struct QueueInner {
    pub(crate) device: Device,
    pub(crate) raw: Retained<ProtocolObject<dyn MTL4CommandQueue>>,
    pub(crate) timeline: Arc<Timeline>,
    options: QueueOptions,
    barrier: AtomicBool,
    idle: Mutex<Vec<Slot>>,
    slots_made: Mutex<u32>,
    ring: Mutex<ConstantRing>,
    sink: Arc<Sink>,
    attached: Mutex<u32>,
    submissions: AtomicU64,
}

// The Metal objects are thread-safe (queue, event) or only used with a lock
// held (slots, ring).
unsafe impl Send for QueueInner {}
unsafe impl Sync for QueueInner {}

/// A Metal 4 command queue with everything the backend hangs on it.
#[derive(Clone)]
pub struct Queue {
    pub(crate) inner: Arc<QueueInner>,
}

/// Commit options with a feedback handler that reports to `sink`. One
/// per commit: Metal runs a handler for the first commit only.
fn commit_options(sink: &Arc<Sink>) -> Retained<MTL4CommitOptions> {
    let options = MTL4CommitOptions::new();
    let sink = sink.clone();
    let block = RcBlock::new(move |fb: NonNull<ProtocolObject<dyn MTL4CommitFeedback>>| {
        // SAFETY: Metal passes a live feedback object for the call.
        let fb = unsafe { fb.as_ref() };
        let gpu_ns = ((fb.GPUEndTime() - fb.GPUStartTime()).max(0.0) * 1e9) as u64;
        let failure = fb.error().map(|e| {
            // SAFETY: a plain extern static.
            let ours = e
                .domain()
                .isEqualToString(unsafe { MTL4CommandQueueErrorDomain });
            Failure {
                // A code from another domain is not ours to interpret.
                code: if ours {
                    e.code()
                } else {
                    MTL4CommandQueueError::Internal.0
                },
                domain: e.domain().to_string(),
                message: e.localizedDescription().to_string(),
            }
        });
        sink.record(failure, gpu_ns);
    });
    // SAFETY: the block is valid; Metal copies it.
    unsafe { options.addFeedbackHandler(RcBlock::as_ptr(&block)) };
    options
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Queue {
    /// Makes a queue with the default options.
    pub fn new(device: &Device) -> Result<Queue> {
        Queue::with_options(device, QueueOptions::default())
    }

    /// Makes a queue; the backend's residency sets are attached.
    pub fn with_options(device: &Device, options: QueueOptions) -> Result<Queue> {
        let raw = device
            .raw()
            .newMTL4CommandQueue()
            .ok_or_else(|| Error::Metal("newMTL4CommandQueue returned nil".into()))?;
        let sets = device.inner.residency.sets();
        for set in &sets {
            raw.addResidencySet(set);
        }
        let event = device
            .raw()
            .newSharedEvent()
            .ok_or_else(|| Error::Metal("newSharedEvent returned nil".into()))?;
        let timeline = Arc::new(Timeline {
            event,
            submitted: AtomicU64::new(0),
        });
        device.register_timeline(&timeline);
        let ring = ConstantRing::new(device, options.ring_segment_bytes, options.ring_segments)?;
        let sink = Arc::new(Sink::default());
        Ok(Queue {
            inner: Arc::new(QueueInner {
                device: device.clone(),
                raw,
                timeline,
                options,
                barrier: AtomicBool::new(options.inter_submit_barrier),
                idle: Mutex::new(Vec::new()),
                slots_made: Mutex::new(0),
                ring: Mutex::new(ring),
                sink,
                attached: Mutex::new(sets.len() as u32),
                submissions: AtomicU64::new(0),
            }),
        })
    }

    /// The device.
    pub fn device(&self) -> &Device {
        &self.inner.device
    }

    /// Turns the conservative all-stage barrier between lists on or off
    /// (for timing its cost). Lists begun afterwards use the new setting.
    pub fn set_inter_submit_barrier(&self, on: bool) {
        self.inner.barrier.store(on, Ordering::Relaxed);
    }

    /// Whether lists begun now start their encoders with the all-stage barrier.
    pub fn inter_submit_barrier(&self) -> bool {
        self.inner.barrier.load(Ordering::Relaxed)
    }

    /// Attaches one more residency set (the presentation set of a
    /// `CAMetalLayer`, say). Fails past the queue's limit of 32.
    pub fn add_residency_set(&self, set: &ProtocolObject<dyn MTLResidencySet>) -> Result<()> {
        let mut n = lock(&self.inner.attached);
        if *n >= MAX_RESIDENCY_SETS_PER_QUEUE {
            return Err(Error::BadArgument(format!(
                "a queue takes at most {MAX_RESIDENCY_SETS_PER_QUEUE} residency sets"
            )));
        }
        self.inner.raw.addResidencySet(set);
        *n += 1;
        Ok(())
    }

    /// How many residency sets are attached.
    pub fn residency_sets(&self) -> u32 {
        *lock(&self.inner.attached)
    }

    /// What the completed work says about the device.
    pub fn status(&self) -> DeviceStatus {
        DeviceStatus::from_u8(self.inner.sink.state.load(Ordering::Acquire))
    }

    /// The last failure reported, if any.
    pub fn last_failure(&self) -> Option<Failure> {
        lock(&self.inner.sink.last).clone()
    }

    /// Submissions the GPU has completed and reported through the commit
    /// feedback handler (one report per `submit`).
    pub fn completed_submissions(&self) -> u64 {
        self.inner.sink.completed.load(Ordering::Relaxed)
    }

    /// Total GPU time reported by feedback, nanoseconds.
    pub fn gpu_time_ns(&self) -> u64 {
        self.inner.sink.gpu_ns.load(Ordering::Relaxed)
    }

    /// Submissions made so far.
    pub fn submissions(&self) -> u64 {
        self.inner.submissions.load(Ordering::Relaxed)
    }

    /// The timeline: value of the latest submission and what completed.
    pub fn timeline(&self) -> &Arc<Timeline> {
        &self.inner.timeline
    }

    /// Waits until the submission with `value` (as returned by
    /// [`submit`](Self::submit)) has completed.
    pub fn wait_for(&self, value: u64, timeout_ms: u64) -> Result<()> {
        if self.inner.timeline.wait(value, timeout_ms) {
            // Feedback (and with it any error) arrives on another thread
            // shortly after the timeline signal. When waiting for the very
            // latest submission, give it a moment so that `status` is
            // current when this returns.
            if value >= self.inner.timeline.submitted() {
                let committed = self.inner.submissions.load(Ordering::Acquire);
                let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
                while self.inner.sink.completed.load(Ordering::Acquire) < committed
                    && std::time::Instant::now() < deadline
                {
                    std::thread::yield_now();
                }
            }
            self.check()
        } else {
            Err(Error::Timeout)
        }
    }

    /// Waits for everything submitted so far.
    pub fn wait_idle(&self, timeout_ms: u64) -> Result<()> {
        self.wait_for(self.inner.timeline.submitted(), timeout_ms)
    }

    fn check(&self) -> Result<()> {
        match self.status() {
            DeviceStatus::Removed => Err(Error::DeviceLost(DeviceStatus::Removed)),
            _ => Ok(()),
        }
    }

    /// `signalEvent:value:` on the queue: after all work submitted so far.
    pub fn signal(&self, fence: &Fence, value: u64) {
        let event: &ProtocolObject<dyn MTLEvent> = ProtocolObject::from_ref(&*fence.event);
        self.inner.raw.signalEvent_value(event, value);
    }

    /// `waitForEvent:value:` on the queue: later work waits for the fence.
    pub fn wait_fence(&self, fence: &Fence, value: u64) {
        let event: &ProtocolObject<dyn MTLEvent> = ProtocolObject::from_ref(&*fence.event);
        self.inner.raw.waitForEvent_value(event, value);
    }

    /// Takes a pooled allocator + command buffer, waiting for a busy one
    /// when the pool is at its limit.
    pub(crate) fn acquire_slot(&self) -> Result<Slot> {
        let completed = self.inner.timeline.completed();
        {
            let mut idle = lock(&self.inner.idle);
            if let Some(i) = idle.iter().position(|s| s.busy_until <= completed) {
                return Ok(idle.swap_remove(i));
            }
        }
        {
            let mut made = lock(&self.inner.slots_made);
            if *made
                < self
                    .inner
                    .options
                    .max_lists
                    .max(self.inner.options.in_flight)
            {
                *made += 1;
                drop(made);
                return self.make_slot();
            }
        }
        // All slots are out or in flight: wait for the one that finishes first.
        let slot = {
            let mut idle = lock(&self.inner.idle);
            let i = idle
                .iter()
                .enumerate()
                .min_by_key(|(_, s)| s.busy_until)
                .map(|(i, _)| i);
            i.map(|i| idle.swap_remove(i))
        };
        match slot {
            Some(slot) => {
                if !self.inner.timeline.wait(slot.busy_until, 10_000) {
                    lock(&self.inner.idle).push(slot);
                    return Err(Error::Timeout);
                }
                Ok(slot)
            }
            None => Err(Error::BadArgument(format!(
                "more than {} command lists are being recorded at once",
                self.inner.options.max_lists
            ))),
        }
    }

    fn make_slot(&self) -> Result<Slot> {
        let device = self.inner.device.raw();
        let allocator = device
            .newCommandAllocator()
            .ok_or_else(|| Error::Metal("newCommandAllocator returned nil".into()))?;
        let cb = device
            .newCommandBuffer()
            .ok_or_else(|| Error::Metal("newCommandBuffer (MTL4) returned nil".into()))?;
        let table = ArgumentTable::new(&self.inner.device, 31)?;
        Ok(Slot {
            allocator,
            cb,
            table,
            busy_until: 0,
        })
    }

    pub(crate) fn return_slot(&self, mut slot: Slot, busy_until: u64) {
        slot.busy_until = busy_until;
        lock(&self.inner.idle).push(slot);
    }

    /// Space in the constant ring for root constants / root descriptors /
    /// top-level argument buffers of one draw or dispatch. Waits for the GPU
    /// when the ring is full; fails when one submission alone needs more
    /// than the whole ring.
    pub fn alloc_constants(&self, size: u64, align: u64) -> Result<RingAlloc> {
        loop {
            let completed = self.inner.timeline.completed();
            let r = lock(&self.inner.ring).try_alloc(size, align, completed);
            match r {
                Ok(a) => return Ok(a),
                Err(RingFull::TooBig) => {
                    return Err(Error::BadArgument(format!(
                        "{size} bytes do not fit a constant-ring segment"
                    )));
                }
                Err(RingFull::Open) => {
                    return Err(Error::BadArgument(
                        "one submission needs more constant space than the whole ring".into(),
                    ));
                }
                Err(RingFull::Busy(value)) => {
                    if !self.inner.timeline.wait(value, 10_000) {
                        return Err(Error::Timeout);
                    }
                }
            }
        }
    }

    /// Bytes of one constant-ring segment: the most one [`alloc_constants`](Self::alloc_constants) gives.
    pub fn constant_segment_bytes(&self) -> u64 {
        lock(&self.inner.ring).segment_bytes()
    }

    /// The constant ring's own buffer (it is in the heap residency set).
    pub fn constant_ring_va(&self) -> u64 {
        lock(&self.inner.ring).base_va()
    }

    /// Commits the lists (in order, one `commit:count:`), then signals the
    /// queue's timeline. Returns the timeline value that is reached when all
    /// of them have completed.
    ///
    /// Lists must be recorded one after the other and submitted in the order
    /// they were recorded (the constant ring is sealed per submission).
    pub fn submit(&self, lists: Vec<CommandList>) -> Result<u64> {
        self.check()?;
        if lists.is_empty() {
            return Err(Error::BadArgument("no command lists".into()));
        }
        for l in &lists {
            if !l.is_ended() {
                return Err(Error::BadArgument("command list was not ended".into()));
            }
            if !Arc::ptr_eq(&l.queue().inner, &self.inner) {
                return Err(Error::BadArgument("list belongs to another queue".into()));
            }
        }
        // Additions to the residency sets must be committed before the GPU
        // can need them; finished retirements are collected on the way.
        self.inner.device.maintain();
        // The value is reserved before the commit: anything destroyed from
        // now on waits for it (see `DeviceInner::in_flight_waits`).
        let value = self.inner.timeline.submitted.fetch_add(1, Ordering::AcqRel) + 1;
        let mut buffers: Vec<NonNull<ProtocolObject<dyn MTL4CommandBuffer>>> =
            lists.iter().map(CommandList::raw_command_buffer).collect();
        // SAFETY: `buffers` holds valid, ended command buffers for the call.
        unsafe {
            self.inner.raw.commit_count_options(
                NonNull::new(buffers.as_mut_ptr()).expect("non-empty"),
                buffers.len(),
                &commit_options(&self.inner.sink),
            );
        }
        let event: &ProtocolObject<dyn MTLEvent> =
            ProtocolObject::from_ref(&*self.inner.timeline.event);
        self.inner.raw.signalEvent_value(event, value);
        lock(&self.inner.ring).seal(value);
        self.inner.submissions.fetch_add(1, Ordering::Relaxed);
        for list in lists {
            list.into_submitted(value);
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_map_as_the_note_says() {
        use MTL4CommandQueueError as E;
        assert_eq!(status_for_error_code(E::None.0), DeviceStatus::Healthy);
        assert_eq!(status_for_error_code(E::Timeout.0), DeviceStatus::Hung);
        for e in [E::AccessRevoked, E::Internal, E::OutOfMemory] {
            assert_eq!(status_for_error_code(e.0), DeviceStatus::Removed, "{e:?}");
        }
        // Not named by the note; removed rather than silently continued.
        for e in [E::NotPermitted, E::DeviceRemoved] {
            assert_eq!(status_for_error_code(e.0), DeviceStatus::Removed, "{e:?}");
        }
        assert_eq!(status_for_error_code(9999), DeviceStatus::Removed);
    }

    #[test]
    fn sink_keeps_the_worst_status() {
        let s = Sink::default();
        let f = |code: isize| Failure {
            code,
            domain: "d".into(),
            message: "m".into(),
        };
        s.record(None, 5);
        assert_eq!(s.state.load(Ordering::Relaxed), 0);
        s.record(Some(f(MTL4CommandQueueError::Timeout.0)), 5);
        assert_eq!(s.state.load(Ordering::Relaxed), DeviceStatus::Hung as u8);
        s.record(Some(f(MTL4CommandQueueError::AccessRevoked.0)), 5);
        assert_eq!(s.state.load(Ordering::Relaxed), DeviceStatus::Removed as u8);
        // A later timeout does not lower it.
        s.record(Some(f(MTL4CommandQueueError::Timeout.0)), 5);
        assert_eq!(s.state.load(Ordering::Relaxed), DeviceStatus::Removed as u8);
        assert_eq!(s.completed.load(Ordering::Relaxed), 4);
        assert_eq!(s.gpu_ns.load(Ordering::Relaxed), 20);
    }
}
