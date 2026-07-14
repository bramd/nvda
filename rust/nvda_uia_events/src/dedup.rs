//! The insertion-ordered, de-duplicating event queue.
//!
//! Port of the queue in `RateLimitedEventHandler::queueEvent` /
//! `flushEvents` (`rateLimitedEventHandler.cpp`), which pairs a
//! `std::list` (insertion order) with a `std::map<key,{iterator,count}>`
//! (dedup index). On a duplicate coalescing key it keeps the **newest**
//! record and moves it to the **back** of the queue; `drain` emits every
//! surviving record in order and empties the queue.
//!
//! Here that is a `Vec` of insertion-ordered slots plus a `HashMap` from
//! key to the live slot. A coalesced-away slot is tombstoned (`None`)
//! rather than physically removed; `drain` skips tombstones. Tombstones
//! only live until the next `drain`, which the flusher issues promptly, so
//! the transient overhead is bounded by one flush window. The C++ `count`
//! field (tracked but never emitted) is intentionally dropped.

use std::collections::HashMap;

/// An insertion-ordered queue that coalesces entries sharing a key, keeping
/// the newest value at the back. `P` is the per-event payload (in Phase 2,
/// the COM record that gets emitted).
pub struct OrderedDedup<P> {
    /// Insertion-ordered slots; `None` == coalesced away (tombstone).
    slots: Vec<(Vec<i32>, Option<P>)>,
    /// Coalescing key -> index of its live slot in `slots`.
    index: HashMap<Vec<i32>, usize>,
}

impl<P> OrderedDedup<P> {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// Number of live (non-coalesced) entries currently queued.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Queue `payload` under `key`. If an entry with the same key is already
    /// queued, it is coalesced: the old one is dropped and this newer one
    /// takes its place at the back of the queue.
    pub fn insert(&mut self, key: Vec<i32>, payload: P) {
        if let Some(&old_idx) = self.index.get(&key) {
            // Tombstone the prior entry; the new one supersedes it.
            self.slots[old_idx].1 = None;
        }
        let new_idx = self.slots.len();
        self.index.insert(key.clone(), new_idx);
        self.slots.push((key, Some(payload)));
    }

    /// Remove and return every live entry in insertion (coalesced) order,
    /// leaving the queue empty.
    pub fn drain(&mut self) -> Vec<P> {
        self.index.clear();
        self.slots
            .drain(..)
            .filter_map(|(_, payload)| payload)
            .collect()
    }
}

impl<P> Default for OrderedDedup<P> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(id: i32) -> Vec<i32> {
        vec![id]
    }

    #[test]
    fn distinct_keys_preserve_insertion_order() {
        let mut q = OrderedDedup::new();
        q.insert(k(1), "a");
        q.insert(k(2), "b");
        q.insert(k(3), "c");
        assert_eq!(q.len(), 3);
        assert_eq!(q.drain(), vec!["a", "b", "c"]);
    }

    #[test]
    fn duplicate_key_keeps_newest_value() {
        let mut q = OrderedDedup::new();
        q.insert(k(1), "old");
        q.insert(k(1), "new");
        assert_eq!(q.len(), 1);
        assert_eq!(q.drain(), vec!["new"]);
    }

    #[test]
    fn coalesced_entry_moves_to_the_back() {
        let mut q = OrderedDedup::new();
        q.insert(k(1), "a1");
        q.insert(k(2), "b");
        q.insert(k(1), "a2"); // coalesces key 1, moves it after b
        assert_eq!(q.len(), 2);
        assert_eq!(q.drain(), vec!["b", "a2"]);
    }

    #[test]
    fn drain_empties_the_queue() {
        let mut q = OrderedDedup::new();
        q.insert(k(1), "a");
        assert_eq!(q.drain(), vec!["a"]);
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);
        // Reusable after drain.
        q.insert(k(2), "b");
        assert_eq!(q.drain(), vec!["b"]);
    }

    #[test]
    fn repeated_coalescing_on_one_key_yields_one_entry() {
        let mut q = OrderedDedup::new();
        for i in 0..1000 {
            q.insert(k(7), i);
        }
        assert_eq!(q.len(), 1);
        assert_eq!(q.drain(), vec![999]);
    }

    #[test]
    fn interleaved_keys_coalesce_independently() {
        let mut q = OrderedDedup::new();
        q.insert(k(1), "a1");
        q.insert(k(2), "b1");
        q.insert(k(1), "a2");
        q.insert(k(3), "c1");
        q.insert(k(2), "b2");
        // Surviving order: key1 last touched before key3/key2 re-touch...
        // trace: [a1] [a1,b1] [b1,a2] [b1,a2,c1] [a2,c1,b2]
        assert_eq!(q.drain(), vec!["a2", "c1", "b2"]);
    }
}
