//! The title's hang watchdog turned off (`VICTORY_MAC_NO_WATCHDOG=1`, off by
//! default; patch L5 in docs/mac/PATCHES.md). **Not fidelity**: it changes
//! what the title does when it is slow.
//!
//! What the title does (game_scarlett_master.exe 1.0.2.1, image offsets,
//! read from the bytes with a disassembler): a thread loops once a second
//! and crashes the title on purpose, with `mov dword ptr [0], 0` at
//! +0x2215c35 (then `int3`), when a registered thread's heartbeat is older
//! than its limit (the age compared at +0x2215bd8), or when the main thread
//! has not finished a frame for 30 checks (`ebp` counts down from 0x1e).
//! The crash path is +0x2215c0c: it calls a reporting function (+0x4e4ba80),
//! then falls to the store. On a Mac a world frame takes seconds, and a
//! recording or capture run makes it slower still, so the title kills itself
//! in the world.
//!
//! Ported from VIctory-macOS (origin/main, commit a38f26d9, "Switches for
//! slow storage and low-memory Macs", `VICTORY_NO_HANG_WATCHDOG`, three
//! sites): each jump to the crash path is made a two-byte NOP, `66 90`, so
//! the loop goes on as if the check had passed:
//!
//! | site | before | after | what the jump does |
//! |---|---|---|---|
//! | +0x2215bdc | `73 2e` (`jae`) | `66 90` | after `vucomiss xmm8, xmm7` (heartbeat age against its limit): to the crash path +0x2215c0c |
//! | +0x2215c70 | `74 04` (`je`) | `66 90` | after `dec ebp` (the 30 checks without a finished frame, first form): to +0x2215c76, which goes on to the crash path +0x2215c16 |
//! | +0x2215d84 | `74 04` (`je`) | `66 90` | the same count-down in the loop's other form: to +0x2215d8a, the reporting call and the crash |
//!
//! The three are compared byte for byte, all of them before any is written,
//! and the build is the one the offsets were made in (hash `b99f876b…`, same
//! as `apps/victory/src/mac.rs`): any other bytes and nothing is patched, and
//! the reason is printed.

/// The build the sites belong to (Series 1.0.2.1), BLAKE3 of the exe.
pub const BUILD_1_0_2_1: &str = "b99f876b74d4a77f03af5f878223b27bd647b5c35a79d5decd9f716504239e93";

/// What to log when it is on.
pub const NOTE: &str = "non-fidelity: title hang watchdog disabled";

/// A place in the title's code and the two bytes at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Site {
    pub rva: u32,
    pub original: [u8; 2],
    pub replacement: [u8; 2],
}

/// `66 90`: a two-byte NOP (`xchg ax, ax`).
const NOP2: [u8; 2] = [0x66, 0x90];

/// The three jumps to the crash path (the title's own crash store is at
/// +0x2215c35).
pub const SITES: [Site; 3] = [
    Site { rva: 0x221_5bdc, original: [0x73, 0x2e], replacement: NOP2 },
    Site { rva: 0x221_5c70, original: [0x74, 0x04], replacement: NOP2 },
    Site { rva: 0x221_5d84, original: [0x74, 0x04], replacement: NOP2 },
];

/// `VICTORY_MAC_NO_WATCHDOG` is set to anything but `0` or empty.
pub fn wanted() -> bool {
    wanted_from(std::env::var("VICTORY_MAC_NO_WATCHDOG").ok().as_deref())
}

