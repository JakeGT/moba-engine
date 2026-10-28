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
//! - `MOBA_SHOP=1` starts with the shop open; `MOBA_SCOREBOARD=1` with the scoreboard.
//! - `MOBA_ASSET_ROOT=<dir>` (default `content`): heroes whose catalogue names a `model`
//!   draw `<dir>/models/<model>.glb`; `MOBA_HERO_MODELS=<hero=model,...>` overrides that per
//!   hero (dev only). Any failure falls back to the rig. A model with a skin is GPU-skinned
//!   (walk clip driven by gait); `MOBA_NO_SKINNING=1` draws it static instead.
//!   The map's visuals name resolves to the descriptor `<dir>/maps/<name>.toml` (terrain
//!   tint, walls, props; #287), falling back to the built-in theme.
//! - `MOBA_CONTENT=<pack.toml>` loads a content pack for the local preview and for the
//!   `MOBA_SERVER=local` loopback server.
//! - `MOBA_CAMERA_DISTANCE=<m>` sets the player-camera distance (default 46, 8-120).
//! - `MOBA_CAPTURE_TICK=<n>` (network mode, with `MOBA_CAPTURE`): capture once the match is
//!   Playing at snapshot tick `n`, with interpolation and animation pinned to `n`. A
//!   `MOBA_SERVER=local` server is lock-stepped to the client and stops at `n`, so the frame
//!   is repeatable (with a remote server it is best effort). Projectile effects still advance
//!   by frame time, so pick a tick before the first fight (the golden uses 120).
//! - `MOBA_WINDOW_SIZE=<w>x<h>`: logical window size (default 1280x720).
//! - `MOBA_WARMUP_TICKS=<n>`: the local preview simulates `n` ticks before the first frame;
//!   in network mode, frame logging and `MOBA_SMOKE_FRAMES` start once Playing reaches tick
//!   `n` (a client that never reaches it, e.g. a lobby waiting for ready, does not exit).
//! - `MOBA_CAPTURE_MAPS=<prefix>` (network mode, evidence for #287): after every map change
//!   (connect, content reload, map rotation), capture `<prefix>-<n>.ppm` once
//!   `MOBA_CAPTURE_FRAME` frames with snapshots have rendered on it, and keep playing.

#![cfg_attr(not(test), forbid(unsafe_code))]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use moba_core::Fix;
use moba_core::math::Vec3;
use moba_game::Team;
use moba_net::protocol::{MatchPhase, SnapshotState, WireCommand, quantize_position};
use moba_net::{ClientEvent, Interpolator, PredictionState};
use moba_platform::{App, Frame, FrameOutcome, Key, Window};
use moba_render::{
    Camera, RenderConfig, Renderer, Scene, UiDrawList, UnitInstance, ValidationMode,
};
use moba_sim::{SimConfig, Simulation, UnitKind};
use std::collections::VecDeque;

use crate::hud::{HitList, build_local_hud, build_network_hud, build_vision};

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
    bones: skinning::BonePool,
    ui: UiDrawList,
    scoreboard_visible: bool,
    shop_visible: bool,
    hit_rects: HitList,
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
    bindings: crate::settings::Bindings,
    camera_sensitivity: f32,
    frame_log: Option<Vec<f32>>,
    /// Per-frame `[cpu_ms, render_ms]` while logging: time from frame start to the render
    /// call, and time inside it (fence wait, acquire, recording, submit, present).
    work_log: Option<Vec<[f32; 2]>>,
    frame_log_path: Option<PathBuf>,
    /// Network-mode warm-up (`MOBA_WARMUP_TICKS`): measure only from this match tick on.
    network_warmup: Option<u32>,
    /// Frames counted towards `MOBA_SMOKE_FRAMES` and logged (after any warm-up).
    measured_frames: u64,
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
        let camera_sensitivity = settings.camera_sensitivity;
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
        let bindings = settings.bindings().0;
        let env_scale = std::env::var("MOBA_UI_SCALE")
            .ok()
            .and_then(|value| value.parse::<f32>().ok());
        let frame_log_path = std::env::var("MOBA_FRAME_LOG").ok().map(PathBuf::from);
        let frame_log = frame_log_path.as_ref().map(|_| Vec::with_capacity(10_000));
        let work_log = frame_log_path.as_ref().map(|_| Vec::with_capacity(10_000));
        let mut app = Self {
            renderer: None,
            simulation: None,
            network: None,
            local_audio: None,
            // Every entity, five extra rig parts per hero, map walls/props and projectiles,
            // so the frame loop never grows the buffer.
            units: Vec::with_capacity(
                moba_sim::MAX_UNITS
                    + 5 * 10
                    + moba_game::MAX_CONTENT_WALLS
                    + moba_assets::MAX_MAP_PROPS
                    + 64,
            ),
            models: models::ModelLibrary::from_env(),
            map_visuals: map_visuals::MapLibrary::new(models::asset_root_from_env()),
            map_capture: map_visuals::SwitchCapture::from_env(),
            bones: skinning::BonePool::new(moba_render::DEFAULT_MAX_BONES),
            ui: UiDrawList::new(),
            scoreboard_visible: std::env::var_os("MOBA_SCOREBOARD").is_some(),
            shop_visible: std::env::var_os("MOBA_SHOP").is_some(),
            hit_rects: HitList::new(),
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
            bindings,
            camera_sensitivity,
            frame_log,
            work_log,
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
                        shop_page: 0,
                        spectator_pan: Vec3::ZERO,
                        spectator_yaw: -0.7,
                        spectator_zoom: camera_zoom,
                        follow_distance: parse_follow_distance(),
                        overview_camera: std::env::var("MOBA_CAMERA")
                            .is_ok_and(|value| value == "overview"),
                        // Only a capture pins the render tick; otherwise play normally.
                        capture_tick: parse_capture_tick()
                            .filter(|_| std::env::var_os("MOBA_CAPTURE").is_some()),
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
                let warmup: u32 = std::env::var("MOBA_WARMUP_TICKS")
                    .ok()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0);
                for _ in 0..warmup {
                    simulation.tick_with_bots();
                }
                simulation.clear_events();
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

