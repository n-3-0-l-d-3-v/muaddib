//! The integration workload, built entirely on the kernel: every
//! earlier ticket exercised together in one real run.
//!
//! ```text
//!                 supervisor (holds full channel caps; spawns everyone)
//!                      |
//!   producer --c0--> stage 0 --c1--> ... --cN--> sink
//!      ^                                           |
//!      +------------------- return ----------------+
//! ```
//!
//! - **002**: the supervisor spawns every stage with `spawn_child`,
//!   handing each *only* a receive-only view of its input channel and a
//!   send-only view of its output channel (`Grant::Derive`). No stage can
//!   read its own output, write its own input, or re-delegate either.
//! - **003/004**: a fixed pool of memory regions circulates around the
//!   ring. Every hop is a `Grant::Move` over IPC, so each region always
//!   has exactly one live owner and every earlier holder's capability is
//!   dead.
//! - **005**: every send and receive is stamped. The report's trace
//!   carries the stamps, and the tests check the causal structure from
//!   them alone. No physical time is read anywhere.
//!
//! Scheduling is `Scheduler::schedule_next` round-robin: each turn, one
//! process takes one step.

use std::collections::VecDeque;

use capability::{Capability, Kernel, Rights};
use ipc::{ChannelRegistry, IpcError};
use memory::{MemoryError, RegionRegistry};
use process::{Grant, Handle, ProcessError, ProcessId, Scheduler, Stamp};

use crate::data::{checksum, fill_batch, transform};

