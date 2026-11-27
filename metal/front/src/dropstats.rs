//! The front's half of the drop statistics (`VICTORY_METAL_DROPSTATS=<directory>`,
//! `docs/metal/dropstats.md`): per frame (a frame ends at `Present`) the draws
//! and dispatches of the command lists the title executed, and those the front
//! could not hand to the back, with the reason and the pipeline. Rows go to
//! `front.csv`; every pipeline creation (made or refused) to `front-pso.csv`.
//! Off unless the variable is set.
//!
//! The counting and the row format are proto's (`dropstats`). The directory is
//! given as a Unix path (the back, in the same process, reads the same
//! variable); in the PE half it is turned into a `Z:` path for Wine.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use victory_metal_proto::dropstats::{
    FrameStats, HEADER, Label, Op, PipeType, op_of, pipeline_of, sanitize,
};
use victory_metal_proto::ids::PsoId;
use victory_metal_proto::{Command, pso::GraphicsPsoDesc};

use crate::device::Item;

/// The columns of `front-pso.csv`.
pub const PSO_HEADER: &str = "epoch_us,kind,hash,pso,result,reason";

/// A Unix path (`/Users/...`) as the Windows process Wine runs sees it.
pub fn wine_path(unix: &str) -> PathBuf {
    match unix.strip_prefix('/') {
        Some(rest) => PathBuf::from(format!("Z:\\{}", rest.replace('/', "\\"))),
        None => PathBuf::from(unix),
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
}

/// A file made on first use (the PE half of the front is two dlls, `d3d12`
/// and `dxgi`, each with its own copy of this module: each makes only the
/// files it writes to, so they do not truncate one another's).
struct Lazy {
    path: PathBuf,
    header: &'static str,
    file: Mutex<Option<File>>,
}

impl Lazy {
    fn new(dir: &Path, name: &str, header: &'static str) -> Lazy {
        Lazy {
            path: dir.join(name),
            header,
            file: Mutex::new(None),
        }
    }

    fn line(&self, text: &str) {
        let mut g = lock(&self.file);
        if g.is_none() {
            *g = File::create(&self.path).ok().inspect(|_| ());
            if let Some(f) = g.as_mut() {
                let _ = writeln!(f, "{}", self.header);
            }
        }
        if let Some(f) = g.as_mut() {
            let _ = writeln!(f, "{text}");
            let _ = f.flush();
        }
    }
}

/// `front.csv` has one row per `ExecuteCommandLists` call (and per list that
/// failed at `Close`), in [`HEADER`]'s columns: the first column counts the
/// rows, not frames. The `d3d12` dll holds the lists and the pipelines, the
/// `dxgi` dll holds `Present`: `front-present.csv` has one line per present
/// (`present,epoch_ms`), and the rows are put into frames by those times.
pub struct Collector {
    stats: Mutex<FrameStats>,
    rows: Mutex<u64>,
    presents: Mutex<u64>,
    csv: Lazy,
    present_csv: Lazy,
    pso_csv: Lazy,
    labels: Mutex<HashMap<u32, Label>>,
}

impl Collector {
    pub fn open(dir: &Path) -> std::io::Result<Collector> {
        std::fs::create_dir_all(dir)?;
        Ok(Collector {
            stats: Mutex::new(FrameStats::new()),
            rows: Mutex::new(0),
            presents: Mutex::new(0),
            csv: Lazy::new(dir, "front.csv", HEADER),
            present_csv: Lazy::new(dir, "front-present.csv", "present,epoch_ms"),
            pso_csv: Lazy::new(dir, "front-pso.csv", PSO_HEADER),
            labels: Mutex::new(HashMap::new()),
        })
    }

    pub fn register(&self, pso: PsoId, label: Label) {
        lock(&self.labels).insert(pso.0, label);
    }

    pub fn label_of(&self, pso: PsoId) -> Label {
        lock(&self.labels)
            .get(&pso.0)
            .copied()
            .unwrap_or(Label::NONE)
    }

    /// The draws and dispatches of a recorded list, each with the pipeline set
    /// when it was recorded.
    pub fn ops_of(&self, items: &[Item]) -> Vec<(Op, Label)> {
        let mut pso = PsoId::NULL;
        let mut out = Vec::new();
        let look = |c: &Command<'_>, pso: &mut PsoId| {
            if let Some(p) = pipeline_of(c) {
                *pso = p;
            }
            op_of(c).map(|op| (op, self.label_of(*pso)))
        };
        for it in items {
            match it {
                Item::Cmd(c) => out.extend(look(c, &mut pso)),
                Item::Indirect(i) => out.extend(look(&i.command(), &mut pso)),
                _ => {}
            }
        }
        out
    }

    /// Writes the counts since the last row as a row.
    fn flush_row(&self) {
        let mut n = lock(&self.rows);
        *n += 1;
        let row = lock(&self.stats).take_row(*n, now_us() / 1000);
        self.csv.line(&row);
    }

    /// `lists` were handed to `ExecuteCommandLists`: their ops are submitted.
    pub fn submitted(&self, lists: &[std::sync::Arc<[Item]>]) -> Vec<(Op, Label)> {
        let ops: Vec<(Op, Label)> = lists.iter().flat_map(|l| self.ops_of(l)).collect();
        let mut s = lock(&self.stats);
        for (op, label) in &ops {
            s.submit(*op, *label);
        }
        ops
    }

    pub fn dropped(&self, ops: &[(Op, Label)], why: &str) {
        let mut s = lock(&self.stats);
        for (op, label) in ops {
            s.drop_ops(*op, *label, why, 1);
        }
    }

    /// The call is over: its row goes out.
    pub fn executed(&self) {
        self.flush_row();
    }

    /// Ops that never reached the queue (a list was not closed or had failed):
    /// submitted and dropped in one row.
    pub fn lost(&self, items: &[&[Item]], why: &str) {
        let ops: Vec<(Op, Label)> = items.iter().flat_map(|l| self.ops_of(l)).collect();
        {
            let mut s = lock(&self.stats);
            for (op, label) in &ops {
                s.submit(*op, *label);
            }
        }
        self.dropped(&ops, why);
        self.flush_row();
    }

    /// A list that failed at `Close`: its ops were recorded, never executed.
    pub fn list_failed(&self, items: &[Item], why: &str) {
        lock(&self.stats).extra("lists failed at Close", 1);
        self.lost(&[items], why);
    }

    /// A pipeline creation, with the front's answer.
    pub fn pso_created(&self, kind: &str, hash: u64, result: Result<PsoId, &str>) {
        let (pso, result, reason) = match result {
            Ok(id) => (id.0, "ok", String::new()),
            Err(why) => (0, "refused", sanitize(why)),
        };
        self.pso_csv.line(&format!(
            "{},{kind},{hash:016x},{pso},{result},{reason}",
            now_us()
        ));
    }

    /// `Present` (the `dxgi` dll's copy).
    pub fn end_frame(&self) {
        let mut n = lock(&self.presents);
        *n += 1;
        self.present_csv
            .line(&format!("{},{}", *n, now_us() / 1000));
    }
}

