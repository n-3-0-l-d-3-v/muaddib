//! Ticket 006: THE KERNEL's closing workload.
//!
//! - `pipeline`: a real multi-process pipeline that uses every earlier
//!   ticket at once (attenuated spawns, regions moved over IPC, causal
//!   stamps) with no physical time anywhere.
//! - `baseline`: the same pipeline with ambient authority. No
//!   capabilities, no process tables, optionally no clocks. It exists to
//!   give a measured cost-of-security number, and as a differential-testing
//!   reference.
//! - `chaos`: a seeded chaos harness. Arbitrary interleavings of spawns,
//!   moves, derives, revocations mid-transfer, destruction and stale
//!   replays, checked against a shadow model. Every failure is replayable
//!   from its seed.
//!
//! See `docs/design/decisions/ADR-006-integration-and-benchmarks.md`.

pub mod data;
pub mod pipeline;
pub mod rng;

pub use pipeline::{
    run_kernel_pipeline, Action, BatchResult, KernelPipeline, PipelineConfig, PipelineError,
    PipelineReport, Role, Stats, TraceEvent,
};
