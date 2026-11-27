//! Where the card's time goes, by shader program (`VICTORY_GPU_PER_PROGRAM=1`,
//! off by default, in release builds too).
//!
//! A timestamp is written inside the open title list wherever the bound
//! pipeline changes (`draw.rs` `bind`): consecutive draws and dispatches of
//! one program form one segment, from its stamp to the next one (or to the
//! stamp `HostList::close` writes before the card's own tail work). The
//! segment is named by the program: a compute program by its shader hash, a
//! graphics one by its pixel and vertex shader hashes (the hashes of the
//! translation cache, as logged at first bind).
//!
//! Nothing waits: a submission's segments are read once the host fence has
//! passed its signal (`GetCompletedValue`), by the next submission or the
//! once-a-second line (`line`), which prints the 20 programs that cost most
//! per frame, and a sum of the ray-tracing ones (the structure builder's
//! stages, and shaders with `image_bvh_intersect_ray`).
//!
//! The stamps live in slots `split::RING..split::TOTAL` of the split's query
//! heap. A stamp is written at the point of the pipeline change, and the
//! card writes it when it has run what came before, so a segment's time is
//! "from the end of the work ahead to the end of its own": work that
//! overlaps on the card is charged to the program whose stamp comes last.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::pipeline::host::{HostPipeline, Stage};

const SLOTS: usize = split::TOTAL - split::RING;
/// Programs listed each second.
const TOP: usize = 20;

/// Whether `VICTORY_GPU_PER_PROGRAM` is on.
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::switch::on("VICTORY_GPU_PER_PROGRAM"))
}

static NEXT: AtomicU64 = AtomicU64::new(1);
/// No stamp older than this is still to be read.
static OLDEST: AtomicU64 = AtomicU64::new(1);
static DROPPED: AtomicU64 = AtomicU64::new(0);

fn slot(c: u64) -> usize {
    split::RING + c as usize % SLOTS
}

