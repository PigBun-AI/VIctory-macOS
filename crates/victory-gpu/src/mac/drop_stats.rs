//! Where the draws and dispatches go (docs/mac/PATCHES.md, patch G8): a
//! runtime switch, `VICTORY_MAC_DROP_STATS=1` (release builds too; it does not
//! need the `diagnostics` feature), that says per present which of what the
//! title recorded did not run on the host and why, and what the host did
//! with the pipelines it was given.
//!
//! Nothing here changes what the title or the host sees; with the switch off
//! every hook is one relaxed load. The Windows line's own counters
//! (`dropped N without a host pipeline`, `N indirect`) lump many causes; this
//! splits them:
//!
//! * a draw or dispatch the shared layer dropped, by operation (draw, indexed,
//!   mesh dispatch, dispatch, indirect), pipeline kind (`vs`, `vs+gs`,
//!   `tess`, `mesh`, `compute`), the title-creation method of the pipeline
//!   (`graphics@0x50`, `compute@0x58`, `compute@0x1f0`, `stream@0x2b0`) and
//!   the reason. Reasons that the upstream `make::missing()` calls "being
//!   made" are told apart here: `compiling` (requested, not finished within
//!   the frame's wait budget), `never_requested:left_out` (the ray-tracing
//!   resolve and activate programs, left out without a BVH), `failed:...`
//!   (a shader that did not translate, or the host refused the pipeline, with
//!   the host's own reason); `rt_builder_off`, `null_pipeline`, `skip_list`,
//!   `kind_mismatch`, `bind_or_index_failed`; and for ExecuteIndirect the
//!   reason of the first `return false` (`ind:args_outside_buffers`, ...);
//! * the calls the title made to the two unimplemented pipeline creation
//!   slots 0x1c8 and 0x218 (`device::unknown_pipeline`), per present and in
//!   all: those pipelines never exist, so no draw with them is ever recorded
//!   and no counter above can see them;
//! * every host pipeline created (a sequence number, its kind, whether it
//!   carries the rectangle geometry stage, the host object's address, the
//!   shaders) and how many draws and dispatches ran with it (a table
//!   rewritten every 5 s to `VICTORY_MAC_DROP_STATS_PSO=<path>` if set).
//!   D3DMetal prints `marking PSO(n) as no-op` for pipelines it failed to
//!   compile after our creation call succeeded; `tools/mac/drop-stats-report.sh`
//!   matches those to this table, since the shared layer counts such draws as
//!   drawn.
//!
//! One log line per present (`drop-stats present=...`), and every 5 s a
//! summary of the largest causes (`drop-stats top ...`), once for the whole
//! run and once for the world only (from [`super::world::world_frame`] 1).

#![cfg_attr(not(windows), allow(dead_code))]

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

// Each site registers its process-lifetime counter only on the first hit.
// Repeated hits use a relaxed increment, without locking or formatting.
struct SilentSite(&'static str, &'static AtomicU64);
static SILENT: Mutex<Vec<SilentSite>> = Mutex::new(Vec::new());
pub(crate) fn register_silent(name: &'static str, counter: &'static AtomicU64) {
    SILENT.lock().unwrap_or_else(|e| e.into_inner()).push(SilentSite(name, counter));
}
macro_rules! silent {
    ($site:literal, $reason:literal) => {{
        static HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        if HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0 {
            $crate::mac::drop_stats::register_silent(concat!($site, ":", $reason), &HITS);
        }
    }};
}
pub(crate) use silent;

#[cfg(test)]
pub(crate) fn silent_hits(name: &str) -> u64 {
    SILENT.lock().unwrap_or_else(|e| e.into_inner()).iter()
        .filter(|s| s.0 == name).map(|s| s.1.load(Relaxed)).sum()
}

#[cfg(test)]
#[test]
fn repeated_silent_hits_register_once() {
    for _ in 0..3 { silent!("test.repeated", "reason"); }
    let sites = SILENT.lock().unwrap_or_else(|e| e.into_inner());
    let mut hits = sites.iter().filter(|site| site.0 == "test.repeated:reason");
    assert_eq!(hits.next().unwrap().1.load(Relaxed), 3);
    assert!(hits.next().is_none());
}

