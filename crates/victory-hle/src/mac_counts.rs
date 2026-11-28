//! How often the title calls each of our HLE functions (docs/mac/PATCHES.md,
//! patch H3): `VICTORY_MAC_HLE_COUNTS=<file>`, off by default.
//!
//! Every function the registry hands out (an import bound by
//! [`crate::HleResolver`], `GetProcAddress` on one of our pseudo modules, a
//! host-module override) gets, when the switch is on, a 32-byte x86-64 stub
//! in front of it:
//!
//! ```text
//!   mov r11, <counter>        49 BB imm64
//!   lock inc qword [r11]      F0 49 FF 03
//!   mov r11, <function>       49 BB imm64
//!   jmp r11                   41 FF E3
//! ```
//!
//! `r11` is volatile and carries no argument in the Windows x64 convention,
//! and the stub ends in a jump, so it works for every signature (varargs,
//! stack arguments, floating point) and leaves no frame behind: the callee
//! returns straight to the title. With the switch off nothing is wrapped:
//! the registry returns the function's own address and costs one `OnceLock`
//! read per bound import (not per call).
//!
//! The counts, per `module` (the [`crate::HleModule`] name, not the DLL the
//! import named: one module answers for several DLLs) and function, are
//! written as a text table to the file: every 5 s
//! (`VICTORY_MAC_HLE_COUNTS_SECS=<n>` changes that, at least 1) while they
//! change, and once more when the process ends: the title's CRT `exit`, `abort`,
//! `TerminateProcess` on itself (each of ours calls [`flush`]) and the CRT's
//! exit functions. A process the host kills outright leaves the last 5 s
//! table. The file is replaced atomically (written beside it, renamed), so a
//! reader never sees half a table. Functions bound but never called are listed
//! with 0: "imported, never reached" is a result too.
//!
//! Only functions of ours are counted. An import the host answers
//! (`Provenance::Host`) goes straight to Wine and does not pass here.

#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// First line of the file; bump when the columns change.
pub const HEADER: &str = "# victory-hle-counts v1";

/// One bound function.
struct Entry {
    module: &'static str,
    name: &'static str,
    calls: &'static AtomicU64,
    /// What the title was given: the stub, or the function itself.
    address: usize,
}

/// The counters of one process (one instance is global; tests make their own).
pub struct Counts {
    entries: Mutex<Vec<Entry>>,
    index: Mutex<HashMap<(&'static str, &'static str), usize>>,
    started: Instant,
}

/// One row of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub module: &'static str,
    pub name: &'static str,
    pub calls: u64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Default for Counts {
    fn default() -> Self {
        Self::new()
    }
}

impl Counts {
    pub fn new() -> Self {
        Counts {
            entries: Mutex::new(Vec::new()),
            index: Mutex::new(HashMap::new()),
            started: Instant::now(),
        }
    }

    /// The address to hand out for `module`/`name` whose function is at
    /// `address`. `stub` builds the counting stub for a counter and a target
    /// (None: this host cannot, the function goes out unwrapped and its counter
    /// stays 0). The same function bound twice (two DLL names, a `GetProcAddress`
    /// after an import) gets one counter and one stub.
    pub fn bind(
        &self,
        module: &'static str,
        name: &'static str,
        address: usize,
        stub: impl FnOnce(&'static AtomicU64, usize) -> Option<usize>,
    ) -> usize {
        let mut index = lock(&self.index);
        if let Some(&i) = index.get(&(module, name)) {
            return lock(&self.entries)[i].address;
        }
        let calls: &'static AtomicU64 = Box::leak(Box::new(AtomicU64::new(0)));
        let given = stub(calls, address).unwrap_or(address);
        let mut entries = lock(&self.entries);
        index.insert((module, name), entries.len());
        entries.push(Entry {
            module,
            name,
            calls,
            address: given,
        });
        given
    }

    /// The counter of a function (made if need be), for code that counts by
    /// itself instead of through a stub.
    pub fn counter(&self, module: &'static str, name: &'static str) -> &'static AtomicU64 {
        let mut index = lock(&self.index);
        let mut entries = lock(&self.entries);
        if let Some(&i) = index.get(&(module, name)) {
            return entries[i].calls;
        }
        let calls: &'static AtomicU64 = Box::leak(Box::new(AtomicU64::new(0)));
        index.insert((module, name), entries.len());
        entries.push(Entry {
            module,
            name,
            calls,
            address: 0,
        });
        calls
    }

