//! Typed queries and query filters.
//!
//! A query selects every entity whose archetype contains all required components and none
//! of the excluded ones. The query compiles to a 64-bit mask plus a small array of
//! component ids. When iteration enters an archetype it resolves the columns **once**,
//! producing typed slices, and then walks rows through those slices. No per-row hash
//! lookup, virtual call or bounds-scan happens in the loop.
//!
//! ```ignore
//! for (entity, (position, velocity)) in world.query::<(&mut Position, &Velocity)>() {
//!     position.x += velocity.x;
//! }
//! ```
//!
//! Queries reject duplicate component types at construction. That check is what makes the
//! interior-mutable column access sound: a tuple can never hand out two references to the
//! same component row.

use core::fmt;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};

use crate::storage::{Archetype, MAX_COMPONENTS};
use crate::{Component, Entity, World};

mod sealed {
    pub trait Sealed {}
}

/// Describes the components a query fetches, plus how to resolve and fetch them.
///
/// This trait is sealed: only references, mutable references and tuples of them
/// implemented by this crate can be queries. Implementing it incorrectly would break the
/// aliasing invariant that makes the ECS sound.
pub trait QueryData: sealed::Sealed {
    /// Item yielded for one entity.
    type Item<'a>;

    /// Per-archetype state: resolved column slices. Built once, then reused for every row.
    type Fetch<'a>;

    /// Number of component types in this query.
    const COMPONENTS: usize;

    /// Registers the components and writes their ids into `ids` starting at `offset`.
    #[doc(hidden)]
    fn init(world: &mut World, ids: &mut [u32; MAX_COMPONENTS], offset: usize) -> u64;

    /// Resolves this query's columns for one matching archetype.
    ///
    /// # Safety
    /// The caller must guarantee that the archetype matches the query and that the world
    /// is exclusively borrowed for the lifetime of the returned fetch state.
    #[doc(hidden)]
    unsafe fn init_fetch<'a>(archetype: &'a Archetype, ids: &[u32], tick: u32) -> Self::Fetch<'a>;

    /// Number of rows the fetch state covers.
    #[doc(hidden)]
    fn len(fetch: &Self::Fetch<'_>) -> usize;

    /// Fetches the item for one row.
    ///
    /// # Safety
    /// The caller must guarantee that `row` is in bounds and is yielded at most once for
    /// this fetch state.
    #[doc(hidden)]
    unsafe fn fetch<'a>(fetch: &mut Self::Fetch<'a>, row: usize) -> Self::Item<'a>;
}

impl<T: Component> sealed::Sealed for &T {}
impl<T: Component> sealed::Sealed for &mut T {}

/// The resolved mutable slice for one component in one archetype.
///
/// Dereferences to `[T]`, so normal slice APIs work. The change-tick array is an
/// implementation detail: writes through [`DerefMut`] are stamped automatically when the
/// query yields the row.
pub struct MutSlice<'a, T> {
    data: &'a mut [T],
    changed: &'a mut [u32],
    tick: u32,
}

impl<T> Deref for MutSlice<'_, T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        self.data
    }
}

impl<T> DerefMut for MutSlice<'_, T> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.data
    }
}

impl<T> fmt::Debug for MutSlice<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MutSlice")
            .field("len", &self.data.len())
            .finish_non_exhaustive()
    }
}

impl<T: Component> QueryData for &T {
    type Item<'a> = &'a T;
    type Fetch<'a> = &'a [T];
    const COMPONENTS: usize = 1;

    fn init(world: &mut World, ids: &mut [u32; MAX_COMPONENTS], offset: usize) -> u64 {
        let id = world.component_id::<T>();
        ids[offset] = id;
        1u64 << id
    }

    unsafe fn init_fetch<'a>(archetype: &'a Archetype, ids: &[u32], _tick: u32) -> Self::Fetch<'a> {
        let column = archetype
            .column(ids[0])
            .expect("query required a component that the archetype does not store");
        let len = column.len();
        let ptr = column.as_ptr().cast::<T>();
        // SAFETY: the column for this id stores `T` and owns `len` contiguous values. The
        // world is exclusively borrowed for `'a`, so the slice cannot alias a mutation.
        unsafe { core::slice::from_raw_parts(ptr, len) }
    }

    fn len(fetch: &Self::Fetch<'_>) -> usize {
        fetch.len()
    }

    unsafe fn fetch<'a>(fetch: &mut Self::Fetch<'a>, row: usize) -> Self::Item<'a> {
        let base = fetch.as_ptr();
        // SAFETY: the query iterator only calls this with `row < len`.
        let ptr = unsafe { base.add(row) };
        // SAFETY: pointer arithmetic stayed inside the slice, each row is yielded once,
        // and the reference lives for the exclusive world borrow.
        unsafe { &*ptr }
    }
}

