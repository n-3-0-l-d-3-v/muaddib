# THE KERNEL — architecture (Phase 4)

## Overview

MUADDIB is a capability-based operating environment simulation: a
userspace kernel (not hardware-privileged code — see "What this is not"
below) enforcing the same security and coordination discipline a real
capability OS would, over simulated processes, memory, and IPC. Per
`docs/design/CONSTRAINTS.md`, three ambient conveniences every
conventional OS assumes are removed outright, not merely discouraged:

1. **No hierarchical path resolution.** Every resource (memory region,
   channel endpoint, process) is named by an opaque object reference,
   never a path string. There is no "filesystem" to walk.
2. **No trusted wall clock.** Nothing in this kernel ever reads
   `SystemTime`/`Instant` to order events. Causal ordering between
   processes is established purely through logical/vector clocks
   attached to IPC messages.
3. **All resource access is capability-mediated.** A process can only
   touch what it holds an unforgeable, explicitly-granted capability
   for — no ambient authority, no "the process can just open anything by
   name."

## Pipeline / subsystem layout

```text
crates/capability   -- unforgeable, revocable, attenuation-only capabilities
                        over opaque object references (ticket 001)
crates/process      -- tasks/processes, each with its own capability
                        set, a scheduler with no wall-clock-based
                        preemption (ticket 002)
crates/ipc          -- message-passing channels; capabilities themselves
                        are transferable message payloads (ticket 003)
crates/memory       -- memory regions with kernel-enforced ownership
                        transfer (not just Rust's own compile-time
                        ownership — a runtime transfer op checked
                        against the capability holding it) (ticket 004)
crates/clock        -- logical (Lamport) and vector clocks; the only
                        ordering primitive anything in this kernel is
                        allowed to use (ticket 005)
```

Each crate is usable independently (as sietch's `storage` crate proved
`BTree`/`Store`/`IndexedStore` could be); the closing ticket wires them
together into one real, observable workload (a multi-process pipeline
passing memory ownership and channel capabilities through IPC, causally
ordered without ever touching a wall clock) plus differential/benchmark
coverage, mirroring every other subsystem in this ecosystem's closing
pattern.

## What this is not

This is **not** hardware-privileged kernel code, a real scheduler
running on bare metal, or an MMU-backed memory-protection system. Like
mentat simulates a machine and sietch simulates a storage engine, this
crate simulates the *coordination discipline* a capability OS enforces
— unforgeability, revocation, attenuation-only delegation, ownership
transfer, causal-not-physical ordering — as a real, testable Rust
library, not a toy. Every invariant a real capability kernel (seL4,
KeyKOS, EROS) enforces at the hardware/kernel boundary, this enforces at
the type-and-runtime-check boundary instead; the research question this
phase exists to answer is what discipline survives that translation,
not to ship a bootable kernel.
