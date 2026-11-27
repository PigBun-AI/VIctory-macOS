//! The M0 measurements, written once against `RawLink` so the same code runs
//! through Wine's unixlib (PE) and in-process (native baseline).

use std::ffi::c_void;
use std::hint::black_box;
use std::time::Instant;

use victory_metal_bridge::abi::*;

use crate::platform::{self, METAL_PAGE, Region};
use crate::stats::Stats;

/// The title's page. The a509c3a base took this from `victory_formats::pagetable::BLOCK`;
/// mac/livetr (feat/live-translator) has no `pagetable` module, so it is spelled out here.
const TITLE_PAGE: usize = 4096;

/// How much work each measurement does.
#[derive(Clone, Copy)]
pub struct Config {
    /// Calls per timed batch of the cheap calls.
    pub nop_batch: u32,
    pub args_batch: u32,
    /// Timed batches (or sample sets) per metric; the median is reported.
    pub reps: u32,
    /// Empty command buffers per rep.
    pub cb_batch: u32,
    /// Bytes checked for CPU/GPU agreement.
    pub check_len: usize,
    /// Bytes read in the bandwidth test.
    pub bw_len: usize,
    pub bw_reps: u32,
}

impl Config {
    /// The numbers behind `docs/metal/m0-results.md`.
    pub fn full() -> Config {
        Config {
            nop_batch: 1_000_000,
            args_batch: 500_000,
            reps: 21,
            cb_batch: 200,
            check_len: 16 << 20,
            bw_len: 64 << 20,
            bw_reps: 21,
        }
    }

    /// Small enough for `cargo test`.
    pub fn quick() -> Config {
        Config {
            nop_batch: 2_000,
            args_batch: 1_000,
            reps: 3,
            cb_batch: 5,
            check_len: 1 << 20,
            bw_len: 4 << 20,
            bw_reps: 3,
        }
    }
}

/// What a run printed, and which checks failed.
#[derive(Default)]
pub struct Report {
    pub lines: Vec<String>,
    pub failures: Vec<String>,
}

impl Report {
    pub fn say(&mut self, line: String) {
        println!("{line}");
        self.lines.push(line);
    }

    pub fn section(&mut self, title: &str) {
        self.say(format!("\n== {title}"));
    }

    pub fn info(&mut self, key: &str, value: impl AsRef<str>) {
        self.say(format!("INFO   {key} = {}", value.as_ref()));
    }

    pub fn metric(&mut self, key: &str, unit: &str, s: &Stats, note: &str) {
        self.say(format!(
            "METRIC {key}: median {:.2} {unit} (min {:.2}, max {:.2}, p99 {:.2}, n={}){}{note}",
            s.median,
            s.min,
            s.max,
            s.p99,
            s.n,
            if note.is_empty() { "" } else { "  # " },
        ));
    }

    pub fn check(&mut self, name: &str, ok: bool, detail: impl AsRef<str>) {
        let line = format!(
            "CHECK  {} {name}: {}",
            if ok { "PASS" } else { "FAIL" },
            detail.as_ref()
        );
        self.say(line.clone());
        if !ok {
            self.failures.push(line);
        }
    }
}

fn zeroed<T>() -> T {
    // SAFETY: only used for the plain-integer argument blocks of `abi`.
    unsafe { std::mem::zeroed() }
}

fn ptr<T>(value: &mut T) -> *mut c_void {
    (value as *mut T).cast()
}

fn pattern(i: usize, seed: u32) -> u32 {
    (i as u32).wrapping_mul(2_654_435_761).wrapping_add(seed)
}

fn ok(status: i32) -> bool {
    status == STATUS_SUCCESS
}

/// Per-call nanoseconds of `batch` calls of `f`, `reps` times.
fn batches(reps: u32, batch: u32, mut f: impl FnMut()) -> Vec<f64> {
    // One untimed batch warms caches, the dispatcher and Rosetta's translator.
    for _ in 0..batch.min(10_000) {
        f();
    }
    (0..reps)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..batch {
                f();
            }
            start.elapsed().as_nanos() as f64 / f64::from(batch)
        })
        .collect()
}

