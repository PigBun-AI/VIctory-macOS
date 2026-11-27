//! L6: a diagnostic for the title's 16-bit `lock cmpxchg` (`VICTORY_MAC_CMPXCHG16_CHECK=1`, off by default).
//!
//! Rosetta 2 under Wine loses the update of a 16-bit `lock cmpxchg [m], r16` whose address is
//! 15 mod 16 when the compare succeeds: memory is not written but ZF is set (Wine probe
//! 2026-11-28, `lock_cmpxchg16_hit` at offsets 15, 31, 47, 63, ... and 4095; every other width
//! and offset, and the non-locked form, are right). The title has 2036 of them (all locked).
//! Only an odd address can be 15 mod 16, so a hit needs a 16-bit atomic field at an odd address,
//! which compilers do not make unless the type is packed. Statically (`tools/mac/census-cmpxchg.py.txt`)
//! none of the 2036 can say so by its operand: 70 are RIP-relative (all at addresses 0, 2, 4 or 8
//! mod 16), 1877 are `[base + even disp]`, 63 `[rsp/rbp + even disp]`, 26 `[base + 2*index + even disp]`.
//! What is left is a base register holding an odd pointer, which only a run can show.
//!
//! This is that run. Each site is redone as a jump to a trampoline that counts the execution, checks
//! `address & 15 == 15`, records the address if so, and then runs the original instruction (copied,
//! RIP-relative displacement redone) with flags, rcx and everything else as they were. A thread
//! prints, every few seconds, how many sites have run and every site that met such an address.
//! If it ever prints one, the rewrite to a safe sequence (see PATCHES.md, L6) is worth its day.
//! Build 1.0.2.1 only is not needed: the scan is bytes and the check is the same on every build.

use super::masked::{Store, lea_operand, operand};
use super::MEM_EXECUTE;
use victory_pe::ImageInfo;

/// One 16-bit `lock cmpxchg` with a memory operand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// RVA of its first prefix byte.
    pub rva: u32,
    pub(crate) store: Store,
    /// The instruction's bytes.
    pub(crate) bytes: Vec<u8>,
}

/// The instruction at `at`: `66 f0 [rex] 0f b1 /r` or `f0 66 [rex] 0f b1 /r` with a memory operand.
pub(crate) fn decode(code: &[u8], at: usize) -> Option<Site> {
    let &[a, b] = code.get(at..at + 2)? else {
        return None;
    };
    if !matches!((a, b), (0x66, 0xf0) | (0xf0, 0x66)) {
        return None;
    }
    let mut i = at + 2;
    let mut rex = 0u8;
    if let Some(&r) = code.get(i).filter(|r| **r >> 4 == 4) {
        rex = r;
        i += 1;
    }
    if code.get(i..i + 2)? != [0x0f, 0xb1] {
        return None;
    }
    let (modrm, sib, disp, end) = operand(code, i + 2)?;
    if modrm >> 6 == 3 {
        return None;
    }
    Some(Site {
        rva: 0,
        store: Store {
            len: end - at,
            wide_lanes: false,
            ymm: false,
            mask: 0,
            src: 0,
            x: rex & 2 != 0,
            b: rex & 1 != 0,
            modrm,
            sib,
            disp,
            plain: None,
        },
        bytes: code[at..end].to_vec(),
    })
}

/// Every such instruction in the executable sections of `bytes`.
pub fn scan(info: &ImageInfo, bytes: &[u8]) -> Vec<Site> {
    let mut found = Vec::new();
    for s in info
        .sections
        .iter()
        .filter(|s| s.characteristics & MEM_EXECUTE != 0)
    {
        let start = (s.file_offset as usize).min(bytes.len());
        let end = start.saturating_add(s.file_size as usize).min(bytes.len());
        let code = &bytes[start..end];
        for at in 0..code.len() {
            if let Some(mut site) = decode(code, at) {
                if let Some(rva) = u32::try_from(at).ok().and_then(|o| s.rva.checked_add(o)) {
                    site.rva = rva;
                    found.push(site);
                }
            }
        }
    }
    found
}

/// Bytes of one site's counters in the pool: executions, hits, the last hit's address, spare.
pub(crate) const SLOT: usize = 32;

