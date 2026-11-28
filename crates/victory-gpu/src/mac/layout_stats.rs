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

// total, nonzero first, explicit size, full, strict subrange, unknown (one coherent snapshot).
static UAV3D: Mutex<[u64; 6]> = Mutex::new([0; 6]);
type Volume = (u64, u32, u16, u16); // width, height, base depth, declared mip count

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Range { Full, Partial, Unknown }

fn range(volume: Option<Volume>, mip: u32, first: u32, size: u32) -> Range {
    let Some((w, h, d, levels)) = volume else { return Range::Unknown };
    if w == 0 || h == 0 || d == 0 { return Range::Unknown; }
    let chain = 1 + w.max(h.into()).max(d.into()).ilog2();
    let levels = if levels == 0 { chain } else { u32::from(levels) };
    if mip >= levels || levels > chain { return Range::Unknown; }
    let depth = u32::from(d).checked_shr(mip).unwrap_or(0).max(1);
    if first >= depth { return Range::Unknown; }
    let rest = depth - first;
    let size = if size == u32::MAX { rest } else { size };
    if size == 0 || size > rest { Range::Unknown }
    else if first == 0 && size == depth { Range::Full }
    else { Range::Partial }
}

#[test]
fn classifies_the_range_at_the_selected_volume_mip() {
    // Microsoft Learn: D3D12_TEX3D_UAV (MipSlice, FirstWSlice, WSize; -1 = remainder),
    // D3D12_RESOURCE_DESC (MipLevels=0 = full chain). Depth halves to a minimum of one.
    use Range::*;
    let v = Some((32, 8, 9, 0));
    for (m,f,s,want) in [(1,0,4,Full),(1,0,u32::MAX,Full),(1,0,3,Partial),
        (1,1,u32::MAX,Partial),(1,2,1,Partial),(1,1,4,Unknown),(1,4,1,Unknown),
        (1,0,0,Unknown),(5,0,1,Full),(6,0,1,Unknown)] {
        assert_eq!(range(v,m,f,s),want);
    }
    assert_eq!(range(Some((32,8,9,1)),1,0,4),Unknown);
    assert_eq!(range(None,0,0,1),Unknown);
}

/// A 3D unordered access view the title made (`words`: the description): counted as all, with a first depth slice
/// other than 0, and with a size that is not "the rest" (`u32::MAX`). D3DMetal refuses partial views of a 3D texture
/// (matrix row C-V-012, finding F1); this says whether the title makes any. Logged at the powers of two of the count.
pub(crate) fn uav_3d(words: &[u32; 10], volume: impl FnOnce() -> Option<Volume>) {
    if words[1] != 8 || (!*ENABLED && !super::drop_stats::enabled()) {
        return;
    }
    // words: format, dimension, MipSlice, FirstWSlice, WSize
    let class = match range(volume(), words[2], words[3], words[4]) {
        Range::Full => 3, Range::Partial => 4, Range::Unknown => 5,
    };
    if super::drop_stats::enabled() { super::drop_stats::uav_range(class - 3); }
    if !*ENABLED { return; }
    let mut counts = UAV3D.lock().unwrap_or_else(|e| e.into_inner());
    counts[0] += 1;
    counts[1] += u64::from(words[3] != 0);
    counts[2] += u64::from(words[4] != u32::MAX);
    counts[class] += 1;
    if counts[0].is_power_of_two() || counts[class] == 1 {
        let [n, first, explicit, full, partial, unknown] = *counts;
        tracing::info!("layout stats: {n} 3D unordered access views, {first} from a depth slice other than 0, {explicit} with a size other than the rest; full={full} strict_subrange={partial} unknown={unknown}");
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
