# ADR-003: IPC reuses ticket 002's grant machinery; messages carry authority through a place no process's table covers

## Status
Accepted

## Context

Ticket 003 needed channels processes can send/receive over, with
capabilities themselves as transferable payload — the feature that lets
authority move at runtime, not only at spawn time. Two design questions,
one of them only visible once implementation started: what a "channel"
is in a system with no built-in resource types beyond `capability`'s
generic objects, and how a sent-but-not-yet-received capability's
authority is represented while it's genuinely in flight, belonging to
neither the sender nor the receiver.

## Decision

**A channel is an ordinary `capability` object; `Rights::WRITE` means
"may send," `Rights::READ` means "may receive."** Ticket 001's `Rights`
bits were deliberately defined without prescribed meaning
(`rights.rs`'s doc comment says as much); this ticket is the first to
give two of them real semantics. `ChannelRegistry::new_channel` mints a
capability with every bit set (`READ|WRITE|GRANT|DESTROY`) — the
creator can then `Kernel::derive` send-only or receive-only views to
hand to other processes via the exact same `process::Grant` mechanism
used for everything else.

**Sending reuses ticket 002's `resolve_grants`/`apply_transfers`
directly — not a parallel implementation.** This was a real discovery
made *while implementing this ticket*, not planned from ticket 002:
"spawn a child with attenuated authority" and "send a capability over a
channel" are the same operation shape — resolve a batch of
`Transfer`/`Derive` requests against a source process's table,
all-or-nothing, then hand the results to a destination. `process::
transfer` was factored out of `Scheduler::spawn_child` specifically so
`ipc::send` could call the identical, already-tested logic instead of a
second copy that could silently drift from the first. `ChannelRegistry::
send`'s all-or-nothing guarantee (a rejected batch touches the sender's
table not at all) is inherited directly from this shared code, not
re-implemented.

**A message's capabilities live in the `ChannelRegistry`'s own queue,
in neither the sender's nor the receiver's `Process` table, for as long
as the message is unreceived.** `send` calls `apply_transfers` (removing
a `Transfer`'d capability from the sender) before the message is even
queued; `receive` only calls `Process::grant` (the *only* way a
capability ever enters a table, ticket 002's ADR-002) once a message is
actually popped. There is no third place a capability could be
"double-counted" — `an_in_flight_capability_exists_in_no_process_table`
proves this state exists and is exactly what it should be, not an
oversight where the capability is secretly still reachable somewhere.

## Alternatives Considered
1. A separate `Kernel`-mediated "in-flight" object per message —
   rejected as unnecessary machinery: a plain `VecDeque<Message>` per
   channel object already gives the registry everywhere it needs to
   look, and `capability::Kernel` intentionally never stores payload
   data (see `ipc`'s own module doc — the same separation of concerns
   sietch's `Store` keeps between its data and its own internal
   bookkeeping).
2. Re-implementing grant resolution inside `ipc` rather than factoring
   `process::transfer` out — rejected once the duplication became
   obvious during implementation; two independently-maintained copies
   of "attenuate-or-transfer, all-or-nothing" is exactly the kind of
   drift risk this project's ADRs exist to call out and avoid, not
   quietly accept for expedience.

## Consequences

- Ticket 003 closes. Ticket 004 (memory ownership) can send a memory
  region's owning capability through a channel using the exact same
  `Grant::Transfer` path already proven here — no new transfer
  mechanism needed, only a new object *kind* (a memory region) riding
  on the same rails.
- `process::transfer` is now public API surface two crates depend on
  (`process`'s own `Scheduler` and `ipc`'s `ChannelRegistry`) — a
  future change to its all-or-nothing contract must account for both
  call sites, not just one.
- Channels have no capacity bound and no blocking-receive semantics
  (`receive` returns `Ok(None)` immediately on an empty queue rather
  than waiting) — a real, current limitation appropriate for this
  ticket's scope (proving capability-carrying messages work), tracked
  as a gap a future scheduling-integration ticket would need to close
  before this could model realistic backpressure.
