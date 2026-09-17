//! Memory regions named by capability, never by an address or path the
//! process picks. A region is an ordinary `capability` object:
//! `Rights::READ` = may read its bytes, `Rights::WRITE` = may write them,
//! `Rights::DESTROY` = may free it — and, since `DESTROY` is exactly what
//! `Kernel::revoke`/`reissue` require, holding `DESTROY` is what
//! *owning* a region means.
//!
//! This crate deliberately has **no transfer API of its own**. Ownership
//! moves through `process::Grant::Move` — over a channel (`ipc::send`) or
//! at spawn (`Scheduler::spawn_child`) — which reissues the region's
//! capability so every copy the old owner kept fails every subsequent
//! access check here. Kernel-enforced, not a convention: the check this
//! registry runs on every `read`/`write` is the same epoch comparison the
//! reissue invalidated. See
//! `docs/design/decisions/ADR-004-memory-ownership.md`.

use std::collections::HashMap;

use capability::{CapError, Capability, Kernel, ObjectId, Rights};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MemoryError {
    #[error("object {0:?} is not a memory region this registry allocated (or it was destroyed)")]
    UnknownRegion(ObjectId),
    #[error(
        "access of {len} bytes at offset {offset} is outside region {object:?} of size {size}"
    )]
    OutOfBounds {
        object: ObjectId,
        offset: usize,
        len: usize,
        size: usize,
    },
    #[error(transparent)]
    Capability(#[from] CapError),
}

/// Every region's backing bytes, keyed by the region object's id. Like
/// `ipc::ChannelRegistry`, deliberately separate from `capability::Kernel`
/// — the kernel only tracks authority (epochs), never payload data.
#[derive(Debug, Default)]
pub struct RegionRegistry {
    regions: HashMap<ObjectId, Box<[u8]>>,
}