thread_local! {
    static BIND_WHY: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}
pub(crate) fn bind_reason(why: Option<&'static str>) {
    if enabled() { BIND_WHY.set(why); }
}

thread_local! {
    static CBV_BIND: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Attribute only synchronous slot creation in this bind, including on early return.
/// Never inherit an unrelated PSO from the last draw on a recording thread.
pub(crate) struct CbvScope(usize);
impl Drop for CbvScope {
    fn drop(&mut self) { CBV_BIND.set(self.0); }
}
pub(crate) fn cbv_bind(object: usize) -> Option<CbvScope> {
    enabled().then(|| CbvScope(CBV_BIND.replace(object)))
}
pub(crate) fn cbv_table(slot: usize, address: u64, size: u32, root: u8, index: u32) {
    with(|s| {
        let seq = s.by_object.get(&CBV_BIND.get()).map_or(0, |i| i + 1);
        if s.cbv_slots.len() >= 65536 && !s.cbv_slots.contains_key(&slot) { s.cbv_slots.clear(); }
        s.cbv_slots.insert(slot, (address, size, seq, root, index));
    });
}
pub(crate) fn cbv_materialized(slot: usize, address: u64, size: u32, gpu: u64, accepted: bool) {
    with(|s| cbv_count(s, slot, address, size, gpu, accepted));
}
fn cbv_count(s: &mut State, slot: usize, address: u64, size: u32, gpu: u64, accepted: bool) {
    *s.cbv_residues.entry((address as u8, gpu as u8, accepted)).or_default() += 1;
    let (seq, how, root, index) = match s.cbv_slots.get(&slot) {
        Some(&(a, z, seq, root, index)) if (a, z) == (address, size) => (seq, "last_named_table", root, index),
        _ => (s.by_object.get(&CBV_BIND.get()).map_or(0, |i| i + 1), "bind_context_only", u8::MAX, u32::MAX),
    };
    s.cbv_psos.entry((seq, accepted, how)).or_insert((0, slot, root, index)).0 += 1;
}

#[test]
fn cbv_residues_and_last_slot_bind_do_not_confuse_a_rewritten_view() {
    let mut s = State::default();
    s.cbv_slots.insert(123, (0x1050, 32, 7, 2, 41));
    cbv_count(&mut s, 123, 0x1050, 32, 0x2050, true);
    cbv_count(&mut s, 123, 0x1100, 32, 0x2004, false);
    cbv_count(&mut s, 123, 0x1050, 16, 0x2004, false);
    assert_eq!(s.cbv_residues[&(0x50, 0x50, true)], 1);
    assert_eq!(s.cbv_residues[&(0, 4, false)], 1);
    assert_eq!(s.cbv_psos[&(7, true, "last_named_table")], (1, 123, 2, 41));
    assert_eq!(s.cbv_psos[&(0, false, "bind_context_only")].0, 2);
}

/// What the title asked the host to do.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum Op {
    Draw,
    Indexed,
    Mesh,
    Dispatch,
    Indirect,
}

impl Op {
    fn name(self) -> &'static str {
        match self {
            Op::Draw => "draw",
            Op::Indexed => "indexed",
            Op::Mesh => "mesh",
            Op::Dispatch => "dispatch",
            Op::Indirect => "indirect",
        }
    }
}

/// Whether the switch is on (`VICTORY_MAC_DROP_STATS`: anything but empty, 0,
/// off or false). Read once.
pub fn enabled() -> bool {
    #[cfg(test)]
    if FORCED.load(Relaxed) {
        return true;
    }
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("VICTORY_MAC_DROP_STATS")
            .is_ok_and(|v| !matches!(v.trim(), "" | "0" | "off" | "false"))
    })
}

/// `VICTORY_MAC_DROP_STATS=2`: the per-present line also lists the host
/// pipelines that ran (for matching D3DMetal's no-op pipelines to draws).
pub fn verbose() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| std::env::var("VICTORY_MAC_DROP_STATS").is_ok_and(|v| v.trim() == "2"))
}

/// Tests switch the statistics on for a moment.
#[cfg(test)]
static FORCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// One cause: what, with which kind of pipeline made by which method, why.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Key {
    pub op: Op,
    pub kind: &'static str,
    pub origin: &'static str,
    pub why: String,
}

