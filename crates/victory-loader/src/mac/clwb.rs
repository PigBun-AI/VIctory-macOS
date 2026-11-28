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

/// `clwb` of 4 bytes (`66 0f ae /6`) and of 5 (`66 REX 0f ae /6`) as NOPs of the same length.
const NOP4: [u8; 4] = [0x0f, 0x1f, 0x40, 0x00];
const NOP5: [u8; 5] = [0x0f, 0x1f, 0x44, 0x00, 0x00];

/// The NOP that replaces a `clwb` of `len` bytes.
fn nop_for(len: usize) -> &'static [u8] {
    if len == 5 { &NOP5 } else { &NOP4 }
}

/// What a pass over the title's code found and did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// Write-back loops the scan found.
    pub found: usize,
    /// Loops turned into NOPs: all of them if exactly [`CLWB_LOOPS`] were found, else none.
    pub patched: usize,
}

impl Outcome {
    /// The log line, with the count: `clwb: 8 loops replaced`, or why not.
    pub fn line(&self) -> String {
        if self.found == CLWB_LOOPS && self.patched == CLWB_LOOPS {
            format!("clwb: {} loops replaced", self.patched)
        } else {
            format!(
                "clwb: {} loops replaced (found {}, expected {CLWB_LOOPS}: not the build they were counted in, nothing patched)",
                self.patched, self.found
            )
        }
    }
}

/// The log line when the patch is left off.
pub const LEFT_AS_THEY_ARE: &str = "clwb: 0 loops replaced (left as they are: not under Rosetta, or VICTORY_NOP_CLWB=0)";

/// Whether the loops are patched: under Rosetta (`clwb` faults there), `VICTORY_NOP_CLWB=0`
/// turns it off and `1` turns it on elsewhere.
pub fn wanted(var: Option<&str>, under_rosetta: bool) -> bool {
    match var {
        Some("0") => false,
        Some("1") => true,
        _ => under_rosetta,
    }
}

/// The sites of every loop in `sections` (the executable sections of the image, as
/// `(section, offset, clwb length)`), and whether that is the count of the supported build.
fn sites_in(sections: &[&[u8]]) -> (Vec<(usize, usize, usize)>, bool) {
    let sites: Vec<_> = sections
        .iter()
        .enumerate()
        .flat_map(|(i, code)| clwb_loop_sites(code).into_iter().map(move |(at, n)| (i, at, n)))
        .collect();
    let ok = sites.len() == CLWB_LOOPS;
    (sites, ok)
}

/// The loops of `sections` turned into NOPs of their length, in plain memory: nothing is
/// patched unless exactly [`CLWB_LOOPS`] loops are found. (The loader's own pass,
/// [`nop_clwb_loops`], does the same to the mapped image and needs `VirtualProtect`; this one
/// is what tests and any buffer copy of the exe can use.)
pub fn nop_in_buffers(sections: &mut [&mut [u8]]) -> Outcome {
    let (sites, ok) = {
        let views: Vec<&[u8]> = sections.iter().map(|s| &**s).collect();
        sites_in(&views)
    };
    if !ok {
        return Outcome { found: sites.len(), patched: 0 };
    }
    for &(i, at, n) in &sites {
        sections[i][at..at + n].copy_from_slice(nop_for(n));
    }
    Outcome { found: sites.len(), patched: sites.len() }
}

