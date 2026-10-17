#![allow(missing_docs)]
//! ECS iteration throughput versus a naive array-of-structs baseline.
//!
//! The M1 exit criterion is that the archetype iteration beats `AoS` for the common
//! "update position from velocity" system. Setup is excluded from the measurement.

use criterion::{Criterion, criterion_group, criterion_main};
use moba_ecs::World;
use std::hint::black_box;

const ENTITIES: usize = 100_000;

#[derive(Clone, Copy, Debug)]
struct Position {
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Debug)]
struct Velocity {
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Debug)]
struct AosUnit {
    position: Position,
    velocity: Velocity,
}

fn aos_baseline(c: &mut Criterion) {
    let mut units: Vec<AosUnit> = (0..ENTITIES)
        .map(|index| AosUnit {
            position: Position {
                x: index as f32,
                y: 0.0,
            },
            velocity: Velocity { x: 1.5, y: 0.5 },
        })
        .collect();

    c.bench_function("ecs/iter_aos", |b| {
        b.iter(|| {
            for unit in &mut units {
                unit.position.x += unit.velocity.x;
                unit.position.y += unit.velocity.y;
            }
            black_box(&units);
        });
    });
}

fn ecs_two_components(c: &mut Criterion) {
    let mut world = World::new();
    for index in 0..ENTITIES {
        let entity = world.spawn();
        world.insert(
            entity,
            Position {
                x: index as f32,
                y: 0.0,
            },
        );
        world.insert(entity, Velocity { x: 1.5, y: 0.5 });
    }

    c.bench_function("ecs/iter_ecs_2c", |b| {
        b.iter(|| {
            for (_, (position, velocity)) in world.query::<(&mut Position, &Velocity)>() {
                position.x += velocity.x;
                position.y += velocity.y;
            }
            black_box(&world);
        });
    });
}

fn ecs_one_component(c: &mut Criterion) {
    let mut world = World::new();
    for index in 0..ENTITIES {
        let entity = world.spawn();
        world.insert(
            entity,
            Position {
                x: index as f32,
                y: 0.0,
            },
        );
    }

    c.bench_function("ecs/iter_ecs_1c", |b| {
        b.iter(|| {
            let mut sum = 0.0f32;
            for (_, position) in world.query::<&Position>() {
                sum += position.x;
            }
            black_box(sum);
        });
    });
}

fn ecs_two_components_chunks(c: &mut Criterion) {
    let mut world = World::new();
    for index in 0..ENTITIES {
        let entity = world.spawn();
        world.insert(
            entity,
            Position {
                x: index as f32,
                y: 0.0,
            },
        );
        world.insert(entity, Velocity { x: 1.5, y: 0.5 });
    }

    c.bench_function("ecs/iter_ecs_2c_chunks", |b| {
        b.iter(|| {
            for (_, (mut positions, velocities)) in
                world.query_chunks::<(&mut Position, &Velocity)>()
            {
                for (position, velocity) in positions.iter_mut().zip(velocities) {
                    position.x += velocity.x;
                    position.y += velocity.y;
                }
            }
            black_box(&world);
        });
    });
}

fn ecs_one_component_chunks(c: &mut Criterion) {
    let mut world = World::new();
    for index in 0..ENTITIES {
        let entity = world.spawn();
        world.insert(
            entity,
            Position {
                x: index as f32,
                y: 0.0,
            },
        );
    }

    c.bench_function("ecs/iter_ecs_1c_chunks", |b| {
        b.iter(|| {
            let mut sum = 0.0f32;
            for (_, positions) in world.query_chunks::<&Position>() {
                for position in positions {
                    sum += position.x;
                }
            }
            black_box(sum);
        });
    });
}

criterion_group!(
    benches,
    aos_baseline,
    ecs_two_components,
    ecs_two_components_chunks,
    ecs_one_component,
    ecs_one_component_chunks
);
criterion_main!(benches);
