//! Residency sets, owned by the backend (architecture note 2.6, item 6;
//! layering rule 11).
//!
//! Metal 4 has no `useResource` / `useHeap`: a resource the GPU may touch
//! must be in a `MTLResidencySet` attached to the queue (or the command
//! buffer). Bindless access (descriptor heaps) makes "may touch" mean
//! "every live resource", so the backend keeps its own layered sets:
//!
//! | [`SetKind`] | holds |
//! |---|---|
//! | `Title` | `wrap(ptr, len)` buffers: the title's memory |
//! | `Heap` | descriptor heaps, sampler heaps, the constant ring |
//! | `Resource` | ordinary buffers, textures, heaps, pipelines |
//! | `Indirect` | indirect-argument buffers |
//!
//! The presentation set (`CAMetalLayer.residencySet`, system maintained,
//! never modified here) is attached to a queue with
//! [`Queue::add_residency_set`](crate::queue::Queue::add_residency_set);
//! it counts towards the 32 sets a queue accepts.
//!
//! Life cycle: a created resource is added at once; additions are committed
//! in batches ([`Residency::commit`], which every submission calls first).
//! A destroyed resource is *retired*: it keeps its place, and its memory,
//! until the queue timelines it was possibly used on have passed the value
//! that was current at destruction; only then is it removed and the sets
//! committed ([`Residency::reclaim`]). Removal is never committed while the
//! GPU may still be using the resource.

use std::any::Any;
use std::sync::{Arc, Mutex, MutexGuard};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLAllocation, MTLDevice, MTLResidencySet, MTLResidencySetDescriptor};

use crate::error::{Error, Result};
use crate::queue::Timeline;

/// Which of the backend's sets a resource lives in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SetKind {
    /// The title's memory (`wrap`).
    Title,
    /// Descriptor heaps, sampler heaps, constant ring, top-level argument
    /// buffers.
    Heap,
    /// Ordinary resources and pipelines.
    Resource,
    /// Indirect argument buffers.
    Indirect,
}

impl SetKind {
    /// All kinds, in set order.
    pub const ALL: [SetKind; 4] = [
        SetKind::Title,
        SetKind::Heap,
        SetKind::Resource,
        SetKind::Indirect,
    ];

    fn index(self) -> usize {
        self as usize
    }
}

/// A destroyed resource waiting for the GPU to be done with it.
pub struct Retired {
    /// The resource, as the set knows it.
    pub object: Retained<ProtocolObject<dyn MTLAllocation>>,
    /// The set it is in.
    pub kind: SetKind,
    /// `(timeline, value)`: not removable before each timeline has reached
    /// its value.
    pub waits: Vec<(Arc<Timeline>, u64)>,
    /// Anything else to drop together with the resource (a page claim).
    pub keep: Option<Box<dyn Any + Send>>,
}

struct Slot {
    set: Retained<ProtocolObject<dyn MTLResidencySet>>,
    dirty: bool,
    count: usize,
}

struct Inner {
    slots: [Slot; 4],
    retired: Vec<Retired>,
    commits: u64,
}

// The Metal objects inside are only touched with the mutex held.
unsafe impl Send for Inner {}

/// The backend's residency sets.
pub struct Residency {
    inner: Mutex<Inner>,
}

