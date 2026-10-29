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
moba-render    Vulkan renderer, terrain/fog, HUD batch, offscreen HUD images, capture
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

`moba-render` owns a Vulkan 1.3 device with core dynamic rendering and synchronization2. A
frame is a small render graph ([ADR-0008](adr/0008-render-graph.md)). Passes declare which
images and buffers they read and write; the graph is compiled once at renderer creation, and
compiling derives the `synchronization2` barriers and layout transitions, pools transients
and culls unused passes. Each pass is timed with GPU timestamps (`Renderer::gpu_timings`). The
passes run in this order:

1. `portraits` and `minimap` (R7b), only in frames that re-render them: the scene pipelines
   draw an `OffscreenRender` into one of the renderer's fixed offscreen images (a 2048×128
   hero-portrait strip and a 512×512 minimap by default, HDR format, each with its own depth
   buffer and per-frame buffers), lit like the scene but with shadows off (their lighting
   set names a default shadow image, ADR-0011);
2. `vfx_simulate` (compute) spawns, ages, integrates and compacts GPU particles and writes
   their indirect draw arguments ([ADR-0013](adr/0013-vfx-runtime.md));
3. `shadow` renders the main view's units (static meshes instanced, skinned heroes through a
   `SHADOW_PASS` permutation of the skinned vertex shader) depth-only into one directional
   shadow map fitted to the camera's view of the board and texel-snapped (1024²–4096² by
   tier; absent when shadows are off, ADR-0011);
4. `depth_prepass` (High and Ultra, [ADR-0016](adr/0016-lights-outlines-ao.md)) draws the
   terrain and units depth-only into the frame's depth (biased back, `invariant` positions)
   plus, with screen-space outlines, an `R8_UINT` outline-slot ID target;
5. `contact_shadows` (High and Ultra) ray-marches that depth towards the sun into an
   `R8_UNORM` visibility target; the pre-pass depth is sampled-capable here, the slot meant
   for R6's GTAO;
6. `scene` renders into an HDR target (`R16G16B16A16_SFLOAT`, or `B10G11R11_UFLOAT_PACK32` on
   the `Low` quality tier) with a sampled-capable depth buffer (loaded from the pre-pass with
   `LESS_OR_EQUAL` when there is one, else cleared), sampling the shadow map, the contact
   shadows and the light grid, and drawing inverted-hull outlines on Low and Medium;
7. `vfx_draw` draws the particles into the HDR target, testing the scene depth read-only and
   sampling it for soft particles; alpha particles accumulate weighted blended OIT targets on
   Medium and up, which
8. `vfx_resolve` composites over the HDR target;
9. `composite` applies exposure and an AgX tonemap into the swapchain image;
10. `outline` (screen-space outlines, High and Ultra) dilates the ID target's slots into
    palette-coloured silhouettes on the swapchain image;
11. `ui` draws the HUD on top, so text is never tonemapped, and samples the offscreen images;
12. `capture` copies the swapchain image for readback, in capture frames only.

The offscreen images are graph imports, not transients: they keep their contents across
frames and start and end every frame in `SHADER_READ_ONLY_OPTIMAL`, so a frame that does not
re-render one still samples last frame's contents with no transition. Every combination of
capture and offscreen passes is a variant compiled at creation (eight), and all share the
transient slots (the shadow map included, which keeps its fixed-size image across resizes).
Without VFX (`RenderConfig::vfx`, or no compute on the graphics queue) the VFX passes are
absent. The passes before `composite` are submitted ahead of the acquire-semaphore wait (they
never touch the swapchain), and the passes from `composite` on wait for the acquired image,
in one `vkQueueSubmit`. Descriptors are plain per-pass sets plus one set per material, allocated at
load from a pool sized by the registered content
([ADR-0009](adr/0009-descriptor-model.md), [ADR-0010](adr/0010-texture-pipeline.md)). Every
pipeline, each shader permutation included, is built at renderer creation through a
persistent `VkPipelineCache` (a per-user file, validated before use), so nothing compiles
mid-match. GLSL shares declarations through `#include` files and a small, enumerated set of
permutations (`MATERIAL_TEXTURED`) is compiled to separate SPIR-V at build time; debug builds
can hot-reload SPIR-V overrides from `MOBA_SHADER_DIR`. The scene pass draws:

- terrain (single procedural grid mesh tinted by the map theme) and instanced units; heroes
  use a client-local `.glb` model when one is configured (GPU-skinned when the file has a
  skin: a per-frame bone SSBO plus a push-constant bone range, ADR-0007; posed on the CPU by
  the animation runtime of ADR-0012 when the model has an animation set, else by its run clip
  and gait) and otherwise a deterministic multi-part animated rig; minions/structures use
  instanced cubes;