/// A CPU-side pattern, then the GPU sums it; then the GPU rewrites it and
/// the CPU reads it back; then the CPU writes again and the GPU sums again.
fn coherence<L: RawLink>(link: &L, r: &mut Report, label: &str, memory: &mut Region, handle: u32) {
    let words = memory.len / 4;
    let data = memory.words();
    for (i, w) in data.iter_mut().enumerate() {
        *w = pattern(i, 1);
    }
    let cpu_sum = data.iter().fold(0u32, |a, &w| a.wrapping_add(w));
    let mut args: GpuArgs = GpuArgs {
        handle,
        words: words as u32,
        key: 0,
        result: 0,
    };
    let status = link.call(OP_GPU_SUM, ptr(&mut args));
    r.check(
        &format!("{label}: GPU reads what the CPU wrote"),
        ok(status) && args.result == cpu_sum,
        format!(
            "status {status:#x}, gpu sum {:#010x}, cpu sum {cpu_sum:#010x}",
            args.result
        ),
    );

    let key = 0x5a5a_5a5a;
    let mut xor = GpuArgs {
        handle,
        words: words as u32,
        key,
        result: 0,
    };
    let status = link.call(OP_GPU_XOR, ptr(&mut xor));
    let bad = memory
        .words()
        .iter()
        .enumerate()
        .position(|(i, &w)| w != pattern(i, 1) ^ key ^ i as u32);
    r.check(
        &format!("{label}: CPU reads what the GPU wrote"),
        ok(status) && bad.is_none(),
        match bad {
            None => format!("{words} words match"),
            Some(i) => format!("status {status:#x}, first mismatch at word {i}"),
        },
    );

    let data = memory.words();
    for (i, w) in data.iter_mut().enumerate() {
        *w = pattern(i, 2);
    }
    let cpu_sum = data.iter().fold(0u32, |a, &w| a.wrapping_add(w));
    let mut again = GpuArgs {
        handle,
        words: words as u32,
        key: 0,
        result: 0,
    };
    let status = link.call(OP_GPU_SUM, ptr(&mut again));
    r.check(
        &format!("{label}: GPU sees a second CPU write"),
        ok(status) && again.result == cpu_sum,
        format!(
            "status {status:#x}, gpu sum {:#010x}, cpu sum {cpu_sum:#010x}",
            again.result
        ),
    );
}

fn wrap<L: RawLink>(link: &L, memory: &Region) -> (Option<u32>, f64) {
    let mut args = WrapArgs {
        ptr: memory.ptr.as_ptr() as u64,
        len: memory.len as u64,
        handle: 0,
        _pad: 0,
    };
    let start = Instant::now();
    let status = link.call(OP_WRAP, ptr(&mut args));
    let us = start.elapsed().as_secs_f64() * 1e6;
    (ok(status).then_some(args.handle), us)
}

fn release<L: RawLink>(link: &L, handle: u32) {
    let mut args = HandleArgs { handle, _pad: 0 };
    link.call(OP_RELEASE, ptr(&mut args));
}

/// GPU read bandwidth of each source in GB/s. The sources take turns round
/// by round, so a GPU clock that moves (a warming Mac) hits all of them alike.
fn bandwidth<L: RawLink>(
    link: &L,
    r: &mut Report,
    sources: &[(String, u32)],
    len: usize,
    reps: u32,
) {
    const WARMUP: u32 = 30;
    let mut gpu: Vec<Vec<f64>> = vec![Vec::new(); sources.len()];
    let mut wall: Vec<Vec<f64>> = vec![Vec::new(); sources.len()];
    for round in 0..WARMUP + reps {
        for (k, (label, handle)) in sources.iter().enumerate() {
            let mut args = BwArgs {
                handle: *handle,
                _pad: 0,
                len: len as u64,
                gpu_ns: 0,
                wall_ns: 0,
            };
            if !ok(link.call(OP_READ_BW, ptr(&mut args))) {
                r.check(&format!("bandwidth {label}"), false, "dispatch failed");
                return;
            }
            if round >= WARMUP {
                gpu[k].push(len as f64 / args.gpu_ns as f64);
                wall[k].push(len as f64 / args.wall_ns as f64);
            }
        }
    }
    for (k, (label, _)) in sources.iter().enumerate() {
        let note = format!("{} MiB, {WARMUP} warm-up rounds dropped", len >> 20);
        if let Some(s) = Stats::of(&gpu[k]) {
            r.metric(
                &format!("bandwidth.{label}.gpu_time"),
                "GB/s",
                &s,
                &format!("{note}; GPU start..end"),
            );
        }
        if let Some(s) = Stats::of(&wall[k]) {
            r.metric(
                &format!("bandwidth.{label}.wall_time"),
                "GB/s",
                &s,
                "commit..completion seen by the CPU",
            );
        }
    }
}

