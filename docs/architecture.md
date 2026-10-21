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

`moba-render` owns a Vulkan 1.3 device with core dynamic rendering and synchronization2. One
frame:

- terrain (single procedural grid mesh tinted by the map theme) and instanced units; heroes
  use a deterministic multi-part animated rig, minions/structures use instanced cubes;
- up to 16 allied vision circles uploaded in the camera UBO; the terrain fragment shader
  darkens unseen ground;
- a screen-space UI batch (solid quads + textured glyph/icon atlas) drawn in the same pass;
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
| Add/tune items | `content/*.toml` runtime packs; `moba-game/src/item.rs` holds defaults/economy |
| Add/tune heroes/abilities | runtime packs; `moba-game/src/hero.rs`/`ability.rs` define defaults and code-driven kinds |
| Change map/lanes/structures | `content/*.toml` `[map]`; `moba-game/src/map.rs` holds defaults |
| Replace rendering | `moba-render` boundary; client uses `Renderer`, `Scene`, `UiDrawList` |
| Replace transport | `moba-net::transport`; protocol/snapshot modules are separate |
| Embed simulation | depend on `moba-sim`, call `tick(&TickInputs)` and read events/state hash |
| Add custom components/systems | `Simulation::world()`/`world_mut()`; see `docs/building-on-moba.md` |
| Add server tooling | `moba-tools` binaries or a custom crate depending on `moba-sim`/`moba-net` |

**Extension limit:** content packs cover items, heroes, ability numbers/kinds, growth, rules,
maps, lanes, structures, vision walls, themes, attestations and map rotation. Live per-client
catalogue updates landed in protocol v24. Real asset meshes/skeletal assets and scripting hooks
remain future work (tracked in GitHub Issues).

## Threading model

- Client: main thread owns window/renderer/simulation view; network polling is non-blocking
  on the main thread; jobs are available for future parallel scene work.
- Server: one thread drives receive → tick → snapshots at 30 Hz. Each client has bounded
  snapshot history and state; no locks are shared with the simulation.
- Tests: real UDP loopback, optional `moba-netsim` proxies, and deterministic single-process
  simulations.

## Visibility

Each map can carry vision-blocking wall segments. The server intersects the source→unit
segment against walls before revealing an enemy; allied units and structures stay visible.
The terrain shader receives the same segments and rejects a vision-circle contribution when
a wall crosses the circle-center→fragment ray. The client renders wall geometry in the world
and as segments on the minimap.

The server binary accepts a TOML deployment file (`--config`); CLI flags override file
values, and secrets can reference paths or 64-hex strings.

`NetServer::run_until` accepts a shutdown flag; the server binary sets it from
Ctrl-C/SIGINT, then flushes the recording before exiting.

`MatchHost` (`crates/net/src/host.rs`) runs many `NetServer`s in one process (`moba-server --matches N`).
- Each match keeps its own UDP socket on consecutive ports, so the protocol and client are unchanged.
- Matches are split across long-lived worker threads. Each worker polls its sockets and ticks every due match on the shared 30 Hz schedule.
- Workers record per-match tick durations in a fixed-size histogram, plus frame lateness and overruns.
- `moba-tools load-test` drives N matches × C real clients and fails on budget misses ([perf note](perf/0010-multi-match.md)).

`NetServer` can watch a content file (`watch_content`/`--reload-content`) and swap it only
in Lobby/Results (never `Playing`); the simulation is recreated and connected clients receive a
full catalogue `ContentUpdate` for the next match.

`NetServer` applies a token-bucket rate limit per source address (default 120 pps) and a
global packet budget (default 2000 pps) before message decoding; `0` disables either. Buckets are bounded and evicted LRU to cap
memory under spoofed-source floods. `ServerStats` exposes bytes, snapshots, rate-limited
datagrams and average/max simulation tick nanoseconds for operators and the soak harness.

## Runtime content

