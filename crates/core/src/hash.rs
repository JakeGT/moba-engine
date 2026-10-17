//! Stable, platform-independent hashing helpers.
//!
//! Determinism tests and snapshot checksums need a hash that never changes between runs,
//! platforms or toolchain versions. `std::collections::hash_map::DefaultHasher` explicitly
//! does not promise that, and `RandomState` is seeded per process. This module implements
//! FNV-1a (64-bit), which is simple, fast enough for debug checksums, and frozen forever.
//!
//! This is *not* a cryptographic hash; do not use it for security decisions.

use core::hash::Hasher;

const FNV_OFFSET: u64 = 0xCBF2_9CE4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;

/// Hashes a byte slice with FNV-1a 64.
#[inline]
#[must_use]
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// A streaming FNV-1a 64 hasher.
#[derive(Clone, Debug)]
pub struct StableHasher(u64);

impl StableHasher {
    /// Creates a hasher initialised with the FNV offset basis.
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self(FNV_OFFSET)
    }

    /// Returns the 64-bit hash of everything written so far.
    #[inline]
    #[must_use]
    pub const fn finish64(&self) -> u64 {
        self.0
    }

    /// Hashes a 32-bit value in little-endian order.
    #[inline]
    pub fn write_u32(&mut self, value: u32) {
        self.write(&value.to_le_bytes());
    }

    /// Hashes a 64-bit value in little-endian order.
    #[inline]
    pub fn write_u64(&mut self, value: u64) {
        self.write(&value.to_le_bytes());
    }

    /// Hashes a fixed-point value by its exact bits.
    #[inline]
    pub fn write_fix(&mut self, value: crate::fix::Fix) {
        self.write_u64(value.to_bits() as u64);
    }

    /// Hashes an `f32` by its exact bit pattern.
    #[inline]
    pub fn write_f32(&mut self, value: f32) {
        self.write_u32(value.to_bits());
    }
}

impl Default for StableHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl Hasher for StableHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_vectors_match_the_reference() {
        assert_eq!(fnv1a64(b""), 0xCBF2_9CE4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xAF63_DC4C_8601_EC8C);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_F739_67E8);
    }

    #[test]
    fn streaming_matches_one_shot() {
        let data = b"the quick brown fox jumps over the lazy dog";
        let mut hasher = StableHasher::new();
        for chunk in data.chunks(7) {
            hasher.write(chunk);
        }
        assert_eq!(hasher.finish64(), fnv1a64(data));
        assert_eq!(hasher.finish(), fnv1a64(data));
    }

    #[test]
    fn typed_writes_are_little_endian_and_stable() {
        let mut hasher = StableHasher::new();
        hasher.write_u32(0x1234_5678);
        let expected = fnv1a64(&[0x78, 0x56, 0x34, 0x12]);
        assert_eq!(hasher.finish64(), expected);

        let mut floats = StableHasher::new();
        floats.write_f32(1.0);
        assert_eq!(floats.finish64(), fnv1a64(&1.0f32.to_bits().to_le_bytes()));
    }
}
