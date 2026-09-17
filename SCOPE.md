# Scope — muaddib

## CORE (required for this repo to be considered complete at all)
- Capability model: unforgeable, revocable, attenuation-only
  capabilities over opaque object references (ticket 001).
- Processes with private capability sets and a wall-clock-free
  scheduler (ticket 002).
- IPC with capability-carrying messages (ticket 003).
- Memory regions with kernel-enforced ownership transfer (ticket 004).

## EXTENSION (required for full integration into the combined ecosystem)
- Logical/vector clocks as the sole ordering primitive (ticket 005).
- A real multi-process integration workload, capability-check-overhead
  benchmarks, and chaos/fuzz testing with reproducible seeds
  (ticket 006).

## EXPERIMENT (only attempted once CORE + EXTENSION are healthy)
- A capability-secured equivalent of a filesystem-like naming
  convenience (a directory-shaped index of object references) — to
  directly test this phase's research question, "is a filesystem
  fundamentally a hierarchy?", without reintroducing path resolution
  as a kernel primitive.
- Preemptive (not just cooperative) scheduling driven by logical
  clocks instead of a wall-clock timer interrupt.
