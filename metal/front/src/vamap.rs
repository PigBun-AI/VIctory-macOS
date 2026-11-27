//! GPU virtual address → buffer, for the calls that name a buffer by address
//! (`WriteBufferImmediate`). The back's addresses are what `GetGPUVirtualAddress`
//! returns, so the front keeps its own table of the buffers it made: an
//! address maps to the resource number and the byte offset inside it.
//!
//! Placed buffers may overlap (aliasing): any live buffer that holds the whole
//! range answers, the one with the highest start first. Handles are the
//! back's (`ResId`); nothing here is derived from addresses beyond lookup, so
//! replaying the same calls gives the same answers (`hot-reload.md`).

use std::collections::BTreeMap;
use std::sync::Mutex;

/// How many starts below the address are looked at before giving up (heaps of
/// overlapping placed buffers make a longer walk possible but never useful).
const WALK: usize = 4096;

#[derive(Default)]
pub struct VaMap {
    /// `(start, resource number)` → size in bytes.
    buffers: Mutex<BTreeMap<(u64, u32), u64>>,
}

impl VaMap {
    pub fn insert(&self, start: u64, res: u32, size: u64) {
        if start != 0 && size != 0 {
            self.buffers.lock().unwrap().insert((start, res), size);
        }
    }

    pub fn remove(&self, start: u64, res: u32) {
        self.buffers.lock().unwrap().remove(&(start, res));
    }

    /// The buffer holding `len` bytes at `va`: its resource number and the
    /// offset of `va` inside it.
    pub fn find(&self, va: u64, len: u64) -> Option<(u32, u64)> {
        let end = va.checked_add(len)?;
        let map = self.buffers.lock().unwrap();
        map.range(..=(va, u32::MAX))
            .rev()
            .take(WALK)
            .find(|((start, _), size)| end <= start + **size)
            .map(|((start, res), _)| (*res, va - start))
    }

    pub fn len(&self) -> usize {
        self.buffers.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_finds_the_buffer_that_holds_the_range() {
        let m = VaMap::default();
        m.insert(0x1000, 1, 0x100);
        m.insert(0x4000, 2, 0x1000);
        assert_eq!(m.find(0x1000, 4), Some((1, 0)));
        assert_eq!(m.find(0x10fc, 4), Some((1, 0xfc)));
        assert_eq!(m.find(0x10fd, 4), None, "runs past the end");
        assert_eq!(m.find(0x2000, 4), None, "between buffers");
        assert_eq!(m.find(0x4ff8, 8), Some((2, 0xff8)));
        m.remove(0x1000, 1);
        assert_eq!(m.find(0x1000, 4), None);
    }

    #[test]
    fn overlapping_placed_buffers_answer_by_the_highest_start_that_fits() {
        let m = VaMap::default();
        m.insert(0x1000, 1, 0x1000);
        m.insert(0x1800, 2, 0x100);
        assert_eq!(m.find(0x1804, 4), Some((2, 4)));
        // Past the small one the big one still holds it.
        assert_eq!(m.find(0x1900, 4), Some((1, 0x900)));
    }
}
