# Architecture

This document describes the engine as a set of layers and explains where to extend it
without breaking determinism, authority or the server/client split.

## Layers

```
moba-core      fixed-point math, RNG, arenas, bitsets, time, hashing
moba-ecs       generational SoA archetype ECS
moba-jobs      scoped work-stealing pool
moba-game      static content tables + pure rules (no ECS, no clock, no I/O)
moba-nav       deterministic spatial grid
moba-sim       authoritative fixed-tick simulation (game + ecs + nav + core)
moba-net       UDP protocol, transport, prediction, snapshot model
moba-netsim    lossy UDP proxy used only by tests
moba-platform  winit window, input, fixed-step frame loop
moba-shaders   GLSL compiled/validated to SPIR-V
moba-render    Vulkan renderer, terrain/fog, HUD batch, capture
moba-client    playable client wiring render + platform + net + UI
moba-server    headless server wiring net + sim
moba-tools     soak/fuzz developer binaries
```

Dependencies point downward only. `moba-sim`, `moba-game`, `moba-nav`, `moba-net` and
`moba-server` must not depend on `moba-render`, `moba-platform`, `ash`, `winit` or
`gpu-allocator`. `just layering` enforces this with `cargo tree`, so a dedicated server can
be built on a machine with no Vulkan SDK.

## Determinism

- The simulation consumes `TickInputs` (intent) and a seeded configuration. It never reads a
  wall clock, environment variable or randomly ordered hash map.
- Gameplay math is integer or Q32.32 fixed-point (`moba_core::Fix`). Rendering interpolation
  may use floats; simulation collision, damage, movement and projectiles do not.
- Entity iteration follows deterministic archetype order. Bonds are broken by entity id.
- `Simulation::state_hash` folds all gameplay-relevant state (units, targets, cooldowns,
  progression, scores and projectiles). The 10k-tick tests assert identical hashes for idle
  and bot-driven matches.
- Content changes affect the hash and therefore replays. Publish content versions alongside
  replays.

## Match flow

`moba-net` runs an authoritative state machine: `Lobby → Countdown → Playing → Results →
rematch`. The simulation is paused outside `Playing`; snapshots carry the phase and remaining
countdown ticks so the client renders waiting/countdown/results overlays. A rematch rebuilds
the simulation with a derived seed, resets per-client network-id/snapshot baselines and sends
a full snapshot to every connected client; connections and slots survive. Human inputs are
accepted in `Playing`; shop purchases remain available at the nexus.

On silence the server marks a client disconnected but reserves its slot and lets a bot cover
for `reconnect_grace` (60 s default). The client can resume with the session token from the
welcome; the server rebinds the address, restores the slot, resets the input sequence and
sends a full snapshot. Resumption is explicit API today (no automatic client retry loop), and
the token is a demultiplexing identifier, not a credential until M7 adds authenticated
encryption.

## Server authority and netcode

1. The client sends one-shot intents (`MoveTo`, `Attack`, `CastAbility`, `BuyItem`) over UDP.
2. The server validates and applies them; movement/attack orders live in the simulation.
3. At 30 Hz the server builds per-team observations. Teammates and known structures are
   always included; enemies are included only inside allied vision radii.
4. Per client, the server assigns stable 16-bit network ids and encodes a snapshot as a
   delta against the latest snapshot the client acknowledged. Positions are quantised to
   1/16 unit; unchanged fields are omitted by a per-entity mask.
5. The client applies deltas from its snapshot history, predicts its own hero, interpolates
   remote units with a 4-tick buffer, and requests a full resync if a baseline is missing.
6. Inputs and snapshots are unreliable; redundancy and periodic full snapshots recover from
   loss. Handshake and welcome packets are retried.

The protocol version is part of every packet. Field-masked updates are additive; a new
version must bump `PROTOCOL_VERSION` and the client/server together.

### Optional encrypted transport

When a server is configured with a 32-byte pre-shared key:

1. `Hello` carries an HMAC-SHA256 proof; the server rejects a bad proof.
2. `Welcome` carries a server nonce and an HMAC over both nonces/session/slot.
3. Both sides derive a per-session key with HKDF-SHA256 and enable ChaCha20-Poly1305 for
   application payloads. The packet header fields are AEAD associated data; the nonce is
   session id + direction + packet sequence.
4. Handshake/Welcome packets stay unencrypted (the Welcome proof is authenticated); all
   gameplay payloads after the handshake are sealed. Fragmented messages are encrypted as a
   whole and reassembled before decryption.