impl<T: Component> QueryData for &mut T {
    type Item<'a> = &'a mut T;
    type Fetch<'a> = MutSlice<'a, T>;
    const COMPONENTS: usize = 1;

    fn init(world: &mut World, ids: &mut [u32; MAX_COMPONENTS], offset: usize) -> u64 {
        let id = world.component_id::<T>();
        ids[offset] = id;
        1u64 << id
    }

    unsafe fn init_fetch<'a>(archetype: &'a Archetype, ids: &[u32], tick: u32) -> Self::Fetch<'a> {
        let column = archetype
            .column_mut(ids[0])
            .expect("query required a component that the archetype does not store");
        let len = column.len();
        let data_ptr = column.as_mut_ptr().cast::<T>();
        let changed_ptr = column.changed_ptr_mut();
        // SAFETY: the column owns `len` contiguous `T`s and the world is exclusively
        // borrowed for `'a`; duplicate query components are rejected before iteration.
        let data = unsafe { core::slice::from_raw_parts_mut(data_ptr, len) };
        // SAFETY: the change-tick array is exactly as long as the data column and is
        // exclusively reachable through this same borrow.
        let changed = unsafe { core::slice::from_raw_parts_mut(changed_ptr, len) };
        MutSlice {
            data,
            changed,
            tick,
        }
    }

    fn len(fetch: &Self::Fetch<'_>) -> usize {
        fetch.data.len()
    }

    unsafe fn fetch<'a>(fetch: &mut Self::Fetch<'a>, row: usize) -> Self::Item<'a> {
        fetch.changed[row] = fetch.tick;
        let base = fetch.data.as_mut_ptr();
        // SAFETY: the query iterator only calls this with `row < len`.
        let ptr = unsafe { base.add(row) };
        // SAFETY: the pointer is in bounds, each row is yielded once, and the world is
        // exclusively borrowed, so this is the only reference to the value.
        unsafe { &mut *ptr }
    }
}

macro_rules! impl_query_tuple {
    ($first:ident $(, $rest:ident)*) => {
        impl<$first: QueryData $(, $rest: QueryData)*> sealed::Sealed for ($first, $($rest,)*) {}

        impl<$first: QueryData $(, $rest: QueryData)*> QueryData for ($first, $($rest,)*) {
            type Item<'a> = ($first::Item<'a>, $($rest::Item<'a>,)*);
            type Fetch<'a> = ($first::Fetch<'a>, $($rest::Fetch<'a>,)*);
            const COMPONENTS: usize = $first::COMPONENTS $(+ $rest::COMPONENTS)*;

            #[allow(unused_assignments)] // The final cursor bump is intentionally dead.
            fn init(
                world: &mut World,
                ids: &mut [u32; MAX_COMPONENTS],
                offset: usize,
            ) -> u64 {
                let mut mask = 0u64;
                let mut cursor = offset;
                mask |= $first::init(world, ids, cursor);
                cursor += $first::COMPONENTS;
                $(
                    mask |= $rest::init(world, ids, cursor);
                    cursor += $rest::COMPONENTS;
                )*
                mask
            }

            #[allow(unused_assignments)] // The final cursor bump is intentionally dead.
            unsafe fn init_fetch<'a>(
                archetype: &'a Archetype,
                ids: &[u32],
                tick: u32,
            ) -> Self::Fetch<'a> {
                let mut cursor = 0usize;
                let first_len = $first::COMPONENTS;
                let first_ids = &ids[..first_len];
                cursor += first_len;
                (
                    // SAFETY: `init` laid the tuple's ids out in this same order and the
                    // archetype was matched against all of them.
                    unsafe { $first::init_fetch(archetype, first_ids, tick) },
                    $({
                        let len = $rest::COMPONENTS;
                        let component_ids = &ids[cursor..cursor + len];
                        cursor += len;
                        // SAFETY: each element has its own disjoint slice of the id list
                        // and was registered by `init` in this exact order.
                        unsafe { $rest::init_fetch(archetype, component_ids, tick) }
                    },)*
                )
            }

            fn len(fetch: &Self::Fetch<'_>) -> usize {
                #[allow(non_snake_case)]
                let ($first, $($rest,)*) = fetch;
                let _ = ($($rest,)*);
                $first::len($first)
            }

            unsafe fn fetch<'a>(
                fetch: &mut Self::Fetch<'a>,
                row: usize,
            ) -> Self::Item<'a> {
                #[allow(non_snake_case)]
                let ($first, $($rest,)*) = fetch;
                (
                    // SAFETY: every element was resolved from the same matching
                    // archetype, and the row is yielded once.
                    unsafe { $first::fetch($first, row) },
                    $(
                        // SAFETY: same matched archetype, and the row is yielded once.
                        unsafe { $rest::fetch($rest, row) },
                    )*
                )
            }
        }
    };
}

