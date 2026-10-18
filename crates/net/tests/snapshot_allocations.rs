//! Allocation guard for steady-state snapshot build/encode.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use moba_net::protocol::{
    NetEntity, NetProgress, SnapshotDelta, build_delta_into, encode_snapshot,
};

struct CountingAllocator;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards every operation to the system allocator unchanged and only wraps it with
// atomic counters that do not affect pointer validity, alignment or lifetime.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        // SAFETY: layout came from the global allocator caller and is forwarded as-is.
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

fn entity(id: u16) -> NetEntity {
    NetEntity {
        net_id: id,
        kind: u8::from(id >= 10),
        team: u8::from(id % 2 != 0),
        hero_slot: if id < 10 { id as u8 } else { 0xFF },
        hero_pick: 0,
        flags: 0,
        status_bits: 0,
        max_health: 600,
        max_mana: 300,
        x: id * 3,
        y: 64 + id * 2,
        health: 500,
        mana: 200,
        cooldowns: [0; 3],
        progress: NetProgress::default(),
    }
}

#[test]
fn steady_state_snapshot_build_and_encode_allocate_nothing() {
    let baseline: Vec<_> = (1..=100).map(entity).collect();
    let mut current = baseline.clone();
    for entity in &mut current {
        entity.x = entity.x.wrapping_add(1);
        entity.health = entity.health.saturating_sub(1);
    }
    let mut delta = SnapshotDelta::default();
    let mut payload = Vec::with_capacity(16 * 1024);
    // Warm capacities in the delta and encode buffers.
    build_delta_into(&current, Some(&baseline), 1, 0, 1, &mut delta);
    encode_snapshot(&delta, &mut payload).expect("encode");

    ALLOCATIONS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    for tick in 0..300 {
        build_delta_into(&current, Some(&baseline), tick, 0, 1, &mut delta);
        encode_snapshot(&delta, &mut payload).expect("encode");
    }
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let bytes = BYTES.load(Ordering::Relaxed);
    eprintln!("snapshot path: {allocations} allocations, {bytes} bytes over 300 builds");
    assert_eq!(allocations, 0);
    assert_eq!(bytes, 0);
}
