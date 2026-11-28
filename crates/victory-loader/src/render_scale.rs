//! The scene's render scale of the Grand Theft Auto VI 1.0.2.1 retail image
//! (`VICTORY_RENDER_SCALE`), pinned.
//!
//! The title's own temporal upscaler sets its scale in one small function
//! (at `+0x28f1e70`): it stores the minimum, an immediate 2/3 on Series X,
//! at `[rcx+0x90]`, asks its dynamic-resolution controller for a scale, and
//! stores `max(that, minimum)` at `[rsi+0x94]` (`vmaxss` at `+0x28f1e87`).
//! Writing only the minimum (as `VICTORY_RENDER_1080P` did) leaves the scale
//! wherever the controller puts it above; turning the `vmaxss` into a
//! `vmovss` makes the scale the minimum, whatever the controller says.
//!
//! The two places and their bytes are VIctory-NVIDIA's
//! (`afd84de:crates/victory-loader/src/hooks.rs:107-125` `render_scale_patches`,
//! `:154` `parse_scale`; GPL-3.0-or-later, as this).
//!
//! The title sizes its scene targets from the scale when it makes them: a
//! minimum written while the title ran (tried 2026-11-28) left the targets
//! at 2560x1440, so the scale is fixed for a run, written into the image.

/// The minimum's immediate (`mov dword [rcx+0x90], imm32`), an RVA.
pub const MINIMUM: u32 = 0x028f_1e7e;
/// The stock minimum: 2/3.
pub const STOCK_MINIMUM: [u8; 4] = [0xAB, 0xAA, 0x2A, 0x3F];
/// `vmaxss xmm0, xmm0, [rsi+0x90]`, an RVA.
pub const MAXIMUM: u32 = 0x028f_1e87;
/// Its stock bytes.
pub const STOCK_MAXIMUM: [u8; 8] = [0xC5, 0xFA, 0x5F, 0x86, 0x90, 0, 0, 0];
/// `vmovss xmm0, [rsi+0x90]`: the scale is the minimum.
pub const PINNED_MAXIMUM: [u8; 8] = [0xC5, 0xFA, 0x10, 0x86, 0x90, 0, 0, 0];

/// A scale from `VICTORY_RENDER_SCALE`'s text: 0.25 to 1.0.
pub fn parse_scale(text: &str) -> Option<f32> {
    let v: f32 = text.trim().parse().ok()?;
    (0.25..=1.0).contains(&v).then_some(v)
}

/// The scene's size at `scale` of a `width` x `height` output, as the title
/// rounds it (down).
pub fn scene_size(width: u32, height: u32, scale: f32) -> (u32, u32) {
    ((width as f32 * scale) as u32, (height as f32 * scale) as u32)
}

/// Run 2026-11-28 11:15-11:23 (72f115b59, 1440p image with its minimum 1.0, not pinned, 8 fps,
/// 122-127 ms frames): 45 lines of 10 s, every sample 1.0 (min = median = max = 1.0000), so the
/// controller does not take the scene below native there: the pin stays off by default.
///
/// The scale the title computes (`max(controller, minimum)` at [`MAXIMUM`]), noted on every
/// call by a detour (`VICTORY_SCALE_WATCH`, default on, =0 off): the 8-byte instruction there
/// jumps to a stub near the image that runs it, stores xmm0 into its own slot and jumps back.
/// No register but xmm0 (the instruction's own result) is touched. `None` if the bytes are not
/// the stock or pinned instruction, or no page within 2 GB is free. Before the title runs.
///
/// # Safety
/// `base` is the mapped retail image, not running yet.
#[cfg(windows)]
pub unsafe fn watch(base: usize) -> Option<&'static std::sync::atomic::AtomicU32> {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualProtect};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let site = base + MAXIMUM as usize;
    // SAFETY: the mapped image (caller).
    let bytes: [u8; 8] = unsafe { std::ptr::read_unaligned(site as *const [u8; 8]) };
    if bytes != STOCK_MAXIMUM && bytes != PINNED_MAXIMUM {
        return None;
    }
    let mut stub = 0usize;
    let mut at = (site & !0xFFFF).saturating_sub(0x10000);
    while site - at < 0x7000_0000 {
        // SAFETY: asking for fresh memory at a hint.
        let got = unsafe { VirtualAlloc(at as *const _, 0x1000, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) } as usize;
        if got != 0 {
            stub = got;
            break;
        }
        at -= 0x10000;
    }
    if stub == 0 {
        return None;
    }
    let code = stub_code(stub, site, bytes);
    // SAFETY: our fresh page.
    unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), stub as *mut u8, code.len()) };
    let jump = site_jump(site, stub);
    let mut old = 0u32;
    // SAFETY: 8 bytes of the image's code, made writable for the write and put back.
    unsafe {
        if VirtualProtect(site as *const _, 8, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            return None;
        }
        std::ptr::copy_nonoverlapping(jump.as_ptr(), site as *mut u8, 8);
        VirtualProtect(site as *const _, 8, old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), site as *const _, 8);
        Some(&*((stub + SLOT) as *const std::sync::atomic::AtomicU32))
    }
}

