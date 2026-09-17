//! Runs the chaos harness from the command line.
//!
//! ```text
//! muaddib-chaos [--seed N] [--runs R] [--steps S] [--inject-fault STEP]
//! ```
//!
//! Runs seeds `N, N+1, ..., N+R-1` for `S` steps each. On the first
//! failure it prints the failure, including the exact command that replays
//! it, and exits 1. Numbers may be decimal or `0x`-prefixed hex.

use std::process::ExitCode;

use workload::chaos::{run_chaos, ChaosConfig, Fault};

fn parse_u64(flag: &str, value: Option<String>) -> Result<u64, String> {
    let value = value.ok_or_else(|| format!("{flag} needs a value"))?;
    let parsed = match value.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => value.parse(),
    };
    parsed.map_err(|e| format!("{flag} {value}: {e}"))
}

fn main() -> ExitCode {
    let mut seed = 0u64;
    let mut runs = 1u64;
    let mut steps = 1000usize;
    let mut fault = None;

    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let result = match flag.as_str() {
            "--seed" => parse_u64(&flag, args.next()).map(|v| seed = v),
            "--runs" => parse_u64(&flag, args.next()).map(|v| runs = v),
            "--steps" => parse_u64(&flag, args.next()).map(|v| steps = v as usize),
            "--inject-fault" => parse_u64(&flag, args.next())
                .map(|v| fault = Some(Fault::CorruptModelAt { step: v as usize })),
            "--help" | "-h" => {
                println!("muaddib-chaos [--seed N] [--runs R] [--steps S] [--inject-fault STEP]");
                return ExitCode::SUCCESS;
            }
            other => Err(format!("unknown argument {other}")),
        };
        if let Err(e) = result {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    }

    let mut events = 0usize;
    let mut stale_refused = 0u64;
    for i in 0..runs {
        let config = ChaosConfig {
            seed: seed.wrapping_add(i),
            steps,
            fault,
        };
        match run_chaos(config) {
            Ok(summary) => {
                events += summary.events;
                stale_refused += summary.stale_replays_refused;
                if runs == 1 {
                    println!("seed {:#x}: {} steps clean", summary.seed, summary.steps);
                    for (op, (ok, rejected)) in &summary.outcomes {
                        println!("  {op:<10} {ok:>6} ok  {rejected:>6} correctly rejected");
                    }
                    println!("  causal events checked: {}", summary.events);
                    println!("  stale replays refused: {}", summary.stale_replays_refused);
                    println!("  log hash: {:#018x}", summary.log_hash);
                }
            }
            Err(failure) => {
                eprintln!("{failure}");
                return ExitCode::FAILURE;
            }
        }
    }
    if runs > 1 {
        println!(
            "{runs} seeds x {steps} steps clean (seeds {seed:#x}..{:#x}); {events} causal events checked, {stale_refused} stale replays refused",
            seed.wrapping_add(runs - 1)
        );
    }
    ExitCode::SUCCESS
}
