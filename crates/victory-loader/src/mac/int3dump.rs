//! L7: a dump of the machine at the title's own `int3` assertions, `VICTORY_MAC_INT3_DUMP=<rva>[,<rva>...]`
//! (a diagnostic, off by default, not fidelity: it only reads and prints).
//!
//! Built for `mac.title.int3_self_exit_r6` (image+0x5149aec) and `mac.title.srl_prestream_stall`: the title's
//! entity pools (BUILDING 22500 slots, OBJECT 27500) trap there when the free list is empty, and a log of the
//! counts cannot say *why* it is empty. Three answers are possible, and this dump tells them apart:
//!
//! 1. **Every slot is held** (all `next` entries carry the allocated mark, the used counter equals the capacity):
//!    nothing is wrong with the list, the nodes are not given back (a leak, or a Prestream that never ends).
//! 2. **The list lost slots** (the head says empty but some slots are not marked allocated): the free list was
//!    broken, for example by a lost `lock cmpxchg` update under Rosetta (see L1, L2, L6).
//! 3. The head is not empty (the trap came from `[rcx] == 0`, a pool without node memory, or a race).
//!
//! The pool layout, read from the title's `Alloc` (`0x145149980..0x145149b04`, build 1.0.2.1), its constructor
//! (`0x1416cbe00`) and its `Free` (`0x1451f2801..`); `rcx` is the pool object (a global; 0x194b8b420 in one
//! run, 0x195f8b420 in another: the runtime value is what counts):
//!
//! ```text
//! [rcx+0x00] u64  base of the node memory (0: the pool has none, Alloc traps too)
//! [rcx+0x08] u32  node size (the step)
//! [rcx+0x0c] u32  capacity: number of nodes (and of entries of the next array)
//! [rcx+0x10] u32  used counter (lock inc in Alloc, lock dec in Free)
//! [rcx+0x14] u32  0 at creation
//! [rcx+0x18] u64  second table (0 or a pointer; Alloc calls a hook if it is not 0)
//! [rcx+0x20] u64  free list head: low 17 bits = index of the first free node, 0x1fffe = EMPTY; high 32 = tag (ABA)
//! [rcx+0x28] u64  pointer to the next array: u32 per node; low 17 bits = next free index (0x1fffe ends the
//!                 list), or 0x1ffff = ALLOCATED (Alloc ors [rcx+0x30] into it); the bits above are a counter
//! [rcx+0x30] u32  0x1ffff (the allocated mark)
//! node = [rcx] + index * [rcx+8]
//! ```
//!
//! The "next" of a free node is therefore **not in the node**: it is `next[index] & 0x1ffff`.
//!
//! Everything here is plain data in, lines out, so it is tested on any host with a fake memory; only the
//! Windows hook at the bottom (called from `crash.rs`' vectored handler) touches the process.

/// The environment variable that turns the dump on.
pub const ENV: &str = "VICTORY_MAC_INT3_DUMP";
/// Optional HLE diagnostic registered by the storage factory (no loader -> HLE dependency).
pub static STORAGE_SNAPSHOT: std::sync::OnceLock<fn(&str)> = std::sync::OnceLock::new();

/// The index field of a head or a next entry.
pub const INDEX_MASK: u32 = 0x1ffff;
/// "The list ends here" / "the pool is empty" (the title compares the head with it).
pub const EMPTY: u32 = 0x1fffe;
/// A next entry with this index field belongs to an allocated node.
pub const ALLOCATED: u32 = 0x1ffff;
/// Free-list nodes listed, following next from the head.
pub const WALK_STEPS: usize = 64;
/// Nodes whose first 32 bytes are listed.
pub const SAMPLE_NODES: usize = 64;
/// Bytes below `rcx` that are listed.
pub const BELOW: u64 = 0x40;
/// Bytes above `rcx` that are listed.
pub const ABOVE: u64 = 0x100;

/// Where the dump reads from: the process's own memory, or a fake one in the tests.
pub trait Memory {
    /// Fills `out` from `addr`; false (and `out` unspecified) if any byte is unreadable.
    fn read(&self, addr: u64, out: &mut [u8]) -> bool;
}

fn read_u32(mem: &dyn Memory, addr: u64) -> Option<u32> {
    let mut b = [0u8; 4];
    mem.read(addr, &mut b).then(|| u32::from_le_bytes(b))
}

fn read_u64(mem: &dyn Memory, addr: u64) -> Option<u64> {
    let mut b = [0u8; 8];
    mem.read(addr, &mut b).then(|| u64::from_le_bytes(b))
}

/// The RVAs a `VICTORY_MAC_INT3_DUMP` value lists (hex, with or without `0x`, comma separated; empty
/// items are skipped), and the items that are not RVAs.
pub fn parse_rvas(list: &str) -> (Vec<u32>, Vec<String>) {
    let (mut rvas, mut rejected) = (Vec::new(), Vec::new());
    for item in list.split(',').map(str::trim).filter(|i| !i.is_empty()) {
        let digits = item
            .strip_prefix("0x")
            .or_else(|| item.strip_prefix("0X"))
            .unwrap_or(item);
        match u32::from_str_radix(digits, 16) {
            Ok(rva) => {
                if !rvas.contains(&rva) {
                    rvas.push(rva);
                }
            }
            Err(_) => rejected.push(item.to_owned()),
        }
    }
    (rvas, rejected)
}

