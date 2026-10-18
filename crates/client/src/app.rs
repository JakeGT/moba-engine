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

#![cfg_attr(not(test), forbid(unsafe_code))]

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moba_core::math::{Mat4, Quat, Vec2, Vec3, Vec4};
use moba_core::{Fix, FixVec2};
use moba_game::Team;
use moba_net::protocol::{
    EVENT_CAST, EVENT_MATCH_ENDED, EVENT_PROJECTILE_SPAWN, EVENT_STRUCTURE_DESTROYED,
    EVENT_UNIT_DIED, MatchPhase, NetEvent, STATUS_BURN, STATUS_HASTE, STATUS_ROOT, STATUS_SHIELD,
    STATUS_SILENCE, STATUS_SLOW, WireCommand, dequantize_position, quantize_position,
};
use moba_net::{ClientEvent, Interpolator, NetClient, PredictionState};
use moba_platform::{
    App, Frame, FrameOutcome, InputState, Key, MouseButton, Window, WindowConfig, run as run_window,
};
use moba_render::{
    Camera, CapturedFrame, RenderConfig, Renderer, Scene, UiDrawList, UnitInstance, ValidationMode,
};
use moba_sim::{SimConfig, Simulation, UnitKind};
use std::collections::VecDeque;

use crate::hud::{
    HitList, HudAction, HudUnit, build_local_hud, build_network_hud, build_vision, map_center,
    minimap_world, rect_at, rendered_entity_position,
};

// Four ticks (133 ms) covers two lost 30 Hz snapshots plus 30 ms of jitter.
const SNAPSHOT_DELAY_TICKS: f32 = 4.0;

#[allow(clippy::struct_excessive_bools)] // Mode flags are independent UI/network state.
pub(crate) struct KillFeedEntry {
    pub(crate) life: u32,
    pub(crate) text: String,
}

impl ProjectileFx {
    fn arc_height(&self) -> f32 {
        if self.total <= 0.0 || self.arc <= 0.0 {
            return 0.0;
        }
        let progress = (self.travelled / self.total).clamp(0.0, 1.0);
        self.arc * (core::f32::consts::PI * progress).sin()
    }
}

pub(crate) struct ProjectileFx {
    pub(crate) team: u8,
    pub(crate) position: FixVec2,
    pub(crate) direction: FixVec2,
    pub(crate) speed: f32,
    pub(crate) radius: f32,
    pub(crate) remaining: f32,
    pub(crate) arc: f32,
    pub(crate) total: f32,
    pub(crate) travelled: f32,
}

#[allow(clippy::struct_excessive_bools)] // Independent connection/UI flags.
pub(crate) struct NetworkView {
    pub(crate) client: NetClient,
    pub(crate) prediction: PredictionState,
    pub(crate) interpolator: Interpolator,
    pub(crate) own_slot: u8,
    pub(crate) own_net_id: Option<u16>,
    pub(crate) last_snapshot_seq: u32,
    pub(crate) phase: MatchPhase,
    pub(crate) countdown: u16,
    pub(crate) spectator: bool,
    pub(crate) ready: bool,
    pub(crate) ready_count: u8,
    pub(crate) player_count: u8,
    pub(crate) last_tick: u32,
    pub(crate) demo_move: Option<FixVec2>,
    pub(crate) demo_sent: bool,
    pub(crate) demo_buy: Option<u8>,
    pub(crate) demo_bought: bool,
    pub(crate) shop_page: usize,
    pub(crate) spectator_pan: Vec3,
    pub(crate) spectator_yaw: f32,
    pub(crate) spectator_zoom: f32,
    pub(crate) kill_feed: VecDeque<KillFeedEntry>,
    pub(crate) projectiles: Vec<ProjectileFx>,
    audio: crate::audio::Audio,
}

