---
status: done
phase: 4
---

# 001 — Capability model

The foundation everything else in this repo builds on: unforgeable,
revocable, attenuation-only capabilities over opaque object references
— no paths, no ambient authority.

## Acceptance criteria
- [x] `ObjectId`: an opaque object reference (a monotonic counter, never
      wall-clock-derived), never a path.
- [x] `Rights`: a bitflag set (`READ`, `WRITE`, `EXECUTE`, `GRANT`,
      `DESTROY`).
- [x] `Capability`: unforgeable by construction — no public constructor;
      the only way to obtain one is `Kernel::new_object` or
      `Kernel::derive`. Carries an epoch so revocation can invalidate
      every outstanding capability for an object without tracking who
      holds them.
- [x] `Kernel::check`: validates a capability's epoch against the
      object's current epoch and that its rights are a superset of what
      is required.
- [x] `Kernel::revoke`: bumps an object's epoch, invalidating every
      capability minted before the bump — proven directly: a captured
      capability that passed `check` before `revoke` must fail it after,
      for arbitrary rights (property test).
- [x] `Kernel::derive`: attenuation only — requires holding `GRANT`, and
      the derived capability's rights must be a subset of the parent's.
      Proven for arbitrary rights combinations that a derived capability
      can never hold a right its parent lacked (property test) — the
      security property that makes "capability" mean something.
- [x] `Kernel::destroy_object`: requires `DESTROY`; every capability for
      that object fails every future `check` with `UnknownObject`
      afterward, not `Revoked` — destruction and revocation are
      observably different failure modes.

See `docs/design/decisions/ADR-001-capability-model.md` for the
unforgeability-via-privacy and epoch-based-revocation design choices.
