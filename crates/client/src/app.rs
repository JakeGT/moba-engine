//! Game client: networked play, prediction/interpolation and the local preview.
//!
//! Modes:
//! - default: local bot match rendered as a development preview;
//! - `MOBA_SERVER=<addr>`: connect to a dedicated server;
//! - `MOBA_SERVER=local`: spawn a loopback server in this process (demos/captures).
//!
//! Controls in network mode: right-click to move, left-click an enemy to attack, Q/W/E to
//! cast at the cursor, Space to stop, Escape to quit.
//!
//! Environment:
//! - `MOBA_SMOKE_FRAMES=<n>` exits after `n` frames.
//! - `MOBA_CAPTURE=<file.ppm>` plus `MOBA_CAPTURE_FRAME=<n>` writes a readback image.
//! - `MOBA_VULKAN_VALIDATION=required|off`.
//! - `MOBA_DEMO_MOVE=x,y` issues one move order after the first snapshot (capture demos).
//! - `MOBA_DEMO_BUY=<item>` buys one catalogue item after the first snapshot.
//! - `MOBA_DEMO_CAST=<slot>@<tick>` casts ability slot 0-2 once at that match tick (captures).
//! - `MOBA_TEAM_PALETTE=standard|colorblind` overrides the settings' team colours.
//! - `MOBA_SHOP=1` starts with the shop open; `MOBA_SCOREBOARD=1` with the scoreboard.
//! - `MOBA_ASSET_ROOT=<dir>` (default `content`): heroes whose catalogue names a `model`
//!   draw `<dir>/models/<model>.glb`; `MOBA_HERO_MODELS=<hero=model,...>` overrides that per
//!   hero (dev only). Any failure falls back to the rig. A model with a skin is GPU-skinned;
//!   with `<dir>/animations/<model>.toml` (ADR-0012) the network view animates it from
//!   snapshots and presentation events (locomotion, attacks, casts, death, springs), else its
//!   run clip follows gait. `MOBA_NO_SKINNING=1` draws it static, `MOBA_NO_ANIMATION_SETS=1`
//!   ignores animation sets and `MOBA_ANIM_LOD=off|<near>,<far>` sets the animation LOD.
//!   The map's visuals name resolves to the descriptor `<dir>/maps/<name>.toml` (terrain
//!   tint, walls, props; #287), falling back to the built-in theme.
//! - R4b (ADR-0012): minions play vertex-animation textures from `<dir>/vat/` (`MOBA_VAT=off`
//!   draws cubes); distant or off-screen heroes draw reduced skins (`MOBA_BONE_LOD=off|<near>,
//!   <far>`); animation-set sockets may carry visible attachments (`MOBA_ATTACHMENTS=off`). The
//!   local preview feeds the same animation drivers as a match. `MOBA_STRESS_MINIONS=<n>` adds
//!   `n` client-only VAT minions to the local preview and reports their CPU time at exit.
//! - `MOBA_CONTENT=<pack.toml>` loads a content pack for the local preview and for the
//!   `MOBA_SERVER=local` loopback server.
//! - `MOBA_CAMERA_DISTANCE=<m>` sets the player-camera distance (default 46, 8-120).
//! - `MOBA_LOCAL_CAMERA_DISTANCE=<m>` sets the local bot preview's overview orbit (default
//!   235, 8-240; R4b stress close-ups). Replays follow `MOBA_CAMERA_DISTANCE`.
//! - Lighting overrides of the quality tier's defaults (ADR-0011):
//!   `MOBA_SHADOWS=off|hard|pcf|pcss`, `MOBA_SHADOW_SIZE=<texels>` (256-8192) and
//!   `MOBA_IBL=0` (hemispheric ambient only) / `MOBA_SPECULAR_IBL=0` (SH ambient only). The
//!   map's `[lighting]` table (and its `environment`) sets the art direction.
//! - Post-processing overrides of the tier's defaults (ADR-0015): `MOBA_BLOOM=off|on`,
//!   `MOBA_AA=off|fxaa|msaa4`, `MOBA_AO=off|on`, `MOBA_RENDER_SCALE=0.5..1` (the settings'
//!   `render_scale`), `MOBA_DYNRES=off|on|<target ms>`, `MOBA_UPSCALE=bilinear|bicubic`,
//!   `MOBA_GRADING=off`, `MOBA_EXPOSURE=auto|fixed|<EV>` and `MOBA_SCREEN_EFFECTS=off` (death
//!   desaturation, damage vignette and low-health pulse, also per-effect settings);
//!   `MOBA_SCREEN_EFFECTS_PREVIEW=death|damage|lowhealth` holds one effect for captures. The
//!   map's `[post]` table sets exposure, eye adaptation, bloom strength and the grading LUT.
//!   Captures pin auto exposure and dynamic resolution off unless they are set explicitly.
//! - R2b overrides (ADR-0016): `MOBA_LIGHTS=off|<global cap>` (effect point lights),
//!   `MOBA_OUTLINES=off|hull|screen` (team outlines), `MOBA_BAKED_AO=off` and
//!   `MOBA_CONTACT_SHADOWS=off|<ray-march steps>`.
//! - Terrain, fog of war and atmosphere (R3a, ADR-0014) overrides of the tier's defaults:
//!   `MOBA_TERRAIN=flat|splat` (flat: the R2a plane), `MOBA_FOW=texture|legacy|off` (the
//!   visibility texture, the pre-R3a 16-circle loop, or none), `MOBA_FOW_SIZE=<64..1024>`,
//!   `MOBA_FOG=off` (height fog) and `MOBA_SKY=off`. The map's `[terrain]`, `[atmosphere]`
//!   and `[fog_of_war]` tables set the art direction.
//! - Decals, foliage and water (R3b, ADR-0017) overrides of the tier's defaults:
//!   `MOBA_DECALS=off` (no projected telegraphs, scorch marks or splats; the effects' particles
//!   stay), `MOBA_FOLIAGE=off` and `MOBA_WATER=off|simple|full`. The map's `[foliage]` and
//!   `[[water]]` tables and the effects' `[[decals]]` set the art direction.
//! - `MOBA_PREVIEW_CAMERA=<yaw>,<pitch>,<distance>[,<x>,<z>]` (local preview): a fixed orbit
//!   camera (radians, metres) instead of the slow turn, around an optional focus (metres from
//!   the map centre); the R3a goldens use it for the horizon view, R3b for the river.
//! - `MOBA_CAPTURE_TICK=<n>` (network mode, with `MOBA_CAPTURE`): capture once the match is
//!   Playing at snapshot tick `n`, with interpolation and animation pinned to `n`. A
//!   `MOBA_SERVER=local` server is lock-stepped to the client and stops at `n`, so the frame
//!   is repeatable (with a remote server it is best effort). Projectile effects still advance
//!   by frame time, so pick a tick before the first fight (the golden uses 120).
//! - `MOBA_WINDOW_SIZE=<w>x<h>`: logical window size (default 1280x720).
//! - `MOBA_WARMUP_TICKS=<n>`: the local preview simulates `n` ticks before the first frame;
//!   in network mode, frame logging and `MOBA_SMOKE_FRAMES` start once Playing reaches tick
//!   `n` (a client that never reaches it, e.g. a lobby waiting for ready, does not exit).
//! - `MOBA_PORTRAITS=off|static|animated|<hz>` (network mode): 3D hero portraits in the hero
//!   panel and scoreboard (R7b). Default by quality tier: `low` static (rendered when the
//!   heroes change), `medium` 5 Hz, `high`/`ultra` 10 Hz with each skinned hero's idle clip.
//! - `MOBA_MINIMAP_BAKE=off|once|<frames>` (network mode): the minimap shows a top-down
//!   render of the map baked at every map bind (R7b; default `once`; `<frames>` re-bakes
//!   periodically for GPU-time measurement). `off` keeps the flat minimap.
//! - `MOBA_CAPTURE_MAPS=<prefix>` (network mode, evidence for #287): after every map change
//!   (connect, content reload, map rotation), capture `<prefix>-<n>.ppm` once
//!   `MOBA_CAPTURE_FRAME` frames with snapshots have rendered on it, and keep playing.
//! - VFX (ADR-0013): effects from `<asset root>/vfx/bindings.toml` play on presentation
//!   events (casts, projectiles, hits, zones, deaths, attacks) and attach to animation
//!   sockets. `MOBA_VFX=off` (or the `vfx` setting) turns them off; the `ally_vfx_opacity`
//!   setting (`MOBA_VFX_ALLY_OPACITY`) dims allied effects. `MOBA_VFX_SCALE=<1..16>` (dev)
//!   multiplies every burst and rate, for budget stress tests.
//! - `MOBA_VFX_PREVIEW=<effect>` (authoring): an empty arena that replays `vfx/<effect>.toml`
//!   in a loop and reloads it when a file in `vfx/` changes; no match runs.
//!   `MOBA_OUTLINE_PROBE=hidden|open|hit` (render tests) adds an outlined enemy unit behind a
//!   wall, in the open or under the effect; hidden, the capture must equal one with
//!   `MOBA_OUTLINES=off`.
//! - `MOBA_REPLAY=<file.mbr>`: the local view plays a recorded match (its content pack from
//!   `MOBA_CONTENT`, default content otherwise) instead of a bot match, with the camera
//!   following the heroes (`MOBA_CAMERA_DISTANCE`, default 46). `MOBA_WARMUP_TICKS` skips
//!   ahead; a capture advances one tick per frame. `MOBA_REPLAY_EXIT=1` exits when the
//!   recording ends (the R8 10-minute pacing runs).
//! - R8 (ADR-0018): `MOBA_GPU_CULL=off|cpu` (frustum culling; `off` draws everything),
//!   `MOBA_MESH_LOD=off|<px>` (mesh level selection; the tier's tolerance by default) and
//!   `MOBA_FRAME_LOG_FRAMES=<n>` (frame-log samples, default 10,000). The frame log and the
//!   startup `[vram]` line report memory against the tier's budgets and any upload or
//!   pipeline build after the match started (`locked_uploads`, `locked_pipelines`).