    /// The rows, most called first (ties by module and name). `wait` false
    /// gives up (None) if the table is locked, for the process-exit path: a
    /// thread the loader ended may have died holding it.
    pub fn rows(&self, wait: bool) -> Option<Vec<Row>> {
        let entries = if wait {
            lock(&self.entries)
        } else {
            match self.entries.try_lock() {
                Ok(g) => g,
                Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => return None,
            }
        };
        let mut rows: Vec<Row> = entries
            .iter()
            .map(|e| Row {
                module: e.module,
                name: e.name,
                calls: e.calls.load(Relaxed),
            })
            .collect();
        drop(entries);
        rows.sort_by(|a, b| {
            b.calls
                .cmp(&a.calls)
                .then(a.module.cmp(b.module))
                .then(a.name.cmp(b.name))
        });
        Some(rows)
    }

    /// The text of the file: [`HEADER`], one `#` line with the process id, the
    /// uptime, why it is written (`periodic`, `exit`, ...) and the totals, a
    /// column line, then `module<TAB>function<TAB>calls` per row.
    pub fn render(&self, rows: &[Row], reason: &str) -> String {
        let total: u64 = rows.iter().map(|r| r.calls).sum();
        let called = rows.iter().filter(|r| r.calls > 0).count();
        let mut s = String::with_capacity(64 + rows.len() * 40);
        let _ = writeln!(s, "{HEADER}");
        let _ = writeln!(
            s,
            "# pid={} uptime_s={:.1} reason={reason} bound={} called={called} total={total}",
            std::process::id(),
            self.started.elapsed().as_secs_f64(),
            rows.len()
        );
        let _ = writeln!(s, "module\tfunction\tcalls");
        for r in rows {
            let _ = writeln!(s, "{}\t{}\t{}", r.module, r.name, r.calls);
        }
        s
    }
}

/// Writes `text` to `path` so that a reader sees the old file or the new one:
/// a file beside it, then a rename over it.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp{}.{}", std::process::id(), SEQ.fetch_add(1, Relaxed)));
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// `VICTORY_MAC_HLE_COUNTS_SECS`: whole seconds, at least 1; anything else is 5.
pub fn interval(value: Option<&str>) -> Duration {
    let secs = value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(5);
    Duration::from_secs(secs)
}

/// `VICTORY_MAC_HLE_COUNTS`: the path, unless empty or `0`/`off`/`false`.
pub fn target(value: Option<&str>) -> Option<PathBuf> {
    let v = value?.trim();
    if matches!(v, "" | "0" | "off" | "false") {
        return None;
    }
    Some(PathBuf::from(v))
}

// ---- the process-wide instance ----

struct Global {
    counts: Counts,
    path: PathBuf,
    /// Serialises the writers (the 5 s thread, the exit paths).
    writing: Mutex<()>,
}

static GLOBAL: OnceLock<Option<Global>> = OnceLock::new();

fn global() -> Option<&'static Global> {
    GLOBAL
        .get_or_init(|| {
            let path = target(std::env::var("VICTORY_MAC_HLE_COUNTS").ok().as_deref())?;
            let every = interval(std::env::var("VICTORY_MAC_HLE_COUNTS_SECS").ok().as_deref());
            tracing::info!(
                path = %path.display(),
                every_s = every.as_secs(),
                "HLE call counts on (VICTORY_MAC_HLE_COUNTS)"
            );
            #[cfg(not(all(windows, target_arch = "x86_64")))]
            tracing::warn!("HLE call counts: no counting stub on this host, every count stays 0");
            register_exit();
            // sleeps a whole interval before it looks at GLOBAL, which is set by then
            start_writer(every);
            Some(Global {
                counts: Counts::new(),
                path,
                writing: Mutex::new(()),
            })
        })
        .as_ref()
}

impl Global {
    fn write(&self, reason: &str, wait: bool) {
        let _w = if wait {
            Some(lock(&self.writing))
        } else {
            match self.writing.try_lock() {
                Ok(g) => Some(g),
                Err(std::sync::TryLockError::Poisoned(e)) => Some(e.into_inner()),
                // another writer is at it; the exit path writes anyway, to a file of its own name
                Err(std::sync::TryLockError::WouldBlock) => None,
            }
        };
        let Some(rows) = self.counts.rows(wait) else {
            return;
        };
        let text = self.counts.render(&rows, reason);
        if let Err(e) = write_atomic(&self.path, &text) {
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                tracing::warn!(path = %self.path.display(), error = %e, "HLE call counts: cannot write the file");
            });
        }
    }
}

