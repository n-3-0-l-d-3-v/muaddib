//! The one place a batch of capabilities ever moves out of a process's
//! table — shared by `Scheduler::spawn_child` (destination: a new
//! child's table) and, once ticket 003 (`ipc`) depends on this crate,
//! channel sends (destination: a message payload). Spawning a child
//! with attenuated authority and sending a capability over a channel
//! are the same operation shape: resolve a batch of `Transfer`/`Derive`/
//! `Move` requests against a source process's table, all-or-nothing,
//! then hand the results to wherever they're going. Factoring this out
//! once ticket 003 needed the identical logic a second time avoids the
//! two copies silently drifting apart, per this project's usual "found
//! this while building the next thing" honesty.

use std::collections::HashSet;

use capability::{CapError, Capability, Kernel, ObjectId, Rights};

use crate::process::{Handle, Process};

/// One capability to move out of a source process's table.
#[derive(Debug, Clone, Copy)]
pub enum Grant {
    /// Move the capability at `Handle` out entirely — the source's own
    /// handle stops resolving to anything afterward. **Not** kernel-
    /// enforced: `Capability` is `Copy`, so a source that read the value
    /// out before transferring still holds a working copy. Right for
    /// shared objects (channel endpoints) whose other holders must keep
    /// working; wrong for exclusive ownership — use `Move` for that.
    Transfer(Handle),
    /// Resolve a `Kernel::derive`d, strictly-weaker-or-equal capability;
    /// the source keeps using its own unaffected original.
    Derive(Handle, Rights),
    /// **Kernel-enforced exclusive ownership transfer** (ticket 004):
    /// like `Transfer`, and additionally `Kernel::reissue`s the object, so
    /// every other outstanding capability for it — copies the source
    /// kept, views it derived, capabilities still in flight in some
    /// channel — fails every future `check`. Only the capability this
    /// grant delivers works afterward. Requires the capability be live
    /// and hold `DESTROY` (the authority `Kernel::revoke` requires).
    Move(Handle),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GrantError {
    #[error("handle does not exist in the source process's capability table")]
    UnknownHandle,
    #[error("handle {0:?} is transferred more than once in the same batch")]
    DuplicateTransfer(Handle),
    #[error("object {0:?} is moved in this batch but also named by another grant, whose capability the move would revoke on arrival")]
    MoveConflict(ObjectId),
    #[error(transparent)]
    Capability(#[from] CapError),
}

/// Validates every `Grant` in `grants` against `source`'s table —
/// **without mutating anything** — and returns the resolved
/// capabilities in order if every single one would succeed. Callers
/// apply the side effects via `apply_grants` only once they know the
/// whole batch is going somewhere real (a new child process, a channel
/// message), so a batch that partially fails never leaves `source`
/// stripped of a capability — or an object's other holders revoked —
/// for nothing.
///
/// A `Move`'s resolved entry is the source's *current* capability: a
/// placeholder `apply_grants` replaces with the reissued one.
pub fn resolve_grants(
    kernel: &Kernel,
    source: &Process,
    grants: &[Grant],
) -> Result<Vec<Capability>, GrantError> {
    let mut resolved = Vec::with_capacity(grants.len());
    let mut transferred = HashSet::new();
    for g in grants {
        // A handle can only leave the table once: `apply_grants` removes
        // it once, so resolving it twice would hand the destination two
        // copies of a capability the source only gave up one of.
        if let Grant::Transfer(h) | Grant::Move(h) = *g {
            if !transferred.insert(h) {
                return Err(GrantError::DuplicateTransfer(h));
            }
        }
        let cap = match *g {
            Grant::Transfer(h) => source.capability(h).ok_or(GrantError::UnknownHandle)?,
            Grant::Derive(h, rights) => {
                let held = source.capability(h).ok_or(GrantError::UnknownHandle)?;
                kernel.derive(&held, rights)?
            }
            Grant::Move(h) => {
                let held = source.capability(h).ok_or(GrantError::UnknownHandle)?;
                kernel.check(&held, Rights::DESTROY)?;
                held
            }
        };
        resolved.push(cap);
    }

    // A moved object's reissue revokes every other capability for it, so
    // any other grant in the same batch naming that object (through any
    // handle) would deliver a capability that is dead on arrival. Reject
    // the batch rather than silently deliver it.
    for (i, g) in grants.iter().enumerate() {
        if let Grant::Move(_) = g {
            let object = resolved[i].object();
            let conflicts = resolved
                .iter()
                .enumerate()
                .any(|(j, cap)| j != i && cap.object() == object);
            if conflicts {
                return Err(GrantError::MoveConflict(object));
            }
        }
    }
    Ok(resolved)
}

/// Performs the side effects of a batch `resolve_grants` already
/// validated: removes every `Transfer`'d/`Move`'d handle from `source`,
/// and reissues every `Move`'d object, returning the capabilities to
/// deliver (each `Move` placeholder replaced by its fresh capability).
/// Callers must pass exactly the `grants` and `resolved` from a
/// successful `resolve_grants`, with no kernel mutation in between —
/// this half never re-validates.
pub fn apply_grants(
    kernel: &mut Kernel,
    source: &mut Process,
    grants: &[Grant],
    mut resolved: Vec<Capability>,
) -> Vec<Capability> {
    for (g, cap) in grants.iter().zip(resolved.iter_mut()) {
        match *g {
            Grant::Transfer(h) => {
                source.take(h);
            }
            Grant::Move(h) => {
                source.take(h);
                *cap = kernel
                    .reissue(cap)
                    .expect("resolve_grants checked this capability is live and holds DESTROY");
            }
            Grant::Derive(..) => {}
        }
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::ProcessId;
    use capability::Kernel;

    #[test]
    fn resolve_grants_never_mutates_the_source() {
        let mut k = Kernel::new();
        let mut p = Process::new(ProcessId(0));
        let cap = k.new_object(Rights::READ);
        let handle = p.grant(cap);

        let resolved = resolve_grants(&k, &p, &[Grant::Transfer(handle)]).unwrap();
        assert_eq!(resolved, vec![cap]);
        // Still there — resolve_grants only validated and read.
        assert_eq!(p.capability(handle), Some(cap));
    }

    #[test]
    fn apply_grants_removes_only_the_transfer_variants() {
        let mut k = Kernel::new();
        let mut p = Process::new(ProcessId(0));
        let cap_a = k.new_object(Rights::READ | Rights::GRANT);
        let cap_b = k.new_object(Rights::WRITE | Rights::GRANT);
        let handle_a = p.grant(cap_a);
        let handle_b = p.grant(cap_b);

        let grants = [
            Grant::Transfer(handle_a),
            Grant::Derive(handle_b, Rights::WRITE),
        ];
        let resolved = resolve_grants(&k, &p, &grants).unwrap();
        apply_grants(&mut k, &mut p, &grants, resolved);

        assert_eq!(p.capability(handle_a), None); // transferred away
        assert_eq!(p.capability(handle_b), Some(cap_b)); // derive doesn't touch the source
    }

    #[test]
    fn transferring_the_same_handle_twice_in_one_batch_is_rejected() {
        let mut k = Kernel::new();
        let mut p = Process::new(ProcessId(0));
        let handle = p.grant(k.new_object(Rights::READ));

        let result = resolve_grants(&k, &p, &[Grant::Transfer(handle), Grant::Transfer(handle)]);
        assert_eq!(result, Err(GrantError::DuplicateTransfer(handle)));
    }

    #[test]
    fn deriving_twice_from_one_handle_is_still_allowed() {
        let mut k = Kernel::new();
        let mut p = Process::new(ProcessId(0));
        let handle = p.grant(k.new_object(Rights::READ | Rights::WRITE | Rights::GRANT));

        let resolved = resolve_grants(
            &k,
            &p,
            &[
                Grant::Derive(handle, Rights::READ),
                Grant::Derive(handle, Rights::WRITE),
            ],
        )
        .unwrap();
        assert_eq!(resolved.len(), 2);
    }

    #[test]
    fn a_move_revokes_every_copy_the_source_kept() {
        let mut k = Kernel::new();
        let mut p = Process::new(ProcessId(0));
        let owner = k.new_object(Rights::ALL);
        let handle = p.grant(owner);
        let stashed = p.capability(handle).unwrap();

        let grants = [Grant::Move(handle)];
        let resolved = resolve_grants(&k, &p, &grants).unwrap();
        // Validation alone revokes nothing.
        assert_eq!(k.check(&stashed, Rights::ALL), Ok(()));
        let delivered = apply_grants(&mut k, &mut p, &grants, resolved);

        assert_eq!(p.capability(handle), None);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].rights(), Rights::ALL);
        assert_eq!(k.check(&delivered[0], Rights::ALL), Ok(()));
        assert_eq!(
            k.check(&stashed, Rights::NONE),
            Err(CapError::Revoked(owner.object()))
        );
    }

    #[test]
    fn a_move_without_destroy_is_rejected_and_nothing_changes() {
        let mut k = Kernel::new();
        let mut p = Process::new(ProcessId(0));
        let cap = k.new_object(Rights::READ | Rights::WRITE);
        let handle = p.grant(cap);

        let result = resolve_grants(&k, &p, &[Grant::Move(handle)]);
        assert!(matches!(
            result,
            Err(GrantError::Capability(CapError::InsufficientRights { .. }))
        ));
        assert_eq!(p.capability(handle), Some(cap));
        assert_eq!(k.check(&cap, Rights::READ | Rights::WRITE), Ok(()));
    }

    #[test]
    fn moving_an_object_also_named_elsewhere_in_the_batch_is_rejected() {
        let mut k = Kernel::new();
        let mut p = Process::new(ProcessId(0));
        let owner = k.new_object(Rights::ALL);
        let view = k.derive(&owner, Rights::READ).unwrap();
        let owner_handle = p.grant(owner);
        let view_handle = p.grant(view);

        for grants in [
            vec![Grant::Move(owner_handle), Grant::Transfer(view_handle)],
            vec![
                Grant::Derive(owner_handle, Rights::READ),
                Grant::Move(owner_handle),
            ],
        ] {
            assert_eq!(
                resolve_grants(&k, &p, &grants),
                Err(GrantError::MoveConflict(owner.object()))
            );
        }
        assert_eq!(
            resolve_grants(
                &k,
                &p,
                &[Grant::Move(owner_handle), Grant::Move(owner_handle)]
            ),
            Err(GrantError::DuplicateTransfer(owner_handle))
        );
    }

    #[test]
    fn resolve_grants_fails_on_an_unknown_handle_without_touching_the_source() {
        let mut k = Kernel::new();
        let mut p = Process::new(ProcessId(0));
        let cap = k.new_object(Rights::READ);
        let handle = p.grant(cap);
        let bogus = Handle(9999);

        let result = resolve_grants(&k, &p, &[Grant::Transfer(handle), Grant::Transfer(bogus)]);
        assert_eq!(result, Err(GrantError::UnknownHandle));
        assert_eq!(p.capability(handle), Some(cap));
    }
}