/// The next stamp's number, its slot cleared; `None` when the ring has no
/// room (stamps not yet read) and the stretch goes unmeasured.
fn claim() -> Option<u64> {
    let ring = split::ring()?;
    let c = NEXT.fetch_add(1, Ordering::Relaxed);
    if c.saturating_sub(OLDEST.load(Ordering::Relaxed)) + 64 >= SLOTS as u64 {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    // SAFETY: a slot of the mapping; the card writes it only when the stamp
    // in the list has run.
    unsafe { std::ptr::write_volatile((ring.back as *mut u64).add(slot(c)), 0) };
    Some(c)
}

fn read(c: u64) -> u64 {
    let Some(ring) = split::ring() else {
        return 0;
    };
    // SAFETY: a slot of the mapping.
    unsafe { std::ptr::read_volatile((ring.back as *const u64).add(slot(c))) }
}

/// Writes a stamp into `list`; its number.
fn stamp(list: &ID3D12GraphicsCommandList) -> Option<u64> {
    let ring = split::ring()?;
    let c = claim()?;
    let s = slot(c) as u32;
    // SAFETY: an open list; our heap and buffer.
    unsafe {
        list.EndQuery(&ring.heap, D3D12_QUERY_TYPE_TIMESTAMP, s);
        list.ResolveQueryData(&ring.heap, D3D12_QUERY_TYPE_TIMESTAMP, s, 1, &ring.buffer, u64::from(s) * 8);
    }
    Some(c)
}

/// What the line says about a program.
struct Prog {
    label: String,
    gfx: bool,
    /// The structure builder's (`XboxBvhBuilder.dll`) stage.
    builder: bool,
    /// Its code has `image_bvh_intersect_ray`.
    intersect: bool,
}

static PROGS: Mutex<Option<(HashMap<String, u32>, Vec<Prog>)>> = Mutex::new(None);

fn short(stage: &str) -> &str {
    match stage {
        "vertex" => "vs",
        "pixel" => "ps",
        "mesh" => "ms",
        "domain" => "ds",
        "hull" => "hs",
        "geometry" => "gs",
        "compute" => "cs",
        other => other,
    }
}

fn program(p: &HostPipeline, builder: bool) -> u32 {
    let mut order: Vec<&Stage> = p.stages.iter().collect();
    // Pixel first, then vertex, then any other.
    order.sort_by_key(|s| match s.shader.stage {
        "pixel" => 0,
        "vertex" => 1,
        _ => 2,
    });
    let label = order
        .iter()
        .map(|s| if p.compute { s.shader.hash.clone() } else { format!("{}={}", short(s.shader.stage), s.shader.hash) })
        .collect::<Vec<_>>()
        .join(" ");
    let mut guard = PROGS.lock().unwrap_or_else(|e| e.into_inner());
    let (ids, progs) = guard.get_or_insert_with(|| (HashMap::new(), Vec::new()));
    if let Some(&id) = ids.get(&label) {
        return id;
    }
    let id = progs.len() as u32;
    ids.insert(label.clone(), id);
    progs.push(Prog {
        label,
        gfx: !p.compute,
        builder,
        intersect: p.stages.iter().any(|s| s.translated().uses_bvh_intersect),
    });
    id
}

/// A stretch of one program: from stamp, to stamp, with its work.
#[derive(Clone, Copy)]
struct Seg {
    from: u64,
    to: u64,
    prog: u32,
    work: u32,
}

struct Open {
    from: u64,
    prog: u32,
    work: u32,
}

/// By host list: the segment being recorded, and the ones closed so far.
static OPEN: Mutex<Option<HashMap<usize, Open>>> = Mutex::new(None);
static CLOSED: Mutex<Option<HashMap<usize, Vec<Seg>>>> = Mutex::new(None);

/// In `bind`, where the pipeline of `list` changes to `p`: closes the
/// segment of the program before and starts one for `p`.
pub(crate) fn switch(list: &HostList, p: &HostPipeline, builder: bool) {
    let id = program(p, builder);
    let key = std::ptr::from_ref::<HostList>(list) as usize;
    let mut open = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    let open = open.get_or_insert_with(HashMap::new);
    if open.get(&key).is_some_and(|o| o.prog == id) {
        return;
    }
    let Some(c) = stamp(&list.list) else {
        open.remove(&key);
        return;
    };
    if let Some(o) = open.insert(key, Open { from: c, prog: id, work: 0 }) {
        CLOSED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert_with(HashMap::new)
            .entry(key)
            .or_default()
            .push(Seg { from: o.from, to: c, prog: o.prog, work: o.work });
    }
}

/// A draw or dispatch was recorded in `list`.
pub(crate) fn work(list: &HostList) {
    let key = std::ptr::from_ref::<HostList>(list) as usize;
    if let Some(o) = OPEN.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|m| m.get_mut(&key)) {
        o.work += 1;
    }
}

/// In `HostList::close`: the title's last segment ends here, ahead of the
/// work of ours at the end of the list.
pub(super) fn end(list: &HostList) {
    let key = std::ptr::from_ref::<HostList>(list) as usize;
    let Some(o) = OPEN.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|m| m.remove(&key)) else {
        return;
    };
    let Some(c) = stamp(&list.list) else {
        return;
    };
    CLOSED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .entry(key)
        .or_default()
        .push(Seg { from: o.from, to: c, prog: o.prog, work: o.work });
}

struct Pending {
    fence: u64,
    first: u64,
    segs: Vec<Seg>,
}

static PENDING: Mutex<VecDeque<Pending>> = Mutex::new(VecDeque::new());

#[derive(Default, Clone, Copy)]
struct Sum {
    ticks: u64,
    segs: u64,
    work: u64,
}

static SUMS: Mutex<Option<HashMap<u32, Sum>>> = Mutex::new(None);

/// Before a submission is sent: takes the segments of its lists, to be read
/// behind its fence value `fence`.
pub(super) fn attach(runs: &[&HostList], fence: u64) {
    let mut segs = Vec::new();
    if let Some(closed) = CLOSED.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        for run in runs {
            if let Some(mut s) = closed.remove(&(std::ptr::from_ref::<HostList>(run) as usize)) {
                segs.append(&mut s);
            }
        }
    }
    if segs.is_empty() {
        return;
    }
    let first = segs.iter().map(|s| s.from.min(s.to)).min().unwrap_or(1);
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).push_back(Pending { fence, first, segs });
}

