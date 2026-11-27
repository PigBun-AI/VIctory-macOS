//! `VICTORY_PRESENT_MAX=WIDTHxHEIGHT` (docs/mac/PATCHES.md, P1): caps the size of
//! the host window and its swap chain, and only that. The title's own render
//! targets, its upscaler and the picture it presents keep their sizes; the
//! host's presentation pass (`compose.rs`, `Composer::record_scaled`, or the
//! two-plane blend) draws the picture into the smaller back buffer, keeping its
//! aspect ratio. A picture that already fits is shown as it is; the cap never
//! enlarges. Unset or invalid (a warning, once): no cap.
//!
//! Ported in behaviour from VIctory-macOS `d093abb1` (`composite.rs`
//! `fit_output` / `output_size`), where the pass was a compute shader; this
//! line's presenter is a draw, so only the size rule and its tests are shared.

/// The size `width` x `height` shrinks to inside `limit`, keeping its aspect
/// ratio (rounded down, at least 1 on each side); the same size when it fits.
pub fn fit_output(width: u32, height: u32, limit: (u32, u32)) -> (u32, u32) {
    if width <= limit.0 && height <= limit.1 {
        return (width, height);
    }
    let (w, h) = (u64::from(width), u64::from(height));
    let (mw, mh) = (u64::from(limit.0), u64::from(limit.1));
    if mw * h <= mh * w {
        (limit.0, (h * mw / w).max(1) as u32)
    } else {
        ((w * mh / h).max(1) as u32, limit.1)
    }
}

/// `WIDTHxHEIGHT` with both positive; `None` for anything else.
pub fn parse(value: &str) -> Option<(u32, u32)> {
    let (w, h) = value.trim().split_once(['x', 'X'])?;
    let size = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    (size.0 > 0 && size.1 > 0).then_some(size)
}

/// The cap from `VICTORY_PRESENT_MAX`, read once.
pub fn limit() -> Option<(u32, u32)> {
    static LIMIT: std::sync::OnceLock<Option<(u32, u32)>> = std::sync::OnceLock::new();
    *LIMIT.get_or_init(|| {
        let value = std::env::var("VICTORY_PRESENT_MAX").ok().filter(|v| !v.trim().is_empty())?;
        let size = parse(&value);
        match size {
            Some((w, h)) => tracing::info!(
                width = w,
                height = h,
                "VICTORY_PRESENT_MAX: host window and swap chain capped; the title's render sizes are not changed"
            ),
            None => tracing::warn!(%value, "ignoring VICTORY_PRESENT_MAX: expected positive WIDTHxHEIGHT"),
        }
        size
    })
}

/// The size to present a `width` x `height` picture at.
pub fn output_size(width: u32, height: u32) -> (u32, u32) {
    limit().map_or((width, height), |limit| fit_output(width, height, limit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cap_keeps_the_aspect_ratio_and_never_enlarges() {
        let cap = (1280, 720);
        assert_eq!(fit_output(1920, 1080, cap), cap);
        assert_eq!(fit_output(3840, 2160, cap), cap);
        assert_eq!(fit_output(2560, 1440, cap), cap);
        assert_eq!(fit_output(1024, 768, cap), (960, 720));
        assert_eq!(fit_output(2560, 1080, cap), (1280, 540));
        assert_eq!(fit_output(640, 360, cap), (640, 360));
        assert_eq!(fit_output(1280, 720, cap), cap);
        assert_eq!(fit_output(1, 1440, cap), (1, 720));
    }

    #[test]
    fn the_value_is_width_x_height() {
        assert_eq!(parse("1280x720"), Some((1280, 720)));
        assert_eq!(parse(" 1920X1080 "), Some((1920, 1080)));
        assert_eq!(parse("0x720"), None);
        assert_eq!(parse("1280"), None);
        assert_eq!(parse("axb"), None);
        assert_eq!(parse("-1x5"), None);
    }
}
