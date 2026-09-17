//! The pipeline's actual data work: shared, pure functions, so the
//! kernel pipeline, the ambient-authority baseline and the reference
//! computation all do byte-for-byte the same work. Any difference in
//! their output is then a bug in the plumbing, not in the arithmetic.

use crate::rng::SplitMix64;

/// Fills `buf` with batch `batch`'s deterministic input bytes.
pub fn fill_batch(seed: u64, batch: u64, buf: &mut [u8]) {
    SplitMix64::new(seed ^ batch.wrapping_mul(0xD6E8_FEB8_6659_FD93)).fill(buf);
}

/// Stage `stage`'s in-place transform. Deliberately order-dependent (a
/// running accumulator), so applying stages out of order, twice, or on
/// the wrong batch's bytes changes the final checksum.
pub fn transform(stage: usize, buf: &mut [u8]) {
    let key = (stage as u8).wrapping_mul(0x9D).wrapping_add(0x3B);
    let mut acc = key;
    for byte in buf {
        acc = acc.rotate_left(3) ^ *byte;
        *byte = acc.wrapping_add(key);
    }
}

/// FNV-1a, 64-bit.
pub fn checksum(buf: &[u8]) -> u64 {
    buf.iter().fold(0xCBF2_9CE4_8422_2325, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

/// The expected checksum for one batch, computed directly: no processes,
/// no channels, no capabilities. The independent reference both pipeline
/// implementations are checked against.
pub fn reference_checksum(seed: u64, batch: u64, region_size: usize, stages: usize) -> u64 {
    let mut buf = vec![0; region_size];
    fill_batch(seed, batch, &mut buf);
    for stage in 0..stages {
        transform(stage, &mut buf);
    }
    checksum(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_matches_known_vectors() {
        assert_eq!(checksum(b""), 0xCBF2_9CE4_8422_2325);
        assert_eq!(checksum(b"a"), 0xAF63_DC4C_8601_EC8C);
    }

    #[test]
    fn transforms_are_order_sensitive() {
        let mut a = vec![0; 64];
        fill_batch(1, 1, &mut a);
        let mut b = a.clone();
        transform(0, &mut a);
        transform(1, &mut a);
        transform(1, &mut b);
        transform(0, &mut b);
        assert_ne!(checksum(&a), checksum(&b));
    }

    #[test]
    fn different_batches_get_different_data() {
        assert_ne!(
            reference_checksum(9, 0, 32, 2),
            reference_checksum(9, 1, 32, 2)
        );
    }
}
