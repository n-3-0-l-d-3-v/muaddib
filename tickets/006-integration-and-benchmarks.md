---
status: done
phase: 4
---

# 006 — Integration workload, differential testing, and benchmarks

The Definition-of-Done items this repo hadn't earned even once tickets
001–005 closed: a real, observable multi-subsystem workload, and measured
performance.

## Scope
- [x] A real multi-process pipeline exercising every earlier ticket
      together — `workload::KernelPipeline`. A supervisor spawns the
      producer, transformer stages and sink with attenuated,
      least-privilege channel views (002). A pool of memory regions
      circulates the ring, and every hop is a `Grant::Move` over IPC
      (003, 004). Every send and receive is stamped, and the tests check
      the causal structure from stamps alone (005). No physical time is
      read anywhere (`no_wall_clock` guard covers this crate too).
- [x] Comparison: capability overhead against an ambient-authority
      baseline, `workload::baseline::run_baseline`, running the same
      workload with no kernel and optional clocks. Measured, not assumed:
      17.4× end to end at 64-byte regions, 1.35× at 4096-byte regions;
      clocks alone account for 4.3×. `Kernel::check` costs ~9 ns, whether
      the capability is live or revoked. Vector-clock cost grows from
      495 ns to 9.09 µs per moved-region hop between 1 and 256 known
      processes. Full numbers in ADR-006.
- [x] Chaos/fuzz testing: arbitrary interleavings and revocation
      mid-transfer, checked against an independent shadow model after
      every step, with any failure reproducible from a recorded seed —
      `workload::chaos::run_chaos` and the `muaddib-chaos --seed N
      --steps S` binary. 500 seeds × 1000 steps clean. It catches all six
      deliberately re-introduced kernel bugs, and seed replay of an
      injected fault is itself tested.

## Also done
- [x] Differential test: kernel pipeline, baseline with and without
      clocks, and a direct reference computation agree on results (in
      order) and scheduler statistics for arbitrary configurations.
- [x] Observability: pipeline `Stats`, a stamped trace, and
      `muaddib-pipeline --trace` printing it in Lamport total order;
      per-op outcome counts and stale-replay refusals in `ChaosSummary`.
- [x] Kernel API improvements the workload needed: `Delivery::handles`,
      and zero-copy `RegionRegistry::inspect`/`update`.
- [x] Two workload bugs found by the tests and fixed (a fully serialized
      schedule, and a LIFO pool leaving regions idle), plus one harness
      gap (kernel panics weren't seed-reported). All in ADR-006.

Workspace: 124 tests, all passing. See
`docs/design/decisions/ADR-006-integration-and-benchmarks.md`.
