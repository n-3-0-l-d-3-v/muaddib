//! Seeded chaos harness: arbitrary interleavings of every kernel
//! operation, checked step by step against a shadow model, with every
//! failure replayable from `(seed, steps)`.
//!
//! **What it throws at the kernel.** Up to `MAX_PROCESSES` processes
//! spawned with random `Transfer`/`Derive`/`Move` grants, including
//! invalid ones; sends carrying random grant batches (duplicates, move
//! conflicts, amplification attempts); receives; reads and writes with
//! random offsets, including out of bounds and with channel capabilities;
//! revocation and destruction, including of regions whose `Move` is still
//! in flight; and **stale replay**, where any capability any process ever
//! held is kept forever and retried at random.
//!
//! **What the model predicts.** The model is written independently of the
//! kernel code. It tracks each region's epoch, liveness and bytes, plus a
//! mirror of every process table and every in-flight message. For every
//! operation it predicts the *exact* `Result` (the error variant and its
//! fields, not just ok/err) and the exact state afterward. After every
//! step it verifies: every process's real table matches the mirror
//! handle-for-handle; every capability ever seen passes or fails `check`
//! exactly as predicted; every live region's bytes match; the region
//! count matches. At the end, every pair of recorded events is checked
//! against the ground-truth causal graph (as in ticket 005's tests).
//!
//! **Determinism.** All randomness comes from `SplitMix64(seed)`, and the
//! harness never iterates a `HashMap` (process handles are sorted), so a
//! run is a pure function of `(seed, steps, fault)`. `ChaosSummary`
//! carries a hash of the whole op/outcome log to prove it.

use std::cell::Cell;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::panic::{self, AssertUnwindSafe};

use capability::{CapError, Capability, Kernel, ObjectId, Rights};
use ipc::ChannelRegistry;
use memory::{MemoryError, RegionRegistry};
use process::{Grant, Handle, ProcessId, Scheduler, Stamp};

use crate::rng::SplitMix64;

pub const MAX_PROCESSES: usize = 8;
pub const CHANNELS: usize = 3;
const MAX_REGION_SIZE: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Harness self-test: at the first step at or after `step` where some
    /// region can be audited, silently corrupt one byte of the *model*.
    /// The next sweep must catch it, which proves that the audit works
    /// and that a failure replays from its seed.
    CorruptModelAt { step: usize },
}

#[derive(Debug, Clone, Copy)]
pub struct ChaosConfig {
    pub seed: u64,
    pub steps: usize,
    pub fault: Option<Fault>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChaosSummary {
    pub seed: u64,
    pub steps: usize,
    /// Per operation kind: (succeeded, correctly rejected).
    pub outcomes: BTreeMap<&'static str, (u64, u64)>,
    /// Causal events recorded (sends, receives, spawns, births, locals).
    pub events: usize,
    /// Stale-capability replays the kernel correctly refused.
    pub stale_replays_refused: u64,
    /// FNV-1a over the full op/outcome log: equal for equal runs.
    pub log_hash: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChaosFailure {
    pub seed: u64,
    pub steps: usize,
    pub step: usize,
    pub op: String,
    pub message: String,
}

impl fmt::Display for ChaosFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "chaos invariant violated at step {} of seed {:#x}\n  op: {}\n  {}\n  replay: muaddib-chaos --seed {:#x} --steps {}",
            self.step, self.seed, self.op, self.message, self.seed, self.steps
        )
    }
}

impl std::error::Error for ChaosFailure {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Obj {
    Region(usize),
    Channel(usize),
}

/// The model's view of one capability value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CapModel {
    obj: Obj,
    rights: Rights,
    epoch: u64,
}

struct RegionModel {
    object: ObjectId,
    alive: bool,
    epoch: u64,
    bytes: Vec<u8>,
}

struct ProcModel {
    pid: ProcessId,
    table: BTreeMap<Handle, CapModel>,
}

struct MessageModel {
    send_event: usize,
    caps: Vec<CapModel>,
}

#[derive(Debug, Clone, Copy)]
enum GrantKind {
    Transfer,
    Derive(Rights),
    Move,
}

