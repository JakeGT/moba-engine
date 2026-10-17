//! `SoA` archetype ECS with per-component change tracking, built for snapshot diffing.
//!
//! Design goals, in order:
//!
//! 1. **Cheap "what changed since tick N"** — every column stores a change tick per row,
//!    so a server can produce interest-managed snapshots without walking unscoped state.
//! 2. **Deterministic iteration** — entities live in archetype rows; nothing is hashed
//!    with a random seed. The same operations produce the same iteration order.
//! 3. **No allocation while iterating** — queries are a mask filter plus typed slice walks.
//!    Structural changes (spawn, insert, remove, despawn) may allocate to grow storage.
//! 4. **Predictable memory** — components are stored once, in contiguous columns. Removing
//!    a component moves it between archetypes; it is never cloned.
//!
//! Safety boundary: `moba-ecs` is one of the crates allowed to use `unsafe`. All of it is
//! confined to type-erased column access, and every block carries a `// SAFETY:` argument.
//!
//! ```ignore
//! #[derive(Clone, Copy, Debug)]
//! struct Position { x: Fix, y: Fix }
//! struct Velocity { x: Fix, y: Fix }
//!
//! let mut world = World::new();
//! let unit = world.spawn();
//! world.insert(unit, Position { x: Fix::ZERO, y: Fix::ZERO });
//! world.insert(unit, Velocity { x: Fix::ONE, y: Fix::ZERO });
//!
//! world.advance_tick();
//! for (_entity, (position, velocity)) in world.query::<(&mut Position, &Velocity)>() {
//!     position.x += velocity.x;
//! }
//! ```

mod entity;
mod query;
mod storage;
mod world;

pub use entity::Entity;
pub use query::{MutSlice, QueryChunks, QueryData, QueryFilter, QueryIter, With, Without};
pub use storage::MAX_COMPONENTS;
pub use world::World;

/// Marker trait for component types.
///
/// Components must be `Send + Sync` because parallel systems will access disjoint
/// archetypes from multiple threads. They must be `'static` so the registry can use
/// [`core::any::TypeId`].
pub trait Component: Send + Sync + 'static {}

impl<T: Send + Sync + 'static> Component for T {}
