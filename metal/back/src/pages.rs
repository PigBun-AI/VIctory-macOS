//! The "one physical page, at most one GPU buffer" registry (layering
//! rule 6).
//!
//! Two GPU buffers over the same page give two GPU addresses for one
//! memory, and Metal does not keep them coherent (nowak's probe, item 3;
//! plan 2.1). So `wrap` registers the page-rounded range of every
//! no-copy buffer here and refuses a range that touches a registered one.
//! Rounding is outward, to the page size of the process: 4 KiB under
//! Rosetta, 16 KiB natively. Two wraps that merely share a page therefore
//! collide, which is what rule 6 asks for.

use std::collections::BTreeMap;

use crate::error::{Error, Result};

/// Ranges of address space that are behind a no-copy buffer.
#[derive(Debug)]
pub struct PageRegistry {
    page_size: usize,
    /// start -> (end, id); disjoint, page aligned.
    ranges: BTreeMap<usize, (usize, u64)>,
    next_id: u64,
}

/// Proof of a registration; hand it back to [`PageRegistry::release`].
#[derive(Debug, PartialEq, Eq)]
pub struct Claim {
    id: u64,
    start: usize,
}

impl PageRegistry {
    /// An empty registry for `page_size`-byte pages (a power of two).
    pub fn new(page_size: usize) -> PageRegistry {
        assert!(page_size.is_power_of_two(), "page size {page_size}");
        PageRegistry {
            page_size,
            ranges: BTreeMap::new(),
            next_id: 1,
        }
    }

    /// The page size this registry rounds to.
    pub fn page_size(&self) -> usize {
        self.page_size
    }

    /// `[addr, addr + len)` rounded outwards to whole pages.
    pub fn page_range(&self, addr: usize, len: usize) -> Result<(usize, usize)> {
        if len == 0 {
            return Err(Error::BadArgument("empty range".into()));
        }
        let end = addr
            .checked_add(len)
            .ok_or_else(|| Error::BadArgument("range wraps the address space".into()))?;
        let mask = self.page_size - 1;
        let start = addr & !mask;
        let end = end
            .checked_add(mask)
            .ok_or_else(|| Error::BadArgument("range wraps the address space".into()))?
            & !mask;
        Ok((start, end))
    }

    /// Registers the pages of `[addr, addr + len)`; fails when any of them
    /// is already registered.
    pub fn claim(&mut self, addr: usize, len: usize) -> Result<Claim> {
        let (start, end) = self.page_range(addr, len)?;
        // Ranges are disjoint, so the only one that can reach into
        // [start, end) from below is the last one starting before `end`.
        if let Some((&s, &(e, _))) = self.ranges.range(..end).next_back()
            && e > start
        {
            return Err(Error::PageConflict {
                start,
                end,
                existing: (s, e),
            });
        }
        let id = self.next_id;
        self.next_id += 1;
        self.ranges.insert(start, (end, id));
        Ok(Claim { id, start })
    }

    /// Gives a claim back. Returns false for a claim already released.
    pub fn release(&mut self, claim: &Claim) -> bool {
        match self.ranges.get(&claim.start) {
            Some(&(_, id)) if id == claim.id => {
                self.ranges.remove(&claim.start);
                true
            }
            _ => false,
        }
    }

    /// How many ranges are registered.
    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    /// True when nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// Whether any page of `[addr, addr + len)` is registered.
    pub fn overlaps(&self, addr: usize, len: usize) -> bool {
        match self.page_range(addr, len) {
            Ok((start, end)) => self
                .ranges
                .range(..end)
                .next_back()
                .is_some_and(|(_, &(e, _))| e > start),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const K4: usize = 4096;
    const K16: usize = 16384;

    #[test]
    fn rounds_outwards() {
        let r = PageRegistry::new(K4);
        assert_eq!(r.page_range(0x1000, 1).unwrap(), (0x1000, 0x2000));
        assert_eq!(r.page_range(0x1fff, 2).unwrap(), (0x1000, 0x3000));
        assert_eq!(r.page_range(0x1000, K4).unwrap(), (0x1000, 0x2000));
        let r = PageRegistry::new(K16);
        assert_eq!(r.page_range(0x4000, 1).unwrap(), (0x4000, 0x8000));
        assert_eq!(r.page_range(0x5000, 0x4000).unwrap(), (0x4000, 0xc000));
    }

    #[test]
    fn overlapping_claim_is_refused() {
        let mut r = PageRegistry::new(K4);
        let a = r.claim(0x10000, 4 * K4).unwrap();
        // Same range, inside, straddling either end, enclosing.
        for (addr, len) in [
            (0x10000, 4 * K4),
            (0x11000, K4),
            (0xf000, 2 * K4),
            (0x13000, 2 * K4),
            (0x8000, 0x10000),
        ] {
            let err = r.claim(addr, len).unwrap_err();
            assert!(
                matches!(
                    err,
                    Error::PageConflict {
                        existing: (0x10000, 0x14000),
                        ..
                    }
                ),
                "({addr:#x}, {len:#x}): {err}"
            );
        }
        assert_eq!(r.len(), 1);
        assert!(r.release(&a));
        assert!(r.claim(0x11000, K4).is_ok());
    }

    #[test]
    fn adjacent_ranges_are_fine() {
        let mut r = PageRegistry::new(K4);
        r.claim(0x10000, K4).unwrap();
        r.claim(0x11000, K4).unwrap();
        r.claim(0xf000, K4).unwrap();
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn sharing_one_page_counts_as_overlap() {
        // Two wraps that are 4 KiB apart collide on a 16 KiB page (native),
        // and do not on a 4 KiB page (Rosetta).
        let mut native = PageRegistry::new(K16);
        native.claim(0x4000, K4).unwrap();
        assert!(native.claim(0x5000, K4).is_err());
        let mut rosetta = PageRegistry::new(K4);
        rosetta.claim(0x4000, K4).unwrap();
        assert!(rosetta.claim(0x5000, K4).is_ok());
    }

    #[test]
    fn a_stale_claim_cannot_release_a_newer_range() {
        let mut r = PageRegistry::new(K4);
        let old = r.claim(0x10000, K4).unwrap();
        assert!(r.release(&old));
        let new = r.claim(0x10000, K4).unwrap();
        assert!(
            !r.release(&old),
            "double release must not free the new claim"
        );
        assert!(r.overlaps(0x10000, 1));
        assert!(r.release(&new));
        assert!(r.is_empty());
    }

    #[test]
    fn bad_ranges_are_errors_not_panics() {
        let mut r = PageRegistry::new(K4);
        assert!(r.claim(0x1000, 0).is_err());
        assert!(r.claim(usize::MAX - 10, 100).is_err());
        assert!(r.claim(usize::MAX - 10, 5).is_err()); // rounds past the end
    }

    #[test]
    fn many_ranges_find_the_right_neighbour() {
        let mut r = PageRegistry::new(K4);
        let mut claims = Vec::new();
        for i in 0..100usize {
            claims.push(r.claim(0x100000 + i * 3 * K4, 2 * K4).unwrap());
        }
        // The gaps (one page each) are free; anything wider collides.
        for i in 0..99usize {
            let gap = 0x100000 + i * 3 * K4 + 2 * K4;
            assert!(!r.overlaps(gap, K4));
            assert!(r.overlaps(gap, K4 + 1));
        }
        assert!(r.claim(0x100000 + 50 * 3 * K4 - K4 / 2, K4).is_err());
    }
}
