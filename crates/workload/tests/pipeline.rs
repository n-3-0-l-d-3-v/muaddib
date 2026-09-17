//! The integration workload's own guarantees: correct output, least
//! privilege, ownership that really moves, and a causal structure
//! recoverable from stamps alone.

use std::collections::HashMap;

use capability::{CapError, Rights};
use ipc::IpcError;
use workload::data::reference_checksum;
use workload::{
    run_kernel_pipeline, Action, KernelPipeline, PipelineConfig, PipelineError, Role, TraceEvent,
};

fn config(batches: u64, pool: usize, stages: usize) -> PipelineConfig {
    PipelineConfig {
        batches,
        region_size: 64,
        pool,
        stages,
        seed: 0xA11CE,
        trace: true,
    }
}

#[test]
fn every_batch_is_processed_exactly_once_with_the_reference_checksum() {
    let cfg = config(40, 3, 4);
    let report = run_kernel_pipeline(cfg).unwrap();
    assert_eq!(report.results.len(), 40);
    let mut seen = [false; 40];
    for r in &report.results {
        assert!(!seen[r.batch as usize], "batch {} twice", r.batch);
        seen[r.batch as usize] = true;
        assert_eq!(
            r.checksum,
            reference_checksum(cfg.seed, r.batch, cfg.region_size, cfg.stages)
        );
    }
}

#[test]
fn stats_account_for_every_hop() {
    let cfg = config(10, 2, 3);
    let report = run_kernel_pipeline(cfg).unwrap();
    // producer send + one send per transformer + the sink's return.
    let hops = (cfg.stages as u64) + 2;
    assert_eq!(report.stats.messages, cfg.batches * hops);
    assert_eq!(
        report.stats.bytes_processed,
        cfg.batches * hops * cfg.region_size as u64
    );
    assert!(report.stats.scheduler_turns >= report.stats.messages / hops);
}

/// The events for one batch, in the order the pipeline must have caused them.
fn hop_sequence(stages: usize) -> Vec<(Role, Action)> {
    let mut seq = vec![(Role::Producer, Action::Send)];
    for i in 0..stages {
        seq.push((Role::Transformer(i), Action::Receive));
        seq.push((Role::Transformer(i), Action::Send));
    }
    seq.push((Role::Sink, Action::Receive));
    seq.push((Role::Sink, Action::Send));
    seq
}

#[test]
fn each_batchs_hops_are_causally_chained_by_stamps_alone() {
    let cfg = config(12, 2, 3);
    let report = run_kernel_pipeline(cfg).unwrap();
    let index: HashMap<(u64, Role, Action), &TraceEvent> = report
        .trace
        .iter()
        .map(|e| ((e.batch, e.role, e.action), e))
        .collect();

    for batch in 0..cfg.batches {
        let chain: Vec<&TraceEvent> = hop_sequence(cfg.stages)
            .into_iter()
            .map(|(role, action)| index[&(batch, role, action)])
            .collect();
        for pair in chain.windows(2) {
            assert!(
                pair[0].stamp.happened_before(&pair[1].stamp),
                "batch {batch}: {:?} {:?} should precede {:?} {:?}",
                pair[0].role,
                pair[0].action,
                pair[1].role,
                pair[1].action
            );
        }
    }
}

#[test]
fn with_one_region_each_batch_causally_follows_the_previous_ones_return() {
    let cfg = config(8, 1, 2);
    let report = run_kernel_pipeline(cfg).unwrap();
    let find = |batch, role, action| {
        report
            .trace
            .iter()
            .find(|e| e.batch == batch && e.role == role && e.action == action)
            .unwrap()
    };
    for batch in 1..cfg.batches {
        let returned = find(batch - 1, Role::Sink, Action::Send);
        let next_sent = find(batch, Role::Producer, Action::Send);
        assert!(returned.stamp.happened_before(&next_sent.stamp));
    }
}

#[test]
fn with_several_regions_some_batches_are_genuinely_concurrent() {
    // Not a correctness requirement, but evidence the vector clocks carry
    // real information: with 4 regions in flight, some transformer events
    // for different batches are causally unrelated.
    let report = run_kernel_pipeline(config(20, 4, 3)).unwrap();
    let transformer_events: Vec<&TraceEvent> = report
        .trace
        .iter()
        .filter(|e| matches!(e.role, Role::Transformer(_)))
        .collect();
    let concurrent = transformer_events.iter().any(|a| {
        transformer_events
            .iter()
            .any(|b| a.role != b.role && a.stamp.vector().concurrent_with(b.stamp.vector()))
    });
    assert!(concurrent);
}

