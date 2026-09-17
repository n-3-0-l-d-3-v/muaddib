//! Ticket 005's property test, run through the real kernel rather than
//! bare clocks: processes created with `Scheduler::spawn`/`spawn_child`,
//! messages sent and received through `ChannelRegistry`, local events
//! recorded on `Process`. The harness controls scheduling, so it knows
//! the true causal structure. It builds the event graph itself (each
//! event's predecessors: the previous event at the same process, plus the
//! matching send for a receive or a child's birth) and computes
//! happened-before by reachability. No clock is involved in the ground
//! truth. The stamps the kernel hands back must agree with it exactly.

use std::collections::VecDeque;

use capability::{Capability, Kernel};
use ipc::ChannelRegistry;
use process::{ProcessId, Scheduler, Stamp};
use proptest::prelude::*;

const MAX_PROCESSES: usize = 6;

#[derive(Debug, Clone)]
enum Op {
    Local(usize),
    Send { from: usize, to: usize },
    Receive(usize),
    Spawn { parent: usize },
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => any::<usize>().prop_map(Op::Local),
        4 => (any::<usize>(), any::<usize>()).prop_map(|(from, to)| Op::Send { from, to }),
        4 => any::<usize>().prop_map(Op::Receive),
        1 => any::<usize>().prop_map(|parent| Op::Spawn { parent }),
    ]
}

struct Harness {
    k: Kernel,
    reg: ChannelRegistry,
    sched: Scheduler,
    pids: Vec<ProcessId>,
    /// One inbox channel per process.
    inbox: Vec<Capability>,
    /// The harness's own record of what's in each inbox: send event indices.
    in_flight: Vec<VecDeque<usize>>,
    stamps: Vec<Stamp>,
    preds: Vec<Vec<usize>>,
    last_event: Vec<Option<usize>>,
}

impl Harness {
    fn add_process(&mut self, pid: ProcessId) {
        self.pids.push(pid);
        self.inbox.push(self.reg.new_channel(&mut self.k));
        self.in_flight.push(VecDeque::new());
        self.last_event.push(None);
    }

    fn record(&mut self, p: usize, stamp: Stamp, mut preds: Vec<usize>) -> usize {
        if let Some(prev) = self.last_event[p] {
            preds.push(prev);
        }
        let idx = self.stamps.len();
        self.last_event[p] = Some(idx);
        self.stamps.push(stamp);
        self.preds.push(preds);
        idx
    }
}

fn transitive_predecessors(preds: &[Vec<usize>]) -> Vec<Vec<bool>> {
    let n = preds.len();
    let mut reach = vec![vec![false; n]; n];
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
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn kernel_stamps_agree_with_the_true_happened_before_relation(
        initial in 2usize..4,
        ops in prop::collection::vec(arb_op(), 1..80),
    ) {
        let mut h = Harness {
            k: Kernel::new(),
            reg: ChannelRegistry::new(),
            sched: Scheduler::new(),
            pids: vec![],
            inbox: vec![],
            in_flight: vec![],
            stamps: vec![],
            preds: vec![],
            last_event: vec![],
        };
        for _ in 0..initial {
            let pid = h.sched.spawn(vec![]);
            h.add_process(pid);
        }

        for op in ops {
            let n = h.pids.len();
            match op {
                Op::Local(p) => {
                    let p = p % n;
                    let s = h.sched.process_mut(h.pids[p]).unwrap().record_local_event();
                    h.record(p, s, vec![]);
                }
                Op::Send { from, to } => {
                    let (from, to) = (from % n, to % n);
                    let channel = h.inbox[to];
                    let sender = h.sched.process_mut(h.pids[from]).unwrap();
                    let s = h.reg.send(&mut h.k, &channel, sender, vec![], 0).unwrap();
                    let idx = h.record(from, s, vec![]);
                    h.in_flight[to].push_back(idx);
                }
                Op::Receive(p) => {
                    let p = p % n;
                    let channel = h.inbox[p];
                    let receiver = h.sched.process_mut(h.pids[p]).unwrap();
                    let delivery = h.reg.receive(&h.k, &channel, receiver).unwrap();
                    let expected = h.in_flight[p].pop_front();
                    match (delivery, expected) {
                        (None, None) => {}
                        (Some(d), Some(send_idx)) => {
                            // The kernel delivered exactly the message the
                            // harness expects, carrying its original stamp.
                            prop_assert_eq!(&d.sent_at, &h.stamps[send_idx]);
                            h.record(p, d.received_at, vec![send_idx]);
                        }
                        (d, e) => prop_assert!(false, "delivery {d:?} vs expected {e:?}"),
                    }
                }
                Op::Spawn { parent } => {
                    if n == MAX_PROCESSES {
                        continue;
                    }
                    let parent = parent % n;
                    let child = h.sched.spawn_child(&mut h.k, h.pids[parent], vec![]).unwrap();
                    let spawned_at = h.sched.process(h.pids[parent]).unwrap().clock();
                    let spawn_idx = h.record(parent, spawned_at, vec![]);
                    h.add_process(child);
                    let birth = h.sched.process(child).unwrap().clock();
                    h.record(n, birth, vec![spawn_idx]);
                }
            }
        }

        let reach = transitive_predecessors(&h.preds);
        let s = &h.stamps;
        for i in 0..s.len() {
            for j in 0..s.len() {
                if i == j {
                    continue;
                }
                let truly_before = reach[j][i];
                prop_assert_eq!(s[i].happened_before(&s[j]), truly_before, "events {} -> {}", i, j);
                let concurrent = !reach[i][j] && !reach[j][i];
                prop_assert_eq!(s[i].vector().concurrent_with(s[j].vector()), concurrent);
                if truly_before {
                    prop_assert!(s[i].lamport() < s[j].lamport(), "clock condition {} -> {}", i, j);
                }
            }
        }
    }
}
