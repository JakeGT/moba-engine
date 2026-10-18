#![allow(missing_docs)]
//! Tick-cost benchmarks for a full 5v5 match with minion waves and bots.

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use moba_sim::{SimConfig, Simulation};
use std::hint::black_box;

const TICKS_PER_BATCH: u32 = 300;

fn full_match_tick(c: &mut Criterion) {
    let config = SimConfig::default();
    c.bench_function("sim/tick_full_5v5_300_ticks", |b| {
        b.iter_batched(
            || Simulation::new(config),
            |mut simulation| {
                for _ in 0..TICKS_PER_BATCH {
                    simulation.tick_with_bots();
                }
                black_box(simulation.state_hash());
            },
            BatchSize::LargeInput,
        );
    });
}

fn steady_state_tick(c: &mut Criterion) {
    let config = SimConfig::default();
    c.bench_function("sim/tick_steady_5v5_300_ticks", |b| {
        b.iter_batched(
            || {
                let mut simulation = Simulation::new(config);
                // Warm up past three waves (90 seconds) so minions, fights and towers are
                // all active while the measured window runs.
                for _ in 0..2_700 {
                    simulation.tick_with_bots();
                }
                simulation
            },
            |mut simulation| {
                for _ in 0..TICKS_PER_BATCH {
                    simulation.tick_with_bots();
                }
                black_box(simulation.state_hash());
            },
            BatchSize::LargeInput,
        );
    });
}

fn observe_views(c: &mut Criterion) {
    let mut simulation = Simulation::new(SimConfig::default());
    for _ in 0..2_700 {
        simulation.tick_with_bots();
    }
    let mut outputs = [Vec::new(), Vec::new(), Vec::new()];
    c.bench_function("sim/observe_3_views", |b| {
        b.iter(|| {
            simulation.observed_units(moba_game::Team::Blue, &mut outputs[0]);
            simulation.observed_units(moba_game::Team::Red, &mut outputs[1]);
            simulation.observed_all(&mut outputs[2]);
            black_box((outputs[0].len(), outputs[1].len(), outputs[2].len()));
        });
    });
}

fn no_wave_tick(c: &mut Criterion) {
    let config = SimConfig {
        bot_mask: 0,
        spawn_waves: false,
        ..SimConfig::default()
    };
    c.bench_function("sim/tick_idle_300_ticks", |b| {
        b.iter_batched(
            || Simulation::new(config),
            |mut simulation| {
                for _ in 0..TICKS_PER_BATCH {
                    simulation.tick(&moba_sim::TickInputs::new());
                }
                black_box(simulation.state_hash());
            },
            BatchSize::LargeInput,
        );
    });
}

criterion_group!(
    benches,
    full_match_tick,
    steady_state_tick,
    no_wave_tick,
    observe_views
);
criterion_main!(benches);
