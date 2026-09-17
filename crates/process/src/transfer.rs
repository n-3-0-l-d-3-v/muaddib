//! The one place a batch of capabilities ever moves out of a process's
//! table — shared by `Scheduler::spawn_child` (destination: a new
//! child's table) and, once ticket 003 (`ipc`) depends on this crate,
//! channel sends (destination: a message payload). Spawning a child
//! with attenuated authority and sending a capability over a channel
//! are the same operation shape: resolve a batch of `Transfer`/`Derive`
//! requests against a source process's table, all-or-nothing, then hand
//! the results to wherever they're going. Factoring this out once ticket
//! 003 needed the identical logic a second time avoids the two copies
//! silently drifting apart, per this project's usual "found this while
//! building the next thing" honesty.

use std::collections::HashSet;

use capability::{CapError, Capability, Kernel, Rights};

use crate::process::{Handle, Process};

/// One capability to move out of a source process's table.
#[derive(Debug, Clone, Copy)]
pub enum Grant {
    /// Move the capability at `Handle` out entirely — the source's own
    /// handle stops resolving to anything afterward.
    Transfer(Handle),
    /// Resolve a `Kernel::derive`d, strictly-weaker-or-equal capability;
    /// the source keeps using its own unaffected original.
    Derive(Handle, Rights),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GrantError {
    #[error("handle does not exist in the source process's capability table")]
    UnknownHandle,
    #[error("handle {0:?} is transferred more than once in the same batch")]
    DuplicateTransfer(Handle),
    #[error(transparent)]
    Capability(#[from] CapError),
}

/// Validates every `Grant` in `grants` against `source`'s table —
/// **without mutating anything** — and returns the resolved
/// capabilities in order if every single one would succeed. Callers
/// apply the actual removal via `apply_transfers` only once they know
/// the whole batch is going somewhere real (a new child process, a
/// channel message), so a batch that partially fails never leaves
/// `source` stripped of a capability it named for nothing.
pub fn resolve_grants(
    kernel: &Kernel,
    source: &Process,
    grants: &[Grant],
) -> Result<Vec<Capability>, GrantError> {
    let mut resolved = Vec::with_capacity(grants.len());
    let mut transferred = HashSet::new();
    for g in grants {
        // A handle can only leave the table once: `apply_transfers`
        // removes it once, so resolving it twice would hand the
        // destination two copies of a capability the source only gave up
        // one of.
        if let Grant::Transfer(h) = *g {
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
        };
        resolved.push(cap);
    }
    Ok(resolved)
}

/// Removes every `Transfer`'d handle in `grants` from `source`. Callers
/// must only call this after `resolve_grants` succeeded for the exact
/// same `grants` — this half never re-validates, it just performs the
/// removal side effect `Transfer` promises.
pub fn apply_transfers(source: &mut Process, grants: &[Grant]) {
    for g in grants {
        if let Grant::Transfer(h) = g {
            source.take(*h);
        }
    }
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
    fn apply_transfers_removes_only_the_transfer_variants() {
        let mut k = Kernel::new();
        let mut p = Process::new(ProcessId(0));
        let cap_a = k.new_object(Rights::READ | Rights::GRANT);
        let cap_b = k.new_object(Rights::WRITE);
        let handle_a = p.grant(cap_a);
        let handle_b = p.grant(cap_b);

        let grants = [
            Grant::Transfer(handle_a),
            Grant::Derive(handle_b, Rights::WRITE),
        ];
        apply_transfers(&mut p, &grants);

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
