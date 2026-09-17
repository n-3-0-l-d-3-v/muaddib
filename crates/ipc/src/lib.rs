//! IPC with capability-carrying messages: the classic capability-system
//! feature that lets authority be delegated at runtime, not just at
//! process-spawn time. A channel is itself a capability-guarded object
//! (`Rights::WRITE` to send, `Rights::READ` to receive — the same bits
//! ticket 001 defined without prescribing their meaning, given real
//! meaning here); a message can carry other capabilities as payload,
//! resolved through exactly the same `process::resolve_grants`/
//! `apply_grants` machinery ticket 002's `Scheduler::spawn_child`
//! already uses — sending a capability and granting one to a spawned
//! child are the same operation shape (see
//! `docs/design/decisions/ADR-003-ipc.md`).

use std::collections::{HashMap, VecDeque};

use capability::{CapError, Capability, Kernel, ObjectId, Rights};
use process::{apply_grants, resolve_grants, Grant, GrantError, Process, Stamp};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub data: i64,
    pub capabilities: Vec<Capability>,
    /// The sender's send-event stamp (ticket 005) — how the receiver
    /// learns what the sender had causally seen.
    pub sent_at: Stamp,
}

/// What `receive` hands back: the payload data, plus both ends' logical
/// timestamps. (Carried capabilities go straight into the receiver's
/// table, never through this value.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub data: i64,
    /// The send event's stamp, as recorded at the sender.
    pub sent_at: Stamp,
    /// The receive event's stamp, as recorded at the receiver. Always
    /// causally after `sent_at`.
    pub received_at: Stamp,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IpcError {
    #[error("object {0:?} is not a channel this registry created (or it was destroyed)")]
    UnknownChannel(ObjectId),
    #[error(transparent)]
    Capability(#[from] CapError),
    #[error(transparent)]
    Grant(#[from] GrantError),
}

/// Every channel's message queue, keyed by the channel object's id.
/// Deliberately separate from `capability::Kernel` — the kernel only
/// ever tracks *authority* (epochs), never payload data; this registry
/// is where the actual queued messages live, the same separation of
/// concerns `sietch`'s `Store` (data) vs. its own bookkeeping keeps.
#[derive(Default)]
pub struct ChannelRegistry {
    queues: HashMap<ObjectId, VecDeque<Message>>,
}

impl ChannelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new channel object and mints the first (full:
    /// send+receive+grant+destroy) capability for it — the creator can
    /// then `Kernel::derive` send-only or receive-only views to share
    /// with other processes via ticket 002's `Grant` mechanism, exactly
    /// like any other capability.
    pub fn new_channel(&mut self, kernel: &mut Kernel) -> Capability {
        let cap = kernel.new_object(Rights::READ | Rights::WRITE | Rights::GRANT | Rights::DESTROY);
        self.queues.insert(cap.object(), VecDeque::new());
        cap
    }

    /// Sends `data` plus every capability named in `grants` (resolved
    /// against `sender`'s own table, all-or-nothing, exactly like
    /// `Scheduler::spawn_child`) into `channel`. `channel` must grant
    /// `WRITE`. While the message sits in the queue, unreceived, the
    /// capabilities it carries exist in **no process's table at all** —
    /// not the sender's (a `Transfer` already removed them), not the
    /// receiver's (it hasn't called `receive` yet) — only inside this
    /// registry, exactly modeling a message genuinely in flight.
    ///
    /// Takes `&mut Kernel` because a `Grant::Move` in `grants` reissues
    /// the moved object: the only live capability for it is then the one
    /// inside this message.
    ///
    /// A successful send is one event at the sender: it advances the
    /// sender's clocks and returns the send event's stamp, which travels
    /// in the message. A rejected send records no event.
    pub fn send(
        &mut self,
        kernel: &mut Kernel,
        channel: &Capability,
        sender: &mut Process,
        grants: Vec<Grant>,
        data: i64,
    ) -> Result<Stamp, IpcError> {
        kernel.check(channel, Rights::WRITE)?;
        let queue = self
            .queues
            .get_mut(&channel.object())
            .ok_or(IpcError::UnknownChannel(channel.object()))?;

        let resolved = resolve_grants(kernel, sender, &grants)?;
        let resolved = apply_grants(kernel, sender, &grants, resolved);
        let sent_at = sender.record_send();
        queue.push_back(Message {
            data,
            capabilities: resolved,
            sent_at: sent_at.clone(),
        });
        Ok(sent_at)
    }

    /// Receives the oldest queued message on `channel`, if any, granting
    /// every capability it carried directly into `receiver`'s table
    /// (via `Process::grant` — the only way a capability ever enters a
    /// table, per ticket 002's ADR-002). `channel` must grant `READ`.
    ///
    /// Receiving a message is one event at the receiver, merging the
    /// sender's causal knowledge. Finding the queue empty is not an event.
    pub fn receive(
        &mut self,
        kernel: &Kernel,
        channel: &Capability,
        receiver: &mut Process,
    ) -> Result<Option<Delivery>, IpcError> {
        kernel.check(channel, Rights::READ)?;
        let queue = self
            .queues
            .get_mut(&channel.object())
            .ok_or(IpcError::UnknownChannel(channel.object()))?;

        let Some(message) = queue.pop_front() else {
            return Ok(None);
        };
        for cap in message.capabilities {
            receiver.grant(cap);
        }
        let received_at = receiver.record_receive(&message.sent_at);
        Ok(Some(Delivery {
            data: message.data,
            sent_at: message.sent_at,
            received_at,
        }))
    }

    pub fn queue_len(&self, channel: &Capability) -> Option<usize> {
        self.queues.get(&channel.object()).map(VecDeque::len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use process::Scheduler;

    #[test]
    fn sending_then_receiving_delivers_the_data() {
        let mut k = Kernel::new();
        let mut reg = ChannelRegistry::new();
        let mut sched = Scheduler::new();

        let sender_id = sched.spawn(vec![]);
        let receiver_id = sched.spawn(vec![]);
        let channel = reg.new_channel(&mut k);

        {
            let sender = sched.process_mut(sender_id).unwrap();
            reg.send(&mut k, &channel, sender, vec![], 42).unwrap();
        }
        let receiver = sched.process_mut(receiver_id).unwrap();
        let delivery = reg.receive(&k, &channel, receiver).unwrap().unwrap();
        assert_eq!(delivery.data, 42);
        assert_eq!(reg.receive(&k, &channel, receiver).unwrap(), None); // drained
    }

    #[test]
    fn a_delivery_is_stamped_causally_after_its_send() {
        let mut k = Kernel::new();
        let mut reg = ChannelRegistry::new();
        let mut sched = Scheduler::new();
        let sender_id = sched.spawn(vec![]);
        let receiver_id = sched.spawn(vec![]);
        let channel = reg.new_channel(&mut k);

        let sent_at = reg
            .send(
                &mut k,
                &channel,
                sched.process_mut(sender_id).unwrap(),
                vec![],
                7,
            )
            .unwrap();
        assert_eq!(sched.process(sender_id).unwrap().clock(), sent_at);

        let receiver = sched.process_mut(receiver_id).unwrap();
        let d = reg.receive(&k, &channel, receiver).unwrap().unwrap();
        assert_eq!(d.sent_at, sent_at);
        assert!(d.sent_at.happened_before(&d.received_at));
        assert!(d.sent_at.lamport() < d.received_at.lamport());
        assert_eq!(receiver.clock(), d.received_at);
    }

    #[test]
    fn empty_receives_and_rejected_sends_record_no_events() {
        let mut k = Kernel::new();
        let mut reg = ChannelRegistry::new();
        let mut sched = Scheduler::new();
        let pid = sched.spawn(vec![]);
        let channel = reg.new_channel(&mut k);
        let read_only = k.derive(&channel, Rights::READ).unwrap();

        let p = sched.process_mut(pid).unwrap();
        assert_eq!(reg.receive(&k, &channel, p).unwrap(), None);
        assert!(reg.send(&mut k, &read_only, p, vec![], 0).is_err());
        assert_eq!(p.clock().lamport(), 0);
    }

    #[test]
    fn sending_without_write_rights_is_rejected() {
        let mut k = Kernel::new();
        let mut reg = ChannelRegistry::new();
        let mut sched = Scheduler::new();
        let sender_id = sched.spawn(vec![]);
        let full = reg.new_channel(&mut k);
        let read_only = k.derive(&full, Rights::READ).unwrap();

        let sender = sched.process_mut(sender_id).unwrap();
        let result = reg.send(&mut k, &read_only, sender, vec![], 1);
        assert!(matches!(
            result,
            Err(IpcError::Capability(CapError::InsufficientRights { .. }))
        ));
    }

    #[test]
    fn a_transferred_capability_reaches_the_receivers_table() {
        let mut k = Kernel::new();
        let mut reg = ChannelRegistry::new();
        let mut sched = Scheduler::new();

        let payload_cap = k.new_object(Rights::READ | Rights::WRITE);
        let sender_id = sched.spawn(vec![payload_cap]);
        let receiver_id = sched.spawn(vec![]);
        let channel = reg.new_channel(&mut k);

        let payload_handle = sched.process(sender_id).unwrap().handles().next().unwrap();
        {
            let sender = sched.process_mut(sender_id).unwrap();
            reg.send(
                &mut k,
                &channel,
                sender,
                vec![Grant::Transfer(payload_handle)],
                0,
            )
            .unwrap();
            // Gone from the sender the instant send() returns.
            assert_eq!(sender.capability(payload_handle), None);
        }

        let receiver = sched.process_mut(receiver_id).unwrap();
        assert_eq!(receiver.handle_count(), 0);
        reg.receive(&k, &channel, receiver).unwrap();
        assert_eq!(receiver.handle_count(), 1);
        let received_handle = receiver.handles().next().unwrap();
        assert_eq!(receiver.capability(received_handle), Some(payload_cap));
    }

    #[test]
    fn a_derived_capability_leaves_the_senders_own_copy_untouched() {
        let mut k = Kernel::new();
        let mut reg = ChannelRegistry::new();
        let mut sched = Scheduler::new();

        let payload_cap = k.new_object(Rights::READ | Rights::WRITE | Rights::GRANT);
        let sender_id = sched.spawn(vec![payload_cap]);
        let receiver_id = sched.spawn(vec![]);
        let channel = reg.new_channel(&mut k);
        let payload_handle = sched.process(sender_id).unwrap().handles().next().unwrap();

        {
            let sender = sched.process_mut(sender_id).unwrap();
            reg.send(
                &mut k,
                &channel,
                sender,
                vec![Grant::Derive(payload_handle, Rights::READ)],
                0,
            )
            .unwrap();
            assert_eq!(sender.capability(payload_handle), Some(payload_cap));
        }

        let receiver = sched.process_mut(receiver_id).unwrap();
        reg.receive(&k, &channel, receiver).unwrap();
        let received_handle = receiver.handles().next().unwrap();
        let received = receiver.capability(received_handle).unwrap();
        assert_eq!(received.rights(), Rights::READ);
        assert!(payload_cap.rights().contains(received.rights()));
    }

    #[test]
    fn an_in_flight_capability_exists_in_no_process_table() {
        let mut k = Kernel::new();
        let mut reg = ChannelRegistry::new();
        let mut sched = Scheduler::new();

        let payload_cap = k.new_object(Rights::READ);
        let sender_id = sched.spawn(vec![payload_cap]);
        let receiver_id = sched.spawn(vec![]);
        let channel = reg.new_channel(&mut k);
        let payload_handle = sched.process(sender_id).unwrap().handles().next().unwrap();

        {
            let sender = sched.process_mut(sender_id).unwrap();
            reg.send(
                &mut k,
                &channel,
                sender,
                vec![Grant::Transfer(payload_handle)],
                0,
            )
            .unwrap();
        }

        // Not received yet: neither process holds it anywhere.
        assert_eq!(sched.process(sender_id).unwrap().handle_count(), 0);
        assert_eq!(sched.process(receiver_id).unwrap().handle_count(), 0);
        assert_eq!(reg.queue_len(&channel), Some(1));
    }

    #[test]
    fn sending_more_rights_than_the_sender_holds_is_rejected_and_nothing_moves() {
        let mut k = Kernel::new();
        let mut reg = ChannelRegistry::new();
        let mut sched = Scheduler::new();

        let payload_cap = k.new_object(Rights::READ | Rights::GRANT);
        let sender_id = sched.spawn(vec![payload_cap]);
        let channel = reg.new_channel(&mut k);
        let payload_handle = sched.process(sender_id).unwrap().handles().next().unwrap();

        let sender = sched.process_mut(sender_id).unwrap();
        let result = reg.send(
            &mut k,
            &channel,
            sender,
            vec![Grant::Derive(payload_handle, Rights::READ | Rights::WRITE)],
            0,
        );
        assert!(matches!(
            result,
            Err(IpcError::Grant(GrantError::Capability(
                CapError::CannotAmplify { .. }
            )))
        ));
        assert_eq!(sender.capability(payload_handle), Some(payload_cap));
        assert_eq!(reg.queue_len(&channel), Some(0));
    }

    #[test]
    fn a_moved_capability_is_the_only_live_one_and_in_flight_views_arrive_dead() {
        let mut k = Kernel::new();
        let mut reg = ChannelRegistry::new();
        let mut sched = Scheduler::new();

        let owned = k.new_object(Rights::ALL);
        let owner_id = sched.spawn(vec![owned]);
        let bystander_id = sched.spawn(vec![]);
        let new_owner_id = sched.spawn(vec![]);
        let views = reg.new_channel(&mut k);
        let handoff = reg.new_channel(&mut k);
        let handle = sched.process(owner_id).unwrap().handles().next().unwrap();

        {
            let owner = sched.process_mut(owner_id).unwrap();
            // A read view, still in flight when ownership moves.
            reg.send(
                &mut k,
                &views,
                owner,
                vec![Grant::Derive(handle, Rights::READ)],
                0,
            )
            .unwrap();
            reg.send(&mut k, &handoff, owner, vec![Grant::Move(handle)], 0)
                .unwrap();
        }
        // While the move is in flight, even the pre-move value is dead.
        assert_eq!(
            k.check(&owned, Rights::NONE),
            Err(CapError::Revoked(owned.object()))
        );

        let bystander = sched.process_mut(bystander_id).unwrap();
        reg.receive(&k, &views, bystander).unwrap();
        let view = bystander
            .capability(bystander.handles().next().unwrap())
            .unwrap();
        assert_eq!(
            k.check(&view, Rights::READ),
            Err(CapError::Revoked(owned.object()))
        );

        let new_owner = sched.process_mut(new_owner_id).unwrap();
        reg.receive(&k, &handoff, new_owner).unwrap();
        let moved = new_owner
            .capability(new_owner.handles().next().unwrap())
            .unwrap();
        assert_eq!(k.check(&moved, Rights::ALL), Ok(()));
    }

    #[test]
    fn receiving_on_an_unknown_channel_is_a_typed_error() {
        let mut k = Kernel::new();
        let mut reg = ChannelRegistry::new();
        let mut sched = Scheduler::new();
        let receiver_id = sched.spawn(vec![]);
        let bogus = k.new_object(Rights::READ); // never registered as a channel
        let receiver = sched.process_mut(receiver_id).unwrap();
        assert_eq!(
            reg.receive(&k, &bogus, receiver),
            Err(IpcError::UnknownChannel(bogus.object()))
        );
    }
}
