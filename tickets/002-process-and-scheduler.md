---
status: done
phase: 4
---

# 002 — Processes and a wall-clock-free scheduler

Tasks/processes, each holding its own capability set (no ambient
authority — a process can only touch what capabilities it was given at
spawn or received over IPC), scheduled without ever consulting a wall
clock.

## Scope
- [x] A `Process` owning a private capability set — indexed by small
      local `Handle`s exactly like a real OS's file-descriptor table.
      `Process::grant` is the *only* insertion path.
- [x] Spawning a child process transfers or derives specific
      capabilities into it explicitly (never "child inherits everything
      the parent can see") — `Scheduler::spawn_child` with a `Vec<Grant>`
      of `Transfer(Handle)`/`Derive(Handle, Rights)`, validated
      **all-or-nothing** before any grant is applied.
- [x] A cooperative scheduler ordering runnable processes by an explicit,
      logical (not wall-clock) FIFO ready-queue position —
      `Scheduler::schedule_next`.
- [x] Property test: a process can never observe or use a capability it
      was never explicitly given, for arbitrary spawn/grant sequences —
      `a_transferred_capability_exists_in_exactly_one_process_at_a_time`
      and
      `deriving_to_multiple_children_never_amplifies_and_never_affects_the_parent`,
      both proven for arbitrary rights combinations and chain
      lengths/derivation counts.

12 unit tests plus 2 property tests, all passing. See
`docs/design/decisions/ADR-002-processes-and-scheduling.md` for the
file-descriptor-table, all-or-nothing-spawn, and purely-logical-
scheduler design choices.
