//! Generation-safe slot arena.
//!
//! An arena hands out stable, copyable keys and reuses removed slots. Each slot carries a
//! generation counter, so a key that outlives its value can never alias a newer value.
//! Unlike a `HashMap`, iteration order here is deterministic (slot order) and lookups are a
//! single indexed read, which makes the arena suitable for simulation-side registries.

use core::fmt;

/// A key into an [`Arena`].
///
/// Keys are plain data, so they can be stored in components, snapshots and network
/// messages without lifetime constraints.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ArenaKey {
    index: u32,
    generation: u32,
}

impl ArenaKey {
    /// An always-invalid key.
    pub const INVALID: Self = Self {
        index: u32::MAX,
        generation: 0,
    };

    /// Slot index this key refers to.
    #[inline]
    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// Generation counter this key was issued with.
    #[inline]
    #[must_use]
    pub const fn generation(self) -> u32 {
        self.generation
    }

    /// Packs the key into a single `u64`, useful for serialisation and hashing.
    #[inline]
    #[must_use]
    pub const fn to_bits(self) -> u64 {
        ((self.generation as u64) << 32) | self.index as u64
    }

    /// Unpacks a key produced by [`ArenaKey::to_bits`].
    #[inline]
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self {
            index: bits as u32,
            generation: (bits >> 32) as u32,
        }
    }
}

impl fmt::Debug for ArenaKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ArenaKey({}#{})", self.index, self.generation)
    }
}

#[derive(Debug)]
enum Slot<T> {
    Occupied { generation: u32, value: T },
    Vacant { generation: u32 },
}

impl<T> Slot<T> {
    #[inline]
    const fn generation(&self) -> u32 {
        match self {
            Self::Occupied { generation, .. } | Self::Vacant { generation } => *generation,
        }
    }
}

/// A dense, generation-checked arena.
///
/// Slot indices are reused but generations prevent stale keys from resolving. The arena
/// never moves the caller's values while they are live; `remove` returns the value by move.
#[derive(Debug)]
pub struct Arena<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    len: usize,
}