#[derive(Debug, Clone)]
enum Op {
    NewRegion {
        p: usize,
        size: usize,
    },
    Spawn {
        parent: usize,
        grants: Vec<(usize, GrantKind)>,
    },
    Send {
        p: usize,
        channel: usize,
        grants: Vec<(usize, GrantKind)>,
    },
    Receive {
        p: usize,
        channel: usize,
    },
    Read {
        p: usize,
        pick: Pick,
        regions_only: bool,
        offset: usize,
        len: usize,
    },
    Write {
        p: usize,
        pick: Pick,
        regions_only: bool,
        offset: usize,
        len: usize,
        byte: u8,
    },
    Revoke {
        p: usize,
        pick: Pick,
    },
    Destroy {
        p: usize,
        pick: Pick,
    },
    Local {
        p: usize,
    },
}

/// Which capability an access op uses: the n-th handle (mod table size)
/// in the process's own table, or the n-th capability ever seen (a
/// potentially stale replay).
#[derive(Debug, Clone, Copy)]
enum Pick {
    Table(usize),
    Stash(usize),
}

impl Op {
    fn name(&self) -> &'static str {
        match self {
            Op::NewRegion { .. } => "new_region",
            Op::Spawn { .. } => "spawn",
            Op::Send { .. } => "send",
            Op::Receive { .. } => "receive",
            Op::Read { .. } => "read",
            Op::Write { .. } => "write",
            Op::Revoke { .. } => "revoke",
            Op::Destroy { .. } => "destroy",
            Op::Local { .. } => "local",
        }
    }
}

struct World {
    k: Kernel,
    sched: Scheduler,
    channels: ChannelRegistry,
    mem: RegionRegistry,
    channel_objects: Vec<ObjectId>,
    regions: Vec<RegionModel>,
    procs: Vec<ProcModel>,
    in_flight: Vec<VecDeque<MessageModel>>,
    /// Every capability any process ever held, with the model of it.
    stash: Vec<(Capability, CapModel)>,
    stamps: Vec<Stamp>,
    preds: Vec<Vec<usize>>,
    last_event: Vec<Option<usize>>,
    summary: ChaosSummary,
    log: u64,
}

type Check = Result<(), String>;

fn dead_mem<T>(result: &Result<T, MemoryError>) -> bool {
    matches!(
        result,
        Err(MemoryError::Capability(
            CapError::Revoked(_) | CapError::UnknownObject(_)
        ))
    )
}

fn ensure(cond: bool, msg: impl FnOnce() -> String) -> Check {
    if cond {
        Ok(())
    } else {
        Err(msg())
    }
}

fn arb_rights(rng: &mut SplitMix64) -> Rights {
    [
        Rights::READ,
        Rights::WRITE,
        Rights::EXECUTE,
        Rights::GRANT,
        Rights::DESTROY,
    ]
    .into_iter()
    .filter(|_| rng.percent(50))
    .fold(Rights::NONE, |acc, r| acc | r)
}

impl World {
    fn new() -> Self {
        let mut w = World {
            k: Kernel::new(),
            sched: Scheduler::new(),
            channels: ChannelRegistry::new(),
            mem: RegionRegistry::new(),
            channel_objects: vec![],
            regions: vec![],
            procs: vec![],
            in_flight: (0..CHANNELS).map(|_| VecDeque::new()).collect(),
            stash: vec![],
            stamps: vec![],
            preds: vec![],
            last_event: vec![],
            summary: ChaosSummary::default(),
            log: 0xCBF2_9CE4_8422_2325,
        };
        let root = w.sched.spawn(vec![]);
        w.procs.push(ProcModel {
            pid: root,
            table: BTreeMap::new(),
        });
        w.last_event.push(None);
        for c in 0..CHANNELS {
            let cap = w.channels.new_channel(&mut w.k);
            w.channel_objects.push(cap.object());
            w.install(
                0,
                cap,
                CapModel {
                    obj: Obj::Channel(c),
                    rights: cap.rights(),
                    epoch: 0,
                },
            );
        }
        for _ in 0..2 {
            w.new_region(0, 8);
        }
        w
    }

    fn log(&mut self, text: &str) {
        for b in text.bytes() {
            self.log = (self.log ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3);
        }
    }