`GameContent` owns the runtime item, hero, ability and map catalogues; the simulation holds
an `Arc<GameContent>` and reads purchases, hero stats/ability bindings and map geometry from it. `NetServer::bind_with_content` accepts a validated pack;
the server publishes it in `Welcome`, and clients use the received `WireItem`, `WireHero`,
`WireCatalog`/`WireAbility`/`WireEffect`/`WireModifier` and `WireMap` entries for shop, HUD,
modifier names, effects, vision, range/radius overlays and minimap rendering. Abilities are
composed from targeting + fixed-capacity effects; projectiles carry on-hit effects and zones
run periodic area effects. Snapshots also carry a bounded, fog-filtered presentation-event
list (casts, projectile spawn/hit, deaths, objectives) for the client kill feed/VFX and audio. The wire catalogue is a complete, destructured projection of
`GameContent`; the client reconstructs it and verifies `GameContent::hash`. Ability
behaviour kinds are code-defined, but their numbers and hero bindings come from the pack; map,
lane, structure and wall geometry, rules, themes and mode sizes are fully data-driven. Packs
hot-reload only between matches (Lobby/Results), never during a live simulation; connected
clients receive a full `ContentUpdate` and atomically replace catalogue-dependent state.
Optional Ed25519 attestations cover the manifest + content hash, and servers can pin the
publisher key. Servers may rotate packs between empty-lobby matches.

## Handshake replay window

Authenticated Hello nonces are tracked for 60 s (bounded to 2,048 entries). A repeated
key/nonce from the same source is treated as a retransmit; the same key/nonce from a different
source is rejected and counted as `replayed_hellos`.

Since protocol v37:
- **Every keyed handshake** (PSK, token or identity) first answers an address-bound server `Challenge`. The Hello HMAC (`moba-hello-v2`) covers that challenge, so a captured Hello can't be replayed to another server or match that shares the same keys.
- **Session ids** are random 32-bit values.
- **Reconnects and re-handshakes** for an existing slot must authenticate with the same token/PSK, and in identity mode the same client public key, that opened it. Another valid credential gets reject code 3.
- **Hello rejects** are sent unsealed, so a spoofed Hello can't produce an authenticated Disconnect for a connected player.
- **PSK-only mode** is one shared trust domain. Every holder of the PSK is the same credential, so a slot is protected only by its random session id. Use per-client tokens or identities when players must not be able to impersonate each other.

## Client identity

With `require_client_identity`, the server sends a random `Challenge` packet; the client
replies with its Ed25519 public key and a signature over protocol, client nonce, challenge,
reconnect token and spectator flag. The challenge is address-bound, expires after 10 s and is
consumed on success. Public keys are allowlisted; combined token + identity modes are
supported.

## Client tokens

`NetServer::add_client_key` keeps a bounded key ring. A Hello HMAC is tried against the global
PSK and every client key; the matching key is stored on the connection and used for that
player's Welcome HMAC and ChaCha20-Poly1305 session. `--allow-token` / `client_tokens` expose
this for per-player access and key rotation without wire changes.

## Server identity

An operator can install a 32-byte Ed25519 seed (`--server-key FILE`). `Welcome` then carries
the public key and a signature over protocol, nonces, session, slot and `GameContent::hash`.
Clients that pin `MOBA_SERVER_PUBKEY` require a matching signature; unpinned clients continue
to work. Identity is independent of the optional PSK encryption mode.

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
simulation configuration, content hash/manifest, every resolved per-tick input frame
(post-bot resolution), periodic state-hash checkpoints (every 150 ticks) and the final state
hash. Verification rejects a mismatched content pack first, then reports the first divergent
checkpoint with its tick. `moba-tools replay` re-simulates the frames and verifies the hash, which
makes bug reports, balance regressions and CI fixtures shareable without video. Replays are
match-scoped; a rematch writes a `.matchN.mbr` file.

## Error and panic policy

Library code returns `Result` for I/O, Vulkan, protocol and validation errors; malformed UDP
is dropped, never panics. Decoders are bounds-checked and fuzzed. `unsafe` is restricted to
the five foundational/render crates and every block has a `// SAFETY:` proof. `debug_assert!`
guards internal invariants; release builds must not rely on them.
