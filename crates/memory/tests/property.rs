//! Property tests for ticket 004's guarantee: after a region's ownership
//! moves, the old owner can never successfully read or write it — for
//! arbitrary sequences of moves, view-sharing, deliveries and access
//! attempts. The adversary is worst-case: every capability any process
//! ever held is kept forever (`Capability` is `Copy`, so a real process
//! could do exactly this) and replayed against the region at random.
//!
//! Checked differentially against a trivially-correct model: a
//! "generation" counter bumped on every move. A capability is live iff it
//! was issued in the current generation; the region's bytes are a plain
//! array updated only by accesses the model says must succeed.

use std::collections::VecDeque;

use capability::{CapError, Capability, Kernel, Rights};
use ipc::ChannelRegistry;
use memory::{MemoryError, RegionRegistry};
use process::{Grant, Handle, ProcessId, Scheduler};
use proptest::prelude::*;

const REGION_SIZE: usize = 8;
const PROCESSES: usize = 4;

#[derive(Debug, Clone)]
enum Op {
    /// The current owner moves the region to process `to` over IPC.
    Move { to: usize },
    /// The current owner sends a derived view (queued, not yet received).
    ShareView { to: usize, write: bool },
    /// The oldest queued view is received by its recipient.
    Deliver,
    /// Replay some stashed capability as a read.
    Read {
        pick: usize,
        offset: usize,
        len: usize,
    },
    /// Replay some stashed capability as a write.
    Write {
        pick: usize,
        offset: usize,
        byte: u8,
        len: usize,
    },
    /// A stale capability tries to revoke (sabotage the new owner).
    RevokeWithStale { pick: usize },
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..PROCESSES).prop_map(|to| Op::Move { to }),
        (0..PROCESSES, any::<bool>()).prop_map(|(to, write)| Op::ShareView { to, write }),
        Just(Op::Deliver),
        (any::<usize>(), 0..REGION_SIZE + 3, 0..5usize).prop_map(|(pick, offset, len)| Op::Read {
            pick,
            offset,
            len
        }),
        (any::<usize>(), 0..REGION_SIZE + 3, any::<u8>(), 0..5usize).prop_map(
            |(pick, offset, byte, len)| Op::Write {
                pick,
                offset,
                byte,
                len
            }
        ),
        any::<usize>().prop_map(|pick| Op::RevokeWithStale { pick }),
    ]
}

struct Stashed {
    cap: Capability,
    generation: u64,
}