/// Where the stub keeps the last scale (f32 bits).
const SLOT: usize = 0x40;

/// The stub at `stub`: the site's instruction `bytes` (position independent: `[rsi+0x90]`),
/// `vmovss [rip+d], xmm0` into [`SLOT`], `jmp` back behind the site.
fn stub_code(stub: usize, site: usize, bytes: [u8; 8]) -> [u8; 21] {
    let mut c = [0u8; 21];
    c[..8].copy_from_slice(&bytes);
    let slot = (stub + SLOT) as i64 - (stub + 16) as i64;
    c[8..12].copy_from_slice(&[0xC5, 0xFA, 0x11, 0x05]);
    c[12..16].copy_from_slice(&(slot as i32).to_le_bytes());
    c[16] = 0xE9;
    let back = (site + 8) as i64 - (stub + 21) as i64;
    c[17..21].copy_from_slice(&(back as i32).to_le_bytes());
    c
}

/// The site's new bytes: `jmp stub`, three `nop`s.
fn site_jump(site: usize, stub: usize) -> [u8; 8] {
    let rel = stub as i64 - (site + 5) as i64;
    let r = (rel as i32).to_le_bytes();
    [0xE9, r[0], r[1], r[2], r[3], 0x90, 0x90, 0x90]
}

/// min, median, max of `values` (scales), `None` for none.
pub fn spread(values: &mut [f32]) -> Option<(f32, f32, f32)> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f32::total_cmp);
    Some((values[0], values[values.len() / 2], values[values.len() - 1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_are_read_within_bounds() {
        assert_eq!(parse_scale("0.75"), Some(0.75));
        assert_eq!(parse_scale(" 1 "), Some(1.0));
        assert_eq!(parse_scale("0.25"), Some(0.25));
        assert_eq!(parse_scale("0.2"), None);
        assert_eq!(parse_scale("1.5"), None);
        assert_eq!(parse_scale("half"), None);
        assert_eq!(scene_size(2560, 1440, 0.75), (1920, 1080));
        assert_eq!(scene_size(2560, 1440, 2.0 / 3.0), (1706, 960));
        assert_eq!(scene_size(2560, 1440, 0.5), (1280, 720));
        assert_eq!(STOCK_MINIMUM, (2.0f32 / 3.0).to_le_bytes());
    }

    /// The detour's code: run on a copy of the stock instruction with rsi set, the stub
    /// leaves max(xmm0, [rsi+0x90]) in xmm0 and in its slot, and comes back behind the site.
    #[cfg(all(windows, target_arch = "x86_64"))]
    #[test]
    fn the_scale_watch_notes_what_the_instruction_made() {
        let (site, stub) = (0x1_4000_0000usize, 0x1_3fff_0000usize);
        let j = site_jump(site, stub);
        assert_eq!(j[0], 0xE9);
        assert_eq!((site + 5).wrapping_add(i32::from_le_bytes(j[1..5].try_into().unwrap()) as isize as usize), stub);
        let c = stub_code(stub, site, STOCK_MAXIMUM);
        assert_eq!(&c[..8], &STOCK_MAXIMUM);
        assert_eq!((stub + 16).wrapping_add(i32::from_le_bytes(c[12..16].try_into().unwrap()) as isize as usize), stub + SLOT);
        assert_eq!((stub + 21).wrapping_add(i32::from_le_bytes(c[17..21].try_into().unwrap()) as isize as usize), site + 8);
        // Run it: a site of the stock bytes then `ret`, the detour in, called with rsi at data
        // whose +0x90 is the minimum. Adversarial: without the detour the slot stays 0.
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc};
        unsafe {
            let page = VirtualAlloc(0x1_5000_0000usize as *const _, 0x10000, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as usize;
            assert_ne!(page, 0);
            let base = page - MAXIMUM as usize + 0x100;
            std::ptr::copy_nonoverlapping(STOCK_MAXIMUM.as_ptr(), (page + 0x100) as *mut u8, 8);
            *((page + 0x108) as *mut u8) = 0xC3;
            let run = |x: f32| -> f32 {
                let mut data = [0f32; 64];
                data[0x90 / 4] = 1.0;
                let out: f32;
                std::arch::asm!("call {f}", f = in(reg) page + 0x100, in("rsi") data.as_ptr(), inout("xmm0") x => out, clobber_abi("C"));
                out
            };
            assert_eq!(run(0.7), 1.0);
            let slot = watch(base).expect("detour");
            assert_eq!(slot.load(std::sync::atomic::Ordering::Relaxed), 0);
            assert_eq!(run(0.7), 1.0);
            assert_eq!(f32::from_bits(slot.load(std::sync::atomic::Ordering::Relaxed)), 1.0);
            assert_eq!(run(1.25), 1.25);
            assert_eq!(f32::from_bits(slot.load(std::sync::atomic::Ordering::Relaxed)), 1.25);
        }
        let mut v = [0.8f32, 1.0, 0.7];
        assert_eq!(spread(&mut v), Some((0.7, 0.8, 1.0)));
        assert_eq!(spread(&mut []), None);
    }
}
