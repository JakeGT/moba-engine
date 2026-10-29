# Intended 1.0 API surface

This is the WP-7.2 API review ([finish plan, Phase 7](plans/finish-plan-2026-10-20.md)). It lists,
per library crate, what the 1.0 surface is meant to be, which types are `#[non_exhaustive]` and
why, what stays exhaustive on purpose, and what is public but not covered by the semver promise.
The tiers and the change rules are in [API-STABILITY.md](API-STABILITY.md); the breaking changes
this review made are listed under `## [Unreleased]` → *Breaking changes* in
[CHANGELOG.md](../CHANGELOG.md).

The crates covered are the ones `scripts/semver-check.sh` diffs against the last release tag:
`moba-core`, `moba-ecs`, `moba-jobs` (Foundation); `moba-game`, `moba-sim`, `moba-net`,
`moba-nav`, `moba-render`, `moba-platform`, `moba-assets` (Engine); `moba-engine` (Embedding).
Application crates (`moba-client`, `moba-server`, `moba-tools`, `moba-netsim`, `moba-shaders`,
`moba-test-support`) have no API promise.

## Conventions

These rules decide whether a public type is `#[non_exhaustive]`.

| Kind of type | Rule | How callers use it |
|---|---|---|
| Error enums | `#[non_exhaustive]` | Match the variants you handle, then add a `_` arm (or use `Display`). |
| Event, outcome, phase, command and input-key enums | `#[non_exhaustive]` | Same: add a `_` arm. New variants arrive in minor releases. |
| Stats, reports and results that callers only read | `#[non_exhaustive]` | Read fields. Use `Default` where a zero value is needed. |
| Configs and options | `#[non_exhaustive]` + `Default` + one `with_*` builder per field | `ServerConfig::default().with_max_clients(4)`; or assign the public fields of a `let mut` binding. A struct literal (also with `..Default::default()`) no longer compiles outside the defining crate. |
| Types with constructors (`Scene`, `UnitInstance`, `Camera`, `Replay`, `ObservedUnit`, `AssetEntry`, `TickInputs`) | `#[non_exhaustive]` | Use the constructor and `with_*` methods, then assign public fields. |
| Content schema (`GameContent`, `*Spec`, content enums) | **Exhaustive by design** (see below) | Struct literals and exhaustive matches work. A new field or variant is a breaking change. |
| Wire messages (`Header`, `Hello`, `Welcome`, `InputMessage`, `NetEntity`, `NetEvent`, `NetProgress`, `SnapshotDelta`) | **Exhaustive by design** | Their layout is fixed per `PROTOCOL_VERSION`; tools and fuzzers build them field by field. |
| Value types (`FixVec2`, `Aabb`, `JointTransform`, `BoneRange`, `UiVertex`, ECS components) | **Exhaustive by design** | Closed mathematical, GPU or component layouts. |

Within the workspace, a `_` arm on a `#[non_exhaustive]` enum from another crate keeps today's
behaviour: every such arm is unreachable today and says so in a comment.

### Why the content schema stays exhaustive

`GameContent`, the `*Spec` structs, `ModifierDef`, `ContentManifest`, `PackAttestation` and the
content enums (`Targeting`, `Effect`, `AreaEffect`, `ModifierKind`, `MapTheme`, `DamageType`,
`StructureKind`) are the content schema. They are left exhaustive on purpose:

- Every field and variant is hashed (`GameContent::hash`) and sent in the wire catalogue, so
  adding one is already a versioned event: the content hash changes, the protocol is bumped and
  old replays stop verifying.
- `moba-net` destructures every spec exhaustively when it builds the wire catalogue
  (`protocol/catalog_from.rs`) and in its parity tests. That is the compile-time check that a new
  field reaches the wire. `moba-sim` and `moba-net` also match the content enums exhaustively, so
  a new effect primitive cannot be silently ignored by the executor or the encoder (see the
  extension checklists in [building-on-moba.md](building-on-moba.md)). `#[non_exhaustive]`
  would turn both checks off outside `moba-game`.
- Embedders, tests and tools legitimately build specs in code (`GameContent { .. }`,
  `AbilitySpec { .. }`). Where a `Default` exists, prefer `..Default::default()` to keep the
  change small when the schema grows.