- a model with a client-local material (`materials/<model>.toml`, KTX2 textures in BC7/BC5/
  BC4 with mips, ADR-0010) draws with the `MATERIAL_TEXTURED` permutation and its material
  set (base colour tinted by the team colour through a mask, metallic/roughness, a normal map
  in the vertex tangent frame); everything else uses the vertex colour as albedo. BC textures
  are decoded on the CPU at load on devices without `textureCompressionBC`;
- everything is lit by one stylised PBR function ([ADR-0011](adr/0011-art-style-lighting.md)):
  GGX, Smith-GGX visibility, Schlick Fresnel and an energy-conserving diffuse, image-based
  ambient (SH-L2 irradiance plus a prefiltered specular cube through a split-sum BRDF LUT,
  from the map's client-local environment; hemispheric ambient without one), the shadow map
  (hard, PCF or PCSS-lite by tier) and a stylisation layer (wrap, ramp, painted specular,
  rim) set per map and per material. R2b ([ADR-0016](adr/0016-lights-outlines-ao.md)) adds
  dynamic point lights (clustered forward: the CPU bins the frame's `Scene::lights` into a
  32×32 grid of columns over the camera's ground footprint, with per-tier caps), a baked
  top-down AO map of the static occluders multiplying the ambient line (with a hook for
  R6's SSAO) and the contact-shadow term multiplying the shadow map. The lighting set (set 3:
  shadow map, environment cube, BRDF LUT, ramp, light list, light grid, baked AO, contact
  shadows) is one set per frame in flight, written at load and resize only (the light
  buffers' contents change per frame);
- team readability outlines on the units the client marks (`UnitInstance::outline`, a slot of
  the client's `OutlinePalette` in its team palette): screen-space from the pre-pass ID
  target on High and Ultra, an inverted hull in this pass on Low and Medium, depth-tested
  either way;
- up to 16 allied vision circles uploaded in the camera UBO; the terrain fragment shader
  darkens unseen ground.

The UI pass draws one screen-space batch: solid quads, the pixel-art icon atlas, MSDF text,
cooldown sweeps and (R7b) offscreen images, picked per vertex (`UiVertex::mode`, and
`UiVertex::image` for the image slot). The images are tonemapped in the UI shader with the
composite's AgX curve, so a portrait matches the world. The capture pass provides the GPU
readback used for deterministic screenshots and goldens.

The client fills the offscreen images from state it already has (`app::offscreen`): one
portrait cell per `(team, hero slot)`, rendered when the heroes change and, above the `Low`
tier, a few times a second with each skinned hero's idle clip; and the minimap, a top-down
orthographic render of the map's terrain, walls and props baked at every map bind. The HUD
darkens the minimap outside allied vision from the units it was sent, so nothing hidden is
revealed.

**VFX** are presentation only. The client's `VfxSystem` turns presentation events (casts,
projectiles, hits, zones, deaths, structures and basic attacks; network events are released
when the interpolated view reaches their snapshot's tick) into instances of client-local
effects (`vfx/bindings.toml` and `vfx/<effect>.toml`, never hashed or sent), whose emitters
follow units, animation sockets and extrapolated projectiles and emit `ParticleSpawn`s each
frame from a client-only RNG. Per-tier budgets, distance/visibility culling and the
readability rules (enemy telegraphs never culled and kept opaque enough; allied effects
dimmable) apply there and in the particle shaders. The renderer owns a fixed-capacity GPU
particle buffer and allocates each spawn a slot; nothing allocates per frame.

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
catalogue updates landed in the v24 protocol. Real asset meshes/skeletal assets and scripting hooks
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
- Polling is round-robin (the first socket rotates every pass) and capped per match at 64 datagrams or 500 µs per pass (`MatchHost::set_poll_budget`); excess datagrams stay in that socket's kernel queue, so a flooded port cannot starve its neighbours.
- An optional `HostBudget` (`--host-max-pps`) is a lock-free token bucket shared by every match on every worker.
- Workers record per-match tick durations in fixed-size histograms, plus frame lateness and overruns.
- `moba-tools load-test` drives N matches × C real clients and fails on budget misses; `--flood-port K --flood-pps N` adds a junk flood against one match ([perf note](perf/0010-multi-match.md)).

`NetServer` can watch a content file (`watch_content`/`--reload-content`) and swap it only
in Lobby/Results (never `Playing`); the simulation is recreated and connected clients receive a
full catalogue `ContentUpdate` for the next match. In a `MatchHost`, a single `ContentWatch`
loads and trust-checks the file once and every match adopts the same `Arc<GameContent>`
(`MatchHost::watch_content`). Replace the file atomically (write a temporary file, then `rename`).

`NetServer` applies a token-bucket rate limit per source address (default 120 pps), a
per-match packet budget (default 2000 pps) and, when installed, the host-wide budget, in that
order; `0` disables either of the first two. The check runs on the source address straight after
`recv_from` (`UdpTransport::recv_admitted`), before header decode, AEAD open or reassembly, so
undecodable datagrams are charged too. Buckets are bounded and evicted LRU to cap
memory under spoofed-source floods. `ServerStats` exposes bytes, snapshots, rate-limited
datagrams, capped polls and average/max simulation tick nanoseconds for operators and the soak harness.

## Runtime content

`GameContent` owns the runtime item, hero, ability and map catalogues; the simulation holds
an `Arc<GameContent>` and reads purchases, hero stats/ability bindings and map geometry from it. `NetServer::bind_with_content` validates the pack (`InvalidInput` if `GameContent::validate` fails);
the server publishes it in `Welcome`, and clients use the received `WireItem`, `WireHero`,
`WireCatalog`/`WireAbility`/`WireEffect`/`WireModifier` and `WireMap` entries for shop, HUD,
modifier names, effects, vision, range/radius overlays and minimap rendering. Abilities are
composed from targeting + fixed-capacity effects; projectiles carry on-hit effects and zones
run periodic area effects. Snapshots also carry a bounded, fog-filtered presentation-event
list (casts, projectile spawn/hit, deaths, objectives and, at the lowest priority, basic attacks) for the client kill feed/VFX, audio and animation. The wire catalogue is a complete, destructured projection of
`GameContent`; the client reconstructs it and verifies `GameContent::hash`. Ability
behaviour kinds are code-defined, but their numbers and hero bindings come from the pack; map,
lane, structure and wall geometry, rules, themes and mode sizes are fully data-driven. Packs
hot-reload only between matches (Lobby/Results), never during a live simulation; connected
clients receive a full `ContentUpdate` and atomically replace catalogue-dependent state.
Optional Ed25519 attestations cover the manifest + content hash, and servers can pin the
publisher key. Servers may rotate packs between empty-lobby matches.

`moba_game::content` is split by concern: `spec.rs` (in-memory specs), `schema.rs` (serde TOML
tables, `deny_unknown_fields`), `load.rs` (TOML → specs), `validate.rs` (range checks),
`hash.rs` (canonical catalogue feed, FNV hash and signing message), `manifest.rs` (pack
identity/attestation) and `error.rs`. A new content field touches `schema.rs`, `load.rs`,
`spec.rs`, `validate.rs` and `hash.rs`; the hash feed order is a replay/signature contract.

## Handshake replay window

Authenticated Hello nonces are tracked for 60 s (bounded to 2,048 entries). A repeated
key/nonce from the same source is treated as a retransmit; the same key/nonce from a different
source is rejected and counted as `replayed_hellos`.

Since the v37 handshake hardening:
- **Every keyed handshake** (PSK, token or identity) first answers an address-bound server `Challenge`. The Hello HMAC (`moba-hello-v2`) covers that challenge, so a captured Hello can't be replayed to another server or match that shares the same keys.
- **Session ids** are random 32-bit values.
- **Reconnects and re-handshakes** for an existing slot must authenticate with the same token/PSK, and in identity mode the same client public key, that opened it. Another valid credential gets reject code 3.
- **Hello rejects** are sent unsealed, so a spoofed Hello can't produce an authenticated Disconnect for a connected player.
- **PSK-only mode** is one shared trust domain. Every holder of the PSK is the same credential, so a slot is protected only by its random session id. Use per-client tokens or identities when players must not be able to impersonate each other.

Since v38, the catalogue carries **every hashed `RulesSpec` field**, including the economy
(`starting_gold`, passive gold, XP/shop radii, shared XP, rewards) and the win conditions
(`structure_heroes_first`, `nexus_win`, `score_limit`). A pack that overrides them is joinable,
and a joining client receives the server's rules instead of defaults.

Since v40 (#285), it also carries each hero's `model` and the map's `visuals` asset **names**.
They are hashed and signed with the rest of the pack. Pack file paths never go on the wire: a
client resolves the names against its own asset root, with procedural fallback, so differing
local assets are cosmetic only.

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
