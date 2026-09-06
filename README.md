# impossible-kernel — THE KERNEL

> A capability-based OS with no filesystem hierarchy and no wall clock.

Part of **[The Impossible Computer](https://github.com/n-3-0-l-d-3-v/impossible-computer)** — a constrained computing
ecosystem built by removing assumptions ordinary computers depend on. This
repository is developed standalone and mirrored into the combined ecosystem
repo commit-for-commit.

## Status

**Phase 4 — QUEUED**

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

- [impossible-machine](https://github.com/n-3-0-l-d-3-v/impossible-machine) — THE MACHINE (ACTIVE)
- [impossible-language](https://github.com/n-3-0-l-d-3-v/impossible-language) — THE LANGUAGE (QUEUED)
- [impossible-vault](https://github.com/n-3-0-l-d-3-v/impossible-vault) — THE VAULT (QUEUED)
- [impossible-database](https://github.com/n-3-0-l-d-3-v/impossible-database) — THE DATABASE (QUEUED)
- [impossible-wire](https://github.com/n-3-0-l-d-3-v/impossible-wire) — THE WIRE (QUEUED)
- [impossible-colony](https://github.com/n-3-0-l-d-3-v/impossible-colony) — THE COLONY (QUEUED)
- [impossible-history](https://github.com/n-3-0-l-d-3-v/impossible-history) — THE HISTORY (QUEUED)
- [impossible-artifact](https://github.com/n-3-0-l-d-3-v/impossible-artifact) — THE ARTIFACT (STRETCH)

## Development

This is a real, tested, benchmarked systems component — not a demo. See
[docs/DEFINITION_OF_DONE.md](docs/DEFINITION_OF_DONE.md) for the acceptance
bar every piece of this repo must clear before it is considered complete.

```bash
cargo build
cargo test
cargo bench
```
