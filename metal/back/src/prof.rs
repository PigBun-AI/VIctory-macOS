//! GPU time by what ran (`VICTORY_METAL_GPUPROF=<N>`; off by default, and then free: one
//! atomic read per command).
//!
//! The question it answers: which dispatch pipeline, copy or render pass takes the card's time.
//! The commit feedback (`GPUStartTime`/`GPUEndTime`, [`crate::queue`]) says only how long a whole
//! submission took.
//!
//! How: every command list gets one Metal 4 timestamp counter heap. The executor writes a
//! timestamp into the open encoder after each compute-side op (a dispatch, a copy, a fill, a
//! clear), after the barriers that open an encoder (the wait for earlier work shows as `gap`), and
//! at the end of a render encoder (one scope per pass). The time of the scope that ended at
//! sample `i` is `ts[i] - ts[i-1]`, so the scopes of a list add up to the time between its first
//! and last sample. After the card is done with a list the heap is read
//! (`MTL4CounterHeap.resolveCounterRange:`), the scopes are summed by label, and every `N`
//! presents the top 20 by time per frame is printed to stderr, together with the single slowest
//! scopes of the window. A scope that alone took more than a second is printed at once.
//!
//! Sources (Xcode 27.2 SDK, MacOSX 27.2, checked 2026-11-28):
//! * `MTL4Counters.h`: `MTL4CounterHeapDescriptor` (`type` =
//!   `MTL4CounterHeapTypeTimestamp`, `count`), `MTL4CounterHeap.resolveCounterRange:` (CPU read of
//!   `MTL4TimestampHeapEntry { uint64_t timestamp }` entries), `MTL4TimestampGranularity`
//!   (`Relaxed`: may sample at encoder boundaries only; `Precise`: may split encoders).
//! * `MTL4ComputeCommandEncoder.h`: `writeTimestampWithGranularity:intoHeap:atIndex:`.
//! * `MTL4RenderCommandEncoder.h`: `writeTimestampWithGranularity:afterStage:intoHeap:atIndex:`.
//! * `MTLDevice.h`: `newCounterHeapWithDescriptor:error:`, `queryTimestampFrequency` (ticks per
//!   second of the heap's timestamps).
//!
//! Windows whose scopes add up to less than `VICTORY_METAL_GPUPROF_MIN_MS` (default 50) per frame
//! are not printed (loading screens, menus).
//!
//! Precise timestamps order the work around them (the header says encoders may be split), so the
//! numbers are those of a run with a barrier after every op: the *shares* are right, the sum is
//! more than an unprofiled frame takes. `VICTORY_METAL_GPUPROF_RELAXED=1` asks for relaxed
//! timestamps: less disturbance, but ops of one encoder then show with their encoder's time at
//! the first and 0 at the rest.
//! `VICTORY_METAL_GPUPROF_DRAWS=1` also samples inside a render pass whenever the pipeline
//! changes (the pass then splits into one scope per pipeline, and Metal may split the encoder).
//!
//! Pipeline cold compiles: [`compiled`] logs every pipeline creation (MSC and Metal compiler)
//! that took more than 100 ms, whether or not the profiler is on
//! (`VICTORY_METAL_COMPILE_LOG_MS` changes the limit; 0 logs all).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSRange;
use objc2_metal::{
    MTL4CounterHeap, MTL4CounterHeapDescriptor, MTL4CounterHeapType, MTL4TimestampGranularity,
    MTLDevice,
};

use crate::queue::Timeline;

/// Entries of one counter heap. Measured 2026-11-28 (M5 Max, macOS 27.2): the device gives about
/// a dozen counter heaps of any size ("AGX: MTLCounterHeap: Invalid heap size requested." after
/// that, whatever the count), so a few big heaps are cut into slices, one per list.
const HEAP_ENTRIES: usize = 4096;
/// Heaps of the pool.
const HEAPS: usize = 12;
/// Samples one list can take; more ops in one list are not attributed (counted as overflow).
const DEFAULT_SLICE: usize = 512;

/// Samples a list can take: `VICTORY_METAL_GPUPROF_SLICE` (64..=4096, rounded down to a power of
/// two), default 512. With 4096 only 12 lists are sampled at a time and the recorder waits for
/// the card when the 13th starts: right for a frame the card is the bottleneck of.
fn slice() -> usize {
    static S: OnceLock<usize> = OnceLock::new();
    *S.get_or_init(|| {
        let v: usize = std::env::var("VICTORY_METAL_GPUPROF_SLICE")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(DEFAULT_SLICE)
            .clamp(64, HEAP_ENTRIES);
        1 << v.ilog2()
    })
}