struct ClientApp {
    renderer: Option<Renderer>,
    simulation: Option<Simulation>,
    network: Option<NetworkView>,
    units: Vec<UnitInstance>,
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
        if std::env::var_os("MOBA_SAVE_SETTINGS").is_some() {
            if let Err(error) = settings.save(&crate::settings::Settings::path()) {
                eprintln!("settings: save failed: {error}");
            }
        }
        let bindings = settings.bindings().0;
        let env_scale = std::env::var("MOBA_UI_SCALE")
            .ok()
            .and_then(|value| value.parse::<f32>().ok());
        let mut app = Self {
            renderer: None,
            simulation: None,
            network: None,
            units: Vec::with_capacity(moba_sim::MAX_UNITS),
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
                        kill_feed: VecDeque::with_capacity(6),
                        projectiles: Vec::with_capacity(32),
                        audio: crate::audio::Audio::new(audio_volume, audio_mute),
                    });
                }
                Err(error) => app.startup_error = Some(error),
            },
            _ => {
                let mut simulation = Simulation::new(SimConfig::default());
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
        if let Ok(mut slot) = self.error.lock() {
            if slot.is_none() {
                *slot = Some(message);
            }
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
        let config = RenderConfig {
            validation,
            max_instances: 1_024,
            vsync: true,
            ui_scale: self.ui_scale,
            ..RenderConfig::default()
        };
        match Renderer::new(window, config) {
            Ok(renderer) => {
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
        self.frames += 1;
        let now = Instant::now();
        let raw_ms = now.duration_since(self.last_frame).as_secs_f32() * 1_000.0;
        self.last_frame = now;
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
        let mut exit_after_capture = false;
        let [width, height] = frame.window().size();
        let (frame_ms, fps) = (self.frame_ms, self.fps);
        {
            let ClientApp {
                network,
                simulation,
                units,
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
                if failure.is_none() {
                    if let Err(error) = network.client.update() {
                        failure = Some(format!("network update failed: {error}"));
                    }
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
                            ClientEvent::Snapshot { .. } => {}
                        }
                    }
                }
                if failure.is_none() {
                    let snapshot = network.client.snapshot().cloned();
                    if let Some(snapshot) = snapshot {
                        if snapshot.seq != network.last_snapshot_seq {
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
                                }) {
                                    if network
                                        .client
                                        .send_command(WireCommand::BuyItem { item })
                                        .is_ok()
                                    {
                                        network.demo_bought = true;
                                    }
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
                    }
                    if network.phase == MatchPhase::Playing {
                        for _ in 0..frame.ticks() {
                            network.prediction.step();
                        }
                    }
                    rebuild_network_units(network, units);
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
                        let scene = Scene::with_vision(camera, units, vision);
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
                            } else if let Err(error) = renderer.render_with_ui(&scene, ui) {
                                failure = Some(format!("render failed: {error}"));
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
                    simulation.clear_events();
                }
                units.clear();
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
                    push_unit(
                        units,
                        position.0,
                        *kind,
                        *team,
                        fraction,
                        0,
                        simulation.content().map.extent.to_f32() * 0.5,
                    );
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
                    let scene = Scene::new(camera, units);
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
                        } else if let Err(error) = renderer.render_with_ui(&scene, ui) {
                            failure = Some(format!("render failed: {error}"));
                        }
                    }
                }
            }
        }

        if let Some(message) = failure {
            self.fail(message);
            return FrameOutcome::Exit;
        }
        if exit_after_capture {
            return FrameOutcome::Exit;
        }
        if frame.input().was_key_pressed(Key::Escape) || self.frames >= self.quit_after_frames {
            return FrameOutcome::Exit;
        }
        FrameOutcome::Continue
    }
}

fn frames_is_capture(frames: u64, capture_frame: u64, capture_path: Option<&Path>) -> bool {
    capture_path.is_some() && frames >= capture_frame
}