    /// Kernel-side bootstrap insertion: grant `cap` into process `p` and
    /// mirror it. (Only used for root setup and fresh allocations, which is
    /// the kernel acting, not a process conjuring authority.)
    fn install(&mut self, p: usize, cap: Capability, model: CapModel) -> Handle {
        let handle = self
            .sched
            .process_mut(self.procs[p].pid)
            .expect("exists")
            .grant(cap);
        self.procs[p].table.insert(handle, model);
        self.stash.push((cap, model));
        handle
    }

    fn new_region(&mut self, p: usize, size: usize) {
        let cap = self.mem.new_region(&mut self.k, size);
        self.regions.push(RegionModel {
            object: cap.object(),
            alive: true,
            epoch: 0,
            bytes: vec![0; size],
        });
        let r = self.regions.len() - 1;
        self.install(
            p,
            cap,
            CapModel {
                obj: Obj::Region(r),
                rights: cap.rights(),
                epoch: 0,
            },
        );
    }

    fn object_id(&self, obj: Obj) -> ObjectId {
        match obj {
            Obj::Region(r) => self.regions[r].object,
            Obj::Channel(c) => self.channel_objects[c],
        }
    }

    /// The model's prediction of `Kernel::check(cap, required)`.
    fn predict_check(&self, c: &CapModel, required: Rights) -> Result<(), CapError> {
        let object = self.object_id(c.obj);
        if let Obj::Region(r) = c.obj {
            let region = &self.regions[r];
            if !region.alive {
                return Err(CapError::UnknownObject(object));
            }
            if region.epoch != c.epoch {
                return Err(CapError::Revoked(object));
            }
        }
        if !c.rights.contains(required) {
            return Err(CapError::InsufficientRights {
                object,
                held: c.rights,
                required,
            });
        }
        Ok(())
    }

    fn record_event(&mut self, p: usize, stamp: Stamp, mut preds: Vec<usize>) -> usize {
        if let Some(prev) = self.last_event[p] {
            preds.push(prev);
        }
        let idx = self.stamps.len();
        self.last_event[p] = Some(idx);
        self.stamps.push(stamp);
        self.preds.push(preds);
        idx
    }

    fn outcome(&mut self, name: &'static str, ok: bool) {
        let entry = self.summary.outcomes.entry(name).or_default();
        if ok {
            entry.0 += 1;
        } else {
            entry.1 += 1;
        }
    }

    fn pick_cap(
        &self,
        p: usize,
        pick: Pick,
        want_region: bool,
    ) -> Option<(Capability, CapModel, bool)> {
        match pick {
            Pick::Table(n) => {
                let candidates: Vec<(&Handle, &CapModel)> = self.procs[p]
                    .table
                    .iter()
                    .filter(|(_, m)| !want_region || matches!(m.obj, Obj::Region(_)))
                    .collect();
                if candidates.is_empty() {
                    return None;
                }
                let (h, m) = candidates[n % candidates.len()];
                let cap = self
                    .sched
                    .process(self.procs[p].pid)
                    .expect("exists")
                    .capability(*h)
                    .expect("mirror checked last step");
                Some((cap, *m, false))
            }
            Pick::Stash(n) => {
                let candidates: Vec<&(Capability, CapModel)> = self
                    .stash
                    .iter()
                    .filter(|(_, m)| !want_region || matches!(m.obj, Obj::Region(_)))
                    .collect();
                if candidates.is_empty() {
                    return None;
                }
                let (cap, m) = candidates[n % candidates.len()];
                Some((*cap, *m, true))
            }
        }
    }

    /// Resolves `(n-th region handle, kind)` pairs against process `p`'s
    /// mirror into concrete handles and grants.
    fn concrete_grants(&self, p: usize, grants: &[(usize, GrantKind)]) -> Vec<(Handle, GrantKind)> {
        let region_handles: Vec<Handle> = self.procs[p]
            .table
            .iter()
            .filter(|(_, m)| matches!(m.obj, Obj::Region(_)))
            .map(|(h, _)| *h)
            .collect();
        if region_handles.is_empty() {
            return vec![];
        }
        grants
            .iter()
            .map(|&(n, kind)| (region_handles[n % region_handles.len()], kind))
            .collect()
    }

