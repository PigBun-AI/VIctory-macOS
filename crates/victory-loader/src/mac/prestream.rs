//! L8: read-only SRL evidence. The writer is integer-only position-independent x64;
//! the Rust side never holds a lock needed by a title thread.

#[macro_use]
#[path = "prestream_asm.rs"]
mod assembly;
/// Same source passed to LLVM by the runtime and by the offline disassembly fixture.
pub const ASSEMBLY: &str = l8_source!();

/// Original mapped bytes, including the short-jump relay in alignment padding.
pub const SITES: &[(usize, &[u8])] = &[
    (0x4c3cf54, &[0xe8, 7, 0xbd, 0x65, 0xfd]),
    (0x4c3cf59, &[0x34, 1, 0x0f, 0xb6, 0xc0]),
    (0x227f357, &[0x8b, 0x40, 8, 0x3b, 0x46, 12]),
    (0x2299582, &[0x88, 0x58, 0x28]),
    (0x2299598, &[0xcc; 5]),
    (0x227cec0, &[0x56, 0x57, 0x48, 0x83, 0xec, 0x28]),
];
/// No prefix matches: length, all bytes and all sites must pass before any write.
pub fn matches(index: usize, bytes: &[u8]) -> bool {
    SITES.get(index).is_some_and(|s| s.1 == bytes)
}
/// Site transfers used by both the runtime and offline fixture. Never overwrite
/// +2299585: it is the destination of *two* independent conditional branches.
pub fn transfers(base: u64, pool: u64) -> Option<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    for (i, ((rva, orig), offset)) in SITES
        .iter()
        .zip([0x80, 0x100, 0x200, 0, 0x300, 0x380])
        .enumerate()
    {
        if i == 3 {
            out.push(vec![0xeb, 0x14, 0x90]);
            continue;
        }
        let mut j = super::masked::jump(
            base.checked_add(*rva as u64)?,
            orig.len(),
            pool.checked_add(offset)?,
        )?;
        if i == 0 {
            j[0] = 0xe8;
        }
        out.push(j);
    }
    Some(out)
}
/// Addresses used by the template's RIP-relative indirect transfers (in header order).
pub const DEST: [u64; 5] = [0x2298c60, 0x4c3cf5e, 0x227f35d, 0x2299585, 0x227cec6];

