//! Deterministic pseudo-random number generation.
//!
//! The simulation is reproducible from `(seed, inputs)`, so it may never touch an
//! OS-seeded generator. [`Rng`] is `xoshiro256++` (Blackman & Vigna): fast, high quality,
//! tiny state, and trivially serialisable for replays and rollback. Seeding goes through
//! `SplitMix64` so that adjacent seeds produce unrelated streams.
//!
//! Each system that needs randomness owns a stream, created with [`Rng::split`]; this
//! keeps results independent of scheduling order.

/// A deterministic random number generator.
#[derive(Clone, Debug)]
pub struct Rng {
    state: [u64; 4],
}

/// `SplitMix64`: converts a single seed into a well-distributed 64-bit state word.
const fn split_mix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Rng {
    /// Creates a generator from a 64-bit seed.
    #[must_use]
    pub fn from_seed(seed: u64) -> Self {
        let mut state = seed;
        let mut words = [0u64; 4];
        let mut index = 0;
        while index < 4 {
            words[index] = split_mix64(&mut state);
            index += 1;
        }
        Self { state: words }
    }

    /// Creates a generator from a full 256-bit state.
    ///
    /// An all-zero state is not valid for xoshiro and is remapped to the first `SplitMix64`
    /// word of seed zero. This keeps deserialised states safe without panicking.
    #[must_use]
    pub fn from_state(state: [u64; 4]) -> Self {
        if state == [0; 4] {
            Self::from_seed(0)
        } else {
            Self { state }
        }
    }

    /// Returns the current 256-bit state, for serialisation and rollback.
    #[must_use]
    pub const fn state(&self) -> [u64; 4] {
        self.state
    }

    /// Generates the next 64-bit value.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let [s0, s1, s2, s3] = &mut self.state;
        let result = s0.wrapping_add(*s3).rotate_left(23).wrapping_add(*s0);
        let t = s1.wrapping_shl(17);
        *s2 ^= *s0;
        *s3 ^= *s1;
        *s1 ^= *s2;
        *s0 ^= *s3;
        *s2 ^= t;
        *s3 = s3.rotate_left(45);
        result
    }

    /// Generates the next 32-bit value.
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Generates a `bool` with probability `1/2`.
    #[inline]
    pub fn next_bool(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }

    /// Generates a value in `[0, 1)` with 24 bits of precision.
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        let bits = self.next_u32() >> 8;
        bits as f32 * (1.0 / 16_777_216.0)
    }

    /// Generates a value in `[0, 1)` with 53 bits of precision.
    #[inline]
    pub fn next_f64(&mut self) -> f64 {
        let bits = self.next_u64() >> 11;
        bits as f64 * (1.0 / 9_007_199_254_740_992.0)
    }

    /// Generates a value in `[0, upper)` without modulo bias.
    ///
    /// # Panics
    /// Panics if `upper == 0`.
    #[inline]
    pub fn gen_range_u32(&mut self, upper: u32) -> u32 {
        assert!(upper > 0, "gen_range_u32 requires a non-empty range");
        let range = u64::from(upper);
        loop {
            let product = u64::from(self.next_u32()) * range;
            let low = product as u32;
            if low < upper {
                let threshold = upper.wrapping_neg() % upper;
                if low < threshold {
                    continue;
                }
            }
            return (product >> 32) as u32;
        }
    }

    /// Generates a value in `[low, high)`.
    ///
    /// # Panics
    /// Panics if `low >= high`.
    #[inline]
    pub fn gen_range_i32(&mut self, low: i32, high: i32) -> i32 {
        assert!(low < high, "gen_range_i32 requires a non-empty range");
        let width = (i64::from(high) - i64::from(low)) as u32;
        let value = i64::from(low) + i64::from(self.gen_range_u32(width));
        value as i32
    }

    /// Generates a value in `[low, high)`.
    ///
    /// # Panics
    /// Panics if `low >= high` or either bound is non-finite.
    #[inline]
    pub fn range_f32(&mut self, low: f32, high: f32) -> f32 {
        assert!(
            low.is_finite() && high.is_finite() && low < high,
            "range_f32 requires finite, ascending bounds"
        );
        low + (high - low) * self.next_f32()
    }

    /// Returns `true` with probability `probability` (clamped to `[0, 1]`).
    #[inline]
    pub fn chance(&mut self, probability: f32) -> bool {
        let probability = probability.clamp(0.0, 1.0);
        self.next_f32() < probability
    }

    /// Shuffles a slice in place with Fisher-Yates.
    pub fn shuffle<T>(&mut self, slice: &mut [T]) {
        for index in (1..slice.len()).rev() {
            let other = self.gen_range_u32((index + 1) as u32) as usize;
            slice.swap(index, other);
        }
    }

    /// Fills a byte buffer.
    pub fn fill_bytes(&mut self, out: &mut [u8]) {
        let mut chunks = out.chunks_exact_mut(8);
        for chunk in &mut chunks {
            let value = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&value);
        }
        let remainder = chunks.into_remainder();
        if !remainder.is_empty() {
            let value = self.next_u64().to_le_bytes();
            remainder.copy_from_slice(&value[..remainder.len()]);
        }
    }

    /// Advances the generator by `2^128` steps (xoshiro256++ jump polynomial).
    ///
    /// Useful for deriving independent substreams without storing extra state.
    pub fn jump(&mut self) {
        const JUMP: [u64; 4] = [
            0x180E_C6D3_3CFD_0ABA,
            0xD5A6_1266_F0C9_392C,
            0xA958_2618_E03F_C9AA,
            0x39AB_DC45_29B1_661C,
        ];
        let mut state = [0u64; 4];
        for word in JUMP {
            for bit in 0..64 {
                if word & (1u64 << bit) != 0 {
                    for (dst, src) in state.iter_mut().zip(self.state.iter()) {
                        *dst ^= src;
                    }
                }
                self.next_u64();
            }
        }
        self.state = state;
    }

    /// Creates an independent child generator.
    ///
    /// The child is seeded from the parent's stream, so the parent's sequence remains
    /// reproducible and the child does not disturb it beyond consuming two words.
    #[must_use]
    pub fn split(&mut self) -> Self {
        Self::from_seed(self.next_u64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn same_seed_same_stream() {
        let mut a = Rng::from_seed(0xDEAD_BEEF);
        let mut b = Rng::from_seed(0xDEAD_BEEF);
        for _ in 0..10_000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = Rng::from_seed(1);
        let mut b = Rng::from_seed(2);
        let mut equal = 0;
        for _ in 0..1_000 {
            if a.next_u64() == b.next_u64() {
                equal += 1;
            }
        }
        assert_eq!(equal, 0);
    }

    #[test]
    fn zero_state_is_remapped() {
        let mut rng = Rng::from_state([0; 4]);
        let mut seen = HashSet::new();
        for _ in 0..16 {
            assert!(seen.insert(rng.next_u64()));
        }
    }

    #[test]
    fn ranges_stay_in_bounds() {
        let mut rng = Rng::from_seed(7);
        for upper in [1u32, 2, 3, 10, 1_000, u32::MAX] {
            for _ in 0..1_000 {
                assert!(rng.gen_range_u32(upper) < upper);
            }
        }
        for _ in 0..1_000 {
            let value = rng.gen_range_i32(-50, 51);
            assert!((-50..51).contains(&value));
        }
        for _ in 0..1_000 {
            let value = rng.range_f32(-2.5, 7.5);
            assert!((-2.5..7.5).contains(&value));
        }
    }

    #[test]
    fn distribution_is_roughly_uniform() {
        let mut rng = Rng::from_seed(99);
        let samples = 120_000;
        let mut buckets = [0u32; 12];
        for _ in 0..samples {
            buckets[rng.gen_range_u32(12) as usize] += 1;
        }
        let expected = samples / 12;
        for count in buckets {
            let deviation = (i64::from(count) - i64::from(expected)).unsigned_abs();
            assert!(
                deviation < 800,
                "bucket {count} deviates from {expected} by {deviation}"
            );
        }
    }

    #[test]
    fn shuffle_is_a_permutation() {
        let mut rng = Rng::from_seed(123);
        let mut values: Vec<u32> = (0..1_000).collect();
        rng.shuffle(&mut values);
        let mut sorted = values.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..1_000).collect::<Vec<_>>());
        assert_ne!(values, sorted, "a shuffle should actually reorder");
    }

    #[test]
    fn fill_bytes_covers_the_buffer() {
        let mut rng = Rng::from_seed(5);
        let mut buffer = [0u8; 37];
        rng.fill_bytes(&mut buffer);
        assert!(buffer.iter().any(|&byte| byte != 0));
        let mut zeros = [0u8; 16];
        rng.fill_bytes(&mut zeros);
        assert_ne!(zeros, [0u8; 16]);
    }

    #[test]
    fn split_and_jump_produce_independent_streams() {
        let mut rng = Rng::from_seed(42);
        let mut child = rng.split();
        let mut jumped = rng.clone();
        jumped.jump();
        let mut values = HashSet::new();
        for _ in 0..1_000 {
            assert!(values.insert(rng.next_u64()));
            assert!(values.insert(child.next_u64()));
            assert!(values.insert(jumped.next_u64()));
        }
        assert!(!values.is_empty());
    }

    #[test]
    fn golden_stream_is_stable() {
        // Replays depend on this. Updating the stream requires a replay-format version bump.
        let mut rng = Rng::from_seed(0x0123_4567_89AB_CDEF);
        let expected = [
            0xB2F2_A310_E96B_D1C5,
            0xB540_6246_5B95_0493,
            0x87AC_A4A9_6688_14B0,
            0xF13D_2E24_48A9_CFFB,
        ];
        for value in expected {
            assert_eq!(rng.next_u64(), value);
        }
    }
}
