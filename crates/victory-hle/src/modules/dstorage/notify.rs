//! Owned completion markers. Workers claim one per lane; no external call under the lane lock.
use super::*;
use std::os::windows::io::{AsRawHandle, BorrowedHandle, OwnedHandle};
use windows_sys::Win32::System::Threading::SetEvent;
type P = *mut c_void;
struct Fence(P);
impl Fence {
    fn reference(p: P) -> Self {
        let f = Self(p);
        let add: extern "system" fn(P) -> u32 = unsafe { std::mem::transmute(f.slot(1)) };
        add(p);
        f
    }
    // ID3D12Fence inherits IUnknown and ID3D12DeviceChild. Victory GPU and native
    // D3D12 both install Signal at byte offset 0x50; let that implementation wake waiters.
    fn slot(&self, s: usize) -> *const () {
        unsafe { *(*self.0.cast::<*const *const ()>()).add(s) }
    }
    fn signal(&self, v: u64) -> HRESULT {
        let signal: extern "system" fn(P, u64) -> HRESULT =
            unsafe { std::mem::transmute(self.slot(10)) };
        signal(self.0, v)
    }
}
impl Drop for Fence {
    fn drop(&mut self) {
        let release: extern "system" fn(P) -> u32 = unsafe { std::mem::transmute(self.slot(2)) };
        release(self.0);
    }
}
enum Target {
    Fence(Fence, u64),
    Event(OwnedHandle),
}
pub(super) struct External {
    target: Target,
    at: u64,
}
impl External {
    pub(super) fn event(owned: OwnedHandle) -> Self {
        Self {
            target: Target::Event(owned),
            at: stream_log::now(),
        }
    }
    pub(super) fn kind(&self) -> usize {
        match self.target {
            Target::Fence(..) => 1,
            Target::Event(_) => 2,
        }
    }
    pub(super) fn publish(&self) -> bool {
        match &self.target {
            Target::Fence(f, v) => f.signal(*v) >= 0,
            Target::Event(h) => unsafe { SetEvent(h.as_raw_handle()) != 0 },
        }
    }
}
pub(super) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var_os("VICTORY_DSTORAGE_NOTIFY").as_deref() != Some(std::ffi::OsStr::new("0"))
    })
}
pub(super) fn enqueue_signal(p: QueueRef, f: P, v: u64) {
    if !enabled() || f.is_null() {
        abi::reject(unsafe { &*p }.state.iface, 5);
        return;
    }
    queue(
        p,
        Command::External(External {
            target: Target::Fence(Fence::reference(f), v),
            at: stream_log::now(),
        }),
    )
}
pub(super) fn enqueue_event(p: QueueRef, h: P) {
    let fail = || abi::reject(unsafe { &*p }.state.iface, 12);
    if !enabled() || h.is_null() || h as isize == -1 {
        fail();
        return;
    }
    let Ok(owned) = (unsafe { BorrowedHandle::borrow_raw(h) }).try_clone_to_owned() else {
        fail();
        return;
    };
    queue(p, Command::External(External::event(owned)))
}
const KIND: [&str; 3] = ["status", "fence", "event"];
impl Command {
    pub(super) fn marker(&self) -> Option<(usize, u64)> {
        match self {
            Self::Read(_) => None,
            Self::Status(s) => Some((0, s.enqueued)),
            Self::External(m) => Some((m.kind(), m.at)),
        }
    }
}
/// Retain cold diagnostics, never queue resources, after Close/last Release.
pub(super) struct Archive {
    id: u64,
    enqueues: u64,
    submits: u64,
    markers: [[u64; 3]; 3],
    history: Vec<(u64, usize, u64, &'static str)>,
}
impl From<Lane> for Archive {
    fn from(l: Lane) -> Self {
        Self {
            id: l.id,
            enqueues: l.enqueues,
            submits: l.submits,
            markers: l.markers,
            history: l.history,
        }
    }
}
impl Archive {
    pub(super) fn dump(&self, reason: &str) {
        let id = self.id;
        eprintln!(
            "dstorage.queue queue={id} reason={reason} closed=true enqueue={} submit={} pending=0 oldest_seq=None",
            self.enqueues, self.submits
        );
        totals(id, &self.markers, &self.history);
    }
}
fn totals(id: u64, markers: &[[u64; 3]; 3], history: &[(u64, usize, u64, &str)]) {
    for (k, m) in markers.iter().enumerate() {
        eprintln!(
            "dstorage.marker.summary queue={id} type={} enqueued={} published_ok={} published_fail={}",
            KIND[k], m[0], m[1], m[2]
        );
    }
    for &(seq, k, at, reason) in history {
        eprintln!(
            "dstorage.unpublished queue={id} seq={seq} type={} enqueue_ms={at} reason={reason}",
            KIND[k]
        );
    }
}
impl Lane {
    pub(super) fn remember_cancelled(&mut self) {
        for (seq, c) in self
            .queued_seq
            .iter()
            .zip(&self.unsubmitted)
            .chain(self.statuses.iter().map(|(s, c)| (s, c)))
        {
            if let Some((k, at)) = c.marker() {
                self.history.push((*seq, k, at, "cancelled-by-close"));
            }
        }
    }
    pub(super) fn take_external(&mut self) -> Option<(u64, External)> {
        let &(seq, Command::External(_)) = self.statuses.front()? else {
            return None;
        };
        if self.closed
            || self.publishing.is_some()
            || self.reads.front().is_some_and(|r| r.0 < seq)
            || self.running.first().is_some_and(|r| *r < seq)
        {
            return None;
        }
        let (_, Command::External(m)) = self.statuses.pop_front()? else {
            unreachable!()
        };
        self.publishing = Some((seq, m.kind(), m.at));
        Some((seq, m))
    }
    pub(super) fn count_marker(&mut self, k: usize, stage: usize, seq: u64) {
        let n = &mut self.markers[k][stage];
        *n += 1;
        if n.is_power_of_two() {
            eprintln!(
                "dstorage.marker queue={} seq={seq} type={} {}={n}",
                self.id,
                KIND[k],
                ["enqueued", "published_ok", "published_fail"][stage]
            );
        }
    }
    pub(super) fn line(&self, id: u64, reason: &str) {
        let oldest = [
            self.running.first().copied(),
            self.reads.front().map(|x| x.0),
            self.statuses.front().map(|x| x.0),
            self.queued_seq.front().copied(),
            self.publishing.map(|x| x.0),
        ]
        .into_iter()
        .flatten()
        .min();
        let newest = [
            self.running.last().copied(),
            self.reads.back().map(|x| x.0),
            self.statuses.back().map(|x| x.0),
            self.queued_seq.back().copied(),
            self.publishing.map(|x| x.0),
        ]
        .into_iter()
        .flatten()
        .max();
        eprintln!(
            "dstorage.queue queue={id} reason={reason} enqueue={} submit={} pending={} oldest_seq={oldest:?} newest_seq={newest:?}",
            self.enqueues,
            self.submits,
            self.entries()
        );
    }
    pub(super) fn dump(&self, id: u64, reason: &str) {
        self.line(id, reason);
        totals(id, &self.markers, &self.history);
        for (seq, k, at) in self
            .queued_seq
            .iter()
            .zip(&self.unsubmitted)
            .filter_map(|(&s, c)| c.marker().map(|(k, t)| (s, k, t)))
            .chain(
                self.statuses
                    .iter()
                    .filter_map(|(s, c)| c.marker().map(|(k, t)| (*s, k, t))),
            )
            .chain(self.publishing)
        {
            eprintln!(
                "dstorage.unpublished queue={id} seq={seq} type={} enqueue_ms={at} reason={reason}",
                KIND[k]
            );
        }
    }
}
