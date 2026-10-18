#![allow(unexpected_cfgs)]

//! A scoped work-stealing scheduler for the per-frame job graph.
//!
//! Design:
//! - A fixed pool of worker threads (created once, never per frame).
//! - Each worker owns a local deque; idle workers steal from other workers and from a
//!   global injector. The thread that calls [`JobSystem::scope`] also executes jobs while
//!   it waits, so a single-core machine or a nested scope never deadlocks.
//! - `scope` accepts closures that borrow caller data for the duration of the scope. All
//!   spawned jobs are guaranteed to finish before `scope` returns, which is the whole
//!   safety argument for erasing their lifetimes (see `JobRef` in the private pool module).
//! - Panics in jobs are captured and resumed on the calling thread after every job has
//!   finished. A panicking system cannot leave a half-torn-down scope behind.
//!
//! The scheduler is intentionally conservative: jobs should be coarse (chunks of entities,
//! one system phase), because scheduling overhead dominates below a few microseconds of
//! work.

mod pool;

#[cfg(loom)]
mod loom_tests;

pub use pool::{JobSystem, Scope};