    /// The model of `resolve_grants` + `apply_grants`: `None` if the batch
    /// must be rejected, otherwise the delivered capability models, having
    /// applied table removals and epoch bumps to the model.
    fn apply_grants_model(
        &mut self,
        p: usize,
        grants: &[(Handle, GrantKind)],
    ) -> Option<Vec<CapModel>> {
        let mut seen = Vec::new();
        let mut resolved = Vec::new();
        for &(h, kind) in grants {
            let held = self.procs[p].table[&h];
            if matches!(kind, GrantKind::Transfer | GrantKind::Move) {
                if seen.contains(&h) {
                    return None;
                }
                seen.push(h);
            }
            match kind {
                GrantKind::Transfer => resolved.push(held),
                GrantKind::Derive(rights) => {
                    self.predict_check(&held, Rights::GRANT).ok()?;
                    if !held.rights.contains(rights) {
                        return None;
                    }
                    resolved.push(CapModel { rights, ..held });
                }
                GrantKind::Move => {
                    self.predict_check(&held, Rights::DESTROY).ok()?;
                    resolved.push(held);
                }
            }
        }
        for (i, &(_, kind)) in grants.iter().enumerate() {
            if matches!(kind, GrantKind::Move)
                && resolved
                    .iter()
                    .enumerate()
                    .any(|(j, m)| j != i && m.obj == resolved[i].obj)
            {
                return None;
            }
        }
        // Valid: apply.
        for (i, &(h, kind)) in grants.iter().enumerate() {
            match kind {
                GrantKind::Transfer => {
                    self.procs[p].table.remove(&h);
                }
                GrantKind::Move => {
                    self.procs[p].table.remove(&h);
                    let Obj::Region(r) = resolved[i].obj else {
                        unreachable!("only regions are moved")
                    };
                    self.regions[r].epoch += 1;
                    resolved[i].epoch = self.regions[r].epoch;
                }
                GrantKind::Derive(_) => {}
            }
        }
        Some(resolved)
    }

    fn to_grants(grants: &[(Handle, GrantKind)]) -> Vec<Grant> {
        grants
            .iter()
            .map(|&(h, kind)| match kind {
                GrantKind::Transfer => Grant::Transfer(h),
                GrantKind::Derive(r) => Grant::Derive(h, r),
                GrantKind::Move => Grant::Move(h),
            })
            .collect()
    }

    fn generate(&self, rng: &mut SplitMix64) -> Op {
        let n = self.procs.len();
        let p = rng.below(n);
        let pick = |rng: &mut SplitMix64| {
            if rng.percent(30) {
                Pick::Stash(rng.below(1 << 16))
            } else {
                Pick::Table(rng.below(1 << 16))
            }
        };
        // Mostly small, plausible accesses, so successful reads and writes
        // (and with them the byte model) get exercised; sometimes wild ones.
        let span = |rng: &mut SplitMix64| {
            if rng.percent(75) {
                (rng.below(4), rng.below(5))
            } else {
                (rng.below(MAX_REGION_SIZE + 8), rng.below(10))
            }
        };
        let grants = |rng: &mut SplitMix64| {
            (0..rng.below(4))
                .map(|_| {
                    let kind = match rng.below(3) {
                        0 => GrantKind::Transfer,
                        1 => GrantKind::Derive(arb_rights(rng)),
                        _ => GrantKind::Move,
                    };
                    (rng.below(1 << 16), kind)
                })
                .collect()
        };
        match rng.below(100) {
            0..=5 => Op::NewRegion {
                p,
                size: 1 + rng.below(MAX_REGION_SIZE),
            },
            6..=10 => Op::Spawn {
                parent: p,
                grants: grants(rng),
            },
            11..=28 => Op::Send {
                p,
                channel: rng.below(CHANNELS),
                grants: grants(rng),
            },
            29..=46 => Op::Receive {
                p,
                channel: rng.below(CHANNELS),
            },
            47..=61 => {
                let (offset, len) = span(rng);
                Op::Read {
                    p,
                    pick: pick(rng),
                    regions_only: !rng.percent(15),
                    offset,
                    len,
                }
            }
            62..=76 => {
                let (offset, len) = span(rng);
                Op::Write {
                    p,
                    pick: pick(rng),
                    regions_only: !rng.percent(15),
                    offset,
                    len,
                    byte: rng.next_u64() as u8,
                }
            }
            77..=85 => Op::Revoke { p, pick: pick(rng) },
            86..=91 => Op::Destroy { p, pick: pick(rng) },
            _ => Op::Local { p },
        }
    }

