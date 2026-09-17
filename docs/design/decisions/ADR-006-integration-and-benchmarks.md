# ADR-006: One pipeline over the whole kernel, an ambient-authority twin, a seeded chaos harness, and what the discipline actually costs

## Status
Accepted

## Context

Tickets 001–005 each proved their own invariant in isolation. Ticket 006
must show three things: that the pieces compose into a real workload with
no physical time anywhere; what the capability discipline *costs*,
measured against an ambient-authority version of the same workload; and
that arbitrary interleavings, including revocation mid-transfer, can't
break the invariants, with any failure replayable from a recorded seed
(`docs/DEFINITION_OF_DONE.md` items 1, 6, 7, 9 and 10).

## Decision

### `crates/workload`, part 1: the pipeline

A supervisor creates the channels and a pool of memory regions, then
spawns a producer, N transformer stages and a sink with `spawn_child`.
Each stage gets only a receive-only view of its input channel and a
send-only view of its output channel (`Grant::Derive`, no `GRANT`, so no
re-delegation). The producer also gets the regions (`Grant::Move`). The
regions circulate producer → transformers → sink → producer, and *every
hop is a `Grant::Move`*. Stages touch region bytes with the new zero-copy
`RegionRegistry::inspect`/`update`. Every send and receive is stamped
(ticket 005), and `Scheduler::schedule_next` round-robin is the only
scheduling. Supporting changes made along the way: `Delivery` now reports
the handles a message's capabilities were granted under (receivers
previously had to guess "the newest handle"), and `inspect`/`update` do
exactly one capability check per call.

Its tests (`tests/pipeline.rs`) check:
- every batch processed exactly once with the independently computed
  reference checksum;
- least privilege (each stage's exact rights, a stage can't send on its
  input or re-derive, the supervisor keeps no region);
- every region capability the producer started with is `Revoked` after
  the run;
- each batch's hops are causally chained *by stamps alone*;
- with one region, each batch causally follows the previous batch's
  return;
- with several regions, some events are genuinely concurrent;
- the Lamport total order is a linear extension of causality;
- an empty pool fails with a typed `Stalled` error instead of spinning.

### Two workload bugs the tests caught, reported plainly

1. **The pipeline was fully serialized.** Stages were first spawned
   upstream-first, and round-robin follows spawn order. So each batch
   went all the way around the ring in one round, and the producer
   drained the return before sending the next batch. Every event landed
   in one total order. The vector clocks were right; the schedule simply
   had no concurrency to show. The test
   `with_several_regions_some_batches_are_genuinely_concurrent` failed
   on it. Fixed by spawning downstream-first, so each round advances
   every in-flight batch one stage (real pipelining).
2. **Only one region was ever used.** The producer's free list was a
   LIFO stack, so the most recently returned region was reused every
   time and the rest of the pool sat idle. It was caught because
   `every_region_capability_the_producer_started_with_is_dead_afterward`
   found live capabilities for regions that never moved. Fixed with a
   FIFO pool.

Neither was a kernel bug. Both would have made the benchmark and the
causality claims quietly meaningless.

### Part 2: the ambient-authority baseline, and differential testing

