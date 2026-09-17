//! Vector clocks (Fidge 1988; Mattern 1989). One counter per process;
//! unlike a Lamport clock these characterize causality exactly: `a`
//! happened before `b` **iff** `V(a) < V(b)` pointwise, and two events
//! are concurrent iff neither vector dominates the other.
//!
//! Stored sparsely: a process absent from the map has counter 0. The
//! map never holds an explicit 0 (counters only ever enter it by being
//! incremented to at least 1, or by merging a value that did), but
//! equality and ordering are defined over the union of keys anyway, so
//! nothing depends on that representation detail.

use std::cmp::Ordering;
use std::collections::BTreeMap;

/// How two vector timestamps relate causally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Causality {
    /// `self` happened before `other`.
    Before,
    /// `self` happened after `other`.
    After,
    /// The same timestamp.
    Equal,
    /// Neither happened before the other.
    Concurrent,
}

#[derive(Debug, Clone)]
pub struct VectorClock<K: Ord + Copy> {
    entries: BTreeMap<K, u64>,
}

impl<K: Ord + Copy> Default for VectorClock<K> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }
}

impl<K: Ord + Copy> VectorClock<K> {
    pub fn new() -> Self {
        Self::default()
    }

    /// `process`'s counter (0 if it has never been seen).
    pub fn get(&self, process: K) -> u64 {
        self.entries.get(&process).copied().unwrap_or(0)
    }

    /// Records one event at `process`.
    pub fn tick(&mut self, process: K) {
        *self.entries.entry(process).or_insert(0) += 1;
    }

    /// Pointwise maximum with `other` — the least upper bound of the two:
    /// everything either timestamp knows about, and nothing more.
    pub fn merge(&mut self, other: &VectorClock<K>) {
        for (&process, &count) in &other.entries {
            let entry = self.entries.entry(process).or_insert(0);
            *entry = (*entry).max(count);
        }
    }

    /// Every process with a non-zero counter, with its counter.
    pub fn entries(&self) -> impl Iterator<Item = (K, u64)> + '_ {
        self.entries.iter().map(|(&k, &v)| (k, v))
    }

    pub fn compare(&self, other: &VectorClock<K>) -> Causality {
        let mut some_less = false;
        let mut some_greater = false;
        for process in self.entries.keys().chain(other.entries.keys()) {
            match self.get(*process).cmp(&other.get(*process)) {
                Ordering::Less => some_less = true,
                Ordering::Greater => some_greater = true,
                Ordering::Equal => {}
            }
        }
        match (some_less, some_greater) {
            (false, false) => Causality::Equal,
            (true, false) => Causality::Before,
            (false, true) => Causality::After,
            (true, true) => Causality::Concurrent,
        }
    }

    pub fn happened_before(&self, other: &VectorClock<K>) -> bool {
        self.compare(other) == Causality::Before
    }

    pub fn concurrent_with(&self, other: &VectorClock<K>) -> bool {
        self.compare(other) == Causality::Concurrent
    }
}

impl<K: Ord + Copy> PartialEq for VectorClock<K> {
    fn eq(&self, other: &Self) -> bool {
        self.compare(other) == Causality::Equal
    }
}

impl<K: Ord + Copy> Eq for VectorClock<K> {}

/// The causal partial order: `None` for concurrent timestamps.
impl<K: Ord + Copy> PartialOrd for VectorClock<K> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match self.compare(other) {
            Causality::Before => Some(Ordering::Less),
            Causality::After => Some(Ordering::Greater),
            Causality::Equal => Some(Ordering::Equal),
            Causality::Concurrent => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vc(pairs: &[(u32, u64)]) -> VectorClock<u32> {
        let mut v = VectorClock::new();
        for &(p, n) in pairs {
            for _ in 0..n {
                v.tick(p);
            }
        }
        v
    }

    #[test]
    fn an_empty_clock_reads_zero_everywhere() {
        let v: VectorClock<u32> = VectorClock::new();
        assert_eq!(v.get(7), 0);
    }

    #[test]
    fn tick_advances_only_its_own_entry() {
        let v = vc(&[(1, 2)]);
        assert_eq!(v.get(1), 2);
        assert_eq!(v.get(2), 0);
    }

    #[test]
    fn a_dominated_clock_happened_before() {
        let a = vc(&[(1, 1)]);
        let b = vc(&[(1, 2), (2, 1)]);
        assert_eq!(a.compare(&b), Causality::Before);
        assert_eq!(b.compare(&a), Causality::After);
        assert!(a < b);
    }

    #[test]
    fn incomparable_clocks_are_concurrent() {
        let a = vc(&[(1, 1)]);
        let b = vc(&[(2, 1)]);
        assert!(a.concurrent_with(&b));
        assert_eq!(a.partial_cmp(&b), None);
    }

    #[test]
    fn merge_is_the_pointwise_maximum() {
        let mut a = vc(&[(1, 3), (2, 1)]);
        a.merge(&vc(&[(2, 4), (3, 1)]));
        assert_eq!(a, vc(&[(1, 3), (2, 4), (3, 1)]));
    }

    #[test]
    fn equality_ignores_explicit_zero_entries() {
        let mut with_zero = VectorClock::new();
        with_zero.entries.insert(5u32, 0);
        assert_eq!(with_zero, VectorClock::new());
    }
}