static COLLECTOR: OnceLock<Option<Collector>> = OnceLock::new();

pub fn get() -> Option<&'static Collector> {
    COLLECTOR
        .get_or_init(|| {
            let dir = std::env::var("VICTORY_METAL_DROPSTATS")
                .ok()
                .filter(|v| !v.is_empty())?;
            #[cfg(windows)]
            let path = wine_path(&dir);
            #[cfg(not(windows))]
            let path = PathBuf::from(&dir);
            match Collector::open(&path) {
                Ok(c) => Some(c),
                Err(e) => {
                    eprintln!("victory-metal front: VICTORY_METAL_DROPSTATS {dir}: {e}");
                    None
                }
            }
        })
        .as_ref()
}

/// Ends the frame of the statistics (`Present`).
pub fn end_frame() {
    if let Some(c) = get() {
        c.end_frame();
    }
}

/// The label of a graphics pipeline description (its stages' bytes in the order
/// the back hashes them: vertex, pixel, amplification, mesh, geometry).
///
/// # Safety
/// The stages' addresses must be readable for their lengths (the description
/// is the caller's for the duration of the pipeline call).
pub unsafe fn graphics_label(d: &GraphicsPsoDesc) -> Label {
    let stages = [d.vs, d.ps, d.amp, d.ms, d.gs];
    let parts: Vec<&[u8]> = stages
        .iter()
        // SAFETY: the caller's contract.
        .map(|s| {
            if s.is_present() && s.addr != 0 {
                unsafe { std::slice::from_raw_parts(s.addr as *const u8, s.len as usize) }
            } else {
                &[][..]
            }
        })
        .collect();
    let ty = if d.ms.is_present() {
        PipeType::Ms
    } else if d.gs.is_present() {
        PipeType::Gs
    } else {
        PipeType::Vs
    };
    Label {
        hash: victory_metal_proto::dropstats::hash_parts(&parts),
        ty,
    }
}

