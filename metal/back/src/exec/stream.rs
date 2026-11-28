//! The command stream (`Link::submit`): checking a block, then running it.
//!
//! **Pass 1, [`validate`]**, looks at the block as a whole and touches no
//! state: the framing (the decoder), the nesting of lists, duplicate list
//! ids, commands in the wrong place, and the opcodes and variants this back
//! does not implement. A block that fails has no effect at all.
//!
//! **Pass 2, `State::run_block`**, executes. What it can still find wrong
//! is about state (dead handles, values out of range, a descriptor that
//! names no live buffer): it stops at the first such command, drops the
//! list being recorded and the lists not yet handed to a queue, and returns
//! the status; lists already submitted stay submitted.
//!
//! Lists are recorded one after another (no nesting). Ended lists are
//! gathered per queue and committed together (one [`Queue::submit`]) before
//! a `QUEUE_SIGNAL` / `QUEUE_WAIT` on that queue and at the end of the block.
//! That is one `ExecuteCommandLists`.

use std::collections::HashSet;
use std::sync::Arc;

use victory_metal_proto::cmd::{
    BeginList, Command, CopyBufferRegion, CopyTextureRegion, Dispatch, FillBuffer, QueueSignal,
    QueueWait, SetDescriptorHeaps, SetPipeline, SetRootConstants, SetRootDescriptor,
    SetRootSignature, SetRootTable,
};
use victory_metal_proto::ids::{HeapId, PsoId, QueueId};
use victory_metal_proto::indirect::IndirectArgKind;
use victory_metal_proto::status::Status;
use victory_metal_proto::stream::Decoder;
use victory_metal_proto::types::{BarrierClass, BindPoint, RootParamKind, format};

use super::tlab::SlotKind;
use super::{Pso, PsoBody, RootSig, State, status_of};
use crate::queue::Queue;
use crate::record::CommandList;

/// Checks everything about a block that does not need the object tables.
/// `VICTORY_METAL_LOG=1`: an error that is about to become a bare status.
pub(super) fn logged(what: &str, e: &crate::error::Error) -> Status {
    if std::env::var_os("VICTORY_METAL_LOG").is_some_and(|v| v != "0") {
        eprintln!("victory-metal back: {what}: {e}");
    }
    status_of(e)
}

/// `VICTORY_METAL_LOG=1`: what the validation pass refused.
fn refused(what: &str) {
    if std::env::var_os("VICTORY_METAL_LOG").is_some_and(|v| v != "0") {
        eprintln!(
            "victory-metal back: a command of the stream is refused: {what} is not implemented"
        );
    }
}

/// Returns the stream's capability bits.
pub(super) fn validate(block: &[u8]) -> Result<victory_metal_proto::Caps, Status> {
    let decoder = Decoder::new(block).map_err(|e| e.status())?;
    let caps = decoder.header().caps;
    let mut open: Option<u32> = None;
    let mut seen = HashSet::new();
    for c in decoder {
        let c = c.map_err(|e| {
            if std::env::var_os("VICTORY_METAL_LOG").is_some_and(|v| v != "0") {
                eprintln!("victory-metal back: a command of the stream is refused: {e}");
            }
            e.status()
        })?;
        match &c {
            Command::BeginList(b) => {
                if open.is_some() || b.list.is_null() || !seen.insert(b.list.0) {
                    return Err(Status::BAD_STREAM);
                }
                open = Some(b.list.0);
            }
            Command::EndList(e) => {
                if open != Some(e.list.0) {
                    return Err(Status::BAD_STREAM);
                }
                open = None;
            }
            Command::QueueSignal(_) | Command::QueueWait(_) => {
                if open.is_some() {
                    return Err(Status::BAD_STREAM);
                }
            }
            Command::DebugLabel(_) => {}
            // Not implemented by this back (the status is the same wherever
            // they stand, so they are refused before anything runs).
            Command::SyncRecord(_) | Command::BufferData(_) | Command::DispatchIndirect(_) => {
                refused("SyncRecord, BufferData or DispatchIndirect");
                return Err(Status::UNSUPPORTED);
            }
            other => {
                if open.is_none() {
                    return Err(Status::BAD_STREAM);
                }
                match other {
                    Command::SetRootSignature(c) if !known_bind_point(c.bind_point) => {
                        return Err(Status::UNSUPPORTED);
                    }
                    Command::SetRootConstants(c) if !known_bind_point(c.bind_point) => {
                        return Err(Status::UNSUPPORTED);
                    }
                    Command::SetRootDescriptor(c) if !known_bind_point(c.bind_point) => {
                        return Err(Status::UNSUPPORTED);
                    }
                    Command::SetRootTable(c) if !known_bind_point(c.bind_point) => {
                        return Err(Status::UNSUPPORTED);
                    }
                    // Draws, mesh dispatch, vertex and index buffers: waiting for the render encoder.
                    Command::ExecuteIndirect(c)
                        if c.arguments()
                            .any(|a| !super::indirect::is_supported_arg(a.kind)) =>
                    {
                        let kinds: Vec<_> = c.arguments().map(|a| a.kind).collect();
                        refused(&format!(
                            "ExecuteIndirect with vertex or index buffer views, or rays (stride {}, max {}, count {}, arguments {kinds:?})",
                            c.stride,
                            c.max_count,
                            !c.count.is_null()
                        ));
                        return Err(Status::UNSUPPORTED);
                    }
                    Command::Barrier(b) if b.class == BarrierClass::LAYOUT => {
                        refused("a layout barrier");
                        return Err(Status::UNSUPPORTED);
                    }
                    _ => {}
                }
            }
        }
    }
    if open.is_some() {
        return Err(Status::BAD_STREAM);
    }
    Ok(caps)
}

/// What a list has bound so far: the shadow of its argument buffer and the
/// objects a dispatch needs.
#[derive(Default, Clone)]
pub(super) struct Side {
    pub(super) sig: Option<Arc<RootSig>>,
    /// The argument buffer as the stream has set it (root constants,
    /// pointers), `layout.size` bytes.
    pub(super) shadow: Vec<u8>,
    /// Shadow changed since the last dispatch or draw wrote it to the ring.
    pub(super) dirty: bool,
}

impl Side {
    /// `SetRootSignature`: a new signature starts a zeroed argument buffer.
    pub(super) fn set_signature(&mut self, sig: Arc<RootSig>) {
        // Setting the signature that is already bound changes nothing.
        if self.sig.as_ref().is_some_and(|s| Arc::ptr_eq(s, &sig)) {
            return;
        }
        self.shadow = vec![0; sig.layout.size() as usize];
        // The pointer to the static-sampler table MSC appends is the one
        // entry no root parameter sets.
        if let (Some(at), Some(table)) = (sig.layout.static_table_offset(), &sig.static_table) {
            let at = at as usize;
            self.shadow[at..at + 8].copy_from_slice(&table.gpu_va().to_le_bytes());
        }
        self.sig = Some(sig);
        self.dirty = true;
    }

    pub(super) fn set_constants(&mut self, c: &SetRootConstants<'_>) -> R {
        let sig = self.sig.clone().ok_or(Status::INVALID_ARG)?;
        let slot = sig.layout.slot(c.root_index).ok_or(Status::INVALID_ARG)?;
        let SlotKind::Constants { count } = slot.kind else {
            return Err(Status::INVALID_ARG);
        };
        let end = u64::from(c.dest_offset) + c.count() as u64;
        if end > u64::from(count) {
            return Err(Status::INVALID_ARG);
        }
        let at = (slot.offset + 4 * c.dest_offset) as usize;
        self.shadow[at..at + c.data.len()].copy_from_slice(c.data);
        self.dirty = true;
        Ok(())
    }

