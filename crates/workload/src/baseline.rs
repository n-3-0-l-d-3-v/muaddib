//! The same pipeline as `pipeline.rs`, with **ambient authority**: no
//! kernel, no capabilities, no process tables, no handles, no epochs.
//! Channels are plain `VecDeque`s any stage can touch, and regions are
//! plain boxed byte slices moved by ordinary Rust moves. Logical clocks
//! are optional, so their cost can be measured separately from the cost
//! of capabilities.
//!
//! Everything else is deliberately identical to the kernel pipeline:
//! topology, downstream-first round-robin schedule (the supervisor's
//! no-op turn included), FIFO region pool, the producer draining returns
//! before sending, and the `data` functions doing the work. So:
//! - the benchmark difference is the price of the kernel's discipline,
//!   not of a different algorithm;
//! - results *and* scheduler statistics must match the kernel pipeline
//!   exactly (the differential test).

use std::collections::VecDeque;

use clock::{EventClock, Stamp};

use crate::data::{checksum, fill_batch, transform};
use crate::pipeline::{BatchResult, PipelineConfig, PipelineError, Stats};

#[derive(Debug, Clone)]
pub struct BaselineReport {
    pub results: Vec<BatchResult>,
    pub stats: Stats,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Supervisor,
    Producer,
    Transformer(usize),
    Sink,
}

struct Message {
    batch: u64,
    region: Box<[u8]>,
    /// Carried only when clocks are on, exactly like `ipc::Message`'s stamp.
    stamp: Option<Stamp<usize>>,
}

struct Stage {
    kind: Kind,
    clock: Option<EventClock<usize>>,
}

impl Stage {
    fn send_stamp(&mut self) -> Option<Stamp<usize>> {
        self.clock.as_mut().map(EventClock::send_event)
    }

    fn receive_stamp(&mut self, sent: &Option<Stamp<usize>>) {
        if let (Some(clock), Some(sent)) = (self.clock.as_mut(), sent) {
            clock.receive_event(sent);
        }
    }
}

pub fn run_baseline(config: PipelineConfig, clocks: bool) -> Result<BaselineReport, PipelineError> {
    let mut report = BaselineReport {
        results: Vec::new(),
        stats: Stats::default(),
    };

    // Same order the kernel pipeline's scheduler round-robins in.
    let mut kinds = vec![Kind::Supervisor, Kind::Sink];
    kinds.extend((0..config.stages).rev().map(Kind::Transformer));
    kinds.push(Kind::Producer);
    let mut stages: Vec<Stage> = kinds
        .into_iter()
        .enumerate()
        .map(|(i, kind)| Stage {
            kind,
            clock: clocks.then(|| EventClock::new(i)),
        })
        .collect();

    // chan[i] feeds transformer i; chan[stages] feeds the sink.
    let mut chan: Vec<VecDeque<Message>> = (0..=config.stages).map(|_| VecDeque::new()).collect();
    let mut returns: VecDeque<Message> = VecDeque::new();
    let mut pool: VecDeque<Box<[u8]>> = (0..config.pool)
        .map(|_| vec![0; config.region_size].into_boxed_slice())
        .collect();
    let mut next_batch = 0u64;

    let processes = stages.len() as u64;
    let mut idle_streak = 0u64;
    let mut turn = 0usize;
    while (report.results.len() as u64) < config.batches {
        let stage = &mut stages[turn % processes as usize];
        turn += 1;
        report.stats.scheduler_turns += 1;

        let progressed = match stage.kind {
            Kind::Supervisor => false,
            Kind::Producer => {
                let mut progressed = false;
                while let Some(m) = returns.pop_front() {
                    stage.receive_stamp(&m.stamp);
                    pool.push_back(m.region);
                    progressed = true;
                }
                if next_batch < config.batches {
                    if let Some(mut region) = pool.pop_front() {
                        fill_batch(config.seed, next_batch, &mut region);
                        report.stats.bytes_processed += config.region_size as u64;
                        let stamp = stage.send_stamp();
                        chan[0].push_back(Message {
                            batch: next_batch,
                            region,
                            stamp,
                        });
                        report.stats.messages += 1;
                        next_batch += 1;
                        progressed = true;
                    }
                }
                progressed
            }
            Kind::Transformer(i) => match chan[i].pop_front() {
                None => false,
                Some(mut m) => {
                    stage.receive_stamp(&m.stamp);
                    transform(i, &mut m.region);
                    report.stats.bytes_processed += config.region_size as u64;
                    m.stamp = stage.send_stamp();
                    chan[i + 1].push_back(m);
                    report.stats.messages += 1;
                    true
                }
            },
            Kind::Sink => match chan[config.stages].pop_front() {
                None => false,
                Some(mut m) => {
                    stage.receive_stamp(&m.stamp);
                    report.results.push(BatchResult {
                        batch: m.batch,
                        checksum: checksum(&m.region),
                    });
                    report.stats.bytes_processed += config.region_size as u64;
                    m.stamp = stage.send_stamp();
                    returns.push_back(m);
                    report.stats.messages += 1;
                    true
                }
            },
        };

        if progressed {
            idle_streak = 0;
        } else {
            report.stats.idle_turns += 1;
            idle_streak += 1;
            if idle_streak > processes {
                return Err(PipelineError::Stalled {
                    turns: report.stats.scheduler_turns,
                });
            }
        }
    }
    Ok(report)
}
