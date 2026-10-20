//! Test-only allocation accounting.
#![allow(unsafe_code)]
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}
fn count(bytes: usize) {
    let _ = ALLOCATIONS.try_with(|c| c.set(c.get().saturating_add(1)));
    let _ = BYTES.try_with(|c| c.set(c.get().saturating_add(bytes)));
}
/// Thread-local counting allocator.
#[derive(Debug)]
pub struct CountingAllocator;
/// Resets calling-thread counters.
pub fn reset() {
    let _ = ALLOCATIONS.try_with(|c| c.set(0));
    let _ = BYTES.try_with(|c| c.set(0));
}
/// Returns `(allocations, bytes)`.
#[must_use]
pub fn counters() -> (usize, usize) {
    (ALLOCATIONS.with(Cell::get), BYTES.with(Cell::get))
}
// SAFETY: delegates to the system allocator and updates thread-local counters.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        // SAFETY: forwards the caller's valid layout to the system allocator.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: forwards the caller's valid pointer/layout pair.
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count(new_size.saturating_sub(layout.size()));
        // SAFETY: forwards the caller's valid pointer/layout/size to the system allocator.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}
