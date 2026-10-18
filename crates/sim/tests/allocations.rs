//! Allocation regression guard for the steady-state simulation tick.
//!
//! The simulation reuses its projectile/event/query scratch buffers. After warmup a tick
//! should not allocate per frame; a small growth allowance covers first-use lazy paths.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use moba_sim::{Modifier, ModifierKind, Modifiers, SimConfig, Simulation, UnitKind};

struct CountingAllocator;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
static TEST_LOCK: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

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
    let _guard = serial();
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
    ALLOCATIONS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    for _ in 0..300 {
        simulation.tick(&moba_sim::TickInputs::new());
    }
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let bytes = BYTES.load(Ordering::Relaxed);
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
    ALLOCATIONS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    for _ in 0..300 {
        simulation.tick(&moba_sim::TickInputs::new());
    }
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let bytes = BYTES.load(Ordering::Relaxed);
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
    ALLOCATIONS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    for _ in 0..300 {
        simulation.tick_with_bots();
    }
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let bytes = BYTES.load(Ordering::Relaxed);
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
    ALLOCATIONS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    for _ in 0..900 {
        simulation.tick(&moba_sim::TickInputs::new());
    }
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let bytes = BYTES.load(Ordering::Relaxed);
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
    ALLOCATIONS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    for _ in 0..300 {
        let mut inputs = moba_sim::TickInputs::new();
        inputs.set_purchase(1, 1);
        simulation.tick(&inputs);
    }
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let bytes = BYTES.load(Ordering::Relaxed);
    eprintln!("shop ticks: {allocations} allocations, {bytes} bytes over 300 ticks");
    assert!(
        allocations <= 2,
        "shop window allocated {allocations} times"
    );
    assert!(bytes <= 512, "shop window allocated {bytes} bytes");
}