#![cfg_attr(not(test), forbid(unsafe_code))]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use moba_core::Fix;
use moba_core::math::Vec3;
use moba_net::protocol::{MatchPhase, SnapshotState, WireCommand, quantize_position};
use moba_net::{ClientEvent, Interpolator, PredictionState};
use moba_platform::{App, Frame, FrameOutcome, Key, Window};
use moba_render::{
    Camera, OffscreenRender, RenderConfig, Renderer, Scene, UiDrawList, UiImage, UnitInstance,
    ValidationMode,
};
use moba_sim::{SimConfig, Simulation};
use std::collections::VecDeque;

use crate::hud::{HitList, HudFx, build_local_hud, build_network_hud, build_vision};

// Four ticks (133 ms) covers two lost 30 Hz snapshots plus 30 ms of jitter.
const SNAPSHOT_DELAY_TICKS: f32 = 4.0;

mod network_view;

pub(crate) use crate::input::handle_network_input;
pub(crate) use network_view::NetworkView;

/// Snapshot processing only needs a clone when the sequence advances.
fn local_sound(event: &moba_sim::SimEvent) -> Option<crate::audio::Sound> {
    use crate::audio::Sound;
    match event {
        moba_sim::SimEvent::AbilityCast { .. } => Some(Sound::Cast),
        moba_sim::SimEvent::ProjectileSpawned { .. } | moba_sim::SimEvent::ProjectileHit { .. } => {
            Some(Sound::Attack)
        }
        moba_sim::SimEvent::UnitDied { .. } => Some(Sound::Death),
        moba_sim::SimEvent::StructureDestroyed { .. } => Some(Sound::Objective),
        moba_sim::SimEvent::MatchEnded { .. } => Some(Sound::Ui),
        _ => None,
    }
}

fn snapshot_is_new(last_seq: u32, incoming_seq: u32) -> bool {
    incoming_seq != last_seq
}

/// Clones a snapshot only when its sequence advances.
fn snapshot_to_process(last_seq: u32, snapshot: Option<&SnapshotState>) -> Option<SnapshotState> {
    snapshot
        .filter(|snapshot| snapshot_is_new(last_seq, snapshot.seq))
        .cloned()
}

struct ClientApp {
    renderer: Option<Renderer>,
    simulation: Option<Simulation>,
    network: Option<NetworkView>,
    local_audio: Option<crate::audio::Audio>,
    units: Vec<UnitInstance>,
    models: models::ModelLibrary,
    map_visuals: map_visuals::MapLibrary,
    map_capture: Option<map_visuals::SwitchCapture>,
    /// `MOBA_CAPTURE_SERIES` (R4b evidence): captures at listed render ticks.
    capture_series: Option<evidence::CaptureSeries>,
    bones: skinning::BonePool,
    /// Per-hero animators for the network view and the local preview (ADR-0012).
    animation: animation::AnimationDriver,
    /// Effects and their particle spawns (ADR-0013).
    vfx: vfx::VfxSystem,
    /// This frame's units effects can attach to.
    anchors: vfx::Anchors,
    /// This frame's foliage displacers, from the drawn units only (R3b).
    displacers: foliage::Displacers,
    /// `MOBA_REPLAY`: the recorded inputs the local view plays, and the next frame.
    replay: Option<(moba_sim::Replay, usize)>,
    /// `MOBA_REPLAY_EXIT=1`: exit once the replay's last frame has played (R8).
    replay_exit: bool,
    /// `MOBA_VFX_PREVIEW`: the authoring loop.
    preview: Option<vfx::Preview>,
    /// VAT minion players and corpses (R4b).
    minions: minions::MinionDriver,
    /// The loaded VAT minion visuals (R4b).
    minion_library: minions::MinionLibrary,
    /// Bone-count LOD distances (R4b).
    bone_lod: bone_lod::BoneLod,
    /// The local preview's snapshot and events for the animation drivers (R4b).
    local_feed: local_feed::LocalFeed,
    /// `MOBA_STRESS_MINIONS` (R4b, perf/0018).
    stress: Option<stress::StressScene>,
    ui: UiDrawList,
    scoreboard_visible: bool,
    shop_visible: bool,
    hit_rects: HitList,
    /// Retained HUD presentation state: damage ghosts, combat text, screen fades (R7a).
    hud_fx: HudFx,
    /// 3D hero portraits for the HUD (R7b).
    portraits: offscreen::PortraitStudio,
    /// The baked top-down minimap (R7b).
    minimap: offscreen::MinimapBaker,
    /// Gameplay screen effects (R6): death desaturation, damage vignette, low-health pulse.
    screen_fx: screen_fx::ScreenFx,
    /// The settings' render scale (`MOBA_RENDER_SCALE` overrides it at renderer creation).
    render_scale: f32,
    /// The fog-of-war texture's input, rebuilt every frame in reused buffers (R3a).
    fog_input: crate::hud::FogInput,
    frames: u64,
    quit_after_frames: u64,
    capture_path: Option<PathBuf>,
    capture_frame: u64,
    startup_error: Option<String>,
    error: Arc<Mutex<Option<String>>>,
    last_frame: Instant,
    frame_ms: f32,
    fps: f32,
    ui_scale: f32,
    quality: moba_render::QualityTier,
    bindings: crate::settings::Bindings,
    camera_sensitivity: f32,
    /// `MOBA_PREVIEW_CAMERA`: a fixed local-preview orbit `[yaw, pitch, distance, x, z]`.
    preview_camera: Option<[f32; 5]>,
    /// `MOBA_SHADER_DIR` (dev): SPIR-V overrides, hot-reloaded when a `.spv` changes.
    shader_dir: Option<PathBuf>,
    /// Override file count and newest modification time at the last poll (`None` before
    /// the first one), for the hot-reload poll.
    shader_stamp: Option<launch::ShaderStamp>,
    frame_log: Option<Vec<f32>>,
    /// Per-frame `[cpu_ms, render_ms]` while logging: time from frame start to the render
    /// call, and time inside it (fence wait, acquire, recording, submit, present).
    work_log: Option<Vec<[f32; 2]>>,
    /// Per-frame GPU times in [`launch::GPU_LANES`] order (`shadow` … `vfx_resolve`) plus the
    /// particle stats `[alive, overdraw]` while logging, from the renderer's timestamp
    /// queries (empty on devices without timestamp support; `0` for a pass that did not run:
    /// shadows off, a post pass the configuration lacks, or an offscreen pass not re-rendered
    /// that frame). One entry per measured GPU frame: a sample whose frame number was already
    /// logged is skipped.
    gpu_log: Option<Vec<[f32; launch::GPU_LANES.len()]>>,
    /// Frame number of the last logged GPU sample ([`moba_render::GpuTimings::frame`]).
    gpu_logged_frame: Option<u64>,
    frame_log_path: Option<PathBuf>,
    /// Network-mode warm-up (`MOBA_WARMUP_TICKS`): measure only from this match tick on.
    network_warmup: Option<u32>,
    /// Frames counted towards `MOBA_SMOKE_FRAMES` and logged (after any warm-up).
    measured_frames: u64,
    /// Measured frames the frame log had no room for (`MOBA_FRAME_LOG_FRAMES`); the frame
    /// log reports them and `perf-gate` fails a run that dropped any.
    frames_dropped: u64,
}