/// The first word of a reason: `failed:shader_translation(...)` is `failed`.
fn class(why: &str) -> &str {
    why.split([':', '(']).next().unwrap_or(why)
}

/// Counts over a span of presents.
#[derive(Default, Clone)]
struct Counts {
    draws: u64,
    dispatches: u64,
    ran: u64,
    drops: HashMap<Key, u64>,
    notimpl: [u64; 2],
    mesh_x: std::collections::BTreeMap<(u64, bool, bool), u64>,
}

impl Counts {
    fn add(&mut self, o: &Counts) {
        self.draws += o.draws;
        self.dispatches += o.dispatches;
        self.ran += o.ran;
        self.notimpl[0] += o.notimpl[0];
        self.notimpl[1] += o.notimpl[1];
        for (k, n) in &o.drops {
            *self.drops.entry(k.clone()).or_default() += n;
        }
    }

    fn dropped(&self) -> u64 {
        self.drops.values().sum()
    }

    /// `class=n ...`, largest first.
    fn by_class(&self) -> String {
        let mut by: HashMap<&str, u64> = HashMap::new();
        for (k, n) in &self.drops {
            *by.entry(class(&k.why)).or_default() += n;
        }
        ranked(by)
    }

    /// `kind=n ...`, largest first.
    fn by_kind(&self) -> String {
        let mut by: HashMap<&str, u64> = HashMap::new();
        for (k, n) in &self.drops {
            *by.entry(k.kind).or_default() += n;
        }
        ranked(by)
    }
}

fn ranked(by: HashMap<&str, u64>) -> String {
    let mut v: Vec<(&str, u64)> = by.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    if v.is_empty() {
        return "-".to_owned();
    }
    v.iter().map(|(k, n)| format!("{k}={n}")).collect::<Vec<_>>().join(" ")
}

/// A host pipeline the shared layer made.
struct Pso {
    /// Order among the graphics and mesh pipelines (0 for compute): what
    /// D3DMetal's `PSO(n)` is compared to.
    gseq: usize,
    kind: &'static str,
    rects: bool,
    key: String,
    shaders: String,
    present: u64,
    ran: u64,
    ran_world: u64,
}

#[derive(Default)]
struct State {
    // G15: five-second deltas. Keep one example CPU heap slot per PSO,
    // not an unbounded list of dynamic slots or title addresses.
    cbv_residues: std::collections::BTreeMap<(u8, u8, bool), u64>,
    cbv_psos: std::collections::BTreeMap<(usize, bool, &'static str), (u64, usize, u8, u32)>,
    cbv_slots: HashMap<usize, (u64, u32, usize, u8, u32)>,
    uav_ranges: [u64; 3], // G13: full, strict subrange, unknown
    feature_results: std::collections::BTreeMap<u32, std::collections::BTreeMap<i32, u64>>,
    frame: Counts,
    total: Counts,
    world: Counts,
    /// One example (a shader hash, a reason text) per cause.
    examples: HashMap<Key, String>,
    /// Title pipeline (its address) -> the method that made it.
    origins: HashMap<usize, &'static str>,
    /// Host pipelines in creation order; `by_object` maps a host object's
    /// address to its index (an address reused later gets a new entry).
    psos: Vec<Pso>,
    by_object: HashMap<usize, usize>,
    /// Why the host refused a pipeline, by pipeline key.
    refused: HashMap<String, String>,
    ran_by_kind: HashMap<&'static str, u64>,
    /// Draws and dispatches that ran this present, by index in `psos` (kept
    /// with `VICTORY_MAC_DROP_STATS=2`).
    ran_frame: HashMap<usize, u64>,
    last_tick: Option<Instant>,
    presents: u64,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(State::default))
}

static NOTIMPL_TOTAL: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