**Maintainer decision (2026-10-28): the content schema stays exhaustive.** The project is in early
development and breaking changes are expected until 1.0, so the schema is not made
`#[non_exhaustive]` now. With this choice, a content-schema extension after 1.0 needs a major
version. The option considered was: `#[non_exhaustive]` specs built through `Default` and field
assignment, with the `moba-net` destructuring guard replaced by a round-trip test over a pack
that sets every field. It would also change how the `moba-net` parity tests work.

## Per-crate surface

"Public but unstable" items are public because the workspace's own binaries, tests or fuzzers
use them, but their shape follows an internal contract (wire protocol, GPU layout, simulation
logic). They are documented as such here rather than hidden with `#[doc(hidden)]`; no item is
`#[doc(hidden)]` today.

### Foundation tier

**`moba-core`** — `Fix`, `FixVec2`, `Rng`, `StableHasher`/`fnv1a64`, `Tick`/`FixedStep`,
`Arena`/`ArenaKey`, `BitSet`/`BitSet64`, `math` (`Aabb`, interpolation helpers, camera
matrices; R7b added `orthographic_rh`, additive).
- No change in this review. Every struct either has private fields (`Arena`, `Rng`,
  `FixedStep`, `BitSet`) or is a closed value type (`Fix`, `FixVec2 { x, y }`, `Aabb { min, max }`,
  `Tick`).
- Public but unstable: `math` re-exports `glam` types (`Vec2`..`Mat4`, `Quat`), so a `glam`
  major upgrade is a breaking change of `moba-core`. The `f32`/`f64` conversions on `Fix` are
  presentation-only and must not be used in gameplay code.

**`moba-ecs`** — `World`, `Entity`, `Component`, `QueryData`/`QueryFilter`, `With`/`Without`,
`QueryIter`, `QueryChunks`, `MutSlice`, `MAX_COMPONENTS`.
- No change. All structs have private fields. `World::register_component` and
  `registered_component_types` are diagnostics.

**`moba-jobs`** — `JobSystem`, `Scope`.
- No change. Both are opaque.

### Engine tier

**`moba-game`** — teams, combat maths, the compile-time default tables (`HEROES`, `ITEMS`,
`ABILITIES`, `MAP`, `LANES`, `STRUCTURES`, minion/structure stats) and runtime content
(`GameContent`, `*Spec`, `load_content_file`/`load_content_toml`, validation limits
`MAX_*`).
- `#[non_exhaustive]`: `ContentError`, `ContentLoadError`.
- Exhaustive by design: the content schema above; the static definition structs (`HeroDef`,
  `ItemDef`, `AbilityDef`, `MapDef`, `LaneDef`, `StructureDef`, `MinionDef`, `StructureStats`,
  `LevelGrowth`, `EffectList`, `AreaEffectList`), which are `const` tables mirrored by the
  specs; `Team` (exactly two teams is a core invariant).

**`moba-sim`** — `Simulation`, `SimConfig`, `TickInputs`, `PlayerCommand`, `SimEvent`,
`MatchOutcome`, `ObservedUnit`, `BotPolicy`/`DefaultBotPolicy`, `Replay`/`ReplayResult`/
`ReplayError`, the ECS components (`Position`, `Health`, `Mana`, `Modifiers`, …) and `UnitKind`.
- `#[non_exhaustive]` enums: `SimEvent`, `MatchOutcome`, `PlayerCommand`, `UnitKind`,
  `ReplayError`.
- `#[non_exhaustive]` structs: `SimConfig` (`Default` + `with_seed`, `with_bot_mask`,
  `with_hero_picks`, `with_spawn_waves`), `TickInputs` (`new`/`Default` + `set`,
  `set_purchase`), `Replay` (`Replay::new` + public fields), `ReplayResult`, `ObservedUnit`
  (new `ObservedUnit::new(entity, kind, team, position)` for tests and custom snapshot
  producers).
- Public but unstable: the ECS component structs. They are public so extension systems and bot
  policies can query `Simulation::world()`, but their layout follows `SIM_LOGIC_VERSION` and
  the state hash. Read them; avoid inserting or rewriting them except through documented
  commands.