    fn channel_cap(&self, p: usize, channel: usize) -> Capability {
        let handle = self.procs[p]
            .table
            .iter()
            .find(|(_, m)| m.obj == Obj::Channel(channel))
            .map(|(h, _)| *h)
            .expect("every process holds every channel");
        self.sched
            .process(self.procs[p].pid)
            .expect("exists")
            .capability(handle)
            .expect("mirror checked")
    }

    fn execute(&mut self, op: &Op) -> Check {
        match op.clone() {
            Op::NewRegion { p, size } => {
                self.new_region(p, size);
                self.outcome("new_region", true);
                self.log(&format!("new_region {p} {size}"));
            }
            Op::Local { p } => {
                let s = self
                    .sched
                    .process_mut(self.procs[p].pid)
                    .expect("exists")
                    .record_local_event();
                self.record_event(p, s, vec![]);
                self.outcome("local", true);
                self.log(&format!("local {p}"));
            }
            Op::Spawn { parent, grants } => {
                if self.procs.len() == MAX_PROCESSES {
                    return Ok(());
                }
                let concrete = self.concrete_grants(parent, &grants);
                let predicted = self.apply_grants_model(parent, &concrete);
                let mut real_grants: Vec<Grant> = (0..CHANNELS)
                    .map(|c| {
                        let h = self.procs[parent]
                            .table
                            .iter()
                            .find(|(_, m)| m.obj == Obj::Channel(c))
                            .map(|(h, _)| *h)
                            .expect("channels");
                        Grant::Derive(h, Rights::READ | Rights::WRITE | Rights::GRANT)
                    })
                    .collect();
                real_grants.extend(Self::to_grants(&concrete));
                let result =
                    self.sched
                        .spawn_child(&mut self.k, self.procs[parent].pid, real_grants);
                self.log(&format!(
                    "spawn {parent} {concrete:?} -> {}",
                    result.is_ok()
                ));
                self.outcome("spawn", result.is_ok());
                match (result, predicted) {
                    (Err(_), None) => {}
                    (Ok(child), Some(delivered)) => {
                        let parent_stamp =
                            self.sched.process(self.procs[parent].pid).unwrap().clock();
                        let spawn_event = self.record_event(parent, parent_stamp, vec![]);
                        let mut handles: Vec<Handle> =
                            self.sched.process(child).unwrap().handles().collect();
                        handles.sort();
                        let mut models: Vec<CapModel> = (0..CHANNELS)
                            .map(|c| CapModel {
                                obj: Obj::Channel(c),
                                rights: Rights::READ | Rights::WRITE | Rights::GRANT,
                                epoch: 0,
                            })
                            .collect();
                        models.extend(delivered);
                        ensure(handles.len() == models.len(), || {
                            format!(
                                "child got {} handles, model {}",
                                handles.len(),
                                models.len()
                            )
                        })?;
                        let child_index = self.procs.len();
                        self.procs.push(ProcModel {
                            pid: child,
                            table: BTreeMap::new(),
                        });
                        self.last_event.push(None);
                        for (h, m) in handles.into_iter().zip(models) {
                            let cap = self.sched.process(child).unwrap().capability(h).unwrap();
                            self.procs[child_index].table.insert(h, m);
                            self.stash.push((cap, m));
                        }
                        let birth = self.sched.process(child).unwrap().clock();
                        self.record_event(child_index, birth, vec![spawn_event]);
                    }
                    (r, p) => {
                        return Err(format!(
                            "spawn: kernel {:?}, model predicted success={}",
                            r.map(|_| ()),
                            p.is_some()
                        ))
                    }
                }
            }
            Op::Send { p, channel, grants } => {
                let concrete = self.concrete_grants(p, &grants);
                let predicted = self.apply_grants_model(p, &concrete);
                let chan = self.channel_cap(p, channel);
                let result = self.channels.send(
                    &mut self.k,
                    &chan,
                    self.sched.process_mut(self.procs[p].pid).expect("exists"),
                    Self::to_grants(&concrete),
                    0,
                );
                self.log(&format!(
                    "send {p} {channel} {concrete:?} -> {}",
                    result.is_ok()
                ));
                self.outcome("send", result.is_ok());
                match (result, predicted) {
                    (Err(_), None) => {}
                    (Ok(stamp), Some(caps)) => {
                        let send_event = self.record_event(p, stamp, vec![]);
                        self.in_flight[channel].push_back(MessageModel { send_event, caps });
                    }
                    (r, pred) => {
                        return Err(format!(
                            "send: kernel {:?}, model predicted success={}",
                            r.map(|_| ()),
                            pred.is_some()
                        ))
                    }
                }
            }
            Op::Receive { p, channel } => {
                let chan = self.channel_cap(p, channel);
                let result = self.channels.receive(
                    &self.k,
                    &chan,
                    self.sched.process_mut(self.procs[p].pid).expect("exists"),
                );
                let expected = self.in_flight[channel].pop_front();
                let delivery = result.map_err(|e| format!("receive failed: {e}"))?;
                self.log(&format!("receive {p} {channel} -> {}", delivery.is_some()));
                self.outcome("receive", delivery.is_some());
                match (delivery, expected) {
                    (None, None) => {}
                    (Some(d), Some(msg)) => {
                        ensure(d.sent_at == self.stamps[msg.send_event], || {
                            "delivered message carries the wrong send stamp".into()
                        })?;
                        ensure(d.handles.len() == msg.caps.len(), || {
                            format!(
                                "delivered {} caps, model {}",
                                d.handles.len(),
                                msg.caps.len()
                            )
                        })?;
                        for (h, m) in d.handles.iter().zip(msg.caps) {
                            let cap = self
                                .sched
                                .process(self.procs[p].pid)
                                .unwrap()
                                .capability(*h)
                                .unwrap();
                            self.procs[p].table.insert(*h, m);
                            self.stash.push((cap, m));
                        }
                        self.record_event(p, d.received_at, vec![msg.send_event]);
                    }
                    (d, e) => {
                        return Err(format!(
                            "receive: kernel delivered={}, model expected={}",
                            d.is_some(),
                            e.is_some()
                        ))
                    }
                }
            }
            Op::Read {
                p,
                pick,
                regions_only,
                offset,
                len,
            } => {
                let Some((cap, model, stale)) = self.pick_cap(p, pick, regions_only) else {
                    return Ok(());
                };
                let predicted = self
                    .predict_access(&model, Rights::READ, offset, len)
                    .map(|()| {
                        let Obj::Region(r) = model.obj else {
                            unreachable!()
                        };
                        self.regions[r].bytes[offset..offset + len].to_vec()
                    });
                let result = self.mem.read(&self.k, &cap, offset, len);
                self.note_access(
                    "read",
                    stale,
                    result.is_ok(),
                    dead_mem(&result),
                    format!("{result:?}"),
                );
                ensure(result == predicted, || {
                    format!("read: kernel {result:?}, model {predicted:?}")
                })?;
            }
            Op::Write {
                p,
                pick,
                regions_only,
                offset,
                len,
                byte,
            } => {
                let Some((cap, model, stale)) = self.pick_cap(p, pick, regions_only) else {
                    return Ok(());
                };
                let predicted = self.predict_access(&model, Rights::WRITE, offset, len);
                let data = vec![byte; len];
                let result = self.mem.write(&self.k, &cap, offset, &data);
                self.note_access(
                    "write",
                    stale,
                    result.is_ok(),
                    dead_mem(&result),
                    format!("{result:?}"),
                );
                ensure(result == predicted, || {
                    format!("write: kernel {result:?}, model {predicted:?}")
                })?;
                if result.is_ok() {
                    let Obj::Region(r) = model.obj else {
                        unreachable!()
                    };
                    self.regions[r].bytes[offset..offset + len].copy_from_slice(&data);
                }
            }
            Op::Revoke { p, pick } => {
                let Some((cap, model, stale)) = self.pick_cap(p, pick, true) else {
                    return Ok(());
                };
                let predicted = self.predict_check(&model, Rights::DESTROY);
                let result = self.k.revoke(&cap);
                let dead = matches!(
                    result,
                    Err(CapError::Revoked(_) | CapError::UnknownObject(_))
                );
                self.note_access("revoke", stale, result.is_ok(), dead, format!("{result:?}"));
                ensure(result == predicted, || {
                    format!("revoke: kernel {result:?}, model {predicted:?}")
                })?;
                if result.is_ok() {
                    let Obj::Region(r) = model.obj else {
                        unreachable!()
                    };
                    self.regions[r].epoch += 1;
                }
            }
            Op::Destroy { p, pick } => {
                let Some((cap, model, stale)) = self.pick_cap(p, pick, true) else {
                    return Ok(());
                };
                let predicted = self
                    .predict_check(&model, Rights::DESTROY)
                    .map_err(MemoryError::from);
                let result = self.mem.destroy_region(&mut self.k, &cap);
                self.note_access(
                    "destroy",
                    stale,
                    result.is_ok(),
                    dead_mem(&result),
                    format!("{result:?}"),
                );
                ensure(result == predicted, || {
                    format!("destroy: kernel {result:?}, model {predicted:?}")
                })?;
                if result.is_ok() {
                    let Obj::Region(r) = model.obj else {
                        unreachable!()
                    };
                    self.regions[r].alive = false;
                }
            }
        }
        Ok(())
    }