/// Takes in the stamps of the submissions the card has finished.
pub(super) fn drain(host: &Host) {
    // SAFETY: a plain query.
    let done = unsafe { host.fence.GetCompletedValue() };
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    let mut sums = SUMS.lock().unwrap_or_else(|e| e.into_inner());
    let sums = sums.get_or_insert_with(HashMap::new);
    while pending.front().is_some_and(|p| p.fence <= done) {
        let Some(p) = pending.pop_front() else {
            break;
        };
        for s in &p.segs {
            let (a, b) = (read(s.from), read(s.to));
            if a == 0 || b < a {
                continue;
            }
            let e = sums.entry(s.prog).or_default();
            e.ticks += b - a;
            e.segs += 1;
            e.work += u64::from(s.work);
        }
    }
    OLDEST.store(pending.front().map_or(NEXT.load(Ordering::Relaxed), |p| p.first), Ordering::Relaxed);
}

/// With the once-a-second line of the frame loop: the most expensive
/// programs per frame.
pub(super) fn line(frames: f64) {
    let Some(host) = host() else {
        return;
    };
    drain(host);
    // SAFETY: a plain query.
    let frequency = unsafe { host.queue.GetTimestampFrequency() }.unwrap_or(1).max(1) as f64;
    let frames = frames.max(1.0);
    let sums = SUMS.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default();
    let ms = |ticks: u64| ticks as f64 * 1e3 / frequency / frames;
    let total: u64 = sums.values().map(|s| s.ticks).sum();
    let mut all: Vec<(u32, Sum)> = sums.into_iter().collect();
    all.sort_by_key(|(_, s)| std::cmp::Reverse(s.ticks));
    let guard = PROGS.lock().unwrap_or_else(|e| e.into_inner());
    let Some((_, progs)) = guard.as_ref() else {
        return;
    };
    let pct = |ticks: u64| if total == 0 { 0.0 } else { 100.0 * ticks as f64 / total as f64 };
    let segs: u64 = all.iter().map(|(_, s)| s.segs).sum();
    tracing::info!(
        "gpu per program: {:.1} ms/frame in {} programs, {:.0} segments/frame; {} stamps dropped, ring full; top {TOP} follow (hash, kind, ms/frame, segments/frame, draws or dispatches/frame, % of the total)",
        ms(total),
        all.len(),
        segs as f64 / frames,
        DROPPED.swap(0, Ordering::Relaxed),
    );
    for (rank, (id, s)) in all.iter().take(TOP).enumerate() {
        let p = &progs[*id as usize];
        let rt = match (p.builder, p.intersect) {
            (true, true) => " rt=builder+intersect",
            (true, false) => " rt=builder",
            (false, true) => " rt=intersect",
            (false, false) => "",
        };
        tracing::info!(
            "gpu per program #{:<2} {} {:<3} {:7.3} ms/frame {:6.1} segs/frame {:7.1} {}/frame {:5.1}%{rt}",
            rank + 1,
            p.label,
            if p.gfx { "gfx" } else { "cs" },
            ms(s.ticks),
            s.segs as f64 / frames,
            s.work as f64 / frames,
            if p.gfx { "draws" } else { "dispatches" },
            pct(s.ticks),
        );
    }
    let (mut ticks, mut n, mut work, mut b_ticks, mut i_ticks) = (0u64, 0usize, 0u64, 0u64, 0u64);
    for (id, s) in &all {
        let p = &progs[*id as usize];
        if p.builder || p.intersect {
            ticks += s.ticks;
            n += 1;
            work += s.work;
            if p.builder {
                b_ticks += s.ticks;
            }
            if p.intersect {
                i_ticks += s.ticks;
            }
        }
    }
    tracing::info!(
        "gpu per program ray-tracing related: {:.2} ms/frame, {:.1}% of the total, {} programs, {:.1} work/frame (structure builder {:.2} ms, intersect shaders {:.2} ms; a program in both is in both)",
        ms(ticks),
        pct(ticks),
        n,
        work as f64 / frames,
        ms(b_ticks),
        ms(i_ticks),
    );
}