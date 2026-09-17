---
status: open
phase: 4
---

# 002 — Processes and a wall-clock-free scheduler

Tasks/processes, each holding its own capability set (no ambient
authority — a process can only touch what capabilities it was given at
spawn or received over IPC), scheduled without ever consulting a wall
clock.

## Scope
- A `Process` owning a private capability set; spawning a child process
  transfers or derives specific capabilities into it explicitly (never
  "child inherits everything the parent can see").
- A cooperative scheduler ordering runnable processes by an explicit,
  logical (not wall-clock) readiness/priority signal.
- Property test: a process can never observe or use a capability it was
  never explicitly given, for arbitrary spawn/grant sequences.

Not started. Depends on ticket 001.