/// How many rows of the table are printed.
const TOP: usize = 20;

/// A scope this long (nanoseconds) is printed when it is read.
const SLOW_NS: u64 = 1_000_000_000;

pub(crate) struct Config {
    /// Presents between two tables.
    frames: u64,
    pub(crate) relaxed: bool,
    pub(crate) draws: bool,
    /// `VICTORY_METAL_GPUPROF_SECS`: also a table every this many seconds (a replay presents
    /// nothing).
    secs: Option<u64>,
}

/// What `VICTORY_METAL_GPUPROF` asked for; `None` = off.
pub(crate) fn config() -> Option<&'static Config> {
    static CONFIG: OnceLock<Option<Config>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let frames: u64 = std::env::var("VICTORY_METAL_GPUPROF")
                .ok()?
                .trim()
                .parse()
                .ok()?;
            let on = |name: &str| std::env::var_os(name).is_some_and(|v| v != "0");
            (frames > 0).then(|| Config {
                frames,
                relaxed: on("VICTORY_METAL_GPUPROF_RELAXED"),
                draws: on("VICTORY_METAL_GPUPROF_DRAWS"),
                secs: std::env::var("VICTORY_METAL_GPUPROF_SECS")
                    .ok()
                    .and_then(|v| v.trim().parse().ok()),
            })
        })
        .as_ref()
}