/// Turns the `clwb` of every loop in the executable sections of the image
/// mapped at `base` into a NOP of its length. The
/// loop's `add`/`cmp`/`jbe` stay, so its registers end as before. Before any
/// title thread runs.
///
/// Nothing is patched, and `patched` is 0, unless exactly [`CLWB_LOOPS`] loops
/// are found: the count found is logged otherwise (and is in [`Outcome::line`]).
#[cfg(windows)]
pub fn nop_clwb_loops(base: u64, info: &victory_pe::ImageInfo) -> Outcome {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let execs: Vec<_> = info.sections.iter().filter(|s| s.characteristics & super::MEM_EXECUTE != 0).collect();
    let starts: Vec<u64> = execs.iter().map(|s| base + u64::from(s.rva)).collect();
    // SAFETY: the mapped sections, readable code.
    let views: Vec<&[u8]> = execs
        .iter()
        .zip(&starts)
        .map(|(s, &start)| unsafe {
            std::slice::from_raw_parts(start as *const u8, s.virtual_size.min(s.file_size) as usize)
        })
        .collect();
    let (sites, ok) = sites_in(&views);
    if !ok {
        tracing::warn!(
            found = sites.len(),
            expected = CLWB_LOOPS,
            "clwb write-back loops are not patched: this is not the build they were counted in"
        );
        return Outcome { found: sites.len(), patched: 0 };
    }
    let mut patched = 0;
    for &(i, at, n) in &sites {
        let site = (starts[i] + at as u64) as *mut u8;
        let mut old = 0;
        // SAFETY: the title's code, writable for the moment of the
        // write, before any title thread runs.
        unsafe {
            if VirtualProtect(site.cast(), n, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
                continue;
            }
            let nop = nop_for(n);
            std::ptr::copy_nonoverlapping(nop.as_ptr(), site, n);
            VirtualProtect(site.cast(), n, old, &mut old);
        }
        patched += 1;
    }
    // SAFETY: flushes the whole instruction cache of our process.
    unsafe { FlushInstructionCache(GetCurrentProcess(), std::ptr::null(), 0) };
    Outcome { found: sites.len(), patched }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real build (Series 1.0.2.1): `CLWB_LOOPS` loops, and every `clwb` (66 [rex] 0f ae /6) of its
    /// executable sections is in one of them. Reads the cached title image (`VICTORY_TITLE_IMAGE`, or
    /// data/shared/cache/images/victory-image-b99f876b74d4a77f.dll under `~/Documents/GTA6Project`); the file is
    /// game data, so without it the test says so and passes.
    #[test]
    fn the_real_build_has_the_loops_and_no_other_clwb() {
        let path = std::env::var_os("VICTORY_TITLE_IMAGE").map(std::path::PathBuf::from).unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join("Documents/GTA6Project/data/shared/cache/images/victory-image-b99f876b74d4a77f.dll")
        });
        let Ok(image) = std::fs::read(&path) else {
            eprintln!("no title image at {}: not checked", path.display());
            return;
        };
        let u32_at = |at: usize| u32::from_le_bytes(image[at..at + 4].try_into().unwrap()) as usize;
        let pe = u32_at(0x3c);
        let sections = usize::from(u16::from_le_bytes([image[pe + 6], image[pe + 7]]));
        let headers = pe + 24 + usize::from(u16::from_le_bytes([image[pe + 20], image[pe + 21]]));
        let (mut loops, mut clwbs, mut text) = (0, 0, 0);
        for n in 0..sections {
            let h = headers + 40 * n;
            let (virtual_size, raw_size, raw, flags) = (u32_at(h + 8), u32_at(h + 16), u32_at(h + 20), u32_at(h + 36));
            if flags & 0x2000_0000 == 0 {
                continue;
            }
            let code = &image[raw..raw + virtual_size.min(raw_size)];
            text += code.len();
            loops += clwb_loop_sites(code).len();
            // Every clwb: 66, an optional REX (40/41), 0f ae, ModRM with mod 00 or 01/10 and reg 6.
            clwbs += (0..code.len().saturating_sub(5))
                .filter(|&at| {
                    let o = if matches!(code[at + 1], 0x40..=0x4f) { at + 2 } else { at + 1 };
                    code[at] == 0x66 && code[o] == 0x0f && code[o + 1] == 0xae && (code[o + 2] >> 3) & 7 == 6 && code[o + 2] >> 6 != 3
                })
                .count();
        }
        assert!(text > 90_000_000, "{text} bytes of code");
        assert_eq!(loops, CLWB_LOOPS);
        // (a byte pattern can also match inside other instructions: the count is of the pattern, so it
        // can only be more than the real clwb count; equal means none is outside a loop)
        assert_eq!(clwbs, CLWB_LOOPS, "clwb instructions outside the loops");
    }

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

    /// A section with `n` loops, alternating the 4-byte (rcx) and 5-byte (r9) forms, with a
    /// `ret` and a `nop` between them.
    fn section_with(n: usize) -> Vec<u8> {
        let rcx = [0x66, 0x0f, 0xae, 0x31, 0x48, 0x83, 0xc1, 0x40, 0x48, 0x39, 0xc1, 0x76, 0xf3];
        let r9 = [0x66, 0x41, 0x0f, 0xae, 0x31, 0x49, 0x83, 0xc1, 0x40, 0x49, 0x39, 0xc1, 0x76, 0xf2];
        let mut code = vec![0x90];
        for i in 0..n {
            code.extend_from_slice(if i % 2 == 0 { &rcx[..] } else { &r9[..] });
            code.extend_from_slice(&[0xc3, 0x90]);
        }
        code
    }

    #[test]
    fn eight_loops_are_replaced_and_logged_with_the_count() {
        // 8 loops over two sections (5 + 3), as the title has them in several functions.
        let (mut a, mut b) = (section_with(5), section_with(3));
        let before = (a.clone(), b.clone());
        let out = nop_in_buffers(&mut [&mut a, &mut b]);
        assert_eq!(out, Outcome { found: 8, patched: 8 });
        assert_eq!(out.line(), "clwb: 8 loops replaced");
        // No clwb is left, each was swapped for a NOP of its own length, and the rest is as it was.
        assert!(clwb_loop_sites(&a).is_empty() && clwb_loop_sites(&b).is_empty());
        assert_eq!(&a[1..5], &NOP4[..]);
        assert_eq!(&a[16..21], &NOP5[..]);
        assert_eq!(a.len(), before.0.len());
        let changed = |x: &[u8], y: &[u8]| x.iter().zip(y).filter(|(p, q)| p != q).count();
        assert!(changed(&a, &before.0) + changed(&b, &before.1) <= 8 * 5);
        assert_eq!(&a[5..13 + 1], &before.0[5..13 + 1], "add/cmp/jbe stay");
    }

    #[test]
    fn any_other_count_patches_nothing_and_says_what_it_found() {
        for n in [0, 7, 9] {
            let mut a = section_with(n);
            let before = a.clone();
            let out = nop_in_buffers(&mut [&mut a]);
            assert_eq!(out, Outcome { found: n, patched: 0 });
            assert_eq!(a, before, "{n} loops: untouched");
            let line = out.line();
            assert!(line.starts_with("clwb: 0 loops replaced (found "), "{line}");
            assert!(line.contains(&format!("found {n}, expected 8")), "{line}");
        }
    }

    #[test]
    fn the_patch_is_wanted_under_rosetta_and_the_variable_decides() {
        assert!(wanted(None, true) && !wanted(None, false));
        assert!(!wanted(Some("0"), true) && wanted(Some("1"), false));
        assert!(wanted(Some("x"), true) && !wanted(Some("x"), false));
        assert!(LEFT_AS_THEY_ARE.starts_with("clwb: 0 loops replaced (left as they are"));
    }
}
