//! Where the card's time goes, by kind of work (`VICTORY_GPU_SPLIT=1`, off by
//! default, in release builds too), and why the card's own steps were refused.
//!
//! A submission's lists are bracketed by timestamp lists made ahead (as in
//! `stamps.rs`), one stamp for each change of kind, not one for each list:
//!
//! * `title`: the title's own draws and dispatches (a host list up to the
//!   point where `HostList::close` starts adding work of ours);
//! * `tail`: what `close` adds at the end of a title list (twin copies of
//!   depth, copy-back of textures, indirect-mesh and budget work);
//! * `hash_cmp`: the compare list that hashes and compares host copies in
//!   the card's memory;
//! * `fill_copy`: the steps given to the card (fills, copies), and every
//!   list's prelude (state transitions, twin copies);
//! * `idle`: the card with nothing queued between one submission's last
//!   stamp and the next one's first.
//!
//! The stamps are resolved into memory the PC reads. Nothing waits: a
//! submission's stamps are read once the host fence has passed its signal
//! (`GetCompletedValue`, a plain query), by the next submission or the
//! once-a-second line. Ray tracing's acceleration structure builds are not
//! a kind of their own: the translator does not record them (they would be
//! in `title`).

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

/// The stamps of a ring: a submission of 600 lists takes some 1,500.
pub(super) const RING: usize = 8192;
/// Slots of the query heap and buffer: the split's [`RING`] first, then the
/// ones of `programs.rs` (`VICTORY_GPU_PER_PROGRAM`), which stamp inside open lists.
pub(super) const TOTAL: usize = 65536;

pub(super) const TITLE: u8 = 0;
pub(super) const TAIL: u8 = 1;
pub(super) const HASH: u8 = 2;
pub(super) const FILL: u8 = 3;
const KINDS: usize = 4;

/// Whether `VICTORY_GPU_SPLIT` is on.
pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::switch::on("VICTORY_GPU_SPLIT"))
}

pub(super) struct Ring {
    /// By slot, the closed list that stamps it.
    lists: Vec<ID3D12CommandList>,
    pub(super) heap: ID3D12QueryHeap,
    pub(super) buffer: ID3D12Resource,
    /// The mapping of the buffer, a tick count a slot.
    pub(super) back: usize,
    _allocator: ID3D12CommandAllocator,
}

// SAFETY: free-threaded objects, and a mapping only read.
unsafe impl Send for Ring {}
// SAFETY: as above.
unsafe impl Sync for Ring {}

static RING_STATE: OnceLock<Option<Ring>> = OnceLock::new();
/// The next stamp's number; its slot is the number modulo [`RING`].
static NEXT: AtomicU64 = AtomicU64::new(1);
/// No stamp older than this is still to be read.
static OLDEST: AtomicU64 = AtomicU64::new(1);
static DROPPED: AtomicU64 = AtomicU64::new(0);
/// By host list, the stamp it holds inside (set when it closes).
static MARKS: Mutex<BTreeMap<usize, u64>> = Mutex::new(BTreeMap::new());

/// A stretch of one kind of work: from stamp, to stamp.
type Seg = (u64, u64, u8);

struct Pending {
    fence: u64,
    first: u64,
    segs: Vec<Seg>,
}

static PENDING: Mutex<VecDeque<Pending>> = Mutex::new(VecDeque::new());

#[derive(Default)]
struct Sums {
    ticks: [u64; KINDS],
    idle: u64,
    segs: u64,
    submissions: u64,
    /// The last stamp of the last submission read, for the idle time.
    last_end: u64,
}

static SUMS: Mutex<Sums> = Mutex::new(Sums { ticks: [0; KINDS], idle: 0, segs: 0, submissions: 0, last_end: 0 });
/// Why the card's own steps were refused, and how often since the last line.
static REFUSED: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());

/// `card_steps` could not give a submission's reads to the card, for `why`.
/// Counted in every build; printed by [`line`].
pub(super) fn refused(why: &'static str) {
    *REFUSED.lock().unwrap_or_else(|e| e.into_inner()).entry(why).or_insert(0) += 1;
}