#[derive(Debug, Clone, Copy)]
pub struct PipelineConfig {
    /// How many batches the producer emits.
    pub batches: u64,
    /// Bytes per memory region.
    pub region_size: usize,
    /// How many regions circulate. Smaller than `batches` means regions
    /// are reused, so ownership goes around the ring repeatedly.
    pub pool: usize,
    /// Number of transformer stages between producer and sink.
    pub stages: usize,
    /// Seed for the batch contents.
    pub seed: u64,
    /// Record a stamped trace event for every send and receive.
    pub trace: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Supervisor,
    Producer,
    Transformer(usize),
    Sink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Send,
    Receive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceEvent {
    pub role: Role,
    pub action: Action,
    pub batch: u64,
    pub stamp: Stamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchResult {
    pub batch: u64,
    pub checksum: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    /// Turns handed out by the scheduler.
    pub scheduler_turns: u64,
    /// Turns in which the process had nothing to do.
    pub idle_turns: u64,
    /// Messages sent (each one moving one region).
    pub messages: u64,
    /// Bytes read or written through region capabilities.
    pub bytes_processed: u64,
}

#[derive(Debug, Clone)]
pub struct PipelineReport {
    /// In the order the sink finished them.
    pub results: Vec<BatchResult>,
    pub trace: Vec<TraceEvent>,
    pub stats: Stats,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PipelineError {
    #[error(transparent)]
    Ipc(#[from] IpcError),
    #[error(transparent)]
    Memory(#[from] MemoryError),
    #[error(transparent)]
    Process(#[from] ProcessError),
    #[error("a message arrived without the region it should carry")]
    MissingRegion,
    #[error("no process made progress for a full round after {turns} turns")]
    Stalled { turns: u64 },
}

struct Stage {
    pid: ProcessId,
    role: Role,
    /// Receive-only view of this stage's input channel.
    input: Option<Handle>,
    /// Send-only view of this stage's output channel.
    output: Option<Handle>,
    /// Producer only: regions it currently owns and isn't using. FIFO, so
    /// the whole pool rotates. (A LIFO stack reused the most recently
    /// returned region every time and left the rest of the pool idle.)
    free_regions: VecDeque<Handle>,
    /// Producer only: the next batch number to emit.
    next_batch: u64,
}

pub struct KernelPipeline {
    config: PipelineConfig,
    kernel: Kernel,
    sched: Scheduler,
    channels: ChannelRegistry,
    memory: RegionRegistry,
    stages: Vec<Stage>,
}

fn sorted_handles(sched: &Scheduler, pid: ProcessId) -> Vec<Handle> {
    let mut handles: Vec<Handle> = sched.process(pid).expect("spawned").handles().collect();
    handles.sort();
    handles
}

fn cap_of(sched: &Scheduler, pid: ProcessId, handle: Handle) -> Capability {
    sched
        .process(pid)
        .expect("stage process exists")
        .capability(handle)
        .expect("a stage only ever uses handles it holds")
}

impl KernelPipeline {
    /// Boots the kernel, creates channels and regions, and spawns every
    /// stage with least-privilege capabilities.
    pub fn new(config: PipelineConfig) -> Result<Self, PipelineError> {
        let mut kernel = Kernel::new();
        let mut sched = Scheduler::new();
        let mut channels = ChannelRegistry::new();
        let mut memory = RegionRegistry::new();

        let supervisor = sched.spawn(vec![]);
        // Channel c_i feeds transformer i; c_stages feeds the sink.
        let chan_caps: Vec<Capability> = (0..=config.stages)
            .map(|_| channels.new_channel(&mut kernel))
            .collect();
        let return_cap = channels.new_channel(&mut kernel);
        let region_caps: Vec<Capability> = (0..config.pool)
            .map(|_| memory.new_region(&mut kernel, config.region_size))
            .collect();

        let sup = sched.process_mut(supervisor).expect("just spawned");
        let chan: Vec<Handle> = chan_caps.into_iter().map(|c| sup.grant(c)).collect();
        let ret = sup.grant(return_cap);
        let regions: Vec<Handle> = region_caps.into_iter().map(|c| sup.grant(c)).collect();

        let mut stages = vec![Stage {
            pid: supervisor,
            role: Role::Supervisor,
            input: None,
            output: None,
            free_regions: VecDeque::new(),
            next_batch: 0,
        }];

        // Spawned downstream-first. The scheduler's round-robin follows
        // spawn order, so each round moves every in-flight batch forward
        // one stage: real pipelining, with up to `pool` batches in flight.
        // Upstream-first order would carry each batch all the way around
        // the ring in one round, serializing the whole run (see ADR-006).
        let grants = vec![
            Grant::Derive(chan[config.stages], Rights::READ),
            Grant::Derive(ret, Rights::WRITE),
        ];
        let sink = sched.spawn_child(&mut kernel, supervisor, grants)?;
        let h = sorted_handles(&sched, sink);
        stages.push(Stage {
            pid: sink,
            role: Role::Sink,
            input: Some(h[0]),
            output: Some(h[1]),
            free_regions: VecDeque::new(),
            next_batch: 0,
        });

        for i in (0..config.stages).rev() {
            let grants = vec![
                Grant::Derive(chan[i], Rights::READ),
                Grant::Derive(chan[i + 1], Rights::WRITE),
            ];
            let pid = sched.spawn_child(&mut kernel, supervisor, grants)?;
            let h = sorted_handles(&sched, pid);
            stages.push(Stage {
                pid,
                role: Role::Transformer(i),
                input: Some(h[0]),
                output: Some(h[1]),
                free_regions: VecDeque::new(),
                next_batch: 0,
            });
        }

        // Producer: send-only c0, receive-only return, owns every region.
        let mut grants = vec![
            Grant::Derive(chan[0], Rights::WRITE),
            Grant::Derive(ret, Rights::READ),
        ];
        grants.extend(regions.iter().map(|&h| Grant::Move(h)));
        let producer = sched.spawn_child(&mut kernel, supervisor, grants)?;
        let ph = sorted_handles(&sched, producer);
        stages.push(Stage {
            pid: producer,
            role: Role::Producer,
            output: Some(ph[0]),
            input: Some(ph[1]),
            free_regions: ph[2..].iter().copied().collect(),
            next_batch: 0,
        });

        Ok(Self {
            config,
            kernel,
            sched,
            channels,
            memory,
            stages,
        })
    }

    pub fn kernel(&self) -> &Kernel {
        &self.kernel
    }

    pub fn scheduler(&self) -> &Scheduler {
        &self.sched
    }

    pub fn memory(&self) -> &RegionRegistry {
        &self.memory
    }

    pub fn process_of(&self, role: Role) -> Option<ProcessId> {
        self.stages.iter().find(|s| s.role == role).map(|s| s.pid)
    }

    /// Attempts a send on `channel` *as* the process playing `role`, with
    /// that process's own table as the grant source. For probing that least
    /// privilege really holds; the pipeline itself never calls it.
    pub fn try_send_as(
        &mut self,
        role: Role,
        channel: &Capability,
        grants: Vec<Grant>,
        data: i64,
    ) -> Result<Stamp, PipelineError> {
        let pid = self.process_of(role).expect("role exists in this pipeline");
        let proc = self.sched.process_mut(pid).expect("exists");
        Ok(self
            .channels
            .send(&mut self.kernel, channel, proc, grants, data)?)
    }

    /// Runs to completion: until the sink has finished every batch.
    pub fn run(&mut self) -> Result<PipelineReport, PipelineError> {
        let mut report = PipelineReport {
            results: Vec::new(),
            trace: Vec::new(),
            stats: Stats::default(),
        };
        let processes = self.stages.len() as u64;
        let mut idle_streak = 0u64;

        while (report.results.len() as u64) < self.config.batches {
            let pid = self.sched.schedule_next().expect("pipeline has processes");
            report.stats.scheduler_turns += 1;
            let index = self
                .stages
                .iter()
                .position(|s| s.pid == pid)
                .expect("every scheduled process is a stage");
            if self.step(index, &mut report)? {
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

    /// One turn for one stage. Returns whether it did anything.
    fn step(&mut self, index: usize, report: &mut PipelineReport) -> Result<bool, PipelineError> {
        let Self {
            config,
            kernel,
            sched,
            channels,
            memory,
            stages,
        } = self;
        let stage = &mut stages[index];
        let pid = stage.pid;
        let role = stage.role;
        let trace = |report: &mut PipelineReport, action, batch, stamp: Stamp| {
            if config.trace {
                report.trace.push(TraceEvent {
                    role,
                    action,
                    batch,
                    stamp,
                });
            }
        };

        match stage.role {
            Role::Supervisor => Ok(false),
            Role::Producer => {
                let mut progressed = false;
                let input = cap_of(sched, pid, stage.input.expect("producer input"));
                while let Some(d) =
                    channels.receive(kernel, &input, sched.process_mut(pid).expect("exists"))?
                {
                    let region = *d.handles.first().ok_or(PipelineError::MissingRegion)?;
                    stage.free_regions.push_back(region);
                    trace(report, Action::Receive, d.data as u64, d.received_at);
                    progressed = true;
                }
                if stage.next_batch < config.batches {
                    if let Some(region) = stage.free_regions.pop_front() {
                        let batch = stage.next_batch;
                        let region_cap = cap_of(sched, pid, region);
                        memory.update(kernel, &region_cap, |bytes| {
                            fill_batch(config.seed, batch, bytes)
                        })?;
                        report.stats.bytes_processed += config.region_size as u64;
                        let output = cap_of(sched, pid, stage.output.expect("producer output"));
                        let sent = channels.send(
                            kernel,
                            &output,
                            sched.process_mut(pid).expect("exists"),
                            vec![Grant::Move(region)],
                            batch as i64,
                        )?;
                        report.stats.messages += 1;
                        trace(report, Action::Send, batch, sent);
                        stage.next_batch += 1;
                        progressed = true;
                    }
                }
                Ok(progressed)
            }
            Role::Transformer(_) | Role::Sink => {
                let input = cap_of(sched, pid, stage.input.expect("stage input"));
                let Some(d) =
                    channels.receive(kernel, &input, sched.process_mut(pid).expect("exists"))?
                else {
                    return Ok(false);
                };
                let batch = d.data as u64;
                trace(report, Action::Receive, batch, d.received_at);
                let region = *d.handles.first().ok_or(PipelineError::MissingRegion)?;
                let region_cap = cap_of(sched, pid, region);
                match stage.role {
                    Role::Transformer(i) => {
                        memory.update(kernel, &region_cap, |bytes| transform(i, bytes))?;
                    }
                    _ => {
                        let sum = memory.inspect(kernel, &region_cap, checksum)?;
                        report.results.push(BatchResult {
                            batch,
                            checksum: sum,
                        });
                    }
                }
                report.stats.bytes_processed += config.region_size as u64;
                let output = cap_of(sched, pid, stage.output.expect("stage output"));
                let sent = channels.send(
                    kernel,
                    &output,
                    sched.process_mut(pid).expect("exists"),
                    vec![Grant::Move(region)],
                    d.data,
                )?;
                report.stats.messages += 1;
                trace(report, Action::Send, batch, sent);
                Ok(true)
            }
        }
    }
}

/// Convenience: build and run.
pub fn run_kernel_pipeline(config: PipelineConfig) -> Result<PipelineReport, PipelineError> {
    KernelPipeline::new(config)?.run()
}