**`moba-net`** — `NetServer`/`ServerConfig`/`ServerStats`/`ServerPhase`, `NetClient`/
`ClientAuth`/`ClientEvent`/`ClientState`/`ClientStats`, `MatchHost`/`HostBudget`/
`ContentWatch`/`HostReport`/`WorkerStats`/`TickHistogram`, `PredictionState`, `Interpolator`,
`UdpTransport`, `protocol` and `crypto`.
- `#[non_exhaustive]` enums: `WireError`, `CryptoError`, `PacketKind`, `WireCommand`,
  `MatchPhase`, `ClientEvent`, `ClientState`, `ServerPhase`.
- `#[non_exhaustive]` structs: `ServerConfig` and `ClientAuth` (`Default` + a `with_*` per
  field); `ServerStats`, `ClientStats`, `WorkerStats`, `HostReport`, `ReceivedMessage`,
  `SnapshotState`, `NetUpdate`; the wire catalogue `WireCatalog` and every `Wire*` record in it
  (`WireItem`, `WireHero`, `WireAbility`, `WireEffect`, `WireAreaEffect`, `WireModifier`,
  `WireMap`, `WireLane`, `WireStructure`, `WireWall`, `WireRules`, `WireRewards`,
  `WireUnitRewards`, `WireMinionStats`, `WireStructureStats`), which only `moba-net` builds.
- Removed (accidental public helpers, unused in the workspace): `client::{encode_raw,
  decode_raw, decode_welcome_payload, decode_hello_payload, decode_snapshot_payload}`,
  `server::{decode_input_message, apply_delta}` and `transport::EventQueue`. Use
  `protocol::{encode_packet, decode_packet, decode_welcome, decode_hello, decode_snapshot,
  decode_input}` and `SnapshotState::apply`.
- Now `pub(crate)`: `crypto::{random_session, random_nonce}` (OS-entropy helpers used only by
  the handshake).
- Exhaustive by design: the wire messages listed in the conventions table; `AckTracker`.
- Public but unstable: `protocol` (codec functions, packet/flag/status/event constants and
  message structs) and the handshake primitives in `crypto` (`hello_tag`, `welcome_tag`,
  `derive_session_key`, `sign_*`/`verify_*`). They are public for third-party clients, the
  fuzzers and the known-answer tests, and their shape follows `PROTOCOL_VERSION`; a protocol
  bump may change them in a minor release.
- Follow-up before 1.0: `NetClient` has 18 constructors: 5 non-blocking `start*` and 13
  blocking `connect*`, one per combination of key, session, pin and spectator.
  `connect_with_auth(ClientAuth)` covers every option except session resumption. With a
  `session` option on `ClientAuth` and a non-blocking `start_with_auth`, the other 16 could be
  deprecated, and a new option would be a `ClientAuth` field instead of more constructors.

**`moba-nav`** — `SpatialGrid`. No change (opaque).