impl App for ClientApp {
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
            .with_max_bones(moba_render::DEFAULT_MAX_BONES)
            .with_skinning(self.models.skinning());
        match Renderer::new(window, config) {
            Ok(mut renderer) => {
                // `on_start` may run again after a resume with a fresh mesh table.
                self.models.renderer_recreated();
                self.map_visuals.renderer_recreated();
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
            if let Some(samples) = self.frame_log.as_mut()
                && samples.len() < 10_000
            {
                samples.push(raw_ms);
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
                bones,
                ui,
                scoreboard_visible,
                shop_visible,
                hit_rects,
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
                            ClientEvent::GameEvent(event) => network.handle_game_event(event),
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
                                // Content reload or map rotation: rebind the map visuals and
                                // hero models (cached, so only new names touch the disk or
                                // the GPU).
                                if let Some(renderer) = renderer.as_mut() {
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
                            network.own_net_id = None;
                            network.prediction.set_target(None);
                            network.client.clear_pending_input();
                        }
                        network.last_tick = snapshot.tick;
                        if network.capture_tick.is_some() {
                            // Lets a lock-stepped local capture server run the next tick.
                            launch::LOCAL_CAPTURE_ACK
                                .store(snapshot.tick, std::sync::atomic::Ordering::Relaxed);
                        }
                        network.interpolator.push(snapshot.clone());
                        if let Some(own) = snapshot.entities.iter().find(|entity| {
                            entity.hero_slot == network.own_slot
                                && entity.team == network.client.team().index() as u8
                        }) {
                            network.own_net_id = Some(own.net_id);
                            network.prediction.set_dead(own.is_dead());
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
                    }
                    if network.phase == MatchPhase::Playing {
                        for _ in 0..frame.ticks() {
                            network.prediction.step();
                        }
                    }
                    rebuild_network_units(network, models, map_visuals, units, bones);
                    if width > 0 && height > 0 {
                        let aspect = width as f32 / height as f32;
                        let camera = network_camera(network, aspect, frame.elapsed_secs() as f32);
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
                        if frame.input().was_key_pressed(self.bindings.scoreboard) {
                            *scoreboard_visible = !*scoreboard_visible;
                        }
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
                        );
                        network.update_effects(frame_ms);
                        let vision = build_vision(network);
                        let scene = Scene::with_vision(camera, units, vision)
                            .with_terrain_tint(map_visuals.terrain_tint())
                            .with_bones(bones.bones());
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
                            });
                        if let Some(renderer) = renderer.as_mut() {
                            if capture_due {
                                capture_and_exit(
                                    renderer,
                                    &scene,
                                    ui,
                                    capture_path.as_deref(),
                                    &mut failure,
                                );
                                exit_after_capture = true;
                            } else if let Some(path) = switch_capture {
                                // Mid-session: a failed capture (minimised window, resize,
                                // validation message) is reported, not fatal.
                                let mut capture_failure = None;
                                capture_and_exit(
                                    renderer,
                                    &scene,
                                    ui,
                                    Some(&path),
                                    &mut capture_failure,
                                );
                                match capture_failure {
                                    None => eprintln!(
                                        "[assets] map capture written to {}",
                                        path.display()
                                    ),
                                    Some(error) => eprintln!(
                                        "[assets] map capture {} failed: {error}",
                                        path.display()
                                    ),
                                }
                            } else {
                                render_started = Some(Instant::now());
                                if let Err(error) = renderer.render_with_ui(&scene, ui) {
                                    failure = Some(format!("render failed: {error}"));
                                }
                            }
                        }
                    }
                }
            } else if let Some(simulation) = simulation {
                // Golden captures must not depend on wall-clock frame pacing.
                let ticks = if capture_path.is_some() {
                    1
                } else {
                    frame.ticks()
                };
                for _ in 0..ticks {
                    simulation.tick_with_bots();
                    if let Some(audio) = self.local_audio.as_mut() {
                        for event in simulation.events() {
                            if let Some(sound) = local_sound(event) {
                                audio.play(sound);
                            }
                        }
                    }
                    simulation.clear_events();
                }
                units.clear();
                bones.clear();
                // The same walls (and descriptor props) the network path draws.
                map_visuals.push_instances(units);
                let world = simulation.world();
                for entity in world.entities() {
                    let (Some(position), Some(team), Some(kind), Some(health)) = (
                        world.get::<moba_sim::Position>(entity),
                        world.get::<Team>(entity),
                        world.get::<UnitKind>(entity),
                        world.get::<moba_sim::Health>(entity),
                    ) else {
                        continue;
                    };
                    if health.current <= 0 {
                        continue;
                    }
                    let fraction = health.current as f32 / health.max as f32;
                    let center = simulation.content().map.extent.to_f32() * 0.5;
                    if *kind == UnitKind::Hero {
                        let model = world
                            .get::<moba_sim::HeroIdentity>(entity)
                            .and_then(|hero| models.hero(usize::from(hero.0.0)));
                        push_hero(
                            units,
                            bones,
                            models,
                            model,
                            &HeroPose {
                                position: position.0,
                                team: *team,
                                health_fraction: fraction,
                                center,
                                time: simulation.current_tick().get() as f32 / 30.0,
                                phase_offset: entity.index() as f32,
                                facing: 0.0,
                                gait: 1.0,
                            },
                        );
                    } else {
                        push_unit(units, position.0, *kind, *team, fraction, 0, center);
                    }
                }
                if width > 0 && height > 0 {
                    let aspect = width as f32 / height as f32;
                    let yaw = if capture_path.is_some() {
                        -0.7
                    } else {
                        frame.elapsed_secs() as f32 * 0.035
                    };
                    build_local_hud(
                        ui,
                        simulation,
                        (width as f32 / self.ui_scale) as u32,
                        (height as f32 / self.ui_scale) as u32,
                        frame_ms,
                        fps,
                        hit_rects,
                    );
                    let camera = Camera::orbit(Vec3::ZERO, 235.0, yaw, 0.92, aspect);
                    let scene = Scene::new(camera, units)
                        .with_terrain_tint(map_visuals.terrain_tint())
                        .with_bones(bones.bones());
                    if let Some(renderer) = renderer.as_mut() {
                        if frames_is_capture(*frames, *capture_frame, capture_path.as_deref()) {
                            capture_and_exit(
                                renderer,
                                &scene,
                                ui,
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

        if measuring
            && let (Some(started), Some(work)) = (render_started, self.work_log.as_mut())
            && work.len() < 10_000
        {
            let cpu_ms = started.duration_since(now).as_secs_f32() * 1_000.0;
            let render_ms = started.elapsed().as_secs_f32() * 1_000.0;
            work.push([cpu_ms, render_ms]);
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
        {
            return FrameOutcome::Exit;
        }
        FrameOutcome::Continue
    }
}

mod launch;
mod map_visuals;
mod models;
mod scene_build;
mod skinning;

pub(crate) use launch::{
    capture_and_exit, connect_network, frames_is_capture, parse_capture_tick, parse_demo_move,
    parse_follow_distance, run,
};
pub(crate) use scene_build::{
    HeroPose, network_camera, push_hero, push_unit, rebuild_network_units,
};

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
    use super::{Camera, Fix, MatchPhase, Team, UnitKind, Vec3};
    use crate::app::scene_build::{kind_from_u8, team_from_u8};
    use crate::hud::HudAction;
    use crate::input::{InputFocus, cursor_ground, ground_orders_allowed};
    use moba_core::FixVec2;
    use moba_core::math::Vec2;

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