fn capture_and_exit(
    renderer: &mut Renderer,
    scene: &Scene<'_>,
    ui: &UiDrawList,
    capture_path: Option<&Path>,
    failure: &mut Option<String>,
) {
    let Some(path) = capture_path else {
        return;
    };
    match renderer.capture_frame_with_ui(scene, ui) {
        Ok(frame) => {
            if let Err(error) = write_ppm(path, &frame) {
                *failure = Some(format!("capture write failed: {error}"));
            }
        }
        Err(error) => *failure = Some(format!("capture failed: {error}")),
    }
}

fn parse_demo_move() -> Option<FixVec2> {
    let value = std::env::var("MOBA_DEMO_MOVE").ok()?;
    let (x, y) = value.split_once(',')?;
    Some(FixVec2::from_ints(
        x.trim().parse().ok()?,
        y.trim().parse().ok()?,
    ))
}

fn parse_secret() -> Option<[u8; 32]> {
    let value = std::env::var("MOBA_SECRET").ok()?;
    let bytes = value.as_bytes();
    if bytes.len() != 64 {
        eprintln!("MOBA_SECRET must be 64 hex characters; ignoring");
        return None;
    }
    let mut key = [0u8; 32];
    for (index, chunk) in bytes.chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk).ok()?;
        key[index] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(key)
}

fn parse_server_public() -> Option<[u8; 32]> {
    let value = std::env::var("MOBA_SERVER_PUBKEY").ok()?;
    let bytes = value.as_bytes();
    if bytes.len() != 64 {
        eprintln!("MOBA_SERVER_PUBKEY must be 64 hex characters; ignoring");
        return None;
    }
    let mut key = [0u8; 32];
    for (index, chunk) in bytes.chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk).ok()?;
        key[index] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(key)
}

fn parse_identity_seed() -> Option<[u8; 32]> {
    let value = std::env::var("MOBA_CLIENT_IDENTITY").ok()?;
    let bytes = value.as_bytes();
    if bytes.len() != 64 {
        eprintln!("MOBA_CLIENT_IDENTITY must be 64 hex characters; ignoring");
        return None;
    }
    let mut seed = [0u8; 32];
    for (index, chunk) in bytes.chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk).ok()?;
        seed[index] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(seed)
}

fn connect_network(spec: &str) -> Result<NetClient, String> {
    let secret = parse_secret();
    let server_public = parse_server_public();
    let identity_seed = parse_identity_seed();
    let spectator = std::env::var("MOBA_SPECTATE").is_ok_and(|value| value == "1");
    if spec == "local" {
        let mut server = moba_net::NetServer::bind(moba_net::ServerConfig {
            bind: "127.0.0.1:0".parse().expect("loopback address"),
            sim: SimConfig::default(),
            snapshot_interval: 1,
            max_clients: 10,
            countdown_ticks: 90,
            results_ticks: 450,
            client_timeout: Duration::from_secs(5),
            reconnect_grace: Duration::from_secs(60),
            pre_shared_key: secret,
            record: None,
            max_spectators: 8,
            max_packets_per_second: 120,
            max_total_packets_per_second: 2_000,
            require_ready: false,
        })
        .map_err(|error| format!("local server failed: {error}"))?;
        let addr = server
            .local_addr()
            .map_err(|error| format!("local server address: {error}"))?;
        std::thread::spawn(move || {
            let _ = server.run_for(Duration::from_secs(3_600));
        });
        let auth = moba_net::client::ClientAuth {
            pre_shared_key: secret,
            expected_server: None,
            identity_seed,
            spectator,
        };
        NetClient::connect_with_auth(addr, Duration::from_secs(5), auth)
            .map_err(|error| format!("local connection failed: {error}"))
    } else {
        let addr = spec
            .parse()
            .map_err(|error| format!("invalid MOBA_SERVER {spec}: {error}"))?;
        let auth = moba_net::client::ClientAuth {
            pre_shared_key: secret,
            expected_server: server_public,
            identity_seed,
            spectator,
        };
        NetClient::connect_with_auth(addr, Duration::from_secs(5), auth)
            .map_err(|error| format!("connection to {spec} failed: {error}"))
    }
}

