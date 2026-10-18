#![allow(missing_docs)]
//! Snapshot build/encode/decode benchmarks for a representative 5v5 view.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use moba_net::protocol::{
    NetEntity, NetProgress, SnapshotDelta, build_delta, build_delta_into, decode_snapshot,
    encode_snapshot,
};
use moba_sim::{SimConfig, Simulation};

fn entity(id: u16, x: u16, y: u16) -> NetEntity {
    NetEntity {
        net_id: id,
        kind: if id < 10 {
            0
        } else if id < 16 {
            3
        } else {
            1
        },
        team: u8::from(id % 2 != 0),
        hero_slot: if id < 10 { id as u8 } else { 0xFF },
        hero_pick: 0,
        flags: 0,
        status_bits: 0,
        max_health: 600,
        max_mana: 300,
        x,
        y,
        health: 500,
        mana: 200,
        cooldowns: [0; 3],
        progress: NetProgress::default(),
    }
}

fn view() -> Vec<NetEntity> {
    let mut entities = Vec::with_capacity(48);
    for index in 0..40u16 {
        entities.push(entity(index + 1, index * 3, 64 + index * 2));
    }
    entities.sort_unstable_by_key(|entity| entity.net_id);
    entities
}

fn changed_view(baseline: &[NetEntity]) -> Vec<NetEntity> {
    let mut changed = baseline.to_vec();
    for (index, entity) in changed.iter_mut().enumerate() {
        if index % 3 == 0 {
            entity.x = entity.x.wrapping_add(1);
            entity.health = entity.health.saturating_sub(1);
        }
        if index == 0 {
            entity.progress.gold = 500;
            entity.progress.items[0] = 1;
        }
    }
    changed
}

fn full_snapshot(c: &mut Criterion) {
    let current = view();
    let delta = build_delta(&current, None, 1_000, 0, 0);
    let mut encoded = Vec::new();
    encode_snapshot(&delta, &mut encoded).expect("encode");
    c.bench_function("net/snapshot_build_full_40_entities", |b| {
        b.iter(|| black_box(build_delta(&current, None, 1_000, 0, 0)));
    });
    c.bench_function("net/snapshot_encode_full_40_entities", |b| {
        let mut out = Vec::with_capacity(encoded.len());
        b.iter(|| {
            encode_snapshot(black_box(&delta), &mut out).expect("encode");
            black_box(out.len())
        });
    });
    c.bench_function("net/snapshot_decode_full_40_entities", |b| {
        b.iter(|| black_box(decode_snapshot(black_box(&encoded)).expect("decode")));
    });
}

fn delta_snapshot(c: &mut Criterion) {
    let baseline = view();
    let current = changed_view(&baseline);
    let delta = build_delta(&current, Some(&baseline), 1_001, 42, 1);
    assert_eq!(delta.updates.len(), 14);
    let mut encoded = Vec::new();
    encode_snapshot(&delta, &mut encoded).expect("encode");
    c.bench_function("net/snapshot_build_delta_40_entities", |b| {
        b.iter(|| black_box(build_delta(&current, Some(&baseline), 1_001, 42, 1)));
    });
    c.bench_function("net/snapshot_encode_delta_40_entities", |b| {
        let mut out = Vec::with_capacity(encoded.len());
        b.iter(|| {
            encode_snapshot(black_box(&delta), &mut out).expect("encode");
            black_box(out.len())
        });
    });
    c.bench_function("net/snapshot_decode_delta_40_entities", |b| {
        b.iter(|| black_box(decode_snapshot(black_box(&encoded)).expect("decode")));
    });
}

fn aged_simulation(minutes: u32) -> (Simulation, Vec<moba_sim::ObservedUnit>) {
    let mut simulation = Simulation::new(SimConfig::default());
    let ticks = minutes * 60 * 30;
    for _ in 0..ticks {
        simulation.tick_with_bots();
        // Keep structures alive so the simulated match does not end during the warmup.
        let entities: Vec<_> = simulation.world().entities().collect();
        for entity in entities {
            if simulation
                .world()
                .get::<moba_sim::UnitKind>(entity)
                .is_some_and(|kind| kind.is_structure())
                && let Some(health) = simulation.world_mut().get_mut::<moba_sim::Health>(entity)
            {
                health.current = health.max;
            }
        }
    }
    let mut observed = Vec::new();
    simulation.observed_all(&mut observed);
    (simulation, observed)
}

fn snapshot_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("net/snapshot_scaling");
    for minutes in [1, 20, 40] {
        let (_simulation, observed) = aged_simulation(minutes);
        let current = observed_entities(&observed);
        eprintln!("{minutes}min snapshot entities: {}", current.len());
        let baseline = current.clone();
        group.bench_function(format!("{minutes}min"), |b| {
            let mut delta = SnapshotDelta::default();
            let mut out = Vec::with_capacity(64 * 1024);
            b.iter(|| {
                build_delta_into(black_box(&current), Some(&baseline), 1, 0, 1, &mut delta);
                encode_snapshot(&delta, &mut out).expect("encode");
                black_box(out.len())
            });
        });
        black_box(current);
    }
    group.finish();
}

fn observed_entities(observed: &[moba_sim::ObservedUnit]) -> Vec<NetEntity> {
    let mut entities: Vec<_> = observed
        .iter()
        .enumerate()
        .map(|(index, unit)| NetEntity::from_observed(u16::try_from(index + 1).unwrap_or(1), unit))
        .collect();
    entities.sort_unstable_by_key(|entity| entity.net_id);
    entities
}

fn replay_apply(c: &mut Criterion) {
    let baseline = view();
    let current = changed_view(&baseline);
    let delta: SnapshotDelta = build_delta(&current, Some(&baseline), 1_001, 42, 1);
    let state =
        moba_net::protocol::SnapshotState::from_full(1, &build_delta(&baseline, None, 1_000, 0, 0));
    c.bench_function("net/snapshot_apply_delta_40_entities", |b| {
        b.iter(|| black_box(state.apply(black_box(&delta), 2).expect("apply")));
    });
}

criterion_group!(
    benches,
    full_snapshot,
    delta_snapshot,
    replay_apply,
    snapshot_scaling
);
criterion_main!(benches);
