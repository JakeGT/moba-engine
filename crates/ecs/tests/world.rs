//! Integration tests for the public ECS surface.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use moba_core::{Rng, Tick};
use moba_ecs::{Entity, With, Without, World};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Position {
    x: i32,
    y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Velocity {
    x: i32,
    y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Tag(u32);

fn spawn_with(world: &mut World, position: Option<Position>, velocity: Option<Velocity>) -> Entity {
    let entity = world.spawn();
    if let Some(position) = position {
        world.insert(entity, position);
    }
    if let Some(velocity) = velocity {
        world.insert(entity, velocity);
    }
    entity
}

#[test]
fn spawn_insert_get_replace_remove_despawn() {
    let mut world = World::new();
    let entity = world.spawn();
    assert!(world.is_alive(entity));
    assert_eq!(world.entity_count(), 1);

    assert_eq!(world.insert(entity, Position { x: 1, y: 2 }), None);
    assert!(world.has::<Position>(entity));
    assert_eq!(
        world.get::<Position>(entity),
        Some(&Position { x: 1, y: 2 })
    );

    let previous = world.insert(entity, Position { x: 3, y: 4 });
    assert_eq!(previous, Some(Position { x: 1, y: 2 }));
    assert_eq!(
        world.get::<Position>(entity),
        Some(&Position { x: 3, y: 4 })
    );

    world.get_mut::<Position>(entity).unwrap().x = 9;
    assert_eq!(world.get::<Position>(entity).unwrap().x, 9);

    assert_eq!(
        world.remove::<Position>(entity),
        Some(Position { x: 9, y: 4 })
    );
    assert!(!world.has::<Position>(entity));
    assert_eq!(world.remove::<Position>(entity), None);

    assert!(world.despawn(entity));
    assert!(!world.is_alive(entity));
    assert!(!world.despawn(entity));
    assert_eq!(world.entity_count(), 0);
}

#[test]
fn archetype_moves_preserve_other_components() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, Position { x: 1, y: 2 });
    world.insert(entity, Velocity { x: 10, y: 20 });
    world.insert(entity, Tag(7));
    assert_eq!(
        world.archetype_count(),
        4,
        "empty, position, position+velocity, position+velocity+tag"
    );

    assert_eq!(
        world.remove::<Velocity>(entity),
        Some(Velocity { x: 10, y: 20 })
    );
    assert_eq!(
        world.get::<Position>(entity),
        Some(&Position { x: 1, y: 2 })
    );
    assert_eq!(world.get::<Tag>(entity), Some(&Tag(7)));
    assert!(!world.has::<Velocity>(entity));

    world.insert(entity, Velocity { x: -1, y: -2 });
    assert_eq!(
        world.get::<Velocity>(entity),
        Some(&Velocity { x: -1, y: -2 })
    );
    assert_eq!(
        world.get::<Position>(entity),
        Some(&Position { x: 1, y: 2 })
    );
    assert_eq!(world.get::<Tag>(entity), Some(&Tag(7)));
}

#[test]
fn swap_remove_updates_every_surviving_location() {
    let mut world = World::new();
    let entities: Vec<Entity> = (0..200)
        .map(|i| {
            spawn_with(
                &mut world,
                Some(Position { x: i, y: -i }),
                Some(Velocity { x: i, y: 0 }),
            )
        })
        .collect();

    for (index, entity) in entities.iter().enumerate() {
        if index % 3 == 0 {
            assert!(world.despawn(*entity));
        }
    }
    for (index, entity) in entities.iter().enumerate() {
        if index % 3 == 0 {
            assert!(!world.is_alive(*entity));
            continue;
        }
        assert_eq!(
            world.get::<Position>(*entity),
            Some(&Position {
                x: i32::try_from(index).unwrap(),
                y: -i32::try_from(index).unwrap()
            })
        );
        assert_eq!(
            world.get::<Velocity>(*entity),
            Some(&Velocity {
                x: i32::try_from(index).unwrap(),
                y: 0
            })
        );
    }
    assert_eq!(world.entity_count(), 200 - 200usize.div_ceil(3));
}

