//! Component storage: type-erased `SoA` columns grouped into archetypes.
//!
//! Each archetype owns one column per component type it contains. Columns are contiguous
//! `Vec<T>`s, so iteration is a typed slice walk and adding an entity never allocates per
//! component. Moving an entity between archetypes is a sequence of typed moves (never a
//! clone or a serialise/deserialise round-trip).
//!
//! Columns are reachable through interior mutability (`UnsafeCell`) because a query needs
//! to hand out `&mut` to disjoint components. Safety is enforced at the [`crate::World`]
//! and query layers:
//! - a world can only be queried through `&mut World`, so no query overlaps another;
//! - a query rejects duplicate component types at construction;
//! - every row is visited once per query.

use core::any::{Any, TypeId};
use core::cell::UnsafeCell;
use core::fmt;
use core::mem;
use core::ptr;

use crate::{Component, Entity};

/// Maximum number of component types in one [`crate::World`].
pub const MAX_COMPONENTS: usize = 64;

/// A type-erased column of component values plus per-row change ticks.
pub(crate) trait AnyColumn: Any + Send + Sync {
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn as_ptr(&self) -> *const u8;
    fn as_mut_ptr(&mut self) -> *mut u8;
    fn len(&self) -> usize;
    fn value_ptr(&self, row: usize) -> *const u8;
    fn changed_ptr_mut(&mut self) -> *mut u32;
    fn value_ptr_mut(&mut self, row: usize) -> *mut u8;
    fn reserve(&mut self, additional: usize);
    fn changed(&self, row: usize) -> u32;
    fn mark_changed(&mut self, row: usize, tick: u32);

    /// Moves a value in from `src`, taking ownership of it.
    ///
    /// # Safety
    /// `src` must point to a valid `T` that the caller will not access or drop again.
    unsafe fn push_from(&mut self, src: *const u8, tick: u32);

    /// Moves the value at `row` out to `dst` and shrinks the column.
    ///
    /// # Safety
    /// `dst` must point to uninitialised storage suitable for one `T`.
    unsafe fn swap_remove_to(&mut self, row: usize, dst: *mut u8);

    /// Removes `row` by swapping in the last value, without dropping the removed value.
    fn swap_remove_forget(&mut self, row: usize);

    /// Whether dropping values of this component type can run user code.
    fn needs_drop(&self) -> bool;

    /// Removes `row` by swapping in the last value and boxing the removed value.
    ///
    /// Only called when [`AnyColumn::needs_drop`] is true. Boxing lets the world finish
    /// structural bookkeeping before invoking user `Drop` code, so a panicking component
    /// cannot leave rows mismatched.
    fn swap_remove_boxed(&mut self, row: usize) -> Box<dyn Any + Send + Sync>;
}

/// The concrete column implementation for component type `T`.
pub(crate) struct VecColumn<T> {
    pub(crate) data: Vec<T>,
    pub(crate) changed: Vec<u32>,
}

impl<T> VecColumn<T> {
    pub(crate) const fn new() -> Self {
        Self {
            data: Vec::new(),
            changed: Vec::new(),
        }
    }
}