thread_local! {
    /// The host refusal reason set by `pipeline::host` on this thread, read by
    /// `make` right after the creation returned None.
    static REFUSAL: RefCell<Option<String>> = const { RefCell::new(None) };
    /// The creation method named by the `create_*` function now running.
    static CREATING: RefCell<Option<&'static str>> = const { RefCell::new(None) };
    /// The reason an ExecuteIndirect gave up (`State::execute_indirect`'s `why`).
    static INDIRECT_WHY: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The title recorded a draw (every kind, ExecuteIndirect included) or a
/// compute dispatch: from `list::count_draw` / `count_dispatch`.
pub(crate) fn recorded(dispatch: bool) {
    if !enabled() {
        return;
    }
    with(|s| {
        if dispatch {
            s.frame.dispatches += 1;
        } else {
            s.frame.draws += 1;
        }
    });
}

/// Slot 98 has an argument destination and a payload pointer, but no count buffer.
pub(crate) fn mesh_x(flags: u64, dest: bool, payload: bool) {
    if enabled() { with(|s| *s.frame.mesh_x.entry((flags, dest, payload)).or_default() += 1); }
}

/// A draw or dispatch was dropped.
pub(crate) fn dropped(op: Op, kind: &'static str, origin: &'static str, why: String, example: String) {
    if !enabled() {
        return;
    }
    with(|s| {
        let key = Key { op, kind, origin, why };
        if !example.is_empty() {
            s.examples.entry(key.clone()).or_insert(example);
        }
        *s.frame.drops.entry(key).or_default() += 1;
    });
}

/// The title called slot 0x1c8 or 0x218 (`device::unknown_pipeline`).
pub(crate) fn notimpl(slot: usize) {
    NOTIMPL_TOTAL[usize::from(slot == 0x218)].fetch_add(1, Relaxed);
    if enabled() {
        with(|s| s.frame.notimpl[usize::from(slot == 0x218)] += 1);
    }
}

/// A title pipeline creation method (`origin`: its name and vtable offset)
/// starts; its `hand_out` calls [`created`].
pub(crate) fn creating(origin: &'static str) {
    if enabled() {
        CREATING.with(|c| *c.borrow_mut() = Some(origin));
    }
}

/// The title pipeline at `this` was made by the method last named by
/// [`creating`] on this thread.
pub(crate) fn created(this: usize) {
    if enabled() {
        let origin = CREATING.with(|c| c.borrow_mut().take()).unwrap_or("unknown");
        with(|s| {
            s.origins.insert(this, origin);
        });
    }
}

fn origin_of(s: &State, this: usize) -> &'static str {
    s.origins.get(&this).copied().unwrap_or("unknown")
}

/// A host pipeline `object` ran a draw or dispatch.
pub(crate) fn ran(object: usize) {
    if !enabled() {
        return;
    }
    let world = super::world::world_frame() > 0;
    with(|s| {
        s.frame.ran += 1;
        if let Some(&i) = s.by_object.get(&object) {
            let p = &mut s.psos[i];
            p.ran += 1;
            if world {
                p.ran_world += 1;
            }
            let kind = p.kind;
            *s.ran_by_kind.entry(kind).or_default() += 1;
            if verbose() {
                *s.ran_frame.entry(i).or_default() += 1;
            }
        }
    });
}

/// `pipeline::host` is about to return no host pipeline, for `reason`.
pub(crate) fn refusal(reason: String) {
    if enabled() {
        REFUSAL.with(|r| *r.borrow_mut() = Some(reason));
    }
}

/// The reason of the last [`refusal`] on this thread, taken.
fn take_refusal() -> Option<String> {
    REFUSAL.with(|r| r.borrow_mut().take())
}