pub(super) fn ring() -> Option<&'static Ring> {
    RING_STATE
        .get_or_init(|| {
            let host = host()?;
            match host.make_ring() {
                Ok(ring) => Some(ring),
                Err(e) => {
                    tracing::warn!(error = %e, "VICTORY_GPU_SPLIT/VICTORY_GPU_PER_PROGRAM: no timestamps, they are off");
                    None
                }
            }
        })
        .as_ref()
}

impl Host {
    fn make_ring(&self) -> windows::core::Result<Ring> {
        let Some((buffer, back)) = self.shared_buffer(8 * TOTAL as u64) else {
            return Err(windows::core::Error::from(windows::Win32::Foundation::E_OUTOFMEMORY));
        };
        // SAFETY: plain object creation; a fresh mapping of that size.
        unsafe {
            std::ptr::write_bytes(back, 0, 8 * TOTAL);
            let mut heap: Option<ID3D12QueryHeap> = None;
            self.device.CreateQueryHeap(
                &D3D12_QUERY_HEAP_DESC { Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP, Count: TOTAL as u32, NodeMask: 0 },
                &mut heap,
            )?;
            let heap = heap.ok_or_else(windows::core::Error::empty)?;
            let allocator: ID3D12CommandAllocator = self.device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;
            let mut lists = Vec::with_capacity(RING);
            for at in 0..RING as u32 {
                let list: ID3D12GraphicsCommandList =
                    self.device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)?;
                list.EndQuery(&heap, D3D12_QUERY_TYPE_TIMESTAMP, at);
                list.ResolveQueryData(&heap, D3D12_QUERY_TYPE_TIMESTAMP, at, 1, &buffer, u64::from(at) * 8);
                list.Close()?;
                lists.push(list.cast()?);
            }
            Ok(Ring { lists, heap, buffer, back: back as usize, _allocator: allocator })
        }
    }
}

