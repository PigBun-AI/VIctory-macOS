//! `clwb` write-back loops turned into NOPs (VIctory-macOS #135).
//!
//! Under Rosetta `clwb` faults, and each loop costs an exception (~300 us
//! through Wine): the world load ran ~15,000 a second on job threads and
//! crawled. The Windows line emulates the instruction in `crash.rs`
//! (`clwb_loop`, one exception per loop); that is right on a PC, but through
//! Wine on a Mac an exception is too dear, so the loops of the supported
//! build are patched out at load instead.
//!
//! The loop recogniser here is a copy of `crash.rs`'s `clwb_loop` (reduced to
//! a yes/no), so that file stays untouched; the two must recognise the same
//! loops (the test below holds the same cases as `crash.rs`'s).

/// The write-back loops the supported build has (#135). Anything else is
/// not that build, or a pattern that is no loop, and nothing is patched.
pub const CLWB_LOOPS: usize = 8;

/// Whether `code` starts with a `clwb` loop, one cache line a pass:
///
/// ```text
/// top: clwb [base]
///      add  base, 0x40
///      cmp  base, limit
///      jbe  top
/// ```
fn is_clwb_loop(code: &[u8]) -> bool {
    // clwb [base]: 66 [40|41] 0f ae, ModRM mod 00, reg 6, no SIB or displacement.
    let (rex, at) = match code.get(1) {
        Some(0x40 | 0x41) => (code[1], 2),
        Some(_) => (0, 1),
        None => return false,
    };
    if code.first() != Some(&0x66) || code.get(at..at + 2) != Some(&[0x0f, 0xae]) {
        return false;
    }
    let Some(&modrm) = code.get(at + 2) else { return false };
    let rm = modrm & 7;
    if modrm >> 6 != 0 || (modrm >> 3) & 7 != 6 || rm == 4 || rm == 5 {
        return false;
    }
    let base = rm | (rex & 1) << 3;
    // add base, 0x40: REX.W(+B) 83 /0 ib.
    let add = at + 3;
    if code.get(add..add + 4) != Some(&[0x48 | base >> 3, 0x83, 0xc0 | (base & 7), 0x40]) {
        return false;
    }
    // cmp base, limit: REX.W 39 /r with base in r/m.
    let cmp = add + 4;
    let Some(&[cmp_rex @ 0x48..=0x4f, 0x39, cmp_modrm]) = code.get(cmp..cmp + 3) else {
        return false;
    };
    if cmp_modrm >> 6 != 3 || (cmp_modrm & 7 | (cmp_rex & 1) << 3) != base {
        return false;
    }
    let limit = (cmp_modrm >> 3) & 7 | (cmp_rex & 4) << 1;
    // jbe top.
    let jump = cmp + 3;
    let len = jump + 2;
    let (Some(&op), Some(&back)) = (code.get(jump), code.get(jump + 1)) else { return false };
    op == 0x76 && i64::from(back as i8) == -(len as i64) && limit != base
}

/// Where `code` has a loop: the offset of its `clwb` and that instruction's
/// length (66, an optional REX, 0f ae, ModRM).
pub fn clwb_loop_sites(code: &[u8]) -> Vec<(usize, usize)> {
    (0..code.len())
        .filter(|&at| code[at] == 0x66)
        .filter_map(|at| {
            is_clwb_loop(&code[at..(at + 32).min(code.len())]).then_some(())?;
            Some((at, if matches!(code[at + 1], 0x40 | 0x41) { 5 } else { 4 }))
        })
        .collect()
}

/// Turns the `clwb` of every loop in the executable sections of the image
/// mapped at `base` into a NOP of its length, and returns how many. The
/// loop's `add`/`cmp`/`jbe` stay, so its registers end as before. Before any
/// title thread runs.
///
/// Nothing is patched, and 0 returned, unless exactly [`CLWB_LOOPS`] loops
/// are found: the count found is logged otherwise.
#[cfg(windows)]
pub fn nop_clwb_loops(base: u64, info: &victory_pe::ImageInfo) -> usize {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut sites = Vec::new();
    for s in info.sections.iter().filter(|s| s.characteristics & super::MEM_EXECUTE != 0) {
        let start = base + u64::from(s.rva);
        let len = s.virtual_size.min(s.file_size) as usize;
        // SAFETY: the mapped section, readable code.
        let code = unsafe { std::slice::from_raw_parts(start as *const u8, len) };
        sites.extend(clwb_loop_sites(code).into_iter().map(|(at, n)| (start, at, n)));
    }
    if sites.len() != CLWB_LOOPS {
        tracing::warn!(
            found = sites.len(),
            expected = CLWB_LOOPS,
            "clwb write-back loops are not patched: this is not the build they were counted in"
        );
        return 0;
    }
    let mut patched = 0;
    for (start, at, n) in sites {
        const NOP4: [u8; 4] = [0x0f, 0x1f, 0x40, 0x00];
        const NOP5: [u8; 5] = [0x0f, 0x1f, 0x44, 0x00, 0x00];
        let site = (start + at as u64) as *mut u8;
        let mut old = 0;
        // SAFETY: the title's code, writable for the moment of the
        // write, before any title thread runs.
        unsafe {
            if VirtualProtect(site.cast(), n, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
                continue;
            }
            let nop: &[u8] = if n == 5 { &NOP5 } else { &NOP4 };
            std::ptr::copy_nonoverlapping(nop.as_ptr(), site, n);
            VirtualProtect(site.cast(), n, old, &mut old);
        }
        patched += 1;
    }
    // SAFETY: flushes the whole instruction cache of our process.
    unsafe { FlushInstructionCache(GetCurrentProcess(), std::ptr::null(), 0) };
    patched
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_loops_the_title_has_and_only_those() {
        // clwb [rcx]; add rcx, 0x40; cmp rcx, rax; jbe top
        let rcx = [0x66, 0x0f, 0xae, 0x31, 0x48, 0x83, 0xc1, 0x40, 0x48, 0x39, 0xc1, 0x76, 0xf3];
        assert!(is_clwb_loop(&rcx));
        // clwb [r9]; add r9, 0x40; cmp r9, rax; jbe top
        let r9 = [
            0x66, 0x41, 0x0f, 0xae, 0x31, 0x49, 0x83, 0xc1, 0x40, 0x49, 0x39, 0xc1, 0x76, 0xf2,
        ];
        assert!(is_clwb_loop(&r9));
        // base == limit is no loop; a lone clwb is none either.
        let same = [0x66, 0x0f, 0xae, 0x31, 0x48, 0x83, 0xc1, 0x40, 0x48, 0x39, 0xc9, 0x76, 0xf3];
        assert!(!is_clwb_loop(&same));
        assert!(!is_clwb_loop(&[0x66, 0x0f, 0xae, 0x31]));
        let mut code = vec![0x90, 0x90];
        code.extend_from_slice(&rcx);
        code.push(0xc3);
        assert_eq!(clwb_loop_sites(&code), vec![(2, 4)]);
        let mut code = vec![0x90];
        code.extend_from_slice(&r9);
        assert_eq!(clwb_loop_sites(&code), vec![(1, 5)]);
    }
}