    /// `SetRootDescriptor` (the address was checked by the caller).
    pub(super) fn set_descriptor(&mut self, c: &SetRootDescriptor) -> R {
        let sig = self.sig.clone().ok_or(Status::INVALID_ARG)?;
        let slot = sig.layout.slot(c.root_index).ok_or(Status::INVALID_ARG)?;
        match (slot.kind, c.kind) {
            (SlotKind::Descriptor(k), kind) if k == kind => {}
            _ => return Err(Status::INVALID_ARG),
        }
        debug_assert!(matches!(
            c.kind,
            RootParamKind::CBV | RootParamKind::SRV | RootParamKind::UAV
        ));
        let at = slot.offset as usize;
        self.shadow[at..at + 8].copy_from_slice(&c.gpu_va.to_le_bytes());
        self.dirty = true;
        Ok(())
    }

    /// `SetRootTable` of the heap entry at `va`.
    pub(super) fn set_table(&mut self, c: &SetRootTable, va: u64, heap_is_sampler: bool) -> R {
        let sig = self.sig.clone().ok_or(Status::INVALID_ARG)?;
        let slot = sig.layout.slot(c.root_index).ok_or(Status::INVALID_ARG)?;
        match slot.kind {
            SlotKind::Table { sampler } if sampler == heap_is_sampler => {}
            _ => return Err(Status::INVALID_ARG),
        }
        let at = slot.offset as usize;
        self.shadow[at..at + 8].copy_from_slice(&va.to_le_bytes());
        self.dirty = true;
        Ok(())
    }
}

/// Compute and graphics have a root signature, root arguments and a
/// pipeline each (as in the original API); they share the argument table's
/// slot 2, so whoever wrote it last owns it (`owner`).
#[derive(Default)]
pub(super) struct Bound {
    pub(super) compute: Side,
    pub(super) graphics: Side,
    pub(super) pso: Option<Arc<Pso>>,
    pub(super) gpso: Option<Arc<Pso>>,
    /// The protocol id of `gpso` (for the profiler's pipeline labels).
    gpso_id: u32,
    /// `Some(true)`: the argument table's slot 2 holds the graphics arguments.
    pub(super) owner: Option<bool>,
    /// What the pre-scan of the open render pass prepared for the mesh
    /// draws that are emulated, in order (`exec/emu.rs`).
    pub(super) emu: std::collections::VecDeque<super::emu::Prepared>,
    /// The descriptor heaps bound: `(cbv_srv_uav, sampler)` heap ids (null:
    /// none yet).
    pub(super) heaps: (u32, u32),
}

fn known_bind_point(b: BindPoint) -> bool {
    b == BindPoint::COMPUTE || b == BindPoint::GRAPHICS
}

pub(super) fn side_of(bound: &mut Bound, b: BindPoint) -> Result<&mut Side, Status> {
    match b {
        BindPoint::COMPUTE => Ok(&mut bound.compute),
        BindPoint::GRAPHICS => Ok(&mut bound.graphics),
        _ => Err(Status::UNSUPPORTED),
    }
}

/// A queue with the lists ended for it and not yet committed.
struct Batch {
    queue_id: u32,
    queue: Queue,
    lists: Vec<CommandList>,
}

pub(super) struct Run<'a> {
    pub(super) st: &'a mut State,
    batches: Vec<Batch>,
    /// The list being recorded.
    pub(super) open: Option<(CommandList, QueueId, Bound)>,
}

/// `VICTORY_METAL_STEP=1`: every command is printed before it runs, so the
/// last one printed is the one a crash of the process happened in.
fn step_log() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("VICTORY_METAL_STEP").is_some_and(|v| v != "0");
        if on {
            crash_report();
        }
        on
    })
}

/// The handler that was there before ours, to pass the signal on to.
static PREVIOUS: [std::sync::atomic::AtomicUsize; 2] =
    [const { std::sync::atomic::AtomicUsize::new(0) }; 2];