fn start_writer(every: Duration) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("hle-counts".into())
            .spawn(move || {
                let mut last = u64::MAX;
                loop {
                    std::thread::sleep(every);
                    let Some(g) = GLOBAL.get().and_then(|g| g.as_ref()) else {
                        continue;
                    };
                    let Some(rows) = g.counts.rows(true) else {
                        continue;
                    };
                    let total: u64 = rows.iter().map(|r| r.calls).sum::<u64>() + rows.len() as u64;
                    if total != last {
                        last = total;
                        g.write("periodic", true);
                    }
                }
            });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "HLE call counts: no writer thread, only the exit table is written");
        }
    });
}

unsafe extern "C" {
    fn atexit(f: extern "C" fn()) -> i32;
}

extern "C" fn at_exit() {
    flush_inner("exit", false);
}

fn register_exit() {
    // SAFETY: `at_exit` has static lifetime and the CRT callback ABI.
    if unsafe { atexit(at_exit) } != 0 {
        tracing::warn!("HLE call counts: could not register the exit table");
    }
}

fn flush_inner(reason: &str, wait: bool) {
    if let Some(Some(g)) = GLOBAL.get() {
        g.write(reason, wait);
    }
}

/// Writes the table now (a no-op when the switch is off). The paths that end the
/// process call it before they go: a normal exit, `abort`, `TerminateProcess`.
pub fn flush(reason: &str) {
    flush_inner(reason, false);
}

/// The registry's hook: the address to give the title for an export. The
/// function's own address when the switch is off.
pub(crate) fn address_for(module: &'static str, name: &'static str, address: usize) -> usize {
    match global() {
        None => address,
        Some(g) => g.counts.bind(module, name, address, stub::make),
    }
}

// ---- the counting stub ----

/// The 27 bytes of the stub (padded to 32 with int3 by the allocator).
pub fn encode_stub(counter: usize, target: usize) -> [u8; 27] {
    let mut c = [0u8; 27];
    c[0..2].copy_from_slice(&[0x49, 0xBB]);
    c[2..10].copy_from_slice(&(counter as u64).to_le_bytes());
    c[10..14].copy_from_slice(&[0xF0, 0x49, 0xFF, 0x03]);
    c[14..16].copy_from_slice(&[0x49, 0xBB]);
    c[16..24].copy_from_slice(&(target as u64).to_le_bytes());
    c[24..27].copy_from_slice(&[0x41, 0xFF, 0xE3]);
    c
}

#[cfg(all(windows, target_arch = "x86_64"))]
mod stub {
    use super::*;
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    const SLOT: usize = 32;
    const CHUNK: usize = 64 * 1024;

    struct Pool {
        next: usize,
        end: usize,
    }
    static POOL: Mutex<Pool> = Mutex::new(Pool { next: 0, end: 0 });

    pub(super) fn make(counter: &'static AtomicU64, target: usize) -> Option<usize> {
        let mut pool = lock(&POOL);
        if pool.next + SLOT > pool.end {
            // SAFETY: a fresh allocation of our own.
            let base = unsafe {
                VirtualAlloc(
                    std::ptr::null(),
                    CHUNK,
                    MEM_RESERVE | MEM_COMMIT,
                    PAGE_EXECUTE_READWRITE,
                )
            } as usize;
            if base == 0 {
                tracing::warn!("HLE call counts: no executable memory for stubs");
                return None;
            }
            pool.next = base;
            pool.end = base + CHUNK;
        }
        let at = pool.next;
        pool.next += SLOT;
        let mut code = [0xCCu8; SLOT];
        code[..27].copy_from_slice(&encode_stub(counter as *const AtomicU64 as usize, target));
        // SAFETY: `at..at+SLOT` is inside the chunk we own, nobody else has the slot.
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), at as *mut u8, SLOT);
            FlushInstructionCache(GetCurrentProcess(), at as *const _, SLOT);
        }
        Some(at)
    }
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
mod stub {
    use super::*;

    pub(super) fn make(_: &'static AtomicU64, _: usize) -> Option<usize> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_stub(_: &'static AtomicU64, target: usize) -> Option<usize> {
        Some(target + 1)
    }

