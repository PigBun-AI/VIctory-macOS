//! Per-frame counts of the draws and dispatches a layer was given and of
//! those it dropped, with the reason and the pipeline they belong to.
//!
//! Both halves (the front in the PE, the back in the unixlib) fill a
//! [`FrameStats`] and write one CSV row per frame ([`FrameStats::take_row`],
//! header [`HEADER`]); the files are written by the crates themselves
//! (`VICTORY_METAL_DROPSTATS=<directory>`, `docs/metal/dropstats.md`). This
//! module is only the counting and the row format: `no_std`, no clock, no
//! files, names nothing of D3D12 or Metal.
//!
//! A *submitted* op is one the layer was asked to run (a `DRAW`, `DRAW_INDEXED`,
//! `DISPATCH_MESH`, `DISPATCH`, or one `EXECUTE_INDIRECT` command: its records
//! are not counted separately). A *dropped* op is a submitted one the layer did
//! not run, whatever the cause: a failed command, the rest of a block a failed
//! command stopped, a deliberate skip. A *no-op* ran with a zero count and is
//! not a drop.

use crate::cmd::Command;
use crate::ids::PsoId;
use crate::indirect::IndirectArgKind;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

/// What was submitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Op {
    Draw,
    DrawIndexed,
    Mesh,
    IndirectDraw,
    Dispatch,
    IndirectDispatch,
}

