# ADR-0003: Custom ECS

**Status:** accepted (2026-10-14)

## Decision
Write a purpose-built archetype ECS with SoA columns, generational entity handles, and per-component change ticks.

## Why not bevy_ecs / hecs
- Snapshot delta compression needs cheap "changed since tick N" queries per component, and stable network IDs. We want to design for both from day one.
- Server memory per match must be predictable: we want fixed-capacity pools and no hidden allocation during iteration.
- Prediction rollback on the client needs fast copy/restore of a subset of components.
- Owning the scheduler integration with `jobs` keeps the frame graph explicit.

## Consequences
- More code to own. M1 exit requires benches against an AoS baseline, plus miri-clean unsafe containers.