    /// The model of `RegionRegistry::read`/`write`'s checks, in the
    /// registry's order: capability, then object kind, then bounds.
    fn predict_access(
        &self,
        c: &CapModel,
        required: Rights,
        offset: usize,
        len: usize,
    ) -> Result<(), MemoryError> {
        self.predict_check(c, required)?;
        let object = self.object_id(c.obj);
        let Obj::Region(r) = c.obj else {
            return Err(MemoryError::UnknownRegion(object));
        };
        let size = self.regions[r].bytes.len();
        match offset.checked_add(len) {
            Some(end) if end <= size => Ok(()),
            _ => Err(MemoryError::OutOfBounds {
                object,
                offset,
                len,
                size,
            }),
        }
    }

    /// Records an access op's outcome. `refused_as_dead` is whether the
    /// kernel refused because the capability was revoked or its object
    /// destroyed: for a stale replay, exactly the refusal that matters.
    fn note_access(
        &mut self,
        name: &'static str,
        stale: bool,
        ok: bool,
        refused_as_dead: bool,
        shown: String,
    ) {
        self.outcome(name, ok);
        if stale && refused_as_dead {
            self.summary.stale_replays_refused += 1;
        }
        self.log(&format!("{name} stale={stale} -> {shown}"));
    }

    /// Full-state audit against the model.
    fn sweep(&self) -> Check {
        for (i, pm) in self.procs.iter().enumerate() {
            let proc = self.sched.process(pm.pid).expect("exists");
            let mut real: Vec<Handle> = proc.handles().collect();
            real.sort();
            let mirrored: Vec<Handle> = pm.table.keys().copied().collect();
            ensure(real == mirrored, || {
                format!("process {i} table {real:?} != model {mirrored:?}")
            })?;
            for (h, m) in &pm.table {
                let cap = proc.capability(*h).expect("present");
                ensure(
                    cap.object() == self.object_id(m.obj) && cap.rights() == m.rights,
                    || format!("process {i} handle {h:?}: {cap:?} != model {m:?}"),
                )?;
            }
        }
        for (cap, m) in &self.stash {
            let real = self.k.check(cap, Rights::NONE);
            let predicted = self.predict_check(m, Rights::NONE);
            ensure(real == predicted, || {
                format!("liveness of {cap:?}: kernel {real:?}, model {predicted:?}")
            })?;
        }
        let alive = self.regions.iter().filter(|r| r.alive).count();
        ensure(self.mem.region_count() == alive, || {
            format!(
                "{} regions allocated, model {alive}",
                self.mem.region_count()
            )
        })?;
        for (r, region) in self.regions.iter().enumerate() {
            if let Some(cap) = self.auditor(r) {
                let real = self.mem.inspect(&self.k, &cap, <[u8]>::to_vec);
                ensure(real.as_ref() == Ok(&region.bytes), || {
                    format!("region {r} bytes {real:?} != model {:?}", region.bytes)
                })?;
            }
        }
        Ok(())
    }