#[cfg(any(all(windows, target_arch = "x86_64"), test))]
mod report {
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Acquire};
    #[repr(C, align(64))]
    pub(super) struct Slot {
        pub pair: AtomicU64,
        pub counts: [AtomicU64; 2],
        pub lane: AtomicU32,
        pad: u32,
        pub cursor_target: AtomicU64,
        pub grade: AtomicU32,
        pub threshold: AtomicU32,
        pub ready: AtomicU32,
        pad2: u32,
        pub lookup: AtomicU64,
    }
    impl Slot {
        pub const fn new() -> Self {
            Self {
                pair: AtomicU64::new(0),
                counts: [const { AtomicU64::new(0) }; 2],
                lane: AtomicU32::new(0),
                pad: 0,
                cursor_target: AtomicU64::new(0),
                grade: AtomicU32::new(0),
                threshold: AtomicU32::new(0),
                ready: AtomicU32::new(0),
                pad2: 0,
                lookup: AtomicU64::new(0),
            }
        }
    }
    const _: () = assert!(size_of::<Slot>() == 64 && std::mem::offset_of!(Slot, lookup) == 56);
    pub(super) fn top(slots: &[Slot]) -> Vec<String> {
        let mut rows: Vec<_> = slots
            .iter()
            .filter_map(|s| {
                let n = s.counts[0].load(Acquire);
                (n > 0 && s.counts[1].load(Acquire) == 0).then_some((n, s.pair.load(Acquire), s))
            })
            .collect();
        rows.sort_unstable_by_key(|(n, k, _)| (std::cmp::Reverse(*n), *k));
        rows.into_iter().take(8).map(|(n,k,s)| {
            let lane=s.lane.load(Acquire); let m=if (1..=6).contains(&lane) {
                let c=s.cursor_target.load(Acquire);
                format!("first_failed_lane={} lookup={:#x} grade={} threshold={} cursor={} target={} ready_e={}",lane-1,s.lookup.load(Acquire),s.grade.load(Acquire) as i32,s.threshold.load(Acquire) as i32,c as u32,c>>32,s.ready.load(Acquire))
            } else {"first_failed_lane=unobserved".into()};
            format!("id={:#010x} key={:#010x} false={n} true=0 {m}",k>>32,k as u32)
        }).collect()
    }
    pub(super) fn powers(last: u64, now: u64) -> impl Iterator<Item = u64> {
        (0..64)
            .map(|n| 1u64 << n)
            .filter(move |&n| n > last && n <= now)
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
            atomic::{AtomicU64, Ordering::Acquire},
        },
        time::{Duration, Instant},
    };
    use windows_sys::Win32::System::{
        Diagnostics::Debug::FlushInstructionCache, Memory::*, Threading::GetCurrentProcess,
    };
    std::arch::global_asm!(l8_source!(), options(raw));
    unsafe extern "C" {
        static l8_begin: u8;
        static l8_end: u8;
        fn atexit(f: extern "C" fn()) -> i32;
    }
    static SLOTS: [Slot; 4097] = [const { Slot::new() }; 4097];
    static LOSS: AtomicU64 = AtomicU64::new(0);
    #[repr(C, align(32))]
    struct Life {
        seq: AtomicU64,
        fields: AtomicU64,
        tick: AtomicU64,
        _pad: u64,
    }
    static LIFE: [Life; 2] = [const {
        Life {
            seq: AtomicU64::new(0),
            fields: AtomicU64::new(0),
            tick: AtomicU64::new(0),
            _pad: 0,
        }
    }; 2];
    static START: OnceLock<(Instant, u64)> = OnceLock::new();
    fn ticks() -> u64 {
        unsafe { std::arch::x86_64::_rdtsc() }
    }
    fn line(text: std::fmt::Arguments<'_>) {
        let _ = std::io::stderr().write_all(format!("VIctory prestream: {text}\n").as_bytes());
    }
    pub fn summary(reason: &str) {
        let Some(&(start, origin)) = START.get() else {
            return;
        };
        let now = ticks();
        let ms = start.elapsed().as_secs_f64() * 1000.;
        let rows = top(&SLOTS);
        line(format_args!(
            "summary={reason} top_always_false={} dropped={} elapsed_ms={ms:.3} tsc={now}",
            rows.len(),
            LOSS.load(Acquire)
        ));
        for row in rows {
            line(format_args!("{row}"))
        }
        for (name, l) in ["phase_write", "remove_enter"].iter().zip(&LIFE) {
            let seq = l.seq.load(Acquire);
            let f = l.fields.load(Acquire);
            let t = l.tick.load(Acquire);
            if seq == 0 || seq & 1 != 0 || seq != l.seq.load(std::sync::atomic::Ordering::SeqCst) {
                line(format_args!("last_{name}=unobserved_or_busy"));
                continue;
            }
            let estimate =
                ms * t.saturating_sub(origin) as f64 / now.saturating_sub(origin).max(1) as f64;
            line(format_args!(
                "last_{name} tsc={t} elapsed_ms_est={estimate:.3} id={:#010x} old={} new_or_ff={} ref={}",
                f as u32,
                (f >> 32) & 255,
                (f >> 40) & 255,
                (f >> 48) & 255
            ));
        }
    }
    extern "C" fn finish() {
        milestones();
        summary("exit")
    }
    fn milestones() {
        static SEEN: [[AtomicU64; 2]; 4097] = [const { [const { AtomicU64::new(0) }; 2] }; 4097];
        for (s, prev) in SLOTS.iter().zip(&SEEN) {
            for (i, v) in prev.iter().enumerate() {
                let n = s.counts[i].load(Acquire);
                let last = v.load(Acquire);
                if last >= n {
                    continue;
                }
                for p in powers(last, n) {
                    if v.fetch_max(p, std::sync::atomic::Ordering::Relaxed) >= p {
                        continue;
                    }
                    let k = s.pair.load(Acquire);
                    line(format_args!(
                        "id={:#010x} key={:#010x} result={i} count={p}",
                        k >> 32,
                        k as u32
                    ));
                }
                v.fetch_max(n, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
    fn monitor() {
        loop {
            milestones();
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    #[test]
    fn wine_mapped_sites_preserve_results_and_join_manager() {
        // A synthetic Win64 caller and title frames, no game code executed. Same
        // install path and moved instructions as the fixture, with real ASLR.
        unsafe {
            let size = 0x4c40000;
            let b = VirtualAlloc(
                ptr::null(),
                size,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_EXECUTE_READWRITE,
            ) as usize;
            assert_ne!(b, 0);
            let put = |r: usize, v: &[u8]| {
                ptr::copy_nonoverlapping(v.as_ptr(), (b + r) as *mut u8, v.len())
            };
            for &(r, v) in SITES {
                put(r, v)
            }
            put(0x4c3cf4c, &[0x48, 0x83, 0xec, 0x28, 0x89, 0x54, 0x24, 0x24]); // sub rsp,28; store key
            put(0x2298c60, &[0x44, 0x89, 0xc0, 0xc3]); // mock predicate AL=r8b
            put(0x4c3cf5e, &[1, 0xc0, 0x48, 0x83, 0xc4, 0x28, 0xc3]);
            put(
                0x227f35d,
                &[0x0f, 0x9d, 0xc0, 0x48, 0x83, 0xc4, 0x20, 0x5e, 0xc3],
            );
            // Mock lane 0's caller, stack locals and exact return address.
            let caller = [
                0x48, 0x83, 0xec, 0x58, 0x48, 0x89, 0x4c, 0x24, 0x48, 0x48, 0x89, 0x54, 0x24, 0x30,
            ];
            put(0x227c6f3 - caller.len(), &caller);
            let mut call = vec![0xe8];
            call.extend_from_slice(&(0x227f340_i32 - 0x227c6f8).to_le_bytes());
            put(0x227c6f3, &call);
            put(0x227c6f8, &[0x48, 0x83, 0xc4, 0x58, 0xc3]);
            let mut setup = vec![
                0x56, 0x48, 0x83, 0xec, 0x20, 0x4c, 0x89, 0xc6, 0x4c, 0x89, 0xc8,
            ];
            setup.resize(0x17, 0x90);
            put(0x227f340, &setup);
            put(0x229957c, &[0x53, 0x48, 0x89, 0xc8, 0x89, 0xd3]);
            put(0x2299585, &[0x5b, 0xc3]);
            put(0x227cec6, &[0x48, 0x83, 0xc4, 0x28, 0x5f, 0x5e, 0xc3]);
            install(b, size, true).unwrap();
            let query: extern "win64" fn(u32, u32, u32) -> u32 = std::mem::transmute(b + 0x4c3cf4c);
            for _ in 0..7 {
                assert_eq!(query(42, 7, 0), 2)
            }
            assert_eq!(query(42, 9, 1), 0);
            let mut obj = [0u8; 0x58];
            obj[0x1c] = 42;
            obj[0x51] = 3;
            let mut ent = [0u8; 0x84];
            ent[4] = 7;
            let manager = [0u32, 0, 0, 2];
            let payload = [11u32, 13, 1, 0];
            let grade: extern "win64" fn(*const u8, *const u8, *const u32, *const u32) -> u8 =
                std::mem::transmute(b + 0x227c6f3 - caller.len());
            assert_eq!(
                grade(
                    obj.as_ptr(),
                    ent.as_ptr(),
                    manager.as_ptr(),
                    payload.as_ptr()
                ),
                0
            );
            assert!(top(&SLOTS).iter().any(|l| {
                l.contains("key=0x00000007 false=7 true=0 first_failed_lane=0")
                    && l.contains("grade=1 threshold=2 cursor=11 target=13")
            }));
            let phase: extern "win64" fn(*mut u8, u32) = std::mem::transmute(b + 0x229957c);
            phase(obj.as_mut_ptr(), 2);
            assert_eq!(obj[0x28], 2);
            let remove: extern "win64" fn(*mut u8) = std::mem::transmute(b + 0x227cec0);
            remove(obj.as_mut_ptr());
            assert_eq!(LIFE[0].fields.load(Acquire), 42 | (2 << 40) | (3 << 48));
            assert_eq!(
                LIFE[1].fields.load(Acquire),
                42 | (2 << 32) | (255 << 40) | (3 << 48)
            );
            assert_ne!(LIFE[0].tick.load(Acquire), 0);
            assert_eq!(LOSS.load(Acquire), 0);
        }
    }
    /// Caller guarantees a live mapped title image and no executing title threads.
    pub unsafe fn install(base: usize, size: usize, known: bool) -> Result<(), &'static str> {
        if !known {
            return Err("unrecognized title build");
        }
        if START.get().is_some() {
            return Err("already installed");
        }
        for (i, &(r, b)) in SITES.iter().enumerate() {
            if r + b.len() > size
                || !matches(i, unsafe {
                    std::slice::from_raw_parts((base + r) as *const u8, b.len())
                })
            {
                return Err("original bytes mismatch; no sites installed");
            }
        }
        let mut pool = ptr::null_mut();
        'near: for step in 65..1900 {
            for hint in [base.checked_sub(step << 20), base.checked_add(step << 20)]
                .into_iter()
                .flatten()
                .filter(|h| *h >= 65536)
            {
                pool = unsafe {
                    VirtualAlloc(hint as _, 65536, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE)
                };
                if !pool.is_null() {
                    break 'near;
                }
            }
        }
        if pool.is_null() {
            return Err("no rel32 pool");
        }
        let p = pool as usize;
        let result = (|| {
            let jumps = transfers(base as u64, p as u64).ok_or("rel32 out of reach")?;
            let src = &raw const l8_begin;
            let len = (&raw const l8_end as usize).wrapping_sub(src as usize);
            if !(0x380..65536).contains(&len) {
                return Err("invalid template size");
            }
            unsafe { ptr::copy_nonoverlapping(src, pool.cast(), len) };
            let mut header = [0usize; 9];
            header[0] = SLOTS.as_ptr() as usize;
            for (v, r) in header[1..6].iter_mut().zip(DEST) {
                *v = base + r as usize;
            }
            header[6] = base;
            header[7] = &LOSS as *const _ as usize;
            header[8] = LIFE.as_ptr() as usize;
            unsafe { ptr::copy_nonoverlapping(header.as_ptr(), pool.cast(), 9) };
            let mut old = 0;
            if unsafe { VirtualProtect(pool, 65536, PAGE_EXECUTE_READ, &mut old) } == 0 {
                return Err("template protection failed");
            }
            // All code pages writable BEFORE making any change. Same-page sites restore in reverse order.
            let mut perms = Vec::new();
            for &(r, b) in SITES {
                if unsafe {
                    VirtualProtect((base + r) as _, b.len(), PAGE_EXECUTE_READWRITE, &mut old)
                } == 0
                {
                    for &(addr, n, prot) in perms.iter().rev() {
                        unsafe { VirtualProtect(addr, n, prot, &mut old) };
                    }
                    return Err("site protection failed; no sites installed");
                }
                perms.push(((base + r) as _, b.len(), old));
            }
            for ((r, _), j) in SITES.iter().zip(jumps) {
                unsafe { ptr::copy_nonoverlapping(j.as_ptr(), (base + r) as *mut u8, j.len()) };
            }
            for &(addr, n, prot) in perms.iter().rev() {
                unsafe { VirtualProtect(addr, n, prot, &mut old) };
            }
            unsafe { FlushInstructionCache(GetCurrentProcess(), ptr::null(), 0) };
            Ok(())
        })();
        if let Err(e) = result {
            unsafe { VirtualFree(pool, 0, MEM_RELEASE) };
            return Err(e);
        }
        START.set((Instant::now(), ticks())).ok();
        if unsafe { atexit(finish) } != 0 {
            line(format_args!("warn: CRT exit callback unavailable"));
        }
        if std::thread::Builder::new()
            .name("srl-counts".into())
            .spawn(monitor)
            .is_err()
        {
            line(format_args!("warn: power-of-two reporter unavailable"));
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::{report::*, *};
    use std::sync::atomic::Ordering::Relaxed;
    #[test]
    fn refuse_wrong_or_short_bytes() {
        for (i, &(_, b)) in SITES.iter().enumerate() {
            assert!(matches(i, b));
            assert!(!matches(i, &b[..b.len() - 1]));
            let mut bad = b.to_vec();
            bad[0] ^= 1;
            assert!(!matches(i, &bad));
        }
    }
    #[test]
    fn count_thresholds() {
        assert_eq!(powers(3, 17).collect::<Vec<_>>(), [4, 8, 16]);
        assert_eq!(powers(1, 1).count(), 0);
    }
    #[test]
    fn exclude_ever_true_and_format_manager() {
        let s = [Slot::new(), Slot::new()];
        s[0].counts[0].store(99, Relaxed);
        s[0].counts[1].store(1, Relaxed);
        s[1].pair.store((7 << 32) | 9, Relaxed);
        s[1].counts[0].store(5, Relaxed);
        s[1].lane.store(2, Relaxed);
        s[1].grade.store(u32::MAX, Relaxed);
        s[1].cursor_target.store((4 << 32) | 2, Relaxed);
        assert_eq!(
            top(&s),
            [
                "id=0x00000007 key=0x00000009 false=5 true=0 first_failed_lane=1 lookup=0x0 grade=-1 threshold=0 cursor=2 target=4 ready_e=0"
            ]
        );
    }
}