impl RegionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocates a zero-filled region of `size` bytes and mints its first,
    /// owning capability (`READ|WRITE|GRANT|DESTROY`). The creator can
    /// `Kernel::derive` weaker, non-owning views (e.g. read-only) to share,
    /// or hand the region off entirely with `Grant::Move`.
    pub fn new_region(&mut self, kernel: &mut Kernel, size: usize) -> Capability {
        let cap = kernel.new_object(Rights::READ | Rights::WRITE | Rights::GRANT | Rights::DESTROY);
        self.regions
            .insert(cap.object(), vec![0; size].into_boxed_slice());
        cap
    }

    /// Validates `cap` for `required`, then the region exists, then the
    /// access range — in that order, so a caller without authority learns
    /// nothing about the region's size from the error it gets back.
    fn checked_range(
        &self,
        kernel: &Kernel,
        cap: &Capability,
        required: Rights,
        offset: usize,
        len: usize,
    ) -> Result<std::ops::Range<usize>, MemoryError> {
        kernel.check(cap, required)?;
        let object = cap.object();
        let size = self
            .regions
            .get(&object)
            .ok_or(MemoryError::UnknownRegion(object))?
            .len();
        match offset.checked_add(len) {
            Some(end) if end <= size => Ok(offset..end),
            _ => Err(MemoryError::OutOfBounds {
                object,
                offset,
                len,
                size,
            }),
        }
    }

    /// Reads `len` bytes starting at `offset`. `cap` must grant `READ`.
    pub fn read(
        &self,
        kernel: &Kernel,
        cap: &Capability,
        offset: usize,
        len: usize,
    ) -> Result<Vec<u8>, MemoryError> {
        let range = self.checked_range(kernel, cap, Rights::READ, offset, len)?;
        Ok(self.regions[&cap.object()][range].to_vec())
    }

    /// Writes `data` starting at `offset`. `cap` must grant `WRITE`. The
    /// whole range is validated first: a write that would run past the
    /// end changes no bytes at all, never a truncated prefix.
    pub fn write(
        &mut self,
        kernel: &Kernel,
        cap: &Capability,
        offset: usize,
        data: &[u8],
    ) -> Result<(), MemoryError> {
        let range = self.checked_range(kernel, cap, Rights::WRITE, offset, data.len())?;
        self.regions
            .get_mut(&cap.object())
            .expect("checked_range confirmed the region exists")[range]
            .copy_from_slice(data);
        Ok(())
    }

    /// The region's size in bytes. Needs a live capability but no
    /// particular rights — any holder, even of a `NONE` view, may know how
    /// big the thing it names is.
    pub fn size(&self, kernel: &Kernel, cap: &Capability) -> Result<usize, MemoryError> {
        self.checked_range(kernel, cap, Rights::NONE, 0, 0)?;
        Ok(self.regions[&cap.object()].len())
    }

    /// Frees the region: destroys the kernel object (requires `DESTROY`)
    /// and drops its bytes. Every capability for it then fails with
    /// `CapError::UnknownObject`.
    pub fn destroy_region(
        &mut self,
        kernel: &mut Kernel,
        cap: &Capability,
    ) -> Result<(), MemoryError> {
        kernel.check(cap, Rights::DESTROY)?;
        if !self.regions.contains_key(&cap.object()) {
            return Err(MemoryError::UnknownRegion(cap.object()));
        }
        kernel.destroy_object(cap)?;
        self.regions.remove(&cap.object());
        Ok(())
    }

    /// Drops the bytes of every region whose kernel object no longer
    /// exists — i.e. one destroyed with `Kernel::destroy_object` directly
    /// rather than `destroy_region`. Such bytes were already unreachable
    /// (every capability fails `check`); this reclaims the storage.
    /// Returns how many regions were reclaimed.
    pub fn reclaim(&mut self, kernel: &Kernel) -> usize {
        let before = self.regions.len();
        self.regions
            .retain(|object, _| kernel.object_exists(*object));
        before - self.regions.len()
    }

    pub fn region_count(&self) -> usize {
        self.regions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipc::ChannelRegistry;
    use process::{Grant, Scheduler};

    #[test]
    fn a_new_region_is_zero_filled_and_the_right_size() {
        let mut k = Kernel::new();
        let mut mem = RegionRegistry::new();
        let cap = mem.new_region(&mut k, 16);
        assert_eq!(mem.size(&k, &cap), Ok(16));
        assert_eq!(mem.read(&k, &cap, 0, 16), Ok(vec![0; 16]));
    }

    #[test]
    fn written_bytes_read_back() {
        let mut k = Kernel::new();
        let mut mem = RegionRegistry::new();
        let cap = mem.new_region(&mut k, 8);
        mem.write(&k, &cap, 2, &[1, 2, 3]).unwrap();
        assert_eq!(mem.read(&k, &cap, 0, 8), Ok(vec![0, 0, 1, 2, 3, 0, 0, 0]));
    }

    #[test]
    fn a_read_only_view_can_read_but_not_write() {
        let mut k = Kernel::new();
        let mut mem = RegionRegistry::new();
        let owner = mem.new_region(&mut k, 4);
        mem.write(&k, &owner, 0, &[9, 9, 9, 9]).unwrap();
        let view = k.derive(&owner, Rights::READ).unwrap();

        assert_eq!(mem.read(&k, &view, 0, 4), Ok(vec![9; 4]));
        assert!(matches!(
            mem.write(&k, &view, 0, &[0]),
            Err(MemoryError::Capability(CapError::InsufficientRights { .. }))
        ));
        assert_eq!(mem.read(&k, &owner, 0, 4), Ok(vec![9; 4]));
    }

    #[test]
    fn an_out_of_bounds_write_changes_nothing() {
        let mut k = Kernel::new();
        let mut mem = RegionRegistry::new();
        let cap = mem.new_region(&mut k, 4);
        assert_eq!(
            mem.write(&k, &cap, 2, &[7, 7, 7]),
            Err(MemoryError::OutOfBounds {
                object: cap.object(),
                offset: 2,
                len: 3,
                size: 4,
            })
        );
        assert_eq!(mem.read(&k, &cap, 0, 4), Ok(vec![0; 4]));
    }

    #[test]
    fn an_offset_overflowing_usize_is_out_of_bounds_not_a_panic() {
        let mut k = Kernel::new();
        let mut mem = RegionRegistry::new();
        let cap = mem.new_region(&mut k, 4);
        assert!(matches!(
            mem.read(&k, &cap, usize::MAX, 2),
            Err(MemoryError::OutOfBounds { .. })
        ));
    }

    #[test]
    fn authority_is_checked_before_bounds() {
        let mut k = Kernel::new();
        let mut mem = RegionRegistry::new();
        let owner = mem.new_region(&mut k, 4);
        let none = k.derive(&owner, Rights::NONE).unwrap();
        // Out of bounds *and* unauthorized: the rights error wins.
        assert!(matches!(
            mem.read(&k, &none, 100, 1),
            Err(MemoryError::Capability(_))
        ));
    }

    #[test]
    fn a_capability_for_a_non_region_object_is_rejected() {
        let mut k = Kernel::new();
        let mem = RegionRegistry::new();
        let mut channels = ChannelRegistry::new();
        let channel = channels.new_channel(&mut k);
        assert_eq!(
            mem.read(&k, &channel, 0, 0),
            Err(MemoryError::UnknownRegion(channel.object()))
        );
    }

    #[test]
    fn destroy_region_frees_it_and_every_capability_fails_with_unknown_object() {
        let mut k = Kernel::new();
        let mut mem = RegionRegistry::new();
        let owner = mem.new_region(&mut k, 4);
        let view = k.derive(&owner, Rights::READ).unwrap();

        assert!(mem.destroy_region(&mut k, &view).is_err()); // no DESTROY
        mem.destroy_region(&mut k, &owner).unwrap();

        assert_eq!(mem.region_count(), 0);
        assert_eq!(
            mem.read(&k, &view, 0, 1),
            Err(MemoryError::Capability(CapError::UnknownObject(
                owner.object()
            )))
        );
    }

    #[test]
    fn destroy_region_on_a_non_region_object_destroys_nothing() {
        let mut k = Kernel::new();
        let mut mem = RegionRegistry::new();
        let other = k.new_object(Rights::ALL);
        assert_eq!(
            mem.destroy_region(&mut k, &other),
            Err(MemoryError::UnknownRegion(other.object()))
        );
        assert!(k.object_exists(other.object()));
    }

    #[test]
    fn reclaim_drops_regions_destroyed_directly_through_the_kernel() {
        let mut k = Kernel::new();
        let mut mem = RegionRegistry::new();
        let kept = mem.new_region(&mut k, 4);
        let leaked = mem.new_region(&mut k, 4);
        k.destroy_object(&leaked).unwrap();

        assert_eq!(mem.region_count(), 2);
        assert_eq!(mem.reclaim(&k), 1);
        assert_eq!(mem.region_count(), 1);
        assert_eq!(mem.size(&k, &kept), Ok(4));
    }

    #[test]
    fn after_a_move_over_ipc_the_old_owners_kept_copy_cannot_read_or_write() {
        let mut k = Kernel::new();
        let mut mem = RegionRegistry::new();
        let mut channels = ChannelRegistry::new();
        let mut sched = Scheduler::new();

        let region = mem.new_region(&mut k, 4);
        mem.write(&k, &region, 0, b"mine").unwrap();
        let a = sched.spawn(vec![region]);
        let b = sched.spawn(vec![]);
        let channel = channels.new_channel(&mut k);
        let handle = sched.process(a).unwrap().handles().next().unwrap();
        let kept_by_a = sched.process(a).unwrap().capability(handle).unwrap();

        channels
            .send(
                &mut k,
                &channel,
                sched.process_mut(a).unwrap(),
                vec![Grant::Move(handle)],
                0,
            )
            .unwrap();
        channels
            .receive(&k, &channel, sched.process_mut(b).unwrap())
            .unwrap();
        let b_proc = sched.process(b).unwrap();
        let b_cap = b_proc.capability(b_proc.handles().next().unwrap()).unwrap();

        let revoked = Err(MemoryError::Capability(CapError::Revoked(region.object())));
        assert_eq!(mem.read(&k, &kept_by_a, 0, 4), revoked);
        assert_eq!(mem.write(&k, &kept_by_a, 0, b"back"), revoked.map(|_| ()));
        assert_eq!(
            mem.destroy_region(&mut k, &kept_by_a),
            Err(MemoryError::Capability(CapError::Revoked(region.object())))
        );
        // The data moved with the ownership, untouched.
        assert_eq!(mem.read(&k, &b_cap, 0, 4), Ok(b"mine".to_vec()));
        mem.write(&k, &b_cap, 0, b"ours").unwrap();
    }

    #[test]
    fn a_plain_transfer_is_not_enough_which_is_why_move_exists() {
        let mut k = Kernel::new();
        let mut mem = RegionRegistry::new();
        let mut sched = Scheduler::new();

        let region = mem.new_region(&mut k, 1);
        let a = sched.spawn(vec![region]);
        let handle = sched.process(a).unwrap().handles().next().unwrap();
        let kept_by_a = sched.process(a).unwrap().capability(handle).unwrap();
        sched
            .spawn_child(&mut k, a, vec![Grant::Transfer(handle)])
            .unwrap();

        // Transfer removed the table entry, but the kept copy still works:
        // not kernel-enforced ownership.
        assert_eq!(mem.write(&k, &kept_by_a, 0, &[1]), Ok(()));
    }
}