#[test]
fn lamport_total_order_is_a_linear_extension_of_causality() {
    let mut trace = run_kernel_pipeline(config(15, 3, 2)).unwrap().trace;
    trace.sort_by_key(|e| e.stamp.total_order_key());
    for (i, earlier) in trace.iter().enumerate() {
        for later in &trace[i + 1..] {
            assert!(!later.stamp.happened_before(&earlier.stamp));
        }
    }
}

#[test]
fn every_stage_holds_only_least_privilege_channel_views() {
    let cfg = config(0, 2, 2);
    let pipeline = KernelPipeline::new(cfg).unwrap();
    let sched = pipeline.scheduler();

    for role in [Role::Transformer(0), Role::Transformer(1), Role::Sink] {
        let proc = sched.process(pipeline.process_of(role).unwrap()).unwrap();
        let mut rights: Vec<Rights> = proc
            .handles()
            .map(|h| proc.capability(h).unwrap().rights())
            .collect();
        rights.sort_by_key(|r| r.to_string());
        assert_eq!(rights, vec![Rights::READ, Rights::WRITE], "{role:?}");
    }

    let producer = sched
        .process(pipeline.process_of(Role::Producer).unwrap())
        .unwrap();
    assert_eq!(producer.handle_count(), 2 + cfg.pool);

    // The supervisor moved every region away: it holds channels only.
    let supervisor = sched
        .process(pipeline.process_of(Role::Supervisor).unwrap())
        .unwrap();
    assert_eq!(supervisor.handle_count(), cfg.stages + 2);
}

#[test]
fn a_stage_cannot_use_its_channel_views_the_wrong_way() {
    let cfg = config(0, 1, 1);
    let mut pipeline = KernelPipeline::new(cfg).unwrap();
    let pid = pipeline.process_of(Role::Transformer(0)).unwrap();
    let proc = pipeline.scheduler().process(pid).unwrap();
    let read_view = proc
        .handles()
        .map(|h| proc.capability(h).unwrap())
        .find(|c| c.rights() == Rights::READ)
        .unwrap();

    // Sending on the input channel (it only holds READ) must fail.
    let err = pipeline.try_send_as(Role::Transformer(0), &read_view, vec![], 0);
    assert!(matches!(
        err,
        Err(PipelineError::Ipc(IpcError::Capability(
            CapError::InsufficientRights { .. }
        )))
    ));
    // And it can't re-delegate: deriving needs GRANT.
    assert!(pipeline.kernel().derive(&read_view, Rights::READ).is_err());
}

#[test]
fn every_region_capability_the_producer_started_with_is_dead_afterward() {
    let cfg = config(9, 3, 2);
    let mut pipeline = KernelPipeline::new(cfg).unwrap();
    let producer = pipeline.process_of(Role::Producer).unwrap();
    let proc = pipeline.scheduler().process(producer).unwrap();
    let original_region_caps: Vec<_> = proc
        .handles()
        .map(|h| proc.capability(h).unwrap())
        .filter(|c| c.rights().contains(Rights::DESTROY))
        .collect();
    assert_eq!(original_region_caps.len(), cfg.pool);

    pipeline.run().unwrap();

    for cap in original_region_caps {
        assert_eq!(
            pipeline.kernel().check(&cap, Rights::NONE),
            Err(CapError::Revoked(cap.object())),
            "a region the producer used must have moved on"
        );
        assert!(pipeline
            .memory()
            .inspect(pipeline.kernel(), &cap, |_| ())
            .is_err());
    }
}

#[test]
fn an_empty_pool_stalls_with_a_typed_error_instead_of_spinning() {
    assert!(matches!(
        run_kernel_pipeline(config(1, 0, 1)),
        Err(PipelineError::Stalled { .. })
    ));
}

#[test]
fn zero_batches_is_an_immediate_empty_run() {
    let report = run_kernel_pipeline(config(0, 1, 1)).unwrap();
    assert!(report.results.is_empty());
    assert_eq!(report.stats.scheduler_turns, 0);
}

#[test]
fn zero_transformer_stages_still_works() {
    let cfg = config(5, 1, 0);
    let report = run_kernel_pipeline(cfg).unwrap();
    for r in report.results {
        assert_eq!(r.checksum, reference_checksum(cfg.seed, r.batch, 64, 0));
    }
}