impl<T: Component> AnyColumn for VecColumn<T> {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_ptr(&self) -> *const u8 {
        self.data.as_ptr().cast()
    }

    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.data.as_mut_ptr().cast()
    }

    fn len(&self) -> usize {
        self.data.len()
    }

    fn changed_ptr_mut(&mut self) -> *mut u32 {
        self.changed.as_mut_ptr()
    }

    fn value_ptr(&self, row: usize) -> *const u8 {
        assert!(row < self.data.len(), "row out of bounds");
        // SAFETY: `row` is in bounds (asserted and upheld by all callers), so the
        // pointer stays inside the allocation. It is returned, not dereferenced here.
        unsafe { self.data.as_ptr().add(row).cast() }
    }

    fn value_ptr_mut(&mut self, row: usize) -> *mut u8 {
        assert!(row < self.data.len(), "row out of bounds");
        // SAFETY: `row` is in bounds, so the pointer stays inside the allocation.
        unsafe { self.data.as_mut_ptr().add(row).cast() }
    }

    fn reserve(&mut self, additional: usize) {
        self.data.reserve(additional);
        self.changed.reserve(additional);
    }

    fn changed(&self, row: usize) -> u32 {
        self.changed[row]
    }

    fn mark_changed(&mut self, row: usize, tick: u32) {
        self.changed[row] = tick;
    }

    unsafe fn push_from(&mut self, src: *const u8, tick: u32) {
        // SAFETY: the caller guarantees `src` points to a valid, owned `T`; ownership
        // moves into the column and the caller forgets its copy.
        let value = unsafe { ptr::read(src.cast::<T>()) };
        self.data.push(value);
        self.changed.push(tick);
    }

    unsafe fn swap_remove_to(&mut self, row: usize, dst: *mut u8) {
        let value = self.data.swap_remove(row);
        // SAFETY: the caller guarantees `dst` is uninitialised storage for one `T`.
        unsafe { ptr::write(dst.cast::<T>(), value) };
        self.changed.swap_remove(row);
    }

    fn swap_remove_forget(&mut self, row: usize) {
        let value = self.data.swap_remove(row);
        // The value was already moved out by `push_from`; dropping it here would be a
        // double free. Ownership now lives in the destination column.
        mem::forget(value);
        self.changed.swap_remove(row);
    }

    fn needs_drop(&self) -> bool {
        mem::needs_drop::<T>()
    }

    fn swap_remove_boxed(&mut self, row: usize) -> Box<dyn Any + Send + Sync> {
        let value = self.data.swap_remove(row);
        self.changed.swap_remove(row);
        Box::new(value)
    }
}

/// Metadata and a column factory for one registered component type.
struct ComponentInfo {
    type_id: TypeId,
    create: fn() -> Box<dyn AnyColumn>,
}

/// Per-world component type registry.
///
/// Registration order defines component ids, and ids are dense. The lookup is a linear
/// scan: worlds contain tens of component types at most, and registration happens during
/// setup, not in the tick loop.
pub(crate) struct Registry {
    infos: Vec<ComponentInfo>,
}

impl Registry {
    pub(crate) const fn new() -> Self {
        Self { infos: Vec::new() }
    }

    /// Returns the id for `T`, registering it on first use.
    ///
    /// # Panics
    /// Panics if the world already has [`MAX_COMPONENTS`] component types.
    pub(crate) fn register<T: Component>(&mut self) -> u32 {
        if let Some(id) = self.id_of::<T>() {
            return id;
        }
        assert!(
            self.infos.len() < MAX_COMPONENTS,
            "a World supports at most {MAX_COMPONENTS} component types"
        );
        let id = u32::try_from(self.infos.len()).expect("component count fits u32");
        self.infos.push(ComponentInfo {
            type_id: TypeId::of::<T>(),
            create: create_column::<T>,
        });
        id
    }

    /// Returns the id for `T` if it has been registered, without registering it.
    pub(crate) fn id_of<T: Component>(&self) -> Option<u32> {
        let type_id = TypeId::of::<T>();
        self.infos
            .iter()
            .position(|info| info.type_id == type_id)
            .map(|index| u32::try_from(index).expect("component count fits u32"))
    }

    pub(crate) fn create(&self, id: u32) -> Box<dyn AnyColumn> {
        (self.infos[id as usize].create)()
    }

    pub(crate) fn len(&self) -> usize {
        self.infos.len()
    }
}

fn create_column<T: Component>() -> Box<dyn AnyColumn> {
    Box::new(VecColumn::<T>::new())
}

