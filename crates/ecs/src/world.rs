//! The world: entity allocation, archetype storage and component access.

use core::fmt;
use core::mem::{self, MaybeUninit};

use moba_core::Tick;

use crate::Component;
use crate::entity::{Entities, Entity};
use crate::query::{QueryChunks, QueryData, QueryFilter, QueryIter};
use crate::storage::{Archetype, Registry, VecColumn};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Location {
    pub(crate) archetype: u32,
    pub(crate) row: u32,
}

impl Location {
    pub(crate) const INVALID: Self = Self {
        archetype: u32::MAX,
        row: 0,
    };

    #[inline]
    pub(crate) const fn is_valid(self) -> bool {
        self.archetype != u32::MAX
    }
}

/// An ECS world: a set of entities, their components and the archetypes that index them.
///
/// A world is deliberately single-threaded at the API level: every mutating operation
/// requires `&mut World`, and queries borrow the world for their lifetime. Parallel
/// systems will use a scheduler-owned "world cell" in a later milestone, not `&World`.
pub struct World {
    pub(crate) entities: Entities,
    pub(crate) archetypes: Vec<Archetype>,
    locations: Vec<Location>,
    registry: Registry,
    empty_archetype: usize,
    pub(crate) tick: u32,
}

impl World {
    /// Creates an empty world with no registered component types.
    #[must_use]
    pub fn new() -> Self {
        let registry = Registry::new();
        let empty_archetype = Archetype::new(0, &registry);
        Self {
            entities: Entities::new(),
            archetypes: vec![empty_archetype],
            locations: Vec::new(),
            registry,
            empty_archetype: 0,
            tick: 0,
        }
    }

    /// Creates a world with capacity for `entities` entities that have no components yet.
    #[must_use]
    pub fn with_capacity(entities: usize) -> Self {
        let mut world = Self::new();
        world.archetypes[world.empty_archetype].reserve(entities);
        world.locations.reserve(entities);
        world
    }

    /// Current simulation tick, used to stamp component changes.
    #[inline]
    #[must_use]
    pub const fn tick(&self) -> Tick {
        Tick::new(self.tick)
    }

    /// Sets the current tick (rollback, replay seeking, tests).
    ///
    /// Change ticks written after this call use the new value. Rewinding the tick does not
    /// retroactively erase change stamps, which is what rollback and snapshot diffing
    /// expect: a component changed in the future is simply newer than any past baseline.
    pub fn set_tick(&mut self, tick: Tick) {
        self.tick = tick.get();
    }

    /// Advances the tick by one and returns it.
    pub fn advance_tick(&mut self) -> Tick {
        self.tick = self.tick.wrapping_add(1);
        Tick::new(self.tick)
    }

    /// Spawns an entity with no components.
    pub fn spawn(&mut self) -> Entity {
        let entity = self.entities.spawn();
        let index = entity.index() as usize;
        if self.locations.len() <= index {
            self.locations.resize(index + 1, Location::INVALID);
        }
        let archetype = &mut self.archetypes[self.empty_archetype];
        let row = archetype.entities.len();
        archetype.entities.push(entity);
        self.locations[index] = Location {
            archetype: self.empty_archetype as u32,
            row: u32::try_from(row).expect("row count fits u32"),
        };
        entity
    }

    /// Despawns an entity and drops all of its components.
    ///
    /// Returns `false` if the entity is not alive (stale handle or already despawned).
    pub fn despawn(&mut self, entity: Entity) -> bool {
        if !self.entities.contains(entity) {
            return false;
        }
        let Some(location) = self.location_of(entity) else {
            return false;
        };
        let archetype_index = location.archetype as usize;
        let row = location.row as usize;
        let component_count = self.archetypes[archetype_index].component_ids().len();
        for column_index in 0..component_count {
            let id = self.archetypes[archetype_index].component_ids()[column_index];
            let column = self.archetypes[archetype_index]
                .column_mut(id)
                .expect("archetype lists a component without a column");
            column.swap_remove_drop(row);
        }
        let moved = self.archetypes[archetype_index].entities.swap_remove(row);
        debug_assert_eq!(moved, entity);
        if row < self.archetypes[archetype_index].entities.len() {
            let moved_entity = self.archetypes[archetype_index].entities[row];
            self.locations[moved_entity.index() as usize].row =
                u32::try_from(row).expect("row count fits u32");
        }
        self.locations[entity.index() as usize] = Location::INVALID;
        self.entities.despawn(entity);
        true
    }