**`moba-render`** — `Renderer`, `RenderConfig`, `RenderError`/`RenderResult`, `Scene`,
`Camera`, `UnitInstance`, `MeshId`, `BoneRange`, `VisionCircles`, `UiDrawList`/`UiVertex`,
`CapturedFrame`, `probe_vulkan`/`VulkanProbe`, the UI atlas builder and the diagnostics counters.
R1a (ADR-0008) added `QualityTier`, `GpuTimings`/`PassTiming` (`Renderer::gpu_timings`) and
`RenderError::RenderGraph`. R1b (ADR-0010) added materials and textures
(`Renderer::register_texture`, `register_materials`, `set_mesh_material`, `material_stats`,
`compressed_textures_supported`; `MaterialData`, `MaterialId`, `TextureId`, `MaterialStats`,
re-exported `TextureData`/`TextureFormat`/`MaterialShader`/`TextureSlot`), the pipeline cache
(`Renderer::pipeline_stats`, `PipelineStats`, `PipelineCacheState`, `PipelineCacheRejection`,
`RenderConfig::with_pipeline_cache`), dev shader overrides (`RenderConfig::with_shader_dir`,
`Renderer::reload_shaders`), `RenderConfig::with_compressed_textures`, `GpuTimings::frame`
and `RenderError::{TooManyTextures, TooManyMaterials, InvalidMaterial}`. `QualityTier::ALL` is
now a `&'static [QualityTier]` slice, so adding a tier is not a breaking change. R7a added `TextStyle` (MSDF text styling) and
`pack_rgba8`. R7b added render-to-texture for the HUD: `UiImage` (offscreen image slots,
`UI_IMAGE_SLOTS`), `OffscreenRender` and `ClearRect` (a frame's renders into them),
`Renderer::{render_with_offscreen, capture_frame_with_offscreen, ui_image_extent,
last_frame_submitted}`,
`RenderConfig::ui_image_sizes`/`with_ui_image_size` (`DEFAULT_UI_IMAGE_SIZES`,
`MAX_UI_IMAGE_SIZE`), `OFFSCREEN_MAX_INSTANCES`/`OFFSCREEN_MAX_BONES`, `Camera::orthographic`,
`UiDrawList::image` and `UiVertex::MODE_IMAGE`. `render_with_offscreen` returns whether the
frame was submitted (`RenderResult<bool>`), so a caller can re-send renders a skipped frame
dropped; an error can still follow a submit (presentation), which `last_frame_submitted`
reports. The alpha of `Scene::terrain_tint` now scales the terrain grid lines (every built-in
tint already uses `1.0`, the old look). R2a (ADR-0011) added lighting: `LightingConfig`
(`for_tier`, `with_shadows`, `with_shadow_resolution`, `with_specular_ibl`, `with_ibl`,
`with_pcss_taps`), `ShadowFilter` (`ALL`, `name`, `from_name`), `SceneLighting` (`DEFAULT` + a
`with_*` per group), `Scene::lighting`/`with_lighting`, `RenderConfig::lighting`/
`with_lighting`/`effective_lighting`, `Renderer::{lighting_config, set_environment,
clear_environment, has_environment, set_lighting_ramp}`, `MaterialData`'s stylisation fields
(`wrap`, `rim`, `ramp`, `specular`, `specular_tint`) with `with_metallic_roughness`,
`with_emissive`, `with_style` and `with_specular`, `MIN_SHADOW_RESOLUTION`/
`MAX_SHADOW_RESOLUTION`, the re-exported `EnvironmentDesc` and
`RenderError::InvalidEnvironment`. R5 (ADR-0013) added GPU particles: `ParticleSpawn`
(`new` + `with_*`), `ParticleShape`, `ParticleBlend`, `ParticleRole`, `VfxFrame` (`new`,
`EMPTY`, `with_ally_opacity`; `Scene::with_vfx`), `VfxBudget` (`QualityTier::vfx_budget`),
`VfxStats`, `VFX_MESH_SLOTS`, `RenderConfig::with_vfx`, `Renderer::{vfx_available,
vfx_budget, vfx_stats, set_vfx_atlas, set_vfx_mesh}` and `RenderError::Vfx`. R4b (ADR-0012
amendment) added vertex-animation-texture meshes: `Renderer::{register_vat_mesh,
vat_available, vat_mesh_count, mesh_capacity}`, `UnitInstance::with_vat` (and its `vat` field),
`MAX_VAT_MESHES`, `RenderError::InvalidVat` and re-exports of `moba-assets`'
`VatDescriptor`, `VatPose` and `VatSample`. R6 (ADR-0015) added post-processing: `PostConfig` (`for_tier`, `off`, a `with_*` per field, `sanitized`,
`scaled`), `AntiAliasing` and `UpscaleFilter` (`name`, `from_name`), `PostLook` (`DEFAULT` +
`with_*`, `sanitized`: per-map exposure, eye adaptation, bloom and grading strength),
`ScreenEffects` (`NONE` + `with_desaturation`, `with_vignette`; `Scene::effects`/
`with_effects`), `MIN_RENDER_SCALE`/`MAX_RENDER_SCALE`, `RenderConfig::post`/`with_post`/
`effective_post`, `Renderer::{post_config, post_look, set_post_look, set_grading_lut,
render_scale, render_extent}` and `GpuTimings::group_ms`.
- `#[non_exhaustive]` enums: `RenderError`, `QualityTier` (more tiers may be added; match with a
  `_` arm).
- `#[non_exhaustive]` enums (R1b): `PipelineCacheState`, `PipelineCacheRejection`; (R2a):
  `ShadowFilter`; (R5): `ParticleShape`, `ParticleBlend`, `ParticleRole`; (R6):
  `AntiAliasing`, `UpscaleFilter`.
- `#[non_exhaustive]` structs (R2a): `LightingConfig` (`for_tier`/`Default` + `with_*`) and
  `SceneLighting` (`DEFAULT`/`Default` + `with_*`).
- `#[non_exhaustive]` structs (R5): `ParticleSpawn` (public fields, built with `new` and
  `with_*`), `VfxFrame`, and the read-only `VfxBudget` and `VfxStats`.
- `#[non_exhaustive]` structs (R6): `PostConfig` (`for_tier`/`Default` + `with_*`), `PostLook`
  (`DEFAULT`/`Default` + `with_*`) and `ScreenEffects` (`NONE`/`Default` + `with_*`); every
  value is clamped by `sanitized`, which the renderer applies.
- `#[non_exhaustive]` structs: `RenderConfig` (`Default` + a `with_*` per field),
  `CapturedFrame`, `VulkanProbe`, `GpuTimings`, `PassTiming`, `PipelineStats` and
  `MaterialStats` (read-only stats), `MaterialData` (`new`, `from_desc`, `with_texture`,
  `with_base_color_factor`, `with_team_tint`; the texture slots are private, read with
  `texture`, so a new `TextureSlot` is not breaking — `MaterialDesc` likewise), and the frame
  inputs that already had
  constructors:
  `Scene` (`new`, `empty`, `with_*`), `UnitInstance` (`new`, `at`, `with_mesh`, `with_bones`),
  `Camera` (`perspective`, `orbit`, R7b `orthographic`), `TextStyle` (`new`/`Default` +
  `with_outline`, `with_shadow`), and R7b's `OffscreenRender` (`new` + `with_terrain`,
  `with_clear_color`, `with_backdrops`).
