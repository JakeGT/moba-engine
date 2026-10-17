//! Allocation regression guard for the steady-state simulation tick.
//!
//! The simulation reuses its projectile/event/query scratch buffers. After warmup a tick
//! should not allocate per frame; a small growth allowance covers first-use lazy paths.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use moba_sim::{SimConfig, Simulation};

struct CountingAllocator;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

// SAFETY: delegates every operation to the system allocator and only wraps it with atomic
// counters that do not affect pointer validity or alignment.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        // SAFETY: layout came from the caller of the global allocator and is forwarded as-is.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: pointer/layout are the pair supplied by the allocator caller.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(new_size.saturating_sub(layout.size()), Ordering::Relaxed);
        // SAFETY: pointer/layout are the pair supplied by the allocator caller.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn steady_state_ticks_do_not_allocate_per_frame() {
    let mut simulation = Simulation::new(SimConfig::default());
    for _ in 0..3_000 {
        simulation.tick_with_bots();
    }
    ALLOCATIONS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    for _ in 0..300 {
        simulation.tick_with_bots();
    }
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let bytes = BYTES.load(Ordering::Relaxed);
    eprintln!("steady ticks: {allocations} allocations, {bytes} bytes over 300 ticks");
    assert!(
        allocations <= 30,
        "steady-state tick allocated {allocations} times over 300 ticks"
    );
    assert!(
        bytes <= 4_096,
        "steady-state tick allocated {bytes} bytes over 300 ticks"
    );
}