/// The next stamp's number, its slot cleared; `None` when the ring has no
/// room (stamps not yet read) and the stretch goes unmeasured.
fn claim() -> Option<u64> {
    let ring = ring()?;
    let c = NEXT.fetch_add(1, Ordering::Relaxed);
    if c.saturating_sub(OLDEST.load(Ordering::Relaxed)) + 64 >= RING as u64 {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    // SAFETY: a slot of the mapping; the card writes it only when the list
    // that stamps it has run.
    unsafe { std::ptr::write_volatile((ring.back as *mut u64).add(c as usize % RING), 0) };
    Some(c)
}

fn read(c: u64) -> u64 {
    let Some(ring) = ring() else {
        return 0;
    };
    // SAFETY: a slot of the mapping.
    unsafe { std::ptr::read_volatile((ring.back as *const u64).add(c as usize % RING)) }
}

/// In `HostList::close`: a stamp inside the list, where the title's work
/// ends and ours begins.
pub(super) fn mark(key: usize, list: &ID3D12GraphicsCommandList) {
    if !on() {
        return;
    }
    let (Some(ring), Some(c)) = (ring(), claim()) else {
        return;
    };
    let slot = (c as usize % RING) as u32;
    // SAFETY: an open list; our heap and buffer.
    unsafe {
        list.EndQuery(&ring.heap, D3D12_QUERY_TYPE_TIMESTAMP, slot);
        list.ResolveQueryData(&ring.heap, D3D12_QUERY_TYPE_TIMESTAMP, slot, 1, &ring.buffer, u64::from(slot) * 8);
    }
    MARKS.lock().unwrap_or_else(|e| e.into_inner()).insert(key, c);
}

fn take_mark(key: usize) -> Option<u64> {
    MARKS.lock().unwrap_or_else(|e| e.into_inner()).remove(&key)
}

enum Event {
    Lists(usize, usize),
    Seg(u8, Option<u64>),
}

/// Before a submission is sent, with its lists as `submit` made them (the
/// runs, and the compare last if `check`): puts stamp lists between the
/// stretches of one kind, and notes which stamps are whose, to be read
/// behind the submission's fence value `fence`. `firsts` (where each run's
/// lists start) is kept right.
pub(super) fn instrument(
    runs: &[&HostList],
    own: bool,
    check: bool,
    lists: &mut Vec<Option<ID3D12CommandList>>,
    firsts: &mut [usize],
    fence: u64,
) {
    if !on() || runs.is_empty() || firsts.len() != runs.len() {
        return;
    }
    let Some(ring) = ring() else {
        return;
    };
    let Some(begin) = claim() else {
        return;
    };
    let body_end = lists.len() - usize::from(check);
    let mut events = Vec::with_capacity(4 * runs.len() + 2);
    // (The run's own prelude is the list ahead of the list, if it has one.)
    for (k, run) in runs.iter().enumerate() {
        let start = firsts[k];
        let end = firsts.get(k + 1).copied().unwrap_or(body_end);
        if own && k == 0 {
            events.push(Event::Lists(start, end));
            events.push(Event::Seg(FILL, None));
            continue;
        }
        let mark = take_mark(std::ptr::from_ref::<HostList>(run) as usize);
        if end - start >= 2 {
            events.push(Event::Lists(start, start + 1));
            events.push(Event::Seg(FILL, None));
            events.push(Event::Lists(start + 1, end));
        } else {
            events.push(Event::Lists(start, end));
        }
        match mark {
            Some(c) => {
                events.push(Event::Seg(TITLE, Some(c)));
                events.push(Event::Seg(TAIL, None));
            }
            None => events.push(Event::Seg(TITLE, None)),
        }
    }
    if check {
        events.push(Event::Lists(body_end, body_end + 1));
        events.push(Event::Seg(HASH, None));
    }
    let old = std::mem::take(lists);
    let old_firsts = firsts.to_vec();
    let mut out: Vec<Option<ID3D12CommandList>> = Vec::with_capacity(old.len() + 2 * runs.len() + 4);
    out.push(Some(ring.lists[begin as usize % RING].clone()));
    let mut segs: Vec<Seg> = Vec::new();
    let mut from = Some(begin);
    let mut next_run = 0;
    for (i, event) in events.iter().enumerate() {
        match *event {
            Event::Lists(a, b) => {
                while next_run < runs.len() && old_firsts[next_run] == a {
                    firsts[next_run] = out.len();
                    next_run += 1;
                }
                out.extend_from_slice(&old[a..b]);
            }
            Event::Seg(kind, end) => {
                let to = match end {
                    Some(c) => Some(c),
                    None => {
                        // The next stretch of the same kind carries on from here.
                        let merged = matches!(events.get(i + 1), Some(&Event::Seg(k, _)) if k == kind);
                        if merged {
                            continue;
                        }
                        claim().inspect(|&c| out.push(Some(ring.lists[c as usize % RING].clone())))
                    }
                };
                if let (Some(f), Some(t)) = (from, to) {
                    segs.push((f, t, kind));
                }
                from = to;
            }
        }
    }
    // A run whose lists were not named (none are empty, but keep it right).
    while next_run < runs.len() {
        firsts[next_run] = out.len();
        next_run += 1;
    }
    *lists = out;
    let first = segs.iter().map(|s| s.0.min(s.1)).min().unwrap_or(begin).min(begin);
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).push_back(Pending { fence, first, segs });
}

/// Takes in the stamps of the submissions the card has finished.
pub(super) fn drain(host: &Host) {
    if !on() {
        return;
    }
    // SAFETY: a plain query.
    let done = unsafe { host.fence.GetCompletedValue() };
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    let mut sums = SUMS.lock().unwrap_or_else(|e| e.into_inner());
    while pending.front().is_some_and(|p| p.fence <= done) {
        let Some(p) = pending.pop_front() else {
            break;
        };
        let mut first_from = 0;
        let mut last_to = 0;
        for &(from, to, kind) in &p.segs {
            let (a, b) = (read(from), read(to));
            if a == 0 || b < a {
                continue;
            }
            sums.ticks[kind as usize] += b - a;
            sums.segs += 1;
            if first_from == 0 {
                first_from = a;
            }
            last_to = last_to.max(b);
        }
        if first_from != 0 {
            sums.submissions += 1;
            if sums.last_end != 0 && first_from > sums.last_end {
                sums.idle += first_from - sums.last_end;
            }
            sums.last_end = sums.last_end.max(last_to);
        }
    }
    OLDEST.store(pending.front().map_or(NEXT.load(Ordering::Relaxed), |p| p.first), Ordering::Relaxed);
}