/// The newest handle in a process's table — the one `receive` just granted.
fn newest_handle(sched: &Scheduler, pid: ProcessId) -> Handle {
    sched.process(pid).unwrap().handles().max().unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn only_capabilities_issued_since_the_last_move_can_touch_the_region(
        ops in prop::collection::vec(arb_op(), 1..60),
    ) {
        let mut k = Kernel::new();
        let mut mem = RegionRegistry::new();
        let mut channels = ChannelRegistry::new();
        let mut sched = Scheduler::new();

        let region = mem.new_region(&mut k, REGION_SIZE);
        let pids: Vec<ProcessId> = (0..PROCESSES).map(|_| sched.spawn(vec![])).collect();
        sched.process_mut(pids[0]).unwrap().grant(region);
        let handoff = channels.new_channel(&mut k);
        let views = channels.new_channel(&mut k);

        // Model.
        let mut generation = 0u64;
        let mut owner = 0usize;
        let mut owner_handle = newest_handle(&sched, pids[0]);
        let mut bytes = [0u8; REGION_SIZE];
        let mut pending: VecDeque<(usize, u64)> = VecDeque::new();
        let mut stash = vec![Stashed { cap: region, generation }];

        for op in ops {
            match op {
                Op::Move { to } => {
                    let sender = sched.process_mut(pids[owner]).unwrap();
                    channels
                        .send(&mut k, &handoff, sender, vec![Grant::Move(owner_handle)], 0)
                        .unwrap();
                    let receiver = sched.process_mut(pids[to]).unwrap();
                    channels.receive(&k, &handoff, receiver).unwrap();
                    generation += 1;
                    owner = to;
                    owner_handle = newest_handle(&sched, pids[to]);
                    let cap = sched.process(pids[to]).unwrap().capability(owner_handle).unwrap();
                    stash.push(Stashed { cap, generation });
                }
                Op::ShareView { to, write } => {
                    let rights = if write { Rights::READ | Rights::WRITE } else { Rights::READ };
                    let sender = sched.process_mut(pids[owner]).unwrap();
                    channels
                        .send(&mut k, &views, sender, vec![Grant::Derive(owner_handle, rights)], 0)
                        .unwrap();
                    pending.push_back((to, generation));
                }
                Op::Deliver => {
                    let Some((to, issued_in)) = pending.pop_front() else { continue };
                    let receiver = sched.process_mut(pids[to]).unwrap();
                    channels.receive(&k, &views, receiver).unwrap();
                    let handle = newest_handle(&sched, pids[to]);
                    let cap = sched.process(pids[to]).unwrap().capability(handle).unwrap();
                    stash.push(Stashed { cap, generation: issued_in });
                }
                Op::Read { pick, offset, len } => {
                    let s = &stash[pick % stash.len()];
                    let live = s.generation == generation;
                    let in_bounds = offset + len <= REGION_SIZE;
                    let result = mem.read(&k, &s.cap, offset, len);
                    if !live {
                        prop_assert_eq!(
                            result,
                            Err(MemoryError::Capability(CapError::Revoked(region.object())))
                        );
                    } else if in_bounds {
                        prop_assert_eq!(result, Ok(bytes[offset..offset + len].to_vec()));
                    } else {
                        let is_out_of_bounds = matches!(result, Err(MemoryError::OutOfBounds { .. }));
                        prop_assert!(is_out_of_bounds);
                    }
                }
                Op::Write { pick, offset, byte, len } => {
                    let s = &stash[pick % stash.len()];
                    let live = s.generation == generation;
                    let may_write = s.cap.rights().contains(Rights::WRITE);
                    let in_bounds = offset + len <= REGION_SIZE;
                    let data = vec![byte; len];
                    let result = mem.write(&k, &s.cap, offset, &data);
                    if !live {
                        prop_assert_eq!(
                            result,
                            Err(MemoryError::Capability(CapError::Revoked(region.object())))
                        );
                    } else if !may_write {
                        let is_rights_error = matches!(
                            result,
                            Err(MemoryError::Capability(CapError::InsufficientRights { .. }))
                        );
                        prop_assert!(is_rights_error);
                    } else if in_bounds {
                        prop_assert_eq!(result, Ok(()));
                        bytes[offset..offset + len].copy_from_slice(&data);
                    } else {
                        let is_out_of_bounds = matches!(result, Err(MemoryError::OutOfBounds { .. }));
                        prop_assert!(is_out_of_bounds);
                    }
                }
                Op::RevokeWithStale { pick } => {
                    let s = &stash[pick % stash.len()];
                    if s.generation != generation {
                        prop_assert_eq!(k.revoke(&s.cap), Err(CapError::Revoked(region.object())));
                    }
                }
            }

            // After every step, liveness of *every* capability ever seen
            // agrees with the model, and the region's bytes match it.
            for s in &stash {
                prop_assert_eq!(k.check(&s.cap, Rights::NONE).is_ok(), s.generation == generation);
            }
            let owner_cap = sched.process(pids[owner]).unwrap().capability(owner_handle).unwrap();
            prop_assert_eq!(mem.read(&k, &owner_cap, 0, REGION_SIZE), Ok(bytes.to_vec()));
        }
    }

    /// Ownership moved down a chain of spawned children, every ancestor
    /// keeping a copy of what it held: only the last child can write, and
    /// what it reads is whatever the most recent *legitimate* owner wrote.
    #[test]
    fn along_a_spawn_chain_only_the_last_owner_holds_authority(
        chain_length in 1usize..10,
        payloads in prop::collection::vec(any::<u8>(), 10),
    ) {
        let mut k = Kernel::new();
        let mut mem = RegionRegistry::new();
        let mut sched = Scheduler::new();

        let region = mem.new_region(&mut k, 1);
        let mut current = sched.spawn(vec![region]);
        let mut kept: Vec<Capability> = vec![region];

        for payload in payloads.iter().take(chain_length) {
            let proc = sched.process(current).unwrap();
            let handle = proc.handles().next().unwrap();
            let cap = proc.capability(handle).unwrap();
            mem.write(&k, &cap, 0, &[*payload]).unwrap();

            current = sched.spawn_child(&mut k, current, vec![Grant::Move(handle)]).unwrap();

            for old in &kept {
                prop_assert_eq!(
                    mem.write(&k, old, 0, &[payload.wrapping_add(1)]),
                    Err(MemoryError::Capability(CapError::Revoked(region.object())))
                );
            }
            let proc = sched.process(current).unwrap();
            let new_cap = proc.capability(proc.handles().next().unwrap()).unwrap();
            prop_assert_eq!(mem.read(&k, &new_cap, 0, 1), Ok(vec![*payload]));
            kept.push(new_cap);
        }
    }
}
