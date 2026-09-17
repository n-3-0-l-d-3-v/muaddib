//! Lamport scalar clocks (Lamport, "Time, Clocks, and the Ordering of
//! Events in a Distributed System", CACM 1978). A single counter per
//! process satisfying the *clock condition*: if event `a` happened before
//! event `b`, then `C(a) < C(b)`. The converse does not hold — two
//! concurrent events can have any relative Lamport times — which is
//! exactly why `VectorClock` exists alongside this.

/// One process's Lamport clock. Starts at 0, meaning "no event yet"; the
/// first event is time 1.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LamportClock {
    time: u64,
}

impl LamportClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// The time of this process's most recent event (0 if none).
    pub fn time(&self) -> u64 {
        self.time
    }

    /// A local or send event: advance by one and return the event's time.
    pub fn tick(&mut self) -> u64 {
        self.time += 1;
        self.time
    }

    /// A receive event for a message stamped `remote`: jump past both
    /// this clock and the sender's, so the receive is strictly later than
    /// the send that caused it.
    pub fn observe(&mut self, remote: u64) -> u64 {
        self.time = self.time.max(remote) + 1;
        self.time
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_advance_by_one_from_zero() {
        let mut c = LamportClock::new();
        assert_eq!(c.time(), 0);
        assert_eq!(c.tick(), 1);
        assert_eq!(c.tick(), 2);
    }

    #[test]
    fn observing_a_later_remote_time_jumps_past_it() {
        let mut c = LamportClock::new();
        c.tick();
        assert_eq!(c.observe(10), 11);
    }

    #[test]
    fn observing_an_earlier_remote_time_still_advances() {
        let mut c = LamportClock::new();
        for _ in 0..5 {
            c.tick();
        }
        assert_eq!(c.observe(2), 6);
    }
}
