//! Bit sets for archetype masks, visibility and input bitfields.
//!
//! [`BitSet64`] is the important one: the ECS limits a world to 64 component types, which
//! makes archetype matching a single register operation and keeps query construction
//! allocation-free. [`BitSet`] is the growable variant used for per-team visibility and
//! other unbounded masks.

use core::fmt;

/// A fixed 64-bit bit set.
#[derive(Copy, Clone, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct BitSet64(u64);

impl BitSet64 {
    /// No bits set.
    pub const EMPTY: Self = Self(0);
    /// Every bit set.
    pub const FULL: Self = Self(u64::MAX);

    /// Creates a set from raw bits.
    #[inline]
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// Returns the raw bits.
    #[inline]
    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Sets a bit. Returns `false` if `index >= 64` (out of range).
    #[inline]
    pub fn insert(&mut self, index: u32) -> bool {
        if index >= 64 {
            return false;
        }
        self.0 |= 1u64 << index;
        true
    }

    /// Clears a bit and returns whether it was set. Returns `false` if `index >= 64`.
    #[inline]
    pub fn remove(&mut self, index: u32) -> bool {
        if index >= 64 {
            return false;
        }
        let mask = 1u64 << index;
        let was_set = self.0 & mask != 0;
        self.0 &= !mask;
        was_set
    }

    /// Returns whether a bit is set.
    #[inline]
    #[must_use]
    pub const fn contains(self, index: u32) -> bool {
        index < 64 && (self.0 & (1u64 << index)) != 0
    }

    /// Number of set bits.
    #[inline]
    #[must_use]
    pub const fn len(self) -> u32 {
        self.0.count_ones()
    }

    /// Returns `true` if no bits are set.
    #[inline]
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns `true` if every bit in `other` is also set in `self`.
    #[inline]
    #[must_use]
    pub const fn is_superset(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Returns `true` if no bit is set in both sets.
    #[inline]
    #[must_use]
    pub const fn is_disjoint(self, other: Self) -> bool {
        self.0 & other.0 == 0
    }

    /// Union of two sets.
    #[inline]
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Intersection of two sets.
    #[inline]
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Bits in `self` that are not in `other`.
    #[inline]
    #[must_use]
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// Iterates set bit indices in ascending order.
    #[inline]
    #[must_use]
    pub const fn iter(self) -> BitSet64Iter {
        BitSet64Iter(self.0)
    }
}

impl fmt::Debug for BitSet64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

/// Iterator over the set bits of a [`BitSet64`].
#[derive(Clone, Debug)]
pub struct BitSet64Iter(u64);

impl Iterator for BitSet64Iter {
    type Item = u32;

    #[inline]
    fn next(&mut self) -> Option<u32> {
        if self.0 == 0 {
            return None;
        }
        let index = self.0.trailing_zeros();
        self.0 &= self.0 - 1;
        Some(index)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.0.count_ones() as usize;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for BitSet64Iter {}

/// A growable bit set backed by 64-bit words.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct BitSet {
    words: Vec<u64>,
}

impl BitSet {
    /// Creates an empty set.
    #[must_use]
    pub const fn new() -> Self {
        Self { words: Vec::new() }
    }

    /// Creates an empty set with room for `bits` bits.
    #[must_use]
    pub fn with_capacity(bits: usize) -> Self {
        Self {
            words: vec![0; bits.div_ceil(64)],
        }
    }

    /// Sets a bit, growing the set if necessary.
    pub fn insert(&mut self, index: u32) {
        let word = index as usize / 64;
        if word >= self.words.len() {
            self.words.resize(word + 1, 0);
        }
        self.words[word] |= 1u64 << (index % 64);
    }

    /// Clears a bit if the set is large enough.
    pub fn remove(&mut self, index: u32) {
        let word = index as usize / 64;
        if let Some(bits) = self.words.get_mut(word) {
            *bits &= !(1u64 << (index % 64));
        }
    }