impl ClientApp {
    #[allow(clippy::too_many_lines)] // Settings/network/bot setup is linear.
    fn new(
        quit_after_frames: u64,
        capture_path: Option<PathBuf>,
        capture_frame: u64,
        error: Arc<Mutex<Option<String>>>,
    ) -> Self {
        let settings_path = crate::settings::Settings::path();
        if std::env::var_os("MOBA_RESET_SETTINGS").is_some() {
            let _ = crate::settings::Settings::default().save(&settings_path);
        }
        let (settings, warnings) = crate::settings::Settings::load(&settings_path);
        let camera_zoom = settings.camera_zoom;
        let vfx_setting = settings.vfx;
        let ally_vfx_opacity = std::env::var("MOBA_VFX_ALLY_OPACITY")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(settings.ally_vfx_opacity);
        let camera_sensitivity = settings.camera_sensitivity;
        let map_capture = map_visuals::SwitchCapture::from_env();
        let screen_fx_preview = std::env::var("MOBA_SCREEN_EFFECTS_PREVIEW")
            .ok()
            .and_then(|name| screen_fx::Preview::from_name(&name));
        // Captures pin the effects off (they run on wall-clock time) unless a preview is set.
        let screen_fx_settings = screen_fx::ScreenFxSettings {
            death_desaturation: settings.death_desaturation,
            damage_vignette: settings.damage_vignette,
            low_health_pulse: settings.low_health_pulse,
        }
        .for_run(
            capture_path.is_some() || map_capture.is_some(),
            screen_fx_preview,
        );
        let audio_volume = settings.audio_volume;
        let audio_mute = settings.audio_mute;
        for warning in warnings {
            eprintln!("{warning}");
        }
        if std::env::var_os("MOBA_SAVE_SETTINGS").is_some()
            && let Err(error) = settings.save(&crate::settings::Settings::path())
        {
            eprintln!("settings: save failed: {error}");
        }
        let (bindings, binding_warnings) = settings.bindings();
        for warning in binding_warnings {
            eprintln!("{warning}");
        }
        let env_scale = std::env::var("MOBA_UI_SCALE")
            .ok()
            .and_then(|value| value.parse::<f32>().ok());
        let frame_log_path = std::env::var("MOBA_FRAME_LOG").ok().map(PathBuf::from);
        // Every log is allocated once here at its full size (R8: no per-frame growth).
        let frame_log_frames = launch::frame_log_frames();
        let frame_log = frame_log_path
            .as_ref()
            .map(|_| Vec::with_capacity(frame_log_frames));
        let work_log = frame_log_path
            .as_ref()
            .map(|_| Vec::with_capacity(frame_log_frames));
        let gpu_log = frame_log_path
            .as_ref()
            .map(|_| Vec::with_capacity(frame_log_frames));
        let quality = std::env::var("MOBA_QUALITY").ok().map_or_else(
            || settings.quality_tier(),
            |name| {
                moba_render::QualityTier::from_name(&name).unwrap_or_else(|| {
                    eprintln!("MOBA_QUALITY={name:?} is not low/medium/high/ultra; using settings");
                    settings.quality_tier()
                })
            },
        );
        let team_palette = std::env::var("MOBA_TEAM_PALETTE").ok().map_or_else(
            || settings.team_palette(),
            |name| {
                crate::hud::theme::TeamPalette::from_name(&name).unwrap_or_else(|| {
                    eprintln!(
                        "MOBA_TEAM_PALETTE={name:?} is not standard/colorblind; using settings"
                    );
                    settings.team_palette()
                })
            },
        );
        let mut app = Self {
            renderer: None,
            simulation: None,
            network: None,
            local_audio: None,
            // Every entity, five extra rig parts per hero, map walls/props and projectiles,
            // so the frame loop never grows the buffer.
            // Plus socket attachments, dying VAT minions and the stress scene (R4b).
            units: Vec::with_capacity(
                moba_sim::MAX_UNITS
                    + 5 * 10
                    + moba_game::MAX_CONTENT_WALLS
                    + moba_assets::MAX_MAP_PROPS
                    + 64
                    + moba_assets::MAX_SOCKETS * 10
                    + 64
                    + stress::MAX_STRESS_MINIONS,
            ),
            models: {
                let mut models = models::ModelLibrary::from_env();
                models.set_bone_lod(bone_lod::BoneLod::from_env(quality).enabled());
                models
            },
            map_visuals: map_visuals::MapLibrary::new(models::asset_root_from_env())
                .with_exposure_override(launch::exposure_override()),
            map_capture,
            capture_series: evidence::CaptureSeries::from_env(),
            bones: skinning::BonePool::new(moba_render::DEFAULT_MAX_BONES),
            animation: animation::AnimationDriver::new(animation::AnimLod::from_env(), 30)
                .with_log(
                    std::env::var("MOBA_SERVER")
                        .map_or(true, |spec| spec.is_empty())
                        .then(evidence::AnimLog::from_env)
                        .flatten(),
                ),
            minions: minions::MinionDriver::new(30),
            minion_library: minions::MinionLibrary::from_env(quality),
            bone_lod: bone_lod::BoneLod::from_env(quality),
            local_feed: local_feed::LocalFeed::new(),
            stress: stress::StressScene::from_env(),
            vfx: {
                let root = models::asset_root_from_env();
                let library = match std::env::var(vfx::VFX_PREVIEW_ENV) {
                    Ok(name) if !name.trim().is_empty() => {
                        vfx::VfxLibrary::preview(root, name.trim())
                    }
                    _ => vfx::VfxLibrary::load(root),
                };
                let mut system = vfx::VfxSystem::new(
                    library,
                    quality.vfx_budget(),
                    vfx::vfx_enabled_from_env(vfx_setting),
                );
                system.set_ally_opacity(ally_vfx_opacity);
                system.set_scale(vfx::vfx_scale_from_env());
                system
            },
            anchors: vfx::Anchors::new(),
            displacers: foliage::Displacers::new(),
            replay: None,
            replay_exit: std::env::var("MOBA_REPLAY_EXIT").is_ok_and(|value| value.trim() == "1"),
            preview: None,
            ui: UiDrawList::new(),
            scoreboard_visible: std::env::var_os("MOBA_SCOREBOARD").is_some(),
            shop_visible: std::env::var_os("MOBA_SHOP").is_some(),
            hit_rects: HitList::new(),
            hud_fx: HudFx::new(team_palette),
            portraits: offscreen::PortraitStudio::new(offscreen::PortraitMode::from_env(quality)),
            minimap: offscreen::MinimapBaker::new(offscreen::MinimapMode::from_env()),
            screen_fx: screen_fx::ScreenFx::new(screen_fx_settings, screen_fx_preview),
            render_scale: settings.render_scale,
            fog_input: crate::hud::FogInput::new(),
            frames: 0,
            quit_after_frames,
            capture_path,
            capture_frame,
            startup_error: None,
            error,
            last_frame: Instant::now(),
            frame_ms: 0.0,
            fps: 0.0,
            ui_scale: env_scale
                .unwrap_or(settings.ui_scale)
                .clamp(crate::settings::UI_SCALE_MIN, crate::settings::UI_SCALE_MAX),
            quality,
            bindings,
            camera_sensitivity,
            preview_camera: launch::parse_preview_camera(
                std::env::var("MOBA_PREVIEW_CAMERA").ok().as_deref(),
            ),
            shader_dir: launch::shader_dir(),
            shader_stamp: None,
            frame_log,
            work_log,
            gpu_log,
            gpu_logged_frame: None,
            frame_log_path,
            // The local preview applies `MOBA_WARMUP_TICKS` before the first frame instead.
            network_warmup: std::env::var("MOBA_SERVER")
                .is_ok_and(|spec| !spec.is_empty())
                .then(|| {
                    std::env::var("MOBA_WARMUP_TICKS")
                        .ok()
                        .and_then(|value| value.parse().ok())
                })
                .flatten(),
            measured_frames: 0,
            frames_dropped: 0,
        };

        match std::env::var("MOBA_SERVER") {
            Ok(spec) if !spec.is_empty() => match connect_network(&spec) {
                Ok(client) => {
                    let slot = client.player_slot();
                    let spectator = client.is_spectator();
                    let hero = usize::from(client.hero_pick()) % client.heroes().len().max(1);
                    let speed = client.heroes().get(hero).map_or(Fix::from_i32(7), |hero| {
                        Fix::from_bits(hero.move_speed_bits)
                    });
                    let prediction = PredictionState::new(speed);
                    let interpolator =
                        Interpolator::new(SNAPSHOT_DELAY_TICKS, f32::from(client.tick_rate()));
                    app.animation = animation::AnimationDriver::new(
                        animation::AnimLod::from_env(),
                        client.tick_rate(),
                    )
                    .with_log(evidence::AnimLog::from_env());
                    app.minions = minions::MinionDriver::new(client.tick_rate());
                    app.network = Some(NetworkView {
                        client,
                        prediction,
                        interpolator,
                        own_slot: slot,
                        own_net_id: None,
                        last_snapshot_seq: 0,
                        phase: MatchPhase::Lobby,
                        countdown: 0,
                        spectator,
                        ready: false,
                        ready_count: 0,
                        player_count: 0,
                        last_tick: 0,
                        demo_move: parse_demo_move(),
                        demo_sent: false,
                        demo_buy: std::env::var("MOBA_DEMO_BUY")
                            .ok()
                            .and_then(|value| value.parse().ok()),
                        demo_bought: false,
                        demo_cast: parse_demo_cast(),
                        demo_orders: launch::parse_demo_orders(),
                        demo_next: 0,
                        own_recall: None,
                        recall_requested: None,
                        shop_page: 0,
                        hover: None,
                        attack_target: None,
                        spectator_pan: Vec3::ZERO,
                        spectator_yaw: -0.7,
                        spectator_zoom: camera_zoom,
                        follow_distance: parse_follow_distance(),
                        overview_camera: std::env::var("MOBA_CAMERA")
                            .is_ok_and(|value| value == "overview"),
                        // Only a capture pins the render tick; otherwise play normally.
                        capture_tick: parse_capture_tick()
                            .filter(|_| std::env::var_os("MOBA_CAPTURE").is_some()),
                        spectator_follow: evidence::spectate_follow_from_env(),
                        kill_feed: VecDeque::with_capacity(6),
                        projectiles: Vec::with_capacity(32),
                        audio: crate::audio::Audio::new(audio_volume, audio_mute),
                    });
                }
                Err(error) => app.startup_error = Some(error),
            },
            _ => {
                let loaded = std::env::var("MOBA_CONTENT").ok().and_then(|path| {
                    match moba_game::load_content_file(std::path::Path::new(&path)) {
                        Ok(content) => Some(content),
                        Err(error) => {
                            eprintln!("MOBA_CONTENT load failed: {error}");
                            None
                        }
                    }
                });
                let mut simulation = loaded.map_or_else(
                    || Simulation::new(SimConfig::default()),
                    |content| {
                        Simulation::new_with_content(
                            SimConfig::default(),
                            std::sync::Arc::new(content),
                        )
                    },
                );
                match launch::load_replay(simulation.content()) {
                    Ok(Some(replay)) => {
                        simulation = Simulation::new_with_content(
                            replay.config,
                            std::sync::Arc::new(simulation.content().clone()),
                        );
                        app.replay = Some((replay, 0));
                    }
                    Ok(None) => {}
                    Err(error) => app.startup_error = Some(error),
                }
                let warmup: u32 = std::env::var("MOBA_WARMUP_TICKS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0);
                for _ in 0..warmup {
                    match app.replay.as_mut() {
                        Some((replay, cursor)) => {
                            if let Some(inputs) = replay.frames.get(*cursor) {
                                simulation.tick(inputs);
                                *cursor += 1;
                            }
                        }
                        None => simulation.tick_with_bots(),
                    }
                    simulation.clear_events();
                }
                simulation.clear_events();
                app.preview = app.vfx.library().preview_effect().map(|effect| {
                    vfx::Preview::new(effect).with_probe(vfx::OutlineProbe::from_env())
                });
                app.simulation = Some(simulation);
            }
        }
        app
    }

    fn fail(&self, message: String) {
        if let Ok(mut slot) = self.error.lock()
            && slot.is_none()
        {
            *slot = Some(message);
        }
    }
}

impl ClientApp {
    /// Dev shader hot reload: with `MOBA_SHADER_DIR` set, checks the overrides' newest
    /// modification time every 30 frames and rebuilds the pipelines when it moves. Without
    /// the variable this is one branch per frame and never touches the file system.
    fn poll_shader_reload(&mut self) {
        let (Some(dir), Some(renderer)) = (self.shader_dir.as_deref(), self.renderer.as_mut())
        else {
            return;
        };
        if !self.frames.is_multiple_of(30) {
            return;
        }
        // The stamp covers the file count too, so deleting an override (even the last one)
        // also reloads, back to the built-in module.
        let stamp = launch::shader_stamp(dir);
        let previous = self.shader_stamp.replace(stamp);
        if previous.is_none_or(|previous| previous == stamp) {
            return; // First poll (the renderer was built from these files) or no change.
        }
        let started = Instant::now();
        // A dev reload rebuilds pipelines on purpose: not a mid-match build (R8).
        renderer.unlock_uploads();
        match renderer.reload_shaders() {
            Ok(_) => eprintln!(
                "[render] shaders reloaded from {} in {:.1} ms",
                dir.display(),
                started.elapsed().as_secs_f32() * 1_000.0
            ),
            Err(error) => {
                eprintln!("[render] shader reload failed ({error}); keeping the old pipelines");
            }
        }
    }
}

impl ClientApp {
    /// `MOBA_VFX_PREVIEW` live reload: every 30 frames, reloads the effects (and rebinds
    /// their meshes) when a file in `<asset root>/vfx` changed.
    fn poll_vfx_reload(&mut self) {
        if self.preview.is_none() || !self.frames.is_multiple_of(30) {
            return;
        }
        if !self.vfx.library().changed_on_disk() {
            return;
        }
        self.vfx.clear();
        self.vfx.library_mut().reload();
        if let Some(renderer) = self.renderer.as_mut() {
            // A dev reload uploads on purpose: not a mid-match upload (R8).
            renderer.unlock_uploads();
            self.vfx.library_mut().bind_renderer(renderer);
        }
        if let Some(preview) = self.preview.as_mut() {
            preview.restart();
        }
        eprintln!("[vfx] preview reloaded");
    }
}

impl App for ClientApp {
    #[allow(clippy::too_many_lines)] // Renderer config, creation and every library's binding.
    fn on_start(&mut self, window: &Window) {
        let validation = match std::env::var("MOBA_VULKAN_VALIDATION").as_deref() {
            Ok("required") => ValidationMode::Required,
            Ok("off") => ValidationMode::Off,
            _ => ValidationMode::IfAvailable,
        };
        let config = RenderConfig::default()
            .with_validation(validation)
            .with_max_instances(1_024)
            .with_vsync(std::env::var("MOBA_VSYNC").map_or(true, |value| value != "0"))
            .with_ui_scale(self.ui_scale)
            .with_quality_tier(self.quality)
            .with_max_bones(moba_render::DEFAULT_MAX_BONES)
            .with_skinning(self.models.skinning())
            .with_pipeline_cache(launch::pipeline_cache_path())
            .with_shader_dir(self.shader_dir.clone())
            .with_pipeline_stats(std::env::var_os("MOBA_PIPELINE_STATS").is_some_and(|v| v == "1"))
            .with_compressed_textures(launch::compressed_textures())
            .with_lighting(Some(launch::lighting_config(self.quality)))
            .with_vfx(self.vfx.enabled())
            .with_distortion(launch::distortion_switch().0)
            .with_distortion_draw(launch::distortion_switch().1)
            .with_post(Some(launch::post_config(
                self.quality,
                self.render_scale,
                self.capture_path.is_some() || self.map_capture.is_some(),
            )))
            .with_world(Some(launch::world_config(self.quality)))
            .with_culling(launch::cull_mode())
            .with_mesh_lod(launch::mesh_lod())
            .with_environment(Some(launch::environment_config(self.quality)));
        // A switched-off offscreen image takes a 1x1 target instead of its full size.
        let config = if self.portraits.mode() == offscreen::PortraitMode::Off {
            config.with_ui_image_size(UiImage::PORTRAITS, [1, 1])
        } else {
            config
        };
        let config = if self.minimap.mode() == offscreen::MinimapMode::Off {
            config.with_ui_image_size(UiImage::MINIMAP, [1, 1])
        } else {
            config
        };
        match Renderer::new(window, config) {
            Ok(mut renderer) => {
                // A new renderer's offscreen images start black.
                self.portraits
                    .configure(renderer.ui_image_extent(UiImage::PORTRAITS));
                self.minimap.invalidate();
                let pipelines = renderer.pipeline_stats();
                eprintln!(
                    "[render] {} pipelines built in {:.1} ms (pipeline cache {}), renderer ready in \
                     {:.1} ms; BC textures {}",
                    pipelines.pipelines,
                    pipelines.build_ms,
                    pipelines.cache,
                    pipelines.startup_ms,
                    if renderer.compressed_textures_supported() {
                        "native"
                    } else {
                        "CPU-decoded"
                    }
                );
                // `on_start` may run again after a resume with a fresh mesh table.
                self.models.renderer_recreated();
                self.map_visuals.renderer_recreated();
                self.vfx.library_mut().renderer_recreated();
                if renderer.vfx_available() {
                    self.vfx.set_budget(renderer.vfx_budget());
                    self.vfx.library_mut().bind_renderer(&mut renderer);
                } else {
                    self.vfx.set_enabled(false);
                }
                self.minion_library.renderer_recreated();
                self.minion_library.bind(|mesh, texture, descriptor| {
                    renderer
                        .register_vat_mesh(mesh, texture, descriptor)
                        .map_err(|error| error.to_string())
                });
                if let Some(network) = &self.network {
                    map_visuals::bind_network_map(
                        &mut self.map_visuals,
                        &mut renderer,
                        network.client.map(),
                    );
                    models::bind_hero_models(
                        &mut self.models,
                        &mut renderer,
                        network
                            .client
                            .heroes()
                            .iter()
                            .map(|hero| (hero.name_str(), hero.model_str())),
                    );
                } else if let Some(simulation) = &self.simulation {
                    map_visuals::bind_local_map(
                        &mut self.map_visuals,
                        &mut renderer,
                        &simulation.content().map,
                    );
                    models::bind_hero_models(
                        &mut self.models,
                        &mut renderer,
                        simulation
                            .content()
                            .heroes
                            .iter()
                            .map(|hero| (hero.name.as_str(), hero.model.as_deref())),
                    );
                }
                let title = if let Some(network) = &self.network {
                    if network.spectator {
                        String::from("MOBA — spectator")
                    } else {
                        format!(
                            "MOBA — online slot {} (right-click move, Q/W/E cast)",
                            network.own_slot
                        )
                    }
                } else {
                    String::from("MOBA — local preview")
                };
                window.set_title(&title);
                // R8: what the content loaded at start costs against the tier's budgets.
                eprintln!(
                    "[vram] {} {}",
                    renderer.quality_tier(),
                    launch::memory_summary(&renderer.memory_stats())
                );
                self.renderer = Some(renderer);
            }
            Err(error) => self.fail(format!("renderer initialisation failed: {error}")),
        }
    }

