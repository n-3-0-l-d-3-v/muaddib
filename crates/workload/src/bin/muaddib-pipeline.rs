//! Runs the integration pipeline and prints what happened: statistics, and
//! the stamped trace in Lamport total order. That order was reconstructed
//! from logical clocks alone; nothing in the run reads physical time.
//!
//! ```text
//! muaddib-pipeline [--batches N] [--stages S] [--pool P] [--region-size B] [--seed X] [--trace]
//! ```

use std::process::ExitCode;

use workload::data::reference_checksum;
use workload::{run_kernel_pipeline, PipelineConfig};

fn main() -> ExitCode {
    let mut config = PipelineConfig {
        batches: 8,
        region_size: 256,
        pool: 3,
        stages: 3,
        seed: 1,
        trace: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        if flag == "--trace" {
            config.trace = true;
            continue;
        }
        let Some(Ok(value)) = args.next().map(|v| v.parse::<u64>()) else {
            eprintln!("error: {flag} needs a numeric value");
            return ExitCode::from(2);
        };
        match flag.as_str() {
            "--batches" => config.batches = value,
            "--stages" => config.stages = value as usize,
            "--pool" => config.pool = value as usize,
            "--region-size" => config.region_size = value as usize,
            "--seed" => config.seed = value,
            other => {
                eprintln!("error: unknown argument {other}");
                return ExitCode::from(2);
            }
        }
    }

    let report = match run_kernel_pipeline(config) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("pipeline failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    let correct = report.results.iter().all(|r| {
        r.checksum == reference_checksum(config.seed, r.batch, config.region_size, config.stages)
    });
    println!("{config:?}");
    println!("{:?}", report.stats);
    println!(
        "{} batches, checksums {}",
        report.results.len(),
        if correct {
            "all match reference"
        } else {
            "MISMATCH"
        }
    );

    if config.trace {
        let mut trace = report.trace;
        trace.sort_by_key(|e| e.stamp.total_order_key());
        println!("trace (Lamport total order):");
        for e in trace {
            println!(
                "  L={:<4} {:?} {:<14} batch {:<3} {:?}",
                e.stamp.lamport(),
                e.action,
                format!("{:?}", e.role),
                e.batch,
                e.stamp.vector().entries().collect::<Vec<_>>()
            );
        }
    }
    if correct {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
