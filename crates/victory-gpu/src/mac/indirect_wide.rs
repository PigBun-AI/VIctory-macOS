//! mac: how many groups in x an indirect dispatch runs as the title asks (`indirect::widest`'s decision, apart
//! from the device query so that it is tested natively).
//!
//! D3D12 names 65,535 at most for a dispatch; the title's tile lists are 67,440 and 47,760 groups at 2560x1440,
//! and cut to 65,535 the first lost its last 1,905 tiles (the lowest band of the picture). An AMD card's packets and
//! the Metal chain's `dispatchThreadgroupsWithIndirectBuffer` (32-bit counts; `metal/back/tests/exec_indirect_wide.rs`
//! runs 3,000,000 groups) take what is asked, so both get 2^20 (the clamp's own bound on the total).
//! `VICTORY_INDIRECT_WIDE=0` (`on` false) keeps 65,535 everywhere.

pub fn widest(amd: bool, metal_chain: bool, on: bool) -> u32 {
    if (amd || metal_chain) && on { 1 << 20 } else { 65_535 }
}

#[cfg(test)]
mod tests {
    use super::widest;

    #[test]
    fn the_metal_chain_and_amd_cards_run_a_tile_list_whole_and_others_keep_the_d3d12_limit() {
        assert_eq!(widest(false, true, true), 1 << 20);
        assert_eq!(widest(true, false, true), 1 << 20);
        assert_eq!(widest(false, false, true), 65_535);
        // `VICTORY_INDIRECT_WIDE=0` turns it off on every chain.
        assert_eq!(widest(false, true, false), 65_535);
        assert_eq!(widest(true, true, false), 65_535);
        // 67,440 and 47,760 are the title's tile lists at 2560x1440.
        assert!(widest(false, true, true) >= 67_440);
    }
}