fn rebuild_network_units(network: &NetworkView, units: &mut Vec<UnitInstance>) {
    units.clear();
    let Some(latest) = network.interpolator.latest() else {
        return;
    };
    let wire_map = network.client.map();
    let center = map_center(network);
    for wall in wire_map.walls.iter().take(usize::from(wire_map.wall_count)) {
        let a = Vec2::new(f32::from(wall.a[0]), f32::from(wall.a[1]));
        let b = Vec2::new(f32::from(wall.b[0]), f32::from(wall.b[1]));
        let direction = b - a;
        let length = direction.length().max(0.5);
        let angle = direction.y.atan2(direction.x);
        let mid = (a + b) * 0.5;
        let transform = Mat4::from_scale_rotation_translation(
            Vec3::new(length, 4.0, 2.5),
            Quat::from_rotation_y(-angle),
            Vec3::new(mid.x - center, 2.0, mid.y - center),
        );
        units.push(UnitInstance::new(
            transform,
            Vec4::new(0.24, 0.21, 0.16, 1.0),
        ));
    }
    let render_tick = network.interpolator.render_tick(Instant::now());
    for entity in &latest.entities {
        if entity.is_dead() {
            continue;
        }
        let Some(kind) = kind_from_u8(entity.kind) else {
            continue;
        };
        let Some(team) = team_from_u8(entity.team) else {
            continue;
        };
        let position = rendered_entity_position(network, HudUnit::new(entity), render_tick);
        let fraction = if entity.max_health == 0 {
            1.0
        } else {
            f32::from(entity.health) / f32::from(entity.max_health)
        };
        push_unit(
            units,
            position,
            kind,
            team,
            fraction,
            entity.status_bits,
            center,
        );
    }
    for projectile in &network.projectiles {
        let color = if projectile.team == 0 {
            Vec4::new(0.35, 0.72, 1.0, 1.0)
        } else {
            Vec4::new(1.0, 0.45, 0.3, 1.0)
        };
        units.push(UnitInstance::at(
            Vec3::new(
                projectile.position.x.to_f32() - center,
                1.2,
                projectile.position.y.to_f32() - center,
            ),
            1.2 + projectile.radius * 0.2 + projectile.arc_height(),
            color,
        ));
    }
}

fn push_unit(
    units: &mut Vec<UnitInstance>,
    position: FixVec2,
    kind: UnitKind,
    team: Team,
    health_fraction: f32,
    status: u16,
    center: f32,
) {
    let team_color = match team {
        Team::Blue => Vec4::new(0.20, 0.46, 1.0, 1.0),
        Team::Red => Vec4::new(1.0, 0.24, 0.18, 1.0),
    };
    let (height, mut color) = match kind {
        UnitKind::Hero => (4.6, team_color),
        UnitKind::MeleeMinion => (
            2.2,
            Vec4::new(
                team_color.x * 0.72 + 0.03,
                team_color.y * 0.72 + 0.03,
                team_color.z * 0.72 + 0.03,
                1.0,
            ),
        ),
        UnitKind::RangedMinion => (
            1.8,
            Vec4::new(
                team_color.x * 0.62 + 0.05,
                team_color.y * 0.62 + 0.05,
                team_color.z * 0.62 + 0.05,
                1.0,
            ),
        ),
        UnitKind::Tower => (
            9.0,
            Vec4::new(
                team_color.x * 0.60 + 0.12,
                team_color.y * 0.60 + 0.12,
                team_color.z * 0.60 + 0.12,
                1.0,
            ),
        ),
        UnitKind::Nexus => (15.0, Vec4::new(1.0, 0.76, 0.22, 1.0)),
    };
    let brightness = 0.55 + 0.45 * health_fraction.clamp(0.0, 1.0);
    color.x *= brightness;
    color.y *= brightness;
    color.z *= brightness;
    if status & STATUS_SHIELD != 0 {
        color = color * 0.65 + Vec4::new(0.15, 0.65, 0.95, 1.0) * 0.35;
    }
    if status & STATUS_SLOW != 0 {
        color = color * 0.75 + Vec4::new(0.35, 0.2, 0.75, 1.0) * 0.25;
    }
    if status & STATUS_ROOT != 0 {
        color = color * 0.7 + Vec4::new(0.9, 0.75, 0.18, 1.0) * 0.3;
    }
    if status & STATUS_SILENCE != 0 {
        color = color * 0.7 + Vec4::new(0.55, 0.55, 0.6, 1.0) * 0.3;
    }
    if status & STATUS_BURN != 0 {
        color = color * 0.7 + Vec4::new(1.0, 0.42, 0.1, 1.0) * 0.3;
    }
    if status & STATUS_HASTE != 0 {
        color = color * 0.75 + Vec4::new(0.25, 0.95, 0.4, 1.0) * 0.25;
    }
    let world_position = Vec3::new(
        position.x.to_f32() - center,
        0.0,
        position.y.to_f32() - center,
    );
    units.push(UnitInstance::at(world_position, height, color));
}