impl Config {
    pub(crate) fn granularity(&self) -> MTL4TimestampGranularity {
        if self.relaxed {
            MTL4TimestampGranularity::Relaxed
        } else {
            MTL4TimestampGranularity::Precise
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The counter heaps, cut into slices; a list holds one until it is read.
struct Pool {
    heaps: Vec<Retained<ProtocolObject<dyn MTL4CounterHeap>>>,
    free: Vec<(usize, usize)>,
    /// Lists that ran without samples because every slice was out.
    skipped: u64,
}

// Metal heaps are thread-safe objects.
unsafe impl Send for Pool {}

fn pool(device: &ProtocolObject<dyn MTLDevice>) -> &'static Mutex<Option<Pool>> {
    static P: OnceLock<Mutex<Option<Pool>>> = OnceLock::new();
    let p = P.get_or_init(|| Mutex::new(None));
    let mut g = lock(p);
    if g.is_none() {
        let mut heaps = Vec::new();
        for _ in 0..HEAPS {
            let desc = MTL4CounterHeapDescriptor::new();
            desc.setType(MTL4CounterHeapType::Timestamp);
            // SAFETY: a plain count.
            unsafe { desc.setCount(HEAP_ENTRIES) };
            match device.newCounterHeapWithDescriptor_error(&desc) {
                Ok(h) => heaps.push(h),
                Err(_) => break,
            }
        }
        let free = (0..heaps.len())
            .flat_map(|h| (0..HEAP_ENTRIES / slice()).map(move |i| (h, i * slice())))
            .rev()
            .collect();
        *g = Some(Pool {
            heaps,
            free,
            skipped: 0,
        });
    }
    drop(g);
    p
}

/// The samples of one command list.
pub(crate) struct ListProf {
    heap: Retained<ProtocolObject<dyn MTL4CounterHeap>>,
    index: usize,
    base: usize,
    /// `labels[i]` names the scope that ended at sample `i`.
    labels: Vec<String>,
    /// Draws seen since the last render sample (for the pass label).
    pub(crate) draws: u32,
    overflow: u32,
}

// The heap is a Metal object (thread-safe); the rest is plain data.
unsafe impl Send for ListProf {}

impl ListProf {
    pub(crate) fn new(device: &ProtocolObject<dyn MTLDevice>) -> Option<ListProf> {
        config()?;
        let p = pool(device);
        let _ = POOL_REF.set(p);
        let started = Instant::now();
        let (index, base) = loop {
            {
                let mut g = lock(p);
                let pool = g.as_mut()?;
                if let Some(slice) = pool.free.pop() {
                    break slice;
                }
            }
            // Every slice is with a list the card has not finished (or one still being
            // recorded): wait for the card, so that the table has every list. Nothing submitted:
            // nothing to wait for.
            let mut s = lock(state());
            resolve_ready(&mut s);
            if s.pending.is_empty() || started.elapsed().as_secs() >= 60 {
                drop(s);
                if let Some(pool) = lock(p).as_mut() {
                    pool.skipped += 1;
                }
                return None;
            }
            drop(s);
            std::thread::sleep(std::time::Duration::from_micros(500));
        };
        let pool_heap = lock(p).as_ref()?.heaps[index].clone();
        Some(ListProf {
            heap: pool_heap,
            index,
            base,
            labels: Vec::new(),
            draws: 0,
            overflow: 0,
        })
    }

    pub(crate) fn heap(&self) -> &ProtocolObject<dyn MTL4CounterHeap> {
        &self.heap
    }

    /// The heap index the next sample goes to, which will carry `label`; `None` when the list's
    /// slice is full.
    pub(crate) fn next(&mut self, label: String) -> Option<usize> {
        if self.labels.len() >= slice() {
            self.overflow += 1;
            return None;
        }
        self.labels.push(label);
        Some(self.base + self.labels.len() - 1)
    }
}

impl Drop for ListProf {
    fn drop(&mut self) {
        // The slice goes back to the pool (a list read, or one abandoned).
        if let Some(p) = POOL_REF.get()
            && let Some(pool) = lock(p).as_mut()
        {
            pool.free.push((self.index, self.base));
        }
    }
}

static POOL_REF: OnceLock<&'static Mutex<Option<Pool>>> = OnceLock::new();

/// A list the card may still be running.
struct Pending {
    timeline: Arc<Timeline>,
    value: u64,
    prof: ListProf,
    freq: u64,
    queued: Instant,
}

#[derive(Default, Clone, Copy)]
struct Row {
    ns: u64,
    count: u64,
    max_ns: u64,
    /// Geometry-shader indirect draws with a snapshot (`VICTORY_METAL_GPUPROF_ARGS`): how many,
    /// and the sums of the draw's count, instances, mesh threadgroups (x*y*z), the largest
    /// threadgroup count.
    gs: [u64; 5],
}

#[derive(Default)]
struct Window {
    rows: HashMap<String, Row>,
    lists: u64,
    scoped_ns: u64,
    overflow: u64,
    /// Samples that were earlier than the one before (not counted).
    unordered: u64,
    /// Samples the card never wrote (read as zero).
    unwritten: u64,
    frames: u64,
    first_frame: u64,
}

struct State {
    pending: Vec<Pending>,
    window: Window,
    frame: u64,
    /// When a table was last printed.
    last_print: Option<Instant>,
    /// Device frequency, set when a list is queued.
    freq: u64,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(State {
            pending: Vec::new(),
            window: Window::default(),
            frame: 0,
            last_print: None,
            freq: 0,
        })
    })
}

/// Pipeline labels by the proto id: set at creation, read when an op names its pipeline.
fn labels() -> &'static Mutex<HashMap<u32, String>> {
    static L: OnceLock<Mutex<HashMap<u32, String>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn register_pipeline(pso: u32, label: String) {
    if config().is_none() && !crate::fault::enabled() {
        return;
    }
    lock(labels()).insert(pso, label);
}

pub(crate) fn pipeline_label(pso: u32) -> String {
    lock(labels())
        .get(&pso)
        .cloned()
        .unwrap_or_else(|| format!("pso#{pso}"))
}

/// A list was committed with timeline `value`: its heap is read when the card is done with it.
pub(crate) fn queue_list(
    device: &ProtocolObject<dyn MTLDevice>,
    timeline: &Arc<Timeline>,
    value: u64,
    prof: ListProf,
) {
    let mut s = lock(state());
    if s.freq == 0 {
        s.freq = device.queryTimestampFrequency().max(1);
    }
    let freq = s.freq;
    s.pending.push(Pending {
        timeline: timeline.clone(),
        value,
        prof,
        freq,
        queued: Instant::now(),
    });
    if let Some(secs) = config().and_then(|c| c.secs) {
        let last = *s.last_print.get_or_insert_with(Instant::now);
        if last.elapsed().as_secs() >= secs {
            resolve_ready(&mut s);
            s.last_print = Some(Instant::now());
            let mut w = std::mem::take(&mut s.window);
            w.frames = w.frames.max(1);
            let pending = s.pending.len();
            drop(s);
            eprintln!("{}", table(&w, pending));
        }
    }
}