    /// Returns `true` if the handle refers to a live entity.
    #[inline]
    #[must_use]
    pub fn is_alive(&self, entity: Entity) -> bool {
        self.entities.contains(entity) && self.location_of(entity).is_some()
    }

    /// Number of live entities.
    #[inline]
    #[must_use]
    pub const fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Number of archetypes, including the empty one.
    #[inline]
    #[must_use]
    pub fn archetype_count(&self) -> usize {
        self.archetypes.len()
    }

    /// Iterates every live entity.
    ///
    /// Order is archetype order, then row order. It is deterministic for a given sequence
    /// of operations, but not sorted by entity index.
    pub fn entities(&self) -> impl Iterator<Item = Entity> + '_ {
        self.archetypes
            .iter()
            .flat_map(|archetype| archetype.entities.iter().copied())
    }

    /// Returns the archetype mask and row for an entity, if alive.
    #[inline]
    fn location_of(&self, entity: Entity) -> Option<Location> {
        if !self.entities.contains(entity) {
            return None;
        }
        let location = *self.locations.get(entity.index() as usize)?;
        if location.is_valid() {
            Some(location)
        } else {
            None
        }
    }

    /// Registers `T` and returns its component id.
    ///
    /// # Panics
    /// Panics if the world already holds [`MAX_COMPONENTS`] component types.
    pub(crate) fn component_id<T: Component>(&mut self) -> u32 {
        self.registry.register::<T>()
    }

    /// Returns the registered id for `T` without registering it.
    #[inline]
    fn registered_id<T: Component>(&self) -> Option<u32> {
        self.registry.id_of::<T>()
    }

    /// Returns the component id for `T`, registering it if needed.
    ///
    /// Useful for tooling and diagnostics; gameplay code normally gets ids implicitly
    /// through [`World::insert`] and [`World::query`].
    pub fn register_component<T: Component>(&mut self) -> u32 {
        self.component_id::<T>()
    }

    fn ensure_archetype(&mut self, mask: u64) -> usize {
        if let Some(index) = self
            .archetypes
            .iter()
            .position(|archetype| archetype.mask == mask)
        {
            return index;
        }
        let archetype = Archetype::new(mask, &self.registry);
        self.archetypes.push(archetype);
        self.archetypes.len() - 1
    }

    /// Returns two distinct archetypes mutably.
    ///
    /// # Panics
    /// Panics if the indices are equal or out of bounds.
    fn archetype_pair(&mut self, a: usize, b: usize) -> (&mut Archetype, &mut Archetype) {
        assert_ne!(a, b, "an entity cannot move to its own archetype");
        if a < b {
            let (head, tail) = self.archetypes.split_at_mut(b);
            (&mut head[a], &mut tail[0])
        } else {
            let (head, tail) = self.archetypes.split_at_mut(a);
            (&mut tail[0], &mut head[b])
        }
    }

    /// Inserts a component, returning the previous value if the entity already had one.
    ///
    /// If the entity already has `T`, the value is replaced in place and the returned old
    /// value is the caller's to drop. Otherwise the entity moves to the archetype that
    /// also contains `T`.
    ///
    /// # Panics
    /// Panics if `entity` is not alive. Callers must not pass stale handles.
    pub fn insert<T: Component>(&mut self, entity: Entity, value: T) -> Option<T> {
        let id = self.component_id::<T>();
        let Some(location) = self.location_of(entity) else {
            panic!("insert on a dead entity: {entity:?}");
        };
        let source_index = location.archetype as usize;
        let row = location.row as usize;
        let tick = self.tick;
        let bit = 1u64 << id;

        if self.archetypes[source_index].mask & bit != 0 {
            let column = self.archetypes[source_index]
                .column_mut(id)
                .expect("mask says the component exists");
            let column = column
                .as_any_mut()
                .downcast_mut::<VecColumn<T>>()
                .expect("component id maps to a column of a different type");
            let old = mem::replace(&mut column.data[row], value);
            column.changed[row] = tick;
            return Some(old);
        }

        let target_index = self.ensure_archetype(self.archetypes[source_index].mask | bit);
        self.archetypes[target_index].reserve(1);

        {
            let (source, target) = self.archetype_pair(source_index, target_index);
            let component_count = source.component_ids().len();
            for column_index in 0..component_count {
                let component = source.component_ids()[column_index];
                let value_ptr = source
                    .column(component)
                    .expect("archetype lists a component without a column")
                    .value_ptr(row);
                // SAFETY: the source row holds a live value. The target reserves one slot
                // and stores the same component type, so this is a single ownership move.
                unsafe {
                    target
                        .column_mut(component)
                        .expect("target archetype is missing a moved component")
                        .push_from(value_ptr, tick);
                }
                source
                    .column_mut(component)
                    .expect("source column disappeared")
                    .swap_remove_forget(row);
            }
        }

        let new_row = self.archetypes[target_index].len();
        {
            let target = &mut self.archetypes[target_index];
            let value_ptr = core::ptr::from_ref(&value).cast::<u8>();
            // SAFETY: `value` is a live, owned `T`; ownership moves into the new column and
            // is forgotten below so it is not dropped twice.
            unsafe {
                target
                    .column_mut(id)
                    .expect("new archetype is missing the inserted component")
                    .push_from(value_ptr, tick);
            }
        }
        mem::forget(value);
        self.archetypes[target_index].entities.push(entity);
        self.locations[entity.index() as usize] = Location {
            archetype: u32::try_from(target_index).expect("archetype count fits u32"),
            row: u32::try_from(new_row).expect("row count fits u32"),
        };
        self.remove_entity_row(source_index, row, entity);
        None
    }

    /// Removes a component from an entity, returning it.
    ///
    /// Returns `None` if the entity is dead or does not have `T`.
    pub fn remove<T: Component>(&mut self, entity: Entity) -> Option<T> {
        let id = self.registered_id::<T>()?;
        let location = self.location_of(entity)?;
        let source_index = location.archetype as usize;
        let row = location.row as usize;
        let bit = 1u64 << id;
        if self.archetypes[source_index].mask & bit == 0 {
            return None;
        }
        let tick = self.tick;
        let target_index = self.ensure_archetype(self.archetypes[source_index].mask & !bit);
        self.archetypes[target_index].reserve(1);

        {
            let (source, target) = self.archetype_pair(source_index, target_index);
            let component_count = source.component_ids().len();
            for column_index in 0..component_count {
                let component = source.component_ids()[column_index];
                if component == id {
                    continue;
                }
                let value_ptr = source
                    .column(component)
                    .expect("archetype lists a component without a column")
                    .value_ptr(row);
                // SAFETY: same ownership move as in `insert`; the removed component is
                // handled separately below.
                unsafe {
                    target
                        .column_mut(component)
                        .expect("target archetype is missing a moved component")
                        .push_from(value_ptr, tick);
                }
                source
                    .column_mut(component)
                    .expect("source column disappeared")
                    .swap_remove_forget(row);
            }
        }

        let mut removed = MaybeUninit::<T>::uninit();
        {
            let column = self.archetypes[source_index]
                .column_mut(id)
                .expect("mask says the component exists");
            // SAFETY: `removed` is valid uninitialised storage for exactly one `T`, and
            // `swap_remove_to` writes the value out and shrinks the column.
            unsafe { column.swap_remove_to(row, removed.as_mut_ptr().cast::<u8>()) };
        }
        let new_row = self.archetypes[target_index].len();
        self.archetypes[target_index].entities.push(entity);
        self.locations[entity.index() as usize] = Location {
            archetype: u32::try_from(target_index).expect("archetype count fits u32"),
            row: u32::try_from(new_row).expect("row count fits u32"),
        };
        self.remove_entity_row(source_index, row, entity);
        // SAFETY: `swap_remove_to` initialised `removed` on the success path above.
        Some(unsafe { removed.assume_init() })
    }

    /// Removes the entity record from `archetype` and fixes up the swapped entity.
    fn remove_entity_row(&mut self, archetype: usize, row: usize, entity: Entity) {
        let moved = self.archetypes[archetype].entities.swap_remove(row);
        debug_assert_eq!(moved, entity, "entity/archetype row bookkeeping diverged");
        if row < self.archetypes[archetype].entities.len() {
            let moved_entity = self.archetypes[archetype].entities[row];
            self.locations[moved_entity.index() as usize].row =
                u32::try_from(row).expect("row count fits u32");
        }
    }

    /// Returns a shared reference to `T` for `entity`.
    #[must_use]
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        let id = self.registered_id::<T>()?;
        let location = self.location_of(entity)?;
        self.archetypes[location.archetype as usize].get::<T>(id, location.row as usize)
    }

    /// Returns a mutable reference to `T` for `entity`, marking it changed.
    #[must_use]
    pub fn get_mut<T: Component>(&mut self, entity: Entity) -> Option<&mut T> {
        let id = self.registered_id::<T>()?;
        let location = self.location_of(entity)?;
        let tick = self.tick;
        self.archetypes[location.archetype as usize].get_mut::<T>(id, location.row as usize, tick)
    }

    /// Returns `true` if the entity has a `T`.
    #[must_use]
    pub fn has<T: Component>(&self, entity: Entity) -> bool {
        let Some(id) = self.registered_id::<T>() else {
            return false;
        };
        let Some(location) = self.location_of(entity) else {
            return false;
        };
        self.archetypes[location.archetype as usize].mask & (1u64 << id) != 0
    }

    /// Returns the tick at which `T` was last changed for `entity`.
    #[must_use]
    pub fn changed_at<T: Component>(&self, entity: Entity) -> Option<Tick> {
        let id = self.registered_id::<T>()?;
        let location = self.location_of(entity)?;
        let changed =
            self.archetypes[location.archetype as usize].changed(id, location.row as usize)?;
        Some(Tick::new(changed))
    }

    /// Returns `true` if `T` changed for `entity` after `since`.
    ///
    /// Comparison is wrap-safe. A component inserted at tick 0 counts as changed after
    /// `Tick::ZERO.wrapping_sub(1)`, which lets snapshot code use "last acked tick" as the
    /// baseline without special-casing initial state.
    #[must_use]
    pub fn changed_since<T: Component>(&self, entity: Entity, since: Tick) -> bool {
        self.changed_at::<T>(entity)
            .is_some_and(|changed| changed.is_newer_than(since))
    }

    /// Builds a query over the world.
    ///
    /// # Panics
    /// Panics if the query requires the same component type twice, or both requires and
    /// excludes the same type. Both are programming errors that would break aliasing.
    pub fn query<Q: QueryData>(&mut self) -> QueryIter<'_, Q, ()> {
        self.query_filtered::<Q, ()>()
    }

    /// Builds a filtered query over the world.
    ///
    /// # Panics
    /// Panics if the query requires the same component type twice, or both requires and
    /// excludes the same type.
    pub fn query_filtered<Q: QueryData, F: QueryFilter>(&mut self) -> QueryIter<'_, Q, F> {
        QueryIter::new(self)
    }

    /// Builds a chunked query over the world.
    ///
    /// Prefer this in systems that touch many entities: the items are whole slices, so the
    /// compiler can vectorise the inner loop.
    ///
    /// # Panics
    /// Panics if the query requires the same component type twice, or both requires and
    /// excludes the same type.
    pub fn query_chunks<Q: QueryData>(&mut self) -> QueryChunks<'_, Q, ()> {
        self.query_chunks_filtered::<Q, ()>()
    }

    /// Builds a filtered chunked query over the world.
    ///
    /// # Panics
    /// Panics if the query requires the same component type twice, or both requires and
    /// excludes the same type.
    pub fn query_chunks_filtered<Q: QueryData, F: QueryFilter>(&mut self) -> QueryChunks<'_, Q, F> {
        QueryChunks::new(self)
    }

    /// Number of distinct component types registered so far.
    #[must_use]
    pub fn registered_component_count(&self) -> usize {
        self.registry.len()
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for World {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("World")
            .field("entities", &self.entities.len())
            .field("archetypes", &self.archetypes.len())
            .field("components", &self.registry.len())
            .field("tick", &self.tick)
            .finish_non_exhaustive()
    }
}
