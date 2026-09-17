//! SplitMix64 (Steele, Lea & Flood, OOPSLA 2014): a tiny, fully
//! deterministic PRNG. Hand-rolled rather than pulled from `rand` so a
//! recorded seed replays identically forever, independent of any
//! dependency's version-to-version stream changes.

#[derive(Debug, Clone)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (Lemire's multiply-shift; the tiny bias for
    /// non-power-of-two `n` is irrelevant for test-input generation).
    /// Panics if `n == 0`.
    pub fn below(&mut self, n: usize) -> usize {
        assert!(n > 0, "below(0)");
        ((self.next_u64() as u128 * n as u128) >> 64) as usize
    }

    /// True with probability `percent`/100.
    pub fn percent(&mut self, percent: u64) -> bool {
        self.next_u64() % 100 < percent
    }

    pub fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let bytes = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_stream() {
        let mut a = SplitMix64::new(42);
        let mut b = SplitMix64::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn matches_the_published_reference_stream() {
        // First outputs of SplitMix64 seeded with 0 (reference C
        // implementation by Vigna).
        let mut r = SplitMix64::new(0);
        assert_eq!(r.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(r.next_u64(), 0x6E78_9E6A_A1B9_65F4);
    }

    #[test]
    fn below_stays_in_range() {
        let mut r = SplitMix64::new(7);
        for n in 1..50 {
            for _ in 0..20 {
                assert!(r.below(n) < n);
            }
        }
    }
}
