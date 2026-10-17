//! Entity identifiers and the generational entity allocator.

use core::fmt;

/// A generational entity handle.
///
/// The low 32 bits index the entity slot; the high 32 bits are a generation. When a slot
/// is reused the generation increments, so a stale handle from a despawned entity can
/// never address the new occupant. Handles are `Copy` and safe to store in components,
/// snapshots and network packets.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Entity {
    index: u32,
    generation: u32,
}

impl Entity {
    /// An entity handle that is never alive.
    pub const PLACEHOLDER: Self = Self {
        index: u32::MAX,
        generation: 0,
    };

    /// Creates a handle from raw parts. Prefer [`World::spawn`](crate::World::spawn).
    #[inline]
    #[must_use]
    pub const fn from_parts(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    /// Slot index of the entity.
    #[inline]
    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// Generation of the entity slot when this handle was issued.
    #[inline]
    #[must_use]
    pub const fn generation(self) -> u32 {
        self.generation
    }

    /// Packs the handle into a `u64` for serialisation.
    #[inline]
    #[must_use]
    pub const fn to_bits(self) -> u64 {
        ((self.generation as u64) << 32) | self.index as u64
    }

    /// Unpacks a handle produced by [`Entity::to_bits`].
    #[inline]
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self {
            index: bits as u32,
            generation: (bits >> 32) as u32,
        }
    }
}

impl fmt::Debug for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Entity({}v{})", self.index, self.generation)
    }
}

impl fmt::Display for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}v{}", self.index, self.generation)
    }
}

/// Allocates and recycles entity slots.
#[derive(Debug, Default)]
pub(crate) struct Entities {
    generations: Vec<u32>,
    free: Vec<u32>,
    len: usize,
}

impl Entities {
    pub(crate) const fn new() -> Self {
        Self {
            generations: Vec::new(),
            free: Vec::new(),
            len: 0,
        }
    }

    pub(crate) fn spawn(&mut self) -> Entity {
        if let Some(index) = self.free.pop() {
            let generation = self.generations[index as usize];
            self.len += 1;
            Entity { index, generation }
        } else {
            let index = u32::try_from(self.generations.len()).expect("entity count fits u32");
            self.generations.push(1);
            self.len += 1;
            Entity {
                index,
                generation: 1,
            }
        }
    }

    pub(crate) fn contains(&self, entity: Entity) -> bool {
        self.generations
            .get(entity.index as usize)
            .is_some_and(|&generation| generation == entity.generation)
    }

    pub(crate) fn despawn(&mut self, entity: Entity) -> bool {
        if !self.contains(entity) {
            return false;
        }
        let slot = &mut self.generations[entity.index as usize];
        *slot = slot.wrapping_add(1);
        // Wrap to 0 would collide with `PLACEHOLDER`'s generation; skip it.
        if *slot == 0 {
            *slot = 1;
        }
        self.free.push(entity.index);
        self.len -= 1;
        true
    }

    pub(crate) const fn len(&self) -> usize {
        self.len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generations_reject_stale_handles() {
        let mut entities = Entities::new();
        let first = entities.spawn();
        assert!(entities.contains(first));
        assert!(entities.despawn(first));
        assert!(!entities.contains(first));
        let second = entities.spawn();
        assert_eq!(second.index(), first.index());
        assert_ne!(second.generation(), first.generation());
        assert!(entities.contains(second));
        assert!(!entities.contains(first));
        assert_eq!(entities.len(), 1);
    }

    #[test]
    fn bits_round_trip() {
        let entity = Entity::from_parts(7, 9);
        assert_eq!(Entity::from_bits(entity.to_bits()), entity);
        assert_eq!(entity.to_string(), "7v9");
    }
}