- Exhaustive by design: `BoneRange`, `MeshId`, `MaterialId`, `TextureId`, `UiImage` (opaque
  ids, checked where used; `UiImage` slots are constants), `UiVertex` (GPU vertex layout; R7a
  renamed `atlas` to `mode` and added `outline`/`params`, 48 bytes; R7b added `image`, the
  offscreen image slot of `MODE_IMAGE`, 52 bytes), `ClearRect` (a closed rectangle + colour),
  `VisionCircles` (uniform layout), `ValidationMode` (a closed policy).
- Public but unstable: `bones_truncated`, `skinned_instances_dropped`,
  `ui_vertices_truncated` (diagnostic counters) and `build_atlas`/`ATLAS_*`.

**`moba-platform`** — `run`, `App`, `Frame`, `FrameOutcome`, `Window`, `WindowConfig`,
`InputState`, `Key`, `MouseButton`, `PlatformError`.
- `#[non_exhaustive]`: `PlatformError`, `Key`, `MouseButton`, `WindowConfig` (`new`/`Default` +
  a `with_*` per field).
- Exhaustive by design: `FrameOutcome` (`Continue`/`Exit`, returned by the application).

**`moba-assets`** — `AssetManifest`/`AssetEntry`, the `.glb` loader (`load_glb`,
`load_glb_file`), `MeshData`, skinning (`Skeleton`, `AnimationClip`, `Track`, `MeshSkin`,
`JointTransform`), map visuals (`MapVisuals`, `MapProp`) and the animation runtime of
ADR-0012 (`Pose`, `BoneMask`, `BlendSpace1d`, `AnimationSet`, `AnimationGraph`, `Animator`,
`AnimationScratch`, `StateId`, `SocketId`), and R1b's textures and materials (ADR-0010:
`TextureData`, `TextureFormat`, `load_ktx2`/`load_ktx2_file`/`write_ktx2`, `MaterialDesc`,
`MaterialShader`, `TextureSlot`, `MaterialReport`, `MeshData::with_uvs`/`uvs`), and R2a's
lighting assets (ADR-0011: `MapLighting`/`MapVisuals::lighting`, the material stylisation
fields, `EnvironmentDesc`/`EnvironmentError`/`environment_paths`/`sh_basis`, cube
`TextureData` (`new_cube`, `is_cube`, `face_count`, `face`), `TextureFormat::Rgba16Float`,
`f16_from_f32`/`f32_from_f16`, `MeshData::with_tangents`/`tangents`/`generate_tangents`/
`with_generated_tangents` and `MeshError::BadTangent`), and R5's VFX
descriptors (ADR-0013: `VfxEffect`, `VfxEmitter`, `VfxBindings`, `VfxAbilityBinding`,
`VfxUnitEffects`, `VfxSprite`/`VFX_SPRITES`, `procedural_atlas`, `vfx_path`; the enums
`VfxPrimitive`, `VfxBlend`, `VfxAnchor`, `VfxDirection`, `VfxShape`, `VfxImportance` and
`VfxError` are `#[non_exhaustive]`, and the structs are validated, built by `parse`/`load_file`),
R4b's additions (ADR-0012 amendment): vertex-animation textures (`VatDescriptor`
(`parse`/`load_file`/`to_toml`/`clip`/`check_texture`/`check_mesh`), `VatClip`,
`VatPlaybackParams` (`validated`), `VatBakeClip::new`, `bake_vat`, `mesh_hash`, `vat_extent`, `vat_paths`,
`VatAnimation::bind`, `VatPlayer`, `VatPose`/`VatSample`/`VatQuality` (`FULL`,
`INTERPOLATED`, `NEAREST`), `VatError` and the `MAX_VAT_*` (including `MAX_VAT_TEXELS`)/`DEFAULT_VAT_FPS`/
`VAT_DEATH_LINGER`/`VAT_DIR` constants), bone-count LOD (`SkinLod::build`/`gather`), 2D blend
spaces (`BlendSpace2d`, `BlendSpaceError::Coincident`, `Animator::evaluate_motion`) and socket
attachments (`AnimationGraph::{attachment, attachments, socket_count, planar_locomotion,
spring_joints}`, `AnimationSet::attachment_models`). `VatError` and every `Vat*` struct with
public fields are `#[non_exhaustive]`; `VatAnimation`, `VatPlayer`, `SkinLod` and
`BlendSpace2d` have private fields and constructors; and R6's post-processing assets
(ADR-0015: `MapPost`/`MapVisuals::post` with the `DEFAULT_BLOOM_STRENGTH`, `DEFAULT_ADAPTATION`, `DEFAULT_EXPOSURE_MIN`/`MAX` defaults, and the
`.cube` grading LUT `GradingLut` (`identity`, `from_texels`, `parse`, `load_file`, `sample`,
`to_cube_string`), `GradingLutError`, `grading_path`, `GRADING_DIR`, `MIN_GRADING_LUT_SIZE`/
`MAX_GRADING_LUT_SIZE`/`MAX_GRADING_LUT_BYTES`; `GradingLut`, `GradingLutError` and `MapPost`
are `#[non_exhaustive]`).
- Added to `scripts/semver-check.sh` in this review: `moba-game` and `moba-render` re-export
  its types, so it carries the Engine-tier promise.
