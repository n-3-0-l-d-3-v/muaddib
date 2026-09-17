# ADR-005: Logical clocks as the only ordering primitive — stamped IPC, spawn as a causal send, unforgeable stamps

## Status
Accepted

## Context

`docs/design/CONSTRAINTS.md` rules out a trusted wall clock. Until now
the kernel got by without *any* notion of "when": the scheduler orders
by FIFO position (ADR-002) and nothing else needed ordering. Ticket 005
supplies what physical time would otherwise supply — an answer to "did
this happen before that?" — using only causality:

- **Lamport scalar clocks** (Lamport 1978): one counter per process;
  if `a` happened before `b` then `L(a) < L(b)`. Cheap, and gives a
  total order, but it can't tell causality from coincidence: a smaller
  Lamport time doesn't mean "happened before."
- **Vector clocks** (Fidge 1988, Mattern 1989): one counter per process
  *per process*; `a` happened before `b` **iff** `V(a) < V(b)`
  pointwise. Exact, at O(processes) size per timestamp.

The questions were where the clocks live, which kernel operations count
as events, and how to prove the result agrees with real causality
rather than just with itself.

## Decision

**`crates/clock` is generic and dependency-free.** `LamportClock`,
`VectorClock<K>`, and `EventClock<K>` (a process's pair of clocks,
advanced together) are generic over the process-id type, so `process`
can depend on `clock` and instantiate it with its own `ProcessId`
without a dependency cycle. Vector clocks are sparse `BTreeMap`s (absent
= 0), with equality and ordering defined over the union of keys so no
code depends on "the map never stores a zero."

**Every process owns an `EventClock`. Events are exactly:**
- a successful `ChannelRegistry::send`: a send event, whose `Stamp`
  travels inside the `Message` and is returned to the caller;
- a successful `ChannelRegistry::receive` of a message: a receive event
  merging the sender's stamp, reported (with the send stamp) in the new
  `Delivery` return value;
- `Scheduler::spawn_child`: **a send at the parent that the child's
  first event receives**, so everything the parent had seen causally
  precedes everything the child does;
- `Process::record_local_event`, for workloads that want internal steps
  ordered.

A rejected send, a failed spawn, and a receive on an empty queue record
**no** event: nothing happened. Unit tests pin each of these.

**`Stamp` has no public constructor.** This came up during design,
before any code shipped. A receive *merges whatever the incoming stamp
claims*. With public fields, any caller could build a stamp saying
"I've seen process 3's first million events" and feed it to
`record_receive`, and the victim's clock would carry that false causal
knowledge forward into every message it sent. So `Stamp`'s fields are
private and the only source of one is an `EventClock` event. This is the
same privacy-as-unforgeability approach `Capability` uses (ADR-001).

**Proving it against ground truth, not against itself.** A test that
checks vector clocks against vector clocks proves nothing. Both
property tests instead have the harness, which controls every
interleaving, build the *actual event graph*: each event's predecessors
are the previous event at the same process, plus the matching send for a
receive (or the parent's spawn event for a child's birth).
Happened-before is then computed by graph reachability, with no clocks
involved. For every ordered pair of events, `happened_before` must equal
reachability, `concurrent_with` must equal mutual unreachability, and
Lamport time must strictly increase along every causal path. This runs
at two levels:
- `clock/tests/property.rs`: bare `EventClock`s, arbitrary
  local/send/receive interleavings, plus semilattice laws for `merge`
  (commutative, associative, idempotent, least upper bound) and
  `compare` (antisymmetric, transitive).
- `ipc/tests/causality.rs`: the **real kernel**, with processes from
  `spawn`/`spawn_child` (up to 6), messages through `ChannelRegistry`,
  and the harness also checking that each delivery carries exactly the
  send stamp it expects.

**Mutation-checked.** Each of these deliberate breakages makes a
property fail with a minimal counterexample: dropping the vector merge
on receive, replacing the Lamport `max` with a plain tick, a child
birth that doesn't merge the parent's spawn stamp, and an IPC receive
that records a local event instead of merging. The seeds proptest
recorded from those runs are kept in the `*.proptest-regressions` files
and replayed on every run.

**The constraint is now a test.** `clock/tests/no_wall_clock.rs` scans
every crate's `src/` for `std::time`, `SystemTime`, `Instant`,
`UNIX_EPOCH` and `chrono` (skipping comments) and fails if any appear.
It was checked by injecting an `Instant::now()` into `memory`, which
failed as it should. Benchmarks are deliberately out of scope for the
scan: a harness timing the kernel from the outside is not the kernel
reading time.

## Findings, reported plainly

- **No correctness bug was found in the clock code itself.** The
  ground-truth properties passed on first run. Given that, the
  mutation checks are what show the tests have teeth. Without them, a
  first-run pass would be weak evidence.
- The one real design flaw, forgeable stamps, was caught in design,
  before it shipped (see above).

## Alternatives Considered

1. **Lamport clocks only.** Rejected: ticket 005 requires telling
   genuine concurrency apart from causal order, and Lamport time can't
   (`independent_local_events_are_concurrent_even_with_ordered_lamport_times`
   shows the gap concretely).
2. **Stamps only on IPC, not on spawn.** Rejected: a child's events
   would then appear concurrent with everything its parent did before
   spawning it, which is false. The causality property test (it
   includes spawns) fails on exactly that mutation.
3. **A hybrid logical clock (physical + logical).** Rejected: it needs
   physical time, which this kernel doesn't have by construction.
4. **Dense `Vec<u64>` vector clocks indexed by process number.**
   Rejected: processes are created dynamically, so every existing
   vector would need resizing; sparse maps grow only with processes a
   stamp has actually heard of.

## Consequences

- Ticket 005 closes; ticket 006's pipeline can order its events by
  stamps alone.
- `ChannelRegistry::send` now returns `Result<Stamp, _>` and `receive`
  returns `Result<Option<Delivery>, _>`. All call sites were updated.
- **Cost not yet measured:** each send clones the sender's vector clock
  into the message, which is O(processes the sender has heard of). That
  goes into ticket 006's capability-and-clock overhead benchmarks rather
  than being assumed cheap.
- **No causal delivery.** Channels stay FIFO per channel. A process
  reading several channels can receive messages out of causal order;
  stamps let it *detect* this (`Delivery::sent_at` comparisons), but
  nothing buffers messages to *enforce* causal order. That would be a
  causal-broadcast layer, not built here.
- **No garbage collection of vector entries.** An exited process's
  counter stays in every vector that ever heard of it. The kernel has no
  process exit yet, so this is latent.
- **Trust boundary, stated honestly.** Stamps can't be forged, but the
  simulation doesn't hide one process's stamps from code that holds the
  whole `Scheduler` (`scheduler.process(id).clock()`). Similarly,
  `Process::grant` is public (ADR-002). As everywhere in this crate, the
  code driving the `Scheduler` plays the kernel; isolation between
  processes is enforced at the capability/stamp boundary, not by Rust
  visibility of the scheduler itself.