/// A host pipeline was made (`object` non-zero), or the host gave none.
pub(crate) fn host_made(
    key: &str,
    shaders: &str,
    made: Option<(usize, &'static str, bool)>,
) {
    if !enabled() {
        return;
    }
    let refusal = take_refusal();
    with(|s| match made {
        Some((object, kind, rects)) => {
            let seq = s.psos.len() + 1;
            let gseq = if kind == "compute" { 0 } else { s.psos.iter().filter(|p| p.gseq != 0).count() + 1 };
            s.by_object.insert(object, s.psos.len());
            s.psos.push(Pso {
                gseq,
                kind,
                rects,
                key: key.chars().take(12).collect(),
                shaders: shaders.to_owned(),
                present: s.presents,
                ran: 0,
                ran_world: 0,
            });
            tracing::info!(
                seq,
                gseq,
                kind,
                rects,
                object = format_args!("{object:#x}"),
                key = &key[..key.len().min(12)],
                shaders,
                present = s.presents,
                "drop-stats pso"
            );
        }
        None => {
            let why = refusal.unwrap_or_else(|| "refused (no reason logged)".to_owned());
            tracing::info!(key = &key[..key.len().min(12)], shaders, why = why.as_str(), "drop-stats pso refused");
            s.refused.insert(key.to_owned(), why);
        }
    });
}

/// Starts an ExecuteIndirect: forgets the last reason.
pub(crate) fn indirect_begin() {
    if enabled() {
        INDIRECT_WHY.with(|r| *r.borrow_mut() = None);
    }
}

/// `State::execute_indirect` gives up, for `what`.
pub(crate) fn indirect_why(what: &str) {
    if enabled() {
        INDIRECT_WHY.with(|r| {
            let mut r = r.borrow_mut();
            if r.is_none() {
                *r = Some(what.to_owned());
            }
        });
    }
}

/// The reason set since [`indirect_begin`], taken.
pub(crate) fn indirect_reason() -> Option<String> {
    INDIRECT_WHY.with(|r| r.borrow_mut().take())
}

/// A reason as a key part: no spaces or commas.
fn token(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_whitespace() || c == ',' { '_' } else { c })
        .collect()
}

/// The title presents: from `queue::present`, after `world::on_present`.
pub(crate) fn on_present(present: u64) {
    if !enabled() {
        return;
    }
    let world = super::world::world_frame();
    with(|s| {
        s.presents = present;
        let frame = std::mem::take(&mut s.frame);
        for ((flags, dest, payload), calls) in &frame.mesh_x {
            tracing::info!("drop-stats mesh-x present={present} world_frame={world} flags={flags:#x} args_dest={dest} payload={payload} calls={calls}");
        }
        let kinds = frame.by_kind();
        let mut psoran = String::new();
        if verbose() {
            let mut v: Vec<(usize, u64)> =
                s.ran_frame.drain().filter_map(|(i, n)| Some((s.psos.get(i)?.gseq, n)).filter(|g| g.0 != 0)).collect();
            v.sort_unstable();
            psoran = v.iter().map(|(g, n)| format!("{g}:{n}")).collect::<Vec<_>>().join(" ");
        }
        tracing::info!(
            "drop-stats present={present} world_frame={world} recorded={}+{} ran={} dropped={} \
             notimpl[0x1c8={} 0x218={}] why[{}] kind[{}] psoran[{psoran}]",
            frame.draws,
            frame.dispatches,
            frame.ran,
            frame.dropped(),
            frame.notimpl[0],
            frame.notimpl[1],
            frame.by_class(),
            kinds
        );
        s.total.add(&frame);
        if world > 0 {
            s.world.add(&frame);
        }
        let now = Instant::now();
        let last = *s.last_tick.get_or_insert(now);
        if now.duration_since(last) >= Duration::from_secs(5) {
            s.last_tick = Some(now);
            summary(s, "all");
            for ((title_mod, gpu_mod, accepted), n) in std::mem::take(&mut s.cbv_residues) {
                tracing::info!(title_mod, gpu_mod, accepted, n, "drop-stats cbv residue delta");
            }
            for ((seq, accepted, how), (n, slot, root, index)) in std::mem::take(&mut s.cbv_psos) {
                let shaders = seq.checked_sub(1).and_then(|i| s.psos.get(i)).map_or("unknown", |p| &p.shaders);
                tracing::info!(seq, shaders, accepted, how, root, index, n, example_cpu_slot = format_args!("{slot:#x}"), "drop-stats cbv bind delta");
            }
            let mut silent = std::collections::BTreeMap::new();
            for SilentSite(name, counter) in SILENT.lock().unwrap_or_else(|e| e.into_inner()).iter() {
                *silent.entry(*name).or_insert(0u64) += counter.load(Relaxed);
            }
            for (name, hits) in silent {
                tracing::info!("drop-stats silent {name}={hits}");
            }
            if world > 0 {
                summary_world(s);
            }
            write_table(s);
        }
    });
}

