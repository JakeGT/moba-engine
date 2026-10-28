//! Allocation guard for steady-state snapshot build/encode.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use moba_net::protocol::{
    EVENT_ATTACK, EVENT_CAST, MAX_SNAPSHOT_EVENTS, NetEntity, NetEvent, NetProgress, SnapshotDelta,
    build_delta_into, encode_snapshot,
};

struct CountingAllocator;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}

fn count(bytes: usize) {
    let _ = ALLOCATIONS.try_with(|counter| counter.set(counter.get().saturating_add(1)));
    let _ = BYTES.try_with(|counter| counter.set(counter.get().saturating_add(bytes)));
}

fn reset_counters() {
    let _ = ALLOCATIONS.try_with(|counter| counter.set(0));
    let _ = BYTES.try_with(|counter| counter.set(0));
}

fn counters() -> (usize, usize) {
    (ALLOCATIONS.with(Cell::get), BYTES.with(Cell::get))
}

// SAFETY: forwards every operation to the system allocator unchanged and only wraps it with
// thread-local counters that do not affect pointer validity, alignment or lifetime.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        // SAFETY: layout came from the global allocator caller and is forwarded as-is.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: pointer/layout are the pair supplied by the allocator caller.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count(new_size.saturating_sub(layout.size()));
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
        team: u8::from(!id.is_multiple_of(2)),
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
    // Warm capacities in the delta and encode buffers, including the first tick value.
    build_delta_into(&current, Some(&baseline), 0, 0, 1, &mut delta);
    encode_snapshot(&delta, &mut payload).expect("encode");

    reset_counters();
    for tick in 1..=300 {
        build_delta_into(&current, Some(&baseline), tick, 0, 1, &mut delta);
        encode_snapshot(&delta, &mut payload).expect("encode");
    }
    let (allocations, bytes) = counters();
    eprintln!("snapshot path: {allocations} allocations, {bytes} bytes over 300 builds");
    assert_eq!(allocations, 0);
    assert_eq!(bytes, 0);
}

#[test]
fn event_overhead_stays_within_snapshot_budget() {
    let baseline: Vec<_> = (1..=100).map(entity).collect();
    let current = baseline.clone();
    let mut delta = SnapshotDelta::default();
    let mut without = Vec::with_capacity(16 * 1024);
    build_delta_into(&current, Some(&baseline), 1, 0, 1, &mut delta);
    encode_snapshot(&delta, &mut without).expect("encode");
    delta.events = vec![
        NetEvent {
            kind: EVENT_CAST,
            ..NetEvent::default()
        };
        MAX_SNAPSHOT_EVENTS
    ];
    let mut with = Vec::with_capacity(16 * 1024);
    encode_snapshot(&delta, &mut with).expect("encode");
    let overhead = with.len().saturating_sub(without.len());
    assert!(
        overhead <= MAX_SNAPSHOT_EVENTS * 28 + 8,
        "event overhead {overhead} bytes exceeds 16-event budget"
    );
}

#[test]
fn encoding_a_full_event_list_with_attacks_allocates_nothing() {
    let baseline: Vec<_> = (1..=100).map(entity).collect();
    let current = baseline.clone();
    let mut delta = SnapshotDelta::default();
    let mut payload = Vec::with_capacity(16 * 1024);
    build_delta_into(&current, Some(&baseline), 1, 0, 1, &mut delta);
    // A teamfight snapshot: the 16-event cap full, mostly compact attacks.
    delta.events = (0..MAX_SNAPSHOT_EVENTS)
        .map(|index| NetEvent {
            kind: if index % 4 == 0 {
                EVENT_CAST
            } else {
                EVENT_ATTACK
            },
            source: index as u16,
            target: 100 - index as u16,
            value: 30,
            aux: 1,
            ..NetEvent::default()
        })
        .collect();
    encode_snapshot(&delta, &mut payload).expect("encode");
    reset_counters();
    for tick in 2..=301 {
        delta.tick = tick;
        encode_snapshot(&delta, &mut payload).expect("encode");
    }
    let (allocations, bytes) = counters();
    assert_eq!((allocations, bytes), (0, 0));
    // 4 full events and 12 compact attacks.
    let mut without = Vec::with_capacity(16 * 1024);
    let events = core::mem::take(&mut delta.events);
    encode_snapshot(&delta, &mut without).expect("encode");
    delta.events = events;
    encode_snapshot(&delta, &mut payload).expect("encode");
    assert_eq!(payload.len() - without.len(), 4 * 26 + 12 * 12);
}