/// Whether `address` is `base + rva` of one of `rvas`.
pub fn is_hit(rvas: &[u32], base: u64, address: u64) -> bool {
    address >= base && u32::try_from(address - base).is_ok_and(|rva| rvas.contains(&rva))
}

/// The registers of the thread that trapped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Regs {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub eflags: u32,
}

/// One thread of the process: where it was when the dump looked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadRip {
    pub id: u32,
    /// `None`: the thread could not be opened, suspended or read.
    pub rip: Option<u64>,
    /// The name the caller gives that address (`module+rva`); empty if it has none.
    pub place: String,
}

/// What the pool's numbers say about why the free list is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Head empty, every slot marked allocated: the nodes are held, not given back.
    Exhausted,
    /// Head empty, but `unmarked` slots are not marked allocated: they are lost to the list.
    FreeListLost { unmarked: u32 },
    /// Head not empty and the chain from it covers exactly the unmarked slots: the list is whole.
    ListIntact,
    /// Head not empty, but the chain from it is not the unmarked slots (`chain` nodes, `unmarked` slots).
    ListBroken { chain: u32, unmarked: u32 },
    /// The header or the next array could not be read, or its numbers are not a pool's.
    NotAPool,
}

/// The lines of the dump of one trap. The first line names the trap; every line starts with
/// `VIctory int3-dump:` so a log can be grepped.
pub fn dump(
    mem: &dyn Memory,
    rva: u32,
    address: u64,
    thread: u32,
    regs: &Regs,
    threads: &[ThreadRip],
) -> Vec<String> {
    let mut lines = vec![format!(
        "VIctory int3-dump: int3 at image+{rva:#x} ({address:#x}) on thread {thread}, pool object rcx = {:#x}",
        regs.rcx
    )];
    lines.extend(format_regs(regs));
    lines.extend(memory_lines(mem, regs.rcx));
    lines.extend(pool_lines(mem, regs.rcx).0);
    lines.extend(thread_lines(threads, thread));
    lines
}

/// All general registers, four to a line.
pub fn format_regs(r: &Regs) -> Vec<String> {
    let all: [(&str, u64); 17] = [
        ("rax", r.rax),
        ("rbx", r.rbx),
        ("rcx", r.rcx),
        ("rdx", r.rdx),
        ("rsi", r.rsi),
        ("rdi", r.rdi),
        ("rbp", r.rbp),
        ("rsp", r.rsp),
        ("r8", r.r8),
        ("r9", r.r9),
        ("r10", r.r10),
        ("r11", r.r11),
        ("r12", r.r12),
        ("r13", r.r13),
        ("r14", r.r14),
        ("r15", r.r15),
        ("rip", r.rip),
    ];
    let mut lines: Vec<String> = all
        .chunks(4)
        .map(|c| {
            let regs: Vec<String> = c.iter().map(|(n, v)| format!("{n} {v:#018x}")).collect();
            format!("VIctory int3-dump: regs  {}", regs.join("  "))
        })
        .collect();
    lines.push(format!(
        "VIctory int3-dump: regs  eflags {:#010x}",
        r.eflags
    ));
    lines
}

fn qword_text(v: Option<u64>) -> String {
    v.map_or_else(|| "????????????????".to_owned(), |v| format!("{v:016x}"))
}

/// `[rcx - 0x40, rcx + 0x100)` as qwords in hex, four to a line, each line with its address and its
/// offset from `rcx`; `????????????????` for a qword that cannot be read.
pub fn memory_lines(mem: &dyn Memory, rcx: u64) -> Vec<String> {
    let start = rcx.wrapping_sub(BELOW);
    let mut lines = vec![format!(
        "VIctory int3-dump: memory [rcx-{BELOW:#x}, rcx+{ABOVE:#x}) = {start:#x}.. as qwords"
    )];
    let total = BELOW + ABOVE;
    let mut off = 0;
    while off < total {
        let mut text = Vec::new();
        for k in 0..4 {
            let at = start.wrapping_add(off + k * 8);
            text.push(qword_text(read_u64(mem, at)));
        }
        let rel = off as i64 - BELOW as i64;
        let sign = if rel < 0 { '-' } else { '+' };
        lines.push(format!(
            "VIctory int3-dump: {:#018x} rcx{sign}{:#x}: {}",
            start.wrapping_add(off),
            rel.unsigned_abs(),
            text.join(" ")
        ));
        off += 32;
    }
    lines
}

/// Threads and where they were; the trapping one is marked.
pub fn thread_lines(threads: &[ThreadRip], current: u32) -> Vec<String> {
    let mut lines = vec![format!("VIctory int3-dump: {} threads", threads.len())];
    for t in threads {
        let me = if t.id == current {
            " (this one trapped)"
        } else {
            ""
        };
        let at = match (t.rip, t.place.is_empty()) {
            (None, _) => "rip unavailable".to_owned(),
            (Some(rip), true) => format!("rip {rip:#x}"),
            (Some(rip), false) => format!("rip {rip:#x} {}", t.place),
        };
        lines.push(format!("VIctory int3-dump: thread {} {at}{me}", t.id));
    }
    lines
}