impl Residency {
    /// Makes the four (empty) sets.
    pub fn new(device: &ProtocolObject<dyn MTLDevice>) -> Result<Residency> {
        let make = |label: &str| -> Result<Slot> {
            let descriptor = MTLResidencySetDescriptor::new();
            descriptor.setLabel(Some(&objc2_foundation::NSString::from_str(label)));
            let set = device
                .newResidencySetWithDescriptor_error(&descriptor)
                .map_err(|e| {
                    Error::Metal(format!(
                        "residency set {label}: {}",
                        e.localizedDescription()
                    ))
                })?;
            Ok(Slot {
                set,
                dirty: false,
                count: 0,
            })
        };
        Ok(Residency {
            inner: Mutex::new(Inner {
                slots: [
                    make("victory title memory")?,
                    make("victory heaps")?,
                    make("victory resources")?,
                    make("victory indirect")?,
                ],
                retired: Vec::new(),
                commits: 0,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Adds a resource to a set. Takes effect at the next [`commit`](Self::commit).
    pub fn add(&self, kind: SetKind, allocation: &ProtocolObject<dyn MTLAllocation>) {
        let mut inner = self.lock();
        let slot = &mut inner.slots[kind.index()];
        slot.set.addAllocation(allocation);
        slot.count += 1;
        slot.dirty = true;
    }

    /// Whether the set `kind` has the resource (committed or not).
    pub fn contains(&self, kind: SetKind, allocation: &ProtocolObject<dyn MTLAllocation>) -> bool {
        self.lock().slots[kind.index()]
            .set
            .containsAllocation(allocation)
    }

    /// Whether any of the four sets has the resource.
    pub fn contains_any(&self, allocation: &ProtocolObject<dyn MTLAllocation>) -> bool {
        let inner = self.lock();
        inner
            .slots
            .iter()
            .any(|s| s.set.containsAllocation(allocation))
    }

    /// How many resources the set `kind` holds.
    pub fn len(&self, kind: SetKind) -> usize {
        self.lock().slots[kind.index()].count
    }

    /// Commits every set that changed; returns how many it committed.
    pub fn commit(&self) -> usize {
        let mut inner = self.lock();
        Self::commit_dirty(&mut inner)
    }

    fn commit_dirty(inner: &mut Inner) -> usize {
        let mut n = 0;
        for slot in &mut inner.slots {
            if slot.dirty {
                slot.set.commit();
                slot.dirty = false;
                n += 1;
            }
        }
        inner.commits += n as u64;
        n
    }

    /// Asks the system to make everything resident now (a warm-up; the
    /// queue would do it at the first commit anyway).
    pub fn request_residency(&self) {
        let mut inner = self.lock();
        Self::commit_dirty(&mut inner);
        for slot in &inner.slots {
            slot.set.requestResidency();
        }
    }

    /// Number of `commit` calls on sets so far (for the tests and stats).
    pub fn commits(&self) -> u64 {
        self.lock().commits
    }

    /// The four sets, for attaching to a queue.
    pub fn sets(&self) -> Vec<Retained<ProtocolObject<dyn MTLResidencySet>>> {
        self.lock().slots.iter().map(|s| s.set.clone()).collect()
    }

    /// Queues a destroyed resource for removal. It stays in its set and
    /// alive until [`reclaim`](Self::reclaim) sees every wait satisfied.
    pub fn retire(&self, retired: Retired) {
        self.lock().retired.push(retired);
    }

    /// How many destroyed resources still wait.
    pub fn pending_retired(&self) -> usize {
        self.lock().retired.len()
    }

    /// Removes every retired resource the GPU is done with (all of its
    /// timelines have reached the recorded value), commits once for the
    /// whole batch, and drops the resources. Returns how many it freed.
    pub fn reclaim(&self) -> usize {
        let freed: Vec<Retired> = {
            let mut inner = self.lock();
            if inner.retired.is_empty() {
                return 0;
            }
            let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut inner.retired)
                .into_iter()
                .partition(|r| r.waits.iter().all(|(t, v)| t.completed() >= *v));
            inner.retired = waiting;
            for r in &ready {
                let slot = &mut inner.slots[r.kind.index()];
                slot.set.removeAllocation(&r.object);
                slot.count = slot.count.saturating_sub(1);
                slot.dirty = true;
            }
            if !ready.is_empty() {
                Self::commit_dirty(&mut inner);
            }
            ready
        };
        // Dropped outside the lock: releasing a buffer may run deallocators.
        let n = freed.len();
        drop(freed);
        n
    }
}