fn empty_cbs<L: RawLink>(link: &L, r: &mut Report, cfg: &Config, kind: u32, name: &str) {
    // From the caller: one call per command buffer, timed one by one.
    let mut per_call = Vec::new();
    for _ in 0..cfg.reps {
        for _ in 0..cfg.cb_batch {
            let mut args = EmptyCbArgs {
                kind,
                iterations: 1,
                elapsed_ns: 0,
            };
            let start = Instant::now();
            let status = link.call(OP_EMPTY_CB, ptr(&mut args));
            let us = start.elapsed().as_secs_f64() * 1e6;
            if !ok(status) {
                r.check(
                    &format!("empty command buffer, {name}"),
                    false,
                    format!("status {status:#x}"),
                );
                return;
            }
            per_call.push(us);
        }
    }
    if let Some(s) = Stats::of(&per_call) {
        r.metric(
            &format!("empty_cb.{name}.round_trip"),
            "us",
            &s,
            "commit and wait, one unixlib call each, timed by the caller",
        );
    }
    // Inside the unixlib: the whole loop in one call, so no per-call overhead.
    let mut inside = Vec::new();
    for _ in 0..cfg.reps {
        let mut args = EmptyCbArgs {
            kind,
            iterations: cfg.cb_batch,
            elapsed_ns: 0,
        };
        if !ok(link.call(OP_EMPTY_CB, ptr(&mut args))) {
            return;
        }
        inside.push(args.elapsed_ns as f64 / f64::from(cfg.cb_batch) / 1e3);
    }
    if let Some(s) = Stats::of(&inside) {
        r.metric(
            &format!("empty_cb.{name}.loop_inside_unixlib"),
            "us",
            &s,
            &format!("mean of {} per rep, loop inside one call", cfg.cb_batch),
        );
    }
}