/// `inc qword [rip + target]` / `mov [rip + target], rcx`, `target` an absolute address, appended
/// to `c`, which will sit at `at`.
fn rip_op(c: &mut Vec<u8>, opcode: &[u8], target: u64, at: u64) -> Option<()> {
    c.extend_from_slice(opcode);
    let next = at as i64 + c.len() as i64 + 4;
    c.extend_from_slice(&i32::try_from(target as i64 - next).ok()?.to_le_bytes());
    Some(())
}

/// The trampoline for `site` (the instruction is at `at_site`), placed at `at`, counting in the
/// slot at `slot`: counts the run, checks the address, records a hit, then the instruction
/// itself and a jump back.
pub(crate) fn trampoline(site: &Site, at_site: u64, at: u64, slot: u64) -> Option<Vec<u8>> {
    let s = &site.store;
    let mut c = vec![0x51, 0x9c]; // push rcx; pushfq
    rip_op(&mut c, &[0x48, 0xff, 0x05], slot, at)?; // inc qword [slot]  (the runs)
    lea_operand(&mut c, 1, s, at_site, at, 16)?; // lea rcx, [operand]
    c.extend_from_slice(&[0x83, 0xe1, 0x0f, 0x83, 0xf9, 0x0f]); // and ecx, 15; cmp ecx, 15
    // jne past the hit block, whose size is known by building it first.
    let hit_at = at + c.len() as u64 + 2;
    let mut hit = Vec::new();
    lea_operand(&mut hit, 1, s, at_site, hit_at, 16)?;
    rip_op(&mut hit, &[0x48, 0x89, 0x0d], slot + 16, hit_at)?; // mov [slot+16], rcx
    rip_op(&mut hit, &[0x48, 0xff, 0x05], slot + 8, hit_at)?; // inc qword [slot+8]
    c.extend_from_slice(&[0x75, u8::try_from(hit.len()).ok()?]);
    c.extend(hit);
    c.extend_from_slice(&[0x9d, 0x59]); // popfq; pop rcx
    // The instruction, its RIP-relative displacement redone for here.
    let here = at + c.len() as u64;
    let mut copy = site.bytes.clone();
    if s.modrm >> 6 == 0 && s.modrm & 7 == 5 {
        let n = copy.len();
        let target = at_site as i64 + n as i64 + i64::from(s.disp);
        let disp = i32::try_from(target - (here as i64 + n as i64)).ok()?;
        copy[n - 4..].copy_from_slice(&disp.to_le_bytes());
    }
    c.extend(copy);
    let back = at_site as i64 + s.len as i64;
    let next = at as i64 + c.len() as i64 + 5;
    c.push(0xe9);
    c.extend_from_slice(&i32::try_from(back - next).ok()?.to_le_bytes());
    Some(c)
}

/// What the check has seen: sites, how many ran, and the hits.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Seen {
    pub sites: usize,
    pub ran: usize,
    /// (RVA, executions, hits, last address 15 mod 16).
    pub hits: Vec<(u32, u64, u64, u64)>,
}

/// Reads the counters at `slots` (one per RVA in `rvas`).
///
/// # Safety
/// `slots` points at `rvas.len() * SLOT` readable bytes.
pub(crate) unsafe fn read_slots(slots: *const u8, rvas: &[u32]) -> Seen {
    let mut seen = Seen {
        sites: rvas.len(),
        ..Seen::default()
    };
    for (i, &rva) in rvas.iter().enumerate() {
        // SAFETY: inside the slots, plain counters another thread may be writing.
        let (runs, hits, last) = unsafe {
            let p = slots.add(i * SLOT).cast::<u64>();
            (
                p.read_volatile(),
                p.add(1).read_volatile(),
                p.add(2).read_volatile(),
            )
        };
        seen.ran += usize::from(runs != 0);
        if hits != 0 {
            seen.hits.push((rva, runs, hits, last));
        }
    }
    seen
}

/// Whether `VICTORY_MAC_CMPXCHG16_CHECK` asks for the check (`1`).
pub fn wanted() -> bool {
    std::env::var("VICTORY_MAC_CMPXCHG16_CHECK").is_ok_and(|v| v == "1")
}

