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
//! The function runs as the title sets its scale, so the minimum's immediate
//! can be written while the title runs ([`set_live`]): the next call stores
//! it. That is how one run measures two scales (`VICTORY_AB`).

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

/// Writes `scale` as the minimum in the mapped image at `base`, while the
/// title runs, if the minimum there is `was` (what the image was given).
/// Returns whether it was written.
///
/// # Safety
/// `base` is the mapped retail image the scale places were checked in.
#[cfg(windows)]
pub unsafe fn set_live(base: usize, was: f32, scale: f32) -> bool {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let at = base + MINIMUM as usize;
    // SAFETY: four bytes of the mapped image (the caller's contract).
    let there = unsafe { std::ptr::read_unaligned(at as *const [u8; 4]) };
    if there != was.to_le_bytes() {
        return false;
    }
    let mut old = 0;
    // SAFETY: the immediate's bytes, made writable for this and put back.
    // A torn read by the title mixes the two scales' bytes for one call:
    // their high halves (sign, exponent) decide it, and both are scales.
    unsafe {
        if VirtualProtect(at as *const _, 4, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            return false;
        }
        std::ptr::write_unaligned(at as *mut [u8; 4], scale.to_le_bytes());
        VirtualProtect(at as *const _, 4, old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), at as *const _, 4);
    }
    true
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

    /// The live write changes only an immediate that holds what it was
    /// given, as the image's code would be; another value is left alone.
    #[cfg(windows)]
    #[test]
    fn the_live_minimum_is_written_only_over_the_expected_one() {
        let mut image = vec![0u8; MINIMUM as usize + 16];
        let base = image.as_mut_ptr() as usize;
        image[MINIMUM as usize..][..4].copy_from_slice(&1.0f32.to_le_bytes());
        // SAFETY: `image` is `base`'s, and holds the minimum's place.
        assert!(unsafe { set_live(base, 1.0, 0.75) });
        assert_eq!(image[MINIMUM as usize..][..4], 0.75f32.to_le_bytes());
        // SAFETY: as above.
        assert!(!unsafe { set_live(base, 1.0, 0.5) }, "the minimum is no longer 1.0");
        assert_eq!(image[MINIMUM as usize..][..4], 0.75f32.to_le_bytes());
    }
}