impl<T> Arena<T> {
    /// Creates an empty arena.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            len: 0,
        }
    }

    /// Creates an empty arena with room for `capacity` values.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: Vec::with_capacity(capacity),
            free: Vec::new(),
            len: 0,
        }
    }

    /// Inserts a value and returns its key.
    pub fn insert(&mut self, value: T) -> ArenaKey {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            let generation = slot.generation();
            *slot = Slot::Occupied { generation, value };
            self.len += 1;
            ArenaKey { index, generation }
        } else {
            let index = u32::try_from(self.slots.len()).expect("arena exceeded u32::MAX slots");
            self.slots.push(Slot::Occupied {
                generation: 1,
                value,
            });
            self.len += 1;
            ArenaKey {
                index,
                generation: 1,
            }
        }
    }

    /// Removes and returns the value for `key`, or `None` if the key is stale.
    pub fn remove(&mut self, key: ArenaKey) -> Option<T> {
        let slot = self.slots.get_mut(key.index as usize)?;
        if slot.generation() != key.generation || !matches!(slot, Slot::Occupied { .. }) {
            return None;
        }
        let generation = key.generation.wrapping_add(1);
        let previous = core::mem::replace(slot, Slot::Vacant { generation });
        self.free.push(key.index);
        self.len -= 1;
        match previous {
            Slot::Occupied { value, .. } => Some(value),
            Slot::Vacant { .. } => unreachable!("checked occupied above"),
        }
    }

    /// Returns a shared reference to the value for `key`.
    #[inline]
    #[must_use]
    pub fn get(&self, key: ArenaKey) -> Option<&T> {
        match self.slots.get(key.index as usize)? {
            Slot::Occupied { generation, value } if *generation == key.generation => Some(value),
            _ => None,
        }
    }

    /// Returns an exclusive reference to the value for `key`.
    #[inline]
    #[must_use]
    pub fn get_mut(&mut self, key: ArenaKey) -> Option<&mut T> {
        match self.slots.get_mut(key.index as usize)? {
            Slot::Occupied { generation, value } if *generation == key.generation => Some(value),
            _ => None,
        }
    }

    /// Returns `true` if `key` refers to a live value.
    #[inline]
    #[must_use]
    pub fn contains(&self, key: ArenaKey) -> bool {
        self.get(key).is_some()
    }

    /// Number of live values.
    #[inline]
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the arena holds no values.
    #[inline]
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Number of slots, occupied or free.
    #[inline]
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Reserves room for at least `additional` more values without reallocating slots.
    pub fn reserve(&mut self, additional: usize) {
        self.slots.reserve(additional);
        self.free.reserve(additional);
    }

    /// Removes every value and invalidates every outstanding key.
    pub fn clear(&mut self) {
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if matches!(slot, Slot::Occupied { .. }) {
                let generation = slot.generation().wrapping_add(1);
                *slot = Slot::Vacant { generation };
                self.free
                    .push(u32::try_from(index).expect("slot count fits u32"));
            }
        }
        self.len = 0;
    }

    /// Iterates over live `(key, value)` pairs in slot order.
    pub fn iter(&self) -> impl Iterator<Item = (ArenaKey, &T)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| match slot {
                Slot::Occupied { generation, value } => Some((
                    ArenaKey {
                        index: u32::try_from(index).ok()?,
                        generation: *generation,
                    },
                    value,
                )),
                Slot::Vacant { .. } => None,
            })
    }

    /// Iterates over live `(key, value)` pairs mutably in slot order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (ArenaKey, &mut T)> + '_ {
        self.slots
            .iter_mut()
            .enumerate()
            .filter_map(|(index, slot)| match slot {
                Slot::Occupied { generation, value } => Some((
                    ArenaKey {
                        index: u32::try_from(index).ok()?,
                        generation: *generation,
                    },
                    value,
                )),
                Slot::Vacant { .. } => None,
            })
    }

    /// Iterates over live values.
    pub fn values(&self) -> impl Iterator<Item = &T> + '_ {
        self.iter().map(|(_, value)| value)
    }

    /// Iterates over live values mutably.
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut T> + '_ {
        self.iter_mut().map(|(_, value)| value)
    }

    /// Iterates over live keys in slot order.
    pub fn keys(&self) -> impl Iterator<Item = ArenaKey> + '_ {
        self.iter().map(|(key, _)| key)
    }
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Extend<T> for Arena<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for value in iter {
            self.insert(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    struct DropCounter(Rc<Cell<u32>>);

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn stale_keys_are_rejected_and_slots_reused() {
        let mut arena = Arena::new();
        let a = arena.insert("a");
        let b = arena.insert("b");
        assert_eq!(arena.get(a), Some(&"a"));
        assert_eq!(arena.remove(a), Some("a"));
        assert_eq!(arena.remove(a), None, "double remove must fail");
        assert_eq!(arena.get(a), None);

        let c = arena.insert("c");
        assert_eq!(c.index(), a.index(), "free slot should be reused");
        assert_ne!(c.generation(), a.generation());
        assert_eq!(arena.get(c), Some(&"c"));
        assert_eq!(arena.get(a), None, "stale key must not alias new value");
        assert_eq!(arena.len(), 2);
        assert!(arena.contains(b));
    }

    #[test]
    fn values_are_dropped_exactly_once() {
        let drops = Rc::new(Cell::new(0));
        {
            let mut arena = Arena::new();
            let a = arena.insert(DropCounter(drops.clone()));
            let _b = arena.insert(DropCounter(drops.clone()));
            drop(arena.remove(a));
            assert_eq!(drops.get(), 1);
            arena.clear();
            assert_eq!(drops.get(), 2);
        }
        assert_eq!(drops.get(), 2);
    }

    #[test]
    fn clear_invalidates_everything() {
        let mut arena = Arena::new();
        let keys: Vec<_> = (0..8).map(|i| arena.insert(i)).collect();
        arena.clear();
        assert!(arena.is_empty());
        assert!(keys.iter().all(|&k| arena.get(k).is_none()));
        let new_key = arena.insert(42);
        assert_eq!(arena.get(new_key), Some(&42));
        assert!(!arena.contains(keys[0]));
    }

    #[test]
    fn iteration_visits_live_values_in_slot_order() {
        let mut arena = Arena::new();
        let a = arena.insert(1);
        let b = arena.insert(2);
        let c = arena.insert(3);
        arena.remove(a);
        let d = arena.insert(4);
        assert_eq!(c.index(), 2);
        assert_eq!(d.index(), 0);

        let values: Vec<_> = arena.values().copied().collect();
        assert_eq!(values, vec![4, 2, 3]);
        let keys: Vec<_> = arena.keys().collect();
        assert_eq!(keys, vec![d, b, c]);
        for (_, value) in arena.iter_mut() {
            *value += 10;
        }
        let values: Vec<_> = arena.values().copied().collect();
        assert_eq!(values, vec![14, 12, 13]);
    }

    #[test]
    fn key_bits_round_trip() {
        let key = ArenaKey {
            index: 123,
            generation: 456,
        };
        assert_eq!(ArenaKey::from_bits(key.to_bits()), key);
    }
}