    /// Returns whether a bit is set.
    #[inline]
    #[must_use]
    pub fn contains(&self, index: u32) -> bool {
        let word = index as usize / 64;
        self.words
            .get(word)
            .is_some_and(|bits| bits & (1u64 << (index % 64)) != 0)
    }

    /// Clears every bit.
    pub fn clear(&mut self) {
        self.words.fill(0);
    }

    /// Number of set bits.
    #[must_use]
    pub fn count(&self) -> usize {
        self.words
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }

    /// Returns `true` if no bits are set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|&word| word == 0)
    }

    /// Number of words currently allocated.
    #[must_use]
    pub fn word_count(&self) -> usize {
        self.words.len()
    }

    /// Iterates set bit indices in ascending order.
    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.words
            .iter()
            .enumerate()
            .flat_map(|(word_index, &word)| {
                BitSet64Iter(word).map(move |bit| (word_index as u32) * 64 + bit)
            })
    }

    /// Sets this set to the union with `other`.
    pub fn union_with(&mut self, other: &Self) {
        if self.words.len() < other.words.len() {
            self.words.resize(other.words.len(), 0);
        }
        for (dst, src) in self.words.iter_mut().zip(&other.words) {
            *dst |= src;
        }
    }

    /// Sets this set to the intersection with `other`.
    pub fn intersect_with(&mut self, other: &Self) {
        for (index, dst) in self.words.iter_mut().enumerate() {
            *dst &= other.words.get(index).copied().unwrap_or(0);
        }
    }

    /// Sets this set to the difference `self - other`.
    pub fn difference_with(&mut self, other: &Self) {
        for (index, dst) in self.words.iter_mut().enumerate() {
            *dst &= !other.words.get(index).copied().unwrap_or(0);
        }
    }
}

impl fmt::Debug for BitSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl FromIterator<u32> for BitSet {
    fn from_iter<I: IntoIterator<Item = u32>>(iter: I) -> Self {
        let mut set = Self::new();
        for bit in iter {
            set.insert(bit);
        }
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitset64_operations() {
        let mut set = BitSet64::EMPTY;
        assert!(set.insert(0));
        assert!(set.insert(63));
        assert!(!set.insert(64));
        assert_eq!(set.len(), 2);
        assert!(set.contains(0) && set.contains(63) && !set.contains(1));
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![0, 63]);

        let other = BitSet64::from_bits(0b1010);
        assert!(other.is_superset(BitSet64::from_bits(0b0010)));
        assert!(!other.is_superset(set));
        assert!(set.is_disjoint(other));
        assert_eq!(other.union(BitSet64::from_bits(0b0101)).bits(), 0b1111);
        assert_eq!(
            other.intersection(BitSet64::from_bits(0b0110)).bits(),
            0b0010
        );
        assert_eq!(other.difference(BitSet64::from_bits(0b0010)).bits(), 0b1000);
    }

    #[test]
    fn bitset64_iterator_is_exact() {
        let set = BitSet64::from_bits(0b1000_0000_0000_0000_0000_0000_0000_0001);
        let mut iter = set.iter();
        assert_eq!(iter.len(), 2);
        assert_eq!(iter.next(), Some(0));
        assert_eq!(iter.len(), 1);
        assert_eq!(iter.next(), Some(31));
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn bitset_grows_and_combines() {
        let mut a = BitSet::new();
        a.insert(3);
        a.insert(130);
        assert_eq!(a.count(), 2);
        assert!(a.contains(130));
        assert!(!a.contains(129));
        assert_eq!(a.word_count(), 3);

        let b: BitSet = [130, 200].into_iter().collect();
        a.union_with(&b);
        assert_eq!(a.count(), 3);
        a.intersect_with(&b);
        assert_eq!(a.iter().collect::<Vec<_>>(), vec![130, 200]);
        a.difference_with(&b);
        assert!(a.is_empty());

        a.insert(1);
        a.remove(1);
        a.remove(999);
        assert!(a.is_empty());
        a.insert(64);
        assert_eq!(a.iter().collect::<Vec<_>>(), vec![64]);
        a.clear();
        assert_eq!(a.count(), 0);
    }
}
