//! `VICTORY_MAC_LAYOUT_STATS=1` (docs/mac/PATCHES.md, patch G11): which texture layouts the title creates, so
//! that the translation map's rows C-V-003 (detiling) and C-V-005 (the tile modes) rest on what the title
//! uses and not on what the detiler happens to cover. Off unless set; with it off the hook is one atomic load
//! and nothing is recorded. It changes nothing the title or the host sees.
//!
//! Every texture a resource creation call hands out (committed, placed and at-address alike, through
//! `hand_out_resource`) is counted by: dimension, the description's layout field (`0x100 | swizzle mode`, 1 row
//! major, 0 the library's pick), the render-target and depth-stencil flags, bytes per element and block size
//! of its format, and whether it has more than one mip level, an array or volume depth above one, or more than
//! one sample. A line per distinct key is logged when the number of textures reaches a power of two from 256
//! on (and every 65536 after 65536), `layout stats: ...`, and `layout stats: N textures` closes the round;
//! the swizzle modes the detiler cannot address (`xor`: the `_T` and `_X` modes) are marked, so a round says
//! how many textures of the run are out of its reach.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{LazyLock, Mutex};

const WANTED: &str = "VICTORY_MAC_LAYOUT_STATS";

static ENABLED: LazyLock<bool> = LazyLock::new(|| std::env::var(WANTED).ok().is_some_and(|v| !matches!(v.trim(), "" | "0")));
static TOTAL: AtomicU64 = AtomicU64::new(0);
static LOGGING: AtomicBool = AtomicBool::new(false);

/// What a texture is counted by.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Key {
    dimension: u32,
    layout: u32,
    /// Render target (1) and depth stencil (2) flags.
    target: u32,
    element: (u64, u64),
    mips: bool,
    deep: bool,
    samples: bool,
}

static COUNTS: Mutex<BTreeMap<Key, u64>> = Mutex::new(BTreeMap::new());

/// A texture the title made: its description's fields.
#[allow(clippy::too_many_arguments)]
pub fn texture(dimension: u32, layout: u32, flags: u32, format: u32, mips: u16, depth: u16, samples: u32) {
    if !*ENABLED {
        return;
    }
    let key = Key {
        dimension,
        layout,
        target: flags & 3,
        element: crate::resource::element(format).unwrap_or((0, 0)),
        mips: mips > 1,
        deep: depth > 1,
        samples: samples > 1,
    };
    *COUNTS.lock().unwrap_or_else(|e| e.into_inner()).entry(key).or_default() += 1;
    let n = TOTAL.fetch_add(1, Relaxed) + 1;
    if (n >= 256 && n.is_power_of_two() || n >= 65536 && n % 65536 == 0) && !LOGGING.swap(true, Relaxed) {
        report(n);
        LOGGING.store(false, Relaxed);
    }
}

static UAV3D: [AtomicU64; 3] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];

/// A 3D unordered access view the title made (`words`: the description): counted as all, with a first depth slice
/// other than 0, and with a size that is not "the rest" (`u32::MAX`). D3DMetal refuses partial views of a 3D texture
/// (matrix row C-V-012, finding F1); this says whether the title makes any. Logged at the powers of two of the count.
pub fn uav_3d(words: &[u32; 10]) {
    if !*ENABLED || words[1] != 8 {
        return;
    }
    // words: format, dimension, MipSlice, FirstWSlice, WSize
    let n = UAV3D[0].fetch_add(1, Relaxed) + 1;
    if words[3] != 0 {
        UAV3D[1].fetch_add(1, Relaxed);
    }
    if words[4] != u32::MAX {
        UAV3D[2].fetch_add(1, Relaxed);
    }
    if n.is_power_of_two() {
        tracing::info!(
            "layout stats: {n} 3D unordered access views, {} from a depth slice other than 0, {} with a size other than the rest",
            UAV3D[1].load(Relaxed),
            UAV3D[2].load(Relaxed)
        );
    }
}

/// The swizzle mode of a layout field, if it names one.
fn mode(layout: u32) -> Option<u32> {
    (layout & !0xff == 0x100).then_some(layout & 0xff)
}

/// One line per key, then the total.
fn report(n: u64) {
    let counts = COUNTS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut out_of_reach = 0u64;
    for (k, c) in &counts {
        let xor = mode(k.layout).is_some_and(|m| victory_formats::texture::Swizzle(m).xor());
        let unnamed = k.layout > 1 && mode(k.layout).is_none();
        if xor || unnamed {
            out_of_reach += c;
        }
        tracing::info!(
            "layout stats: dim={} layout={:#x} swizzle={} target={} element={}B/{} mips={} deep={} msaa={} xor={} count={}",
            k.dimension,
            k.layout,
            mode(k.layout).map_or("-".to_string(), |m| m.to_string()),
            k.target,
            k.element.0,
            k.element.1,
            u8::from(k.mips),
            u8::from(k.deep),
            u8::from(k.samples),
            u8::from(xor),
            c
        );
    }
    tracing::info!("layout stats: {n} textures, {out_of_reach} of them with a swizzle mode the detiler does not address (xor modes or a layout field it does not know)");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_fields_name_swizzle_modes() {
        assert_eq!(mode(0x100 | 9), Some(9));
        assert_eq!(mode(0x100 | 27), Some(27));
        assert_eq!(mode(0), None);
        assert_eq!(mode(1), None);
        assert_eq!(mode(0x200), None);
    }
}