fn summary_world(s: &State) {
    summary_of(s, "world", &s.world);
}

pub(crate) fn feature_answer(id: u32, hr: i32) {
    if enabled() {
        with(|s| *s.feature_results.entry(id).or_default().entry(hr).or_default() += 1);
    }
}

pub(crate) fn uav_range(index: usize) {
    with(|s| s.uav_ranges[index] += 1);
}

fn summary(s: &State, label: &str) {
    let [full, strict_subrange, unknown] = s.uav_ranges;
    if full + strict_subrange + unknown != 0 {
        tracing::info!("drop-stats uav3d layer=shared full={full} strict_subrange={strict_subrange} unknown={unknown}");
    }
    for (id, results) in &s.feature_results {
        let total: u64 = results.values().sum();
        for (&hr, n) in results {
            tracing::info!("drop-stats feature layer=shared id={id} total={total} hr={hr:#010x} count={n}");
        }
    }
    summary_of(s, label, &s.total);
}

fn summary_of(s: &State, label: &str, c: &Counts) {
    tracing::info!(
        "drop-stats summary span={label} recorded={}+{} ran={} dropped={} notimpl[0x1c8={} 0x218={}] \
         why[{}] kind[{}] ran_by_kind[{}] psos={}",
        c.draws,
        c.dispatches,
        c.ran,
        c.dropped(),
        NOTIMPL_TOTAL[0].load(Relaxed),
        NOTIMPL_TOTAL[1].load(Relaxed),
        c.by_class(),
        c.by_kind(),
        ranked(s.ran_by_kind.iter().map(|(k, n)| (*k, *n)).collect()),
        s.psos.len()
    );
    let mut top: Vec<(&Key, &u64)> = c.drops.iter().collect();
    top.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (k, n) in top.into_iter().take(20) {
        tracing::info!(
            "drop-stats top span={label} n={n} op={} kind={} origin={} why={} e.g. {}",
            k.op.name(),
            k.kind,
            k.origin,
            token(&k.why),
            s.examples.get(k).map_or("-", String::as_str)
        );
    }
}

/// Rewrites the per-host-pipeline table to `VICTORY_MAC_DROP_STATS_PSO`.
fn write_table(s: &State) {
    let Some(path) = std::env::var_os("VICTORY_MAC_DROP_STATS_PSO") else {
        return;
    };
    let mut out = String::from("seq\tgseq\tkind\trects\tkey\tcreated_present\tran\tran_world\tshaders\n");
    for (i, p) in s.psos.iter().enumerate() {
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            i + 1,
            p.gseq,
            p.kind,
            u8::from(p.rects),
            p.key,
            p.present,
            p.ran,
            p.ran_world,
            p.shaders
        );
    }
    if let Err(e) = std::fs::write(&path, out) {
        tracing::warn!(error = %e, "drop-stats: the pipeline table was not written");
    }
}

#[cfg(windows)]
pub(crate) use glue::{dispatch_dropped, draw_dropped, indirect_dropped, host_pipeline_made, ran_pipeline};

/// What the Windows-only pipeline code is asked: the state of a title
/// pipeline and its host pipeline.
#[cfg(windows)]
mod glue {
    use super::{Op, State, indirect_reason, origin_of, with};
    use crate::object::This;
    use crate::pipeline::host::HostPipeline;

    /// What kind of host work was being done.
    #[derive(Clone, Copy, PartialEq)]
    pub(crate) enum Wants {
        Graphics,
        Mesh,
        Compute,
    }

