# ADR-004: Ownership transfer is a kernel reissue, not a table move — and three latent holes it exposed

## Status
Accepted

## Context

Ticket 004 asks for memory regions named by capability, with ownership
transfer enforced by the kernel: after a region's owning capability
moves from process A to process B, *A's copy must fail every subsequent
`check`*.

ADR-003's Consequences section predicted this would need "no new
transfer mechanism — only a new object kind riding on the same rails"
(`Grant::Transfer` through a channel). **That prediction was wrong**,
and the first thing this ticket did was check it against the code
rather than assume it. A probe test showed:

1. **`Grant::Transfer` is not kernel-enforced.** It removes the entry
   from the sender's table, but `Capability` is `Copy` (deliberately —
   ADR-001) and `Process::capability(handle)` returns a copy. A process
   that read its capability out before sending it still passed
   `Kernel::check` afterward with full rights. Removing a table entry is
   bookkeeping; it is not revocation.
2. **`Kernel::revoke` checked nothing but the object's existence** — not
   the capability's epoch, not its rights. A `NONE`-rights view derived
   from an owner could revoke the owner. Worse for this ticket: an
   *already-revoked* capability could revoke again. Any transfer built
   on revocation would let the old owner bump the epoch one more time
   and kill the new owner's capability — denial of service by exactly
   the party the transfer was meant to cut off.
3. **A handle transferred twice in one batch was duplicated.**
   `resolve_grants` resolved `[Transfer(h), Transfer(h)]` to two copies
   while `apply_transfers` removed the handle once, so the destination
   received two capabilities the source gave up one of.

Hole 2 has existed since ticket 001; hole 3 since ticket 002. Neither
was caught by those tickets' property tests because none of them
generated the adversarial case — a holder acting with a capability it
should no longer be able to use.

## Decision

**Fix `revoke`: the capability must be live and hold `DESTROY`.**
Revoking every outstanding capability for an object is the same class
of power as destroying it, so it requires the same right. A stale
capability now fails with `Revoked`; a weaker view fails with
`InsufficientRights`. Proven for arbitrary owner/view rights
(`only_a_live_destroy_holding_capability_can_revoke`).