    #[allow(clippy::too_many_lines)] // The frame is one explicit presentation sequence.
    fn on_frame(&mut self, frame: &mut Frame<'_>) -> FrameOutcome {
        if frame.input().was_key_pressed(Key::M)
            && let Some(network) = self.network.as_mut()
        {
            network.audio.toggle_mute();
        }
        if let Some(network) = self.network.as_mut() {
            network.audio.poll();
        }
        if let Some(audio) = self.local_audio.as_mut() {
            audio.poll();
        }
        self.frames += 1;
        self.poll_shader_reload();
        self.poll_vfx_reload();
        let now = Instant::now();
        let raw_ms = now.duration_since(self.last_frame).as_secs_f32() * 1_000.0;
        self.last_frame = now;
        let measuring = self.network.as_ref().is_none_or(|network| {
            self.network_warmup.is_none_or(|warmup| {
                network.phase == MatchPhase::Playing && network.last_tick >= warmup
            })
        });
        if measuring {
            self.measured_frames += 1;
            if let Some(samples) = self.frame_log.as_mut() {
                if samples.len() < samples.capacity() {
                    samples.push(raw_ms);
                } else {
                    self.frames_dropped += 1;
                }
            }
        }
        self.frame_ms = if self.frame_ms == 0.0 {
            raw_ms
        } else {
            self.frame_ms * 0.9 + raw_ms * 0.1
        };
        self.fps = if self.frame_ms > 0.0 {
            1_000.0 / self.frame_ms
        } else {
            0.0
        };
        if let Some(message) = self.startup_error.take() {
            self.fail(message);
            return FrameOutcome::Exit;
        }
        if self.renderer.is_none() {
            if self.error.lock().is_ok_and(|slot| slot.is_some()) {
                return FrameOutcome::Exit;
            }
            return FrameOutcome::Continue;
        }

        let mut failure: Option<String> = None;
        let mut render_started: Option<Instant> = None;
        let mut exit_after_capture = false;
        let [width, height] = frame.window().size();
        let (frame_ms, fps) = (self.frame_ms, self.fps);
        {
            let ClientApp {
                network,
                simulation,
                units,
                models,
                map_visuals,
                map_capture,
                capture_series,
                bones,
                animation,
                vfx,
                anchors,
                displacers,
                replay,
                preview,
                minions,
                minion_library,
                bone_lod,
                local_feed,
                stress,
                ui,
                scoreboard_visible,
                shop_visible,
                hit_rects,
                hud_fx,
                portraits,
                minimap,
                screen_fx,
                fog_input,
                renderer,
                capture_path,
                capture_frame,
                frames,
                ..
            } = self;
            if let Some(network) = network {
                if let Err(error) = network.client.poll() {
                    failure = Some(format!("receive failed: {error}"));
                }
                if failure.is_none()
                    && let Err(error) = network.client.update()
                {
                    failure = Some(format!("network update failed: {error}"));
                }
                if failure.is_none() {
                    while let Some(event) = network.client.next_event() {
                        match event {
                            ClientEvent::Disconnected => {
                                failure = Some(String::from("server disconnected"));
                                break;
                            }
                            ClientEvent::Connected { slot } => network.own_slot = slot,
                            ClientEvent::GameEvent(event) => {
                                animation.queue_event(&event);
                                queue_network_vfx(vfx, network, &event);
                                minions.queue_event(&event);
                                network.handle_game_event(event);
                            }
                            // The events above belong to this snapshot: stamp them with its
                            // tick for the animators.
                            ClientEvent::Snapshot { tick, .. } => {
                                vfx.stamp(tick);
                                let client = &network.client;
                                // Heroes only: a minion's death must not claim an animator.
                                let pick = |net_id: u16| {
                                    client
                                        .entity(net_id)
                                        .filter(|entity| entity.kind == 0)
                                        .map(|entity| entity.hero_pick)
                                };
                                animation.flush_events(
                                    tick,
                                    |net_id| {
                                        pick(net_id).and_then(|hero| {
                                            animated_rig(models, models.hero(usize::from(hero)))
                                        })
                                    },
                                    |net_id, slot| {
                                        pick(net_id).and_then(|hero| {
                                            animation::ability_name(
                                                client.heroes(),
                                                client.abilities(),
                                                hero,
                                                slot,
                                            )
                                        })
                                    },
                                );
                                minions.flush_events(
                                    tick,
                                    tick as f32 - SNAPSHOT_DELAY_TICKS,
                                    minion_library,
                                );
                            }
                            ClientEvent::ContentUpdated { .. } => {
                                let hero = usize::from(network.client.hero_pick())
                                    % network.client.heroes().len().max(1);
                                let speed = network
                                    .client
                                    .heroes()
                                    .get(hero)
                                    .map_or(Fix::from_i32(7), |hero| {
                                        Fix::from_bits(hero.move_speed_bits)
                                    });
                                network.prediction = PredictionState::new(speed);
                                network.interpolator = Interpolator::new(
                                    SNAPSHOT_DELAY_TICKS,
                                    f32::from(network.client.tick_rate()),
                                );
                                network.own_net_id = None;
                                network.last_snapshot_seq = 0;
                                network.projectiles.clear();
                                network.kill_feed.clear();
                                animation.clear();
                                minions.clear();
                                hud_fx.reset();
                                hud_fx.restart_splash();
                                // Hero models may change; the map bind below re-bakes the
                                // minimap by itself.
                                portraits.invalidate();
                                vfx.clear();
                                // Content reload or map rotation: rebind the map visuals and
                                // hero models (cached, so only new names touch the disk or
                                // the GPU).
                                if let Some(renderer) = renderer.as_mut() {
                                    // A content reload is a load point, not mid-match: the
                                    // lock goes back on with the next measured frame (R8).
                                    renderer.unlock_uploads();
                                    map_visuals::bind_network_map(
                                        map_visuals,
                                        renderer,
                                        network.client.map(),
                                    );
                                    models::bind_hero_models(
                                        models,
                                        renderer,
                                        network
                                            .client
                                            .heroes()
                                            .iter()
                                            .map(|hero| (hero.name_str(), hero.model_str())),
                                    );
                                }
                            }
                            // `Snapshot` is consumed below through `snapshot()`; later
                            // (`#[non_exhaustive]`) events are ignored by this client.
                            _ => {}
                        }
                    }
                }
                if failure.is_none() {
                    if let Some(snapshot) =
                        snapshot_to_process(network.last_snapshot_seq, network.client.snapshot())
                    {
                        network.last_snapshot_seq = snapshot.seq;
                        network.phase =
                            MatchPhase::from_u8(snapshot.phase).unwrap_or(MatchPhase::Lobby);
                        network.countdown = snapshot.countdown;
                        network.ready_count =
                            u8::try_from(snapshot.ready_mask.count_ones()).unwrap_or(u8::MAX);
                        network.player_count =
                            u8::try_from(snapshot.player_mask.count_ones()).unwrap_or(u8::MAX);
                        network.ready = network.own_slot < 10
                            && snapshot.ready_mask & (1 << network.own_slot) != 0;
                        if snapshot.tick < network.last_tick {
                            // Rematch: server restarted at tick zero with fresh net ids.
                            network.interpolator.clear();
                            animation.clear();
                            vfx.clear();
                            minions.clear();
                            network.own_net_id = None;
                            network.prediction.set_target(None);
                            network.client.clear_pending_input();
                            hud_fx.reset();
                            screen_fx.reset();
                        }
                        network.last_tick = snapshot.tick;
                        if network.capture_tick.is_some() {
                            // Lets a lock-stepped local capture server run the next tick.
                            launch::LOCAL_CAPTURE_ACK
                                .store(snapshot.tick, std::sync::atomic::Ordering::Relaxed);
                        }
                        network.interpolator.push(snapshot.clone());
                        animation.on_snapshot(&snapshot, |hero| {
                            animated_rig(models, models.hero(usize::from(hero)))
                        });
                        minions.on_snapshot(&snapshot);
                        sync_network_recalls(vfx, network, &snapshot);
                        if let Some(own) = snapshot.entities.iter().find(|entity| {
                            entity.hero_slot == network.own_slot
                                && entity.team == network.client.team().index() as u8
                        }) {
                            network.own_net_id = Some(own.net_id);
                            network.recall_confirmed(own.is_recalling(), snapshot.tick);
                            network.prediction.set_dead(own.is_dead());
                            screen_fx.observe(own.health, own.max_health, own.is_dead());
                            network.prediction.reconcile(own.position());
                            if let Some(item) = network.demo_buy.filter(|_| {
                                !network.demo_bought && network.phase == MatchPhase::Playing
                            }) && network
                                .client
                                .send_command(WireCommand::BuyItem { item })
                                .is_ok()
                            {
                                network.demo_bought = true;
                            }
                            if let Some((slot, _)) = network.demo_cast.filter(|(_, tick)| {
                                network.phase == MatchPhase::Playing && snapshot.tick >= *tick
                            }) {
                                let (x, y) = quantize_position(
                                    own.position() + moba_core::FixVec2::from_ints(4, 4),
                                );
                                if network
                                    .client
                                    .send_command(WireCommand::CastAbility { slot, x, y })
                                    .is_ok()
                                {
                                    network.demo_cast = None;
                                }
                            }
                            while network.phase == MatchPhase::Playing
                                && let Some(&(tick, order)) =
                                    network.demo_orders.get(network.demo_next)
                                && snapshot.tick >= tick
                            {
                                network.demo_next += 1;
                                send_demo_order(network, order, own.position());
                            }
                            if let Some(target) = network.demo_move.filter(|_| {
                                !network.demo_sent && network.phase == MatchPhase::Playing
                            }) {
                                let (x, y) = quantize_position(target);
                                if network
                                    .client
                                    .send_command(WireCommand::MoveTo { x, y })
                                    .is_ok()
                                {
                                    network.prediction.set_target(Some(target));
                                    network.demo_sent = true;
                                }
                            }
                        }
                        hud_fx.observe(
                            &snapshot,
                            network.own_net_id,
                            network.client.team().index() as u8,
                        );
                    }
                    if network.phase == MatchPhase::Playing {
                        for _ in 0..frame.ticks() {
                            network.prediction.step();
                        }
                    }
                    if network.spectator
                        && let Some((team, slot)) = network.spectator_follow
                    {
                        let followed = network.interpolator.latest().and_then(|latest| {
                            latest
                                .entities
                                .iter()
                                .find(|entity| {
                                    entity.kind == 0
                                        && entity.team == team
                                        && entity.hero_slot == slot
                                })
                                .copied()
                        });
                        if let Some(hero) = followed {
                            let position = crate::hud::rendered_entity_position(
                                network,
                                crate::hud::HudUnit::new(&hero),
                                network.render_tick(),
                            );
                            let center = crate::hud::map_center(network);
                            network.spectator_pan = Vec3::new(
                                position.x.to_f32() - center,
                                0.0,
                                position.y.to_f32() - center,
                            );
                            network.spectator_zoom = network.follow_distance;
                        }
                    }
                    let lod_view = (width > 0 && height > 0).then(|| {
                        let camera = network_camera(
                            network,
                            width as f32 / height as f32,
                            frame.elapsed_secs() as f32,
                        );
                        animation::LodView {
                            eye: camera.position,
                            view_proj: camera.view_proj,
                        }
                    });
                    rebuild_network_units(
                        network,
                        models,
                        map_visuals,
                        units,
                        bones,
                        scene_build::AnimationFrame {
                            driver: animation,
                            view: lod_view,
                            minions,
                            minion_library,
                            bone_lod: *bone_lod,
                            far_attachments: self.quality != moba_render::QualityTier::Low,
                        },
                        anchors,
                        !vfx.enabled(),
                    );
                    if width > 0 && height > 0 {
                        let aspect = width as f32 / height as f32;
                        let mut camera =
                            network_camera(network, aspect, frame.elapsed_secs() as f32);
                        map_visuals.light_camera(&mut camera);
                        let ui_input = frame.input().scaled_cursor(self.ui_scale);
                        handle_network_input(
                            network,
                            frame.input(),
                            &ui_input,
                            &camera,
                            width,
                            height,
                            shop_visible,
                            hit_rects,
                            &self.bindings,
                            self.camera_sensitivity,
                        );
                        // Used by the next frame's outlines (the units are already built).
                        network.hover = crate::input::hovered_unit(
                            network,
                            frame.input(),
                            &camera,
                            width,
                            height,
                        );
                        if frame.input().was_key_pressed(self.bindings.scoreboard) {
                            *scoreboard_visible = !*scoreboard_visible;
                        }
                        // R7b: decide the offscreen renders first, so the HUD only draws
                        // what the images will hold after this frame.
                        let portraits_due = portraits.prepare(network, models);
                        let minimap_due =
                            minimap.prepare(map_visuals, crate::hud::map_extent(network));
                        portraits.hud(&mut hud_fx.images);
                        hud_fx.images.minimap = minimap.ready(map_visuals.generation());
                        hud_fx.images.fog = renderer.as_ref().map_or_else(
                            crate::hud::images::MinimapFog::default,
                            |renderer| {
                                crate::hud::images::MinimapFog::from_mode(
                                    renderer.world_config().fog_of_war,
                                )
                            },
                        );
                        build_network_hud(
                            ui,
                            network,
                            *scoreboard_visible,
                            *shop_visible,
                            &ui_input,
                            &camera,
                            (width as f32 / self.ui_scale) as u32,
                            (height as f32 / self.ui_scale) as u32,
                            frame_ms,
                            fps,
                            hit_rects,
                            hud_fx,
                        );
                        network.update_effects(frame_ms);
                        // A pinned capture freezes effects with the view.
                        let vfx_dt = if network.pinned_tick().is_some() {
                            0.0
                        } else {
                            frame_ms / 1_000.0
                        };
                        let view = vfx::VfxView::new(
                            &camera,
                            [width as f32, height as f32],
                            (!network.spectator)
                                .then(|| u8::try_from(network.client.team().index()).unwrap_or(0)),
                            network.own_net_id.map(u32::from),
                        );
                        let render_tick = network.render_tick();
                        let socket = |unit: u32, name: &str| {
                            network_socket(anchors, models, animation, unit, name)
                        };
                        vfx.sync_ambient(map_visuals.generation(), map_visuals.ambient_effects());
                        vfx.update(vfx_dt, render_tick, &view, anchors, &socket);
                        vfx::push_placeholders(vfx, units);
                        let vision = build_vision(network);
                        // A pinned capture (`MOBA_CAPTURE_TICK`) settles the fog in one frame,
                        // so the image never depends on how many frames led up to the tick.
                        let fog_dt = if network.capture_tick.is_some() {
                            moba_render::FOG_OF_WAR_SETTLE_DT
                        } else {
                            frame_ms / 1_000.0
                        };
                        let fog = fog_input.build(network, fog_dt);
                        // R3b: only the drawn units bend the foliage; wind and water follow
                        // the render tick, so a pinned capture is repeatable.
                        displacers.build(anchors, view.focus);
                        let environment = moba_render::EnvironmentFrame::new(
                            render_tick / 30.0,
                            displacers.as_slice(),
                        );
                        let mut scene = Scene::with_vision(camera, units, vision)
                            .with_terrain_tint(map_visuals.terrain_tint())
                            .with_lighting(map_visuals.scene_lighting())
                            .with_atmosphere(map_visuals.scene_atmosphere())
                            .with_bones(bones.bones())
                            .with_vfx(vfx.frame())
                            .with_lights(vfx.lights())
                            .with_outlines(outlines::palette(hud_fx.palette))
                            .with_decals(vfx.decals())
                            .with_environment(environment)
                            .with_effects(screen_fx.update(frame_ms / 1_000.0));
                        if let Some(fog) = fog {
                            scene = scene.with_fog_of_war(fog);
                        }
                        // With `MOBA_CAPTURE_TICK` the capture waits for the match tick, not
                        // a frame count, so it does not depend on frame pacing.
                        let capture_due = if network.capture_tick.is_some() {
                            capture_path.is_some() && network.pinned_tick().is_some()
                        } else {
                            frames_is_capture(*frames, *capture_frame, capture_path.as_deref())
                        };
                        let switch_capture = map_capture
                            .as_mut()
                            .filter(|_| network.interpolator.latest().is_some())
                            .and_then(|capture| {
                                capture.due(map_visuals.generation(), *capture_frame)
                            })
                            .or_else(|| {
                                capture_series
                                    .as_mut()
                                    .filter(|_| network.phase == MatchPhase::Playing)
                                    .and_then(|series| series.due(network.render_tick()))
                            });
                        let mut offscreen: arrayvec::ArrayVec<OffscreenRender<'_>, 2> =
                            arrayvec::ArrayVec::new();
                        if portraits_due {
                            offscreen.push(portraits.render());
                        }
                        if minimap_due {
                            offscreen.push(minimap.render());
                        }
                        let mut submitted = false;
                        if let Some(renderer) = renderer.as_mut() {
                            if capture_due {
                                submitted = capture_and_exit(
                                    renderer,
                                    &scene,
                                    ui,
                                    &offscreen,
                                    capture_path.as_deref(),
                                    &mut failure,
                                );
                                exit_after_capture = true;
                            } else if let Some(path) = switch_capture {
                                // Mid-session: a failed capture (minimised window, resize,
                                // validation message) is reported, not fatal.
                                let mut capture_failure = None;
                                submitted = capture_and_exit(
                                    renderer,
                                    &scene,
                                    ui,
                                    &offscreen,
                                    Some(&path),
                                    &mut capture_failure,
                                );
                                match capture_failure {
                                    None => eprintln!("[capture] written to {}", path.display()),
                                    Some(error) => {
                                        eprintln!("[capture] {} failed: {error}", path.display());
                                    }
                                }
                            } else {
                                render_started = Some(Instant::now());
                                match renderer.render_with_offscreen(&scene, ui, &offscreen) {
                                    Ok(done) => submitted = done,
                                    Err(error) => {
                                        // Presenting can fail after the submit.
                                        submitted = renderer.last_frame_submitted();
                                        failure = Some(format!("render failed: {error}"));
                                    }
                                }
                            }
                        }
                        drop(offscreen);
                        portraits.finish(submitted);
                        minimap.finish(submitted);
                    }
                }
            } else if let Some(simulation) = simulation {
                // Golden captures must not depend on wall-clock frame pacing.
                let ticks = if capture_path.is_some() {
                    1
                } else {
                    frame.ticks()
                };
                let center = simulation.content().map.extent.to_f32() * 0.5;
                for _ in 0..ticks {
                    if preview.is_some() {
                        break;
                    }
                    match replay.as_mut() {
                        Some((recording, cursor)) => {
                            if let Some(inputs) = recording.frames.get(*cursor) {
                                simulation.tick(inputs);
                                *cursor += 1;
                            }
                        }
                        None => simulation.tick_with_bots(),
                    }
                    if let Some(audio) = self.local_audio.as_mut() {
                        for event in simulation.events() {
                            if let Some(sound) = local_sound(event) {
                                audio.play(sound);
                            }
                        }
                    }
                    queue_local_vfx(vfx, simulation, center);
                    // The same animation inputs a match delivers (R4b): this tick's snapshot
                    // and events, flushed and applied in the network order.
                    local_feed.capture(simulation);
                    for event in local_feed.events() {
                        animation.queue_event(event);
                        minions.queue_event(event);
                    }
                    let tick = local_feed.snapshot().tick;
                    let content = simulation.content();
                    let feed = &*local_feed;
                    animation.flush_events(
                        tick,
                        |net_id| {
                            feed.hero_pick(net_id).and_then(|hero| {
                                animated_rig(models, models.hero(usize::from(hero)))
                            })
                        },
                        |net_id, slot| {
                            feed.hero_pick(net_id)
                                .and_then(|hero| local_ability_name(content, hero, slot))
                        },
                    );
                    minions.flush_events(tick, tick as f32, minion_library);
                    animation.on_snapshot(feed.snapshot(), |hero| {
                        animated_rig(models, models.hero(usize::from(hero)))
                    });
                    minions.on_snapshot(feed.snapshot());
                    simulation.clear_events();
                }
                units.clear();
                bones.clear();
                anchors.clear();
                // The same walls (and descriptor props) the network path draws.
                map_visuals.push_instances(units);
                let aspect = if height > 0 {
                    width as f32 / height as f32
                } else {
                    1.0
                };
                let yaw = if capture_path.is_some() || replay.is_some() {
                    -0.7
                } else {
                    frame.elapsed_secs() as f32 * 0.035
                };
                let local_camera = if let Some([yaw, pitch, distance, x, z]) = self.preview_camera {
                    Camera::orbit(Vec3::new(x, 0.0, z), distance, yaw, pitch, aspect)
                } else if preview.is_some() {
                    Camera::orbit(Vec3::ZERO, 30.0, -0.7, 0.75, aspect)
                } else if replay.is_some() {
                    let (focus, distance) = launch::replay_focus(simulation, center);
                    Camera::orbit(focus, distance, yaw, 0.95, aspect)
                } else {
                    Camera::orbit(
                        Vec3::ZERO,
                        launch::local_camera_distance(),
                        yaw,
                        0.92,
                        aspect,
                    )
                };
                let pinned = capture_path.is_some();
                // The VFX authoring preview draws only its own stage.
                if preview.is_none() {
                    rebuild_local_units(
                        local_feed,
                        models,
                        units,
                        bones,
                        scene_build::AnimationFrame {
                            driver: animation,
                            view: (width > 0 && height > 0).then_some(animation::LodView {
                                eye: local_camera.position,
                                view_proj: local_camera.view_proj,
                            }),
                            minions,
                            minion_library,
                            bone_lod: *bone_lod,
                            far_attachments: self.quality != moba_render::QualityTier::Low,
                        },
                        anchors,
                        center,
                        30.0,
                        pinned,
                    );
                }
                if let Some(stress) = stress.as_mut() {
                    stress.push(
                        units,
                        minion_library,
                        local_feed.snapshot().tick as f32 / 30.0,
                    );
                }
                if let Some(preview) = preview.as_mut() {
                    // Captures replay on the fixed capture tick, so they are reproducible.
                    let preview_ms = if capture_path.is_some() {
                        1_000.0 / 30.0
                    } else {
                        frame_ms
                    };
                    preview.frame(vfx, anchors, units, preview_ms);
                }
                if width > 0 && height > 0 {
                    let hud_mode = if preview.is_some() {
                        crate::hud::LocalHudMode::VfxPreview
                    } else if replay.is_some() {
                        crate::hud::LocalHudMode::Replay
                    } else {
                        crate::hud::LocalHudMode::BotPreview
                    };
                    build_local_hud(
                        ui,
                        simulation,
                        hud_mode,
                        (width as f32 / self.ui_scale) as u32,
                        (height as f32 / self.ui_scale) as u32,
                        frame_ms,
                        fps,
                        hit_rects,
                    );
                    let mut camera = local_camera;
                    map_visuals.light_camera(&mut camera);
                    // Captures advance one tick per frame, so effects step a tick too.
                    let vfx_dt = if capture_path.is_some() {
                        1.0 / 30.0
                    } else {
                        frame_ms / 1_000.0
                    };
                    let view =
                        vfx::VfxView::new(&camera, [width as f32, height as f32], Some(0), None);
                    let render_tick = simulation.current_tick().get() as f32;
                    let socket = |unit: u32, name: &str| {
                        network_socket(anchors, models, animation, unit, name)
                    };
                    vfx.sync_ambient(map_visuals.generation(), map_visuals.ambient_effects());
                    vfx.update(vfx_dt, render_tick, &view, anchors, &socket);
                    vfx::push_placeholders(vfx, units);
                    // The local preview has no player hero: only a preview effect applies.
                    displacers.build(anchors, view.focus);
                    let environment = moba_render::EnvironmentFrame::new(
                        render_tick / 30.0,
                        displacers.as_slice(),
                    );
                    let scene = Scene::new(camera, units)
                        .with_terrain_tint(map_visuals.terrain_tint())
                        .with_lighting(map_visuals.scene_lighting())
                        .with_atmosphere(map_visuals.scene_atmosphere())
                        .with_bones(bones.bones())
                        .with_vfx(vfx.frame())
                        .with_lights(vfx.lights())
                        .with_outlines(outlines::palette(self.hud_fx.palette))
                        .with_decals(vfx.decals())
                        .with_environment(environment)
                        .with_effects(screen_fx.update(frame_ms / 1_000.0));
                    if let Some(renderer) = renderer.as_mut() {
                        if frames_is_capture(*frames, *capture_frame, capture_path.as_deref()) {
                            capture_and_exit(
                                renderer,
                                &scene,
                                ui,
                                &[],
                                capture_path.as_deref(),
                                &mut failure,
                            );
                            exit_after_capture = true;
                        } else {
                            render_started = Some(Instant::now());
                            if let Err(error) = renderer.render_with_ui(&scene, ui) {
                                failure = Some(format!("render failed: {error}"));
                            }
                        }
                    }
                }
            }
        }

        // R8 (ADR-0018 §4): every asset is loaded before the match plays; from the first
        // measured frame of a playing match on, any upload or pipeline build is counted and
        // logged. Leaving the match (or a content or dev reload) unlocks; the next measured
        // frame of a playing match locks again.
        let lock_wanted = launch::upload_lock_wanted(
            measuring,
            self.network.as_ref().map(|network| network.phase),
        );
        if let Some(renderer) = self.renderer.as_mut()
            && renderer.uploads_locked() != lock_wanted
        {
            if lock_wanted {
                renderer.lock_uploads();
            } else if self.network.is_some() {
                renderer.unlock_uploads();
            }
        }
        if measuring
            && let (Some(started), Some(work)) = (render_started, self.work_log.as_mut())
            && work.len() < work.capacity()
        {
            let cpu_ms = started.duration_since(now).as_secs_f32() * 1_000.0;
            let render_ms = started.elapsed().as_secs_f32() * 1_000.0;
            work.push([cpu_ms, render_ms]);
            if let (Some(gpu), Some(timings)) = (
                self.gpu_log.as_mut(),
                self.renderer
                    .as_ref()
                    .and_then(moba_render::Renderer::gpu_timings),
            ) && gpu.len() < gpu.capacity()
                && self.gpu_logged_frame != Some(timings.frame)
            {
                // Only fresh samples: when no newer frame completed since the last call, the
                // renderer returns the same timings again.
                self.gpu_logged_frame = Some(timings.frame);
                let stats = self
                    .renderer
                    .as_ref()
                    .and_then(moba_render::Renderer::vfx_stats)
                    .filter(|stats| stats.frame == timings.frame);
                let lights = self
                    .renderer
                    .as_ref()
                    .map_or(0.0, |renderer| renderer.light_stats().binned as f32);
                gpu.push(launch::gpu_sample(&timings, stats, lights));
            }
        }
        if let Some(message) = failure {
            self.fail(message);
            return FrameOutcome::Exit;
        }
        if exit_after_capture {
            return FrameOutcome::Exit;
        }
        if frame.input().was_key_pressed(Key::Escape)
            || self.measured_frames >= self.quit_after_frames
            || (self.replay_exit
                && self
                    .replay
                    .as_ref()
                    .is_some_and(|(replay, cursor)| *cursor >= replay.frames.len()))
        {
            return FrameOutcome::Exit;
        }
        FrameOutcome::Continue
    }
}

mod animation;
mod bone_lod;
mod evidence;
mod foliage;
mod launch;
mod local_feed;
mod map_visuals;
mod materials;
mod minions;
mod models;
mod offscreen;
mod outlines;
mod scene_build;
mod screen_fx;
mod skinning;
mod stress;
mod vfx;

pub(crate) use launch::{
    capture_and_exit, connect_network, frames_is_capture, parse_capture_tick, parse_demo_cast,
    parse_demo_move, parse_follow_distance, run,
};
pub(crate) use scene_build::{
    animated_rig, network_camera, rebuild_local_units, rebuild_network_units,
};

/// Sends one scripted `MOBA_DEMO_ORDERS` order for the own hero standing at `own`.
fn send_demo_order(network: &mut NetworkView, order: launch::DemoOrder, own: moba_core::FixVec2) {
    match order {
        launch::DemoOrder::Recall => crate::input::request_recall(network),
        launch::DemoOrder::Stop => {
            if network.client.send_command(WireCommand::Stop).is_ok() {
                network.prediction.set_target(None);
            }
        }
        launch::DemoOrder::Cast(slot) => {
            let (x, y) = quantize_position(own + moba_core::FixVec2::from_ints(4, 4));
            let _ = network
                .client
                .send_command(WireCommand::CastAbility { slot, x, y });
        }
        launch::DemoOrder::Attack => {
            let team = network.client.team().index() as u8;
            let nearest = network.client.snapshot().and_then(|snapshot| {
                snapshot
                    .entities
                    .iter()
                    .filter(|entity| entity.team != team && entity.team <= 1 && !entity.is_dead())
                    .min_by_key(|entity| entity.position().distance(own).to_bits())
                    .map(|entity| entity.net_id)
            });
            if let Some(net_id) = nearest
                && network
                    .client
                    .send_command(WireCommand::Attack { net_id })
                    .is_ok()
            {
                network.attack_target = Some(net_id);
            }
        }
        launch::DemoOrder::Move(target) => {
            let (x, y) = quantize_position(target);
            if network
                .client
                .send_command(WireCommand::MoveTo { x, y })
                .is_ok()
            {
                network.prediction.set_target(Some(target));
            }
        }
    }
}

/// Queues a replicated event's effects until its snapshot's tick is known.
fn queue_network_vfx(
    vfx: &mut vfx::VfxSystem,
    network: &NetworkView,
    event: &moba_net::protocol::NetEvent,
) {
    if !vfx.enabled() {
        return;
    }
    let client = &network.client;
    let center = crate::hud::map_center(network);
    let tick_rate = f32::from(client.tick_rate().max(1));
    let library = vfx.library();
    let mapped = vfx::from_net(event, center, tick_rate, |source, slot| {
        let hero = client
            .entity(source)
            .filter(|entity| entity.kind == 0)?
            .hero_pick;
        animation::ability_name(client.heroes(), client.abilities(), hero, slot)
            .and_then(|name| library.ability(name))
    });
    if let Some(mapped) = mapped {
        vfx.queue_unstamped(mapped);
    }
}

/// Starts and stops the recall effects from the snapshot's recall flags, behind the
/// unreliable recall events ([`vfx::VfxSystem::sync_recalls`]).
fn sync_network_recalls(vfx: &mut vfx::VfxSystem, network: &NetworkView, snapshot: &SnapshotState) {
    let center = crate::hud::map_center(network);
    let rules = network.client.rules();
    let seconds = rules.recall_ticks as f32 / f32::from(network.client.tick_rate().max(1));
    vfx.sync_recalls(
        snapshot.tick,
        snapshot
            .entities
            .iter()
            .filter(|entity| entity.kind == 0 && entity.is_recalling() && !entity.is_dead())
            .map(|entity| {
                vfx::VfxEvent::recall(
                    u32::from(entity.net_id),
                    entity.team,
                    vfx::world(entity.position(), center),
                    seconds,
                )
            }),
    );
}

/// Queues this tick's local simulation events for the effects (the preview and replays).
fn queue_local_vfx(vfx: &mut vfx::VfxSystem, simulation: &Simulation, center: f32) {
    if !vfx.enabled() {
        return;
    }
    let tick = simulation.current_tick().get();
    let content = simulation.content();
    let world = simulation.world();
    for event in simulation.events() {
        let library = vfx.library();
        let mapped = vfx::from_sim(
            event,
            center,
            30.0,
            // The anchors use the local feed's net ids (R4b), as the animators do.
            |entity| u32::from(local_feed::net_id(entity.index())),
            |caster, slot| {
                let hero = world.get::<moba_sim::HeroIdentity>(caster)?.0.0;
                let ability = *content
                    .heroes
                    .get(usize::from(hero))?
                    .abilities
                    .get(usize::from(slot))?;
                let name = &content.abilities.get(usize::from(ability))?.name;
                library.ability(name)
            },
        );
        if let Some(mapped) = mapped {
            vfx.queue(tick, mapped);
        }
    }
}

/// A hero's socket in world space: its animator's last evaluation, placed by the frame's
/// model-to-world transform.
fn network_socket(
    anchors: &vfx::Anchors,
    models: &models::ModelLibrary,
    animation: &animation::AnimationDriver,
    unit: u32,
    name: &str,
) -> Option<Vec3> {
    let (rig, model_to_world) = anchors.get(unit)?.model?;
    let socket = models.graph(rig)?.socket(name)?;
    let local = animation
        .animator(u16::try_from(unit).ok()?)?
        .socket_transform(socket)?;
    Some((model_to_world * local).w_axis.truncate())
}

/// The catalogue ability name of a local hero's cast slot (as the wire catalogue gives a
/// network client).
fn local_ability_name(content: &moba_game::GameContent, hero: u8, slot: u8) -> Option<&str> {
    let hero = content.heroes.get(usize::from(hero))?;
    let ability = *hero.abilities.get(usize::from(slot))?;
    content
        .abilities
        .get(usize::from(ability))
        .map(|ability| ability.name.as_str())
        .filter(|name| !name.is_empty())
}

#[allow(clippy::too_many_lines)] // One HUD layout, kept together for review.
#[cfg(test)]
pub(crate) mod allocation_probe {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static ENABLED: Cell<bool> = const { Cell::new(false) };
        static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    }