/// Redoes the sites of the image mapped at `base` as counting trampolines and starts the
/// reporting thread; returns how many it patched. Before any title thread runs.
#[cfg(windows)]
pub fn patch(base: u64, info: &ImageInfo, file: &[u8]) -> usize {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualProtect,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let sites = scan(info, file);
    if sites.is_empty() {
        return 0;
    }
    // Slots, then trampolines, within reach of a rel32 jump (below the image first). The pool is
    // writable and executable: this is a diagnostic.
    let size = (sites.len() * (SLOT + 96)).next_multiple_of(1 << 16);
    let mut pool = std::ptr::null_mut();
    for step in 1..2048u64 {
        let Some(want) = base.checked_sub((step + 64) << 20) else {
            break;
        };
        // SAFETY: a plain reservation at a hint; failure returns null.
        pool = unsafe {
            VirtualAlloc(want as *const _, size, MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE)
        };
        if !pool.is_null() {
            break;
        }
    }
    if pool.is_null() {
        tracing::warn!("no memory near the image for the cmpxchg16 check");
        return 0;
    }
    let slots = pool as u64;
    let mut used = sites.len() * SLOT;
    let mut rvas = Vec::new();
    for site in &sites {
        let at_site = base + u64::from(site.rva);
        let at = pool as u64 + used as u64;
        let slot = slots + (rvas.len() * SLOT) as u64;
        let (Some(code), Some(jump)) = (
            trampoline(site, at_site, at, slot),
            super::masked::jump(at_site, site.store.len, at),
        ) else {
            continue;
        };
        if used + code.len() > size {
            break;
        }
        let mut old = 0;
        // SAFETY: our pool has room; the site is the title's code, writable for the moment of the
        // write, before any title thread runs.
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), at as *mut u8, code.len());
            if VirtualProtect(at_site as *const _, jump.len(), PAGE_EXECUTE_READWRITE, &mut old) == 0 {
                continue;
            }
            std::ptr::copy_nonoverlapping(jump.as_ptr(), at_site as *mut u8, jump.len());
            VirtualProtect(at_site as *const _, jump.len(), old, &mut old);
        }
        used = (used + code.len() + 15) & !15;
        rvas.push(site.rva);
    }
    // SAFETY: as above.
    unsafe { FlushInstructionCache(GetCurrentProcess(), std::ptr::null(), 0) };
    let patched = rvas.len();
    let slots = slots as usize;
    std::thread::Builder::new()
        .name("cmpxchg16-check".into())
        .spawn(move || report_loop(slots, rvas))
        .ok();
    patched
}

