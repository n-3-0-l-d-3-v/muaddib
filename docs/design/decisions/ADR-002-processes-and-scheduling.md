# ADR-002: Processes as file-descriptor-style handle tables; all-or-nothing spawn_child; a purely logical scheduler

## Status
Accepted

## Context

Ticket 001 gave this repo unforgeable, revocable, attenuation-only
capabilities, but no notion of a process yet — nothing enforcing "no
ambient authority" at the level of *who currently holds what*. Three
design questions had to be settled: how a process names the
capabilities it holds without exposing raw `capability::Capability`
values as its whole interface, how a child process's initial authority
gets constructed without accidentally inheriting everything the parent
can see, and how the scheduler orders work without ever touching a
wall clock.

## Decision

**A process's capability table is indexed by small local integer
`Handle`s, exactly like a real OS's file-descriptor table** — a
process never refers to a capability by object identity, only by a
handle meaningful within its own table. `Process::grant` is the *only*
way a capability enters that table; there is no other public API that
inserts one. This is the same pattern ticket 001's `Capability`
unforgeability uses (restrict the construction path, not add a runtime
check), applied one layer up.

**`Scheduler::spawn_child` is all-or-nothing.** Every `Grant` in a batch
(`Transfer` or `Derive`) is validated — the named handle exists, and a
`Derive`'s requested rights are actually a subset — *before any of them
are applied*. Only once every grant in the batch is known to succeed
does the method touch the parent's table (removing transferred handles)
or the child's (inserting everything). Without this, a batch like
`[Transfer(valid_handle), Transfer(bogus_handle)]` could strip the
parent of `valid_handle` and then fail on the second grant, leaving the
capability nowhere — neither parent nor (never-created) child could
reach it again. `an_invalid_grant_leaves_the_parent_completely_unchanged`
pins this down directly.

**No ambient authority is structural, not policed.** A freshly spawned
child's table starts empty; `spawn_child` inserts *only* what the
caller's `Grant` list names. There is no "copy the parent's whole
table" code path anywhere to accidentally call — the property
(`a_transferred_capability_exists_in_exactly_one_process_at_a_time`,
`deriving_to_multiple_children_never_amplifies_and_never_affects_the_parent`)
is really testing "the two `Grant` variants behave correctly," since the
absence of implicit inheritance is a fact about what code exists, not a
runtime check that could pass or fail.

**The scheduler's only ordering concept is FIFO ready-queue position.**
`schedule_next` pops the front of a `VecDeque<ProcessId>` and pushes it
back — cooperative round-robin, with zero notion of elapsed time,
priority decay, or a timer interrupt. This satisfies
`docs/design/CONSTRAINTS.md`'s "no trusted wall clock" as literally as
possible for a first scheduler: there is nothing resembling a clock
anywhere in `Scheduler`'s code, not even one this ticket promises not to
read from.

## Alternatives Considered
1. Give a `Process` a public method returning `&Capability` by object
   ID, or let a child simply clone the parent's whole `HashMap` at spawn
   — rejected outright: either reopens ambient authority (any process
   could construct a query for any object it happens to know the ID of)
   or defeats the entire point of ticket 002 (a child seeing everything
   the parent could).
2. Best-effort `spawn_child` (apply whatever grants succeed, report
   which failed) — rejected: silently-partial capability grants are
   exactly the kind of "did I actually get what I asked for" ambiguity
   a capability system exists to eliminate. All-or-nothing with a single
   typed error is simpler to reason about and matches how ticket 001's
   own `Kernel` operations are already atomic (`check` is pass/fail,
   never partial).

## Consequences

- Ticket 002 closes. Ticket 003 (IPC) can build directly on `Handle`/
  `Process`/`Grant` — a channel send is naturally "derive or transfer a
  capability into a message" using the exact same primitives, not a
  parallel mechanism.
- `Process::capability`/`take` return **copies** (`Capability` is
  `Copy`, per ticket 001's ADR-001) — this crate's properties are about
  what `Process`/`Scheduler`'s own API surface can produce, not a
  defense against code that stashes a raw `Capability` value obtained
  earlier and calls `Kernel::check` on it directly, bypassing the
  `Process` abstraction entirely. That's out of scope the same way
  sietch's `Store` doesn't defend against a caller manually corrupting
  its on-disk files — this is a simulation of the coordination
  discipline, not a sandbox against actively adversarial code sharing
  the same address space (see `docs/design/KERNEL.md`'s "What this is
  not").
- The scheduler is cooperative only (no preemption) — a real limitation
  for anything modeling actual concurrent execution, tracked as
  `SCOPE.md`'s EXPERIMENT-tier "preemptive, logical-clock-driven
  scheduling," not silently assumed sufficient forever.
