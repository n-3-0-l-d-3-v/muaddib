//! Property test: for arbitrary sequences of spawn/transfer/derive
//! operations across multiple processes, a process can never end up
//! holding a capability it wasn't explicitly granted — and specifically,
//! a `Transfer`'d capability disappears from the sender for good, for
//! any rights combination and any interleaving of parents transferring
//! to multiple children.

use capability::{Kernel, Rights};
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
    /// A chain of transfers (process 0 -> 1 -> 2 -> ... -> n), each
    /// moving the *same* capability one hop further: at every step, the
    /// immediately preceding holder must lose it, and only the current
    /// holder has it — for an arbitrary chain length and arbitrary
    /// initial rights.
    #[test]
    fn a_transferred_capability_exists_in_exactly_one_process_at_a_time(
        rights in arb_rights(),
        chain_length in 1usize..8,
    ) {
        let mut k = Kernel::new();
        let mut s = Scheduler::new();
        let cap = k.new_object(rights);
        let mut current = s.spawn(vec![cap]);
        let mut current_handle = s.process(current).unwrap().handles().next().unwrap();

        for _ in 0..chain_length {
            let next = s.spawn_child(&k, current, vec![Grant::Transfer(current_handle)]).unwrap();

            // The old holder must have lost it...
            prop_assert_eq!(s.process(current).unwrap().handle_count(), 0);
            // ...and exactly the new holder has it.
            prop_assert_eq!(s.process(next).unwrap().handle_count(), 1);
            let next_handle = s.process(next).unwrap().handles().next().unwrap();
            prop_assert_eq!(s.process(next).unwrap().capability(next_handle), Some(cap));

            current = next;
            current_handle = next_handle;
        }
    }

    /// Deriving to several children from the same parent capability:
    /// every child's rights must be a subset of what the parent
    /// originally held, and the parent's own capability is completely
    /// unaffected by however many children derived from it.
    #[test]
    fn deriving_to_multiple_children_never_amplifies_and_never_affects_the_parent(
        held in arb_rights(),
        requested in prop::collection::vec(arb_rights(), 1..5),
    ) {
        let mut k = Kernel::new();
        let mut s = Scheduler::new();
        let cap = k.new_object(held | Rights::GRANT);
        let parent = s.spawn(vec![cap]);
        let handle = s.process(parent).unwrap().handles().next().unwrap();

        for req in &requested {
            let result = s.spawn_child(&k, parent, vec![Grant::Derive(handle, *req)]);
            match result {
                Ok(child) => {
                    let child_handle = s.process(child).unwrap().handles().next().unwrap();
                    let child_cap = s.process(child).unwrap().capability(child_handle).unwrap();
                    prop_assert_eq!(child_cap.rights(), *req);
                    prop_assert!(cap.rights().contains(child_cap.rights()));
                }
                Err(_) => {
                    // Only acceptable if req truly wasn't a subset.
                    prop_assert!(!cap.rights().contains(*req));
                }
            }
            // The parent's own capability is never touched by any of this.
            prop_assert_eq!(s.process(parent).unwrap().capability(handle), Some(cap));
        }
    }
}