    #[test]
    fn the_stub_is_the_documented_code() {
        let code = encode_stub(0x1122_3344_5566_7788, 0x0102_0304_0506_0708);
        assert_eq!(
            code,
            [
                0x49, 0xBB, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, // mov r11, counter
                0xF0, 0x49, 0xFF, 0x03, // lock inc qword [r11]
                0x49, 0xBB, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, // mov r11, target
                0x41, 0xFF, 0xE3, // jmp r11
            ]
        );
    }

    #[test]
    fn the_switch_and_its_interval() {
        assert_eq!(target(None), None);
        for off in ["", "  ", "0", "off", "false"] {
            assert_eq!(target(Some(off)), None, "{off:?}");
        }
        assert_eq!(
            target(Some(" /tmp/counts.txt ")),
            Some(PathBuf::from("/tmp/counts.txt"))
        );
        assert_eq!(interval(None), Duration::from_secs(5));
        assert_eq!(interval(Some("2")), Duration::from_secs(2));
        for bad in ["0", "-1", "x", "", "1.5"] {
            assert_eq!(interval(Some(bad)), Duration::from_secs(5), "{bad:?}");
        }
    }

    #[test]
    fn one_function_bound_twice_has_one_counter_and_one_stub() {
        let c = Counts::new();
        let mut made = 0;
        let a = c.bind("kernel", "Sleep", 0x1000, |_, t| {
            made += 1;
            Some(t + 1)
        });
        assert_eq!(a, 0x1001);
        assert_eq!(c.bind("kernel", "Sleep", 0x1000, |_, _| panic!("made twice")), 0x1001);
        assert_eq!(made, 1);
        // the same name in another module is another function
        assert_eq!(c.bind("xmem", "Sleep", 0x2000, fake_stub), 0x2001);
        assert_eq!(c.rows(true).unwrap().len(), 2);
    }

    #[test]
    fn no_stub_means_the_function_goes_out_unwrapped() {
        let c = Counts::new();
        assert_eq!(c.bind("kernel", "Sleep", 0x1000, |_, _| None), 0x1000);
        assert_eq!(c.bind("kernel", "Sleep", 0x1000, fake_stub), 0x1000);
    }