fn wanted_from(value: Option<&str>) -> bool {
    value.is_some_and(|v| !matches!(v.trim(), "" | "0"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

/// Compares every site with `image`, the mapped image from its base on
/// (`image.len()` its size): `Err` says which site differs and what is there,
/// and nothing may be written then.
pub fn check(image: &[u8]) -> Result<(), String> {
    for s in &SITES {
        let at = s.rva as usize;
        let Some(found) = image.get(at..at + s.original.len()) else {
            return Err(format!("title+{:#x} is outside the image ({} bytes)", s.rva, image.len()));
        };
        if found != s.original {
            return Err(format!(
                "title+{:#x} holds {} where {} was expected (another build of the title?)",
                s.rva,
                hex(found),
                hex(&s.original)
            ));
        }
    }
    Ok(())
}

/// Writes the replacements into `image` after [`check`] passed: all three or
/// none. (The loader's version of this writes the mapped image; this one is
/// what the tests run.)
pub fn patch_bytes(image: &mut [u8]) -> Result<usize, String> {
    check(image)?;
    for s in &SITES {
        image[s.rva as usize..s.rva as usize + 2].copy_from_slice(&s.replacement);
    }
    Ok(SITES.len())
}

/// Turns the watchdog off in the image mapped at `base` (`size` bytes), or
/// says why not: the bytes are compared first, all three sites, and nothing
/// is written unless every one matches. Before any title thread runs.
///
/// # Safety
/// The image stays mapped for `size` bytes and no thread runs its code yet.
#[cfg(windows)]
pub unsafe fn apply(base: usize, size: usize) -> Result<usize, String> {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: the caller's promise; read only, the whole image.
    check(unsafe { std::slice::from_raw_parts(base as *const u8, size) })?;
    let mut done = 0;
    for s in &SITES {
        let at = (base + s.rva as usize) as *mut u8;
        let mut old = 0;
        // SAFETY: inside the image (checked); writable for the moment of the
        // write and put back; nothing runs it yet.
        unsafe {
            if VirtualProtect(at.cast(), 2, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
                return Err(format!("Windows would not make title+{:#x} writable ({done} of 3 written)", s.rva));
            }
            std::ptr::copy_nonoverlapping(s.replacement.as_ptr(), at, 2);
            VirtualProtect(at.cast(), 2, old, &mut old);
            FlushInstructionCache(GetCurrentProcess(), at.cast(), 2);
        }
        done += 1;
    }
    Ok(done)
}

/// Says what became of it, on standard output and in the log: [`NOTE`] when
/// the watchdog is off, the reason when it stays on.
pub fn report(done: &Result<usize, String>) {
    match done {
        Ok(sites) => {
            println!("{NOTE}");
            tracing::warn!(sites, "{NOTE}");
        }
        Err(why) => {
            println!("title hang watchdog left on: {why}");
            tracing::warn!(why, "title hang watchdog left on (VICTORY_MAC_NO_WATCHDOG)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up image: filler, with the title's bytes only at the three
    /// sites (nothing of the title is stored here beyond the two-byte
    /// originals the site table already holds).
    fn image() -> Vec<u8> {
        let mut image = vec![0xcc; 0x221_5e00];
        for s in &SITES {
            image[s.rva as usize..s.rva as usize + 2].copy_from_slice(&s.original);
        }
        image
    }

    #[test]
    fn the_switch_is_off_unless_asked() {
        assert!(!wanted_from(None));
        assert!(!wanted_from(Some("")));
        assert!(!wanted_from(Some("0")));
        assert!(wanted_from(Some("1")));
        assert!(wanted_from(Some("yes")));
    }

    #[test]
    fn sites_are_the_three_the_old_base_patched() {
        let sites: Vec<(u32, [u8; 2])> = SITES.iter().map(|s| (s.rva, s.original)).collect();
        assert_eq!(
            sites,
            vec![(0x2215bdc, [0x73, 0x2e]), (0x2215c70, [0x74, 0x04]), (0x2215d84, [0x74, 0x04])]
        );
        // All three sit around the crash store at +0x2215c35 and are two bytes.
        assert!(SITES.iter().all(|s| (0x2215b00..0x2215e00).contains(&s.rva) && s.replacement == [0x66, 0x90]));
    }

    #[test]
    fn matching_bytes_are_patched_all_three() {
        let mut image = image();
        assert_eq!(check(&image), Ok(()));
        assert_eq!(patch_bytes(&mut image), Ok(3));
        for s in &SITES {
            assert_eq!(&image[s.rva as usize..s.rva as usize + 2], &[0x66, 0x90]);
        }
        // Nothing else moved.
        assert_eq!(image.iter().filter(|&&b| b == 0x66).count(), 3);
        // A second time the bytes are not the originals any more: refused.
        assert!(patch_bytes(&mut image).is_err());
    }

    #[test]
    fn one_wrong_byte_patches_nothing_and_says_where() {
        for (i, s) in SITES.iter().enumerate() {
            for byte in 0..2 {
                let mut image = image();
                image[s.rva as usize + byte] ^= 0x01;
                let before = image.clone();
                let why = patch_bytes(&mut image).unwrap_err();
                assert!(why.contains(&format!("title+{:#x}", s.rva)), "site {i}: {why}");
                assert!(why.contains("another build"), "{why}");
                assert_eq!(image, before, "site {i} byte {byte}: written despite a mismatch");
            }
        }
    }

    #[test]
    fn a_short_image_is_refused() {
        let mut image = image();
        image.truncate(0x221_5d85);
        let why = check(&image).unwrap_err();
        assert!(why.contains("outside the image"), "{why}");
        assert!(patch_bytes(&mut image).is_err());
        assert_eq!(check(&[]).is_err(), true);
    }
}
