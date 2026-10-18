# ADR 0005: Content-composition extension for new ability behaviour

## Status
Accepted (2026-10-18).

## Context
New ability behaviour (arcing projectiles, summons, auras) previously required ad-hoc engine
matches. Dynamic scripting would add code execution, sandboxing and dependency risk to the
authoritative path.

## Decision
Extend the typed, data-only `Effect`/`AreaEffect` composition instead of embedding a scripting
runtime. The first application is a visual arcing projectile:

- `Effect::Projectile { arc }` carries a bounded fixed-point arc height.
- TOML abilities accept optional `arc`; validation/hash/wire carry it.
- The simulation stores/hashes the arc; protocol v17 sends it in projectile spawn events.
- The client renders height `arc * sin(pi * progress)` from fixed-point event data.

## Threat model
- Content is not executable; only validated numbers/enums reach the simulation.
- All fields are bounded, fixed-point and hashed; unknown TOML fields are rejected.
- Wire decoders reject out-of-range kinds/counts; malformed catalogues fail handshake.
- A future scripting proposal must be a separate ADR with a sandbox, resource limits,
  deterministic host-call surface and replay/content versioning.

## Consequences
- Deterministic and allocation-free on the hot path; no new dependency.
- New primitive types still need engine code, but composition covers projectile arcs,
  on-hit effects, areas and zones.
- Replay compatibility is explicit through content/protocol hashes (`SIM_LOGIC_VERSION` stays 5:
  default behaviour is unchanged; packs with arcs change their content hash).
