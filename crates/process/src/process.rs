//! A process's private capability table: a real OS's file-descriptor-
//! table pattern, applied to capabilities in general rather than just
//! open files. A process never touches a `capability::Capability` by
//! object identity — only by a small local integer `Handle` it was
//! explicitly given. There is no way to reach into a `Process` and get
//! a capability it wasn't granted; the only insertion path is
//! `Process::grant`, called either directly (`Scheduler::spawn`'s
//! initial grants) or internally by `Scheduler::spawn_child` (ticket
//! 002's whole point: no ambient authority, no "child inherits
//! everything the parent can see").

use std::collections::HashMap;

use capability::Capability;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProcessId(pub(crate) u64);

/// A process-local reference to one of its own capabilities — meaningful
/// only within the `Process` that issued it, the same way a file
/// descriptor is meaningless outside the process that opened it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Handle(pub(crate) u32);

pub struct Process {
    id: ProcessId,
    capabilities: HashMap<Handle, Capability>,
    next_handle: u32,
}

impl Process {
    pub(crate) fn new(id: ProcessId) -> Self {
        Self {
            id,
            capabilities: HashMap::new(),
            next_handle: 0,
        }
    }

    pub fn id(&self) -> ProcessId {
        self.id
    }

    /// Adds `cap` to this process's table under a fresh handle. The
    /// **only** way a capability ever enters a process's table.
    pub fn grant(&mut self, cap: Capability) -> Handle {
        let handle = Handle(self.next_handle);
        self.next_handle += 1;
        self.capabilities.insert(handle, cap);
        handle
    }

    /// Looks up a capability by the process's own handle. Returns a
    /// copy (capabilities are values, per
    /// `capability`'s ADR-001) — this does not remove it; use
    /// `take` to additionally remove it (e.g. as part of a transfer).
    pub fn capability(&self, handle: Handle) -> Option<Capability> {
        self.capabilities.get(&handle).copied()
    }

    /// Removes and returns the capability at `handle`, if any — after
    /// this call, `capability(handle)` returns `None`. Used by
    /// `Scheduler::spawn_child`'s `Grant::Transfer` to enforce that a
    /// transferred capability actually leaves the sender's table, not
    /// just gets copied into the receiver's.
    pub fn take(&mut self, handle: Handle) -> Option<Capability> {
        self.capabilities.remove(&handle)
    }

    /// Every handle this process currently holds — for inspection/
    /// testing, not a way to bypass `capability`'s access control (it
    /// still only yields handles, not the underlying rights beyond what
    /// `capability()` already exposes).
    pub fn handles(&self) -> impl Iterator<Item = Handle> + '_ {
        self.capabilities.keys().copied()
    }

    pub fn handle_count(&self) -> usize {
        self.capabilities.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use capability::{Kernel, Rights};

    #[test]
    fn a_granted_capability_is_reachable_by_its_handle() {
        let mut k = Kernel::new();
        let cap = k.new_object(Rights::READ);
        let mut p = Process::new(ProcessId(0));
        let handle = p.grant(cap);
        assert_eq!(p.capability(handle), Some(cap));
    }

    #[test]
    fn an_unused_handle_value_is_not_reachable() {
        let p = Process::new(ProcessId(0));
        assert_eq!(p.capability(Handle(0)), None);
    }

    #[test]
    fn take_removes_the_capability_from_the_table() {
        let mut k = Kernel::new();
        let cap = k.new_object(Rights::READ);
        let mut p = Process::new(ProcessId(0));
        let handle = p.grant(cap);
        assert_eq!(p.take(handle), Some(cap));
        assert_eq!(p.capability(handle), None);
        assert_eq!(p.take(handle), None); // already gone
    }

    #[test]
    fn distinct_grants_get_distinct_handles() {
        let mut k = Kernel::new();
        let mut p = Process::new(ProcessId(0));
        let a = p.grant(k.new_object(Rights::READ));
        let b = p.grant(k.new_object(Rights::WRITE));
        assert_ne!(a, b);
        assert_eq!(p.handle_count(), 2);
    }
}