/// Reads the lists the card has finished.
fn resolve_ready(s: &mut State) {
    let mut i = 0;
    while i < s.pending.len() {
        if s.pending[i].timeline.completed() < s.pending[i].value {
            i += 1;
            continue;
        }
        let p = s.pending.swap_remove(i);
        absorb(&mut s.window, &p);
    }
}

fn absorb(w: &mut Window, p: &Pending) {
    let n = p.prof.labels.len();
    w.lists += 1;
    w.overflow += u64::from(p.prof.overflow);
    if n == 0 {
        return;
    }
    // SAFETY: `n` samples were written to this heap, and the card is done with the list.
    let Some(data) = (unsafe {
        p.prof
            .heap
            .resolveCounterRange(NSRange::new(p.prof.base, n))
    }) else {
        return;
    };
    let bytes = data.to_vec();
    // The slice is used again: its entries must not keep these values (an entry a later list
    // does not write would read as a time of this one).
    // SAFETY: the same range, read above.
    unsafe {
        p.prof
            .heap
            .invalidateCounterRange(NSRange::new(p.prof.base, n))
    };

    let stamps: Vec<u64> = bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| u64::from_le_bytes(*c))
        .collect();
    let ns_of = |ticks: u64| (ticks as u128 * 1_000_000_000 / p.freq as u128) as u64;
    let mut slow = Vec::new();
    // A sample the card did not write reads as zero, or as what an earlier list left in the
    // slice (older than this list's first sample, which is always written). The time up to the
    // next valid sample goes to the scopes since the last valid one, shared equally.
    let first = stamps.first().copied().unwrap_or(0);
    let mut prev = if first == 0 { None } else { Some(0usize) };
    let mut gaps = Vec::new();
    for i in 1..stamps.len().min(n) {
        let Some(from) = prev else { break };
        if stamps[i] == 0 || stamps[i] < first {
            w.unwritten += 1;
            continue;
        }
        if stamps[i] < stamps[from] {
            w.unordered += 1;
            continue;
        }
        gaps.push((from, i));
        prev = Some(i);
    }
    for (from, i) in gaps {
        let ns = ns_of(stamps[i] - stamps[from]);
        let share = (i - from) as u64;
        for k in from + 1..=i {
            let part = ns / share;
            let row = w
                .rows
                .entry(split_snap(&p.prof.labels[k]).0.to_string())
                .or_default();
            row.ns += part;
            row.count += 1;
            row.max_ns = row.max_ns.max(part);
            if let (Some(i), Some(sn)) = (
                split_snap(&p.prof.labels[k]).1,
                SNAP.get().and_then(|s| s.as_ref()),
            ) && split_snap(&p.prof.labels[k])
                .0
                .starts_with("execute-indirect graphics-gs")
            {
                let a = sn.read(i);
                let b = sn.read((i + 1) % SNAP_ENTRIES as u32);
                if a.len() >= 3 && b.len() >= 3 {
                    let groups = u64::from(b[0]) * u64::from(b[1]) * u64::from(b[2]);
                    row.gs[0] += 1;
                    row.gs[1] += u64::from(a[0]);
                    row.gs[2] += u64::from(a[1]);
                    row.gs[3] += groups;
                    row.gs[4] = row.gs[4].max(groups);
                }
            }
            // `VICTORY_METAL_GPUPROF_ARGS`: indirect compute dispatches of 60,000 groups or more in x,
            // once per distinct label and count (the clamp in victory-gpu keeps 65,535 on a card it
            // does not know; Metal itself takes any 32-bit count).
            if let (Some(i), Some(sn)) = (
                split_snap(&p.prof.labels[k]).1,
                SNAP.get().and_then(|s| s.as_ref()),
            ) && !split_snap(&p.prof.labels[k]).0.contains("graphics-gs")
            {
                let a = sn.read(i);
                if a.len() >= 3
                    && a[0]
                        >= std::env::var("VICTORY_METAL_GPUPROF_ARGS_MINX")
                            .ok()
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(60_000)
                {
                    static SEEN: Mutex<Vec<(String, u32)>> = Mutex::new(Vec::new());
                    let head = split_snap(&p.prof.labels[k]).0.to_string();
                    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
                    if seen.len() < 200 && !seen.contains(&(head.clone(), a[0])) {
                        eprintln!(
                            "victory-metal gpuprof: indirect dispatch of {} x {} x {} groups reached the back: {head}",
                            a[0], a[1], a[2]
                        );
                        seen.push((head, a[0]));
                    }
                }
            }
            w.scoped_ns += part;
            if part >= SLOW_NS {
                slow.push((part, &p.prof.labels[k]));
            }
        }
    }
    // `VICTORY_METAL_GPUPROF_RAW=<n>`: the raw samples of the first n lists with a slow scope.
    static RAW_LEFT: AtomicU64 = AtomicU64::new(u64::MAX);
    if !slow.is_empty() || std::env::var_os("VICTORY_METAL_GPUPROF_RAW_ALL").is_some() {
        let mut left = RAW_LEFT.load(Ordering::Relaxed);
        if left == u64::MAX {
            left = std::env::var("VICTORY_METAL_GPUPROF_RAW")
                .ok()
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
        }
        if left > 0 {
            RAW_LEFT.store(left - 1, Ordering::Relaxed);
            let mut out = format!("victory-metal gpuprof: raw list, freq {} ticks/s:", p.freq);
            for (i, (stamp, label)) in stamps.iter().zip(&p.prof.labels).enumerate() {
                out.push_str(&format!("\n  [{i}] {stamp} {label}"));
            }
            eprintln!("{out}");
        } else {
            RAW_LEFT.store(0, Ordering::Relaxed);
        }
    }
    if vis_enabled()
        && p.prof
            .labels
            .iter()
            .any(|l| l.contains("execute-indirect graphics-gs"))
        && let Some(n) = vis_last()
    {
        eprintln!("victory-metal gpuprof: visibility samples of the last counted GS draw: {n}");
        eprintln!("victory-metal gpuprof: that draw read:{}", dump_text());
    }
    for (ns, label) in slow {
        let (head, snapped) = split_snap(label);
        let args = match (snapped, SNAP.get().and_then(|s| s.as_ref())) {
            (Some(i), Some(sn)) if head.contains("graphics-gs") => format!(
                " GS draw words (count, instances, start) {:?}, mesh threadgroups {:?}",
                sn.read(i),
                sn.read((i + 1) % SNAP_ENTRIES as u32)
            ),
            (Some(i), Some(sn)) => {
                format!(" dispatch arguments (now in the ring) {:?}", sn.read(i))
            }
            _ => String::new(),
        };
        let args = match vis_last().filter(|_| vis_enabled()) {
            Some(n) => format!("{args} [visibility samples {n}]"),
            None => args,
        };
        eprintln!(
            "victory-metal gpuprof: a scope took {:.1} ms on the card: {head}{args} (list of {} scopes, queued {:.1} s ago)",
            ns as f64 / 1e6,
            n,
            p.queued.elapsed().as_secs_f64()
        );
    }
}