#[test]
fn queries_iterate_mutate_and_filter() {
    let mut world = World::new();
    let both = spawn_with(
        &mut world,
        Some(Position { x: 0, y: 0 }),
        Some(Velocity { x: 1, y: 2 }),
    );
    let position_only = spawn_with(&mut world, Some(Position { x: 5, y: 5 }), None);
    let velocity_only = spawn_with(&mut world, None, Some(Velocity { x: 9, y: 9 }));
    let tagged = spawn_with(&mut world, Some(Position { x: -1, y: -1 }), None);
    world.insert(tagged, Tag(1));

    assert_eq!(world.query::<(&Position, &Velocity)>().count(), 1);
    assert_eq!(world.query::<&Position>().count(), 3);
    assert_eq!(world.query::<(&mut Position, &Velocity)>().count(), 1);

    for (_, (position, velocity)) in world.query::<(&mut Position, &Velocity)>() {
        position.x += velocity.x;
        position.y += velocity.y;
    }
    assert_eq!(world.get::<Position>(both), Some(&Position { x: 1, y: 2 }));

    let with_velocity: Vec<Entity> = world
        .query_filtered::<&Position, With<Velocity>>()
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(with_velocity, vec![both]);

    let without_velocity: BTreeSet<Entity> = world
        .query_filtered::<&Position, Without<Velocity>>()
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(without_velocity, BTreeSet::from([position_only, tagged]));

    let filtered: Vec<Entity> = world
        .query_filtered::<&Position, (With<Position>, Without<Tag>)>()
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(filtered, vec![position_only, both]);
    assert!(world.is_alive(velocity_only));
    assert_eq!(world.entities().count(), 4);
}

#[test]
fn change_ticks_track_mutation_and_insertion() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, Position { x: 0, y: 0 });
    assert_eq!(world.changed_at::<Position>(entity), Some(Tick::ZERO));
    assert!(!world.changed_since::<Position>(entity, Tick::ZERO));

    world.advance_tick();
    world.get_mut::<Position>(entity).unwrap().x = 1;
    assert!(world.changed_since::<Position>(entity, Tick::ZERO));
    assert_eq!(world.changed_at::<Position>(entity), Some(Tick::new(1)));

    world.advance_tick();
    for (_, position) in world.query::<&mut Position>() {
        position.x += 1;
    }
    assert_eq!(world.changed_at::<Position>(entity), Some(Tick::new(2)));
    assert!(world.changed_since::<Position>(entity, Tick::new(1)));

    world.advance_tick();
    let _ = world.query::<&Position>().count();
    assert_eq!(
        world.changed_at::<Position>(entity),
        Some(Tick::new(2)),
        "a read-only query must not mark components changed"
    );
    assert_eq!(world.changed_at::<Tag>(entity), None);
}

#[test]
fn entity_generations_are_reused_safely() {
    let mut world = World::new();
    let old = world.spawn();
    world.insert(old, Position { x: 1, y: 1 });
    assert!(world.despawn(old));
    let new = world.spawn();
    assert_eq!(new.index(), old.index());
    assert_ne!(new.generation(), old.generation());
    assert!(!world.is_alive(old));
    assert!(world.is_alive(new));
    assert_eq!(world.get::<Position>(old), None);
    assert!(!world.has::<Position>(old));
    assert_eq!(world.remove::<Position>(old), None);
}

#[test]
#[should_panic(expected = "dead entity")]
fn inserting_into_a_dead_entity_panics() {
    let mut world = World::new();
    let entity = world.spawn();
    world.despawn(entity);
    world.insert(entity, Position { x: 0, y: 0 });
}

#[test]
#[should_panic(expected = "more than once")]
fn duplicate_query_components_panic() {
    let mut world = World::new();
    world.spawn();
    let _ = world.query::<(&Position, &Position)>();
}

#[test]
#[should_panic(expected = "requires and excludes")]
fn conflicting_filter_panics() {
    let mut world = World::new();
    world.spawn();
    let _ = world.query_filtered::<&Position, (With<Position>, Without<Position>)>();
}

#[derive(Debug)]
struct Tracked(Arc<AtomicUsize>);

impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn components_drop_exactly_once() {
    let drops = Arc::new(AtomicUsize::new(0));
    {
        let mut world = World::new();
        let a = world.spawn();
        world.insert(a, Tracked(Arc::clone(&drops)));
        world.insert(a, Position { x: 0, y: 0 });
        let b = world.spawn();
        world.insert(b, Tracked(Arc::clone(&drops)));

        assert!(world.despawn(a));
        assert_eq!(drops.load(Ordering::SeqCst), 1);

        let removed = world.remove::<Tracked>(b).expect("component exists");
        assert_eq!(drops.load(Ordering::SeqCst), 1, "remove returns ownership");
        drop(removed);
        assert_eq!(drops.load(Ordering::SeqCst), 2);

        world.insert(b, Tracked(Arc::clone(&drops)));
        world.insert(b, Position { x: 1, y: 1 });
    }
    assert_eq!(
        drops.load(Ordering::SeqCst),
        3,
        "world drop releases the rest"
    );
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Model {
    position: Option<Position>,
    velocity: Option<Velocity>,
    tag: Option<Tag>,
}

#[test]
fn randomised_operations_match_a_reference_model() {
    let mut world = World::new();
    let mut model: BTreeMap<Entity, Model> = BTreeMap::new();
    let mut rng = Rng::from_seed(0x5EED_1234);
    let mut next_value = 0i32;

    for step in 0..4_000 {
        let alive: Vec<Entity> = model.keys().copied().collect();
        let operation = rng.gen_range_u32(10);
        if operation == 0 || alive.is_empty() {
            let entity = world.spawn();
            model.insert(entity, Model::default());
            assert!(world.is_alive(entity));
            continue;
        }
        let target = alive[rng.gen_range_u32(alive.len() as u32) as usize];
        next_value += 1;
        if operation == 1 {
            assert!(world.despawn(target));
            model.remove(&target);
            continue;
        }
        let state = model.get_mut(&target).unwrap();
        match operation {
            2..=4 => {
                let previous = world.insert(
                    target,
                    Position {
                        x: next_value,
                        y: -next_value,
                    },
                );
                assert_eq!(
                    previous,
                    state.position.replace(Position {
                        x: next_value,
                        y: -next_value
                    })
                );
            }
            5..=6 => {
                let previous = world.insert(
                    target,
                    Velocity {
                        x: next_value,
                        y: 0,
                    },
                );
                assert_eq!(
                    previous,
                    state.velocity.replace(Velocity {
                        x: next_value,
                        y: 0
                    })
                );
            }
            7 => {
                let previous = world.insert(target, Tag(next_value as u32));
                assert_eq!(previous, state.tag.replace(Tag(next_value as u32)));
            }
            8 => {
                assert_eq!(world.remove::<Position>(target), state.position.take());
            }
            9 => {
                if rng.next_bool() {
                    assert_eq!(world.remove::<Velocity>(target), state.velocity.take());
                } else {
                    assert_eq!(world.remove::<Tag>(target), state.tag.take());
                }
            }
            _ => {
                if let Some(position) = world.get_mut::<Position>(target) {
                    position.x = next_value;
                    state.position.as_mut().unwrap().x = next_value;
                }
            }
        }

        if step % 97 == 0 {
            verify(&world, &model);
        }
    }
    verify(&world, &model);
}

fn verify(world: &World, model: &BTreeMap<Entity, Model>) {
    assert_eq!(world.entity_count(), model.len());
    let actual: BTreeSet<Entity> = world.entities().collect();
    let expected: BTreeSet<Entity> = model.keys().copied().collect();
    assert_eq!(actual, expected);
    for (entity, state) in model {
        assert_eq!(
            world.get::<Position>(*entity),
            state.position.as_ref(),
            "{entity:?}"
        );
        assert_eq!(
            world.get::<Velocity>(*entity),
            state.velocity.as_ref(),
            "{entity:?}"
        );
        assert_eq!(world.get::<Tag>(*entity), state.tag.as_ref(), "{entity:?}");
    }
}

#[test]
fn chunk_queries_match_item_queries() {
    let mut world = World::new();
    for index in 0..100i32 {
        let entity = world.spawn();
        world.insert(
            entity,
            Position {
                x: index,
                y: -index,
            },
        );
        world.insert(entity, Velocity { x: 1, y: 2 });
    }

    let mut chunk_count = 0;
    let mut chunk_sum = 0i64;
    for (entities, positions) in world.query_chunks::<&Position>() {
        assert_eq!(entities.len(), positions.len());
        chunk_count += entities.len();
        chunk_sum += positions
            .iter()
            .map(|position| i64::from(position.x))
            .sum::<i64>();
    }
    let item_count = world.query::<&Position>().count();
    let item_sum: i64 = world
        .query::<&Position>()
        .map(|(_, p)| i64::from(p.x))
        .sum();
    assert_eq!(chunk_count, item_count);
    assert_eq!(chunk_sum, item_sum);

    for (_, (mut positions, velocities)) in world.query_chunks::<(&mut Position, &Velocity)>() {
        for (position, velocity) in positions.iter_mut().zip(velocities) {
            position.x += velocity.x;
        }
    }
    let updated_sum: i64 = world
        .query::<&Position>()
        .map(|(_, p)| i64::from(p.x))
        .sum();
    assert_eq!(updated_sum, item_sum + 100);
}

#[test]
fn chunk_queries_respect_filters() {
    let mut world = World::new();
    let a = spawn_with(
        &mut world,
        Some(Position { x: 1, y: 0 }),
        Some(Velocity { x: 0, y: 0 }),
    );
    let _b = spawn_with(&mut world, Some(Position { x: 2, y: 0 }), None);
    let filtered: Vec<Entity> = world
        .query_chunks_filtered::<&Position, With<Velocity>>()
        .flat_map(|(entities, _)| entities.iter().copied())
        .collect();
    assert_eq!(filtered, vec![a]);
}