No key material is logged. This is a private-server mode, not public identity; see
[SECURITY.md](../SECURITY.md).

## Rendering

`moba-render` owns a Vulkan 1.2 device with dynamic rendering and synchronization2. One
frame:

- terrain (single procedural grid mesh) and instanced cubes for units;
- up to 16 allied vision circles uploaded in the camera UBO; the terrain fragment shader
  darkens unseen ground;
- a screen-space UI batch (solid quads + built-in 5x7 bitmap font) drawn in the same pass;
- optional GPU readback capture for deterministic screenshots.

No renderer types leak into simulation or networking. A different renderer only needs
`Camera`, unit instances and a UI draw list (or can ignore the latter).

## ECS and jobs

`moba-ecs` stores each component type in per-archetype SoA columns with generational entity
ids. Queries compile to a per-archetype fetch and support item and chunk iteration; the chunk
API is the zero-allocation hot path. `moba-jobs` provides a scoped pool for data-parallel
work, with panic propagation; the scheduler is currently used by tools and benchmarks, not
the authoritative tick (which is single-threaded for determinism).

## Extension points

| Goal | Where |
|---|---|
| Add/tune items | `moba-game/src/item.rs` (`ITEMS`, costs, level growth, economy) |
| Add/tune heroes/abilities | `moba-game/src/hero.rs`, `ability.rs`; simulation spawn/bot logic picks them up |
| Change map/lanes/structures | `moba-game/src/map.rs` |
| Replace rendering | `moba-render` boundary; client uses `Renderer`, `Scene`, `UiDrawList` |
| Replace transport | `moba-net::transport`; protocol/snapshot modules are separate |
| Embed simulation | depend on `moba-sim`, call `tick(&TickInputs)` and read events/state hash |
| Add custom components/systems | `Simulation::world()`/`world_mut()`; see `crates/tools/examples/custom_component.rs` |
| Add server tooling | `moba-tools` binaries or a custom crate depending on `moba-sim`/`moba-net` |

**Current limit:** content is compile-time static data. Runtime content packs, hot reload and
asset definitions are M8. Until then, extending content means editing the tables and rebuilding;
the table layout is intentionally simple so a loader can replace it without changing the
simulation loop.

## Threading model

- Client: main thread owns window/renderer/simulation view; network polling is non-blocking
  on the main thread; jobs are available for future parallel scene work.
- Server: one thread drives receive → tick → snapshots at 30 Hz. Each client has bounded
  snapshot history and state; no locks are shared with the simulation.
- Tests: real UDP loopback, optional `moba-netsim` proxies, and deterministic single-process
  simulations.

## Runtime content

`GameContent` owns the runtime item and hero catalogues; the simulation holds an
`Arc<GameContent>` and reads purchase costs/bonuses and hero stats/ability bindings from it. `NetServer::bind_with_content` accepts a validated pack;
the server publishes it in `Welcome`, and clients use the received `WireItem` list for shop and
inventory labels plus `WireHero` for names/vision/ability bindings. Ability behaviour and
map/lane definitions remain compile-time; the content crate is designed to grow into them.

## Spectators

The server starts a match immediately by default. With `--ready` (or
`ServerConfig::require_ready`), every connected player must send `Ready { ready }`; snapshots
carry ready/player masks. Disconnecting or unreadying a player during the countdown returns
the lobby to `Lobby`. `F5` sends `Surrender`; each connected human on a team must vote, then a
recorded `PlayerCommand::Surrender` ends the match deterministically for replay.

A connection can request `spectate` in the handshake. Spectators occupy extra slots beyond the
ten players, receive the union of all units (no fog filtering), have gold/inventory fields
zeroed, cannot submit inputs, and render with a free overview camera. They use the same
snapshot/ack/reconnect machinery as players; no gameplay state is duplicated. Spectators are
capped by `ServerConfig::max_spectators`.

## Replays and debugging

The server can write deterministic `.mbr` replays (`--record FILE`). A replay stores the
simulation configuration plus every resolved per-tick input frame (post-bot resolution) and
the final state hash. `moba-tools replay` re-simulates the frames and verifies the hash, which
makes bug reports, balance regressions and CI fixtures shareable without video. Replays are
match-scoped; a rematch writes a `.matchN.mbr` file.

## Error and panic policy

Library code returns `Result` for I/O, Vulkan, protocol and validation errors; malformed UDP
is dropped, never panics. Decoders are bounds-checked and fuzzed. `unsafe` is restricted to
the five foundational/render crates and every block has a `// SAFETY:` proof. `debug_assert!`
guards internal invariants; release builds must not rely on them.
