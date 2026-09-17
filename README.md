# MUADDIB — THE KERNEL

> A capability-based OS with no filesystem hierarchy and no wall clock.

## Why "MUADDIB"

Paul's chosen Fremen name, taken from the desert mouse — an animal known for surviving without ever needing open water, because it adapted by removing a requirement every other creature depends on. That is the kernel's entire design: an operating environment that runs without the filesystem hierarchy and the wall clock every other OS assumes it needs, having adapted around their absence instead of faking them.

Part of **[ARRAKIS](https://github.com/n-3-0-l-d-3-v/arrakis)** — a constrained computing
ecosystem built by removing assumptions ordinary computers depend on. This
repository is developed standalone and mirrored into the combined ecosystem
repo commit-for-commit.

## Status

**Phase 4 — ACTIVE.** See [docs/design/KERNEL.md](docs/design/KERNEL.md)
for the full architecture (capability model -> processes -> IPC ->
memory ownership -> logical clocks) and what this simulation deliberately
is and isn't.

**Ticket 001 (capability model) is done.** `crates/capability`:
unforgeable capabilities (no public constructor — Rust's own privacy
system is the unforgeability guarantee, not a runtime check), epoch-
based revocation (invalidates every outstanding capability for an
object with no holder-tracking needed), and attenuation-only derivation
— proven, not just asserted, that a derived capability can never hold a
right its parent lacked, for arbitrary rights combinations. 16 unit
tests plus 4 property tests. See
[ADR-001](docs/design/decisions/ADR-001-capability-model.md).

**Ticket 002 (processes and scheduler) is done.** `crates/process`: a
`Process` indexes its capabilities by small local `Handle`s — a real
OS's file-descriptor-table pattern, applied to capabilities generally.
`Scheduler::spawn_child` hands a child *only* the capabilities its
caller explicitly names (transferred or attenuated via `derive`),
validated all-or-nothing before any of them are applied — never "child
inherits everything the parent can see." The scheduler's only notion of
order is FIFO ready-queue position; nothing resembling a clock exists
anywhere in it. 12 unit tests plus 2 property tests, proven for
arbitrary rights combinations and transfer-chain lengths. See
[ADR-002](docs/design/decisions/ADR-002-processes-and-scheduling.md).

**Ticket 003 (IPC) is done.** `crates/ipc`: channels are ordinary
`capability` objects (`WRITE` = may send, `READ` = may receive — the
first real meaning given to ticket 001's previously-uninterpreted
rights bits), and sending a capability reuses ticket 002's grant-
resolution machinery directly rather than a parallel implementation — a
discovery made while building this ticket, not planned in advance:
spawning a child with attenuated authority and sending a capability
over a channel are the same operation shape. A sent-but-unreceived
capability provably exists in *no* process's table at all, only in the
channel's own queue. 7 unit tests plus 2 property tests prove a
received capability can never exceed what the sender held, for
arbitrary rights and arbitrary-length relay chains across multiple
channels. See [ADR-003](docs/design/decisions/ADR-003-ipc.md).

**Ticket 004 (memory ownership) is done — CORE scope is complete.**
`crates/memory`: regions named only by capability, with ownership
transfer the kernel actually enforces. Checking ADR-003's prediction
("no new transfer mechanism needed") against the code showed it was
wrong: `Grant::Transfer` only empties a table slot, and since
capabilities are `Copy`, a process that kept a copy still had full
access. The fix is `Grant::Move`, backed by a new `Kernel::reissue`
(revoke every outstanding capability, mint exactly one fresh one,
delivered straight into the message so the sender never sees it). It
works through both IPC and spawn, for any object kind. Building it
exposed two older bugs, both fixed: `Kernel::revoke` required no rights
and not even a live capability (a stale old owner could have revoked
the *new* owner), and a handle transferred twice in one batch was
delivered twice. The property test is model-based: every capability
any process ever held is kept and replayed at random across arbitrary
moves, in-flight views, reads and writes. It was mutation-checked by
deleting the reissue and reverting the revoke fix, and fails in both
cases. See [ADR-004](docs/design/decisions/ADR-004-memory-ownership.md).

**Ticket 005 (logical clocks) is done.** `crates/clock`: Lamport and
vector clocks are the kernel's only notion of "when." Every process owns
a pair of clocks. IPC sends and receives are stamped events, and
spawning a child counts as a causal send that the child's first event
receives. Stamps have no public constructor, so a process can't claim
knowledge of events it never saw. The property tests don't check the
clocks against themselves: the harness builds the real event graph and
computes happened-before by reachability, then requires the vector-clock
order to match it exactly. This runs both for bare clocks and through
real spawns and channels. No correctness bug turned up, so the tests
were mutation-checked instead; four separate breakages each fail them.
The "no wall clock" constraint is now a test too: the build fails if any
kernel source uses a physical-time API. See
[ADR-005](docs/design/decisions/ADR-005-logical-clocks.md).

See [tickets/](tickets/) for the live phase-by-phase ticket board and
[docs/design/](docs/design/) for constraints, invariants and architecture
decision records.

## The constraint

There is no hierarchical path resolution anywhere in the system, no trusted wall clock, and all resource access happens through explicit capabilities.

## What the constraint forces

Capability passing, IPC, logical/vector clocks for ordering, and object-reference-based resource naming instead of paths.

## Research question

> Is a filesystem fundamentally a hierarchy, or is naming merely one possible interface to persistent objects? What can an OS do when physical time is unavailable as a coordination primitive?

## Sibling repositories

- [mentat](https://github.com/n-3-0-l-d-3-v/mentat) — THE MACHINE (COMPLETE)
- [chakobsa](https://github.com/n-3-0-l-d-3-v/chakobsa) — THE LANGUAGE (COMPLETE)
- [sietch](https://github.com/n-3-0-l-d-3-v/sietch) — THE VAULT (COMPLETE)
- [choam](https://github.com/n-3-0-l-d-3-v/choam) — THE DATABASE (QUEUED)
- [distrans](https://github.com/n-3-0-l-d-3-v/distrans) — THE WIRE (QUEUED)
- [landsraad](https://github.com/n-3-0-l-d-3-v/landsraad) — THE COLONY (QUEUED)
- [ghola](https://github.com/n-3-0-l-d-3-v/ghola) — THE HISTORY (QUEUED)
- [shai-hulud](https://github.com/n-3-0-l-d-3-v/shai-hulud) — THE ARTIFACT (STRETCH)

## Development

This is a real, tested, benchmarked systems component — not a demo. See
[docs/DEFINITION_OF_DONE.md](docs/DEFINITION_OF_DONE.md) for the acceptance
bar every piece of this repo must clear before it is considered complete.

```bash
cargo build
cargo test
cargo bench
```