/// Prints the faulting address and the call stack of a `SIGSEGV` / `SIGBUS`
/// in the process (for `VICTORY_METAL_STEP`), then hands the signal on.
fn crash_report() {
    extern "C" fn handler(sig: libc::c_int, info: *mut libc::siginfo_t, ctx: *mut libc::c_void) {
        let mut frames = [std::ptr::null_mut::<libc::c_void>(); 48];
        // SAFETY: a diagnostic in a process that is about to fault anyway.
        unsafe {
            let n = libc::backtrace(frames.as_mut_ptr(), 48);
            let addr = if info.is_null() {
                std::ptr::null_mut()
            } else {
                (*info).si_addr
            };
            let msg = format!("victory-metal crash: signal {sig} at address {addr:p}\n");
            libc::write(2, msg.as_ptr().cast(), msg.len());
            libc::backtrace_symbols_fd(frames.as_ptr(), n, 2);
            let i = usize::from(sig == libc::SIGBUS);
            let prev = PREVIOUS[i].load(std::sync::atomic::Ordering::Relaxed);
            if prev > 1 {
                let f: extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void) =
                    std::mem::transmute(prev);
                f(sig, info, ctx);
            }
        }
    }
    for (i, sig) in [libc::SIGSEGV, libc::SIGBUS].into_iter().enumerate() {
        // SAFETY: installs a handler with the platform's `sigaction`.
        unsafe {
            let mut old: libc::sigaction = std::mem::zeroed();
            let mut new: libc::sigaction = std::mem::zeroed();
            new.sa_sigaction = handler as *const () as usize;
            new.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
            libc::sigemptyset(&mut new.sa_mask);
            if libc::sigaction(sig, &new, &mut old) == 0 {
                PREVIOUS[i].store(old.sa_sigaction, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

/// Work commands that refuse before they record anything into the open list.
fn is_isolable(c: &Command<'_>) -> bool {
    matches!(
        c,
        Command::CopyTextureRegion(_)
            | Command::CopyResource(_)
            | Command::CopyBufferRegion(_)
            | Command::FillBuffer(_)
            | Command::ClearUav(_)
            | Command::ClearUavTexture(_)
            | Command::ExecuteIndirect(_)
            | Command::Dispatch(_)
            | Command::DispatchMesh(_)
            | Command::Draw(_)
            | Command::DrawIndexed(_)
            | Command::WriteImmediate(_)
    )
}

impl State {
    /// Runs a block that [`validate`] accepted.
    pub(super) fn run_block(&mut self, block: &[u8]) -> Status {
        let Ok(mut decoder) = Decoder::new(block) else {
            return Status::BAD_STREAM;
        };
        let mut run = Run {
            st: self,
            batches: Vec::new(),
            open: None,
        };
        let mut failed: Option<(Status, String)> = None;
        // The first command that failed alone (see `ExecOptions::strict_block`): the block
        // runs on, then answers with its status so the caller can log it.
        let mut first_isolated: Option<(Status, String)> = None;
        run.st.note = None;
        // The drop statistics (`VICTORY_METAL_DROPSTATS`): ops counted as they
        // run, and what a failure loses with them.
        let stats = crate::dropstats::get();
        let mut counted = Counted::default();
        // `VICTORY_METAL_GPUPROF`: the pipeline the open list has set.
        let mut prof_pso = PsoId::NULL;
        while let Some(c) = decoder.next() {
            let Ok(c) = c else {
                return Status::BAD_STREAM;
            };
            if step_log() {
                eprintln!("victory-metal step: {c:?}");
            }
            let before = stats.map(|s| counted.before(s, &c));
            let result = run.command(&c);
            if crate::prof::config().is_some() || crate::fault::enabled() {
                match &c {
                    Command::BeginList(_) => prof_pso = PsoId::NULL,
                    Command::SetPipeline(p) => prof_pso = p.pso,
                    _ => {}
                }
                if result.is_ok() {
                    run.prof_after(&c, prof_pso);
                }
            }
            // A render pass with emulated mesh draws: their compute work is
            // encoded now, before the pass's render encoder (`exec/emu.rs`).
            if result.is_ok()
                && matches!(c, Command::BeginRenderPass(_))
                && run.st.emulations > 0
                && run.st.options.mesh_compute
            {
                run.prescan(decoder.clone());
            }
            if let (Some(s), Some(before)) = (stats, before) {
                counted.after(s, &c, before, &result, &run);
            }
            if let Err(s) = result {
                // A refused piece of work (a copy, clear, draw, dispatch the back cannot do)
                // fails alone: it is logged and counted, and the block goes on with the list
                // still open. The command refuses before it records anything; structural
                // commands (lists, queues, pipeline and pass state) still stop the block, as
                // does everything with `ExecOptions::strict_block` (VICTORY_METAL_STRICT_BLOCK=1).
                if !run.st.options.strict_block
                    && matches!(s, Status::UNSUPPORTED | Status::INVALID_ARG)
                    && run.open.is_some()
                    && is_isolable(&c)
                {
                    let why = run
                        .st
                        .note
                        .take()
                        .unwrap_or_else(|| "no reason recorded".into());
                    run.st.isolated += 1;
                    if first_isolated.is_none() {
                        first_isolated = Some((s, format!("{:?}: {why}", c.opcode())));
                    }
                    if std::env::var_os("VICTORY_METAL_LOG").is_some_and(|v| v != "0") {
                        eprintln!(
                            "victory-metal back: {:?} failed alone: {s:?}: {why} (the block goes on; isolated so far {})",
                            c.opcode(),
                            run.st.isolated
                        );
                    }
                    continue;
                }
                if std::env::var_os("VICTORY_METAL_LOG").is_some_and(|v| v != "0") {
                    // `note` still holds the reason a refusal recorded (the
                    // block's own note below replaces it).
                    let why = run.st.note.as_deref().unwrap_or("no reason recorded");
                    eprintln!("victory-metal back: {:?} failed: {s:?}: {why}", c.opcode());
                }
                // The list being recorded is lost with the failed command. A fence the title
                // waits for that is signalled further down this block still has to be: a
                // signal that is never given leaves the title waiting for ever (0% CPU, no
                // log), where a lost list only costs a picture. The signals wait for the lists
                // of the block that were already finished.
                run.open = None;
                for later in decoder.clone() {
                    if let Ok(Command::QueueSignal(q)) = later {
                        let given = run.queue_signal(&q);
                        if std::env::var_os("VICTORY_METAL_LOG").is_some_and(|v| v != "0") {
                            eprintln!(
                                "victory-metal back: after {:?} failed, the fence signal of queue {} fence {} value {} is still given: {given:?}",
                                c.opcode(),
                                q.queue.0,
                                q.fence.0,
                                q.value,
                            );
                        }
                    }
                }
                if let Some(st) = stats {
                    let why = run.st.note.clone().unwrap_or_else(|| format!("{s:?}"));
                    counted.stopped(
                        st,
                        &format!("block stopped at {:?}: {why}", c.opcode()),
                        decoder.by_ref(),
                    );
                }
                // Dropping `run` drops the unflushed lists.
                failed = Some((s, format!("{c:?}")));
                break;
            }
        }
        let result = match failed {
            Some((s, what)) => Err((s, what)),
            None => run.flush_all().map_err(|s| (s, "commit".to_string())),
        };
        if let (Some(st), Err((s, what))) = (stats, &result)
            && what == "commit"
        {
            // The lists were run in the stream but their commit failed.
            counted.lose_uncommitted(st, &format!("commit failed: {s:?}"));
        }
        drop(run);
        match result {
            Ok(()) => match first_isolated {
                Some((s, what)) => {
                    self.note = Some(format!(
                        "{what} (it failed alone, the rest of the block ran)"
                    ));
                    s
                }
                None => Status::OK,
            },
            Err((s, what)) => {
                // The wire carries a status only; `ExecuteCommandLists` has no
                // result at all, so the command that stopped the block is
                // kept for `VICTORY_METAL_LOG` and `Executor::last_error`.
                self.note = Some(format!("block stopped with {s:?} at {what}"));
                s
            }
        }
    }
}

/// A block that was refused before it ran (or never reached `run_block`):
/// every op in it is submitted and dropped, for `why`.
pub(super) fn count_refused_block(block: &[u8], why: &str) {
    use victory_metal_proto::dropstats::{op_of, pipeline_of};
    let Some(stats) = crate::dropstats::get() else {
        return;
    };
    let Ok(decoder) = Decoder::new(block) else {
        return;
    };
    let mut pso = PsoId::NULL;
    for c in decoder.flatten() {
        if matches!(c, Command::BeginList(_)) {
            pso = PsoId::NULL;
        }
        if let Some(p) = pipeline_of(&c) {
            pso = p;
        }
        if let Some(op) = op_of(&c) {
            let label = stats.label_of(pso.0);
            stats.submit(op, label);
            stats.drop_ops(op, label, why, 1);
        }
    }
}

/// What one block's statistics need between commands.
#[derive(Default)]
struct Counted {
    pso: PsoId,
    queue: u32,
    tracker: victory_metal_proto::dropstats::BlockTracker,
}

/// What `Counted::before` saw of a command.
struct Before {
    op: Option<(
        victory_metal_proto::dropstats::Op,
        victory_metal_proto::dropstats::Label,
    )>,
}

impl Counted {
    /// A command is about to run: an op is submitted with the bound pipeline.
    fn before(&mut self, stats: &crate::dropstats::Collector, c: &Command<'_>) -> Before {
        use victory_metal_proto::dropstats::{op_of, pipeline_of};
        if let Command::BeginList(_) = c {
            self.pso = PsoId::NULL;
            self.tracker.begin_list();
        }
        if let Command::BeginList(b) = c {
            self.queue = b.queue.0;
        }
        if let Some(p) = pipeline_of(c) {
            self.pso = p;
        }
        let op = op_of(c).map(|op| (op, stats.label_of(self.pso.0)));
        if let Some((op, label)) = op {
            stats.submit(op, label);
            // A skip noted by an earlier command must not be taken for this one.
            crate::dropstats::take_skip();
        }
        Before { op }
    }

    /// The command ran (or failed): drops, no-ops, the lists' bookkeeping.
    fn after(
        &mut self,
        stats: &crate::dropstats::Collector,
        c: &Command<'_>,
        before: Before,
        result: &R,
        run: &Run<'_>,
    ) {
        if let Some((op, label)) = before.op {
            match result {
                Ok(()) => match crate::dropstats::take_skip() {
                    Some(why) => stats.drop_ops(op, label, &why, 1),
                    None => {
                        match c {
                            Command::Draw(d) if d.vertex_count == 0 || d.instance_count == 0 => {
                                stats.noop()
                            }
                            Command::DrawIndexed(d)
                                if d.index_count == 0 || d.instance_count == 0 =>
                            {
                                stats.noop()
                            }
                            Command::DispatchMesh(d) if d.x == 0 || d.y == 0 || d.z == 0 => {
                                stats.noop()
                            }
                            _ => {}
                        }
                        self.tracker.ran(op, label);
                    }
                },
                Err(s) => {
                    let why = run.st.note.clone().unwrap_or_else(|| format!("{s:?}"));
                    stats.drop_ops(op, label, &format!("{:?} failed: {why}", c.opcode()), 1);
                }
            }
        }
        if result.is_ok()
            && let Command::EndList(_) = c
        {
            self.tracker.end_list(self.queue);
        }
        self.tracker.sync(|q| {
            run.batches
                .iter()
                .find(|b| b.queue_id == q)
                .map_or(0, |b| b.lists.len())
        });
    }

    /// A command stopped the block: the ops of the uncommitted lists and every
    /// op still to come are lost.
    fn stopped<'a>(
        &mut self,
        stats: &crate::dropstats::Collector,
        why: &str,
        rest: impl Iterator<Item = Result<Command<'a>, victory_metal_proto::Error>>,
    ) {
        self.lose_uncommitted(stats, &format!("lost with its block: {why}"));
        use victory_metal_proto::dropstats::{op_of, pipeline_of};
        for c in rest.flatten() {
            if let Some(p) = pipeline_of(&c) {
                self.pso = p;
            }
            if matches!(c, Command::BeginList(_)) {
                self.pso = PsoId::NULL;
            }
            if let Some(op) = op_of(&c) {
                let label = stats.label_of(self.pso.0);
                stats.submit(op, label);
                stats.drop_ops(op, label, &format!("not run after a stop: {why}"), 1);
            }
        }
    }

    fn lose_uncommitted(&mut self, stats: &crate::dropstats::Collector, why: &str) {
        for (op, label) in self.tracker.lost() {
            stats.drop_ops(op, label, why, 1);
        }
    }
}

pub(super) type R = Result<(), Status>;

impl Run<'_> {
    /// What a resource is, for a line of the GPU time table.
    fn prof_res(&self, id: victory_metal_proto::ids::ResId) -> String {
        match self.st.resources.get(id.0) {
            Some(super::Res::Texture(t)) => {
                let d = t.desc();
                format!(
                    "tex{:?} f{} {}x{}x{} m{}",
                    d.dim, d.format, d.width, d.height, d.depth_or_array, d.mips
                )
            }
            Some(super::Res::Buffer(b)) => format!("buf {}", b.len()),
            None => "?".into(),
        }
    }

    /// `VICTORY_METAL_GPUPROF`: the command `c` ran; if it is an op the table names, a timestamp
    /// goes after it, named by what it did.
    fn prof_after(&mut self, c: &Command<'_>, pso: PsoId) {
        // The journal of a failed submission (`VICTORY_METAL_LOG`, `crate::fault`): pipelines as
        // they are set, dispatches, and the kinds of copy; draws are the pipeline's.
        if crate::fault::enabled()
            && let Some((list, _, _)) = self.open.as_mut()
        {
            match c {
                Command::SetPipeline(_) | Command::Dispatch(_) | Command::ExecuteIndirect(_) => {
                    let kind = match c {
                        Command::SetPipeline(_) => "set",
                        Command::Dispatch(_) => "dispatch",
                        _ => "execute-indirect",
                    };
                    list.journal_note(|| format!("{kind} {}", crate::prof::pipeline_label(pso.0)));
                }
                Command::CopyBufferRegion(_) => list.journal_note(|| "copy buf->buf".into()),
                Command::CopyResource(_) | Command::CopyTextureRegion(_) => {
                    list.journal_note(|| "copy texture".into());
                }
                Command::ClearUav(_) | Command::ClearUavTexture(_) | Command::FillBuffer(_) => {
                    list.journal_note(|| "clear/fill".into());
                }
                _ => {}
            }
        }
        let Some(cfg) = crate::prof::config() else {
            return;
        };
        let label = match c {
            Command::Dispatch(_) => Some(crate::prof::pipeline_label(pso.0)),
            Command::ExecuteIndirect(e) => Some(format!(
                "execute-indirect {} args [{}]+{} stride {} max {} count [{}]+{}{}",
                crate::prof::pipeline_label(pso.0),
                self.prof_res(e.args),
                e.args_offset,
                e.stride,
                e.max_count,
                if e.count.is_null() {
                    "none".to_string()
                } else {
                    self.prof_res(e.count)
                },
                e.count_offset,
                match self.open.as_mut().and_then(|(l, _, _)| l.last_snap.take()) {
                    Some(n) => format!(" snap#{n}"),
                    None => String::new(),
                }
            )),
            Command::CopyBufferRegion(c) => Some(format!("copy buf->buf {} bytes", c.size)),
            Command::FillBuffer(c) => Some(format!("fill buf {} bytes", c.size)),
            Command::ClearUav(c) => Some(format!("clear-uav buf f{} {} bytes", c.format, c.size)),
            Command::ClearUavTexture(c) => Some(format!(
                "clear-uav tex f{} [{}]",
                c.format,
                self.prof_res(c.res)
            )),
            Command::CopyResource(c) => Some(format!(
                "copy-resource [{}] <- [{}]",
                self.prof_res(c.dst),
                self.prof_res(c.src)
            )),
            Command::CopyTextureRegion(c) => {
                use victory_metal_proto::types::LocKind;
                let side = |l: &victory_metal_proto::cmd::TextureLocation| {
                    if l.kind == LocKind::SUBRESOURCE {
                        format!(
                            "{} mip{} s{} p{}",
                            self.prof_res(l.res),
                            l.mip,
                            l.slice,
                            l.plane
                        )
                    } else {
                        format!(
                            "footprint f{} {}x{}x{}",
                            l.format, l.width, l.height, l.depth
                        )
                    }
                };
                Some(format!(
                    "copy-texture [{}] <- [{}]",
                    side(&c.dst),
                    side(&c.src)
                ))
            }
            Command::Draw(_) | Command::DrawIndexed(_) | Command::DispatchMesh(_) => {
                if let Some((list, _, _)) = self.open.as_mut()
                    && let Some(p) = list.prof.as_mut()
                {
                    p.draws += 1;
                }
                cfg.draws
                    .then(|| format!("draw {}", crate::prof::pipeline_label(pso.0)))
            }
            _ => None,
        };
        let Some(label) = label else { return };
        if let Some((list, _, _)) = self.open.as_mut() {
            // Ops inside a render pass belong to the pass's scope (unless draws are sampled).
            if list.render.is_none() || cfg.draws {
                list.prof_sample(|| label);
            }
        }
    }
}

impl Run<'_> {
    fn command(&mut self, c: &Command<'_>) -> R {
        match c {
            Command::BeginList(c) => self.begin_list(c),
            Command::EndList(_) => self.end_list(),
            Command::QueueSignal(c) => self.queue_signal(c),
            Command::QueueWait(c) => self.queue_wait(c),
            // Labels have no Metal 4 counterpart wired up yet: accepted, dropped.
            Command::DebugLabel(_) => Ok(()),
            Command::SetPipeline(c) => self.set_pipeline(c),
            Command::SetRootSignature(c) => self.set_root_signature(c),
            Command::SetDescriptorHeaps(c) => self.set_descriptor_heaps(c),
            Command::SetRootConstants(c) => self.set_root_constants(c),
            Command::SetRootDescriptor(c) => self.set_root_descriptor(c),
            Command::SetRootTable(c) => self.set_root_table(c),
            Command::Dispatch(c) => self.dispatch(c),
            Command::ExecuteIndirect(c) => self.execute_indirect(c),
            Command::CopyBufferRegion(c) => self.copy_buffer_region(c),
            Command::FillBuffer(c) => self.fill_buffer(c),
            Command::ClearUav(c) => self.clear_uav(c),
            Command::Barrier(c) => self.barrier(c),
            // Refused by `validate`; kept as a status rather than a panic.
            Command::CopyTextureRegion(c) => self.copy_texture_region(c),
            Command::CopyResource(c) => self.copy_resource(c),
            Command::ClearUavTexture(c) => self.clear_uav_texture(c),
            Command::WriteImmediate(c) => self.write_immediate(c),
            Command::BeginRenderPass(c) => self.begin_render_pass(c),
            Command::EndRenderPass(c) => self.end_render_pass(c),
            Command::SetViewports(c) => self.set_viewports(c),
            Command::SetScissors(c) => self.set_scissors(c),
            Command::SetBlendFactor(c) => self.render_state(|l| l.set_blend_factor(c.color)),
            Command::SetStencilRef(c) => self.render_state(|l| l.set_stencil_ref(c.value)),
            Command::SetPrimitiveTopology(c) => self.set_primitive(c),
            Command::SetIndexBuffer(c) => self.set_index_buffer(c),
            Command::SetVertexBuffers(c) => self.set_vertex_buffers(c),
            Command::Draw(c) => self.draw(c),
            Command::DrawIndexed(c) => self.draw_indexed(c),
            Command::DispatchMesh(c) => self.dispatch_mesh(c),
            Command::ClearAttachment(c) => self.clear_attachment(c),
            Command::SyncRecord(_) | Command::BufferData(_) | Command::DispatchIndirect(_) => {
                Err(Status::UNSUPPORTED)
            }
        }
    }

    pub(super) fn list(&mut self) -> Result<(&mut CommandList, &mut Bound), Status> {
        match self.open.as_mut() {
            Some((l, _, b)) => Ok((l, b)),
            None => Err(Status::BAD_STREAM),
        }
    }

    /// Commits the lists ended for one queue.
    fn flush(&mut self, queue_id: u32) -> R {
        let Some(i) = self.batches.iter().position(|b| b.queue_id == queue_id) else {
            return Ok(());
        };
        let batch = self.batches.remove(i);
        self.commit(batch)
    }

    fn commit(&mut self, batch: Batch) -> R {
        let result = if batch.lists.is_empty() {
            Ok(())
        } else {
            batch.queue.submit(batch.lists).map(|_| ()).map_err(|e| {
                if std::env::var_os("VICTORY_METAL_LOG").is_some_and(|v| v != "0") {
                    eprintln!("victory-metal back: committing the lists failed: {e}");
                }
                status_of(&e)
            })
        };
        // Every submit ends with maintain.
        self.st.device.maintain();
        result
    }

    fn flush_all(&mut self) -> R {
        let batches = std::mem::take(&mut self.batches);
        let mut first = Ok(());
        for b in batches {
            let r = self.commit(b);
            if first.is_ok() {
                first = r;
            }
        }
        first
    }

    fn begin_list(&mut self, c: &BeginList) -> R {
        if c.flags != 0 {
            return Err(Status::INVALID_ARG);
        }
        let queue = self
            .st
            .queues
            .get(c.queue.0)
            .ok_or(Status::NOT_FOUND)?
            .clone();
        // A queue's pool of command buffers is finite, and `begin_list` waits
        // when it is used up: commit what is waiting before that can happen.
        let limit = (self.st.options.queue.max_lists / 2).max(1) as usize;
        if self
            .batches
            .iter()
            .find(|b| b.queue_id == c.queue.0)
            .is_some_and(|b| b.lists.len() >= limit)
        {
            self.flush(c.queue.0)?;
        }
        let mut list = queue
            .begin_list()
            .map_err(|e| logged("begin of a list", &e))?;
        // Argument-table slots keep the addresses of the list that used them
        // last: clear the three binding points the stream sets.
        for point in [
            crate::descriptors::bind_point::DESCRIPTOR_HEAP,
            crate::descriptors::bind_point::SAMPLER_HEAP,
            crate::descriptors::bind_point::ARGUMENT_BUFFER,
        ] {
            list.set_address(point as u32, 0)
                .map_err(|e| status_of(&e))?;
        }
        self.open = Some((list, c.queue, Bound::default()));
        Ok(())
    }

    fn end_list(&mut self) -> R {
        let (mut list, queue_id, _) = self.open.take().ok_or(Status::BAD_STREAM)?;
        list.end().map_err(|e| logged("end of a list", &e))?;
        let queue = self
            .st
            .queues
            .get(queue_id.0)
            .ok_or(Status::NOT_FOUND)?
            .clone();
        match self.batches.iter_mut().find(|b| b.queue_id == queue_id.0) {
            Some(b) => b.lists.push(list),
            None => self.batches.push(Batch {
                queue_id: queue_id.0,
                queue,
                lists: vec![list],
            }),
        }
        Ok(())
    }

    fn queue_signal(&mut self, c: &QueueSignal) -> R {
        let queue = self
            .st
            .queues
            .get(c.queue.0)
            .ok_or(Status::NOT_FOUND)?
            .clone();
        let fence = self
            .st
            .fences
            .get(c.fence.0)
            .ok_or(Status::NOT_FOUND)?
            .clone();
        self.flush(c.queue.0)?;
        queue.signal(&fence, c.value);
        Ok(())
    }

    fn queue_wait(&mut self, c: &QueueWait) -> R {
        let queue = self
            .st
            .queues
            .get(c.queue.0)
            .ok_or(Status::NOT_FOUND)?
            .clone();
        let fence = self
            .st
            .fences
            .get(c.fence.0)
            .ok_or(Status::NOT_FOUND)?
            .clone();
        self.flush(c.queue.0)?;
        queue.wait_fence(&fence, c.value);
        Ok(())
    }

    fn set_pipeline(&mut self, c: &SetPipeline) -> R {
        let pso = self.st.psos.get(c.pso.0).ok_or(Status::NOT_FOUND)?.clone();
        let (_, bound) = self.list()?;
        match pso.body {
            PsoBody::Compute { .. } => bound.pso = Some(pso),
            PsoBody::Graphics(_) | PsoBody::Emulated(_) => {
                bound.gpso = Some(pso);
                bound.gpso_id = c.pso.0;
            }
        }
        Ok(())
    }

    fn set_root_signature(&mut self, c: &SetRootSignature) -> R {
        let sig = self
            .st
            .root_sigs
            .get(c.root_sig.0)
            .ok_or(Status::NOT_FOUND)?
            .clone();
        let (_, bound) = self.list()?;
        side_of(bound, c.bind_point)?.set_signature(sig);
        Ok(())
    }

    fn set_descriptor_heaps(&mut self, c: &SetDescriptorHeaps) -> R {
        let find = |st: &State,
                    id: HeapId,
                    sampler: bool|
         -> Result<Option<crate::descriptors::DescriptorHeap>, Status> {
            if id.is_null() {
                return Ok(None);
            }
            let h = st.heaps.get(id.0).ok_or(Status::NOT_FOUND)?;
            // A staging heap (not shader visible) cannot be bound.
            if h.sampler != sampler || !h.shader_visible {
                return Err(Status::INVALID_ARG);
            }
            Ok(Some(h.heap.clone()))
        };
        let resources = find(self.st, c.cbv_srv_uav, false)?;
        let samplers = find(self.st, c.sampler, true)?;
        let (list, bound) = self.list()?;
        if resources.is_some() {
            bound.heaps.0 = c.cbv_srv_uav.0;
        }
        if samplers.is_some() {
            bound.heaps.1 = c.sampler.0;
        }
        let result = match (&resources, &samplers) {
            (Some(r), s) => list.set_heaps(r, s.as_ref()),
            (None, Some(s)) => list.set_sampler_heap(s),
            (None, None) => Ok(()),
        };
        result.map_err(|e| status_of(&e))
    }

    fn set_root_constants(&mut self, c: &SetRootConstants<'_>) -> R {
        let (_, bound) = self.list()?;
        side_of(bound, c.bind_point)?.set_constants(c)
    }

    fn set_root_descriptor(&mut self, c: &SetRootDescriptor) -> R {
        // A root descriptor is an address: 0 means "none", anything else
        // must lie in a live buffer that is resident (the GPU would read
        // memory Metal does not know it must keep).
        if c.gpu_va != 0 {
            match self.st.device.resolve_va(c.gpu_va, 1) {
                Some((_, e)) if e.resident => {}
                _ => return Err(Status::INVALID_ARG),
            }
        }
        let (_, bound) = self.list()?;
        side_of(bound, c.bind_point)?.set_descriptor(c)
    }

    fn set_root_table(&mut self, c: &SetRootTable) -> R {
        let heap = self.st.heaps.get(c.heap.0).ok_or(Status::NOT_FOUND)?;
        if c.index >= heap.heap.capacity() {
            return Err(Status::INVALID_ARG);
        }
        let va = heap.heap.entry_va(c.index);
        let heap_is_sampler = heap.sampler;
        let (_, bound) = self.list()?;
        side_of(bound, c.bind_point)?.set_table(c, va, heap_is_sampler)
    }

    fn dispatch(&mut self, c: &Dispatch) -> R {
        let why = match self.open.as_ref() {
            None => None,
            Some((_, _, b)) => match (&b.pso, &b.compute.sig) {
                (None, _) => Some("DISPATCH: no compute pipeline bound"),
                (_, None) => Some("DISPATCH: no compute root signature bound"),
                (Some(p), Some(s)) if !matches!(p.body, PsoBody::Compute { .. }) => {
                    let _ = s;
                    Some("DISPATCH: the bound pipeline is not a compute pipeline")
                }
                (Some(p), Some(s)) if !p.root_sig.compatible_with(s) => {
                    Some("DISPATCH: the bound pipeline was made for another root signature")
                }
                _ => None,
            },
        };
        if let Some(why) = why {
            self.st.note = Some(why.to_string());
            return Err(Status::INVALID_ARG);
        }
        let (list, bound) = self.list()?;
        let pso = bound.pso.clone().ok_or(Status::INVALID_ARG)?;
        let sig = bound.compute.sig.clone().ok_or(Status::INVALID_ARG)?;
        let PsoBody::Compute { pipeline, threads } = &pso.body else {
            // A dispatch needs a compute pipeline.
            return Err(Status::INVALID_ARG);
        };
        // The pipeline was made for one root signature (or an identical
        // one); its argument buffer layout is what the shader reads.
        if !pso.root_sig.compatible_with(&sig) {
            return Err(Status::INVALID_ARG);
        }
        write_root_arguments(list, bound, false)?;
        list.dispatch(pipeline, [c.x, c.y, c.z], *threads)
            .map_err(|e| status_of(&e))
    }

    fn execute_indirect(&mut self, c: &victory_metal_proto::indirect::ExecuteIndirect<'_>) -> R {
        if let Some(r) = self.emu_indirect_command(c) {
            return r;
        }
        let args = self.st.buffer(c.args)?.clone();
        let count = if c.count.is_null() {
            None
        } else {
            Some((self.st.buffer(c.count)?.clone(), c.count_offset))
        };
        let signature_root = if c.root_sig.is_null() {
            None
        } else {
            Some(
                self.st
                    .root_sigs
                    .get(c.root_sig.0)
                    .ok_or(Status::NOT_FOUND)?
                    .clone(),
            )
        };
        let kernels = &self.st.indirect_kernels;
        let (list, bound) = match self.open.as_mut() {
            Some((l, _, b)) => (l, b),
            None => return Err(Status::BAD_STREAM),
        };
        // The record's last argument says which side runs it.
        let graphics = c
            .arguments()
            .any(|a| a.kind.is_action() && a.kind != IndirectArgKind::DISPATCH);
        let side = if graphics { "graphics" } else { "compute" };
        let Some(sig) = (if graphics {
            bound.graphics.sig.clone()
        } else {
            bound.compute.sig.clone()
        }) else {
            self.st.note = Some(format!("EXECUTE_INDIRECT: no {side} root signature bound"));
            return Err(Status::INVALID_ARG);
        };
        let Some(pso) = (if graphics {
            bound.gpso.clone()
        } else {
            bound.pso.clone()
        }) else {
            self.st.note = Some(format!(
                "EXECUTE_INDIRECT: no {side} pipeline bound (its creation was refused, or none was set)"
            ));
            return Err(Status::INVALID_ARG);
        };
        if !pso.root_sig.compatible_with(&sig) {
            self.st.note = Some(format!(
                "EXECUTE_INDIRECT: the bound {side} pipeline was made for another root signature"
            ));
            return Err(Status::INVALID_ARG);
        }
        // The signature was made for a root signature; the list's must be
        // the same one (or an identical one).
        if let Some(made_for) = &signature_root
            && !made_for.compatible_with(&sig)
        {
            self.st.note = Some(
                "EXECUTE_INDIRECT: the command signature was made for another root signature"
                    .to_string(),
            );
            return Err(Status::INVALID_ARG);
        }
        let plan = super::indirect::plan(c, Some(&sig))?;
        // `VICTORY_METAL_INDIRECT_BARRIER=1` (a test of whether a missing barrier ahead of the
        // arguments' read is what makes one run take minutes): everything before it is done and
        // visible first.
        if indirect_barrier() {
            list.barrier(
                crate::stages::DISPATCH | crate::stages::BLIT,
                crate::stages::DISPATCH | crate::stages::BLIT,
                crate::stages::VIS_DEVICE,
            )
            .map_err(|e| status_of(&e))?;
        }
        let (side_graphics, action) = (graphics, plan.action);
        if side_graphics != (action != super::indirect::Action::Dispatch) {
            return Err(Status::INVALID_ARG);
        }
        if plan.roots.is_empty() {
            write_root_arguments(list, bound, graphics)?;
        }
        let side = if graphics {
            &bound.graphics
        } else {
            &bound.compute
        };
        use super::indirect::Action;
        let draws = matches!(action, Action::Draw | Action::DrawIndexed);
        let spec = crate::indirect::IndirectDispatch {
            args: &args,
            args_offset: c.args_offset,
            count: count.as_ref().map(|(b, off)| (b, *off)),
            max_count: c.max_count,
            stride: c.stride,
            dispatch_offset: plan.dispatch_offset,
            record_bytes: plan.record_bytes,
            roots: &plan.roots,
            base: &side.shadow,
            action_words: match action {
                Action::Draw => 4,
                Action::DrawIndexed => 5,
                _ => 3,
            },
            draws,
            index_type: list.index_ir_type(),
            consumer: if graphics {
                crate::stages::VERTEX
                    | crate::stages::OBJECT
                    | crate::stages::MESH
                    | crate::stages::FRAGMENT
            } else {
                crate::stages::DISPATCH
            },
            gs: None,
            always_expand: false,
        };
        // The kernel makes its own copies of the argument buffer; otherwise
        // the draws and dispatches read the one the list has, written as for
        // a direct one.
        let replaced = spec.replaces_root_arguments();
        // `VICTORY_METAL_SKIP_GS_INDIRECT=<hash>[,<hash>...]` skips the indirect geometry-shader
        // draws of those pipelines (the hash as the GPU time table prints it; needs GPUPROF on).
        let gpso_id = bound.gpso_id;
        let skip_gs = matches!(&pso.body, PsoBody::Graphics(g) if g.kind() == crate::render::Kind::Geometry)
            && std::env::var("VICTORY_METAL_SKIP_GS_INDIRECT").is_ok_and(|v| {
                let label = crate::prof::pipeline_label(gpso_id);
                v != "0"
                    && v != "1"
                    && v.split(',')
                        .any(|h| !h.trim().is_empty() && label.contains(h.trim()))
            });
        let r = match (&pso.body, action) {
            (PsoBody::Graphics(_), a) if skip_gs && a != Action::Dispatch => {
                crate::dropstats::skip(
                    "indirect GS draw skipped (VICTORY_METAL_SKIP_GS_INDIRECT=<hash>)",
                );
                Ok(())
            }
            (PsoBody::Compute { pipeline, threads }, Action::Dispatch) => {
                list.execute_indirect(kernels, pipeline, *threads, &spec)
            }
            (PsoBody::Emulated(_), Action::Mesh) => {
                unreachable!("an emulated mesh dispatch is handled before the plan")
            }
            (PsoBody::Graphics(g), a) if a != Action::Dispatch => {
                let kind = match a {
                    Action::Draw => crate::draw::IndirectDraw::Draw,
                    Action::DrawIndexed => crate::draw::IndirectDraw::DrawIndexed,
                    _ => crate::draw::IndirectDraw::Mesh,
                };
                // `VICTORY_METAL_GS_GROUP_CAP=<hash>:<n>[,...]`: see `prof::GS_CAP`.
                let cap = std::env::var("VICTORY_METAL_GS_GROUP_CAP")
                    .ok()
                    .filter(|_| g.kind() == crate::render::Kind::Geometry)
                    .and_then(|v| {
                        let label = crate::prof::pipeline_label(gpso_id);
                        v.split(',').find_map(|e| {
                            let (h, n) = e.split_once(':')?;
                            label
                                .contains(h.trim())
                                .then(|| n.trim().parse::<u32>().ok())?
                        })
                    });
                crate::prof::GS_CAP.with(|c| c.set(cap));
                let vis = g.kind() == crate::render::Kind::Geometry
                    && std::env::var("VICTORY_METAL_GS_VISIBILITY").is_ok_and(|v| {
                        let label = crate::prof::pipeline_label(gpso_id);
                        v.split(',')
                            .any(|h| !h.trim().is_empty() && label.contains(h.trim()))
                    });
                crate::prof::GS_VIS.with(|c| c.set(vis));
                let r = list.execute_indirect_draws(kernels, g, kind, &spec);
                crate::prof::GS_CAP.with(|c| c.set(None));
                crate::prof::GS_VIS.with(|c| c.set(false));
                r
            }
            _ => return Err(Status::INVALID_ARG),
        }
        .map_err(|e| {
            self.st.note = Some(format!("EXECUTE_INDIRECT: {e}"));
            status_of(&e)
        });
        // The argument table now points at a record's copy: the next draw or
        // dispatch writes its own.
        if replaced {
            if graphics {
                bound.graphics.dirty = true;
            } else {
                bound.compute.dirty = true;
            }
        }
        r
    }

    fn copy_buffer_region(&mut self, c: &CopyBufferRegion) -> R {
        let dst = self.st.buffer(c.dst)?.clone();
        let src = self.st.buffer(c.src)?.clone();
        let (list, _) = self.list()?;
        list.copy_buffer(&dst, c.dst_offset, &src, c.src_offset, c.size)
            .map_err(|e| status_of(&e))
    }

    fn fill_buffer(&mut self, c: &FillBuffer) -> R {
        let byte = repeated_byte(c.value).ok_or(Status::UNSUPPORTED)?;
        let dst = self.st.buffer(c.dst)?.clone();
        let (list, _) = self.list()?;
        list.fill_buffer(&dst, c.offset, c.size, byte)
            .map_err(|e| status_of(&e))
    }

    /// `WRITE_IMMEDIATE`: the value goes to a small shared buffer (written by
    /// the CPU now) and a blit copies it to the destination, so it is ordered
    /// with the list's other blits and copies. The staging buffer lives as
    /// long as the list. `MARKER_IN` / `MARKER_OUT` add a full barrier before /
    /// after the write (everything before is done / the write is done before
    /// anything after starts).
    fn write_immediate(&mut self, c: &victory_metal_proto::cmd::WriteImmediate) -> R {
        use crate::memory::BufferKind;
        use victory_metal_proto::cmd::WriteImmediate;
        use victory_metal_proto::types::stage;
        let size = u64::from(c.size);
        let dst = self.st.buffer(c.dst)?.clone();
        if c.offset.checked_add(size).is_none_or(|end| end > dst.len()) {
            return Err(Status::INVALID_ARG);
        }
        let staging = self
            .st
            .device
            .create_buffer(BufferKind::Upload, 8)
            .map_err(|e| status_of(&e))?;
        staging
            .write(0, &c.value.to_le_bytes()[..c.size as usize])
            .map_err(|e| status_of(&e))?;
        let (list, _) = self.list()?;
        if c.mode == WriteImmediate::MARKER_IN {
            list.barrier(stage::ALL, stage::ALL, 1)
                .map_err(|e| status_of(&e))?;
        }
        list.copy_buffer(&dst, c.offset, &staging, 0, size)
            .map_err(|e| status_of(&e))?;
        if c.mode == WriteImmediate::MARKER_OUT {
            list.barrier(stage::ALL, stage::ALL, 1)
                .map_err(|e| status_of(&e))?;
        }
        Ok(())
    }

    fn clear_uav(&mut self, c: &victory_metal_proto::cmd::ClearUav) -> R {
        // Raw (format UNKNOWN) or one 32-bit channel: the value is the first
        // word. Only values whose four bytes agree are done (a blit fill
        // writes one byte); others are refused, not approximated.
        match c.format {
            format::UNKNOWN | format::R32_UINT | format::R32_SINT | format::R32_FLOAT => {}
            _ => return Err(Status::UNSUPPORTED),
        }
        let byte = repeated_byte(c.values[0]).ok_or(Status::UNSUPPORTED)?;
        let dst = self.st.buffer(c.res)?.clone();
        let (list, _) = self.list()?;
        list.fill_buffer(&dst, c.offset, c.size, byte)
            .map_err(|e| status_of(&e))
    }

    fn copy_texture_region(&mut self, c: &CopyTextureRegion) -> R {
        use crate::copy::{BufLoc, TexLoc};
        use victory_metal_proto::types::LocKind;
        let (dst, src) = (&c.dst, &c.src);
        let boxed = c.flags & CopyTextureRegion::HAS_BOX != 0;
        let [left, top, front] = if boxed {
            [c.box_left, c.box_top, c.box_front]
        } else {
            [0; 3]
        };
        let box_size = [
            c.box_right.saturating_sub(c.box_left),
            c.box_bottom.saturating_sub(c.box_top),
            c.box_back.saturating_sub(c.box_front),
        ];
        match (dst.kind, src.kind) {
            (LocKind::SUBRESOURCE, LocKind::FOOTPRINT) => {
                let t = self.st.texture(dst.res)?.clone();
                let b = self.st.buffer(src.res)?.clone();
                let size = if boxed {
                    box_size
                } else {
                    [src.width, src.height, src.depth]
                };
                let offset = footprint_offset(src, [left, top, front])?;
                let dl = TexLoc {
                    texture: &t,
                    mip: u32::from(dst.mip),
                    slice: dst.slice,
                    plane: u32::from(dst.plane),
                    origin: [c.dst_x, c.dst_y, c.dst_z],
                };
                let bl = BufLoc {
                    buffer: &b,
                    offset,
                    format: src.format,
                    row_pitch: src.row_pitch,
                    rows: src.height.saturating_sub(top),
                };
                let (list, _) = self.list()?;
                list.copy_buffer_to_texture(&dl, &bl, size)
            }
            (LocKind::FOOTPRINT, LocKind::SUBRESOURCE) => {
                let t = self.st.texture(src.res)?.clone();
                let b = self.st.buffer(dst.res)?.clone();
                let mip = u32::from(src.mip);
                let whole = t.desc().mip_size(mip.min(t.desc().mips - 1));
                let size = if boxed { box_size } else { whole };
                let offset = footprint_offset(dst, [c.dst_x, c.dst_y, c.dst_z])?;
                let sl = TexLoc {
                    texture: &t,
                    mip,
                    slice: src.slice,
                    plane: u32::from(src.plane),
                    origin: [left, top, front],
                };
                let bl = BufLoc {
                    buffer: &b,
                    offset,
                    format: dst.format,
                    row_pitch: dst.row_pitch,
                    rows: dst.height.saturating_sub(c.dst_y),
                };
                let (list, _) = self.list()?;
                list.copy_texture_to_buffer(&bl, &sl, size)
            }
            (LocKind::SUBRESOURCE, LocKind::SUBRESOURCE) => {
                let d = self.st.texture(dst.res)?.clone();
                let s = self.st.texture(src.res)?.clone();
                let mip = u32::from(src.mip);
                let whole = s.desc().mip_size(mip.min(s.desc().mips - 1));
                let size = if boxed { box_size } else { whole };
                let dl = TexLoc {
                    texture: &d,
                    mip: u32::from(dst.mip),
                    slice: dst.slice,
                    plane: u32::from(dst.plane),
                    origin: [c.dst_x, c.dst_y, c.dst_z],
                };
                let sl = TexLoc {
                    texture: &s,
                    mip,
                    slice: src.slice,
                    plane: u32::from(src.plane),
                    origin: [left, top, front],
                };
                let (list, _) = self.list()?;
                list.copy_texture(&dl, &sl, size)
            }
            // Buffer to buffer is `CopyBufferRegion`.
            _ => return Err(Status::INVALID_ARG),
        }
        .map_err(|e| self.st.refuse("COPY_TEXTURE_REGION", &e))
    }

    fn copy_resource(&mut self, c: &victory_metal_proto::cmd::CopyResource) -> R {
        use super::Res;
        let both = match (
            self.st.resources.get(c.dst.0).ok_or(Status::NOT_FOUND)?,
            self.st.resources.get(c.src.0).ok_or(Status::NOT_FOUND)?,
        ) {
            (Res::Buffer(d), Res::Buffer(s)) => Ok((d.clone(), s.clone())),
            (Res::Texture(d), Res::Texture(s)) => Err((d.clone(), s.clone())),
            _ => return Err(Status::INVALID_ARG),
        };
        let r = match both {
            Ok((d, s)) => {
                if d.len() != s.len() {
                    return Err(Status::INVALID_ARG);
                }
                let (list, _) = self.list()?;
                list.copy_buffer(&d, 0, &s, 0, s.len())
            }
            Err((d, s)) => {
                let (list, _) = self.list()?;
                list.copy_texture_resource(&d, &s)
            }
        };
        r.map_err(|e| self.st.refuse("COPY_RESOURCE", &e))
    }

    fn clear_uav_texture(&mut self, c: &victory_metal_proto::cmd::ClearUavTexture) -> R {
        use victory_metal_proto::cmd::ClearUavTexture;
        let t = self.st.texture(c.res)?.clone();
        let req = crate::clear::ClearRequest {
            format: c.format,
            mip: c.mip,
            first_slice: c.first_slice,
            slice_count: c.slice_count,
            values: c.values,
            float_values: c.float_values != 0,
            rect: (c.flags & ClearUavTexture::HAS_RECT != 0).then_some([
                c.rect_left,
                c.rect_top,
                c.rect_right,
                c.rect_bottom,
            ]),
        };
        let kernels = &self.st.clear_kernels;
        let (list, _) = match self.open.as_mut() {
            Some((l, _, b)) => (l, b),
            None => return Err(Status::BAD_STREAM),
        };
        list.clear_texture(kernels, &t, &req).map_err(|e| {
            self.st.note = Some(format!("CLEAR_UAV_TEXTURE: {e}"));
            status_of(&e)
        })
    }

    fn barrier(&mut self, c: &victory_metal_proto::cmd::Barrier) -> R {
        let (list, _) = self.list()?;
        let r = match c.class {
            BarrierClass::MEMORY => list.barrier(c.before, c.after, c.visibility),
            BarrierClass::ALIASING => list.aliasing_barrier(),
            _ => return Err(Status::UNSUPPORTED),
        };
        r.map_err(|e| status_of(&e))
    }
}

/// Writes the root arguments of the compute (`graphics` false) or graphics
/// side to the ring and binds them, when they changed or the other side
/// wrote the table slot last.
pub(super) fn write_root_arguments(list: &mut CommandList, bound: &mut Bound, graphics: bool) -> R {
    let switched = bound.owner != Some(graphics);
    let side = if graphics {
        &mut bound.graphics
    } else {
        &mut bound.compute
    };
    if (side.dirty || switched) && !side.shadow.is_empty() {
        let tlab = list
            .alloc_constants(side.shadow.len() as u64, 8)
            .map_err(|e| status_of(&e))?;
        tlab.write_bytes(0, &side.shadow)
            .map_err(|e| status_of(&e))?;
        list.set_root_arguments(&tlab).map_err(|e| status_of(&e))?;
        bound.owner = Some(graphics);
    }
    side.dirty = false;
    Ok(())
}

/// `Some(b)` when all four bytes of `v` are `b`.
fn repeated_byte(v: u32) -> Option<u8> {
    let b = (v & 0xff) as u8;
    (v == u32::from_ne_bytes([b; 4])).then_some(b)
}

/// Byte offset of texel `at` (`[x, y, z]` from the footprint's corner) in a
/// buffer footprint: the footprint's offset plus whole images, rows and
/// texels. A block compressed format moves in blocks, so `at` must be on one.
fn footprint_offset(
    loc: &victory_metal_proto::cmd::TextureLocation,
    at: [u32; 3],
) -> Result<u64, Status> {
    let f = crate::format::lookup(loc.format).ok_or(Status::INVALID_ARG)?;
    let (bw, bh) = (u32::from(f.block.0), u32::from(f.block.1));
    if !at[0].is_multiple_of(bw) || !at[1].is_multiple_of(bh) {
        return Err(Status::INVALID_ARG);
    }
    let rows_per_image = u64::from(f.block_rows(loc.height));
    let image = u64::from(loc.row_pitch) * rows_per_image;
    Ok(loc.offset
        + u64::from(at[2]) * image
        + u64::from(at[1] / bh) * u64::from(loc.row_pitch)
        + u64::from(at[0] / bw) * u64::from(f.bytes))
}

/// `VICTORY_METAL_INDIRECT_BARRIER` (see `execute_indirect`).
fn indirect_barrier() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_METAL_INDIRECT_BARRIER").is_some_and(|v| v != "0"))
}