    struct CountingAllocator;

    // SAFETY: every method forwards exactly to the system allocator with unchanged arguments.
    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            record();
            // SAFETY: forwarding the same layout to the system allocator.
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            record();
            // SAFETY: forwarding the same layout to the system allocator.
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            record();
            // SAFETY: `ptr`/`layout` came from this allocator; forwarding unchanged.
            unsafe { System.realloc(ptr, layout, size) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: `ptr`/`layout` came from this allocator; forwarding unchanged.
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    fn record() {
        if ENABLED.try_with(Cell::get).unwrap_or(false) {
            ALLOCATIONS.with(|count| count.set(count.get().saturating_add(1)));
        }
    }

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;

    /// Runs `f` while counting allocations made on the calling thread.
    pub(crate) fn probe<R>(f: impl FnOnce() -> R) -> (R, u64) {
        // Force both TLS slots to initialise before counting: Windows lazily allocates
        // thread-local storage on first access.
        ENABLED.with(|_| {});
        ALLOCATIONS.with(|count| count.set(0));
        ENABLED.with(|enabled| enabled.set(true));
        let value = f();
        ENABLED.with(|enabled| enabled.set(false));
        (value, ALLOCATIONS.with(Cell::get))
    }
}

#[cfg(test)]
mod tests {
    use super::{Camera, Fix, MatchPhase, Vec3};
    use crate::app::scene_build::{kind_from_u8, team_from_u8};
    use crate::hud::HudAction;
    use crate::input::{InputFocus, cursor_ground, ground_orders_allowed};
    use moba_core::FixVec2;
    use moba_core::math::Vec2;
    use moba_game::Team;
    use moba_sim::UnitKind;

