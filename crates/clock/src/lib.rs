//! THE KERNEL's only ordering primitive: logical clocks. Nothing in this
//! kernel reads physical time — per `docs/design/CONSTRAINTS.md` there is
//! no trusted wall clock — so "what happened first" is answered purely by
//! causality: Lamport scalar clocks for a cheap total order consistent
//! with causality, vector clocks for the exact happened-before relation.
//! See `docs/design/decisions/ADR-005-logical-clocks.md`.
//!
//! Generic over the process-identifier type so this crate depends on
//! nothing; `process` instantiates it with its own `ProcessId`.

mod lamport;
mod vector;

pub use lamport::LamportClock;
pub use vector::{Causality, VectorClock};

/// The logical timestamp of one event: which process it happened at, its
/// Lamport time, and its vector time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp<K: Ord + Copy> {
    pub process: K,
    pub lamport: u64,
    pub vector: VectorClock<K>,
}

impl<K: Ord + Copy> Stamp<K> {
    /// Lamport's total order: Lamport time, ties broken by process id.
    /// Consistent with causality (if `a` happened before `b`, `a`'s key is
    /// smaller) but it also orders concurrent events, arbitrarily.
    pub fn total_order_key(&self) -> (u64, K) {
        (self.lamport, self.process)
    }

    pub fn causality(&self, other: &Stamp<K>) -> Causality {
        self.vector.compare(&other.vector)
    }

    pub fn happened_before(&self, other: &Stamp<K>) -> bool {
        self.vector.happened_before(&other.vector)
    }
}

/// One process's pair of clocks, advanced together. Each method is one
/// event and returns that event's stamp.
#[derive(Debug, Clone)]
pub struct EventClock<K: Ord + Copy> {
    process: K,
    lamport: LamportClock,
    vector: VectorClock<K>,
}

impl<K: Ord + Copy> EventClock<K> {
    pub fn new(process: K) -> Self {
        Self {
            process,
            lamport: LamportClock::new(),
            vector: VectorClock::new(),
        }
    }

    /// The stamp of this process's most recent event (all zeros if none).
    pub fn current(&self) -> Stamp<K> {
        Stamp {
            process: self.process,
            lamport: self.lamport.time(),
            vector: self.vector.clone(),
        }
    }

    /// A purely local event.
    pub fn local_event(&mut self) -> Stamp<K> {
        self.lamport.tick();
        self.vector.tick(self.process);
        self.current()
    }

    /// A send event. The returned stamp is what the message carries.
    pub fn send_event(&mut self) -> Stamp<K> {
        self.local_event()
    }

    /// Receiving a message stamped `sent`: merge what the sender knew,
    /// then record the receive as a new event.
    pub fn receive_event(&mut self, sent: &Stamp<K>) -> Stamp<K> {
        self.lamport.observe(sent.lamport);
        self.vector.merge(&sent.vector);
        self.vector.tick(self.process);
        self.current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_receive_happens_after_its_send() {
        let mut a = EventClock::new(0u32);
        let mut b = EventClock::new(1u32);
        let sent = a.send_event();
        let received = b.receive_event(&sent);
        assert!(sent.happened_before(&received));
        assert!(sent.lamport < received.lamport);
    }

    #[test]
    fn independent_local_events_are_concurrent_even_with_ordered_lamport_times() {
        let mut a = EventClock::new(0u32);
        let mut b = EventClock::new(1u32);
        let a1 = a.local_event();
        b.local_event();
        let b2 = b.local_event();
        assert_eq!(a1.causality(&b2), Causality::Concurrent);
        // Lamport alone would have suggested an order that isn't causal.
        assert!(a1.lamport < b2.lamport);
    }

    #[test]
    fn total_order_breaks_lamport_ties_by_process() {
        let mut a = EventClock::new(0u32);
        let mut b = EventClock::new(1u32);
        let a1 = a.local_event();
        let b1 = b.local_event();
        assert_eq!(a1.lamport, b1.lamport);
        assert!(a1.total_order_key() < b1.total_order_key());
    }

    #[test]
    fn current_before_any_event_is_all_zero() {
        let c = EventClock::new(3u32);
        let s = c.current();
        assert_eq!(s.lamport, 0);
        assert_eq!(s.vector, VectorClock::new());
    }
}
