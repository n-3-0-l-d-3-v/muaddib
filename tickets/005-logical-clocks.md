---
status: open
phase: 4
---

# 005 — Logical and vector clocks

The only ordering primitive anything in this kernel is allowed to use —
no `SystemTime`/`Instant` reads anywhere in the design, per
`docs/design/CONSTRAINTS.md`.

## Scope
- Lamport scalar clocks: every IPC send/receive (ticket 003) advances
  and stamps a message with a Lamport timestamp.
- Vector clocks: per-process vector timestamps, so genuine causal
  concurrency (two events neither of which happened-before the other)
  is distinguishable from causally-ordered events.
- Property test: for arbitrary interleavings of sends/receives across
  simulated processes, the vector-clock partial order correctly agrees
  with the true happened-before relationship the simulation itself
  knows (since the test harness controls scheduling, it knows ground
  truth to check against).

Not started. Depends on ticket 003.