/// With the once-a-second line of the frame loop: per frame, the card's
/// time by kind and the refusals of the card's own steps.
pub(super) fn line(frames: f64) {
    let refusals = std::mem::take(&mut *REFUSED.lock().unwrap_or_else(|e| e.into_inner()));
    if !refusals.is_empty() {
        let text: Vec<String> = refusals.iter().map(|(why, n)| format!("{n} x {why}")).collect();
        tracing::info!("card steps refused, in the last second: {}", text.join("; "));
    }
    if !on() {
        return;
    }
    let Some(host) = host() else {
        return;
    };
    drain(host);
    tracing::info!("{}", render(host, frames));
}

/// The line: the sums since the last, per frame.
fn render(host: &Host, frames: f64) -> String {
    // SAFETY: a plain query.
    let frequency = unsafe { host.queue.GetTimestampFrequency() }.unwrap_or(1).max(1) as f64;
    let mut guard = SUMS.lock().unwrap_or_else(|e| e.into_inner());
    let last_end = guard.last_end;
    let sums = std::mem::take(&mut *guard);
    guard.last_end = last_end;
    drop(guard);
    let ms = |ticks: u64| ticks as f64 * 1e3 / frequency / frames.max(1.0);
    let busy: u64 = sums.ticks.iter().sum();
    format!(
        "gpu split per frame: title {:.1} ms, tail {:.1} (ours at the end of title lists), hash_cmp {:.1}, fill_copy {:.1} (steps on the card, preludes), \
         idle {:.1} (nothing queued); busy {:.1} ms, busy+idle {:.1} ms; {:.0} stretches in {:.1} submissions; {} stamps dropped, ring full",
        ms(sums.ticks[TITLE as usize]),
        ms(sums.ticks[TAIL as usize]),
        ms(sums.ticks[HASH as usize]),
        ms(sums.ticks[FILL as usize]),
        ms(sums.idle),
        ms(busy),
        ms(busy + sums.idle),
        sums.segs as f64 / frames.max(1.0),
        sums.submissions as f64 / frames.max(1.0),
        DROPPED.swap(0, Ordering::Relaxed),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stamps around a copy are read back behind the fence without a wait
    /// of the split's own, and come out in the line.
    #[test]
    fn stamps_read_back_behind_the_fence() {
        let Some(host) = host() else {
            return;
        };
        // SAFETY: before anything in this process reads the switch.
        unsafe { std::env::set_var("VICTORY_GPU_SPLIT", "1") };
        if !on() {
            return;
        }
        let Some(ring) = ring() else {
            return;
        };
        let (a, b, c) = (claim().unwrap(), claim().unwrap(), claim().unwrap());
        let fence = {
            let mut submitted = host.submitted.lock().unwrap_or_else(|e| e.into_inner());
            *submitted += 1;
            let list = |c: u64| Some(ring.lists[c as usize % RING].clone());
            // SAFETY: closed lists; the fence and event are the host's, under its lock.
            unsafe {
                host.queue.ExecuteCommandLists(&[list(a), list(b), list(c)]);
                host.queue.Signal(&host.fence, *submitted).unwrap();
                host.fence.SetEventOnCompletion(*submitted, host.event).unwrap();
                WaitForSingleObject(host.event, INFINITE);
            }
            *submitted
        };
        PENDING.lock().unwrap().push_back(Pending { fence, first: a, segs: vec![(a, b, TITLE), (b, c, HASH)] });
        drain(host);
        assert!(PENDING.lock().unwrap().is_empty(), "read behind the fence");
        assert!(read(a) != 0 && read(b) >= read(a) && read(c) >= read(b), "{} {} {}", read(a), read(b), read(c));
        refused("a step of ours runs");
        let text = render(host, 1.0);
        println!("{text}");
        assert!(text.contains("gpu split per frame: title "));
        assert!(text.contains("1.0 submissions"), "{text}");
    }
}