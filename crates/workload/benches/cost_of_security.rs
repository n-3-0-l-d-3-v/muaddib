//! Ticket 006's comparison: what does the kernel's discipline cost?
//!
//! - `pipeline/*`: the integration workload end to end, full kernel vs
//!   ambient authority (with and without logical clocks), at a small and a
//!   page-sized region. Kernel setup (boot, spawns) is excluded via
//!   `iter_batched`; only `run` is timed.
//! - `check/*`, `region_access/*`: the per-operation primitives.
//! - `ipc_move/*`: one send+receive carrying a moved region, against a
//!   plain queue push/pop of a boxed slice, with the sender's vector clock
//!   knowing about 1, 16 and 256 processes. Each send clones that vector,
//!   so this is where O(processes) should show up if it matters.
//!
//! This harness times the kernel from the outside, so the wall-clock ban
//! (`clock/tests/no_wall_clock.rs`) deliberately doesn't cover `benches/`.

use std::collections::VecDeque;
use std::hint::black_box;
use std::time::Duration;

use capability::{Kernel, Rights};
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};
use ipc::ChannelRegistry;
use memory::RegionRegistry;
use process::{Grant, Scheduler};
use workload::baseline::run_baseline;
use workload::{KernelPipeline, PipelineConfig};

fn pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("pipeline");
    group.measurement_time(Duration::from_secs(4));
    for region_size in [64usize, 4096] {
        let cfg = PipelineConfig {
            batches: 200,
            region_size,
            pool: 4,
            stages: 3,
            seed: 7,
            trace: false,
        };
        group.throughput(Throughput::Elements(cfg.batches));
        group.bench_with_input(BenchmarkId::new("kernel", region_size), &cfg, |b, cfg| {
            b.iter_batched(
                || KernelPipeline::new(*cfg).unwrap(),
                |mut p| black_box(p.run().unwrap()),
                BatchSize::SmallInput,
            )
        });
        group.bench_with_input(
            BenchmarkId::new("ambient_with_clocks", region_size),
            &cfg,
            |b, cfg| b.iter(|| black_box(run_baseline(*cfg, true).unwrap())),
        );
        group.bench_with_input(
            BenchmarkId::new("ambient_no_clocks", region_size),
            &cfg,
            |b, cfg| b.iter(|| black_box(run_baseline(*cfg, false).unwrap())),
        );
    }
    group.finish();
}

fn primitives(c: &mut Criterion) {
    let mut k = Kernel::new();
    let live = k.new_object(Rights::ALL);
    let stale = k.new_object(Rights::ALL);
    k.revoke(&stale).unwrap();

    let mut group = c.benchmark_group("check");
    group.bench_function("live_capability", |b| {
        b.iter(|| black_box(k.check(black_box(&live), Rights::READ | Rights::WRITE)))
    });
    group.bench_function("revoked_capability", |b| {
        b.iter(|| black_box(k.check(black_box(&stale), Rights::READ)))
    });
    group.bench_function("ambient_no_check", |b| {
        b.iter(|| black_box(Ok::<(), ()>(())))
    });
    group.finish();

    let mut mem = RegionRegistry::new();
    let region = mem.new_region(&mut k, 64);
    let mut plain = vec![0u8; 64].into_boxed_slice();
    let mut group = c.benchmark_group("region_access");
    group.bench_function("kernel_update_64b", |b| {
        b.iter(|| {
            mem.update(&k, &region, |bytes| bytes[black_box(7)] ^= 1)
                .unwrap()
        })
    });
    group.bench_function("ambient_slice_64b", |b| b.iter(|| plain[black_box(7)] ^= 1));
    group.finish();
}

fn ipc_move(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc_move");
    for known_processes in [1usize, 16, 256] {
        let mut k = Kernel::new();
        let mut sched = Scheduler::new();
        let mut channels = ChannelRegistry::new();
        let mut mem = RegionRegistry::new();
        let inbox = channels.new_channel(&mut k);
        let wire = channels.new_channel(&mut k);
        let sender = sched.spawn(vec![]);
        let receiver = sched.spawn(vec![]);

        // Grow the sender's vector clock: hear from `known_processes`
        // distinct processes.
        for _ in 1..known_processes {
            let other = sched.spawn(vec![]);
            channels
                .send(&mut k, &inbox, sched.process_mut(other).unwrap(), vec![], 0)
                .unwrap();
            channels
                .receive(&k, &inbox, sched.process_mut(sender).unwrap())
                .unwrap();
        }
        let region = mem.new_region(&mut k, 64);
        let mut handle = sched.process_mut(sender).unwrap().grant(region);
        let mut holder = sender;

        group.bench_function(
            BenchmarkId::new("kernel_send_receive_move", known_processes),
            |b| {
                b.iter(|| {
                    // Bounce the region back and forth; each hop is a full
                    // Move: resolve, reissue, stamp, enqueue, dequeue, grant.
                    let to = if holder == sender { receiver } else { sender };
                    channels
                        .send(
                            &mut k,
                            &wire,
                            sched.process_mut(holder).unwrap(),
                            vec![Grant::Move(handle)],
                            0,
                        )
                        .unwrap();
                    let d = channels
                        .receive(&k, &wire, sched.process_mut(to).unwrap())
                        .unwrap()
                        .unwrap();
                    handle = d.handles[0];
                    holder = to;
                })
            },
        );
    }
    let mut queue: VecDeque<Box<[u8]>> = VecDeque::new();
    let mut slot = Some(vec![0u8; 64].into_boxed_slice());
    group.bench_function("ambient_queue_push_pop", |b| {
        b.iter(|| {
            queue.push_back(slot.take().unwrap());
            slot = queue.pop_front();
        })
    });
    group.finish();
}

criterion_group!(benches, pipeline, primitives, ipc_move);
criterion_main!(benches);