/// A present: the frame ends. Every `N` frames the table is printed.
pub(crate) fn end_frame() {
    let Some(cfg) = config() else { return };
    let mut s = lock(state());
    s.frame += 1;
    resolve_ready(&mut s);
    if s.window.frames == 0 {
        s.window.first_frame = s.frame;
    }
    s.window.frames += 1;
    if s.window.frames < cfg.frames {
        return;
    }
    s.last_print = Some(Instant::now());
    let w = std::mem::take(&mut s.window);
    let pending = s.pending.len();
    drop(s);
    // Windows of loading screens and menus (a few ms of GPU time per frame) are not worth a table.
    let min_ms: f64 = std::env::var("VICTORY_METAL_GPUPROF_MIN_MS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(50.0);
    if w.scoped_ns as f64 / 1e6 / w.frames.max(1) as f64 >= min_ms {
        eprintln!("{}", table(&w, pending));
    }
}

/// The table of what has been read so far, and the counts start again (tests, and a final report
/// at the end of a run). Empty when the profiler is off.
pub fn flush_table() -> String {
    if config().is_none() {
        return String::new();
    }
    let mut s = lock(state());
    resolve_ready(&mut s);
    let mut w = std::mem::take(&mut s.window);
    w.frames = w.frames.max(1);
    let pending = s.pending.len();
    drop(s);
    table(&w, pending)
}

fn notes(w: &Window) -> String {
    let skipped = POOL_REF
        .get()
        .and_then(|p| lock(p).as_ref().map(|p| p.skipped))
        .unwrap_or(0);
    let mut note = String::new();
    if w.overflow > 0 {
        note += &format!(
            ", {} scopes beyond a list's {} samples not counted",
            w.overflow,
            slice()
        );
    }
    if w.unwritten > 0 {
        note += &format!(", {} samples never written", w.unwritten);
    }
    if w.unordered > 0 {
        note += &format!(", {} samples out of order not counted", w.unordered);
    }
    if skipped > 0 {
        note += &format!(", {skipped} lists so far ran unsampled (no free slice)");
    }
    note
}

fn table(w: &Window, pending: usize) -> String {
    let frames = w.frames.max(1) as f64;
    let mut rows: Vec<(&String, &Row)> = w.rows.iter().collect();
    rows.sort_by_key(|(_, r)| std::cmp::Reverse(r.ns));
    let mut out = format!(
        "victory-metal gpuprof: presents {}..{} ({} frames), {} lists read, {} still on the card, {:.1} ms/frame in scopes{}; top {TOP} by GPU time per frame:",
        w.first_frame,
        w.first_frame + w.frames - 1,
        w.frames,
        w.lists,
        pending,
        w.scoped_ns as f64 / 1e6 / frames,
        notes(w),
    );
    for (i, (label, r)) in rows.iter().take(TOP).enumerate() {
        out.push_str(&format!(
            "\n  #{:<2} {:>9.1} ms/frame  {:>5.1}%  n={:<6.1}/frame  max {:>9.1} ms  avg {:>8.2} ms  {label}",
            i + 1,
            r.ns as f64 / 1e6 / frames,
            100.0 * r.ns as f64 / w.scoped_ns.max(1) as f64,
            r.count as f64 / frames,
            r.max_ns as f64 / 1e6,
            r.ns as f64 / 1e6 / r.count.max(1) as f64,
        ));
        if r.gs[0] > 0 {
            let n = r.gs[0] as f64;
            out.push_str(&format!(
                "\n        GS draws with args: {:.1}/frame; per draw avg count {:.0}, instances {:.1}, mesh threadgroups {:.0} (max {})",
                n / frames,
                r.gs[1] as f64 / n,
                r.gs[2] as f64 / n,
                r.gs[3] as f64 / n,
                r.gs[4]
            ));
        }
    }
    if vis_enabled()
        && let Some(n) = vis_last()
    {
        out.push_str(&format!(
            "\n  visibility samples of the last counted GS draw: {n}"
        ));
    }
    out
}

static COMPILE_LIMIT_MS: AtomicU64 = AtomicU64::new(u64::MAX);

/// A pipeline was made (`what`: "cs", "vs", "ms"; `hash`: the pipeline's FNV of its DXIL, as
/// `back-pso.csv` names it); `total`: the whole call, `metal`: the part in the Metal compiler
/// (a pipeline found in an archive takes well under a millisecond).
pub(crate) fn compiled(
    what: &str,
    hash: u64,
    total: std::time::Duration,
    metal: std::time::Duration,
) {
    let mut limit = COMPILE_LIMIT_MS.load(Ordering::Relaxed);
    if limit == u64::MAX {
        limit = std::env::var("VICTORY_METAL_COMPILE_LOG_MS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(100);
        COMPILE_LIMIT_MS.store(limit, Ordering::Relaxed);
    }
    if total.as_millis() as u64 >= limit {
        eprintln!(
            "victory-metal compile: {what} {hash:016x} took {:.0} ms ({:.0} ms of it in the Metal compiler, the rest the shader converter)",
            total.as_secs_f64() * 1e3,
            metal.as_secs_f64() * 1e3,
        );
    }
}

/// What an indirect dispatch's arguments held when it ran (`snap#<n>` on the scope's label): a
/// tiny kernel copies the three words, in the encoder just ahead of the dispatch, to a ring the
/// CPU reads when the scope turns out to be slow.
const SNAP_ENTRIES: u64 = 4096;
const SNAP_SOURCE: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
kernel void snap(constant ulong2& p [[buffer(30)]]) {\n\
  device const uint* s = (device const uint*)p.x; device uint* d = (device uint*)p.y;\n\
  d[0] = s[0]; d[1] = s[1]; d[2] = s[2]; d[3] = 0x5a5a5a5au;\n}\n";

pub(crate) struct Snap {
    pub(crate) ring: crate::memory::Buffer,
    pub(crate) pipeline: crate::pipeline::ComputePipeline,
    next: AtomicU64,
}

static SNAP: OnceLock<Option<Snap>> = OnceLock::new();

pub(crate) fn snap(device: &crate::device::Device) -> Option<&'static Snap> {
    SNAP.get_or_init(|| {
        let ring = device
            .create_buffer(crate::memory::BufferKind::Shared, SNAP_ENTRIES * 16)
            .ok()?;
        let pipeline = device.compute_pipeline_from_msl(SNAP_SOURCE, "snap").ok()?;
        Some(Snap {
            ring,
            pipeline,
            next: AtomicU64::new(0),
        })
    })
    .as_ref()
}

impl Snap {
    /// The next entry: its number and the address the kernel writes to.
    pub(crate) fn take(&self) -> (u32, u64) {
        let n = self.next.fetch_add(1, Ordering::Relaxed) % SNAP_ENTRIES;
        (n as u32, self.ring.gpu_va() + n * 16)
    }

    /// The words entry `n` holds now.
    pub(crate) fn read(&self, n: u32) -> Vec<u32> {
        self.ring
            .read_u32s(u64::from(n) * 16, 4)
            .unwrap_or_default()
    }
}

/// `label` without its ` snap#<n>` tail, and the entry number.
fn split_snap(label: &str) -> (&str, Option<u32>) {
    match label.rsplit_once(" snap#") {
        Some((head, n)) => (head, n.parse().ok()),
        None => (label, None),
    }
}

// `VICTORY_METAL_GS_GROUP_CAP=<hash>:<n>` (a diagnostic): the geometry-shader indirect draws of
// that pipeline run at most `n` mesh threadgroups (the first n instances), to tell whether their
// cost is per pixel or has a knee. The executor sets the cap for the draw it is about to record.
thread_local! {
    pub(crate) static GS_CAP: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}

const CAP_SOURCE: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
kernel void capk(constant ulong4& p [[buffer(30)]]) {\n\
  device uint* d = (device uint*)p.x; uint cap = (uint)p.y; uint cnt = (uint)(p.y >> 32);\n\
  for (int i = 5; i < 8; i++) if (d[i] > cap) d[i] = cap;\n\
  if (cnt != 0) d[0] = cnt;\n\
  if (p.z != 0) d[10] = (d[10] & 0xffffu) | (((uint)p.z & 0xffffu) << 16);\n}\n";

pub(crate) fn cap_kernel(
    device: &crate::device::Device,
) -> Option<&'static crate::pipeline::ComputePipeline> {
    static K: OnceLock<Option<crate::pipeline::ComputePipeline>> = OnceLock::new();
    K.get_or_init(|| device.compute_pipeline_from_msl(CAP_SOURCE, "capk").ok())
        .as_ref()
}