impl_query_tuple!(A, B);
impl_query_tuple!(A, B, C);
impl_query_tuple!(A, B, C, D);
impl_query_tuple!(A, B, C, D, E);
impl_query_tuple!(A, B, C, D, E, F);
impl_query_tuple!(A, B, C, D, E, F, G);
impl_query_tuple!(A, B, C, D, E, F, G, H);

/// A filter applied to the set of archetypes a query visits.
///
/// Filters resolve to masks when the query is built. They never touch component data.
pub trait QueryFilter {
    /// Returns `(required, excluded)` component masks.
    #[doc(hidden)]
    fn init(world: &mut World) -> (u64, u64);
}

impl QueryFilter for () {
    fn init(_world: &mut World) -> (u64, u64) {
        (0, 0)
    }
}

/// Requires the presence of `T`.
pub struct With<T: Component>(PhantomData<fn() -> T>);

impl<T: Component> With<T> {
    /// Creates the filter.
    #[must_use]
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T: Component> Default for With<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Component> fmt::Debug for With<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("With")
    }
}

impl<T: Component> QueryFilter for With<T> {
    fn init(world: &mut World) -> (u64, u64) {
        (1u64 << world.component_id::<T>(), 0)
    }
}

/// Excludes any entity that has `T`.
pub struct Without<T: Component>(PhantomData<fn() -> T>);

impl<T: Component> Without<T> {
    /// Creates the filter.
    #[must_use]
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T: Component> Default for Without<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Component> fmt::Debug for Without<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Without")
    }
}

impl<T: Component> QueryFilter for Without<T> {
    fn init(world: &mut World) -> (u64, u64) {
        (0, 1u64 << world.component_id::<T>())
    }
}

macro_rules! impl_filter_tuple {
    ($($name:ident),+) => {
        impl<$($name: QueryFilter),+> QueryFilter for ($($name,)+) {
            fn init(world: &mut World) -> (u64, u64) {
                let mut required = 0u64;
                let mut excluded = 0u64;
                $(
                    let (filter_required, filter_excluded) = $name::init(world);
                    required |= filter_required;
                    excluded |= filter_excluded;
                )+
                (required, excluded)
            }
        }
    };
}

impl_filter_tuple!(A, B);
impl_filter_tuple!(A, B, C);
impl_filter_tuple!(A, B, C, D);

/// Iterator over the entities matching a query.
///
/// Yields `(Entity, item)` pairs. The iterator borrows the world; mutating components
/// through it marks their change tick.
pub struct QueryIter<'w, Q: QueryData, F: QueryFilter = ()> {
    archetypes: core::slice::Iter<'w, Archetype>,
    current: Option<(&'w Archetype, Q::Fetch<'w>)>,
    row: usize,
    tick: u32,
    required: u64,
    excluded: u64,
    ids: [u32; MAX_COMPONENTS],
    component_count: usize,
    marker: PhantomData<fn() -> F>,
}

struct Compiled {
    ids: [u32; MAX_COMPONENTS],
    component_count: usize,
    required: u64,
    excluded: u64,
    tick: u32,
}

fn compile<Q: QueryData, F: QueryFilter>(world: &mut World) -> Compiled {
    let mut ids = [0u32; MAX_COMPONENTS];
    let component_count = Q::COMPONENTS;
    let required = Q::init(world, &mut ids, 0);
    let (filter_required, excluded) = F::init(world);
    assert_eq!(
        required.count_ones() as usize,
        Q::COMPONENTS,
        "query requires the same component type more than once"
    );
    assert_eq!(
        required & excluded,
        0,
        "query both requires and excludes the same component type"
    );
    Compiled {
        ids,
        component_count,
        required: required | filter_required,
        excluded,
        tick: world.tick,
    }
}