`baseline::run_baseline` is the same pipeline with **no kernel**: plain
`VecDeque` channels any stage can touch, and regions as boxed slices
moved by ordinary Rust moves. Clocks are optional. Topology, schedule
(the supervisor's no-op turn included), FIFO pool and the `data`
functions are identical, so the benchmark difference is the discipline
and not a different algorithm. This was chosen over "stub out
`Kernel::check` with a feature flag": that would have left every other
piece of kernel bookkeeping in place (process tables, grant resolution,
epochs), and Cargo's feature unification makes a stubbed kernel easy to
build by accident.

`tests/differential.rs` runs the kernel pipeline, the baseline with and
without clocks, and `data::reference_checksum` over arbitrary batch
counts, region sizes, pool sizes (including 0), stage counts (including
0) and seeds. Results must match *in order*, scheduler statistics must be
identical, and stalls must happen at the same turn.

### Part 3: the chaos harness

`chaos::run_chaos(seed, steps)` drives every kernel operation at random,
with up to 8 processes and 3 channels:
- spawns and sends with random `Transfer`/`Derive`/`Move` batches,
  including duplicates, move conflicts and amplification attempts;
- receives;
- reads and writes, including out-of-bounds ones and ones made with
  channel capabilities;
- revocation and destruction, including of regions whose move is still
  in flight;
- **stale replay**, where every capability any process ever held is
  kept and retried.

A shadow model written independently of the kernel predicts the
**exact** `Result` of each operation (variant and fields). After *every*
step the harness audits:
- every process table against its mirror, handle by handle;
- the liveness of every capability ever seen;
- the bytes of every region that can be audited;
- the allocated-region count.

At the end, every pair of events is checked against the ground-truth
causal graph. A panic inside the kernel is caught and reported as a
failure like any other. Runs are a pure function of the seed: the PRNG
is SplitMix64 (hand-rolled, so a recorded seed can't be changed by a
dependency upgrade), and handles are sorted, never taken from `HashMap`
iteration order. The `muaddib-chaos` binary runs seed ranges and prints
the exact replay command on failure.

**Does it catch real bugs?** A harness that has never failed proves
nothing, so it was run with six deliberately re-introduced kernel bugs
(200 seeds × 400 steps each):

| Mutation | Seeds failing |
|---|---|
| `Grant::Move` doesn't reissue (the original ticket-004 hole) | 196/200 |
| `revoke` checks existence only (the original ticket-001 hole) | 200/200 |
| no duplicate-transfer check (the original ticket-002 hole) | 169/200 |
| no `MoveConflict` check | 93/200 |
| `read` requires no rights | 39/200 |
| IPC receive doesn't merge the sender's stamp | 200/200 |

**A harness gap this exposed.** The `MoveConflict` mutation first showed
up as a *crash*, not a reported failure: a second reissue hit an
`expect` inside `apply_grants`. A crash doesn't print its seed, which
undermines replayability, so `run_chaos` now catches panics and converts
them into a `ChaosFailure` with seed, step and op. The first coverage run
also showed the harness almost never generated a *successful* write
(0 of 45). Accesses were biased toward plausible spans after that, and
`the_harness_exercises_every_operation_both_ways` now requires every
operation to both succeed and be rejected at least once.

**Unmutated kernel:** 500 seeds × 1000 steps clean, with 133,696 causal
events checked and 48,212 stale replays correctly refused. Seed replay is
itself tested: an injected model corruption fails at the same step with
the same op and message on every run.

## Measurements

`cargo bench -p workload --bench cost_of_security` (Criterion, release
build, Windows 11 laptop, warm-up 1 s / measurement 3–4 s). The pipeline
has 200 batches, 3 transformer stages and 4 regions, so 1,000 messages
and 1,000 region moves per run.

| Pipeline run | 64-byte regions | 4096-byte regions |
|---|---|---|
| ambient, no clocks | 40.9 µs | 2.161 ms |
| ambient + logical clocks | 177.6 µs (4.3×) | 2.329 ms (1.08×) |
| **full kernel** | **712.5 µs (17.4×)** | **2.922 ms (1.35×)** |

| Primitive | Kernel | Ambient |
|---|---|---|
| `Kernel::check` (live) | 8.95 ns | 0.32 ns (no-op) |
| `Kernel::check` (revoked) | 8.75 ns | n/a |
| region `update`, 64 B | 19.1 ns | 1.53 ns |
| one send+receive moving a region, sender knows 1 process | 495 ns | 2.41 ns (queue push/pop) |
| same, 16 processes | 926 ns | n/a |
| same, 256 processes | 9.09 µs | n/a |

**What the numbers say, without spin:**

- **For small messages, the discipline is expensive: 17.4× end to end.**
  At page-sized regions the real data work dominates and it shrinks to
  1.35×. The cost is per *hop*, not per byte.
- **Capability checks are not where it goes.** A check is ~9 ns, a
  `HashMap` lookup with the default SipHash hasher. A hop does roughly
  half a dozen, so about 50 ns of the ~535 ns per hop the kernel adds on
  top of clocks. **This is an estimate, not a profile.** The rest is
  plausibly per-hop bookkeeping: `resolve_grants` allocates a `Vec` and a
  `HashSet` per send, the stamp is cloned into the message and again as
  the return value, and there are process-table `HashMap` inserts and
  lookups. Profiling and fixing this is left as a follow-up, not claimed
  here.
- **Logical clocks alone cost 4.3× on small messages,** more than
  capabilities' share. Every event clones a `BTreeMap` vector.
- **Vector clocks scale linearly and it shows:** a moved-region hop goes
  from 495 ns (1 known process) to 926 ns (16) to 9.09 µs (256), 18× from
  1 to 256. Removing the wall clock has a real price that grows with the
  number of processes that have talked to each other. Known mitigations
  (sparse deltas, interval tree clocks, pruning dead processes) are not
  implemented.
- **Revoked and live checks cost the same** (8.75 vs 8.95 ns). Epoch
  revocation adds nothing to the fast path, which confirms ADR-001's
  design at the hardware level.

## Alternatives Considered

1. **Stub `Kernel::check` behind a feature for the baseline.** Rejected
   (see Part 2): it measures the cost of `check`, not of the discipline,
   and feature unification makes it fragile.
2. **`rand` with a seeded `StdRng` for chaos.** Rejected: its stream is
   not guaranteed stable across versions, so a recorded seed could stop
   reproducing after a dependency bump.
3. **proptest for chaos instead of a custom harness.** proptest shrinks
   well but its seeds are tied to its own runner. A standalone
   `muaddib-chaos --seed N` is what DoD item 9 asks for. Both are used:
   proptest for bounded properties, the harness for long random
   schedules.

## Consequences

- Ticket 006 closes; every muaddib ticket is done and CORE + EXTENSION
  scope is complete.
- Two concrete performance follow-ups are measured rather than assumed:
  per-hop allocation in grant resolution and stamp cloning, and O(n)
  vector clocks.
- **The chaos model is itself code that could be wrong.** It was checked
  by mutation (every mutation caught) and by fault injection, not proven.
  A bug shared by model and kernel would go unseen; writing the model
  from the ADRs rather than from the kernel code is the only mitigation.
- The benchmarks time the kernel from outside with Criterion (which uses
  a timer). `no_wall_clock` scans only kernel `src/` and deliberately
  excludes `benches/`.
- The EXPERIMENT scope (a capability-secured directory-like naming layer;
  logical-clock-driven preemptive scheduling) is not attempted, as with
  chakobsa's EXPERIMENT scope.