// `VICTORY_METAL_NO_RASTER=<hash>[,..]` (a diagnostic): the geometry-shader pipelines with these
// label hashes are made with rasterization off (no fragment work), to tell whether their cost is
// in the object and mesh stages or in the pixels. The executor sets the flag while it makes one.
thread_local! {
    pub(crate) static NO_RASTER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// `VICTORY_METAL_NO_PS=<hash>[,..]`: the same pipelines without their fragment function,
    /// rasterization on (blending and the colour writes stay): the pixel shader's own cost.
    pub(crate) static NO_PS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

// `VICTORY_METAL_GS_VISIBILITY=<hash>[,..]` (a diagnostic): counts the samples the geometry
// pipelines with these label hashes rasterize (Metal's visibility result, counting mode) and
// prints the last count with the GPU time table. A pixel drawn once per sample: the draw's
// coverage; a draw of tiles with more samples than the target has pixels overdraws.
thread_local! {
    pub(crate) static GS_VIS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn vis_enabled() -> bool {
    static E: OnceLock<bool> = OnceLock::new();
    *E.get_or_init(|| {
        std::env::var_os("VICTORY_METAL_GS_VISIBILITY").is_some_and(|v| !v.is_empty())
    })
}

static VIS: OnceLock<Option<crate::memory::Buffer>> = OnceLock::new();

/// The buffer the render passes write visibility results to (one 8-byte slot).
pub(crate) fn vis_buffer(device: &crate::device::Device) -> Option<&'static crate::memory::Buffer> {
    VIS.get_or_init(|| {
        device
            .create_buffer(crate::memory::BufferKind::Shared, 4096)
            .ok()
    })
    .as_ref()
}

/// The last visibility count, if a pass wrote one.
pub(crate) fn vis_last() -> Option<u64> {
    let w = VIS.get()?.as_ref()?.read_u32s(0, 2).ok()?;
    Some(u64::from(w[0]) | (u64::from(w[1]) << 32))
}

// `VICTORY_METAL_GS_VISIBILITY` also dumps what the counted draw reads: the first 64 bytes of each
// bound vertex buffer and the first 32 of the index buffer (a kernel copies them in the list), with
// the viewports and scissors of the state, printed with the count.
const DUMP_SOURCE: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
kernel void dumpk(constant ulong4& p [[buffer(30)]]) {\n\
  device const uint* s = (device const uint*)p.x; device uint* d = (device uint*)p.y;\n\
  uint n = (uint)p.z; for (uint i = 0; i < n; i++) d[i] = s[i];\n}\n";

pub(crate) fn dump_kernel(
    device: &crate::device::Device,
) -> Option<&'static crate::pipeline::ComputePipeline> {
    static K: OnceLock<Option<crate::pipeline::ComputePipeline>> = OnceLock::new();
    K.get_or_init(|| device.compute_pipeline_from_msl(DUMP_SOURCE, "dumpk").ok())
        .as_ref()
}

