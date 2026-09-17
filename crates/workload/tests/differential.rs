//! Differential test: three independent computations of the same
//! workload must agree for arbitrary configurations.
//!
//! 1. the kernel pipeline (capabilities, spawns, IPC, moves, clocks);
//! 2. the ambient-authority baseline, with and without clocks;
//! 3. `data::reference_checksum`, with no pipeline at all.
//!
//! Results must match *in order*, and the kernel pipeline and baseline
//! must produce identical scheduler statistics. So the capability
//! discipline changes nothing about what the workload computes or how
//! it is scheduled, only what it costs.

use proptest::prelude::*;
use workload::baseline::run_baseline;
use workload::data::reference_checksum;
use workload::{run_kernel_pipeline, PipelineConfig, PipelineError};

fn arb_config() -> impl Strategy<Value = PipelineConfig> {
    (
        0u64..40,
        1usize..48,
        0usize..6,
        0usize..5,
        any::<u64>(),
        any::<bool>(),
    )
        .prop_map(
            |(batches, region_size, pool, stages, seed, trace)| PipelineConfig {
                batches,
                region_size,
                pool,
                stages,
                seed,
                trace,
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn kernel_pipeline_baseline_and_reference_agree(cfg in arb_config()) {
        let kernel = run_kernel_pipeline(cfg);
        let ambient = run_baseline(cfg, false);
        let ambient_clocked = run_baseline(cfg, true);

        match (kernel, ambient, ambient_clocked) {
            (Ok(k), Ok(a), Ok(ac)) => {
                prop_assert_eq!(&k.results, &a.results);
                prop_assert_eq!(&k.results, &ac.results);
                prop_assert_eq!(&k.stats, &a.stats);
                prop_assert_eq!(&k.stats, &ac.stats);
                prop_assert_eq!(k.results.len() as u64, cfg.batches);
                for r in &k.results {
                    prop_assert_eq!(
                        r.checksum,
                        reference_checksum(cfg.seed, r.batch, cfg.region_size, cfg.stages)
                    );
                }
            }
            (Err(PipelineError::Stalled { turns: kt }), Err(PipelineError::Stalled { turns: at }), Err(_)) => {
                // Only an empty pool with work to do can stall, and both
                // must give up at the same point.
                prop_assert!(cfg.pool == 0 && cfg.batches > 0);
                prop_assert_eq!(kt, at);
            }
            (k, a, ac) => prop_assert!(
                false,
                "diverged: kernel {:?} / ambient {:?} / ambient+clocks {:?}",
                k.map(|r| r.results.len()),
                a.map(|r| r.results.len()),
                ac.map(|r| r.results.len())
            ),
        }
    }
}
