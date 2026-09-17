---
status: done
phase: 4
---

# 005 — Logical and vector clocks

The only ordering primitive anything in this kernel is allowed to use —
no `SystemTime`/`Instant` reads anywhere in the design, per
`docs/design/CONSTRAINTS.md`.

## Scope
- [x] Lamport scalar clocks: every IPC send/receive (ticket 003)
      advances and stamps a message with a Lamport timestamp —
      `clock::LamportClock`, carried in each `ipc::Message`'s `sent_at`
      stamp; `send` returns the send stamp, `receive` returns a
      `Delivery` with both stamps.
- [x] Vector clocks: per-process vector timestamps, so genuine causal
      concurrency is distinguishable from causally-ordered events —
      `clock::VectorClock`, `Causality::{Before, After, Equal,
      Concurrent}`. Every `Process` owns an `EventClock`; spawning a
      child is a causal send the child's first event receives.
- [x] Property test: for arbitrary interleavings of sends/receives across
      simulated processes, the vector-clock partial order agrees with
      the true happened-before relationship, computed by the harness from
      the actual event graph with no clocks involved —
      `vector_order_is_exactly_the_true_happened_before_relation` (bare
      clocks) and `kernel_stamps_agree_with_the_true_happened_before_relation`
      (real spawns and IPC). Both also check the Lamport clock condition.
      Mutation-checked.

## Also done
- [x] `Stamp` has no public constructor, so causal knowledge can't be
      forged (found in design, before shipping).
- [x] Semilattice-law property tests for `merge`/`compare`.
- [x] `no_wall_clock` guard test: fails the build if any kernel `src/`
      uses a physical-time API (checked against an injected violation).

`clock`: 13 unit tests, 4 property tests, 2 guard tests. `process` and
`ipc` gained event-recording tests plus the kernel-level causality
property. See `docs/design/decisions/ADR-005-logical-clocks.md`.