    #[test]
    fn rows_are_by_calls_then_name_and_the_table_has_the_totals() {
        let c = Counts::new();
        for (m, n, k) in [
            ("kernel", "Sleep", 3u64),
            ("files", "CreateFileW", 10),
            ("kernel", "CloseHandle", 3),
            ("net", "connect", 0),
        ] {
            c.counter(m, n).fetch_add(k, Relaxed);
        }
        let rows = c.rows(true).unwrap();
        let order: Vec<_> = rows.iter().map(|r| (r.module, r.name, r.calls)).collect();
        assert_eq!(
            order,
            [
                ("files", "CreateFileW", 10),
                ("kernel", "CloseHandle", 3),
                ("kernel", "Sleep", 3),
                ("net", "connect", 0),
            ]
        );
        let text = c.render(&rows, "periodic");
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines[0], HEADER);
        assert!(lines[1].starts_with("# pid="), "{}", lines[1]);
        assert!(
            lines[1].ends_with("reason=periodic bound=4 called=3 total=16"),
            "{}",
            lines[1]
        );
        assert_eq!(lines[2], "module\tfunction\tcalls");
        assert_eq!(lines[3], "files\tCreateFileW\t10");
        assert_eq!(lines[6], "net\tconnect\t0");
        assert_eq!(lines.len(), 7);
    }

    #[test]
    fn counting_is_exact_across_threads() {
        let c = Counts::new();
        let n = c.counter("kernel", "Sleep");
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    for _ in 0..10_000 {
                        n.fetch_add(1, Relaxed);
                    }
                });
            }
        });
        assert_eq!(c.rows(true).unwrap()[0].calls, 80_000);
    }

    #[test]
    fn a_held_table_is_skipped_on_the_exit_path() {
        let c = Counts::new();
        c.counter("kernel", "Sleep");
        let held = lock(&c.entries);
        assert!(c.rows(false).is_none());
        drop(held);
        assert!(c.rows(false).is_some());
    }

    #[test]
    fn the_file_is_replaced_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("counts.txt");
        write_atomic(&path, "one\n").unwrap();
        write_atomic(&path, "two\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two\n");
        let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(left.len(), 1, "no temporary file stays: {left:?}");
        // an unwritable place is an error, not a panic
        assert!(write_atomic(&dir.path().join("no/such/dir/x"), "x").is_err());
    }

    /// The registry's wrapping with the switch on, through a child process
    /// (the switch is read once per process): bind, count, `exit`, read the file.
    #[test]
    fn the_table_is_written_when_the_process_exits() {
        if std::env::var_os("VICTORY_HLE_COUNTS_CHILD").is_some() {
            return child();
        }
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("counts.txt");
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "mac_counts::tests::the_table_is_written_when_the_process_exits",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("VICTORY_HLE_COUNTS_CHILD", "1")
            .env("VICTORY_MAC_HLE_COUNTS", &file)
            .env("VICTORY_MAC_HLE_COUNTS_SECS", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let text = std::fs::read_to_string(&file).expect("the child wrote the table");
        assert!(text.contains("reason=exit "), "{text}");
        assert!(text.contains("kernel\tSleep\t7\n"), "{text}");
        assert!(text.contains("files\tCreateFileW\t0\n"), "{text}");
        // and what the registry bound in the child, through `export_address`
        let m = &crate::registry()[0];
        let row = format!("{}\t{}\t0\n", m.name, m.exports[0].name);
        assert!(text.contains(&row), "{row:?} not in {text}");
    }

    fn child() {
        unsafe extern "C" {
            fn exit(code: i32) -> !;
        }
        let sleep = address_for("kernel", "Sleep", 0x1000);
        address_for("files", "CreateFileW", 0x2000);
        let m = &crate::registry()[0];
        let e = &m.exports[0];
        let given = crate::registry::export_address(m, e);
        #[cfg(all(windows, target_arch = "x86_64"))]
        assert_ne!(given, e.address, "the registry hands out the stub");
        #[cfg(not(all(windows, target_arch = "x86_64")))]
        assert_eq!(given, e.address);
        // without a stub (non-x86 hosts) the count is ours to make; with one, the
        // stub is not called here, so make it the same way
        let _ = sleep;
        global().unwrap().counts.counter("kernel", "Sleep").fetch_add(7, Relaxed);
        // the 1 s writer has time to run once; the exit table must still win
        std::thread::sleep(Duration::from_millis(1300));
        // SAFETY: the CRT's exit, as the title's `exit` ends in.
        unsafe { exit(0) }
    }

    /// With the stub really executed (x86-64 Windows, i.e. under Wine): the call
    /// reaches the function with every argument (four in registers, three on the
    /// stack) and its result, and the counter goes up by one per call.
    #[cfg(all(windows, target_arch = "x86_64"))]
    #[test]
    fn the_stub_counts_and_passes_arguments_through() {
        extern "system" fn seven(a: u64, b: u64, c: u64, d: u64, e: u64, f: u64, g: u64) -> u64 {
            a + 10 * b + 100 * c + 1000 * d + 10_000 * e + 100_000 * f + 1_000_000 * g
        }
        extern "system" fn float(a: f64, b: u32) -> f64 {
            a * f64::from(b)
        }
        let c = Counts::new();
        let at = c.bind("t", "seven", seven as *const () as usize, stub::make);
        assert_ne!(at, seven as *const () as usize);
        let f: extern "system" fn(u64, u64, u64, u64, u64, u64, u64) -> u64 =
            unsafe { std::mem::transmute(at) };
        for _ in 0..1000 {
            assert_eq!(f(1, 2, 3, 4, 5, 6, 7), 7_654_321);
        }
        let at = c.bind("t", "float", float as *const () as usize, stub::make);
        let g: extern "system" fn(f64, u32) -> f64 = unsafe { std::mem::transmute(at) };
        assert_eq!(g(1.5, 4), 6.0);
        let by: HashMap<_, _> = c
            .rows(true)
            .unwrap()
            .into_iter()
            .map(|r| (r.name, r.calls))
            .collect();
        assert_eq!(by["seven"], 1000);
        assert_eq!(by["float"], 1);
    }

    /// Eight threads through one stub: no count is lost.
    #[cfg(all(windows, target_arch = "x86_64"))]
    #[test]
    fn the_stub_counts_exactly_across_threads() {
        extern "system" fn one(x: u64) -> u64 {
            x + 1
        }
        let c = Counts::new();
        let at = c.bind("t", "one", one as *const () as usize, stub::make);
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(move || {
                    let f: extern "system" fn(u64) -> u64 = unsafe { std::mem::transmute(at) };
                    for i in 0..20_000 {
                        assert_eq!(f(i), i + 1);
                    }
                });
            }
        });
        assert_eq!(c.rows(true).unwrap()[0].calls, 160_000);
    }
}