fn kind_from_u8(value: u8) -> Option<UnitKind> {
    Some(match value {
        0 => UnitKind::Hero,
        1 => UnitKind::MeleeMinion,
        2 => UnitKind::RangedMinion,
        3 => UnitKind::Tower,
        4 => UnitKind::Nexus,
        _ => return None,
    })
}

fn team_from_u8(value: u8) -> Option<Team> {
    Some(match value {
        0 => Team::Blue,
        1 => Team::Red,
        _ => return None,
    })
}

fn network_camera(network: &NetworkView, aspect: f32, elapsed: f32) -> Camera {
    if network.spectator {
        let idle_yaw = network.spectator_yaw + (elapsed * 0.002).sin() * 0.02;
        return Camera::orbit(
            network.spectator_pan,
            network.spectator_zoom,
            idle_yaw,
            0.92,
            aspect,
        );
    }
    if std::env::var("MOBA_CAMERA").is_ok_and(|value| value == "overview") {
        return Camera::orbit(Vec3::ZERO, 240.0, -0.7, 0.92, aspect);
    }
    let predicted = network.prediction.position();
    let world_center = map_center(network);
    let center = Vec3::new(
        predicted.x.to_f32() - world_center,
        0.0,
        predicted.y.to_f32() - world_center,
    );
    Camera::orbit(center, 46.0, -0.7, 0.95, aspect)
}

fn handle_spectator_input(
    network: &mut NetworkView,
    input: &InputState,
    bindings: &crate::settings::Bindings,
    sensitivity: f32,
) {
    let pan_speed = if input.is_key_down(Key::Shift) {
        14.0
    } else {
        5.0
    };
    if input.is_key_down(bindings.move_up) {
        network.spectator_pan.z -= pan_speed;
    }
    if input.is_key_down(bindings.move_down) {
        network.spectator_pan.z += pan_speed;
    }
    if input.is_key_down(bindings.move_left) {
        network.spectator_pan.x -= pan_speed;
    }
    if input.is_key_down(bindings.move_right) {
        network.spectator_pan.x += pan_speed;
    }
    let pan_limit = map_center(network);
    network.spectator_pan.x = network.spectator_pan.x.clamp(-pan_limit, pan_limit);
    network.spectator_pan.z = network.spectator_pan.z.clamp(-pan_limit, pan_limit);
    if input.is_key_down(bindings.camera_left) {
        network.spectator_yaw -= 0.03 * sensitivity;
    }
    if input.is_key_down(bindings.camera_right) {
        network.spectator_yaw += 0.03 * sensitivity;
    }
    if input.was_key_pressed(bindings.camera_up) {
        network.spectator_zoom =
            (network.spectator_zoom - 10.0 * sensitivity).max(crate::settings::CAMERA_ZOOM_MIN);
    }
    if input.was_key_pressed(bindings.camera_down) {
        network.spectator_zoom =
            (network.spectator_zoom + 10.0 * sensitivity).min(crate::settings::CAMERA_ZOOM_MAX);
    }
}

