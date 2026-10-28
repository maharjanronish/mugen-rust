//! Frame-number LRU — the decision core shared by the GPU texture cache.
//!
//! Pure data structure, no GL: the caller owns the heavy resources, asks the
//! LRU **which** entries to drop ([`FrameLru::evict_to_budget`]), and does the
//! GPU cleanup itself. That split keeps the eviction policy unit-testable on
//! the host and the GL layer thin.
//!
//! "Time" is a monotonically advancing frame counter, not a clock: fighting
//! games draw in strict frame order, and a counter makes the policy
//! deterministic and immune to clock jitter.

use std::collections::HashMap;
use std::hash::Hash;

/// Cached-entry bookkeeping (cost + last-use frame).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LruEntry {
    /// Bytes this entry accounts for (texture w*h*4 at RGBA8).
    pub cost: usize,
    /// Frame number of the last use.
    pub last: u64,
}

/// Budgeted LRU keyed by `K`, ordered by last-use frame.
#[derive(Debug)]
pub struct FrameLru<K: Copy + Eq + Hash> {
    budget: usize,
    used: usize,
    frame: u64,
    entries: HashMap<K, LruEntry>,
}

impl<K: Copy + Eq + Hash> FrameLru<K> {
    pub fn new(budget: usize) -> Self {
        FrameLru {
            budget,
            used: 0,
            frame: 0,
            entries: HashMap::new(),
        }
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    pub fn used(&self) -> usize {
        self.used
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Current frame number.
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Advance to the next frame (call once per rendered frame).
    pub fn advance_frame(&mut self) {
        self.frame += 1;
    }

    /// Register/refresh a use of `key` this frame. Inserting an existing key
    /// updates its cost (caller is responsible for the resource change — the
    /// LRU only tracks bytes).
    pub fn insert(&mut self, key: K, cost: usize) {
        let frame = self.frame;
        match self.entries.get_mut(&key) {
            Some(e) => {
                self.used = self.used - e.cost + cost;
                e.cost = cost;
                e.last = frame;
            }
            None => {
                self.used += cost;
                self.entries.insert(key, LruEntry { cost, last: frame });
            }
        }
    }

    /// Record a use without changing the cost (returns the entry).
    pub fn note_use(&mut self, key: &K) -> Option<LruEntry> {
        let frame = self.frame;
        self.entries.get_mut(key).map(|e| {
            e.last = frame;
            *e
        })
    }

    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    /// Drop least-recently-used entries until `used <= budget`. Returns the
    /// evicted `(key, cost)` pairs so the caller can destroy their resources.
    ///
    /// A single entry larger than the whole budget is still kept (the
    /// alternative — refusing the insert — would thrash: load, evict, load
    /// again every frame).
    pub fn evict_to_budget(&mut self) -> Vec<(K, usize)> {
        let mut evicted = Vec::new();
        if self.used <= self.budget || self.entries.len() <= 1 {
            return evicted;
        }
        let mut order: Vec<(K, u64, usize)> = self
            .entries
            .iter()
            .map(|(k, e)| (*k, e.last, e.cost))
            .collect();
        // Oldest last-use first; ties broken arbitrarily but stably in
        // practice (sort_unstable on (last, cost) — deterministic enough).
        order.sort_unstable_by_key(|&(_, last, cost)| (last, cost));
        let mut i = 0;
        while self.used > self.budget && i < order.len() - 1 {
            let (k, _, cost) = order[i];
            i += 1;
            if self.entries.remove(&k).is_some() {
                self.used -= cost;
                evicted.push((k, cost));
            }
        }
        evicted
    }

    /// Drop everything (context loss / scene teardown). Returns evicted pairs.
    pub fn clear(&mut self) -> Vec<(K, usize)> {
        let out: Vec<_> = self.entries.drain().map(|(k, e)| (k, e.cost)).collect();
        self.used = 0;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_enforced_with_lru_order() {
        let mut lru: FrameLru<u32> = FrameLru::new(100);
        lru.insert(1, 40);
        lru.advance_frame();
        lru.insert(2, 40);
        lru.advance_frame();
        lru.insert(3, 40); // over budget → evict key 1 (oldest)
        let ev = lru.evict_to_budget();
        assert_eq!(ev, vec![(1, 40)]);
        assert!(lru.contains(&2) && lru.contains(&3));
        assert!(!lru.contains(&1));
        assert_eq!(lru.used(), 80);
    }

    #[test]
    fn touching_rescues_an_entry() {
        let mut lru: FrameLru<u32> = FrameLru::new(100);
        lru.insert(1, 40);
        lru.advance_frame();
        lru.insert(2, 40);
        lru.advance_frame();
        lru.note_use(&1); // key 1 used THIS frame → 2 is now the oldest
        lru.insert(3, 40);
        let ev = lru.evict_to_budget();
        assert_eq!(ev, vec![(2, 40)]);
        assert!(lru.contains(&1) && lru.contains(&3));
    }

    #[test]
    fn single_oversize_entry_is_kept() {
        let mut lru: FrameLru<u32> = FrameLru::new(10);
        lru.insert(7, 500);
        assert!(lru.evict_to_budget().is_empty());
        assert!(lru.contains(&7));
    }

    #[test]
    fn insert_updates_cost_in_place() {
        let mut lru: FrameLru<u32> = FrameLru::new(1000);
        lru.insert(1, 100);
        lru.insert(1, 300);
        assert_eq!(lru.len(), 1);
        assert_eq!(lru.used(), 300);
        lru.advance_frame();
        assert_eq!(lru.note_use(&1).unwrap().last, 1);
    }

    #[test]
    fn clear_empties_and_reports() {
        let mut lru: FrameLru<(u16, u16)> = FrameLru::new(10);
        lru.insert((0, 0), 4);
        lru.insert((0, 1), 6);
        let out = lru.clear();
        assert_eq!(out.len(), 2);
        assert_eq!(lru.used(), 0);
        assert!(lru.is_empty());
    }
}