**Add `Kernel::reissue`: revoke, then mint one fresh capability with the
same rights at the new epoch.** This is the entire kernel-side
mechanism. After it, the returned capability is provably the only live
authority over the object — no matter how many copies of the old one
exist, what was derived from it, or what is sitting unreceived in a
channel queue. It uses the existing epoch scheme, so it still needs no
holder-tracking (ADR-001's central design win survives intact).

**Add `Grant::Move(Handle)` alongside `Transfer`/`Derive`, rather than
changing `Transfer`.** `Move` = `Transfer` + `reissue` at apply time.
`Transfer` keeps its old meaning because it is the right operation for
*shared* objects: making every transfer revoke would, for instance,
kill every other process's endpoint whenever one process passed a
channel capability along. Owning something exclusively and sharing it
are genuinely different, so they are different grants. `Move` requires
a live capability holding `DESTROY` — the same authority as
`revoke` — so *owning* an object means holding `DESTROY` for it.

Because `Move` lives in `process::transfer`, it works through both
existing paths with no per-object-kind code: `ChannelRegistry::send`
and `Scheduler::spawn_child`. ADR-003's "same operation shape" finding
paid off again — just not the way ADR-003 predicted.

The reissue happens in `apply_grants` (renamed from `apply_transfers`),
*after* all-or-nothing validation and *atomically with* removal from
the sender's table. The fresh capability goes straight into the message
(or child); the sender never sees it, so it has nothing to copy.
Consequences for the API: `apply_grants` needs `&mut Kernel`, so
`spawn_child` and `send` now take `&mut Kernel` too.

**Batch rules.** `resolve_grants` now rejects a handle transferred or
moved twice (`DuplicateTransfer`, fixing hole 3), and a batch that moves
an object while naming the same object in any other grant, via any
handle (`MoveConflict`) — the move would revoke that other capability
on arrival, so delivering it would be delivering something already dead.

**`crates/memory`: `RegionRegistry`, with no transfer API of its own.**
A region is an ordinary capability object backed by a zero-filled
`Box<[u8]>` in the registry (like `ChannelRegistry`, the kernel never
holds payload data). `READ`/`WRITE`/`DESTROY` get their meanings for
this object kind. Every access checks authority *before* bounds, so an
unauthorized caller learns nothing about a region's size from the
error. An out-of-bounds write changes no bytes (whole range validated
first; `offset + len` overflow is an error, not a panic). A capability
for some other object kind (e.g. a channel) is `UnknownRegion`.
`reclaim` drops the bytes of regions destroyed directly through
`Kernel::destroy_object` instead of `destroy_region`, using a new
authority-free `Kernel::object_exists`.

## Testing

- The ticket's property test is **model-based with a worst-case
  adversary**: over arbitrary sequences of moves, shared views (sent and
  delivered at arbitrary later points, so views can be in flight across
  a move), reads, writes, and revoke attempts by stale capabilities, every
  capability any process *ever* held is kept and replayed at random. A
  generation counter bumped on every move is the model: a capability is
  live iff issued in the current generation, and the region's bytes are
  a plain array updated only when the model says a write must succeed.
  After every step, every stashed capability's liveness and the region's
  entire contents are compared against the model. A second property
  moves a region down a `spawn_child` chain with every ancestor keeping
  its copy.
- **Mutation-checked, not just green.** With `apply_grants`' reissue
  deleted, both properties fail immediately. With `revoke`'s liveness
  check reverted to the old existence-only check, the IPC property fails
  (a stale owner's revoke kills the new owner). The failing seeds proptest
  recorded during those runs were kept in
  `crates/memory/tests/property.proptest-regressions`, so those exact
  inputs are replayed on every run.
- A unit test pins the contrast directly:
  `a_plain_transfer_is_not_enough_which_is_why_move_exists`.

Counts: `capability` 22 unit + 5 property, `process` 21 + 2, `ipc` 8 + 2,
`memory` 12 + 2.

## Alternatives Considered

1. **Make `Transfer` always reissue.** Rejected: that breaks shared
   objects (channels), whose other holders must not be revoked when one
   holder passes its copy along.
2. **Track holders so a transfer can revoke exactly the sender's copy.**
   Rejected for the same reason ADR-001 rejected a holder list: `Copy`
   capabilities make "the sender's copy" unknowable. A value that was
   copied can't be tracked; an epoch can invalidate all of them at once.
3. **A `memory::transfer` API separate from IPC.** Rejected: it would
   be a second implementation of all-or-nothing grant resolution (the
   exact duplication ADR-003 removed), and it would only work for memory.
   `Move` works for any object kind.
4. **Reissue into the sender's table first, then `Transfer` it.**
   Rejected: between the two steps the sender could copy the fresh
   capability, which is the original hole again. Reissue must happen
   atomically with removal, with the result never visible to the sender.

## Consequences

- Ticket 004 closes. CORE scope (tickets 001–004) is complete.
- **Moving an object also kills every view derived from it**, including
  views the old owner legitimately shared with third parties. That is
  what exclusive ownership means, but a new owner who wants those
  readers back must re-share. A system wanting "transfer ownership but
  keep readers" would need a derivation tree (seL4-style CDT), which
  this kernel deliberately doesn't have.
- `revoke` is stricter than ADR-001 described: it now needs `DESTROY`.
  No existing caller relied on the looser behaviour; unit and property
  tests that revoked capabilities without `DESTROY` were updated.
- `spawn_child`/`send` take `&mut Kernel`. `apply_transfers` is now
  `apply_grants`, with a different signature.
- **Known gap:** `ObjectId`s are per-`Kernel` counters, so a capability
  from one `Kernel` instance can name a real, unrelated object in
  another. Every workload so far uses a single kernel; ticket 006's
  integration work should keep it that way or tag capabilities with a
  kernel identity.
- **Known gap:** regions are fixed-size. No resize, no sharing of
  sub-ranges as separately capable objects. Neither is needed by ticket
  006's pipeline.
