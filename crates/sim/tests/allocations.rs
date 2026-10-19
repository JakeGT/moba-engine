//! Allocation regression guard for the steady-state simulation tick.
//!
//! The simulation reuses its projectile/event/query scratch buffers. After warmup a tick
//! should not allocate per frame; a small growth allowance covers first-use lazy paths.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Mutex;

use moba_sim::{Modifier, ModifierKind, Modifiers, SimConfig, Simulation, UnitKind};

struct CountingAllocator;

// Per-thread counters: parallel integration tests, test harness threads and the job pool
// cannot pollute the measured window. `try_with` is used because the allocator may run after
// a thread's TLS has been destroyed.
thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}
static TEST_LOCK: Mutex<()> = Mutex::new(());

fn count(allocation: bool, bytes: usize) {
    let _ = ALLOCATIONS.try_with(|counter| {
        if allocation {
            counter.set(counter.get().saturating_add(1));
        }
    });
    let _ = BYTES.try_with(|counter| counter.set(counter.get().saturating_add(bytes)));
}

fn reset_counters() {
    let _ = ALLOCATIONS.try_with(|counter| counter.set(0));
    let _ = BYTES.try_with(|counter| counter.set(0));
}

fn counters() -> (usize, usize) {
    (ALLOCATIONS.with(Cell::get), BYTES.with(Cell::get))
}

fn serial() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

// SAFETY: delegates every operation to the system allocator and only wraps it with
// thread-local counters that do not affect pointer validity or alignment.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(true, layout.size());
        // SAFETY: layout came from the caller of the global allocator and is forwarded as-is.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: pointer/layout are the pair supplied by the allocator caller.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count(true, new_size.saturating_sub(layout.size()));
        // SAFETY: pointer/layout are the pair supplied by the allocator caller.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn steady_state_ticks_do_not_allocate_per_frame() {
    let _guard = serial();
    let mut simulation = Simulation::new(SimConfig::default());
    for _ in 0..3_000 {
        simulation.tick_with_bots();
    }
    reset_counters();
    for _ in 0..300 {
        simulation.tick_with_bots();
    }
    let (allocations, bytes) = counters();
    eprintln!("steady ticks: {allocations} allocations, {bytes} bytes over 300 ticks");
    assert!(
        allocations <= 2,
        "steady-state tick allocated {allocations} times"
    );
    assert!(bytes <= 512, "steady-state tick allocated {bytes} bytes");
}

#[test]
fn status_modifier_ticks_stay_within_budget() {
    let _guard = serial();
    let mut simulation = Simulation::new(SimConfig {
        bot_mask: 0,
        spawn_waves: false,
        ..SimConfig::default()
    });
    for _ in 0..30 {
        simulation.tick(&moba_sim::TickInputs::new());
    }
    let hero = simulation
        .world_mut()
        .query::<&UnitKind>() // any query establishes the borrow pattern
        .find(|(_, kind)| **kind == UnitKind::Hero)
        .map(|(entity, _)| entity)
        .expect("hero");
    let modifiers = simulation
        .world_mut()
        .get_mut::<Modifiers>(hero)
        .expect("always-present modifiers");
    modifiers.clear();
    let source = hero;
    for kind in [
        ModifierKind::Shield,
        ModifierKind::Slow,
        ModifierKind::Haste,
        ModifierKind::Root,
        ModifierKind::Silence,
        ModifierKind::Burn,
    ] {
        assert!(modifiers.push(Modifier {
            def_id: kind as u16,
            kind,
            remaining: u32::MAX,
            factor: moba_core::Fix::ONE,
            amount: if kind == ModifierKind::Shield {
                10_000
            } else {
                1
            },
            damage_type: moba_game::DamageType::Physical,
            source,
        }));
    }

    for _ in 0..300 {
        simulation.tick(&moba_sim::TickInputs::new());
    }
    reset_counters();
    for _ in 0..300 {
        simulation.tick(&moba_sim::TickInputs::new());
    }
    let (allocations, bytes) = counters();
    eprintln!("modifier ticks: {allocations} allocations, {bytes} bytes over 300 ticks");
    assert!(
        allocations <= 2,
        "modifier tick allocated {allocations} times"
    );
    assert!(bytes <= 512, "modifier tick allocated {bytes} bytes");
}