- `#[non_exhaustive]` enums: `AssetError`, `AssetManifestError`, `AssetFileProblem`,
  `AssetList`, `MeshError`, `GlbError`, `SkinError`, `MapVisualsError`, `TrackPath`,
  `Interpolation`, `AnimationSetError`, `AnimationEvent`, `PlayMode`, `Layer`, `ClipWrap`,
  `BlendSpaceError`, `TextureFormat`, `TextureError`, `MaterialShader`, `MaterialError`,
  `TextureSlot`, `EnvironmentError`; structs `AnimatorReport` and `MaterialReport` (read-only),
  `MaterialDesc` (validated, built by `parse`/`load_file`), `MapLighting` (validated with its
  `MapVisuals`; `Default` = no overrides) and `EnvironmentDesc` (`new` + `with_*`, or
  `parse`/`load_file`). `TextureData` has private fields and the
  validating `TextureData::new`. The other animation types have
  private fields and constructors (`Pose::rest`, `BlendSpace1d::new`, `AnimationSet::parse`,
  `AnimationGraph::bind`, `Animator::new`).
- `#[non_exhaustive]` structs: `AssetManifest`, `AssetEntry` (new `AssetEntry::new` +
  `with_fallback`), `AssetFileIssue`, `MapVisuals`, `MapProp`.
- Exhaustive by design: `JointTransform` (translation, rotation, scale).

### Embedding tier

**`moba-engine`** — feature-gated re-exports (`core`, `ecs`, `game`, `nav`, `sim` with `sim`;
`net` with `net`; `platform`, `render` with `render`) and `prelude` (`Fix`, `FixVec2`, `Team`,
`PlayerCommand`, `SimConfig`, `SimEvent`, `Simulation`, `TickInputs`). It adds no types of its
own and follows the Engine tier. No change.
