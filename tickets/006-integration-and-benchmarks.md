---
status: open
phase: 4
---

# 006 — Integration workload, differential testing, and benchmarks

The Definition-of-Done items this repo hasn't earned yet even once
tickets 001–005 close: a real, observable multi-subsystem workload, and
measured performance.

## Scope
- A real multi-process pipeline exercising every earlier ticket
  together: processes spawned with attenuated capabilities (002),
  passing memory-region ownership through IPC (003, 004), causally
  ordered by vector clocks alone (005) — with no wall clock touched
  anywhere in the whole run.
- Comparison: measure capability-check overhead against an ambient-
  authority baseline (a version of the same workload with capability
  checks stubbed out) to give a real, honest cost-of-security number
  rather than an assumed one.
- Chaos/fuzz-style testing: arbitrary interleavings and revocation-mid-
  transfer sequences, with a failure reproducible from a recorded seed
  per `docs/DEFINITION_OF_DONE.md`.

Not started. Depends on tickets 001–005.