    /// Kind, creation method and reason for a drop with `pipeline` (the
    /// title's pipeline address, maybe 0) when the host work wanted `wants`.
    fn classify(s: &State, pipeline: usize, wants: Wants) -> (&'static str, &'static str, String, String) {
        if pipeline == 0 {
            return ("none", "none", "null_pipeline".into(), String::new());
        }
        let this = pipeline as This;
        let origin = origin_of(s, pipeline);
        let Some(view) = crate::pipeline::mac_view(this) else {
            return ("none", origin, "null_pipeline".into(), String::new());
        };
        let kind = view.kind();
        let hashes = view.hashes();
        #[cfg(feature = "diagnostics")]
        if crate::pipeline::is_skipped(this) {
            return (kind, origin, "skip_list".into(), hashes);
        }
        use crate::pipeline::MacState;
        let why = match view.state {
            MacState::LeftOut(reason) => return (kind, origin, "never_requested:left_out".into(), format!("{reason}; {hashes}")),
            MacState::Compiling => return (kind, origin, "compiling".into(), hashes),
            MacState::Failed => {
                if let Some(bad) = view.shaders.iter().find(|x| x.2) {
                    return (
                        kind,
                        origin,
                        format!("failed:shader_translation({} {})", bad.0, bad.1),
                        hashes,
                    );
                }
                let reason = s
                    .refused
                    .get(&view.key)
                    .cloned()
                    .unwrap_or_else(|| "host_refused(no reason logged)".to_owned());
                return (kind, origin, format!("failed:{}", super::token(&reason)), hashes);
            }
            MacState::Ready { compute, mesh } => {
                let right = match wants {
                    Wants::Compute => compute,
                    Wants::Mesh => !compute && mesh,
                    Wants::Graphics => !compute && !mesh,
                };
                if right { super::BIND_WHY.take().unwrap_or("bind_or_index_failed") } else { "kind_mismatch" }
            }
        };
        (kind, origin, why.into(), hashes)
    }

    /// A draw, indexed draw or mesh dispatch the shared layer did not run
    /// (`State::prepare_for` returned false).
    pub(crate) fn draw_dropped(op: Op, pipeline: usize) {
        if !super::enabled() {
            return;
        }
        let wants = if op == Op::Mesh { Wants::Mesh } else { Wants::Graphics };
        let (kind, origin, why, example) = with(|s| classify(s, pipeline, wants));
        super::dropped(op, kind, origin, why, example);
    }

    /// A dispatch not run; `why` if the call site knows (`rt_builder_off`,
    /// `skip_list`), else from the pipeline's state.
    pub(crate) fn dispatch_dropped(pipeline: usize, why: Option<&'static str>) {
        if !super::enabled() {
            return;
        }
        let (kind, origin, why, example) = with(|s| {
            let c = classify(s, pipeline, Wants::Compute);
            match why {
                Some(w) => (c.0, c.1, w.to_owned(), c.3),
                None => c,
            }
        });
        super::dropped(Op::Dispatch, kind, origin, why, example);
    }

    /// An ExecuteIndirect that did not run. `pipeline` is the list state's
    /// pipeline; `detail` the reason the caller knows of (the signature or
    /// its buffers), else the reason `State::execute_indirect` gave, else the
    /// pipeline's state (no host pipeline, builder off).
    pub(crate) fn indirect_dropped(pipeline: usize, indirect_kind: u32, detail: Option<String>) {
        if !super::enabled() {
            return;
        }
        let given = detail.or_else(indirect_reason);
        // PC numbering of the signature's argument: 0 draw, 1 indexed draw,
        // 2 dispatch, 10 mesh dispatch.
        let wants = match indirect_kind {
            2 => Wants::Compute,
            10 => Wants::Mesh,
            _ => Wants::Graphics,
        };
        let (kind, origin, why, example) = with(|s| {
            let c = classify(s, pipeline, wants);
            match given {
                Some(g) => (c.0, c.1, format!("ind:{}", super::token(&g)), c.3),
                None => {
                    let builder = pipeline != 0
                        && crate::pipeline::is_builder(pipeline as This)
                        && !crate::pipeline::builder_runs();
                    if builder {
                        (c.0, c.1, "ind:rt_builder_off".to_owned(), c.3)
                    } else {
                        (c.0, c.1, format!("ind:{}", c.2), c.3)
                    }
                }
            }
        });
        super::dropped(Op::Indirect, kind, origin, why, example);
    }

    /// A host pipeline was made (or not) for the pipelines with `key`.
    pub(crate) fn host_pipeline_made(key: &str, made: Option<&HostPipeline>) {
        if !super::enabled() {
            return;
        }
        let shaders = made.map_or(String::new(), |h| h.name.clone());
        let made = made.map(|h| {
            let kind = if h.compute {
                "compute"
            } else if h.mesh {
                "mesh"
            } else if h.rects {
                "vs+rect_gs"
            } else {
                "vs"
            };
            (windows::core::Interface::as_raw(&h.pso) as usize, kind, h.rects)
        });
        super::host_made(key, &shaders, made);
    }

