//! Process spawning and a cooperative scheduler ordered by an explicit
//! logical signal (FIFO ready-queue position) — never a wall clock, per
//! `docs/design/CONSTRAINTS.md`. `spawn_child` is where "no ambient
//! authority" actually gets enforced: a child's capability table starts
//! empty and receives *only* what the caller explicitly names, either
//! transferred (the parent loses it) or derived (the parent keeps its
//! own, weaker copies go to the child).

use std::collections::{HashMap, VecDeque};

use capability::{Capability, Kernel};

use crate::process::{Process, ProcessId};
use crate::transfer::{apply_transfers, resolve_grants, GrantError};

pub use crate::transfer::Grant;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProcessError {
    #[error("process {0:?} does not exist")]
    UnknownProcess(ProcessId),
    #[error(transparent)]
    Grant(#[from] GrantError),
}

pub struct Scheduler {
    processes: HashMap<ProcessId, Process>,
    next_process_id: u64,
    /// The *only* notion of order this scheduler has: position in a
    /// FIFO queue, advanced explicitly by `schedule_next`. No timestamp,
    /// no timer interrupt, nothing wall-clock-derived anywhere near it.
    ready_queue: VecDeque<ProcessId>,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            processes: HashMap::new(),
            next_process_id: 0,
            ready_queue: VecDeque::new(),
        }
    }

    fn fresh_id(&mut self) -> ProcessId {
        let id = ProcessId(self.next_process_id);
        self.next_process_id += 1;
        id
    }

    /// Spawns a process with exactly the capabilities in `initial` — a
    /// root process (no parent to inherit from, or shouldn't inherit
    /// from). Every real spawn in a running system goes through
    /// `spawn_child` instead; this exists for bootstrapping the first
    /// process(es) a simulation starts with.
    pub fn spawn(&mut self, initial: Vec<Capability>) -> ProcessId {
        let id = self.fresh_id();
        let mut proc = Process::new(id);
        for cap in initial {
            proc.grant(cap);
        }
        self.processes.insert(id, proc);
        self.ready_queue.push_back(id);
        id
    }

    /// Spawns a child of `parent`, handing it exactly the capabilities
    /// named in `grants` — nothing else the parent can see is visible to
    /// the child. **All-or-nothing**: every grant is validated (the
    /// handle exists; a `Derive` would actually succeed) before any of
    /// them are applied, so a single invalid grant can never leave the
    /// parent partially stripped of a capability it named.
    pub fn spawn_child(
        &mut self,
        kernel: &Kernel,
        parent: ProcessId,
        grants: Vec<Grant>,
    ) -> Result<ProcessId, ProcessError> {
        let parent_proc = self
            .processes
            .get(&parent)
            .ok_or(ProcessError::UnknownProcess(parent))?;
        let resolved = resolve_grants(kernel, parent_proc, &grants)?;

        // Every grant validated; now actually apply. Transfers remove
        // from the parent's table only now, never during validation.
        apply_transfers(
            self.processes.get_mut(&parent).expect("checked above"),
            &grants,
        );

        let child_id = self.fresh_id();
        let mut child = Process::new(child_id);
        for cap in resolved {
            child.grant(cap);
        }
        self.processes.insert(child_id, child);
        self.ready_queue.push_back(child_id);
        Ok(child_id)
    }

    pub fn process(&self, id: ProcessId) -> Option<&Process> {
        self.processes.get(&id)
    }

    /// Mutable access to one process's own table — needed by anything
    /// that must call `Process::grant`/`take` directly (e.g. `ipc`'s
    /// `send`/`receive`, which aren't `Scheduler` methods since IPC
    /// doesn't need to know about scheduling at all, only about
    /// individual processes' tables).
    pub fn process_mut(&mut self, id: ProcessId) -> Option<&mut Process> {
        self.processes.get_mut(&id)
    }

    pub fn process_count(&self) -> usize {
        self.processes.len()
    }

    /// Advances the cooperative round-robin: returns the next process
    /// due to run and moves it to the back of the queue. Purely a
    /// logical position in a queue this scheduler itself controls —
    /// never derived from elapsed wall-clock time.
    pub fn schedule_next(&mut self) -> Option<ProcessId> {
        let id = self.ready_queue.pop_front()?;
        self.ready_queue.push_back(id);
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use capability::{CapError, Rights};

    #[test]
    fn spawn_grants_exactly_the_initial_capabilities() {
        let mut k = Kernel::new();
        let mut s = Scheduler::new();
        let cap = k.new_object(Rights::READ);
        let id = s.spawn(vec![cap]);
        let proc = s.process(id).unwrap();
        assert_eq!(proc.handle_count(), 1);
    }

    #[test]
    fn a_freshly_spawned_process_holds_nothing_by_default() {
        let mut s = Scheduler::new();
        let id = s.spawn(vec![]);
        assert_eq!(s.process(id).unwrap().handle_count(), 0);
    }

    #[test]
    fn spawn_child_via_transfer_removes_the_capability_from_the_parent() {
        let mut k = Kernel::new();
        let mut s = Scheduler::new();
        let cap = k.new_object(Rights::READ);
        let parent = s.spawn(vec![cap]);
        let handle = s.process(parent).unwrap().handles().next().unwrap();

        let child = s
            .spawn_child(&k, parent, vec![Grant::Transfer(handle)])
            .unwrap();

        assert_eq!(s.process(parent).unwrap().handle_count(), 0);
        assert_eq!(s.process(parent).unwrap().capability(handle), None);
        assert_eq!(s.process(child).unwrap().handle_count(), 1);
    }

    #[test]
    fn spawn_child_via_derive_leaves_the_parents_own_capability_untouched() {
        let mut k = Kernel::new();
        let mut s = Scheduler::new();
        let cap = k.new_object(Rights::READ | Rights::WRITE | Rights::GRANT);
        let parent = s.spawn(vec![cap]);
        let handle = s.process(parent).unwrap().handles().next().unwrap();

        let child = s
            .spawn_child(&k, parent, vec![Grant::Derive(handle, Rights::READ)])
            .unwrap();

        // Parent still has its own, unweakened capability.
        assert_eq!(s.process(parent).unwrap().capability(handle), Some(cap));
        // Child got a strictly weaker one.
        let child_handle = s.process(child).unwrap().handles().next().unwrap();
        let child_cap = s.process(child).unwrap().capability(child_handle).unwrap();
        assert_eq!(child_cap.rights(), Rights::READ);
        assert!(cap.rights().contains(child_cap.rights()));
    }

    #[test]
    fn deriving_more_rights_than_the_parent_holds_is_rejected() {
        let mut k = Kernel::new();
        let mut s = Scheduler::new();
        let cap = k.new_object(Rights::READ | Rights::GRANT);
        let parent = s.spawn(vec![cap]);
        let handle = s.process(parent).unwrap().handles().next().unwrap();

        let result = s.spawn_child(
            &k,
            parent,
            vec![Grant::Derive(handle, Rights::READ | Rights::WRITE)],
        );
        assert!(matches!(
            result,
            Err(ProcessError::Grant(GrantError::Capability(
                CapError::CannotAmplify { .. }
            )))
        ));
        // Nothing should have changed: no child spawned, parent untouched.
        assert_eq!(s.process(parent).unwrap().capability(handle), Some(cap));
    }

    #[test]
    fn an_invalid_grant_leaves_the_parent_completely_unchanged() {
        let mut k = Kernel::new();
        let mut s = Scheduler::new();
        let cap = k.new_object(Rights::READ);
        let parent = s.spawn(vec![cap]);
        let valid_handle = s.process(parent).unwrap().handles().next().unwrap();
        let bogus_handle = crate::process::Handle(9999);

        // A batch mixing one valid transfer with one invalid handle must
        // apply *none* of it — including the valid transfer.
        let result = s.spawn_child(
            &k,
            parent,
            vec![Grant::Transfer(valid_handle), Grant::Transfer(bogus_handle)],
        );
        assert_eq!(result, Err(ProcessError::Grant(GrantError::UnknownHandle)));
        assert_eq!(
            s.process(parent).unwrap().capability(valid_handle),
            Some(cap)
        );
        assert_eq!(s.process(parent).unwrap().handle_count(), 1);
    }

    #[test]
    fn schedule_next_cycles_through_every_process_in_order() {
        let mut s = Scheduler::new();
        let a = s.spawn(vec![]);
        let b = s.spawn(vec![]);
        let c = s.spawn(vec![]);
        assert_eq!(s.schedule_next(), Some(a));
        assert_eq!(s.schedule_next(), Some(b));
        assert_eq!(s.schedule_next(), Some(c));
        assert_eq!(s.schedule_next(), Some(a)); // wrapped back around
    }

    #[test]
    fn spawn_child_of_an_unknown_parent_is_a_typed_error() {
        let k = Kernel::new();
        let mut s = Scheduler::new();
        let bogus = ProcessId(999);
        assert_eq!(
            s.spawn_child(&k, bogus, vec![]),
            Err(ProcessError::UnknownProcess(bogus))
        );
    }
}
