---
status: open
phase: 4
---

# 004 — Memory regions with kernel-enforced ownership transfer

Memory regions named by capability (never a path/address the process
picks), with ownership transfer enforced at the kernel level — not just
Rust's own compile-time move semantics, since transfer here crosses a
simulated process boundary a single Rust ownership check can't see
across.

## Scope
- A `MemoryRegion` object type allocated via `Kernel::new_object`.
- A transfer operation: after a region's owning capability moves from
  process A to process B (e.g. via ticket 003's IPC), A's copy of that
  capability must fail every subsequent `check` — a real, kernel-
  enforced revocation-on-transfer, not merely "please don't use this
  anymore."
- Property test: after a transfer, the old owner can never successfully
  read/write the region, for arbitrary sequences of transfers and
  access attempts.

Not started. Depends on tickets 001, 003.
