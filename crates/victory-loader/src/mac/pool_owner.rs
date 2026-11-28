//! L9: bounded, integer-only Win64 ownership telemetry. SITES are validated as one group.
#[macro_use]
#[path = "pool_owner_asm.rs"]
mod assembly;
pub const ASSEMBLY: &str = l9_source!();
pub const GLOBAL: usize = 0xa9ec938;
pub const SITES: [(usize, &[u8], usize); 3] = [
    (0x5149aef, &[0x48, 0x89, 0xf8, 0x48, 0x83, 0xc4, 0x28], 0x80),
    (0x20559e2, &[0x48, 0x89, 0xf8, 0x48, 0x83, 0xc4, 0x28], 0xa0),
    (0x51f2cb5, &[0x4c, 0x8b, 0x51, 0x28, 0x48, 0x89, 0xc2], 0xc0),
];
pub fn transfers(base: u64, pool: u64) -> Option<Vec<Vec<u8>>> {
    SITES
        .iter()
        .map(|&(r, b, t)| {
            super::masked::jump(
                base.checked_add(r as u64)?,
                b.len(),
                pool.checked_add(t as u64)?,
            )
        })
        .collect()
}
pub fn continuations(base: usize) -> [usize; 3] {
    SITES.map(|(r, b, _)| base + r + b.len())
}
#[cfg(any(test, all(windows, target_arch = "x86_64")))]
mod report {
    use std::{
        collections::BTreeMap,
        sync::atomic::{
            AtomicU64,
            Ordering::{Acquire, SeqCst},
            fence,
        },
    };
    pub type Row = [AtomicU64; 4];
    fn rva(k: u64) -> String {
        if k == 0x100000001 {
            "outside_image".into()
        } else {
            format!("{:#x}", k - 1)
        }
    }
    fn id(i: u64) -> String {
        if i > u32::MAX as u64 {
            "unknown".into()
        } else {
            format!("{i:#010x}")
        }
    }
    pub fn lines(
        slots: &[Row],
        hist: &[Row],
        now: u64,
        ms_per_tick: f64,
        mut csv: Option<&mut String>,
    ) -> Vec<String> {
        use std::fmt::Write as _;
        if let Some(dst) = csv.as_mut() {
            dst.push_str("slot,owner_return_rva,alloc_tsc,age_ticks,age_ms_est,alloc_srl_id\n");
        }
        let mut owners = BTreeMap::<u64, usize>::new();
        let mut phases = BTreeMap::<u64, usize>::new();
        let mut oldest = Vec::new();
        let mut busy = 0;
        for (i, s) in slots.iter().enumerate() {
            let seq = s[0].load(Acquire);
            let k = s[1].load(Acquire);
            let phase = s[2].load(Acquire);
            let tick = s[3].load(Acquire);
            fence(SeqCst);
            if seq & 1 != 0 || s[0].load(Acquire) != seq {
                busy += 1;
                continue;
            }
            if k == 0 {
                continue;
            }
            *owners.entry(k).or_default() += 1;
            *phases.entry(phase).or_default() += 1;
            oldest.push((tick, i, k, phase));
            if let Some(dst) = csv.as_mut() {
                let age = now.saturating_sub(tick);
                let _ = writeln!(
                    dst,
                    "{i},{},{tick},{age},{:.3},{}",
                    rva(k),
                    age as f64 * ms_per_tick,
                    id(phase)
                );
            }
        }
        let mut rows = vec![format!(
            "pool=BUILDING live_sample={} busy={busy}",
            oldest.len()
        )];
        let mut sorted: Vec<_> = owners.into_iter().collect();
        sorted.sort_unstable_by_key(|&(k, n)| (std::cmp::Reverse(n), k));
        for (k, n) in sorted.into_iter().take(20) {
            rows.push(format!("owner_rva={} live={n}", rva(k)))
        }
        for h in hist {
            let k = h[0].load(Acquire);
            if k != 0 {
                rows.push(format!(
                    "caller_rva={} alloc={} free={} released_from_owner={}",
                    rva(k),
                    h[1].load(Acquire),
                    h[2].load(Acquire),
                    h[3].load(Acquire)
                ))
            }
        }
        oldest.sort_unstable();
        for (tick, i, k, p) in oldest.into_iter().take(20) {
            let age = now.saturating_sub(tick);
            rows.push(format!(
                "oldest slot={i} owner_rva={} age_ticks={age} age_ms_est={:.3} alloc_srl={}",
                rva(k),
                age as f64 * ms_per_tick,
                id(p)
            ))
        }
        for (p, n) in phases {
            rows.push(format!("alloc_srl={} live={n}", id(p)))
        }
        rows
    }
    #[test]
    fn slot_table_and_summary() {
        let slots = [
            [2, 0x124, 0, 40],
            [4, 0, 42, 9],
            [2, 0x124, 42, 20],
            [1, 2, 2, 1],
        ]
        .map(|s| s.map(AtomicU64::new));
        let hist = [[0x124, 3, 0, 1], [0x234, 0, 1, 0]].map(|s| s.map(AtomicU64::new));
        let mut csv = String::new();
        let rows = lines(&slots, &hist, 60, 0.5, Some(&mut csv));
        assert_eq!(csv.lines().count(), 3);
        assert!(csv.contains("2,0x123,20,40,20.000,0x0000002a\n"));
        assert_eq!(
            rows,
            [
                "pool=BUILDING live_sample=2 busy=1",
                "owner_rva=0x123 live=2",
                "caller_rva=0x123 alloc=3 free=0 released_from_owner=1",
                "caller_rva=0x233 alloc=0 free=1 released_from_owner=0",
                "oldest slot=2 owner_rva=0x123 age_ticks=40 age_ms_est=20.000 alloc_srl=0x0000002a",
                "oldest slot=0 owner_rva=0x123 age_ticks=20 age_ms_est=10.000 alloc_srl=0x00000000",
                "alloc_srl=0x00000000 live=1",
                "alloc_srl=0x0000002a live=1"
            ]
        );
    }
}
#[cfg(all(windows, target_arch = "x86_64"))]
pub use runtime::{install, summary};
#[cfg(all(windows, target_arch = "x86_64"))]
mod runtime {
    use super::{report::*, *};
    use std::{
        io::Write,
        ptr,
        sync::{
            OnceLock,
            atomic::{
                AtomicU64,
                Ordering::{AcqRel, Acquire},
            },
        },
        time::{Duration, Instant},
    };
    use windows_sys::Win32::System::{
        Diagnostics::Debug::FlushInstructionCache, Memory::*, Threading::GetCurrentProcess,
    };
    std::arch::global_asm!(l9_source!(), options(raw));
    unsafe extern "C" {
        static l9_begin: u8;
        static l9_end: u8;
        fn atexit(f: extern "C" fn()) -> i32;
    }
    static SLOTS: [Row; 22500] = [const { [const { AtomicU64::new(0) }; 4] }; 22500];
    static HIST: [Row; 4096] = [const { [const { AtomicU64::new(0) }; 4] }; 4096];
    static STATS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
    static START: OnceLock<(Instant, u64)> = OnceLock::new();
    static SEEN: AtomicU64 = AtomicU64::new(0);
    fn ticks() -> u64 {
        unsafe { std::arch::x86_64::_rdtsc() }
    }
    pub fn summary(reason: &str) {
        let Some((start, origin)) = START.get() else {
            return;
        };
        let now = ticks();
        let scale =
            start.elapsed().as_secs_f64() * 1000. / now.saturating_sub(*origin).max(1) as f64;
        let mut text = format!(
            "VIctory pool-owner: summary={reason} tracked={} histogram_loss={} replaced_live={} free_untracked={}\n",
            STATS[0].load(Acquire),
            STATS[2].load(Acquire),
            STATS[3].load(Acquire),
            STATS[4].load(Acquire)
        );
        let out = matches!(reason, "5149aec" | "exit")
            .then(|| std::env::var_os("VICTORY_POOL_OWNER_DIR"))
            .flatten()
            .filter(|s| !s.is_empty());
        let mut csv = String::new();
        for line in lines(&SLOTS, &HIST, now, scale, out.as_ref().map(|_| &mut csv)) {
            text.push_str("VIctory pool-owner: ");
            text.push_str(&line);
            text.push('\n');
        }
        if let Some(dir) = out {
            let name = format!("pool-owner-{reason}-{now}.csv");
            let path = std::path::PathBuf::from(dir).join(&name);
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .and_then(|mut f| f.write_all(csv.as_bytes()))
            {
                Ok(()) => text.push_str(&format!("VIctory pool-owner: slots_file={name}\n")),
                Err(e) => text.push_str(&format!(
                    "VIctory pool-owner: warn: slots_file={name}: {e}\n"
                )),
            }
        }
        let _ = std::io::stderr().write_all(text.as_bytes());
    }
    fn milestones() {
        let mask = STATS[1].load(Acquire);
        let new = mask & !SEEN.fetch_or(mask, AcqRel);
        for i in 0..32 {
            if new & (1 << i) != 0 {
                summary(&format!("power:{}:deferred", 1u64 << i));
            }
        }
    }
    extern "C" fn finish() {
        milestones();
        summary("exit")
    }
    #[test]
    fn wine_pool_owner() {
        unsafe {
            // Synthetic code and data only. Two independent allocation callers, same
            // Win64 frame depth as the title. Reuse exercises free-before-publication.
            let size = GLOBAL + 0x1000;
            let base = VirtualAlloc(
                ptr::null(),
                size,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_EXECUTE_READWRITE,
            ) as usize;
            assert_ne!(base, 0);
            let put = |r: usize, b: &[u8]| {
                ptr::copy_nonoverlapping(b.as_ptr(), (base + r) as *mut u8, b.len())
            };
            for &(r, b, _) in &SITES {
                put(r, b)
            }
            put(SITES[2].0, &[0xcc]);
            assert!(install(base, size, true).is_err());
            for &(r, b, _) in &SITES[..2] {
                assert_eq!(
                    std::slice::from_raw_parts((base + r) as *const u8, b.len()),
                    b
                )
            }
            put(SITES[2].0, SITES[2].1);
            let mut nodes = vec![0u8; 22500 * 224];
            let mut next = vec![0u32; 22500];
            next[0] = 1;
            next[1] = 2;
            let mut pool = [0u64; 8];
            pool[0] = nodes.as_mut_ptr() as u64;
            pool[1] = 224u64 | (22500u64 << 32);
            pool[5] = next.as_mut_ptr() as u64;
            put(GLOBAL, &(pool.as_ptr() as usize).to_le_bytes());
            let branch = |r: usize, dest: usize, op: u8| {
                put(r, &[op]);
                put(r + 1, &((dest as i64 - r as i64 - 5) as i32).to_le_bytes());
            };
            for (k, &(r, b, _)) in SITES.iter().enumerate() {
                let caller = 0x2000 + 0x100 * k;
                let body = caller + 0x40;
                put(caller, &[0x48, 0x83, 0xec, 0x28]);
                branch(caller + 4, body, 0xe8);
                put(caller + 9, &[0x48, 0x83, 0xc4, 0x28, 0xc3]);
                if k < 2 {
                    put(
                        body,
                        &[0x56, 0x57, 0x48, 0x83, 0xec, 0x28, 0x48, 0x89, 0xcf],
                    );
                    branch(body + 9, r, 0xe9);
                    put(r + b.len(), &[0x5f, 0x5e, 0xc3]);
                } else {
                    put(
                        body,
                        &[0x56, 0x57, 0x53, 0x48, 0x83, 0xec, 0x20, 0x48, 0x89, 0xd0],
                    );
                    branch(body + 10, r, 0xe9);
                    put(
                        r + b.len(),
                        &[
                            0x49, 0x8b, 0x04, 0x92, 0x48, 0x83, 0xc4, 0x20, 0x5b, 0x5f, 0x5e, 0xc3,
                        ],
                    );
                }
            }
            static PHASE: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
            install_inner(base, size, true, PHASE.as_ptr() as usize).unwrap();
            let aa: extern "win64" fn(usize) -> usize = std::mem::transmute(base + 0x2000);
            let ab: extern "win64" fn(usize) -> usize = std::mem::transmute(base + 0x2100);
            let free: extern "win64" fn(usize, usize) -> u64 = std::mem::transmute(base + 0x2200);
            let mut head = 0usize;
            let pop = |alloc: extern "win64" fn(usize) -> usize, head: &mut usize| {
                let index = *head;
                *head = next[index] as usize;
                let p = nodes.as_ptr() as usize + index * 224;
                assert_eq!(alloc(p), p);
                index
            };
            let i = pop(aa, &mut head);
            let j = pop(ab, &mut head);
            assert_eq!((i, j), (0, 1));
            assert_eq!(aa(0), 0);
            assert_eq!(aa(1), 1);
            assert_eq!(free(pool.as_ptr() as usize, i) as u32, 1);
            next[i] = head as u32;
            head = i;
            // L8 record layout: seq zero means unobserved, even nonzero publishes the low u32 ID.
            PHASE[1].store(42 | (2 << 40), std::sync::atomic::Ordering::Release);
            PHASE[0].store(2, std::sync::atomic::Ordering::Release);
            assert_eq!(
                ab(nodes.as_ptr() as usize + head * 224),
                nodes.as_ptr() as usize
            );
            let rows = lines(&SLOTS, &HIST, ticks(), 0., None);
            for expected in [
                "owner_rva=0x2109 live=2",
                "caller_rva=0x2009 alloc=1 free=0 released_from_owner=1",
                "caller_rva=0x2209 alloc=0 free=1 released_from_owner=0",
                "alloc_srl=unknown live=1",
                "alloc_srl=0x0000002a live=1",
            ] {
                assert!(rows.iter().any(|x| x == expected), "{rows:?}")
            }
            assert_eq!(STATS[0].load(Acquire), 2);
            assert_eq!(STATS[1].load(Acquire) & 3, 3);
            summary("wine-synthetic");
        }
    }
    /// Safety: caller owns a mapped image and no title threads can yet execute it.
    pub unsafe fn install(base: usize, size: usize, known: bool) -> Result<(), &'static str> {
        unsafe {
            install_inner(
                base,
                size,
                known,
                super::super::prestream::last_phase_address(),
            )
        }
    }
    unsafe fn install_inner(
        base: usize,
        size: usize,
        known: bool,
        phase_address: usize,
    ) -> Result<(), &'static str> {
        if !known || GLOBAL + 8 > size {
            return Err("unrecognized title build");
        }
        if START.get().is_some() {
            return Err("already installed");
        }
        for &(r, b, _) in &SITES {
            if r + b.len() > size
                || unsafe { std::slice::from_raw_parts((base + r) as *const u8, b.len()) } != b
            {
                return Err("original bytes mismatch; no sites installed");
            }
        }
        let mut pool = ptr::null_mut();
        'outer: for step in 65..1900 {
            for hint in [base.checked_sub(step << 20), base.checked_add(step << 20)]
                .into_iter()
                .flatten()
                .filter(|&h| h >= 65536)
            {
                pool = unsafe {
                    VirtualAlloc(hint as _, 65536, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE)
                };
                if !pool.is_null() {
                    break 'outer;
                }
            }
        }
        if pool.is_null() {
            return Err("no rel32 pool");
        }
        let res = (|| {
            let jumps = transfers(base as u64, pool as u64).ok_or("rel32 out of range")?;
            let src = &raw const l9_begin;
            let len = (&raw const l9_end as usize).wrapping_sub(src as usize);
            if !(0xc0..65536).contains(&len) {
                return Err("invalid template size");
            }
            let [a, b, f] = continuations(base);
            let header = [
                base,
                base + GLOBAL,
                SLOTS.as_ptr() as usize,
                HIST.as_ptr() as usize,
                phase_address,
                STATS.as_ptr() as usize,
                a,
                b,
                f,
                size,
            ];
            unsafe {
                ptr::copy_nonoverlapping(src, pool.cast(), len);
                ptr::copy_nonoverlapping(header.as_ptr(), pool.cast(), header.len())
            };
            let mut old = 0;
            if unsafe { VirtualProtect(pool, 65536, PAGE_EXECUTE_READ, &mut old) } == 0 {
                return Err("template protection failed");
            }
            let mut pages = Vec::new();
            for &(r, b, _) in &SITES {
                if unsafe {
                    VirtualProtect((base + r) as _, b.len(), PAGE_EXECUTE_READWRITE, &mut old)
                } == 0
                {
                    for &(p, n, flags) in pages.iter().rev() {
                        unsafe { VirtualProtect(p, n, flags, &mut old) };
                    }
                    return Err("site protection failed; no sites installed");
                }
                pages.push(((base + r) as _, b.len(), old))
            }
            for ((r, _, _), j) in SITES.iter().zip(jumps) {
                unsafe { ptr::copy_nonoverlapping(j.as_ptr(), (base + r) as *mut u8, j.len()) }
            }
            for (p, n, flags) in pages.into_iter().rev() {
                unsafe { VirtualProtect(p, n, flags, &mut old) };
            }
            unsafe { FlushInstructionCache(GetCurrentProcess(), ptr::null(), 0) };
            Ok(())
        })();
        if let Err(e) = res {
            unsafe { VirtualFree(pool, 0, MEM_RELEASE) };
            return Err(e);
        }
        START.set((Instant::now(), ticks())).ok();
        if unsafe { atexit(finish) } != 0 {
            eprintln!("VIctory pool-owner: warn: exit callback unavailable")
        }
        if std::thread::Builder::new()
            .name("pool-owner-counts".into())
            .spawn(|| {
                loop {
                    milestones();
                    std::thread::sleep(Duration::from_millis(250));
                }
            })
            .is_err()
        {
            eprintln!("VIctory pool-owner: warn: milestone reporter unavailable")
        }
        Ok(())
    }
}