/// A group of entities that all have exactly the same set of components.
///
/// This type is exposed only because query implementations name it in their signatures;
/// it is not part of the supported API surface.
#[doc(hidden)]
pub struct Archetype {
    pub(crate) mask: u64,
    pub(crate) entities: Vec<Entity>,
    component_ids: Vec<u32>,
    columns: Vec<UnsafeCell<Box<dyn AnyColumn>>>,
}

impl Archetype {
    pub(crate) fn new(mask: u64, registry: &Registry) -> Self {
        let component_ids: Vec<u32> = {
            let mut bits = mask;
            let mut ids = Vec::with_capacity(mask.count_ones() as usize);
            while bits != 0 {
                let id = bits.trailing_zeros();
                bits &= bits - 1;
                ids.push(id);
            }
            ids
        };
        let columns = component_ids
            .iter()
            .map(|&id| UnsafeCell::new(registry.create(id)))
            .collect();
        Self {
            mask,
            entities: Vec::new(),
            component_ids,
            columns,
        }
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.entities.len()
    }

    pub(crate) fn reserve(&mut self, additional: usize) {
        self.entities.reserve(additional);
        for cell in &mut self.columns {
            cell.get_mut().reserve(additional);
        }
    }

    #[inline]
    pub(crate) fn component_ids(&self) -> &[u32] {
        &self.component_ids
    }

    #[inline]
    fn column_index(&self, id: u32) -> Option<usize> {
        self.component_ids.binary_search(&id).ok()
    }

    #[inline]
    pub(crate) fn column(&self, id: u32) -> Option<&dyn AnyColumn> {
        let index = self.column_index(id)?;
        let cell = self.columns.get(index)?;
        // SAFETY: `columns` and `component_ids` are never resized after construction, so
        // the cell lives here. Only a shared reference is produced; mutation goes through
        // `column_mut`, which the world serialises.
        let column: &dyn AnyColumn = unsafe { &**cell.get() };
        Some(column)
    }

    #[allow(clippy::mut_from_ref)] // Justified by the `&mut World` requirement documented above.
    pub(crate) fn column_mut(&self, id: u32) -> Option<&mut dyn AnyColumn> {
        let index = self.column_index(id)?;
        let cell = self.columns.get(index)?;
        // SAFETY: the world guarantees exclusive access to this archetype while a query
        // or `get_mut` is live, and queries reject duplicate component ids, so no two
        // mutable references to the same column can coexist.
        let column: &mut dyn AnyColumn = unsafe { &mut **cell.get() };
        Some(column)
    }

    pub(crate) fn changed(&self, id: u32, row: usize) -> Option<u32> {
        Some(self.column(id)?.changed(row))
    }

    pub(crate) fn get<T: Component>(&self, id: u32, row: usize) -> Option<&T> {
        let column = self.column(id)?;
        if row >= column.len() {
            return None;
        }
        let ptr = column.value_ptr(row).cast::<T>();
        // SAFETY: the column registered for `id` stores `T`, and the caller supplies an
        // in-bounds row. The shared reference cannot alias a mutable one because `World`
        // requires `&mut self` to create any mutation.
        Some(unsafe { &*ptr })
    }

    #[allow(clippy::mut_from_ref)] // Same invariant as `column_mut`.
    pub(crate) fn get_mut<T: Component>(&self, id: u32, row: usize, tick: u32) -> Option<&mut T> {
        let column = self.column_mut(id)?;
        if row >= column.len() {
            return None;
        }
        column.mark_changed(row, tick);
        let ptr = column.value_ptr_mut(row).cast::<T>();
        // SAFETY: same type/in-bounds argument as `get`; `World` exclusivity means no other
        // reference to this value is live.
        Some(unsafe { &mut *ptr })
    }
}

impl fmt::Debug for Archetype {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Archetype")
            .field("mask", &self.mask)
            .field("component_ids", &self.component_ids)
            .field("entities", &self.entities.len())
            .finish_non_exhaustive()
    }
}