/// Input focus for the network client.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum InputFocus {
    Gameplay,
    Shop,
}

impl InputFocus {
    const fn from_shop(shop: bool) -> Self {
        if shop { Self::Shop } else { Self::Gameplay }
    }
}

fn handle_shop_input(network: &mut NetworkView, input: &InputState, ui_action: Option<HudAction>) {
    let page_count = network.client.items().len().div_ceil(8).max(1);
    if input.was_key_pressed(Key::ArrowLeft) {
        network.shop_page = network.shop_page.saturating_sub(1);
    }
    if input.was_key_pressed(Key::ArrowRight) {
        network.shop_page = (network.shop_page + 1).min(page_count - 1);
    }
    network.shop_page = network.shop_page.min(page_count - 1);
    for (key, offset) in [
        (Key::Digit1, 0u8),
        (Key::Digit2, 1),
        (Key::Digit3, 2),
        (Key::Digit4, 3),
        (Key::Digit5, 4),
        (Key::Digit6, 5),
        (Key::Digit7, 6),
        (Key::Digit8, 7),
    ] {
        if input.was_key_pressed(key) {
            let item = network.shop_page * 8 + usize::from(offset);
            if item < network.client.items().len() {
                let _ = network
                    .client
                    .send_command(WireCommand::BuyItem { item: item as u8 });
            }
        }
    }
    if let Some(HudAction::ShopItem(item)) = ui_action
        && input.was_mouse_pressed(MouseButton::Left)
    {
        let _ = network.client.send_command(WireCommand::BuyItem { item });
    }
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // One ordered focus/input sequence.
fn handle_network_input(
    network: &mut NetworkView,
    input: &InputState,
    ui_input: &InputState,
    camera: &Camera,
    width: u32,
    height: u32,
    shop: &mut bool,
    hits: &HitList,
    bindings: &crate::settings::Bindings,
    sensitivity: f32,
) {
    if network.spectator {
        handle_spectator_input(network, input, bindings, sensitivity);
        return;
    }
    let ui_rect = ui_input
        .cursor()
        .and_then(|cursor| rect_at(hits, cursor.x, cursor.y));
    let ui_action = ui_rect.map(|rect| rect.action);
    if let (Some(rect), Some(cursor)) = (ui_rect, ui_input.cursor())
        && rect.action == HudAction::Minimap
        && (input.was_mouse_pressed(MouseButton::Left)
            || input.was_mouse_pressed(MouseButton::Right))
    {
        let (x, y) = minimap_world(rect, cursor.x, cursor.y);
        let target = FixVec2::new(Fix::from_f32(x), Fix::from_f32(y));
        let (x, y) = quantize_position(target);
        if network
            .client
            .send_command(WireCommand::MoveTo { x, y })
            .is_ok()
        {
            network.prediction.set_target(Some(target));
        }
        return;
    }
    if input.was_key_pressed(bindings.ready) && network.phase == MatchPhase::Lobby {
        let ready = !network.ready;
        if network
            .client
            .send_command(WireCommand::Ready { ready })
            .is_ok()
        {
            network.ready = ready;
        }
    }
    if input.was_key_pressed(bindings.surrender) && network.phase == MatchPhase::Playing {
        let _ = network.client.send_command(WireCommand::Surrender);
    }
    if input.was_key_pressed(bindings.shop) {
        *shop = !*shop;
    }
    match InputFocus::from_shop(*shop) {
        InputFocus::Shop => {
            handle_shop_input(network, input, ui_action);
            return;
        }
        InputFocus::Gameplay => {}
    }
    if network.phase != MatchPhase::Playing {
        return;
    }
    let Some(cursor) = input.cursor() else {
        return;
    };
    if ui_action.is_some() {
        return;
    }
    let Some(ground) = cursor_ground(camera, cursor, width, height, map_center(network)) else {
        return;
    };

    if input.was_mouse_pressed(MouseButton::Right) {
        let (x, y) = quantize_position(ground);
        if network
            .client
            .send_command(WireCommand::MoveTo { x, y })
            .is_ok()
        {
            network.prediction.set_target(Some(ground));
        }
    }
    let attack_button = match bindings.attack_button {
        crate::settings::MouseBinding::Left => MouseButton::Left,
        crate::settings::MouseBinding::Right => MouseButton::Right,
    };
    if input.was_mouse_pressed(attack_button) {
        if let Some(target) = nearest_enemy(network, ground) {
            let _ = network
                .client
                .send_command(WireCommand::Attack { net_id: target });
        }
    }
    for (key, slot) in [
        (bindings.cast_q, 0u8),
        (bindings.cast_w, 1),
        (bindings.cast_e, 2),
    ] {
        if input.was_key_pressed(key) {
            let (x, y) = quantize_position(ground);
            let _ = network
                .client
                .send_command(WireCommand::CastAbility { slot, x, y });
        }
    }
    if input.was_key_pressed(bindings.stop) {
        let _ = network.client.send_command(WireCommand::Stop);
        network.prediction.set_target(None);
    }
}

fn nearest_enemy(network: &NetworkView, ground: FixVec2) -> Option<u16> {
    let own_team = network.client.team().index() as u8;
    let snapshot = network.client.snapshot()?;
    let mut best: Option<(Fix, u16)> = None;
    for entity in &snapshot.entities {
        if entity.team == own_team || entity.is_dead() {
            continue;
        }
        let position = dequantize_position(entity.x, entity.y);
        let distance = position.distance_sq(ground);
        if distance <= Fix::from_i32(4) * Fix::from_i32(4)
            && best.is_none_or(|(best_distance, _)| distance < best_distance)
        {
            best = Some((distance, entity.net_id));
        }
    }
    best.map(|(_, id)| id)
}

fn cursor_ground(
    camera: &Camera,
    cursor: Vec2,
    width: u32,
    height: u32,
    center: f32,
) -> Option<FixVec2> {
    if width == 0 || height == 0 {
        return None;
    }
    let ndc_x = 2.0 * cursor.x / width as f32 - 1.0;
    let ndc_y = 1.0 - 2.0 * cursor.y / height as f32;
    let inverse = camera.view_proj.inverse();
    let near_h = inverse * Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
    let far_h = inverse * Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
    if near_h.w.abs() < 1e-6 || far_h.w.abs() < 1e-6 {
        return None;
    }
    let near = near_h.truncate() / near_h.w;
    let far = far_h.truncate() / far_h.w;
    let direction = far - near;
    if direction.y.abs() < 1e-5 {
        return None;
    }
    let distance = -near.y / direction.y;
    if distance < 0.0 {
        return None;
    }
    let hit = near + direction * distance;
    Some(FixVec2::new(
        Fix::from_f32(hit.x + center),
        Fix::from_f32(hit.z + center),
    ))
}

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let quit_after_frames = std::env::var("MOBA_SMOKE_FRAMES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(u64::MAX);
    let capture_path = std::env::var("MOBA_CAPTURE").ok().map(PathBuf::from);
    let capture_frame = std::env::var("MOBA_CAPTURE_FRAME")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(3);
    let error = Arc::new(Mutex::new(None::<String>));
    let app = ClientApp::new(
        quit_after_frames,
        capture_path,
        capture_frame,
        Arc::clone(&error),
    );
    let config = WindowConfig::new("MOBA", 1280, 720);
    run_window(config, app)?;
    if let Some(message) = error.lock().ok().and_then(|mut slot| slot.take()) {
        return Err(message.into());
    }
    Ok(())
}

fn write_ppm(path: &Path, frame: &CapturedFrame) -> std::io::Result<()> {
    let mut file = BufWriter::new(File::create(path)?);
    write!(file, "P6\n{} {}\n255\n", frame.width, frame.height)?;
    let mut rgb = Vec::with_capacity(frame.rgba.len() / 4 * 3);
    for pixel in frame.rgba.chunks_exact(4) {
        rgb.extend_from_slice(&pixel[..3]);
    }
    file.write_all(&rgb)?;
    file.flush()
}

#[allow(clippy::too_many_lines)] // One HUD layout, kept together for review.
impl NetworkView {
    #[allow(clippy::too_many_lines)] // Event-to-HUD/audio mapping is linear.
    fn handle_game_event(&mut self, event: NetEvent) {
        match event.kind {
            EVENT_CAST => self.audio.play(crate::audio::Sound::Cast),
            EVENT_PROJECTILE_SPAWN => {
                self.audio.play(crate::audio::Sound::Attack);
                let direction = FixVec2::new(
                    Fix::from_f32(f32::from(event.dx) / 32_767.0),
                    Fix::from_f32(f32::from(event.dy) / 32_767.0),
                );
                let position = dequantize_position(event.x, event.y);
                let end = dequantize_position(event.x2, event.y2);
                let total = position.distance(end).to_f32();
                self.projectiles.push(ProjectileFx {
                    team: event.team,
                    position,
                    direction,
                    speed: f32::from(event.value),
                    radius: f32::from(event.aux),
                    remaining: total,
                    arc: f32::from(event.arc) / 16.0,
                    total,
                    travelled: 0.0,
                });
                if self.projectiles.len() > 32 {
                    self.projectiles.remove(0);
                }
            }
            EVENT_UNIT_DIED => {
                self.audio.play(crate::audio::Sound::Death);
                let text = if event.target_slot == u8::MAX {
                    String::from("Unit destroyed")
                } else {
                    let team = if event.team == 0 { "Blue" } else { "Red" };
                    format!(
                        "{team} hero {} eliminated",
                        u16::from(event.target_slot) + 1
                    )
                };
                self.kill_feed.push_back(KillFeedEntry { life: 600, text });
                if self.kill_feed.len() > 6 {
                    self.kill_feed.pop_front();
                }
            }
            EVENT_STRUCTURE_DESTROYED => {
                self.audio.play(crate::audio::Sound::Objective);
                self.kill_feed.push_back(KillFeedEntry {
                    life: 600,
                    text: String::from("Structure destroyed"),
                });
                if self.kill_feed.len() > 6 {
                    self.kill_feed.pop_front();
                }
            }
            EVENT_MATCH_ENDED => {
                self.audio.play(crate::audio::Sound::Ui);
                let text = if event.team == 0 {
                    String::from("Blue team wins")
                } else {
                    String::from("Red team wins")
                };
                self.kill_feed.push_back(KillFeedEntry { life: 600, text });
            }
            _ => {}
        }
    }

    fn update_effects(&mut self, frame_ms: f32) {
        let dt = frame_ms.max(0.0) / 1_000.0;
        for projectile in &mut self.projectiles {
            projectile.position += projectile.direction * Fix::from_f32(projectile.speed * dt);
            projectile.remaining -= projectile.speed * dt;
            projectile.travelled += projectile.speed * dt;
        }
        self.projectiles
            .retain(|projectile| projectile.remaining > 0.0);
        for entry in &mut self.kill_feed {
            entry.life = entry.life.saturating_sub(1);
        }
        self.kill_feed.retain(|entry| entry.life > 0);
    }
}

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
    use super::{
        Camera, Fix, FixVec2, InputFocus, Team, UnitKind, Vec2, Vec3, cursor_ground, kind_from_u8,
        team_from_u8,
    };

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
}
