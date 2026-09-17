//! The chaos harness, run as part of the normal test suite. Longer runs
//! go through the `muaddib-chaos` binary; see ADR-006 for the
//! mutation-testing record showing this harness catches real kernel bugs.

use workload::chaos::{run_chaos, ChaosConfig, Fault};

fn config(seed: u64, steps: usize) -> ChaosConfig {
    ChaosConfig {
        seed,
        steps,
        fault: None,
    }
}

#[test]
fn many_seeds_run_clean() {
    for seed in 0..48 {
        if let Err(failure) = run_chaos(config(seed, 300)) {
            panic!("{failure}");
        }
    }
}

#[test]
fn a_run_is_a_pure_function_of_its_seed() {
    let a = run_chaos(config(0xDEAD_BEEF, 400)).unwrap();
    let b = run_chaos(config(0xDEAD_BEEF, 400)).unwrap();
    assert_eq!(a, b);
    let c = run_chaos(config(0xDEAD_BEF0, 400)).unwrap();
    assert_ne!(a.log_hash, c.log_hash);
}

#[test]
fn an_injected_fault_is_caught_and_replays_identically_from_its_seed() {
    let faulty = ChaosConfig {
        seed: 77,
        steps: 500,
        fault: Some(Fault::CorruptModelAt { step: 40 }),
    };
    let first = run_chaos(faulty).expect_err("the audit must notice the corrupted byte");
    assert!(first.step >= 40);
    assert!(first.message.contains("bytes"), "{first}");

    let replay = run_chaos(faulty).expect_err("replay must fail too");
    assert_eq!(
        first, replay,
        "same seed, same failure: same step, op, message"
    );
    assert!(first
        .to_string()
        .contains("replay: muaddib-chaos --seed 0x4d --steps 500"));
}

#[test]
fn the_harness_exercises_every_operation_both_ways() {
    let mut totals = std::collections::BTreeMap::<&str, (u64, u64)>::new();
    let mut stale_refused = 0;
    for seed in 100..116 {
        let summary = run_chaos(config(seed, 300)).unwrap();
        stale_refused += summary.stale_replays_refused;
        for (op, (ok, rejected)) in summary.outcomes {
            let t = totals.entry(op).or_default();
            t.0 += ok;
            t.1 += rejected;
        }
    }
    for op in [
        "spawn", "send", "receive", "read", "write", "revoke", "destroy",
    ] {
        let (ok, rejected) = totals[op];
        assert!(
            ok > 0 && rejected > 0,
            "{op}: {ok} ok / {rejected} rejected"
        );
    }
    assert!(totals["new_region"].0 > 0 && totals["local"].0 > 0);
    assert!(stale_refused > 0);
}