#[test]
fn wave_spawn_ticks_stay_within_budget() {
    let _guard = serial();
    let mut simulation = Simulation::new(SimConfig {
        bot_mask: 0,
        spawn_waves: true,
        ..SimConfig::default()
    });
    // Default first wave is at 2730 ticks; measure a window that crosses it.
    for _ in 0..2_700 {
        simulation.tick(&moba_sim::TickInputs::new());
    }
    reset_counters();
    for _ in 0..300 {
        simulation.tick(&moba_sim::TickInputs::new());
    }
    let (allocations, bytes) = counters();
    eprintln!("wave ticks: {allocations} allocations, {bytes} bytes over 300 ticks");
    assert!(
        allocations <= 8,
        "wave window allocated {allocations} times"
    );
    assert!(bytes <= 1_024, "wave window allocated {bytes} bytes");
}

#[test]
fn teamfight_ticks_stay_within_budget() {
    let _guard = serial();
    let mut simulation = Simulation::new(SimConfig::default());
    for _ in 0..3_000 {
        simulation.tick_with_bots();
    }
    reset_counters();
    for _ in 0..300 {
        simulation.tick_with_bots();
    }
    let (allocations, bytes) = counters();
    eprintln!("teamfight ticks: {allocations} allocations, {bytes} bytes over 300 ticks");
    assert!(
        allocations <= 2,
        "teamfight window allocated {allocations} times"
    );
    assert!(bytes <= 512, "teamfight window allocated {bytes} bytes");
}

#[test]
fn respawn_ticks_stay_within_budget() {
    let _guard = serial();
    let mut simulation = Simulation::new(SimConfig {
        bot_mask: 0,
        spawn_waves: false,
        ..SimConfig::default()
    });
    for _ in 0..30 {
        simulation.tick(&moba_sim::TickInputs::new());
    }
    let hero = simulation
        .world_mut()
        .query::<&UnitKind>()
        .find(|(_, kind)| **kind == UnitKind::Hero)
        .map(|(entity, _)| entity)
        .expect("hero");
    simulation
        .world_mut()
        .get_mut::<moba_sim::Health>(hero)
        .expect("health")
        .current = 0;
    for _ in 0..10 {
        simulation.tick(&moba_sim::TickInputs::new());
    }
    reset_counters();
    for _ in 0..900 {
        simulation.tick(&moba_sim::TickInputs::new());
    }
    let (allocations, bytes) = counters();
    eprintln!("respawn ticks: {allocations} allocations, {bytes} bytes over 900 ticks");
    assert!(
        allocations <= 2,
        "respawn window allocated {allocations} times"
    );
    assert!(bytes <= 512, "respawn window allocated {bytes} bytes");
}

#[test]
fn shop_ticks_stay_within_budget() {
    let _guard = serial();
    let mut simulation = Simulation::new(SimConfig {
        bot_mask: 0,
        spawn_waves: false,
        ..SimConfig::default()
    });
    for _ in 0..30 {
        simulation.tick(&moba_sim::TickInputs::new());
    }
    for _ in 0..10 {
        let mut inputs = moba_sim::TickInputs::new();
        inputs.set_purchase(0, 0);
        simulation.tick(&inputs);
    }
    reset_counters();
    for _ in 0..300 {
        let mut inputs = moba_sim::TickInputs::new();
        inputs.set_purchase(1, 1);
        simulation.tick(&inputs);
    }
    let (allocations, bytes) = counters();
    eprintln!("shop ticks: {allocations} allocations, {bytes} bytes over 300 ticks");
    assert!(
        allocations <= 2,
        "shop window allocated {allocations} times"
    );
    assert!(bytes <= 512, "shop window allocated {bytes} bytes");
}
