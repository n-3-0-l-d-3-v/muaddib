//! THE KERNEL's foundation: unforgeable, revocable, attenuation-only
//! capabilities over opaque object references — no paths, no ambient
//! authority. See `docs/design/KERNEL.md` and
//! `docs/design/decisions/ADR-001-capability-model.md`.

mod rights;

use std::collections::HashMap;

pub use rights::Rights;

/// An opaque reference to a kernel object — never a path, never
/// meaningful to compare against anything but another `ObjectId`. Built
/// from a monotonic counter, not derived from wall-clock time (nothing
/// in this crate needs "when," only "which").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId(u64);

/// An unforgeable, possibly-attenuated handle to an object plus the
/// rights it grants. **Deliberately has no public constructor** — every
/// field is private, so the only way client code ever obtains one is
/// `Kernel::new_object` (a fresh object) or `Kernel::derive` (a
/// weaker view of one already held). This is what "unforgeable" means
/// here: it isn't a runtime check, it's a type the Rust compiler simply
/// will not let external code construct with values of its own choosing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Capability {
    object: ObjectId,
    rights: Rights,
    epoch: u64,
}

impl Capability {
    pub fn object(&self) -> ObjectId {
        self.object
    }

    pub fn rights(&self) -> Rights {
        self.rights
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CapError {
    #[error("object {0:?} does not exist (destroyed, or never allocated by this kernel)")]
    UnknownObject(ObjectId),
    #[error("capability for {0:?} was revoked (issued for an earlier epoch)")]
    Revoked(ObjectId),
    #[error("capability for {object:?} lacks required rights: has {held}, needs {required}")]
    InsufficientRights {
        object: ObjectId,
        held: Rights,
        required: Rights,
    },
    #[error("cannot derive a capability for {object:?} with rights {requested} from one that only holds {held} — derivation can only attenuate, never amplify")]
    CannotAmplify {
        object: ObjectId,
        held: Rights,
        requested: Rights,
    },
}

/// The capability table: every object this kernel instance has ever
/// allocated, and the epoch each is currently on. Revoking an object
/// bumps its epoch; every capability minted before the bump then fails
/// `check` — without the kernel ever needing to know who holds them.
#[derive(Debug, Default)]
pub struct Kernel {
    next_id: u64,
    epochs: HashMap<ObjectId, u64>,
}

impl Kernel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocates a fresh object and mints the first (necessarily
    /// maximal, for whatever rights the caller requests) capability for
    /// it. There is no notion of an object existing without at least one
    /// capability having been minted for it at creation.
    pub fn new_object(&mut self, rights: Rights) -> Capability {
        let object = ObjectId(self.next_id);
        self.next_id += 1;
        self.epochs.insert(object, 0);
        Capability {
            object,
            rights,
            epoch: 0,
        }
    }

    /// Validates that `cap` is still live (its object exists and hasn't
    /// been revoked past its epoch) and grants at least `required`.
    pub fn check(&self, cap: &Capability, required: Rights) -> Result<(), CapError> {
        let current_epoch = *self
            .epochs
            .get(&cap.object)
            .ok_or(CapError::UnknownObject(cap.object))?;
        if current_epoch != cap.epoch {
            return Err(CapError::Revoked(cap.object));
        }
        if !cap.rights.contains(required) {
            return Err(CapError::InsufficientRights {
                object: cap.object,
                held: cap.rights,
                required,
            });
        }
        Ok(())
    }

    /// Bumps the object's epoch, invalidating every capability minted
    /// before this call — including `cap` itself, which becomes just as
    /// unusable as any copy of it anyone else was holding, since none of
    /// them are tracked individually.
    ///
    /// `cap` must itself be **live** and hold `DESTROY`: revoking every
    /// outstanding capability for an object is the same class of power
    /// as destroying it. Before ticket 004 this checked only that the
    /// object existed — so a `NONE`-rights derived view could revoke its
    /// own owner, and an already-revoked capability could revoke again
    /// (see `docs/design/decisions/ADR-004-memory-ownership.md`).
    pub fn revoke(&mut self, cap: &Capability) -> Result<(), CapError> {
        self.check(cap, Rights::DESTROY)?;
        *self
            .epochs
            .get_mut(&cap.object)
            .expect("check just confirmed the object exists") += 1;
        Ok(())
    }

    /// Derives a new capability for the same object with `requested`
    /// rights — **strictly attenuation**: `cap` must currently be valid,
    /// must hold `GRANT`, and `requested` must be a subset of what `cap`
    /// already holds. A capability system where derivation could
    /// amplify rights wouldn't be a capability system; this is the
    /// property `docs/design/decisions/ADR-001-capability-model.md`'s
    /// property test exists to pin down for arbitrary rights
    /// combinations, not just the cases written here.
    pub fn derive(&self, cap: &Capability, requested: Rights) -> Result<Capability, CapError> {
        self.check(cap, Rights::GRANT)?;
        if !cap.rights.contains(requested) {
            return Err(CapError::CannotAmplify {
                object: cap.object,
                held: cap.rights,
                requested,
            });
        }
        Ok(Capability {
            object: cap.object,
            rights: requested,
            epoch: cap.epoch,
        })
    }

    /// Permanently removes the object from the table — every capability
    /// for it, including `cap`, now fails `check` with `UnknownObject`
    /// rather than `Revoked`. Requires `DESTROY`.
    pub fn destroy_object(&mut self, cap: &Capability) -> Result<(), CapError> {
        self.check(cap, Rights::DESTROY)?;
        self.epochs.remove(&cap.object);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_capability_passes_check_for_rights_it_holds() {
        let mut k = Kernel::new();
        let cap = k.new_object(Rights::READ | Rights::WRITE);
        assert_eq!(k.check(&cap, Rights::READ), Ok(()));
        assert_eq!(k.check(&cap, Rights::READ | Rights::WRITE), Ok(()));
    }

    #[test]
    fn check_fails_for_rights_not_held() {
        let mut k = Kernel::new();
        let cap = k.new_object(Rights::READ);
        assert_eq!(
            k.check(&cap, Rights::WRITE),
            Err(CapError::InsufficientRights {
                object: cap.object(),
                held: Rights::READ,
                required: Rights::WRITE,
            })
        );
    }

    #[test]
    fn checking_a_capability_for_an_unknown_object_fails_distinctly() {
        let k1 = Kernel::new();
        let mut k2 = Kernel::new();
        let cap_from_other_kernel = k2.new_object(Rights::READ);
        assert_eq!(
            k1.check(&cap_from_other_kernel, Rights::READ),
            Err(CapError::UnknownObject(cap_from_other_kernel.object()))
        );
    }

    #[test]
    fn revoke_invalidates_every_capability_for_that_object() {
        let mut k = Kernel::new();
        let cap = k.new_object(Rights::READ | Rights::DESTROY);
        let cap_copy = cap; // Capability is Copy — this models "another holder's copy"
        k.revoke(&cap).unwrap();
        assert_eq!(
            k.check(&cap, Rights::READ),
            Err(CapError::Revoked(cap.object()))
        );
        assert_eq!(
            k.check(&cap_copy, Rights::READ),
            Err(CapError::Revoked(cap.object()))
        );
    }

    #[test]
    fn revoke_does_not_affect_other_objects() {
        let mut k = Kernel::new();
        let cap_a = k.new_object(Rights::READ | Rights::DESTROY);
        let cap_b = k.new_object(Rights::READ);
        k.revoke(&cap_a).unwrap();
        assert_eq!(k.check(&cap_b, Rights::READ), Ok(()));
    }

    #[test]
    fn derive_produces_a_capability_with_exactly_the_requested_subset() {
        let mut k = Kernel::new();
        let cap = k.new_object(Rights::READ | Rights::WRITE | Rights::GRANT);
        let derived = k.derive(&cap, Rights::READ).unwrap();
        assert_eq!(derived.rights(), Rights::READ);
        assert_eq!(derived.object(), cap.object());
    }

    #[test]
    fn derive_without_grant_is_rejected() {
        let mut k = Kernel::new();
        let cap = k.new_object(Rights::READ | Rights::WRITE);
        assert_eq!(
            k.derive(&cap, Rights::READ),
            Err(CapError::InsufficientRights {
                object: cap.object(),
                held: Rights::READ | Rights::WRITE,
                required: Rights::GRANT,
            })
        );
    }

    #[test]
    fn derive_cannot_amplify_rights() {
        let mut k = Kernel::new();
        let cap = k.new_object(Rights::READ | Rights::GRANT);
        assert_eq!(
            k.derive(&cap, Rights::READ | Rights::WRITE),
            Err(CapError::CannotAmplify {
                object: cap.object(),
                held: Rights::READ | Rights::GRANT,
                requested: Rights::READ | Rights::WRITE,
            })
        );
    }

    #[test]
    fn derive_from_a_revoked_capability_fails() {
        let mut k = Kernel::new();
        let cap = k.new_object(Rights::READ | Rights::GRANT | Rights::DESTROY);
        k.revoke(&cap).unwrap();
        assert_eq!(
            k.derive(&cap, Rights::READ),
            Err(CapError::Revoked(cap.object()))
        );
    }

    #[test]
    fn revoke_requires_destroy_right() {
        let mut k = Kernel::new();
        let cap = k.new_object(Rights::READ | Rights::GRANT);
        assert_eq!(
            k.revoke(&cap),
            Err(CapError::InsufficientRights {
                object: cap.object(),
                held: Rights::READ | Rights::GRANT,
                required: Rights::DESTROY,
            })
        );
        assert_eq!(k.check(&cap, Rights::READ), Ok(()));
    }

    #[test]
    fn a_weaker_derived_view_cannot_revoke_its_owner() {
        let mut k = Kernel::new();
        let owner = k.new_object(Rights::ALL);
        let view = k.derive(&owner, Rights::READ).unwrap();
        assert!(k.revoke(&view).is_err());
        assert_eq!(k.check(&owner, Rights::ALL), Ok(()));
    }

    #[test]
    fn a_revoked_capability_cannot_revoke_again() {
        let mut k = Kernel::new();
        let old = k.new_object(Rights::ALL);
        k.revoke(&old).unwrap();
        // A second revoke with the stale capability must not be able to
        // bump the epoch again (which would kill any capability issued
        // at the new epoch — exactly how an old owner could sabotage a
        // new one after an ownership transfer).
        assert_eq!(k.revoke(&old), Err(CapError::Revoked(old.object())));
    }

    #[test]
    fn destroy_requires_destroy_right() {
        let mut k = Kernel::new();
        let cap = k.new_object(Rights::READ);
        assert_eq!(
            k.destroy_object(&cap),
            Err(CapError::InsufficientRights {
                object: cap.object(),
                held: Rights::READ,
                required: Rights::DESTROY,
            })
        );
    }

    #[test]
    fn destroy_makes_every_capability_fail_with_unknown_object_not_revoked() {
        let mut k = Kernel::new();
        let cap = k.new_object(Rights::DESTROY | Rights::READ);
        let cap_copy = cap;
        k.destroy_object(&cap).unwrap();
        assert_eq!(
            k.check(&cap, Rights::READ),
            Err(CapError::UnknownObject(cap.object()))
        );
        assert_eq!(
            k.check(&cap_copy, Rights::READ),
            Err(CapError::UnknownObject(cap.object()))
        );
    }

    #[test]
    fn distinct_objects_get_distinct_ids() {
        let mut k = Kernel::new();
        let a = k.new_object(Rights::NONE);
        let b = k.new_object(Rights::NONE);
        assert_ne!(a.object(), b.object());
    }
}
