# ADR-001: Capability model — unforgeability via Rust privacy, revocation via epoch bump

## Status
Accepted

## Context

`docs/design/CONSTRAINTS.md` requires "all resource access happens
through explicit capabilities" with no ambient authority. Two design
questions had to be settled before any other ticket in this repo could
build on top: how "unforgeable" is actually enforced in a language
without hardware-privilege separation, and how revocation invalidates a
capability without the kernel needing to track every copy of it that
exists.

## Decision

**Unforgeability is enforced by Rust's own privacy system, not a
runtime check.** `Capability`'s three fields (`object`, `rights`,
`epoch`) are all private; the type has no public constructor at all.
The only ways client code can ever obtain a `Capability` value are
`Kernel::new_object` (mints a fresh one) and `Kernel::derive` (produces
a strictly-weaker view of one already held). There is no code path,
anywhere outside this crate, that can construct a `Capability` naming
an arbitrary object or arbitrary rights — the compiler itself refuses
it, the same guarantee a real capability kernel enforces at the
hardware/kernel boundary, translated to what this simulation's boundary
actually is (this crate's own module boundary). See
`docs/design/KERNEL.md`'s "What this is not" for why that translation
is the honest scope here, not a shortcut being passed off as more than
it is.

**Revocation is an epoch bump, not a holder-tracking scheme.** Every
object has a current epoch (starting at 0); every capability carries the
epoch it was minted at. `check` fails any capability whose epoch doesn't
match the object's *current* epoch. `revoke` simply increments the
object's epoch — every capability minted before that call, no matter
how many processes hold copies of it (`Capability` is `Copy`, by design:
a capability is a value, not a reference to shared mutable state), fails
`check` afterward. This is the standard "generation counter" revocation
technique (used by real capability systems like KeyKOS/EROS), chosen
specifically because it needs **no bookkeeping of who holds what** — the
kernel only ever tracks one number per object, regardless of how many
capabilities for it are in circulation.

**Derivation is attenuation-only, checked twice.** `derive` requires the
parent capability to (a) currently be valid (pass `check` for `GRANT`)
and (b) have its rights be a superset of whatever is requested — a
derived capability's rights are always `requested`, never `held`, and
`requested` is rejected outright (`CannotAmplify`) if it isn't a subset
of what the parent already had. This is the property that makes
"capability" mean something rather than just being a fancy handle: a
process can only ever hand out *less* authority than it has, never more,
and this is proven directly (not just asserted) for arbitrary rights
combinations via `derivation_never_amplifies_rights` and
`attenuation_composes_across_multiple_derivations`.

**Destruction is a distinct failure mode from revocation.**
`destroy_object` removes the object from the table entirely; every
future `check` against any capability for it returns `UnknownObject`,
never `Revoked`. A caller can legitimately want to distinguish "this
still exists but my access was cut off" from "this doesn't exist
anymore" (e.g. to decide whether retrying with a fresh capability could
ever succeed), so the two are kept observably different rather than
collapsed into one generic "invalid" error.

## Alternatives Considered
1. A `Vec<Capability>`/`HashSet` per object tracking every capability
   ever minted, with `revoke` removing specific entries — rejected: it
   would require the kernel to know about every copy anyone made
   (`Capability` being `Copy` makes this actively impossible to track
   completely), and doesn't scale to "revoke everyone" without
   iterating a growing set.
2. Runtime-checked "sealing" (e.g. an HMAC or random unguessable token
   embedded in the capability) instead of Rust-privacy-based
   unforgeability — rejected as unnecessary complexity: nothing in this
   simulation crosses a real trust boundary an HMAC would need to defend
   (no untrusted process can construct a `Capability` value at all,
   Rust-privacy already fully prevents it within this simulation's own
   scope).

## Consequences

- Ticket 001 closes. Every later ticket (processes, IPC, memory,
  clocks) builds directly on `Kernel`/`Capability`/`Rights` rather than
  inventing its own authority model.
- `Capability` being `Copy` is deliberate and load-bearing (see above)
  — a future ticket must not "fix" this into requiring `Clone`-only or
  move semantics without re-deriving why revocation's epoch scheme
  specifically depends on not needing to track copies.
- This crate's "kernel boundary" is this crate's own module boundary,
  not a process/hardware boundary — ticket 002 (processes) is where a
  simulated process-level boundary gets modeled on top of this
  foundation, not here.
