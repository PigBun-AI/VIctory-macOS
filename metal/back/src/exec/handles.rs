//! Handle tables: the back allocates every handle (`u32`, 0 is null).
//!
//! Handles count up from 1 per table and are never reused, so a stale
//! handle stays stale (it names nothing) instead of silently naming a new
//! object; a table that has used every number refuses new objects. The
//! counting makes handles deterministic for a deterministic caller, which
//! the golden replay relies on in tests, but nothing may depend on it.

use std::collections::HashMap;

/// One kind of object, by handle.
pub(crate) struct Table<T> {
    next: u32,
    map: HashMap<u32, T>,
}

impl<T> Default for Table<T> {
    fn default() -> Self {
        Table {
            next: 1,
            map: HashMap::new(),
        }
    }
}

impl<T> Table<T> {
    /// Stores `value` and returns its new handle, or gives it back when the
    /// numbers ran out.
    pub(crate) fn insert(&mut self, value: T) -> Result<u32, T> {
        if self.next == 0 {
            return Err(value);
        }
        let id = self.next;
        // `next` wraps to 0 after u32::MAX: the table is then full for good.
        self.next = self.next.wrapping_add(1);
        self.map.insert(id, value);
        Ok(id)
    }

    pub(crate) fn get(&self, id: u32) -> Option<&T> {
        self.map.get(&id)
    }

    /// Puts `value` under an existing handle.
    pub(crate) fn replace(&mut self, id: u32, value: T) {
        if self.map.contains_key(&id) {
            self.map.insert(id, value);
        }
    }

    pub(crate) fn remove(&mut self, id: u32) -> Option<T> {
        self.map.remove(&id)
    }

    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &T> {
        self.map.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_count_from_one_and_are_not_reused() {
        let mut t = Table::default();
        assert_eq!(t.insert("a"), Ok(1));
        assert_eq!(t.insert("b"), Ok(2));
        assert_eq!(t.remove(1), Some("a"));
        assert_eq!(t.get(1), None, "a stale handle names nothing");
        assert_eq!(t.insert("c"), Ok(3));
        assert_eq!(t.get(0), None, "0 is null");
        assert_eq!(t.get(2), Some(&"b"));
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn a_table_that_ran_out_of_numbers_refuses() {
        let mut t = Table {
            next: u32::MAX,
            ..Table::default()
        };
        assert_eq!(t.insert("last"), Ok(u32::MAX));
        assert_eq!(t.insert("none"), Err("none"));
        assert_eq!(t.get(u32::MAX), Some(&"last"));
    }
}
