//! Property test: for arbitrary rights combinations and arbitrary
//! chains of channel sends, a capability received over IPC can never
//! grant more authority than the sender actually held at send time —
//! the specific guarantee ticket 003 exists to prove, not just assert.

use capability::{Kernel, Rights};
use ipc::ChannelRegistry;
use process::{Grant, Scheduler};
use proptest::prelude::*;

fn arb_rights() -> impl Strategy<Value = Rights> {
    let bits = [
        Rights::READ,
        Rights::WRITE,
        Rights::EXECUTE,
        Rights::GRANT,
        Rights::DESTROY,
    ];
    prop::collection::vec(prop::bool::ANY, bits.len()).prop_map(move |flags| {
        flags
            .iter()
            .zip(bits.iter())
            .filter(|(on, _)| **on)
            .fold(Rights::NONE, |acc, (_, r)| acc | *r)
    })
}

proptest! {
    /// A capability derived and sent over a channel, for arbitrary held
    /// rights and an arbitrary requested subset: the receiver's copy
    /// either has exactly the requested rights (a subset of what the
    /// sender held) or the send is rejected outright — it can never
    /// silently arrive with more authority than was requested, and the
    /// request can never exceed what the sender actually had.
    #[test]
    fn a_received_derived_capability_never_exceeds_what_the_sender_held(
        held in arb_rights(),
        requested in arb_rights(),
    ) {
        let mut k = Kernel::new();
        let mut reg = ChannelRegistry::new();
        let mut sched = Scheduler::new();

        let payload = k.new_object(held | Rights::GRANT);
        let sender_id = sched.spawn(vec![payload]);
        let receiver_id = sched.spawn(vec![]);
        let channel = reg.new_channel(&mut k);
        let handle = sched.process(sender_id).unwrap().handles().next().unwrap();

        let send_result = {
            let sender = sched.process_mut(sender_id).unwrap();
            reg.send(&k, &channel, sender, vec![Grant::Derive(handle, requested)], 0)
        };

        match send_result {
            Ok(()) => {
                let receiver = sched.process_mut(receiver_id).unwrap();
                reg.receive(&k, &channel, receiver).unwrap();
                let received_handle = receiver.handles().next().unwrap();
                let received = receiver.capability(received_handle).unwrap();
                prop_assert_eq!(received.rights(), requested);
                prop_assert!(payload.rights().contains(received.rights()));
            }
            Err(_) => {
                // Only acceptable if requested truly wasn't a subset of
                // what the sender held.
                prop_assert!(!payload.rights().contains(requested));
                prop_assert_eq!(reg.queue_len(&channel), Some(0));
            }
        }
    }

    /// A chain of transfers over several channels (process 0 sends to
    /// process 1's channel, 1 receives then sends to 2's channel, ...):
    /// at every hop, the capability exists in exactly one place — either
    /// one process's table, or in flight in exactly one queue — never
    /// duplicated, never lost.
    #[test]
    fn a_capability_relayed_through_a_chain_of_channels_is_never_duplicated_or_lost(
        rights in arb_rights(),
        hops in 1usize..6,
    ) {
        let mut k = Kernel::new();
        let mut reg = ChannelRegistry::new();
        let mut sched = Scheduler::new();

        let payload = k.new_object(rights);
        let mut holder = sched.spawn(vec![payload]);

        for _ in 0..hops {
            let next_holder = sched.spawn(vec![]);
            let channel = reg.new_channel(&mut k);
            let handle = sched.process(holder).unwrap().handles().next().unwrap();

            {
                let sender = sched.process_mut(holder).unwrap();
                reg.send(&k, &channel, sender, vec![Grant::Transfer(handle)], 0).unwrap();
            }
            prop_assert_eq!(sched.process(holder).unwrap().handle_count(), 0);
            prop_assert_eq!(reg.queue_len(&channel), Some(1));

            let receiver = sched.process_mut(next_holder).unwrap();
            reg.receive(&k, &channel, receiver).unwrap();
            prop_assert_eq!(receiver.handle_count(), 1);
            let received_handle = receiver.handles().next().unwrap();
            prop_assert_eq!(receiver.capability(received_handle), Some(payload));

            holder = next_holder;
        }
    }
}
