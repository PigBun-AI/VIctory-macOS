//! The back's half of the drop statistics (`VICTORY_METAL_DROPSTATS=<directory>`,
//! `docs/metal/dropstats.md`): per frame (a frame ends at `PRESENT`) the
//! draws and dispatches the executor was given, and those it did not run with
//! the reason and the pipeline. Rows go to `back.csv`; every pipeline
//! creation (made or refused, with the refusal's text) to `back-pso.csv`.
//! Off (and free: one atomic read) unless the variable is set.
//!
//! The counting and the row format are proto's (`dropstats`); this file is
//! the clock, the files and the pipeline labels of this process.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use victory_metal_proto::dropstats::{FrameStats, HEADER, Label, Op, PipeType};

/// The pipelines' labels (by the proto id), the frame's counts, the files.
pub(crate) struct Collector {
    stats: Mutex<FrameStats>,
    frame: Mutex<u64>,
    csv: Mutex<File>,
    pso_csv: Mutex<File>,
    labels: Mutex<HashMap<u32, Label>>,
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// A pipeline's creation row: when (ms and us of the day, for matching
/// against logs), kind, hash, result, reason.
pub(crate) const PSO_HEADER: &str = "epoch_us,kind,hash,pso,result,reason";

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Collector {
    pub(crate) fn open(dir: &Path) -> std::io::Result<Collector> {
        std::fs::create_dir_all(dir)?;
        let mut csv = File::create(dir.join("back.csv"))?;
        writeln!(csv, "{HEADER}")?;
        let mut pso_csv = File::create(dir.join("back-pso.csv"))?;
        writeln!(pso_csv, "{PSO_HEADER}")?;
        Ok(Collector {
            stats: Mutex::new(FrameStats::new()),
            frame: Mutex::new(0),
            csv: Mutex::new(csv),
            pso_csv: Mutex::new(pso_csv),
            labels: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn register(&self, pso: u32, label: Label) {
        lock(&self.labels).insert(pso, label);
    }

    pub(crate) fn label_of(&self, pso: u32) -> Label {
        lock(&self.labels).get(&pso).copied().unwrap_or(Label::NONE)
    }

    pub(crate) fn submit(&self, op: Op, label: Label) {
        lock(&self.stats).submit(op, label);
    }

    pub(crate) fn drop_ops(&self, op: Op, label: Label, reason: &str, n: u64) {
        lock(&self.stats).drop_ops(op, label, reason, n);
    }

    pub(crate) fn noop(&self) {
        lock(&self.stats).noop();
    }

    pub(crate) fn extra(&self, name: &str, n: u64) {
        lock(&self.stats).extra(name, n);
    }

    /// A pipeline creation: `ok` with the id, or the refusal's text.
    pub(crate) fn pso_created(&self, kind: &str, hash: u64, result: Result<u32, &str>) {
        let us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_micros() as u64);
        let (pso, result, reason) = match result {
            Ok(id) => (id, "ok", String::new()),
            Err(why) => (0, "refused", victory_metal_proto::dropstats::sanitize(why)),
        };
        let _ = writeln!(
            lock(&self.pso_csv),
            "{us},{kind},{hash:016x},{pso},{result},{reason}"
        );
        if result != "ok" {
            self.extra(&format!("pso refused {kind}"), 1);
        }
    }

    /// The frame ends (`PRESENT`): its row goes out.
    pub(crate) fn end_frame(&self) {
        let mut frame = lock(&self.frame);
        *frame += 1;
        let row = lock(&self.stats).take_row(*frame, epoch_ms());
        let mut csv = lock(&self.csv);
        let _ = writeln!(csv, "{row}");
        let _ = csv.flush();
    }
}

static COLLECTOR: OnceLock<Option<Collector>> = OnceLock::new();

/// The process's collector, if `VICTORY_METAL_DROPSTATS` names a directory.
pub(crate) fn get() -> Option<&'static Collector> {
    COLLECTOR
        .get_or_init(|| {
            let dir = std::env::var_os("VICTORY_METAL_DROPSTATS").filter(|v| !v.is_empty())?;
            match Collector::open(Path::new(&dir)) {
                Ok(c) => Some(c),
                Err(e) => {
                    eprintln!("victory-metal back: VICTORY_METAL_DROPSTATS {dir:?}: {e}");
                    None
                }
            }
        })
        .as_ref()
}

/// Ends the frame of the statistics (what `PRESENT` does; public for tests).
pub fn end_frame() {
    if let Some(c) = get() {
        c.end_frame();
    }
}

thread_local! {
    /// Why the op the thread is running was skipped without an error (a
    /// deliberate skip deep in the draw code).
    static SKIP: RefCell<Option<String>> = const { RefCell::new(None) };
    /// The last refusal text of this thread (what `note_error` was told).
    static REASON: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The op being run is skipped on purpose (it still returns `Ok`).
pub(crate) fn skip(reason: &str) {
    if get().is_some() {
        SKIP.with(|s| *s.borrow_mut() = Some(reason.to_owned()));
    }
}

pub(crate) fn take_skip() -> Option<String> {
    SKIP.with(|s| s.borrow_mut().take())
}

/// Remembers the text of a refusal for the creation row.
pub(crate) fn note_reason(text: &str) {
    if get().is_some() {
        REASON.with(|s| *s.borrow_mut() = Some(text.to_owned()));
    }
}

pub(crate) fn take_reason() -> Option<String> {
    REASON.with(|s| s.borrow_mut().take())
}

/// A pipeline's label from the bytes of its stages.
pub(crate) fn label_of_stages(ty: PipeType, stages: &[&[u8]]) -> Label {
    Label {
        hash: victory_metal_proto::dropstats::hash_parts(stages),
        ty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("victory-dropstats-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn frames_are_written_one_row_each_at_end_frame() {
        let dir = temp("rows");
        let c = Collector::open(&dir).unwrap();
        let l = Label {
            hash: 0x1234,
            ty: PipeType::Ms,
        };
        c.register(5, l);
        assert_eq!(c.label_of(5), l);
        assert_eq!(c.label_of(6), Label::NONE);
        c.submit(Op::Mesh, l);
        c.submit(Op::Draw, Label::NONE);
        c.drop_ops(Op::Mesh, l, "over a Metal limit", 1);
        c.end_frame();
        c.submit(Op::Dispatch, Label::NONE);
        c.end_frame();
        let text = std::fs::read_to_string(dir.join("back.csv")).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], HEADER);
        assert_eq!(lines.len(), 3);
        assert!(lines[1].starts_with("1,"));
        assert!(lines[1].contains("0000000000001234:MS:over a Metal limit=1"));
        assert!(lines[2].starts_with("2,"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refused_pipeline_is_a_row_and_an_extra_of_the_frame() {
        let dir = temp("pso");
        let c = Collector::open(&dir).unwrap();
        c.pso_created("ms", 0xabcd, Err("MSC refused, attributeAtVertex"));
        c.pso_created("vs", 0xef, Ok(3));
        c.end_frame();
        let pso = std::fs::read_to_string(dir.join("back-pso.csv")).unwrap();
        let lines: Vec<&str> = pso.lines().collect();
        assert_eq!(lines[0], PSO_HEADER);
        assert!(
            lines[1].ends_with(",ms,000000000000abcd,0,refused,MSC refused  attributeAtVertex")
        );
        assert!(lines[2].ends_with(",vs,00000000000000ef,3,ok,"));
        let frames = std::fs::read_to_string(dir.join("back.csv")).unwrap();
        assert!(frames.lines().nth(1).unwrap().ends_with("pso refused ms=1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_stage_hash_depends_on_the_bytes_and_their_order() {
        let a = label_of_stages(PipeType::Vs, &[b"vs", b"ps"]);
        let b = label_of_stages(PipeType::Vs, &[b"ps", b"vs"]);
        assert_ne!(a.hash, b.hash);
        assert_eq!(a, label_of_stages(PipeType::Vs, &[b"vs", b"ps"]));
    }
}