impl<'w, Q: QueryData, F: QueryFilter> QueryIter<'w, Q, F> {
    pub(crate) fn new(world: &'w mut World) -> Self {
        let compiled = compile::<Q, F>(world);
        Self {
            archetypes: world.archetypes.iter(),
            current: None,
            row: 0,
            tick: compiled.tick,
            required: compiled.required,
            excluded: compiled.excluded,
            ids: compiled.ids,
            component_count: compiled.component_count,
            marker: PhantomData,
        }
    }
}

impl<'w, Q: QueryData, F: QueryFilter> Iterator for QueryIter<'w, Q, F> {
    type Item = (Entity, Q::Item<'w>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((archetype, fetch)) = self.current.as_mut() {
                if self.row < archetype.entities.len() {
                    let row = self.row;
                    self.row += 1;
                    let entity = archetype.entities[row];
                    // SAFETY: the row is in bounds and yielded exactly once by this
                    // iterator; the fetch state was built from this same archetype.
                    let item = unsafe { Q::fetch(fetch, row) };
                    return Some((entity, item));
                }
                self.current = None;
            }
            let archetype = self.archetypes.next()?;
            if archetype.len() > 0
                && (archetype.mask & self.required) == self.required
                && (archetype.mask & self.excluded) == 0
            {
                // SAFETY: the archetype matches the compiled masks, and the world borrow
                // is exclusive for `'w`, so resolving its columns cannot alias.
                let fetch = unsafe {
                    Q::init_fetch(archetype, &self.ids[..self.component_count], self.tick)
                };
                debug_assert_eq!(Q::len(&fetch), archetype.len());
                self.current = Some((archetype, fetch));
                self.row = 0;
            }
        }
    }
}

/// Iterator over whole archetype chunks of a query.
///
/// Yields the entity slice and the resolved component slices for one archetype at a time.
/// This is the hot-path API: the inner loops are plain slice iterations, which the
/// compiler can vectorise, and systems that process many entities per frame should prefer
/// it over [`QueryIter`].
///
/// ```ignore
/// for (_entities, (positions, velocities)) in world.query_chunks::<(&mut Position, &Velocity)>() {
///     for (position, velocity) in positions.iter_mut().zip(velocities) {
///         position.x += velocity.x;
///     }
/// }
/// ```
pub struct QueryChunks<'w, Q: QueryData, F: QueryFilter = ()> {
    archetypes: core::slice::Iter<'w, Archetype>,
    required: u64,
    excluded: u64,
    ids: [u32; MAX_COMPONENTS],
    component_count: usize,
    tick: u32,
    marker: PhantomData<fn() -> (Q, F)>,
}

impl<'w, Q: QueryData, F: QueryFilter> QueryChunks<'w, Q, F> {
    pub(crate) fn new(world: &'w mut World) -> Self {
        let compiled = compile::<Q, F>(world);
        Self {
            archetypes: world.archetypes.iter(),
            required: compiled.required,
            excluded: compiled.excluded,
            ids: compiled.ids,
            component_count: compiled.component_count,
            tick: compiled.tick,
            marker: PhantomData,
        }
    }
}

impl<'w, Q: QueryData, F: QueryFilter> Iterator for QueryChunks<'w, Q, F> {
    type Item = (&'w [Entity], Q::Fetch<'w>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let archetype = self.archetypes.next()?;
            if archetype.len() == 0
                || (archetype.mask & self.required) != self.required
                || (archetype.mask & self.excluded) != 0
            {
                continue;
            }
            // SAFETY: the archetype matches the compiled masks, and the world is
            // exclusively borrowed for `'w`, so resolving its columns cannot alias.
            let fetch =
                unsafe { Q::init_fetch(archetype, &self.ids[..self.component_count], self.tick) };
            debug_assert_eq!(Q::len(&fetch), archetype.len());
            return Some((&archetype.entities, fetch));
        }
    }
}

impl<Q: QueryData, F: QueryFilter> fmt::Debug for QueryChunks<'_, Q, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueryChunks")
            .field("required", &self.required)
            .field("excluded", &self.excluded)
            .field("component_count", &self.component_count)
            .finish_non_exhaustive()
    }
}

impl<Q: QueryData, F: QueryFilter> fmt::Debug for QueryIter<'_, Q, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueryIter")
            .field("required", &self.required)
            .field("excluded", &self.excluded)
            .field("component_count", &self.component_count)
            .finish_non_exhaustive()
    }
}