/// Prints what the counters say, every 5 s while it changes (and every minute anyway).
#[cfg(windows)]
fn report_loop(slots: usize, rvas: Vec<u32>) {
    let mut last = Seen::default();
    let mut quiet = 0;
    loop {
        std::thread::sleep(std::time::Duration::from_secs(5));
        // SAFETY: the pool stays allocated for the life of the process.
        let seen = unsafe { read_slots(slots as *const u8, &rvas) };
        quiet += 1;
        if seen != last || quiet >= 12 {
            quiet = 0;
            println!(
                "cmpxchg16      {} of {} sites ran, {} met an address = 15 mod 16{}",
                seen.ran,
                seen.sites,
                seen.hits.len(),
                seen.hits
                    .iter()
                    .take(8)
                    .map(|(rva, runs, hits, at)| format!(
                        "\n  site {rva:#x}: {hits} of {runs} runs, last at {at:#x}"
                    ))
                    .collect::<String>()
            );
            last = seen;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_both_prefix_orders_with_and_without_rex() {
        // 66 f0 0f b1 97 58 01 00 00: lock cmpxchg word ptr [rdi+0x158], dx (from the title)
        let s = decode(&[0x66, 0xf0, 0x0f, 0xb1, 0x97, 0x58, 0x01, 0x00, 0x00], 0).unwrap();
        assert_eq!((s.store.len, s.store.disp, s.store.b), (9, 0x158, false));
        // 66 f0 43 0f b1 0c 48: [r8 + 2*r9], cx: REX.XB
        let s = decode(&[0x66, 0xf0, 0x43, 0x0f, 0xb1, 0x0c, 0x48], 0).unwrap();
        assert_eq!((s.store.len, s.store.x, s.store.b), (7, true, true));
        // f0 66 0f b1 0a: [rdx], cx
        let s = decode(&[0xf0, 0x66, 0x0f, 0xb1, 0x0a], 0).unwrap();
        assert_eq!(s.store.len, 5);
        // rip-relative
        let s = decode(&[0x66, 0xf0, 0x0f, 0xb1, 0x0d, 0x10, 0, 0, 0], 0).unwrap();
        assert_eq!((s.store.len, s.store.disp), (9, 0x10));
    }

    #[test]
    fn leaves_other_widths_registers_and_unlocked_alone() {
        let no = |b: &[u8]| assert_eq!(decode(b, 0), None, "{b:02x?}");
        no(&[0xf0, 0x0f, 0xb1, 0x0a]); // 32-bit
        no(&[0xf0, 0x48, 0x0f, 0xb1, 0x0a]); // 64-bit
        no(&[0x66, 0x0f, 0xb1, 0x0a]); // not locked
        no(&[0x66, 0xf0, 0x0f, 0xb1, 0xca]); // register operand
        no(&[0x66, 0xf0, 0x0f, 0xb0, 0x0a]); // the 8-bit opcode
        no(&[0x66, 0xf0, 0x0f]); // cut short
    }

    #[test]
    fn the_trampoline_counts_checks_and_runs_the_instruction() {
        let code = [0x66, 0xf0, 0x0f, 0xb1, 0x0a]; // lock cmpxchg [rdx], cx
        let mut site = decode(&code, 0).unwrap();
        site.rva = 0x1000;
        let t = trampoline(&site, 0x1_4000_1000, 0x1_3000_0000, 0x1_3000_0000 - 64).unwrap();
        // push rcx; pushfq; inc qword [rip+..] ...
        assert_eq!(&t[..5], &[0x51, 0x9c, 0x48, 0xff, 0x05]);
        // ends with the instruction's own bytes and a jump back past it
        let n = t.len();
        assert_eq!(&t[n - 10..n - 5], &code);
        assert_eq!(t[n - 5], 0xe9);
        let rel = i32::from_le_bytes(t[n - 4..].try_into().unwrap());
        assert_eq!(0x1_3000_0000 + n as i64 + i64::from(rel), 0x1_4000_1000 + 5);
        // the jne skips exactly the hit block: it lands on `popfq`
        let at = t.iter().position(|&b| b == 0x75).unwrap();
        assert_eq!(t[at + 2 + t[at + 1] as usize], 0x9d);
    }

    #[test]
    fn a_rip_relative_operand_is_redone_for_the_trampoline() {
        let code = [0x66, 0xf0, 0x0f, 0xb1, 0x0d, 0x10, 0, 0, 0]; // [rip+0x10], cx
        let site = decode(&code, 0).unwrap();
        let (at_site, at) = (0x1_4000_1000u64, 0x1_3000_0000u64);
        let t = trampoline(&site, at_site, at, at - 64).unwrap();
        let n = t.len();
        let copy = &t[n - 5 - 9..n - 5];
        let disp = i32::from_le_bytes(copy[5..].try_into().unwrap());
        // the operand's address is the same from the copy as from the original
        assert_eq!(
            at + (n - 5) as i64 as u64 + disp as i64 as u64,
            at_site + 9 + 0x10
        );
        // the lea in the trampoline reaches the same address too (rip-relative, disp redone)
    }

    #[test]
    fn the_slots_are_read_back() {
        let mut mem = vec![0u64; 8];
        mem[0] = 5; // site 0: ran 5 times, no hit
        mem[4] = 9; // site 1: ran 9 times,
        mem[5] = 2; // 2 hits,
        mem[6] = 0x1000f; // the last at 0x1000f
        // SAFETY: our vector, 2 slots of 32 bytes.
        let seen = unsafe { read_slots(mem.as_ptr().cast(), &[0x10, 0x20]) };
        assert_eq!(
            seen,
            Seen {
                sites: 2,
                ran: 2,
                hits: vec![(0x20, 9, 2, 0x1000f)]
            }
        );
    }

    /// The trampolines against the instructions themselves: the same memory, AX, ZF and the
    /// other registers after, and the counters count what ran and what met 15 mod 16. Needs
    /// Windows (executable memory at an address within 2 GiB of the code); run under Wine.
    #[cfg(all(windows, target_arch = "x86_64"))]
    #[test]
    fn the_checked_instruction_does_what_the_instruction_does() {
        use windows_sys::Win32::System::Memory::*;
        // f(p rcx, new rdx, old r8) -> rax = AX after, rdx = flags: mov rax, r8; <instr with
        // [rcx+disp]>; pushfq; pop rdx...  Build: mov rax,r8 ; lock cmpxchg [rcx+disp], dx ; ret
        // with the flags returned through r9 (pointer in the 4th argument).
        let forms: [(&str, Vec<u8>); 3] = [
            ("[rcx], dx", vec![0x66, 0xf0, 0x0f, 0xb1, 0x11]),
            ("[rcx+0x20], dx", vec![0x66, 0xf0, 0x0f, 0xb1, 0x51, 0x20]),
            ("[rcx+2*rdx-0x10], dx", vec![0x66, 0xf0, 0x0f, 0xb1, 0x54, 0x51, 0xf0]),
        ];
        // SAFETY: our own allocations and code.
        unsafe {
            let page = VirtualAlloc(std::ptr::null(), 1 << 16, MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE)
                as *mut u8;
            let data = VirtualAlloc(std::ptr::null(), 1 << 16, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) as *mut u8;
            assert!(!page.is_null() && !data.is_null());
            for (k, (what, instr)) in forms.iter().enumerate() {
                let mut code = vec![0x4c, 0x89, 0xc0]; // mov rax, r8
                let site_at = code.len();
                code.extend_from_slice(instr);
                // pushfq; pop r10; mov [r9], r10; ret
                code.extend_from_slice(&[0x9c, 0x41, 0x5a, 0x4d, 0x89, 0x11, 0xc3]);
                let base = page.add(k * 0x400);
                std::ptr::copy_nonoverlapping(code.as_ptr(), base, code.len());
                let mut site = decode(&code, site_at).unwrap();
                site.rva = 0;
                let slot = page.add(0x8000 + k * SLOT);
                std::ptr::write_bytes(slot, 0, SLOT);
                let plain: extern "win64" fn(*mut u8, u64, u64, *mut u64) -> u64 = std::mem::transmute(base);
                // the patched copy of the same code
                let copy = page.add(0x4000 + k * 0x400);
                std::ptr::copy_nonoverlapping(code.as_ptr(), copy, code.len());
                let at2 = copy.add(0x200) as u64;
                let t2 = trampoline(&site, copy as u64 + site_at as u64, at2, slot as u64).unwrap();
                assert!(t2.len() < 0x200);
                std::ptr::copy_nonoverlapping(t2.as_ptr(), at2 as *mut u8, t2.len());
                let j = super::super::masked::jump(copy as u64 + site_at as u64, site.store.len, at2).unwrap();
                std::ptr::copy_nonoverlapping(j.as_ptr(), copy.add(site_at), j.len());
                let patched: extern "win64" fn(*mut u8, u64, u64, *mut u64) -> u64 = std::mem::transmute(copy);
                let mut hits = 0;
                for off in [0usize, 2, 14, 15, 16, 31, 100, 4094, 4095] {
                    for (old, new, cmp) in [(0x1234u64, 0xbeefu64, 0x1234u64), (0x1234, 0xbeef, 0x4321)] {
                        let run = |f: extern "win64" fn(*mut u8, u64, u64, *mut u64) -> u64| {
                            let addr = data.add(0x400 + off);
                            addr.cast::<u16>().write_unaligned(old as u16);
                            addr.add(2).write(0x55);
                            // The register the form addresses from, so that it lands on `addr`;
                            // the last form also uses dx as the index, so dx is 8 there.
                            let (p, new) = match k {
                                1 => (addr.sub(0x20), new),
                                2 => (addr, 8),
                                _ => (addr, new),
                            };
                            let mut flags = 0u64;
                            let rax = f(p, new, cmp, &mut flags);
                            (rax & 0xffff, flags & 0x8d5, addr.cast::<u16>().read_unaligned(), addr.add(2).read())
                        };
                        let a = run(plain);
                        let b = run(patched);
                        assert_eq!(a, b, "{what} off {off} cmp {cmp:#x}");
                    }
                    // `data` is 64 KiB aligned: the address's low bits are 0x400 + off's.
                    if (0x400 + off) & 15 == 15 {
                        hits += 1;
                    }
                }
                let runs = slot.cast::<u64>().read();
                let met = slot.add(8).cast::<u64>().read();
                assert_eq!(runs, 18, "{what}: every run counted");
                assert_eq!(met, 2 * hits, "{what}: only 15 mod 16 counts");
                if hits > 0 {
                    assert_eq!(slot.add(16).cast::<u64>().read() & 15, 15, "{what}");
                }
            }
            VirtualFree(page.cast(), 0, MEM_RELEASE);
            VirtualFree(data.cast(), 0, MEM_RELEASE);
        }
    }
}