/// The pool analysis: header, node sample, free list walk, census of the next array and the verdict.
pub fn pool_lines(mem: &dyn Memory, rcx: u64) -> (Vec<String>, Verdict) {
    let mut out = Vec::new();
    let mut say = |s: String| out.push(format!("VIctory int3-dump: {s}"));
    let (
        Some(base),
        Some(step),
        Some(cap),
        Some(used),
        Some(f14),
        Some(f18),
        Some(head),
        Some(arr),
        Some(mark),
    ) = (
        read_u64(mem, rcx),
        read_u32(mem, rcx.wrapping_add(8)),
        read_u32(mem, rcx.wrapping_add(0xc)),
        read_u32(mem, rcx.wrapping_add(0x10)),
        read_u32(mem, rcx.wrapping_add(0x14)),
        read_u64(mem, rcx.wrapping_add(0x18)),
        read_u64(mem, rcx.wrapping_add(0x20)),
        read_u64(mem, rcx.wrapping_add(0x28)),
        read_u32(mem, rcx.wrapping_add(0x30)),
    )
    else {
        say(format!(
            "pool: the object at rcx {rcx:#x} (0x38 bytes) is not readable: rcx is not a pool"
        ));
        return (out, Verdict::NotAPool);
    };
    let head_index = head as u32 & INDEX_MASK;
    let head_tag = (head >> 32) as u32;
    say(format!(
        "pool: base {base:#x}  step {step:#x} ({step})  capacity {cap:#x} ({cap})  used {used}  [rcx+0x14] {f14:#x}  \
         [rcx+0x18] {f18:#x}"
    ));
    say(format!(
        "pool: head {head:#018x} = index {head_index:#x}{} tag {head_tag:#x}; next array {arr:#x}; allocated mark {mark:#x}{}",
        if head_index == EMPTY { " (EMPTY)" } else { "" },
        if mark == ALLOCATED {
            ""
        } else {
            " (NOT 0x1ffff: the census below assumes 0x1ffff)"
        }
    ));
    if base == 0 {
        say("pool: [rcx] is 0: the pool has no node memory, which makes Alloc trap too".to_owned());
    }
    let plausible = step != 0 && step <= 1 << 20 && cap != 0 && cap < EMPTY && arr != 0;
    if !plausible {
        say("pool: step, capacity or next array are not a pool's: the rest of the analysis is skipped".to_owned());
        return (out, Verdict::NotAPool);
    }

    // the next array, whole (at most 0x1fffe entries): one read per 1024
    let mut next = vec![u32::MAX; cap as usize];
    let mut unreadable = 0u32;
    let mut known = vec![false; cap as usize];
    let mut i = 0usize;
    while i < cap as usize {
        let n = (cap as usize - i).min(1024);
        let mut buf = vec![0u8; n * 4];
        if mem.read(arr + 4 * i as u64, &mut buf) {
            for (k, c) in buf.chunks_exact(4).enumerate() {
                next[i + k] = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                known[i + k] = true;
            }
        } else {
            unreadable += n as u32;
        }
        i += n;
    }

    // a sample of nodes: their first 32 bytes, with the next entry of their index
    if base != 0 {
        say(format!(
            "pool: first {} nodes, 32 bytes each (index: next entry, then node bytes as qwords)",
            SAMPLE_NODES.min(cap as usize)
        ));
        for i in 0..SAMPLE_NODES.min(cap as usize) {
            let at = base.wrapping_add(u64::from(step) * i as u64);
            let mut b = [0u8; 32];
            let bytes = if mem.read(at, &mut b) {
                (0..4)
                    .map(|k| {
                        format!(
                            "{:016x}",
                            u64::from_le_bytes(b[k * 8..k * 8 + 8].try_into().unwrap())
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                "???????????????? ???????????????? ???????????????? ????????????????".to_owned()
            };
            let entry = if known[i] {
                format!("{:#010x}", next[i])
            } else {
                "unreadable".to_owned()
            };
            say(format!("node {i:>3} {at:#014x} next[{i}] {entry}: {bytes}"));
        }
    }

    // the free list from the head
    let (verdict_chain, walk_clean) = {
        if head_index == EMPTY {
            say("free list: the head is EMPTY (0x1fffe): nothing to follow".to_owned());
        } else {
            say(format!(
                "free list: from head index {head_index:#x}, at most {WALK_STEPS} steps listed (next[index] & 0x1ffff)"
            ));
        }
        let mut index = head_index;
        let mut seen = vec![false; cap as usize];
        let mut chain = 0u32;
        let mut clean = true;
        let mut ended = "the head is EMPTY";
        while index != EMPTY {
            let listed = (chain as usize) < WALK_STEPS;
            if index >= cap {
                clean = false;
                ended = "STOP: an index at or above the capacity: the list is corrupt";
                if listed {
                    say(format!(
                        "free[{chain}] index {index:#x}: >= capacity {cap:#x}"
                    ));
                }
                break;
            }
            if seen[index as usize] {
                clean = false;
                ended = "STOP: an index seen before: the list has a cycle";
                if listed {
                    say(format!("free[{chain}] index {index:#x}: seen before"));
                }
                break;
            }
            seen[index as usize] = true;
            if !known[index as usize] {
                clean = false;
                ended = "STOP: the next entry is unreadable";
                break;
            }
            let raw = next[index as usize];
            if raw & INDEX_MASK == ALLOCATED {
                clean = false;
                ended = "STOP: a node on the free list is marked ALLOCATED: the list reaches a held node";
                if listed {
                    say(format!(
                        "free[{chain}] index {index:#x}: next[{index:#x}] {raw:#010x} is marked ALLOCATED"
                    ));
                }
                break;
            }
            if listed {
                let node = base.wrapping_add(u64::from(step) * u64::from(index));
                say(format!(
                    "free[{chain}] index {index:#x} node {node:#x} next[{index:#x}] {raw:#010x} -> {:#x}",
                    raw & INDEX_MASK
                ));
            }
            chain += 1;
            index = raw & INDEX_MASK;
            if index == EMPTY {
                ended = "the list ends at the EMPTY sentinel";
            }
        }
        say(format!(
            "free list: {chain} nodes reachable from the head in all; {ended}"
        ));
        (chain, clean)
    };

    // the census of the next array: held and unheld slots
    let (mut held, mut unheld, mut odd) = (0u32, 0u32, 0u32);
    let mut odd_first = Vec::new();
    for i in 0..cap as usize {
        if !known[i] {
            continue;
        }
        match next[i] & INDEX_MASK {
            ALLOCATED => held += 1,
            x if x < cap || x == EMPTY => unheld += 1,
            _ => {
                odd += 1;
                if odd_first.len() < 8 {
                    odd_first.push(format!("[{i}]={:#x}", next[i]));
                }
            }
        }
    }
    say(format!(
        "census: of {cap} slots {held} marked allocated, {unheld} not marked (free, on or off the list), {odd} with an index outside the pool, {unreadable} unreadable; used counter {used}"
    ));
    if !odd_first.is_empty() {
        say(format!(
            "census: first entries outside the pool: {}",
            odd_first.join(" ")
        ));
    }

    let unmarked = unheld;
    let verdict = if unreadable != 0 && held + unheld + odd == 0 {
        Verdict::NotAPool
    } else if head_index == EMPTY {
        if unmarked == 0 && odd == 0 {
            Verdict::Exhausted
        } else {
            Verdict::FreeListLost {
                unmarked: unmarked + odd,
            }
        }
    } else if walk_clean && verdict_chain == unmarked && odd == 0 {
        Verdict::ListIntact
    } else {
        Verdict::ListBroken {
            chain: verdict_chain,
            unmarked: unmarked + odd,
        }
    };
    let text = match verdict {
        Verdict::Exhausted => format!(
            "verdict: EXHAUSTED: all {held} slots are marked allocated and the head is empty: nothing is wrong with the list, the nodes are held and not given back \
             (a leak, or a stream that does not finish); used counter {used}{}",
            if used == held {
                " agrees"
            } else {
                " DIFFERS from the marks (frees or allocs racing the dump, or a damaged counter)"
            }
        ),
        Verdict::FreeListLost { unmarked } => format!(
            "verdict: FREE LIST LOST: the head is empty but {unmarked} slots are not marked allocated: they cannot be reached by Alloc (a damaged list, e.g. a lost lock cmpxchg; \
             or frees that landed after the trap); used counter {used}, marks {held}"
        ),
        Verdict::ListIntact => format!(
            "verdict: LIST INTACT: the head is not empty and its chain ({verdict_chain}) is exactly the unmarked slots: the trap did not come from an empty list \
             (base 0, or a free that landed after it); used counter {used}, marks {held}"
        ),
        Verdict::ListBroken { chain, unmarked } => format!(
            "verdict: LIST BROKEN: the head is not empty but its chain reaches {chain} nodes while {unmarked} slots are unmarked: the list is damaged; used counter {used}, marks {held}"
        ),
        Verdict::NotAPool => "verdict: not a pool (the next array could not be read)".to_owned(),
    };
    say(text);
    (out, verdict)
}

#[cfg(windows)]
pub(crate) use hook::{announce, on_breakpoint};

#[cfg(windows)]
mod hook {
    use super::{Memory, Regs, ThreadRip, dump, is_hit, parse_rvas};
    use std::cell::Cell;
    use std::sync::{Mutex, OnceLock};
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        CONTEXT_FULL_AMD64, EXCEPTION_POINTERS, GetThreadContext, ReadProcessMemory,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleHandleExW,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, OpenThread, ResumeThread,
        SuspendThread, THREAD_GET_CONTEXT, THREAD_SUSPEND_RESUME,
    };

    /// Full dumps per process: seven threads trap at once in a real run, one dump says it all.
    const MAX_DUMPS: u32 = 4;

    /// The listed RVAs, read once; empty (the default) means off.
    fn wanted() -> &'static [u32] {
        static WANTED: OnceLock<Vec<u32>> = OnceLock::new();
        WANTED.get_or_init(|| {
            let Ok(list) = std::env::var(super::ENV) else {
                return Vec::new();
            };
            let (rvas, rejected) = parse_rvas(&list);
            for r in rejected {
                eprintln!(
                    "VIctory int3-dump: {} lists {r:?}, which is not a hex RVA: ignored",
                    super::ENV
                );
            }
            if !rvas.is_empty() {
                let shown: Vec<String> = rvas.iter().map(|r| format!("{r:#x}")).collect();
                eprintln!(
                    "VIctory int3-dump: on for int3 at image+{}",
                    shown.join(", +")
                );
            }
            rvas
        })
    }

    /// Reads the switch at start-up, so its line (and any item it rejects) is in the log before the trap.
    pub(crate) fn announce() {
        wanted();
    }

    /// The process's own memory, through `ReadProcessMemory`: an unreadable address is an error, not a
    /// second fault inside the exception handler.
    pub(super) struct Own;

    impl Memory for Own {
        fn read(&self, addr: u64, out: &mut [u8]) -> bool {
            let mut got = 0usize;
            // SAFETY: `out` is ours and `out.len()` long; the call only fails on unreadable addresses.
            let ok = unsafe {
                ReadProcessMemory(
                    GetCurrentProcess(),
                    addr as usize as *const _,
                    out.as_mut_ptr().cast(),
                    out.len(),
                    &mut got,
                )
            };
            ok != 0 && got == out.len()
        }
    }

    thread_local! {
        /// This thread is inside the dump: a fault in it must not start another.
        static INSIDE: Cell<bool> = const { Cell::new(false) };
    }
    static DUMPS: Mutex<u32> = Mutex::new(0);

    /// The base of the module that holds `address`.
    fn module_base(address: usize) -> Option<usize> {
        let mut module = std::ptr::null_mut();
        // SAFETY: looks up the module of `address`; no reference is taken.
        let found = unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                address as *const u16,
                &mut module,
            )
        };
        (found != 0).then_some(module as usize)
    }

    /// Where every other thread is: each is suspended only while its context is read, into room made
    /// beforehand (it may hold the heap lock), and named after it is running again.
    fn other_threads(me: u32) -> Vec<ThreadRip> {
        let mut ids = Vec::new();
        // SAFETY: a snapshot of this process's threads, closed below.
        unsafe {
            let process = GetCurrentProcessId();
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot != INVALID_HANDLE_VALUE {
                let mut entry: THREADENTRY32 = std::mem::zeroed();
                entry.dwSize = size_of::<THREADENTRY32>() as u32;
                let mut more = Thread32First(snapshot, &mut entry) != 0;
                while more {
                    if entry.th32OwnerProcessID == process && entry.th32ThreadID != me {
                        ids.push(entry.th32ThreadID);
                    }
                    more = Thread32Next(snapshot, &mut entry) != 0;
                }
                CloseHandle(snapshot);
            }
        }
        let mut found: Vec<(u32, Option<u64>)> = Vec::with_capacity(ids.len());
        for id in ids {
            let mut rip = None;
            // SAFETY: opens a thread of our own process.
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT, 0, id) };
            if !thread.is_null() {
                // SAFETY: zeroed is a valid CONTEXT; `Aligned16` gives it the alignment the call needs.
                let mut context = crate::crash::Aligned16(unsafe { std::mem::zeroed() });
                context.0.ContextFlags = CONTEXT_FULL_AMD64;
                // SAFETY: a thread of this process that is not this one; nothing between the suspend and
                // the resume allocates or takes a lock.
                unsafe {
                    if SuspendThread(thread) != u32::MAX {
                        if GetThreadContext(thread, &mut context.0) != 0 {
                            rip = Some(context.0.Rip);
                        }
                        ResumeThread(thread);
                    }
                    CloseHandle(thread);
                }
            }
            found.push((id, rip));
        }
        found
            .into_iter()
            .map(|(id, rip)| ThreadRip {
                id,
                rip,
                place: rip.map_or_else(String::new, |r| crate::crash::locate(r as usize)),
            })
            .collect()
    }

    /// For the vectored handler, after it has reported a breakpoint: if it is a listed `int3`, dumps the
    /// pool at `rcx` and where every thread is. Reads only; the handler goes on as it did.
    ///
    /// # Safety
    /// `info` comes from the system, for an exception being dispatched.
    pub(crate) unsafe fn on_breakpoint(info: *const EXCEPTION_POINTERS) {
        let rvas = wanted();
        if rvas.is_empty() {
            return;
        }
        // SAFETY: the system passes valid exception pointers.
        let (record, context) = unsafe { (&*(*info).ExceptionRecord, &*(*info).ContextRecord) };
        let at = record.ExceptionAddress as usize;
        let Some(base) = module_base(at).or_else(|| module_base(context.Rip as usize)) else {
            return;
        };
        // the exception address is the int3; some hosts report the address after it
        let Some(trap) = [at as u64, context.Rip, context.Rip.wrapping_sub(1)]
            .into_iter()
            .find(|a| is_hit(rvas, base as u64, *a))
        else {
            return;
        };
        if INSIDE.with(|i| i.replace(true)) {
            return;
        }
        let _ = std::panic::catch_unwind(|| {
            let mut dumps = DUMPS.lock().unwrap_or_else(|e| e.into_inner());
            if *dumps >= MAX_DUMPS {
                if *dumps == MAX_DUMPS {
                    eprintln!("VIctory int3-dump: {MAX_DUMPS} dumps made, no more of them");
                    *dumps += 1;
                }
                return;
            }
            *dumps += 1;
            if let Some(snapshot) = super::STORAGE_SNAPSHOT.get() { snapshot("L7-int3"); }
            #[cfg(target_arch="x86_64")]
            if trap - base as u64 == 0x5149aec {
                crate::mac::prestream::summary("5149aec"); // mac L8
                crate::mac::pool_owner::summary("5149aec"); // mac L9
            }
            let regs = Regs {
                rax: context.Rax,
                rbx: context.Rbx,
                rcx: context.Rcx,
                rdx: context.Rdx,
                rsi: context.Rsi,
                rdi: context.Rdi,
                rbp: context.Rbp,
                rsp: context.Rsp,
                r8: context.R8,
                r9: context.R9,
                r10: context.R10,
                r11: context.R11,
                r12: context.R12,
                r13: context.R13,
                r14: context.R14,
                r15: context.R15,
                rip: context.Rip,
                eflags: context.EFlags,
            };
            // SAFETY: a plain query.
            let me = unsafe { GetCurrentThreadId() };
            let mut threads = other_threads(me);
            threads.push(ThreadRip {
                id: me,
                rip: Some(context.Rip),
                place: crate::crash::locate(context.Rip as usize),
            });
            threads.sort_by_key(|t| t.id);
            let lines = dump(&Own, (trap - base as u64) as u32, trap, me, &regs, &threads);
            let mut text = String::new();
            for line in &lines {
                eprintln!("{line}");
                text.push_str(line);
                text.push('\n');
            }
            crate::crash::append_to_log(&text);
        });
        INSIDE.with(|i| i.set(false));
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn the_process_reads_its_own_memory_and_refuses_the_rest() {
        let data = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let mut out = [0u8; 8];
        assert!(hook::Own.read(data.as_ptr() as u64, &mut out));
        assert_eq!(out, data);
        // nothing is mapped at the bottom of the address space, nor above the user range
        assert!(!hook::Own.read(0x10, &mut out));
        assert!(!hook::Own.read(0xffff_8000_0000_0000, &mut out));
        assert!(!hook::Own.read(u64::MAX - 3, &mut out));
        // so the pool analysis of an address that is not a pool is a verdict, not a fault
        assert_eq!(pool_lines(&hook::Own, 0x10).1, Verdict::NotAPool);
        assert_eq!(memory_lines(&hook::Own, 0).len(), 11);
    }

    #[test]
    fn a_pool_built_in_memory_is_read_through_the_process() {
        // object: base, step|cap, used, head, next array, mark
        let next: Vec<u32> = vec![ALLOCATED; 10];
        let nodes = vec![0u8; 10 * 32];
        let mut object = [0u64; 7];
        object[0] = nodes.as_ptr() as u64;
        object[1] = 32 | 10 << 32;
        object[2] = 10;
        object[4] = u64::from(EMPTY);
        object[5] = next.as_ptr() as u64;
        object[6] = u64::from(ALLOCATED);
        let (_, verdict) = pool_lines(&hook::Own, object.as_ptr() as u64);
        assert_eq!(verdict, Verdict::Exhausted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake address space: regions that are readable whole or not at all.
    struct Fake(Vec<(u64, Vec<u8>)>);

    impl Fake {
        fn put(&mut self, at: u64, bytes: Vec<u8>) {
            self.0.push((at, bytes));
        }
    }

    impl Memory for Fake {
        fn read(&self, addr: u64, out: &mut [u8]) -> bool {
            self.0.iter().any(|(at, b)| {
                let ok = addr >= *at && addr - at + out.len() as u64 <= b.len() as u64;
                if ok {
                    let o = (addr - at) as usize;
                    out.copy_from_slice(&b[o..o + out.len()]);
                }
                ok
            })
        }
    }

    const RCX: u64 = 0x1_9400_0420;
    const NODES: u64 = 0x2_0000_0000;
    const ARR: u64 = 0x3_0000_0000;

    /// A pool object at `RCX` with `cap` nodes of `step` bytes whose `next` entries are `next`.
    fn pool(cap: u32, step: u32, used: u32, head: u64, next: &[u32]) -> Fake {
        let mut f = Fake(Vec::new());
        // the object, with 0x40 readable below and 0x100 above it
        let mut obj = vec![0u8; 0x140];
        let put = |obj: &mut Vec<u8>, off: usize, b: &[u8]| {
            obj[0x40 + off..0x40 + off + b.len()].copy_from_slice(b)
        };
        put(&mut obj, 0, &NODES.to_le_bytes());
        put(&mut obj, 8, &step.to_le_bytes());
        put(&mut obj, 0xc, &cap.to_le_bytes());
        put(&mut obj, 0x10, &used.to_le_bytes());
        put(&mut obj, 0x20, &head.to_le_bytes());
        put(&mut obj, 0x28, &ARR.to_le_bytes());
        put(&mut obj, 0x30, &ALLOCATED.to_le_bytes());
        obj[0..8].copy_from_slice(&0xdead_beef_u64.to_le_bytes());
        f.put(RCX - 0x40, obj);
        f.put(ARR, next.iter().flat_map(|n| n.to_le_bytes()).collect());
        let mut nodes = vec![0u8; (cap * step) as usize];
        for i in 0..cap as usize {
            if step < 8 {
                break;
            }
            nodes[i * step as usize..i * step as usize + 8]
                .copy_from_slice(&(0x1000 + i as u64).to_le_bytes());
        }
        f.put(NODES, nodes);
        f
    }

    fn empty_marks(cap: u32) -> Vec<u32> {
        vec![ALLOCATED; cap as usize]
    }

    fn text(lines: &[String]) -> String {
        lines.join("\n")
    }

    #[test]
    fn rvas_are_hex_with_or_without_the_prefix() {
        assert_eq!(parse_rvas("5149aec"), (vec![0x5149aec], vec![]));
        assert_eq!(
            parse_rvas("0x5149aec, 0X31f50 ,,"),
            (vec![0x5149aec, 0x31f50], vec![])
        );
        assert_eq!(parse_rvas("0x5149aec,5149aec"), (vec![0x5149aec], vec![]));
        assert_eq!(parse_rvas(""), (vec![], vec![]));
    }

    #[test]
    fn items_that_are_not_rvas_are_named_and_skipped() {
        let (rvas, bad) = parse_rvas("0x10,xyz,0x1ffffffff,-5,0x20");
        assert_eq!(rvas, vec![0x10, 0x20]);
        assert_eq!(bad, vec!["xyz", "0x1ffffffff", "-5"]);
    }

    #[test]
    fn a_hit_is_base_plus_a_listed_rva_only() {
        let base = 0x1_4000_0000;
        let list = [0x5149aec, 0x31f50];
        assert!(is_hit(&list, base, base + 0x5149aec));
        assert!(is_hit(&list, base, base + 0x31f50));
        assert!(!is_hit(&list, base, base + 0x5149aed));
        assert!(!is_hit(&list, base, base - 1));
        assert!(!is_hit(&list, base, base + (1 << 32) + 0x31f50));
        assert!(!is_hit(&[], base, base + 0x31f50));
    }

    #[test]
    fn registers_are_all_there_four_to_a_line() {
        let r = Regs {
            rax: 0x1ef4_0001_fffe,
            rcx: RCX,
            r15: 0xf,
            rip: 0x1_4514_9aec,
            eflags: 0x246,
            ..Regs::default()
        };
        let lines = format_regs(&r);
        assert_eq!(lines.len(), 6);
        let t = text(&lines);
        for name in [
            "rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rbp", "rsp", "r8 ", "r9 ", "r10", "r11",
            "r12", "r13", "r14", "r15", "rip", "eflags",
        ] {
            assert!(t.contains(name), "{name} in {t}");
        }
        assert!(t.contains("rax 0x00001ef40001fffe"), "{t}");
        assert!(t.contains("eflags 0x00000246"), "{t}");
        assert!(lines[0].starts_with("VIctory int3-dump: regs"));
    }

    #[test]
    fn memory_lines_cover_below_and_above_with_offsets() {
        let f = pool(
            4,
            0x20,
            4,
            0x1_0000_0000 | u64::from(EMPTY),
            &[ALLOCATED; 4],
        );
        let lines = memory_lines(&f, RCX);
        // header + 0x140 bytes / 32
        assert_eq!(lines.len(), 1 + 10);
        assert!(lines[1].contains("rcx-0x40"), "{}", lines[1]);
        assert!(
            lines[1].contains("00000000deadbeef") || lines[1].contains("deadbeef"),
            "{}",
            lines[1]
        );
        assert!(lines[3].contains("rcx+0x0:"), "{}", lines[3]);
        assert!(lines[3].contains(&format!("{NODES:016x}")), "{}", lines[3]);
        assert!(lines[10].contains("rcx+0xe0"), "{}", lines[10]);
    }

    #[test]
    fn unreadable_memory_prints_question_marks_and_does_not_stop() {
        let mut f = Fake(Vec::new());
        f.put(RCX, vec![0x11; 16]);
        let lines = memory_lines(&f, RCX);
        assert_eq!(lines.len(), 11);
        assert!(lines[1].contains("????????????????"));
        assert!(lines.iter().any(|l| l.contains("1111111111111111")));
        // nothing readable at all, and an rcx too small to subtract from
        let none = Fake(Vec::new());
        assert_eq!(memory_lines(&none, 0).len(), 11);
        assert_eq!(pool_lines(&none, 0).1, Verdict::NotAPool);
    }

    #[test]
    fn an_empty_list_with_every_slot_held_is_exhaustion() {
        let cap = 100;
        let f = pool(
            cap,
            0xe0,
            cap,
            0x1ef4_0000_0000 | u64::from(EMPTY),
            &empty_marks(cap),
        );
        let (lines, v) = pool_lines(&f, RCX);
        assert_eq!(v, Verdict::Exhausted);
        let t = text(&lines);
        assert!(t.contains("EXHAUSTED"), "{t}");
        assert!(t.contains("used counter 100 agrees"), "{t}");
        assert!(t.contains("100 marked allocated"), "{t}");
        assert!(t.contains("tag 0x1ef4"), "{t}");
        // the first 64 nodes are sampled, 32 bytes each, with the qword written at their start
        assert_eq!(lines.iter().filter(|l| l.contains(" next[")).count(), 64);
        assert!(t.contains("node   3"), "{t}");
        assert!(t.contains(&format!("{:016x}", 0x1000 + 3)), "{t}");
    }

    #[test]
    fn an_empty_list_with_slots_left_over_is_a_lost_free_list() {
        let cap = 50;
        let mut next = empty_marks(cap);
        // slots 7 and 9 are not marked: nobody can reach them from an empty head
        next[7] = 9;
        next[9] = EMPTY;
        let f = pool(cap, 0x110, cap - 2, u64::from(EMPTY), &next);
        let (lines, v) = pool_lines(&f, RCX);
        assert_eq!(v, Verdict::FreeListLost { unmarked: 2 });
        assert!(text(&lines).contains("FREE LIST LOST"));
    }

    #[test]
    fn a_whole_free_list_is_walked_through_the_next_array_not_the_nodes() {
        let cap = 80;
        let mut next = empty_marks(cap);
        // head 5 -> 2 -> 70 -> end, with counters in the bits above the index
        next[5] = 2 | 0x0006_0000;
        next[2] = 70 | 0x0002_0000;
        next[70] = EMPTY;
        let f = pool(cap, 0x20, cap - 3, 0x77 << 32 | 5, &next);
        let (lines, v) = pool_lines(&f, RCX);
        assert_eq!(v, Verdict::ListIntact);
        let t = text(&lines);
        assert!(t.contains("free[0] index 0x5"), "{t}");
        assert!(t.contains("free[1] index 0x2"), "{t}");
        assert!(t.contains("free[2] index 0x46"), "{t}");
        assert!(t.contains("-> 0x46"), "{t}");
        assert!(t.contains("3 nodes reachable"), "{t}");
        assert!(t.contains("ends at the EMPTY sentinel"), "{t}");
    }

    #[test]
    fn the_walk_stops_at_64_steps_but_counts_the_whole_chain() {
        let cap = 200;
        let mut next = empty_marks(cap);
        for i in 0..cap - 1 {
            next[i as usize] = i + 1;
        }
        next[cap as usize - 1] = EMPTY;
        let f = pool(cap, 0x20, 0, 0, &next);
        let (lines, v) = pool_lines(&f, RCX);
        assert_eq!(v, Verdict::ListIntact);
        assert_eq!(lines.iter().filter(|l| l.contains("free[")).count(), 64);
        assert!(text(&lines).contains("200 nodes reachable"));
    }

    #[test]
    fn a_cycle_an_index_outside_the_pool_and_a_held_node_on_the_list_are_named() {
        let cap = 30;
        // cycle: 1 -> 2 -> 1
        let mut next = empty_marks(cap);
        next[1] = 2;
        next[2] = 1;
        let (lines, v) = pool_lines(&pool(cap, 0x20, 0, 1, &next), RCX);
        assert!(matches!(v, Verdict::ListBroken { .. }), "{v:?}");
        assert!(text(&lines).contains("cycle"));
        // out of range
        let mut next = empty_marks(cap);
        next[3] = 0x1000;
        let (lines, v) = pool_lines(&pool(cap, 0x20, 0, 3, &next), RCX);
        assert!(matches!(v, Verdict::ListBroken { .. }), "{v:?}");
        assert!(text(&lines).contains("outside the pool") || text(&lines).contains(">= capacity"));
        // a free node pointing to a held one
        let mut next = empty_marks(cap);
        next[4] = 6;
        let (lines, v) = pool_lines(&pool(cap, 0x20, 0, 4, &next), RCX);
        assert!(matches!(v, Verdict::ListBroken { .. }), "{v:?}");
        assert!(text(&lines).contains("reaches a held node") || text(&lines).contains("ALLOCATED"));
    }

    #[test]
    fn a_pool_without_node_memory_says_so_and_skips_the_sample() {
        let cap = 10;
        let mut f = pool(cap, 0x20, 3, u64::from(EMPTY), &empty_marks(cap));
        // [rcx] = 0
        f.0[0].1[0x40..0x48].copy_from_slice(&[0; 8]);
        let (lines, _) = pool_lines(&f, RCX);
        let t = text(&lines);
        assert!(t.contains("[rcx] is 0"), "{t}");
        assert!(!t.contains(" next[0] 0x"), "{t}");
    }

    #[test]
    fn an_implausible_header_is_not_walked() {
        let cap = 10;
        let f = pool(cap, 0, 3, u64::from(EMPTY), &empty_marks(cap));
        assert_eq!(pool_lines(&f, RCX).1, Verdict::NotAPool);
        let f = pool(0x20000, 0x20, 3, u64::from(EMPTY), &[]);
        assert_eq!(pool_lines(&f, RCX).1, Verdict::NotAPool);
    }

    #[test]
    fn a_missing_part_of_the_next_array_is_counted_not_fatal() {
        let cap = 3000;
        let mut next = empty_marks(cap);
        next.truncate(1024); // the rest of the array is not readable
        let f = pool(cap, 0x20, cap, u64::from(EMPTY), &next);
        let (lines, v) = pool_lines(&f, RCX);
        assert_eq!(v, Verdict::Exhausted);
        assert!(text(&lines).contains("1976 unreadable"), "{}", text(&lines));
    }

    #[test]
    fn threads_are_listed_with_the_trapping_one_marked() {
        let t = [
            ThreadRip {
                id: 7,
                rip: Some(0x1_4514_9aec),
                place: "victory-image-ab.dll+0x5149aec".into(),
            },
            ThreadRip {
                id: 9,
                rip: Some(0x7ffe_0000),
                place: String::new(),
            },
            ThreadRip {
                id: 11,
                rip: None,
                place: String::new(),
            },
        ];
        let lines = thread_lines(&t, 7);
        assert_eq!(lines[0], "VIctory int3-dump: 3 threads");
        assert!(
            lines[1].contains("victory-image-ab.dll+0x5149aec")
                && lines[1].contains("this one trapped")
        );
        assert!(lines[2].contains("rip 0x7ffe0000") && !lines[2].contains("trapped"));
        assert!(lines[3].contains("rip unavailable"));
    }

    #[test]
    fn the_whole_dump_reads_in_order_and_every_line_is_greppable() {
        let cap = 64;
        let f = pool(
            cap,
            0xe0,
            cap,
            0x1eb9_0000_0000 | u64::from(EMPTY),
            &empty_marks(cap),
        );
        let regs = Regs {
            rcx: RCX,
            rax: 0x1eb9_0001_fffe,
            rip: 0x1_4514_9aec,
            ..Regs::default()
        };
        let threads = [ThreadRip {
            id: 5,
            rip: Some(0x1234),
            place: "x+0x1".into(),
        }];
        let lines = dump(&f, 0x5149aec, 0x1_4514_9aec, 5, &regs, &threads);
        assert!(lines.iter().all(|l| l.starts_with("VIctory int3-dump: ")));
        assert!(lines[0].contains("image+0x5149aec") && lines[0].contains("thread 5"));
        let t = text(&lines);
        let pos = |s: &str| t.find(s).unwrap_or_else(|| panic!("{s} in {t}"));
        assert!(
            pos("regs") < pos("memory [rcx")
                && pos("memory [rcx") < pos("pool: base")
                && pos("pool: base") < pos("verdict")
                && pos("verdict") < pos("1 threads")
        );
    }
}
