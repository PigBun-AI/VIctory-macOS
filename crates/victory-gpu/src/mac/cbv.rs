//! G15: Scarlett constant-buffer placement on the Metal chain. Raw SRVs have
//! a separate contract. Never round an address: that shifts shader reads.
#![cfg_attr(not(windows), allow(dead_code))]

fn placement(metal: bool, setting: Option<&str>) -> u64 {
    if metal && setting != Some("256") { 16 } else { 256 }
}

pub(crate) fn alignment() -> u64 {
    static ALIGN: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *ALIGN.get_or_init(|| placement(super::on_metal_chain(), std::env::var("VICTORY_CBV_ALIGN").ok().as_deref()))
}

pub(crate) fn extent(size: u32, align: u64) -> u32 {
    size.min(64 << 10).next_multiple_of(align as u32)
}

pub(crate) fn accepts(title: u64, gpu: u64, align: u64) -> bool {
    // Preserve the old host-only check exactly for A/B and non-Metal.
    gpu.is_multiple_of(align) && (align == 256 || title.is_multiple_of(16))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metal_accepts_scarlett_placement_without_bias() {
        let a = placement(true, None);
        for residue in 0..256 {
            assert_eq!(accepts(0x1000 + residue, 0x2000 + residue, a), residue % 16 == 0);
        }
        assert_eq!(extent(17, a), 32);
        assert_eq!(extent(480, a), 480);
        assert_eq!(extent(u32::MAX, a), 65536);
        assert!(accepts(0xddc4ac050, 0x1010, a));
        assert!(!accepts(0x1004, 0x2010, a));
        assert!(!accepts(0x1010, 0x2004, a));
    }
    #[test]
    fn reference_and_ab_keep_the_original_host_only_guard() {
        for (metal, setting) in [(false, None), (false, Some("16")), (true, Some("256"))] {
            let a = placement(metal, setting);
            for residue in 0..256 {
                assert_eq!(accepts(0x1050, 0x2000 + residue, a), residue == 0);
            }
        }
    }
}