    /// A draw or dispatch ran with `p`.
    pub(crate) fn ran_pipeline(p: &HostPipeline) {
        if super::enabled() {
            super::ran(windows::core::Interface::as_raw(&p.pso) as usize);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(op: Op, kind: &'static str, why: &str) -> Key {
        Key { op, kind, origin: "stream@0x2b0", why: why.to_owned() }
    }

    #[test]
    fn classes_are_the_first_word_of_a_reason() {
        assert_eq!(class("failed:shader_translation(compute 02384a60fda6f2d0)"), "failed");
        assert_eq!(class("never_requested:left_out"), "never_requested");
        assert_eq!(class("compiling"), "compiling");
        assert_eq!(class("ind:args_outside_buffers"), "ind");
    }

    #[test]
    fn counts_add_and_rank() {
        let mut a = Counts::default();
        *a.drops.entry(key(Op::Dispatch, "compute", "rt_builder_off")).or_default() += 3;
        *a.drops.entry(key(Op::Indirect, "compute", "never_requested:left_out")).or_default() += 5;
        let mut b = Counts::default();
        *b.drops.entry(key(Op::Dispatch, "compute", "rt_builder_off")).or_default() += 4;
        b.draws = 2;
        b.notimpl = [1, 2];
        a.add(&b);
        assert_eq!(a.dropped(), 12);
        assert_eq!(a.draws, 2);
        assert_eq!(a.notimpl, [1, 2]);
        assert_eq!(a.by_class(), "rt_builder_off=7 never_requested=5");
        assert_eq!(a.by_kind(), "compute=12");
        assert_eq!(Counts::default().by_class(), "-");
    }

    #[test]
    fn reasons_are_one_token() {
        assert_eq!(token("host mesh pipeline refused, 0x80004001"), "host_mesh_pipeline_refused__0x80004001");
    }

    /// Counts of one present reach the totals, the per-present line resets, and
    /// the creation method of a pipeline is remembered (other tests in this
    /// process may add counts of their own: only this test's keys are checked).
    #[test]
    fn a_present_moves_the_frame_into_the_totals() {
        FORCED.store(true, Relaxed);
        pipeline_created_for_test(0x1234, "stream@0x2b0");
        dropped(Op::Dispatch, "compute", "stream@0x2b0", "test_reason".into(), "hash".into());
        dropped(Op::Dispatch, "compute", "stream@0x2b0", "test_reason".into(), "hash".into());
        notimpl(0x1c8);
        notimpl(0x218);
        host_made("abcdef0123456789", "[vertex x]", Some((0xdead, "vs", false)));
        ran(0xdead);
        on_present(7);
        FORCED.store(false, Relaxed);
        with(|s| {
            let k = Key { op: Op::Dispatch, kind: "compute", origin: "stream@0x2b0", why: "test_reason".into() };
            assert_eq!(s.total.drops.get(&k), Some(&2));
            assert_eq!(s.examples.get(&k).map(String::as_str), Some("hash"));
            assert!(s.total.notimpl[0] >= 1 && s.total.notimpl[1] >= 1);
            assert_eq!(origin_of(s, 0x1234), "stream@0x2b0");
            let i = s.by_object[&0xdead];
            assert_eq!((s.psos[i].kind, s.psos[i].ran), ("vs", 1));
            assert!(!s.frame.drops.contains_key(&k), "the present emptied the frame");
        });
    }

    fn pipeline_created_for_test(this: usize, origin: &'static str) {
        creating(origin);
        created(this);
    }

    #[test]
    fn refusal_reasons_travel_by_thread() {
        // (Set directly: the switch is shared with the other tests.)
        REFUSAL.with(|r| *r.borrow_mut() = Some("no mesh".into()));
        assert_eq!(take_refusal().as_deref(), Some("no mesh"));
        assert_eq!(take_refusal(), None);
        std::thread::spawn(|| assert_eq!(take_refusal(), None)).join().unwrap();
    }
}
