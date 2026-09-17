//! Property tests for the two security guarantees a capability system
//! actually exists to make: revocation invalidates every outstanding
//! capability for an object, and derivation can never amplify rights —
//! proven for arbitrary rights combinations, not just the hand-picked
//! cases in `src/lib.rs`'s unit tests.

use capability::{CapError, Kernel, Rights};
use proptest::prelude::*;

/// Every non-empty subset of the five defined rights bits, so
/// arbitrary-rights property tests exercise real combinations rather
/// than only `Rights::ALL`/`Rights::NONE`.
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
    /// A capability that passed `check` for some required rights before
    /// `revoke` must fail `check` for those exact same rights after —
    /// for any rights the object was created with and any subset
    /// checked.
    #[test]
    fn revocation_invalidates_every_outstanding_capability(
        created in arb_rights(),
        required in arb_rights(),
    ) {
        let mut k = Kernel::new();
        let cap = k.new_object(created | Rights::DESTROY); // revoke requires DESTROY
        let was_valid = k.check(&cap, required).is_ok();

        k.revoke(&cap).unwrap();
        let result = k.check(&cap, required);

        if was_valid {
            prop_assert_eq!(result, Err(CapError::Revoked(cap.object())));
        } else {
            // Already invalid before revocation (missing rights) — must
            // stay invalid, just not necessarily for the same reason.
            prop_assert!(result.is_err());
        }
    }

    /// A second capability for a *different* object is never affected
    /// by revoking the first — revocation is per-object, not global.
    #[test]
    fn revocation_does_not_leak_to_other_objects(rights in arb_rights()) {
        let mut k = Kernel::new();
        let cap_a = k.new_object(rights | Rights::DESTROY);
        let cap_b = k.new_object(rights);
        k.revoke(&cap_a).unwrap();
        prop_assert_eq!(k.check(&cap_b, rights), Ok(()));
    }

    /// `derive`'s whole reason to exist: for arbitrary held rights and
    /// an arbitrary requested subset, either the derivation succeeds and
    /// the result's rights are *exactly* what was requested (never
    /// more), or it's rejected — it can never silently grant more than
    /// asked for, and it can never grant something the parent didn't
    /// have.
    #[test]
    fn derivation_never_amplifies_rights(
        held in arb_rights(),
        requested in arb_rights(),
    ) {
        let mut k = Kernel::new();
        let cap = k.new_object(held | Rights::GRANT); // ensure derive is at least reachable
        let result = k.derive(&cap, requested);

        match result {
            Ok(derived) => {
                prop_assert_eq!(derived.rights(), requested);
                // Every bit the derived capability holds must also have
                // been held by the parent.
                prop_assert!(cap.rights().contains(derived.rights()));
            }
            Err(CapError::CannotAmplify { .. }) => {
                // Correctly rejected: requested must not have been a
                // subset of what the parent held.
                prop_assert!(!cap.rights().contains(requested));
            }
            Err(other) => prop_assert!(false, "unexpected error: {other}"),
        }
    }

    /// A capability derived from another derived capability still can
    /// never exceed the *original* object's very first minted rights —
    /// attenuation composes, it never un-attenuates.
    #[test]
    fn attenuation_composes_across_multiple_derivations(
        original in arb_rights(),
        first_step in arb_rights(),
        second_step in arb_rights(),
    ) {
        let mut k = Kernel::new();
        let root = k.new_object(original | Rights::GRANT);

        let Ok(first) = k.derive(&root, first_step) else {
            return Ok(()); // first_step wasn't a subset of root's rights; nothing to compose
        };
        let Ok(second) = k.derive(&first, second_step) else {
            return Ok(()); // second_step wasn't a subset of first's rights; correctly rejected
        };

        prop_assert!(root.rights().contains(second.rights()));
        prop_assert!(first.rights().contains(second.rights()));
    }

    /// Revocation authority can't be forged by attenuation: for any
    /// derived view, `revoke` succeeds iff the view holds `DESTROY`, and a
    /// rejected attempt leaves the owner's capability fully usable.
    #[test]
    fn only_a_live_destroy_holding_capability_can_revoke(
        owner_rights in arb_rights(),
        view_rights in arb_rights(),
    ) {
        let mut k = Kernel::new();
        let owner = k.new_object(owner_rights | Rights::GRANT);
        let Ok(view) = k.derive(&owner, view_rights) else {
            return Ok(());
        };
        let result = k.revoke(&view);
        if view_rights.contains(Rights::DESTROY) {
            prop_assert_eq!(result, Ok(()));
            // Now stale: can never revoke a second time.
            prop_assert_eq!(k.revoke(&view), Err(CapError::Revoked(view.object())));
        } else {
            prop_assert!(result.is_err());
            prop_assert_eq!(k.check(&owner, owner.rights()), Ok(()));
        }
    }
}