    #[test]
    fn shop_focus_routes_to_shop() {
        assert_eq!(InputFocus::from_shop(true), InputFocus::Shop);
        assert_eq!(InputFocus::from_shop(false), InputFocus::Gameplay);
    }

    #[test]
    fn centre_cursor_maps_to_camera_target() {
        let camera = Camera::orbit(Vec3::ZERO, 46.0, -0.7, 0.95, 16.0 / 9.0);
        let ground = cursor_ground(&camera, Vec2::new(640.0, 360.0), 1280, 720, 128.0)
            .expect("centre ray hits the ground");
        assert!(
            ground.distance(FixVec2::from_ints(128, 128)) < Fix::from_f64(0.25),
            "unexpected ground point {ground:?}"
        );
    }

    #[test]
    fn wire_kinds_round_trip() {
        for kind in [
            UnitKind::Hero,
            UnitKind::MeleeMinion,
            UnitKind::RangedMinion,
            UnitKind::Tower,
            UnitKind::Nexus,
        ] {
            assert_eq!(kind_from_u8(kind as u8), Some(kind));
        }
        assert_eq!(kind_from_u8(99), None);
        assert_eq!(team_from_u8(0), Some(Team::Blue));
        assert_eq!(team_from_u8(1), Some(Team::Red));
        assert_eq!(team_from_u8(2), None);
    }