    /// Any capability the model says can currently read region `r`.
    fn auditor(&self, r: usize) -> Option<Capability> {
        self.stash
            .iter()
            .find(|(_, m)| m.obj == Obj::Region(r) && self.predict_check(m, Rights::READ).is_ok())
            .map(|(cap, _)| *cap)
    }

    fn check_causality(&self) -> Check {
        let n = self.stamps.len();
        let mut reach = vec![vec![false; n]; n];
        for (i, preds) in self.preds.iter().enumerate() {
            let (earlier, rest) = reach.split_at_mut(i);
            let row = &mut rest[0];
            for &p in preds {
                row[p] = true;
                for (dst, src) in row.iter_mut().zip(&earlier[p]) {
                    *dst |= *src;
                }
            }
        }
        for (i, stamp_i) in self.stamps.iter().enumerate() {
            for (j, stamp_j) in self.stamps.iter().enumerate() {
                if i == j {
                    continue;
                }
                let truth = reach[j][i];
                ensure(stamp_i.happened_before(stamp_j) == truth, || {
                    format!("causality: event {i} before {j} is {truth}, vector clocks disagree")
                })?;
                if truth {
                    ensure(stamp_i.lamport() < stamp_j.lamport(), || {
                        format!("Lamport clock condition violated for {i} -> {j}")
                    })?;
                }
            }
        }
        Ok(())
    }
}