pub fn compute_label(dxil: &[u8]) -> Label {
    Label {
        hash: victory_metal_proto::dropstats::hash_parts(&[dxil]),
        ty: PipeType::Cs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use victory_metal_proto::cmd::{Dispatch, SetPipeline};
    use victory_metal_proto::render::Draw;
    use victory_metal_proto::wire::Wire;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "victory-front-dropstats-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn list() -> Vec<Item> {
        vec![
            Item::Cmd(Command::Draw(<Draw as Wire>::ZERO)),
            Item::Cmd(Command::SetPipeline(SetPipeline {
                pso: PsoId(4),
                _pad: 0,
            })),
            Item::Cmd(Command::Draw(<Draw as Wire>::ZERO)),
            Item::Cmd(Command::Dispatch(<Dispatch as Wire>::ZERO)),
            Item::Label(b"x".to_vec()),
        ]
    }

    #[test]
    fn unix_paths_become_wine_z_paths() {
        assert_eq!(
            wine_path("/Users/user/b c/out"),
            PathBuf::from("Z:\\Users\\user\\b c\\out")
        );
        assert_eq!(wine_path("relative/dir"), PathBuf::from("relative/dir"));
    }

    #[test]
    fn a_list_s_ops_carry_the_pipeline_set_before_them() {
        let dir = temp("ops");
        let c = Collector::open(&dir).unwrap();
        let l = Label {
            hash: 9,
            ty: PipeType::Vs,
        };
        c.register(PsoId(4), l);
        let ops = c.ops_of(&list());
        assert_eq!(
            ops,
            [(Op::Draw, Label::NONE), (Op::Draw, l), (Op::Dispatch, l)]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn executed_lists_are_submitted_and_a_refusal_drops_them() {
        let dir = temp("exec");
        let c = Collector::open(&dir).unwrap();
        let lists: Vec<std::sync::Arc<[Item]>> = vec![list().into(), list().into()];
        let ops = c.submitted(&lists);
        assert_eq!(ops.len(), 6);
        c.dropped(&ops, "ExecuteCommandLists refused");
        c.executed();
        c.list_failed(&list(), "list failed at x.rs:1");
        c.end_frame();
        let text = std::fs::read_to_string(dir.join("front.csv")).unwrap();
        assert_eq!(text.lines().count(), 3);
        let present = std::fs::read_to_string(dir.join("front-present.csv")).unwrap();
        assert!(present.starts_with("present,epoch_ms\n1,"));
        let row: Vec<&str> = text.lines().nth(1).unwrap().split(',').collect();
        let header: Vec<&str> = HEADER.split(',').collect();
        let at = |n: &str| row[header.iter().position(|h| *h == n).unwrap()];
        assert_eq!((at("submitted_draw"), at("submitted_dispatch")), ("4", "2"));
        assert_eq!((at("dropped_draw"), at("dropped_dispatch")), ("4", "2"));
        assert!(at("reasons").contains("ExecuteCommandLists refused=6"));
        let row: Vec<&str> = text.lines().nth(2).unwrap().split(',').collect();
        let at = |n: &str| row[header.iter().position(|h| *h == n).unwrap()];
        assert_eq!((at("submitted_draw"), at("dropped_draw")), ("2", "2"));
        assert!(at("extras").contains("lists failed at Close=1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_creation_row_names_the_hash_and_the_result() {
        let dir = temp("pso");
        let c = Collector::open(&dir).unwrap();
        c.pso_created("graphics-ms", 0xfeed, Err("0x80070057"));
        c.pso_created("compute", 1, Ok(PsoId(2)));
        let text = std::fs::read_to_string(dir.join("front-pso.csv")).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[1].ends_with(",graphics-ms,000000000000feed,0,refused,0x80070057"));
        assert!(lines[2].ends_with(",compute,0000000000000001,2,ok,"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_front_and_the_back_hash_stage_bytes_alike() {
        let (vs, ps) = (b"vsbytes".to_vec(), b"psbytes".to_vec());
        let mut d = GraphicsPsoDesc::new();
        d.vs = victory_metal_proto::pso::StageCode::of(&vs);
        d.ps = victory_metal_proto::pso::StageCode::of(&ps);
        // SAFETY: the stages are the vectors above.
        let l = unsafe { graphics_label(&d) };
        let none: &[u8] = &[];
        assert_eq!(
            l.hash,
            victory_metal_proto::dropstats::hash_parts(&[&vs, &ps, none, none, none])
        );
        assert_eq!(l.ty, PipeType::Vs);
    }
}
