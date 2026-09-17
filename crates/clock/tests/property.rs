//! Property tests for the clocks in isolation (the kernel-integrated
//! version, driven through real IPC and spawns, lives in `ipc`'s tests).
//!
//! 1. Vector clocks form a join-semilattice under `merge`, with `compare`
//!    a genuine partial order.
//! 2. **Ground truth**: for arbitrary interleavings of local events, sends
//!    and receives across simulated processes, the harness builds the
//!    actual event graph (each event's predecessors: the previous event at
//!    the same process, plus the matching send for a receive) and computes
//!    happened-before by reachability. Vector-clock order must equal it
//!    exactly, and Lamport time must respect it (the clock condition).

use clock::{Causality, EventClock, Stamp, VectorClock};
use proptest::prelude::*;

const PROCESSES: u32 = 4;

fn arb_vector() -> impl Strategy<Value = VectorClock<u32>> {
    prop::collection::vec((0..PROCESSES, 0u64..4), 0..8).prop_map(|ticks| {
        let mut v = VectorClock::new();
        for (p, n) in ticks {
            for _ in 0..n {
                v.tick(p);
            }
        }
        v
    })
}

fn merged(a: &VectorClock<u32>, b: &VectorClock<u32>) -> VectorClock<u32> {
    let mut m = a.clone();
    m.merge(b);
    m
}

#[derive(Debug, Clone)]
enum Op {
    Local(u32),
    Send {
        from: u32,
        to: u32,
    },
    /// Receive the oldest undelivered message addressed to this process.
    Receive(u32),
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..PROCESSES).prop_map(Op::Local),
        (0..PROCESSES, 0..PROCESSES).prop_map(|(from, to)| Op::Send { from, to }),
        (0..PROCESSES).prop_map(Op::Receive),
    ]
}

/// The harness's own ground truth: `reach[i]` = every event `i` causally
/// depends on (transitively), computed from explicit predecessor edges —
/// no clocks involved.
fn transitive_predecessors(preds: &[Vec<usize>]) -> Vec<Vec<bool>> {
    let n = preds.len();
    let mut reach = vec![vec![false; n]; n];
    // Events are appended in execution order, so every predecessor has a
    // smaller index and one forward pass suffices.
    for (i, event_preds) in preds.iter().enumerate() {
        let (earlier, rest) = reach.split_at_mut(i);
        let row = &mut rest[0];
        for &p in event_preds {
            row[p] = true;
            for (dst, src) in row.iter_mut().zip(&earlier[p]) {
                *dst |= *src;
            }
        }
    }
    reach
}

proptest! {
    #[test]
    fn merge_is_commutative_associative_and_idempotent(
        a in arb_vector(), b in arb_vector(), c in arb_vector(),
    ) {
        prop_assert_eq!(merged(&a, &b), merged(&b, &a));
        prop_assert_eq!(merged(&merged(&a, &b), &c), merged(&a, &merged(&b, &c)));
        prop_assert_eq!(merged(&a, &a), a);
    }

    #[test]
    fn merge_is_the_least_upper_bound(
        a in arb_vector(), b in arb_vector(), upper in arb_vector(),
    ) {
        let m = merged(&a, &b);
        prop_assert!(a <= m && b <= m);
        if a <= upper && b <= upper {
            prop_assert!(m <= upper);
        }
    }

    #[test]
    fn compare_is_antisymmetric_and_transitive(
        a in arb_vector(), b in arb_vector(), c in arb_vector(),
    ) {
        let mirrored = match a.compare(&b) {
            Causality::Before => Causality::After,
            Causality::After => Causality::Before,
            other => other,
        };
        prop_assert_eq!(b.compare(&a), mirrored);
        if a < b && b < c {
            prop_assert!(a < c);
        }
    }

    #[test]
    fn vector_order_is_exactly_the_true_happened_before_relation(
        ops in prop::collection::vec(arb_op(), 1..80),
    ) {
        let mut clocks: Vec<EventClock<u32>> = (0..PROCESSES).map(EventClock::new).collect();
        let mut stamps: Vec<Stamp<u32>> = Vec::new();
        let mut preds: Vec<Vec<usize>> = Vec::new();
        let mut last_event: Vec<Option<usize>> = vec![None; PROCESSES as usize];
        // Per recipient: FIFO of (send event index, stamp).
        let mut inbox: Vec<std::collections::VecDeque<(usize, Stamp<u32>)>> =
            (0..PROCESSES).map(|_| Default::default()).collect();

        for op in ops {
            let (p, stamp, mut event_preds) = match op {
                Op::Local(p) => (p, clocks[p as usize].local_event(), vec![]),
                Op::Send { from, to } => {
                    let s = clocks[from as usize].send_event();
                    inbox[to as usize].push_back((stamps.len(), s.clone()));
                    (from, s, vec![])
                }
                Op::Receive(p) => {
                    let Some((send_idx, sent)) = inbox[p as usize].pop_front() else { continue };
                    (p, clocks[p as usize].receive_event(&sent), vec![send_idx])
                }
            };
            if let Some(prev) = last_event[p as usize] {
                event_preds.push(prev);
            }
            last_event[p as usize] = Some(stamps.len());
            stamps.push(stamp);
            preds.push(event_preds);
        }

        let reach = transitive_predecessors(&preds);
        for i in 0..stamps.len() {
            for j in 0..stamps.len() {
                if i == j {
                    continue;
                }
                let truly_before = reach[j][i];
                prop_assert_eq!(
                    stamps[i].happened_before(&stamps[j]),
                    truly_before,
                    "event {} vs {}", i, j
                );
                if truly_before {
                    prop_assert!(stamps[i].lamport < stamps[j].lamport);
                    prop_assert!(stamps[i].total_order_key() < stamps[j].total_order_key());
                }
                let concurrent = !reach[i][j] && !reach[j][i];
                prop_assert_eq!(stamps[i].causality(&stamps[j]) == Causality::Concurrent, concurrent);
            }
        }
    }
}
