---
status: done
phase: 4
---

# 004 — Memory regions with kernel-enforced ownership transfer

Memory regions named by capability (never a path/address the process
picks), with ownership transfer enforced at the kernel level — not just
Rust's own compile-time move semantics, since transfer here crosses a
simulated process boundary a single Rust ownership check can't see
across.

## Scope
- [x] A memory region object type allocated via `Kernel::new_object` —
      `memory::RegionRegistry::new_region`, with `READ`/`WRITE`/`DESTROY`
      given region semantics, authority checked before bounds, and
      all-or-nothing out-of-bounds writes.
- [x] A transfer operation: after a region's owning capability moves
      from process A to process B (via ticket 003's IPC, or at spawn), A's
      copy fails every subsequent `check` — `process::Grant::Move`, backed
      by the new `Kernel::reissue`. Checking ADR-003's prediction against
      the code first showed the existing `Grant::Transfer` did **not**
      provide this (`Capability` is `Copy`; a kept copy still worked).
- [x] Property test: after a transfer, the old owner can never
      successfully read/write the region, for arbitrary sequences of
      transfers and access attempts —
      `only_capabilities_issued_since_the_last_move_can_touch_the_region`
      (model-based, every capability ever held replayed at random,
      in-flight views across moves) and
      `along_a_spawn_chain_only_the_last_owner_holds_authority`. Both
      mutation-checked.

## Latent bugs found and fixed along the way
- [x] `Kernel::revoke` required no rights and not even a live
      capability: a `NONE` view could revoke its owner, and a stale
      capability could revoke again (which would let an old owner kill
      the new owner's access). Now requires a live capability with
      `DESTROY`.
- [x] A handle transferred twice in one grant batch was delivered
      twice. Now rejected (`GrantError::DuplicateTransfer`).

12 unit tests plus 2 property tests in `memory`; `capability`, `process`
and `ipc` gained tests for the fixes and `Move`. See
`docs/design/decisions/ADR-004-memory-ownership.md`.