    #[test]
    fn local_sim_events_map_to_audio() {
        assert_eq!(
            super::local_sound(&moba_sim::SimEvent::MatchEnded {
                winner: Some(Team::Blue)
            }),
            Some(crate::audio::Sound::Ui)
        );
    }

    #[test]
    fn snapshots_clone_only_when_the_sequence_advances() {
        assert!(!super::snapshot_is_new(7, 7));
        assert!(super::snapshot_is_new(7, 8));
        assert!(super::snapshot_is_new(8, 7));
    }

    #[test]
    fn ui_actions_block_ground_orders() {
        assert!(!ground_orders_allowed(
            Some(HudAction::Block),
            MatchPhase::Playing,
            InputFocus::Gameplay
        ));
        assert!(!ground_orders_allowed(
            None,
            MatchPhase::Playing,
            InputFocus::Shop
        ));
        assert!(ground_orders_allowed(
            None,
            MatchPhase::Playing,
            InputFocus::Gameplay
        ));
    }

    #[test]
    fn unchanged_snapshots_do_not_allocate() {
        let snapshot = moba_net::protocol::SnapshotState::default();
        let ((), allocations) = super::allocation_probe::probe(|| {
            for _ in 0..100 {
                let _ = super::snapshot_to_process(0, Some(&snapshot));
            }
        });
        assert_eq!(allocations, 0);
    }
}
