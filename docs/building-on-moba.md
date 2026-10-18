# Building on MOBA Engine

The workspace is layered so you can embed only what you need.

| Crate | Responsibility |
|---|---|
| `moba-core` | fixed-point math, deterministic hashing, time |
| `moba-ecs` | archetype SoA world and queries |
| `moba-jobs` | work-stealing scheduler |
| `moba-game` | static definitions and validated content packs |
| `moba-sim` | deterministic tick, combat, bots, replay |
| `moba-net` | UDP protocol, snapshots, prediction, server/client |
| `moba-render` / `moba-platform` | Vulkan renderer and window/input |
| `moba-engine` | umbrella crate with `sim`, `net`, `render` features |

## Headless simulation

```rust
use moba_engine::prelude::*;

let mut sim = Simulation::new(SimConfig::default());
let mut inputs = TickInputs::new();
inputs.set(0, PlayerCommand::MoveTo(FixVec2::from_ints(80, 128)));
for _ in 0..30 {
    sim.tick(&inputs);
    sim.clear_events();
}
println!("{:?}", sim.state_hash());
```

Runnable examples:

```bash
cargo run -p moba-engine --example headless_match
cargo run -p moba-engine --example custom_content
```

Rules for anything that affects gameplay:

- no wall-clock time, threads, hashing or floating-point in the tick;
- commands enter through `TickInputs`, never directly from sockets in simulation code;
- every content-defined number must be hashed;
- run the workspace tests and `cargo test -p moba-sim --test allocations` before submitting.

## Custom ECS components and systems

```rust
use moba_engine::ecs::World;

#[derive(Copy, Clone)]
struct Burn { remaining: u32, damage: i32 }

let mut world = World::new();
let entity = world.spawn();
world.insert(entity, Burn { remaining: 30, damage: 5 });
for (entity, burn) in world.query::<&mut Burn>() {
    burn.remaining = burn.remaining.saturating_sub(1);
    if burn.remaining == 0 {
        // read/write other components or queue an event here
    }
}
```

Keep systems order-explicit: the simulation's fixed order is part of the replay contract. Add a
regression test with the same seed and assert `state_hash()`.

`NetServer` has **no custom-system hook yet** (#39). Embed your own `Simulation` loop if you
need extra systems; a dedicated-server hook is planned, not available today.

## Content packs

Runtime packs are TOML validated by `moba-game` and carried by `Welcome`:

```bash
cargo run -p moba-tools --bin content-check -- content/example-pack.toml
```

See [content-authoring.md](content-authoring.md). Servers can reload an empty lobby with
`--reload-content`; connected clients keep their original handshake catalogue.

## Embedding the server

```rust
use moba_net::{NetServer, ServerConfig};

let mut server = NetServer::bind(ServerConfig::default())?;
server.run_for(std::time::Duration::from_secs(600))?;
```

`NetServer::add_client_key` enables per-player token sessions. `--allow-token` and
`--server-key` are the CLI equivalents; see [SECURITY.md](../SECURITY.md). Dedicated
deployments should use [server.example.toml](../server.example.toml).

## Adding a new ability behaviour

1. Add a variant to `AbilityKind` in `moba-game`.
2. Handle it in validation, `content_hash`, and the TOML parser.
3. Execute it in `Simulation::cast_ability`; add components/tick timers as needed.
4. Add a presentation event/status tag and bump the protocol. Do not assume a spare snapshot
   flag bit; the current flags bitset is nearly full (#41).
5. Add a unit test for application, expiry, and state hash; update docs/content-authoring.md.
6. Add the kind to `AbilityKind` hashing, the content→wire parity conversion and the client
   catalogue.

## Extension checklists

Use these touch-point lists for changes that cross the content/simulation/wire boundary.

**Add an item stat**
- `moba-game`: `ItemSpec`, `ItemDef`/default table, `ItemEntry` TOML parser, `validate`,
  `GameContent::hash`.
- `moba-net`: `WireItem`, encode/decode, catalogue hash, `GameContent → WireCatalog` parity.
- `moba-sim`: item purchase/stat application and state hash (plus snapshot progress).
- Client: shop/tooltip rendering and tests.
- Docs/tests: content-authoring, API/wire version note if the shape changes, replay fixture.

**Add a status effect**
- `moba-sim`: effect/modifier data, apply/expiry/recompute path, `state_hash` unique tag, allocation guard.
- `moba-game`: ability/content definition and validation.
- `moba-net`: wire status flag/tag, snapshot encode/decode, client tint/icon; bump protocol.
- Tests: application, independent expiry, replay verification, snapshot round-trip.
- Docs: content-authoring, status, CHANGELOG.

**Add an ability kind**
- `moba-game`: `AbilityKind`, parser/validation/hash, defaults and content examples.
- `moba-sim`: cast/targeting/execution, cooldown/mana/range rejection, events, state hash.
- `moba-net`: wire kind/display fields, parity test, protocol bump when needed.
- Client: tooltip/range/visual; add a capture/golden check if visual.
- Docs/tests: content-authoring, ability checklist, replay/fixture, CHANGELOG.

## Replays and diagnostics

`Replay` stores content hash/manifest, resolved inputs (including purchases), periodic state checkpoints (v5) and the
final hash. `moba-tools replay --items pack.toml match.mbr` reports the first divergent
checkpoint if a replay breaks. Add `--json` to `replay` or `content-check` for machine-readable
summaries.

## Quality gates

```bash
just ci-core          # fmt, clippy, tests, docs, feature matrix, layering
just ci-tools         # content-check, 10-match soak, benchmarks compile
just ci               # ci-core + ci-tools + cargo-deny
just soak             # 5-minute lossy 10-client network soak
```

CI runs the gates on Linux, Windows and macOS; Miri covers the unsafe containers in
`moba-core`, `moba-ecs` and `moba-jobs`.