/// Runs one chaos schedule. Pure function of `config`.
///
/// A panic anywhere inside the kernel is also an invariant violation. It
/// is caught and reported as a `ChaosFailure` carrying the seed, step and
/// op, so a crash is exactly as replayable as a wrong answer.
pub fn run_chaos(config: ChaosConfig) -> Result<ChaosSummary, ChaosFailure> {
    let progress = Cell::new((0usize, String::from("setup")));
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| run_steps(config, &progress)));
    outcome.unwrap_or_else(|payload| {
        let what = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("non-string panic payload");
        let (step, op) = progress.take();
        Err(ChaosFailure {
            seed: config.seed,
            steps: config.steps,
            step,
            op,
            message: format!("kernel panicked: {what}"),
        })
    })
}

fn run_steps(
    config: ChaosConfig,
    progress: &Cell<(usize, String)>,
) -> Result<ChaosSummary, ChaosFailure> {
    let mut rng = SplitMix64::new(config.seed);
    let mut world = World::new();
    let mut fault_pending = config.fault;
    let fail = |step: usize, op: String, message: String| ChaosFailure {
        seed: config.seed,
        steps: config.steps,
        step,
        op,
        message,
    };

    for step in 0..config.steps {
        let op = world.generate(&mut rng);
        progress.set((step, format!("{op:?}")));
        world
            .execute(&op)
            .map_err(|m| fail(step, format!("{op:?}"), m))?;

        if let Some(Fault::CorruptModelAt { step: at }) = fault_pending {
            if step >= at {
                let target = (0..world.regions.len())
                    .find(|&r| !world.regions[r].bytes.is_empty() && world.auditor(r).is_some());
                if let Some(r) = target {
                    world.regions[r].bytes[0] ^= 0xFF;
                    fault_pending = None;
                }
            }
        }

        world
            .sweep()
            .map_err(|m| fail(step, format!("{op:?} ({})", op.name()), m))?;
    }
    progress.set((config.steps, "end-of-run causality check".into()));
    world
        .check_causality()
        .map_err(|m| fail(config.steps, "end-of-run causality check".into(), m))?;

    let mut summary = world.summary;
    summary.seed = config.seed;
    summary.steps = config.steps;
    summary.events = world.stamps.len();
    summary.log_hash = world.log;
    Ok(summary)
}