/// Runs everything that does not need a window.
pub fn run<L: RawLink>(link: &L, cfg: &Config) -> Report {
    let mut r = Report::default();

    r.section("device");
    let mut init: InitArgs = zeroed();
    let status = link.call(OP_INIT, ptr(&mut init));
    if !ok(status) {
        r.check(
            "Metal device and queue created",
            false,
            format!("status {status:#x}"),
        );
        return r;
    }
    r.info("device", get_text(&init.name));
    r.info("unified memory", init.unified_memory.to_string());
    r.info("translated (Rosetta)", init.translated.to_string());
    r.info("process page size", init.page_size.to_string());
    r.info(
        "max buffer length",
        format!("{} MiB", init.max_buffer_len >> 20),
    );
    r.info(
        "Metal 4 GPU family supported",
        init.metal4_family.to_string(),
    );
    r.info(
        "Metal 4 queue/allocator/command buffer/event",
        if init.metal4_ok == 1 {
            "created".to_owned()
        } else {
            format!("not created: {}", get_text(&init.metal4_note))
        },
    );
    let metal4 = init.metal4_ok == 1;

    r.section("call cost (caller to unixlib and back)");
    if let Some(s) = Stats::of(&batches(cfg.reps, cfg.nop_batch, || {
        black_box(link.call(OP_NOP, std::ptr::null_mut()));
    })) {
        r.metric(
            "call.empty",
            "ns",
            &s,
            &format!("{} calls per rep, {} reps", cfg.nop_batch, cfg.reps),
        );
    }
    let mut block = Args256 { v: [0; 256] };
    let block_ptr = ptr(&mut block);
    if let Some(s) = Stats::of(&batches(cfg.reps, cfg.nop_batch, || {
        black_box(link.call(OP_NOP, black_box(block_ptr)));
    })) {
        r.metric(
            "call.pointer_only",
            "ns",
            &s,
            "1 KiB block passed, never touched",
        );
    }
    let mut counter = 0u32;
    if let Some(s) = Stats::of(&batches(cfg.reps, cfg.args_batch, || {
        counter = counter.wrapping_add(1);
        // SAFETY: `block_ptr` is the live `Args256` above.
        let block = unsafe { &mut *block_ptr.cast::<Args256>() };
        for (i, w) in block.v.iter_mut().enumerate() {
            *w = counter.wrapping_add(i as u32);
        }
        black_box(link.call(OP_ARGS256, block_ptr));
    })) {
        r.metric(
            "call.args256_filled_and_read",
            "ns",
            &s,
            "caller writes 256 u32 each call, unixlib reads all 256",
        );
    }
    if let Some(s) = Stats::of(&batches(cfg.reps, cfg.args_batch, || {
        counter = counter.wrapping_add(1);
        // SAFETY: as above.
        let block = unsafe { &mut *block_ptr.cast::<Args256>() };
        for (i, w) in block.v.iter_mut().enumerate() {
            *w = counter.wrapping_add(i as u32);
        }
        black_box(&*block);
    })) {
        r.metric(
            "call.args256_fill_only",
            "ns",
            &s,
            "the caller's fill alone, no call: subtract",
        );
    }

    r.section("empty command buffers");
    empty_cbs(link, &mut r, cfg, KIND_CLASSIC, "classic");
    if metal4 {
        empty_cbs(link, &mut r, cfg, KIND_METAL4, "metal4");
    } else {
        r.info("empty_cb.metal4", "skipped: no Metal 4 queue (see above)");
    }

    r.section(&format!("no-copy memory: {}", platform::PRIVATE_NAME));
    if let Some(mut memory) = platform::alloc_private(cfg.check_len) {
        r.check(
            &format!(
                "{} is {} KiB aligned",
                platform::PRIVATE_NAME,
                METAL_PAGE >> 10
            ),
            (memory.ptr.as_ptr() as usize).is_multiple_of(METAL_PAGE),
            format!("address {:p}", memory.ptr.as_ptr()),
        );
        let (handle, us) = wrap(link, &memory);
        r.check(
            "newBufferWithBytesNoCopy accepts the allocation",
            handle.is_some(),
            format!("{} MiB wrapped in {us:.1} us", memory.len >> 20),
        );
        if let Some(handle) = handle {
            coherence(link, &mut r, platform::PRIVATE_NAME, &mut memory, handle);
            release(link, handle);
        }
        // One title page (4 KiB) into the region: not a Metal page boundary.
        let skew = TITLE_PAGE;
        let mut args = WrapArgs {
            ptr: memory.ptr.as_ptr() as u64 + skew as u64,
            len: (memory.len - METAL_PAGE) as u64,
            handle: 0,
            _pad: 0,
        };
        let status = link.call(OP_WRAP, ptr(&mut args));
        r.info(
            "wrap at +4 KiB (TITLE_PAGE) inside the region",
            if ok(status) { "accepted" } else { "refused" },
        );
        if ok(status) {
            release(link, args.handle);
        }
    } else {
        r.check(
            &format!("{} allocation", platform::PRIVATE_NAME),
            false,
            "failed",
        );
    }

    r.section(&format!("no-copy memory: {}", platform::VIEW_NAME));
    match platform::alloc_view(cfg.check_len) {
        Ok(mut memory) => {
            let (handle, us) = wrap(link, &memory);
            r.check(
                "newBufferWithBytesNoCopy accepts the view",
                handle.is_some(),
                format!(
                    "{} MiB wrapped in {us:.1} us, address {:p}",
                    memory.len >> 20,
                    memory.ptr.as_ptr()
                ),
            );
            if let Some(handle) = handle {
                coherence(link, &mut r, platform::VIEW_NAME, &mut memory, handle);
                release(link, handle);
            }
        }
        Err(e) => r.info(platform::VIEW_NAME, format!("unavailable: {e}")),
    }

    r.section("residency set over the wrapped memory");
    // Two private regions of the same kind: one gets a residency set, one not.
    let plain = platform::alloc_private(cfg.bw_len);
    let with_set = platform::alloc_private(cfg.bw_len);
    let view = platform::alloc_view(cfg.bw_len);
    let mut sources: Vec<(String, u32)> = Vec::new();
    let mut keep: Vec<Region> = Vec::new();
    let mut held: Vec<u32> = Vec::new();
    let mut set_handle = None;
    for (label, memory) in [
        (platform::PRIVATE_NAME.to_owned(), plain),
        ("with_residency_set".to_owned(), with_set),
    ] {
        let Some(mut memory) = memory else { continue };
        memory
            .words()
            .iter_mut()
            .enumerate()
            .for_each(|(i, w)| *w = i as u32);
        if let (Some(handle), _) = wrap(link, &memory) {
            if label == "with_residency_set" {
                set_handle = Some((handle, memory.len));
            }
            sources.push((label, handle));
            held.push(handle);
            keep.push(memory);
        }
    }
    match view {
        Ok(mut memory) => {
            memory
                .words()
                .iter_mut()
                .enumerate()
                .for_each(|(i, w)| *w = i as u32);
            if let (Some(handle), _) = wrap(link, &memory) {
                sources.push((platform::VIEW_NAME.to_owned(), handle));
                held.push(handle);
                keep.push(memory);
            }
        }
        Err(e) => r.info(platform::VIEW_NAME, format!("unavailable: {e}")),
    }
    let mut shared = WrapArgs {
        ptr: 0,
        len: cfg.bw_len as u64,
        handle: 0,
        _pad: 0,
    };
    if ok(link.call(OP_WRAP, ptr(&mut shared))) {
        sources.push(("metal_shared_buffer".to_owned(), shared.handle));
        held.push(shared.handle);
    }

    if let Some((handle, len)) = set_handle {
        let words = (len / 4) as u32;
        let cpu_sum = (0..words).fold(0u32, |a, i| a.wrapping_add(i));
        let mut build = ResidencyArgs {
            handle,
            step: RES_BUILD,
            kind: 0,
            words,
            result: 0,
            flag: 0,
            t: [0; 4],
        };
        let status = link.call(OP_RESIDENCY, ptr(&mut build));
        r.check(
            "wrapped buffer added to a residency set, committed, residency requested",
            ok(status),
            format!(
                "create {:.1} us, add {:.1} us, commit {:.1} us, request {:.1} us",
                build.t[0] as f64 / 1e3,
                build.t[1] as f64 / 1e3,
                build.t[2] as f64 / 1e3,
                build.t[3] as f64 / 1e3
            ),
        );
        if ok(status) {
            let mut state = ResidencyArgs {
                step: RES_STATE,
                ..build
            };
            let status = link.call(OP_RESIDENCY, ptr(&mut state));
            r.check(
                "the set reports the wrapped buffer",
                ok(status) && state.flag == 1,
                format!("contains {}, allocation count {}", state.flag, state.t[0]),
            );
            let mut classic = ResidencyArgs {
                step: RES_USE,
                kind: KIND_CLASSIC,
                ..build
            };
            let status = link.call(OP_RESIDENCY, ptr(&mut classic));
            r.check(
                "classic queue with the set attached: GPU sum matches",
                ok(status) && classic.result == cpu_sum,
                format!(
                    "status {status:#x}, gpu {:#010x}, cpu {cpu_sum:#010x}",
                    classic.result
                ),
            );
            if metal4 {
                let mut m4 = ResidencyArgs {
                    step: RES_USE,
                    kind: KIND_METAL4,
                    ..build
                };
                let status = link.call(OP_RESIDENCY, ptr(&mut m4));
                r.check(
                    "Metal 4 queue with the set attached: empty command buffer completes",
                    ok(status),
                    format!("status {status:#x}"),
                );
            }
        }
    }

    r.section(&format!(
        "GPU read bandwidth, {} MiB (sources interleaved round by round)",
        cfg.bw_len >> 20
    ));
    bandwidth(link, &mut r, &sources, cfg.bw_len, cfg.bw_reps);
    for handle in held {
        release(link, handle);
    }
    drop(keep);

    let failed = r.failures.len();
    r.section("summary");
    r.info("checks failed", failed.to_string());
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_are_deterministic() {
        assert_eq!(pattern(0, 7), 7);
        assert_eq!(pattern(1, 0), 2_654_435_761);
        assert_eq!(TITLE_PAGE, 4096);
    }
}