impl Op {
    /// The compute side: counted in the `dispatch` columns, the rest are draws.
    pub fn is_dispatch(self) -> bool {
        matches!(self, Op::Dispatch | Op::IndirectDispatch)
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// The kind of pipeline an op ran with (`NONE`: no pipeline was bound or it is unknown).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PipeType {
    /// Vertex (and pixel) stages.
    Vs,
    /// Mesh (with or without amplification).
    Ms,
    /// Vertex, geometry and pixel stages.
    Gs,
    Cs,
    None,
}

impl PipeType {
    pub const ALL: [PipeType; 5] = [
        PipeType::Vs,
        PipeType::Ms,
        PipeType::Gs,
        PipeType::Cs,
        PipeType::None,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PipeType::Vs => "VS",
            PipeType::Ms => "MS",
            PipeType::Gs => "GS",
            PipeType::Cs => "CS",
            PipeType::None => "NONE",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// A pipeline as the rows name it: the hash of its stages' bytes and its type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Label {
    pub hash: u64,
    pub ty: PipeType,
}

impl Default for Label {
    fn default() -> Label {
        Label::NONE
    }
}

impl Label {
    pub const NONE: Label = Label {
        hash: 0,
        ty: PipeType::None,
    };
}

/// FNV-1a (64 bit) over `parts`, a 0xff byte between them (so `["ab", "c"]`
/// and `["a", "bc"]` differ). Front and back hash the same stage bytes with
/// this, which is how their rows name the same pipeline.
pub fn hash_parts(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut put = |b: u8| {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for p in parts {
        for &b in *p {
            put(b);
        }
        put(0xff);
    }
    h
}

/// A reason as one CSV-safe token: no commas, semicolons, equal signs,
/// quotes or line breaks, at most 160 characters.
pub fn sanitize(reason: &str) -> String {
    let mut out = String::new();
    for c in reason.chars().take(160) {
        out.push(match c {
            ',' | ';' | '=' | '"' | '\n' | '\r' | '\t' => ' ',
            c => c,
        });
    }
    let trimmed = out.trim();
    if trimmed.is_empty() {
        "unknown".into()
    } else {
        trimmed.into()
    }
}

/// The op a command is, if it is a draw or a dispatch.
pub fn op_of(c: &Command<'_>) -> Option<Op> {
    Some(match c {
        Command::Draw(_) => Op::Draw,
        Command::DrawIndexed(_) => Op::DrawIndexed,
        Command::DispatchMesh(_) => Op::Mesh,
        Command::Dispatch(_) => Op::Dispatch,
        Command::ExecuteIndirect(e) => {
            // The record's action decides which side runs it (as the back does).
            if e.arguments()
                .any(|a| a.kind.is_action() && a.kind != IndirectArgKind::DISPATCH)
            {
                Op::IndirectDraw
            } else {
                Op::IndirectDispatch
            }
        }
        _ => return None,
    })
}

/// The pipeline a command sets, if it sets one.
pub fn pipeline_of(c: &Command<'_>) -> Option<PsoId> {
    match c {
        Command::SetPipeline(p) => Some(p.pso),
        _ => None,
    }
}

/// The header of [`FrameStats::take_row`]'s rows.
pub const HEADER: &str = "frame,epoch_ms,draw,draw_indexed,mesh,indirect_draw,dispatch,indirect_dispatch,\
submitted_draw,submitted_dispatch,dropped_draw,dropped_dispatch,noop,\
sub_vs,sub_ms,sub_gs,sub_cs,sub_none,drop_vs,drop_ms,drop_gs,drop_cs,drop_none,\
reasons,pipelines,extras";

/// One frame's counts.
#[derive(Clone, Debug, Default)]
pub struct FrameStats {
    ops: [u64; 6],
    noops: u64,
    submitted_by_type: [u64; 5],
    dropped_by_type: [u64; 5],
    dropped_draw: u64,
    dropped_dispatch: u64,
    reasons: BTreeMap<String, u64>,
    pipelines: BTreeMap<(Label, String), u64>,
    extras: BTreeMap<String, u64>,
}

impl FrameStats {
    pub fn new() -> FrameStats {
        FrameStats::default()
    }

    /// One op submitted with the pipeline `label`.
    pub fn submit(&mut self, op: Op, label: Label) {
        self.ops[op.index()] += 1;
        self.submitted_by_type[label.ty.index()] += 1;
    }

    /// `n` of the ops already submitted were not run, for `reason`.
    pub fn drop_ops(&mut self, op: Op, label: Label, reason: &str, n: u64) {
        if n == 0 {
            return;
        }
        if op.is_dispatch() {
            self.dropped_dispatch += n;
        } else {
            self.dropped_draw += n;
        }
        self.dropped_by_type[label.ty.index()] += n;
        let reason = sanitize(reason);
        *self.pipelines.entry((label, reason.clone())).or_default() += n;
        *self.reasons.entry(reason).or_default() += n;
    }

    /// An op that ran with a zero count (nothing to draw, not a drop).
    pub fn noop(&mut self) {
        self.noops += 1;
    }

    /// A free counter of the frame (pipelines refused, lists failed ...).
    pub fn extra(&mut self, name: &str, n: u64) {
        *self.extras.entry(sanitize(name)).or_default() += n;
    }

    /// Ops submitted so far this frame.
    pub fn submitted(&self) -> u64 {
        self.ops.iter().sum()
    }

    /// Ops dropped so far this frame.
    pub fn dropped(&self) -> u64 {
        self.dropped_draw + self.dropped_dispatch
    }

    /// The frame's CSV row (no line break) and a fresh frame.
    pub fn take_row(&mut self, frame: u64, epoch_ms: u64) -> String {
        let s = core::mem::take(self);
        let draw_like: u64 = s
            .ops
            .iter()
            .enumerate()
            .filter(|(i, _)| *i < 4)
            .map(|(_, n)| n)
            .sum();
        let dispatch_like = s.ops[Op::Dispatch.index()] + s.ops[Op::IndirectDispatch.index()];
        let mut row = String::new();
        let _ = write!(row, "{frame},{epoch_ms}");
        for n in s.ops {
            let _ = write!(row, ",{n}");
        }
        let _ = write!(
            row,
            ",{draw_like},{dispatch_like},{},{},{}",
            s.dropped_draw, s.dropped_dispatch, s.noops
        );
        for n in s.submitted_by_type {
            let _ = write!(row, ",{n}");
        }
        for n in s.dropped_by_type {
            let _ = write!(row, ",{n}");
        }
        row.push(',');
        join(&mut row, s.reasons.iter().map(|(k, n)| (k.clone(), *n)));
        row.push(',');
        join(
            &mut row,
            s.pipelines
                .iter()
                .map(|((l, r), n)| (alloc::format!("{:016x}:{}:{r}", l.hash, l.ty.name()), *n)),
        );
        row.push(',');
        join(&mut row, s.extras.iter().map(|(k, n)| (k.clone(), *n)));
        row
    }
}

/// What a block's ops are worth if a later command stops it: the back
/// commits a queue's lists together (at a signal, a wait, a full pool, the end
/// of the block), so a failure loses the ops of the open list and of every
/// list ended since the queue was last committed, as well as the commands
/// after it.
#[derive(Debug, Default)]
pub struct BlockTracker {
    open: Vec<(Op, Label)>,
    ended: BTreeMap<u32, Vec<Vec<(Op, Label)>>>,
}

impl BlockTracker {
    pub fn begin_list(&mut self) {
        self.open.clear();
    }

    /// An op ran in the open list.
    pub fn ran(&mut self, op: Op, label: Label) {
        self.open.push((op, label));
    }

    /// The open list ended for `queue`.
    pub fn end_list(&mut self, queue: u32) {
        let list = core::mem::take(&mut self.open);
        self.ended.entry(queue).or_default().push(list);
    }

    /// Call after every command: `waiting(queue)` is how many lists of the
    /// queue are still uncommitted. A queue with fewer than we hold was
    /// committed (the back commits all of a queue's lists at once).
    pub fn sync(&mut self, waiting: impl Fn(u32) -> usize) {
        for (q, lists) in self.ended.iter_mut() {
            if lists.len() > waiting(*q) {
                lists.clear();
            }
        }
    }

    /// The ops not committed yet, and forget them.
    pub fn lost(&mut self) -> Vec<(Op, Label)> {
        let mut out = core::mem::take(&mut self.open);
        for lists in self.ended.values_mut() {
            for l in lists.drain(..) {
                out.extend(l);
            }
        }
        out
    }
}

fn join(out: &mut String, items: impl Iterator<Item = (String, u64)>) {
    let mut first = true;
    for (k, n) in items {
        if !first {
            out.push(';');
        }
        first = false;
        let _ = write!(out, "{k}={n}");
    }
}

/// Parses `a=1;b=2` back into pairs (the analysis and the tests read rows with it).
pub fn split_pairs(field: &str) -> Vec<(&str, u64)> {
    field
        .split(';')
        .filter(|p| !p.is_empty())
        .filter_map(|p| {
            let (k, n) = p.rsplit_once('=')?;
            Some((k, n.parse().ok()?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(ty: PipeType) -> Label {
        Label { hash: 0xabc, ty }
    }

    #[test]
    fn a_row_counts_submitted_and_dropped_by_op_type_and_reason() {
        let mut s = FrameStats::new();
        for _ in 0..5 {
            s.submit(Op::Draw, label(PipeType::Vs));
        }
        s.submit(Op::Mesh, label(PipeType::Ms));
        s.submit(Op::IndirectDraw, label(PipeType::Gs));
        s.submit(Op::Dispatch, label(PipeType::Cs));
        s.submit(Op::IndirectDispatch, label(PipeType::Cs));
        s.drop_ops(Op::Mesh, label(PipeType::Ms), "no pipeline", 1);
        s.drop_ops(Op::IndirectDraw, label(PipeType::Gs), "gs skipped", 1);
        s.drop_ops(Op::Dispatch, label(PipeType::Cs), "no pipeline", 1);
        assert_eq!((s.submitted(), s.dropped()), (9, 3));
        let row = s.take_row(7, 1234);
        let c: Vec<&str> = row.split(',').collect();
        let header: Vec<&str> = HEADER.split(',').collect();
        assert_eq!(c.len(), header.len(), "{row}");
        let at = |name: &str| c[header.iter().position(|h| *h == name).unwrap()];
        assert_eq!((at("frame"), at("epoch_ms")), ("7", "1234"));
        assert_eq!(
            (at("draw"), at("mesh"), at("indirect_draw")),
            ("5", "1", "1")
        );
        assert_eq!((at("submitted_draw"), at("submitted_dispatch")), ("7", "2"));
        assert_eq!((at("dropped_draw"), at("dropped_dispatch")), ("2", "1"));
        assert_eq!(
            (at("sub_vs"), at("sub_ms"), at("sub_gs"), at("sub_cs")),
            ("5", "1", "1", "2")
        );
        assert_eq!(
            (at("drop_vs"), at("drop_ms"), at("drop_gs"), at("drop_cs")),
            ("0", "1", "1", "1")
        );
        assert_eq!(
            split_pairs(at("reasons")),
            [("gs skipped", 1), ("no pipeline", 2)]
        );
        assert_eq!(
            split_pairs(at("pipelines")),
            [
                ("0000000000000abc:MS:no pipeline", 1),
                ("0000000000000abc:GS:gs skipped", 1),
                ("0000000000000abc:CS:no pipeline", 1)
            ]
        );
    }

    #[test]
    fn taking_a_row_starts_a_new_frame() {
        let mut s = FrameStats::new();
        s.submit(Op::Draw, Label::NONE);
        s.drop_ops(Op::Draw, Label::NONE, "x", 1);
        s.noop();
        s.extra("pso refused", 2);
        let first = s.take_row(1, 0);
        assert!(first.contains("pso refused=2"));
        let empty = s.take_row(2, 0);
        assert_eq!(empty, "2,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,,,");
    }

    #[test]
    fn reasons_are_csv_safe_and_never_empty() {
        assert_eq!(sanitize("a, b; c=d \"e\"\nf"), "a  b  c d  e  f");
        assert_eq!(sanitize(""), "unknown");
        assert_eq!(sanitize(&"x".repeat(500)).len(), 160);
        let mut s = FrameStats::new();
        s.submit(Op::Draw, Label::NONE);
        s.drop_ops(Op::Draw, Label::NONE, "a,b", 1);
        let row = s.take_row(1, 1);
        assert_eq!(row.split(',').count(), HEADER.split(',').count());
    }

    #[test]
    fn zero_drops_leave_no_trace() {
        let mut s = FrameStats::new();
        s.drop_ops(Op::Draw, Label::NONE, "x", 0);
        assert_eq!(s.dropped(), 0);
        assert!(s.take_row(1, 1).ends_with(",,,"));
    }

    #[test]
    fn the_hash_is_fnv_1a_with_part_separators() {
        assert_eq!(hash_parts(&[]), 0xcbf2_9ce4_8422_2325);
        assert_ne!(hash_parts(&[b"ab", b"c"]), hash_parts(&[b"a", b"bc"]));
        assert_eq!(hash_parts(&[b"abc"]), hash_parts(&[b"abc"]));
        // FNV-1a of "a" then the 0xff separator, worked by hand.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in [b'a', 0xffu8] {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        assert_eq!(hash_parts(&[b"a"]), h);
    }
}

#[cfg(test)]
mod op_tests {
    use super::*;
    use crate::cmd::{Dispatch, SetPipeline};
    use crate::indirect::{ExecuteIndirect, IndirectArg};
    use crate::render::{DispatchMesh, Draw, DrawIndexed};
    use crate::wire::Wire;

    #[test]
    fn commands_map_to_ops() {
        let z = |d: Draw| Command::Draw(d);
        assert_eq!(op_of(&z(<Draw as Wire>::ZERO)), Some(Op::Draw));
        assert_eq!(
            op_of(&Command::DrawIndexed(<DrawIndexed as Wire>::ZERO)),
            Some(Op::DrawIndexed)
        );
        assert_eq!(
            op_of(&Command::DispatchMesh(<DispatchMesh as Wire>::ZERO)),
            Some(Op::Mesh)
        );
        assert_eq!(
            op_of(&Command::Dispatch(<Dispatch as Wire>::ZERO)),
            Some(Op::Dispatch)
        );
        assert_eq!(op_of(&Command::DebugLabel(b"x")), None);
        let set = Command::SetPipeline(SetPipeline {
            pso: PsoId(9),
            _pad: 0,
        });
        assert_eq!(pipeline_of(&set), Some(PsoId(9)));
        assert_eq!(op_of(&set), None);
        assert_eq!(pipeline_of(&Command::Draw(<Draw as Wire>::ZERO)), None);
    }

    #[test]
    fn an_indirect_execution_is_a_draw_or_a_dispatch_by_its_action() {
        let mut dispatch = <IndirectArg as Wire>::ZERO;
        dispatch.kind = IndirectArgKind::DISPATCH;
        let mut draw = <IndirectArg as Wire>::ZERO;
        draw.kind = IndirectArgKind::DRAW;
        let mut scratch = Vec::new();
        let sig = ExecuteIndirect::encode_signature(&[dispatch], &mut scratch).to_vec();
        let mut e = ExecuteIndirect {
            args: crate::ids::ResId(1),
            args_offset: 0,
            count: crate::ids::ResId::NULL,
            count_offset: 0,
            max_count: 1,
            stride: 12,
            root_sig: crate::ids::RootSigId::NULL,
            signature: &sig,
        };
        assert_eq!(
            op_of(&Command::ExecuteIndirect(e)),
            Some(Op::IndirectDispatch)
        );
        let mut scratch = Vec::new();
        let sig = ExecuteIndirect::encode_signature(&[draw], &mut scratch).to_vec();
        e.signature = &sig;
        assert_eq!(op_of(&Command::ExecuteIndirect(e)), Some(Op::IndirectDraw));
    }
}

#[cfg(test)]
mod tracker_tests {
    use super::*;

    #[test]
    fn a_failure_loses_the_open_list_and_the_uncommitted_ones_only() {
        let mut t = BlockTracker::default();
        let l = Label::NONE;
        // list 1 on queue 7 ends, nothing committed it yet
        t.begin_list();
        t.ran(Op::Draw, l);
        t.end_list(7);
        t.sync(|q| usize::from(q == 7));
        // list 2 on queue 7 ends; the back committed the queue at the begin of list 3
        t.begin_list();
        t.ran(Op::Dispatch, l);
        t.end_list(7);
        t.sync(|q| if q == 7 { 2 } else { 0 });
        t.begin_list();
        t.sync(|_| 0);
        t.ran(Op::Mesh, l);
        // a command in list 3 failed: only the open list's op is lost
        assert_eq!(t.lost(), [(Op::Mesh, l)]);
        assert!(t.lost().is_empty());
    }

    #[test]
    fn uncommitted_lists_are_lost_with_the_open_one() {
        let mut t = BlockTracker::default();
        let l = Label::NONE;
        t.begin_list();
        t.ran(Op::Draw, l);
        t.end_list(1);
        t.sync(|_| 1);
        t.begin_list();
        t.ran(Op::DrawIndexed, l);
        let mut lost = t.lost();
        lost.sort();
        assert_eq!(lost, [(Op::Draw, l), (Op::DrawIndexed, l)]);
    }
}