static DUMP: OnceLock<Option<crate::memory::Buffer>> = OnceLock::new();

pub(crate) fn dump_buffer(
    device: &crate::device::Device,
) -> Option<&'static crate::memory::Buffer> {
    DUMP.get_or_init(|| {
        device
            .create_buffer(crate::memory::BufferKind::Shared, 4096)
            .ok()
    })
    .as_ref()
}

pub(crate) static LAST_ARGS_VA: AtomicU64 = AtomicU64::new(0);
pub(crate) static LAST_HEAP_VA: AtomicU64 = AtomicU64::new(0);

const DUMP2_SOURCE: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
kernel void dump2k(constant ulong4& p [[buffer(30)]]) {\n\
  device const uint* a = (device const uint*)p.x; device uint* d = (device uint*)p.z;\n\
  for (uint i = 0; i < 12; i++) d[i] = a[i];\n\
  uint base = a[9];\n\
  for (uint k = 0; k < 2; k++) {\n\
    uint idx = ((base + (k == 0 ? 224u : 32u)) >> 5) & 0x1fffffu;\n\
    device const ulong* e = (device const ulong*)(p.y + (ulong)idx * 24);\n\
    device const uint* s = (device const uint*)e[0];\n\
    d[16 + k * 16] = (uint)(e[0]); d[17 + k * 16] = (uint)(e[0] >> 32); d[18 + k * 16] = (uint)e[2]; d[19 + k * 16] = idx;\n\
    for (uint i = 0; i < 12; i++) d[20 + k * 16 + i] = s[i];\n\
  }\n}\n";

pub(crate) fn dump2_kernel(
    device: &crate::device::Device,
) -> Option<&'static crate::pipeline::ComputePipeline> {
    static K: OnceLock<Option<crate::pipeline::ComputePipeline>> = OnceLock::new();
    K.get_or_init(|| {
        device
            .compute_pipeline_from_msl(DUMP2_SOURCE, "dump2k")
            .ok()
    })
    .as_ref()
}

pub(crate) static DUMP_META: Mutex<String> = Mutex::new(String::new());

/// The dump as text: the state's facts, then each region's words (floats when they look like
/// floats).
pub(crate) fn dump_text() -> String {
    let Some(Some(b)) = DUMP.get() else {
        return String::new();
    };
    let meta = lock(&DUMP_META).clone();
    let mut out = meta;
    for k in 0..8u64 {
        let words = b.read_u32s(k * 64, 16).unwrap_or_default();
        let f: Vec<String> = words
            .iter()
            .map(|w| format!("{:.3}", f32::from_bits(*w)))
            .collect();
        out.push_str(&format!(
            "\n  region {k}: hex {:x?}\n            f32 {}",
            words,
            f.join(" ")
        ));
    }
    out
}
