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

#![forbid(unsafe_code)]

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moba_core::math::{Mat4, Quat, Vec2, Vec3, Vec4};
use moba_core::{Fix, FixVec2};
use moba_game::{
    EMPTY_ITEM, LEVEL_CAP, MELEE_MINION, NEXUS, RANGED_MINION, TOWER, Team, xp_to_next,
};
use moba_net::protocol::{MatchPhase, WireCommand, dequantize_position, quantize_position};
use moba_net::{ClientEvent, Interpolator, NetClient, PredictionState};
use moba_platform::{
    App, Frame, FrameOutcome, InputState, Key, MouseButton, Window, WindowConfig, run,
};
use moba_render::{
    Camera, CapturedFrame, RenderConfig, Renderer, Scene, UiDrawList, UnitInstance, ValidationMode,
    VisionCircles,
};
use moba_sim::{SimConfig, Simulation, UnitKind};

// Four ticks (133 ms) covers two lost 30 Hz snapshots plus 30 ms of jitter.
const SNAPSHOT_DELAY_TICKS: f32 = 4.0;

#[allow(clippy::struct_excessive_bools)] // Mode flags are independent UI/network state.
struct NetworkView {
    client: NetClient,
    prediction: PredictionState,
    interpolator: Interpolator,
    own_slot: u8,
    own_net_id: Option<u16>,
    last_snapshot_seq: u32,
    phase: MatchPhase,
    countdown: u16,
    spectator: bool,
    ready: bool,
    ready_count: u8,
    player_count: u8,
    last_tick: u32,
    demo_move: Option<FixVec2>,
    demo_sent: bool,
    demo_buy: Option<u8>,
    demo_bought: bool,
}

struct ClientApp {
    renderer: Option<Renderer>,
    simulation: Option<Simulation>,
    network: Option<NetworkView>,
    units: Vec<UnitInstance>,
    ui: UiDrawList,
    scoreboard_visible: bool,
    shop_visible: bool,
    frames: u64,
    quit_after_frames: u64,
    capture_path: Option<PathBuf>,
    capture_frame: u64,
    startup_error: Option<String>,
    error: Arc<Mutex<Option<String>>>,
    last_frame: Instant,
    frame_ms: f32,
    fps: f32,
}

impl ClientApp {
    fn new(
        quit_after_frames: u64,
        capture_path: Option<PathBuf>,
        capture_frame: u64,
        error: Arc<Mutex<Option<String>>>,
    ) -> Self {
        let mut app = Self {
            renderer: None,
            simulation: None,
            network: None,
            units: Vec::with_capacity(moba_sim::MAX_UNITS),
            ui: UiDrawList::new(),
            scoreboard_visible: std::env::var_os("MOBA_SCOREBOARD").is_some(),
            shop_visible: std::env::var_os("MOBA_SHOP").is_some(),
            frames: 0,
            quit_after_frames,
            capture_path,
            capture_frame,
            startup_error: None,
            error,
            last_frame: Instant::now(),
            frame_ms: 0.0,
            fps: 0.0,
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
                        handle_network_input(
                            network,
                            frame.input(),
                            &camera,
                            width,
                            height,
                            shop_visible,
                        );
                        if frame.input().was_key_pressed(Key::Tab) {
                            *scoreboard_visible = !*scoreboard_visible;
                        }
                        build_network_hud(
                            ui,
                            network,
                            *scoreboard_visible,
                            *shop_visible,
                            &camera,
                            width,
                            height,
                            frame_ms,
                            fps,
                        );
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
                for _ in 0..frame.ticks() {
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
                    push_unit(units, position.0, *kind, *team, fraction, 0);
                }
                if width > 0 && height > 0 {
                    let aspect = width as f32 / height as f32;
                    let yaw = frame.elapsed_secs() as f32 * 0.035;
                    build_local_hud(ui, simulation, width, height, frame_ms, fps);
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
    for wall in wire_map.walls.iter().take(usize::from(wire_map.wall_count)) {
        let a = Vec2::new(f32::from(wall.a[0]), f32::from(wall.a[1]));
        let b = Vec2::new(f32::from(wall.b[0]), f32::from(wall.b[1]));
        let direction = b - a;
        let length = direction.length().max(0.5);
        let angle = direction.y.atan2(direction.x);
        let center = (a + b) * 0.5;
        let transform = Mat4::from_scale_rotation_translation(
            Vec3::new(length, 4.0, 2.5),
            Quat::from_rotation_y(-angle),
            Vec3::new(center.x - 128.0, 2.0, center.y - 128.0),
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
        let position = rendered_entity_position(network, entity, render_tick);
        let fraction = if entity.max_health == 0 {
            1.0
        } else {
            f32::from(entity.health) / f32::from(entity.max_health)
        };
        push_unit(units, position, kind, team, fraction, entity.flags);
    }
}

fn push_unit(
    units: &mut Vec<UnitInstance>,
    position: FixVec2,
    kind: UnitKind,
    team: Team,
    health_fraction: f32,
    status: u8,
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
    if status & moba_net::protocol::ENTITY_FLAG_SHIELD != 0 {
        color = color * 0.65 + Vec4::new(0.15, 0.65, 0.95, 1.0) * 0.35;
    }
    if status & moba_net::protocol::ENTITY_FLAG_SLOW != 0 {
        color = color * 0.75 + Vec4::new(0.35, 0.2, 0.75, 1.0) * 0.25;
    }
    if status & moba_net::protocol::ENTITY_FLAG_ROOT != 0 {
        color = color * 0.7 + Vec4::new(0.9, 0.75, 0.18, 1.0) * 0.3;
    }
    if status & moba_net::protocol::ENTITY_FLAG_SILENCE != 0 {
        color = color * 0.7 + Vec4::new(0.55, 0.55, 0.6, 1.0) * 0.3;
    }
    if status & moba_net::protocol::ENTITY_FLAG_BURN != 0 {
        color = color * 0.7 + Vec4::new(1.0, 0.42, 0.1, 1.0) * 0.3;
    }
    let world_position = Vec3::new(
        position.x.to_f32() - 128.0,
        0.0,
        position.y.to_f32() - 128.0,
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
        return Camera::orbit(Vec3::ZERO, 240.0, -0.7 + elapsed * 0.015, 0.92, aspect);
    }
    if std::env::var("MOBA_CAMERA").is_ok_and(|value| value == "overview") {
        return Camera::orbit(Vec3::ZERO, 240.0, -0.7, 0.92, aspect);
    }
    let predicted = network.prediction.position();
    let center = Vec3::new(
        predicted.x.to_f32() - 128.0,
        0.0,
        predicted.y.to_f32() - 128.0,
    );
    Camera::orbit(center, 46.0, -0.7, 0.95, aspect)
}

fn handle_network_input(
    network: &mut NetworkView,
    input: &InputState,
    camera: &Camera,
    width: u32,
    height: u32,
    shop: &mut bool,
) {
    if network.spectator {
        return;
    }
    if input.was_key_pressed(Key::R) && network.phase == MatchPhase::Lobby {
        let ready = !network.ready;
        if network
            .client
            .send_command(WireCommand::Ready { ready })
            .is_ok()
        {
            network.ready = ready;
        }
    }
    if input.was_key_pressed(Key::F5) && network.phase == MatchPhase::Playing {
        let _ = network.client.send_command(WireCommand::Surrender);
    }
    if input.was_key_pressed(Key::B) {
        *shop = !*shop;
    }
    if *shop {
        for (key, item) in [
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
                let _ = network.client.send_command(WireCommand::BuyItem { item });
            }
        }
        return;
    }
    if network.phase != MatchPhase::Playing {
        return;
    }
    let Some(cursor) = input.cursor() else {
        return;
    };
    let Some(ground) = cursor_ground(camera, cursor, width, height) else {
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
    if input.was_mouse_pressed(MouseButton::Left) {
        if let Some(target) = nearest_enemy(network, ground) {
            let _ = network
                .client
                .send_command(WireCommand::Attack { net_id: target });
        }
    }
    for (key, slot) in [(Key::Q, 0u8), (Key::W, 1), (Key::E, 2)] {
        if input.was_key_pressed(key) {
            let (x, y) = quantize_position(ground);
            let _ = network
                .client
                .send_command(WireCommand::CastAbility { slot, x, y });
        }
    }
    if input.was_key_pressed(Key::Space) {
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

fn cursor_ground(camera: &Camera, cursor: Vec2, width: u32, height: u32) -> Option<FixVec2> {
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
        Fix::from_f32(hit.x + 128.0),
        Fix::from_f32(hit.z + 128.0),
    ))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
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
    run(config, app)?;
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
fn rendered_entity_position(
    network: &NetworkView,
    entity: &moba_net::NetEntity,
    render_tick: f32,
) -> FixVec2 {
    if Some(entity.net_id) == network.own_net_id {
        network.prediction.position()
    } else {
        network
            .interpolator
            .position(entity.net_id, render_tick)
            .unwrap_or_else(|| dequantize_position(entity.x, entity.y))
    }
}

fn project_to_screen(camera: &Camera, world: Vec3, width: u32, height: u32) -> Option<Vec2> {
    let clip = camera.view_proj * Vec4::new(world.x, world.y, world.z, 1.0);
    if clip.w <= 0.0001 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if !(0.0..=1.0).contains(&ndc.z) {
        return None;
    }
    let x = (ndc.x * 0.5 + 0.5) * width as f32;
    let y = (1.0 - ndc.y) * 0.5 * height as f32;
    Some(Vec2::new(x, y))
}

fn unit_hud_height(kind: u8) -> f32 {
    match kind {
        0 => 6.2,
        1 => 3.2,
        2 => 2.8,
        3 => 10.5,
        4 => 17.0,
        _ => 3.0,
    }
}

fn draw_health_bars(
    ui: &mut UiDrawList,
    network: &NetworkView,
    camera: &Camera,
    width: u32,
    height: u32,
) {
    let Some(snapshot) = network.client.snapshot() else {
        return;
    };
    let render_tick = network.interpolator.render_tick(Instant::now());
    let width_f = width as f32;
    for entity in &snapshot.entities {
        if entity.is_dead() || (entity.kind != 0 && entity.kind < 3) {
            continue;
        }
        let position = rendered_entity_position(network, entity, render_tick);
        let world = Vec3::new(
            position.x.to_f32() - 128.0,
            unit_hud_height(entity.kind),
            position.y.to_f32() - 128.0,
        );
        let Some(screen) = project_to_screen(camera, world, width, height) else {
            continue;
        };
        let bar_width = if entity.kind == 0 { 46.0 } else { 64.0 };
        let bar_height = if entity.kind == 0 { 5.0 } else { 7.0 };
        let x = (screen.x - bar_width * 0.5).clamp(4.0, (width_f - bar_width - 4.0).max(4.0));
        let y = (screen.y - 8.0).clamp(4.0, height as f32 - 4.0);
        let fraction = if entity.max_health == 0 {
            0.0
        } else {
            f32::from(entity.health) / f32::from(entity.max_health)
        };
        let team_color = if entity.team == 0 {
            Vec4::new(0.35, 0.62, 1.0, 1.0)
        } else {
            Vec4::new(1.0, 0.42, 0.32, 1.0)
        };
        ui.rect(
            x,
            y,
            bar_width,
            bar_height,
            Vec4::new(0.02, 0.03, 0.04, 0.85),
        );
        ui.rect(
            x,
            y,
            bar_width * fraction.clamp(0.0, 1.0),
            bar_height,
            Vec4::new(0.2, 0.85, 0.38, 1.0),
        );
        ui.outline(x, y, bar_width, bar_height, 1.0, team_color);
        if entity.kind == 0 {
            let mut icon_x = x;
            for (flag, color) in [
                (
                    moba_net::protocol::ENTITY_FLAG_SHIELD,
                    Vec4::new(0.3, 0.75, 1.0, 1.0),
                ),
                (
                    moba_net::protocol::ENTITY_FLAG_SLOW,
                    Vec4::new(0.6, 0.4, 0.95, 1.0),
                ),
                (
                    moba_net::protocol::ENTITY_FLAG_ROOT,
                    Vec4::new(0.95, 0.8, 0.25, 1.0),
                ),
                (
                    moba_net::protocol::ENTITY_FLAG_SILENCE,
                    Vec4::new(0.7, 0.7, 0.75, 1.0),
                ),
                (
                    moba_net::protocol::ENTITY_FLAG_BURN,
                    Vec4::new(1.0, 0.45, 0.12, 1.0),
                ),
            ] {
                if entity.flags & flag != 0 {
                    ui.rect(icon_x, y + bar_height + 2.0, 4.0, 3.0, color);
                    icon_x += 6.0;
                }
            }
        }
        if Some(entity.net_id) == network.own_net_id {
            ui.rect(
                x + bar_width * 0.5 - 5.0,
                y - 5.0,
                10.0,
                2.0,
                Vec4::new(0.95, 0.78, 0.3, 1.0),
            );
        }
    }
}

fn draw_minimap(ui: &mut UiDrawList, network: &NetworkView, width: u32, height: u32) {
    let Some(snapshot) = network.client.snapshot() else {
        return;
    };
    let size = 188.0;
    let margin = 24.0;
    let x = width as f32 - size - margin;
    let y = height as f32 - size - margin;
    ui.panel(
        x,
        y,
        size,
        size,
        Vec4::new(0.01, 0.02, 0.035, 0.82),
        Vec4::new(0.22, 0.42, 0.58, 0.9),
    );
    let inner = size - 20.0;
    let left = x + 10.0;
    let top = y + 10.0;
    ui.rect(left, top, inner, inner, Vec4::new(0.04, 0.08, 0.06, 0.9));
    let map = |position: FixVec2| {
        Vec2::new(
            left + position.x.to_f32() / 256.0 * inner,
            top + position.y.to_f32() / 256.0 * inner,
        )
    };
    let lane_color = Vec4::new(0.22, 0.38, 0.28, 0.85);
    let wire_map = network.client.map();
    for lane in wire_map.lanes.iter().take(usize::from(wire_map.lane_count)) {
        let count = usize::from(lane.point_count).min(lane.points.len());
        for pair in lane.points[..count].windows(2) {
            let a = map(FixVec2::from_ints(
                i32::from(pair[0][0]),
                i32::from(pair[0][1]),
            ));
            let b = map(FixVec2::from_ints(
                i32::from(pair[1][0]),
                i32::from(pair[1][1]),
            ));
            ui.line(a.x, a.y, b.x, b.y, 1.5, lane_color);
        }
    }
    let wall_color = Vec4::new(0.55, 0.42, 0.28, 1.0);
    for wall in wire_map.walls.iter().take(usize::from(wire_map.wall_count)) {
        let a = map(FixVec2::from_ints(
            i32::from(wall.a[0]),
            i32::from(wall.a[1]),
        ));
        let b = map(FixVec2::from_ints(
            i32::from(wall.b[0]),
            i32::from(wall.b[1]),
        ));
        ui.line(a.x, a.y, b.x, b.y, 2.0, wall_color);
    }
    for entity in &snapshot.entities {
        if entity.is_dead() {
            continue;
        }
        let position = dequantize_position(entity.x, entity.y);
        let point = map(position);
        let team_color = if entity.team == 0 {
            Vec4::new(0.35, 0.62, 1.0, 1.0)
        } else {
            Vec4::new(1.0, 0.42, 0.32, 1.0)
        };
        let dot = if entity.kind == 0 || entity.kind >= 3 {
            5.0
        } else {
            3.0
        };
        ui.rect(
            point.x - dot * 0.5,
            point.y - dot * 0.5,
            dot,
            dot,
            team_color,
        );
        if Some(entity.net_id) == network.own_net_id {
            ui.outline(
                point.x - 4.5,
                point.y - 4.5,
                9.0,
                9.0,
                1.5,
                Vec4::new(0.95, 0.78, 0.3, 1.0),
            );
        }
    }
    ui.text(
        x + 10.0,
        y + size - 14.0,
        1.5,
        Vec4::new(0.55, 0.68, 0.78, 1.0),
        "MAP",
    );
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn draw_perf(ui: &mut UiDrawList, width: f32, frame_ms: f32, fps: f32) {
    let label = format!("{fps:.0} FPS   {frame_ms:.1} MS");
    ui.text(
        width - 24.0 - UiDrawList::text_width(&label, 1.8),
        54.0,
        1.8,
        Vec4::new(0.55, 0.62, 0.72, 1.0),
        &label,
    );
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // HUD composition is one pass.
fn build_network_hud(
    ui: &mut UiDrawList,
    network: &NetworkView,
    scoreboard: bool,
    shop: bool,
    camera: &Camera,
    width: u32,
    height: u32,
    frame_ms: f32,
    fps: f32,
) {
    ui.clear();
    if width == 0 || height == 0 {
        return;
    }
    let bg = Vec4::new(0.015, 0.025, 0.045, 0.82);
    let border = Vec4::new(0.22, 0.42, 0.58, 0.9);
    let text = Vec4::new(0.88, 0.94, 1.0, 1.0);
    let muted = Vec4::new(0.55, 0.68, 0.78, 1.0);
    let health = Vec4::new(0.18, 0.82, 0.36, 1.0);
    let mana = Vec4::new(0.22, 0.50, 1.0, 1.0);
    let track = Vec4::new(0.055, 0.075, 0.095, 1.0);
    let width_f = width as f32;
    let height_f = height as f32;
    draw_perf(ui, width_f, frame_ms, fps);

    draw_health_bars(ui, network, camera, width, height);
    let snapshot = network.client.snapshot();
    let own = network
        .own_net_id
        .and_then(|id| snapshot.and_then(|state| state.get(id)))
        .or_else(|| {
            snapshot.and_then(|state| {
                state.entities.iter().find(|entity| {
                    entity.hero_slot == network.own_slot
                        && entity.team == network.client.team().index() as u8
                })
            })
        });
    let hero = network
        .client
        .heroes()
        .get(usize::from(network.client.hero_pick()))
        .copied()
        .unwrap_or_default();
    let hero_name = {
        let name = hero.name_str();
        if name.is_empty() { "UNKNOWN" } else { name }
    };

    // Match timer and team score.
    let tick = snapshot.map_or(0, |state| state.tick);
    let seconds = tick / 30;
    let timer = format!("{:02}:{:02}", seconds / 60, seconds % 60);
    let blue = snapshot.map_or(0, |state| state.blue_score);
    let red = snapshot.map_or(0, |state| state.red_score);
    let timer_width = 360.0;
    ui.panel(
        (width_f - timer_width) * 0.5,
        14.0,
        timer_width,
        72.0,
        bg,
        border,
    );
    ui.text_shadowed(
        (width_f - UiDrawList::text_width(&timer, 3.5)) * 0.5,
        20.0,
        3.5,
        text,
        &timer,
    );
    ui.text(
        (width_f - timer_width) * 0.5 + 18.0,
        52.0,
        2.0,
        Vec4::new(0.4, 0.65, 1.0, 1.0),
        &format!("BLUE {blue}"),
    );
    ui.text(
        width_f.midpoint(timer_width) - 18.0 - UiDrawList::text_width(&format!("RED {red}"), 2.0),
        52.0,
        2.0,
        Vec4::new(1.0, 0.42, 0.32, 1.0),
        &format!("RED {red}"),
    );

    // Connection/status block.
    ui.panel(18.0, 18.0, 250.0, 82.0, bg, border);
    ui.text(
        30.0,
        28.0,
        2.5,
        text,
        if network.spectator {
            "MOBA SPECTATOR"
        } else {
            "MOBA ONLINE"
        },
    );
    let stats = network.client.stats();
    let ping = stats.rtt_ms.unwrap_or(0);
    ui.text(
        30.0,
        54.0,
        2.0,
        muted,
        &format!(
            "SLOT {}   PING {} MS",
            if network.spectator {
                String::from("--")
            } else {
                network.own_slot.to_string()
            },
            ping
        ),
    );
    ui.text(
        30.0,
        74.0,
        2.0,
        muted,
        &format!("TICK {}   {} SNAPSHOTS", tick, stats.snapshots),
    );

    if !network.spectator {
        ui.text(30.0, 108.0, 1.8, muted, "R READY   F5 SURRENDER");
    }
    // Controls.
    let controls = if network.spectator {
        "SPECTATOR VIEW   TAB SCOREBOARD"
    } else {
        "RMB MOVE   LMB ATTACK   Q/W/E CAST"
    };
    ui.text(
        width_f - 24.0 - UiDrawList::text_width(controls, 2.0),
        24.0,
        2.0,
        muted,
        controls,
    );

    let progress = own.map_or(moba_net::protocol::NetProgress::default(), |entity| {
        entity.progress
    });
    if !network.spectator {
        // Hero panel.
        let panel_w = 452.0;
        let panel_h = 196.0;
        let x = 24.0;
        let y = height_f - panel_h - 24.0;
        ui.panel(x, y, panel_w, panel_h, bg, border);
        let level = progress.level.max(1);
        let xp_needed = if level >= LEVEL_CAP {
            0
        } else {
            xp_to_next(level)
        };
        ui.text_shadowed(x + 14.0, y + 12.0, 3.0, text, hero_name);
        ui.text(
            x + 14.0 + UiDrawList::text_width(hero_name, 3.0) + 14.0,
            y + 16.0,
            2.0,
            Vec4::new(0.95, 0.78, 0.3, 1.0),
            &format!("LV {level}"),
        );
        ui.text(
            x + 250.0,
            y + 16.0,
            2.0,
            muted,
            &format!(
                "TEAM {}   {}G",
                if network.client.team() == Team::Blue {
                    "BLUE"
                } else {
                    "RED"
                },
                progress.gold
            ),
        );

        let (hp, max_hp, mp, max_mp) = own.map_or((0, 1, 0, 1), |entity| {
            (
                i32::from(entity.health),
                i32::from(entity.max_health).max(1),
                i32::from(entity.mana),
                i32::from(entity.max_mana).max(1),
            )
        });
        ui.text(x + 14.0, y + 42.0, 2.0, muted, &format!("HP {hp}/{max_hp}"));
        ui.bar(
            x + 14.0,
            y + 58.0,
            panel_w - 28.0,
            14.0,
            hp as f32 / max_hp as f32,
            health,
            track,
            border,
        );
        ui.text(x + 14.0, y + 78.0, 2.0, muted, &format!("MP {mp}/{max_mp}"));
        ui.bar(
            x + 14.0,
            y + 94.0,
            panel_w - 28.0,
            10.0,
            mp as f32 / max_mp as f32,
            mana,
            track,
            border,
        );
        let xp_fraction = if xp_needed == 0 {
            1.0
        } else {
            f32::from(progress.xp) / f32::from(u16::try_from(xp_needed).unwrap_or(u16::MAX))
        };
        let xp_label = if xp_needed == 0 {
            String::from("XP MAX")
        } else {
            format!("XP {}/{}", progress.xp, xp_needed)
        };
        ui.text(x + 14.0, y + 112.0, 1.8, muted, &xp_label);
        ui.bar(
            x + 14.0,
            y + 128.0,
            panel_w - 28.0,
            8.0,
            xp_fraction,
            Vec4::new(0.85, 0.68, 0.25, 1.0),
            track,
            border,
        );

        let cooldowns = own.map_or([0; 3], |entity| entity.cooldowns);
        for slot in 0..3 {
            let box_size = 38.0;
            let bx = x + 14.0 + slot as f32 * 48.0;
            let by = y + 146.0;
            let key = ["Q", "W", "E"][slot];
            ui.rect(bx, by, box_size, box_size, track);
            ui.outline(bx, by, box_size, box_size, 1.0, border);
            ui.text(bx + 8.0, by + 8.0, 2.5, text, key);
            let remaining = cooldowns[slot];
            let max_cooldown = network
                .client
                .abilities()
                .get(usize::from(hero.abilities[slot]))
                .map_or(1, |ability| {
                    u16::try_from(ability.cooldown_ticks).unwrap_or(u16::MAX)
                })
                .max(1);
            if remaining > 0 {
                let fraction = f32::from(remaining) / f32::from(max_cooldown);
                ui.rect(
                    bx,
                    by,
                    box_size,
                    box_size * fraction.clamp(0.0, 1.0),
                    Vec4::new(0.0, 0.0, 0.0, 0.62),
                );
                let seconds = f32::from(remaining) / 30.0;
                ui.text(bx + 4.0, by + 24.0, 1.8, text, &format!("{seconds:.1}"));
            }
        }
        // Inventory.
        ui.text(x + 172.0, y + 148.0, 1.8, muted, "ITEMS");
        for slot in 0..6 {
            let bx = x + 172.0 + slot as f32 * 43.0;
            let by = y + 164.0;
            ui.rect(bx, by, 34.0, 26.0, track);
            ui.outline(bx, by, 34.0, 26.0, 1.0, border);
            let item = progress.items[slot];
            if item != EMPTY_ITEM {
                if let Some(definition) = network.client.items().get(usize::from(item)) {
                    let name = definition.name_str();
                    let short = name.get(..5).unwrap_or(name);
                    ui.text(bx + 2.0, by + 8.0, 1.4, text, short);
                }
            }
        }
    }
    draw_minimap(ui, network, width, height);
    if scoreboard {
        let panel_width = 620.0;
        let panel_height = 330.0;
        let sx = (width_f - panel_width) * 0.5;
        let sy = (height_f - panel_height) * 0.5;
        ui.panel(
            sx,
            sy,
            panel_width,
            panel_height,
            Vec4::new(0.01, 0.02, 0.035, 0.9),
            border,
        );
        ui.text_shadowed(sx + 24.0, sy + 18.0, 3.5, text, "SCOREBOARD");
        ui.text(
            sx + panel_width - 200.0,
            sy + 26.0,
            2.0,
            muted,
            "TAB TO CLOSE",
        );
        let mut rows = [(None, 0u8, 0u8, 0u16, 0u16, 0u8); 10];
        let mut count = 0usize;
        if let Some(state) = snapshot {
            for entity in state
                .entities
                .iter()
                .filter(|entity| entity.hero_slot != 0xFF)
            {
                if count >= rows.len() {
                    break;
                }
                rows[count] = (
                    Some(entity.team),
                    entity.hero_slot,
                    entity.hero_pick,
                    entity.health,
                    entity.max_health,
                    entity.progress.level,
                );
                count += 1;
            }
        }
        rows[..count].sort_unstable_by_key(|(team, slot, _, _, _, _)| (*team, *slot));
        for (index, (team, slot, pick, hp, max_hp, level)) in rows[..count].iter().enumerate() {
            let y = sy + 64.0 + index as f32 * 24.0;
            let color = if *team == Some(0) {
                Vec4::new(0.45, 0.7, 1.0, 1.0)
            } else {
                Vec4::new(1.0, 0.5, 0.4, 1.0)
            };
            let name = network
                .client
                .heroes()
                .get(usize::from(*pick))
                .map_or("UNKNOWN", |hero| {
                    let name = hero.name_str();
                    if name.is_empty() { "UNKNOWN" } else { name }
                });
            ui.text(
                sx + 32.0,
                y,
                2.0,
                color,
                &format!("SLOT {slot}  {name}  LV {level}"),
            );
            ui.text(sx + 400.0, y, 2.0, muted, &format!("{hp}/{max_hp} HP"));
        }
        ui.text(
            sx + 32.0,
            sy + panel_height - 34.0,
            2.0,
            muted,
            "ENEMY ROWS APPEAR ONLY WHILE VISIBLE",
        );
    }
    if shop && !network.spectator {
        let shop_w = 700.0;
        let shop_h = 390.0;
        let sx = (width_f - shop_w) * 0.5;
        let sy = ((height_f - shop_h) * 0.5 - 80.0).max(40.0);
        ui.panel(
            sx,
            sy,
            shop_w,
            shop_h,
            Vec4::new(0.01, 0.02, 0.035, 0.94),
            border,
        );
        ui.text_shadowed(sx + 24.0, sy + 18.0, 3.5, text, "SHOP");
        ui.text(
            sx + shop_w - 180.0,
            sy + 26.0,
            2.0,
            Vec4::new(0.95, 0.78, 0.3, 1.0),
            &format!("{} GOLD", progress.gold),
        );
        for (index, item) in network.client.items().iter().take(8).enumerate() {
            let row_y = sy + 62.0 + index as f32 * 36.0;
            let affordable = u32::from(progress.gold) >= item.cost;
            let name_color = if affordable {
                text
            } else {
                Vec4::new(0.45, 0.5, 0.58, 1.0)
            };
            ui.text(sx + 24.0, row_y, 2.0, muted, &format!("[{}]", index + 1));
            ui.text(sx + 70.0, row_y, 2.0, name_color, item.name_str());
            ui.text(
                sx + 300.0,
                row_y,
                2.0,
                if affordable {
                    Vec4::new(0.95, 0.78, 0.3, 1.0)
                } else {
                    Vec4::new(0.55, 0.4, 0.3, 1.0)
                },
                &format!("{}G", item.cost),
            );
            ui.text(sx + 390.0, row_y, 1.7, muted, &item_summary(item));
        }
        ui.text(
            sx + 24.0,
            sy + shop_h - 30.0,
            1.8,
            muted,
            "B CLOSE   NUMBER BUY   PURCHASES REQUIRE OWN NEXUS",
        );
    }
    if matches!(network.phase, MatchPhase::Lobby | MatchPhase::Countdown) {
        let seconds = u32::from(network.countdown).div_ceil(30);
        let (label, footer) = if network.spectator {
            (
                format!("SPECTATOR — MATCH STARTS IN {seconds}"),
                String::from("ALL UNITS VISIBLE"),
            )
        } else if network.phase == MatchPhase::Lobby {
            if network.ready {
                (
                    String::from("READY — WAITING FOR PLAYERS"),
                    format!(
                        "{}/{} PLAYERS READY",
                        network.ready_count, network.player_count
                    ),
                )
            } else {
                (
                    String::from("PRESS R TO READY"),
                    format!(
                        "{}/{} PLAYERS READY",
                        network.ready_count, network.player_count
                    ),
                )
            }
        } else {
            (
                format!("MATCH STARTS IN {seconds}"),
                String::from("BOTS FILL EMPTY SLOTS"),
            )
        };
        let panel_w = 560.0;
        let panel_h = 110.0;
        ui.panel(
            (width_f - panel_w) * 0.5,
            height_f * 0.35,
            panel_w,
            panel_h,
            bg,
            border,
        );
        ui.text_shadowed(
            (width_f - UiDrawList::text_width(&label, 4.0)) * 0.5,
            height_f * 0.35 + 26.0,
            4.0,
            text,
            &label,
        );
        ui.text(
            (width_f - UiDrawList::text_width(&footer, 2.0)) * 0.5,
            height_f * 0.35 + 72.0,
            2.0,
            muted,
            &footer,
        );
    }
    if let Some(outcome) = snapshot
        .map(|state| state.outcome)
        .filter(|outcome| *outcome != 0)
    {
        let winner = if outcome == 1 { Team::Blue } else { Team::Red };
        let victory = winner == network.client.team();
        let label = if victory { "VICTORY" } else { "DEFEAT" };
        let color = if victory {
            Vec4::new(0.35, 1.0, 0.5, 1.0)
        } else {
            Vec4::new(1.0, 0.35, 0.28, 1.0)
        };
        ui.rect(0.0, 0.0, width_f, height_f, Vec4::new(0.0, 0.0, 0.0, 0.35));
        ui.panel(
            (width_f - 460.0) * 0.5,
            height_f * 0.36,
            460.0,
            130.0,
            bg,
            border,
        );
        ui.text_shadowed(
            (width_f - UiDrawList::text_width(label, 7.0)) * 0.5,
            height_f * 0.36 + 24.0,
            7.0,
            color,
            label,
        );
        let seconds = u32::from(network.countdown).div_ceil(30);
        let footer = if network.phase == MatchPhase::Results {
            format!("NEXT MATCH IN {seconds}   /   ESC TO EXIT")
        } else {
            String::from("PRESS ESC TO EXIT")
        };
        ui.text(
            (width_f - UiDrawList::text_width(&footer, 2.0)) * 0.5,
            height_f * 0.36 + 92.0,
            2.0,
            muted,
            &footer,
        );
    }
    if own.is_some_and(|entity| entity.is_dead()) {
        ui.text_shadowed(
            (width_f - UiDrawList::text_width("RESPAWNING", 5.0)) * 0.5,
            height_f * 0.42,
            5.0,
            Vec4::new(1.0, 0.35, 0.25, 1.0),
            "RESPAWNING",
        );
    }
}

fn item_summary(item: &moba_net::protocol::WireItem) -> String {
    let mut parts = Vec::new();
    if item.health != 0 {
        parts.push(format!("+{} HP", item.health));
    }
    if item.mana != 0 {
        parts.push(format!("+{} MP", item.mana));
    }
    if item.attack_damage != 0 {
        parts.push(format!("+{} DMG", item.attack_damage));
    }
    if item.armor != 0 {
        parts.push(format!("+{} ARMOR", item.armor));
    }
    if item.magic_resist != 0 {
        parts.push(format!("+{} MR", item.magic_resist));
    }
    let move_speed = Fix::from_bits(item.move_speed_bits);
    if move_speed != Fix::ZERO {
        parts.push(format!("+{} MS", move_speed.to_f32()));
    }
    parts.join("  ")
}

fn build_local_hud(
    ui: &mut UiDrawList,
    simulation: &Simulation,
    width: u32,
    height: u32,
    frame_ms: f32,
    fps: f32,
) {
    ui.clear();
    draw_perf(ui, width as f32, frame_ms, fps);
    if width == 0 || height == 0 {
        return;
    }
    let bg = Vec4::new(0.015, 0.025, 0.045, 0.82);
    let border = Vec4::new(0.22, 0.42, 0.58, 0.9);
    let text = Vec4::new(0.88, 0.94, 1.0, 1.0);
    let muted = Vec4::new(0.55, 0.68, 0.78, 1.0);
    ui.panel(18.0, 18.0, 330.0, 112.0, bg, border);
    ui.text_shadowed(32.0, 30.0, 3.0, text, "LOCAL BOT PREVIEW");
    ui.text(
        32.0,
        64.0,
        2.0,
        muted,
        &format!(
            "TICK {}   WAVE {}   UNITS {}",
            simulation.current_tick().get(),
            simulation.wave(),
            simulation.unit_count()
        ),
    );
    ui.text(32.0, 88.0, 2.0, muted, "SET MOBA_SERVER TO PLAY ONLINE");
    ui.text(
        width as f32 - 24.0 - UiDrawList::text_width("OFFLINE", 2.0),
        height as f32 - 36.0,
        2.0,
        muted,
        "OFFLINE",
    );
}

fn build_vision(network: &NetworkView) -> VisionCircles {
    let mut vision = VisionCircles::default();
    if network.spectator {
        return vision;
    }
    let wire_map = network.client.map();
    for wall in wire_map.walls.iter().take(usize::from(wire_map.wall_count)) {
        vision.add_wall(
            f32::from(wall.a[0]) - 128.0,
            f32::from(wall.a[1]) - 128.0,
            f32::from(wall.b[0]) - 128.0,
            f32::from(wall.b[1]) - 128.0,
        );
    }
    let Some(snapshot) = network.client.snapshot() else {
        return vision;
    };
    let own_team = network.client.team().index() as u8;
    let add = |entity: &moba_net::NetEntity, vision: &mut VisionCircles| {
        if entity.team != own_team || entity.is_dead() {
            return;
        }
        let radius = vision_radius(network, entity);
        let position = dequantize_position(entity.x, entity.y);
        vision.add(
            position.x.to_f32() - 128.0,
            position.y.to_f32() - 128.0,
            radius,
        );
    };
    // Heroes and structures are the most useful fog anchors; minions fill the rest.
    for priority in [0u8, 1, 2] {
        for entity in &snapshot.entities {
            let kind_priority = match entity.kind {
                0 => 0,
                3 | 4 => 1,
                _ => 2,
            };
            if kind_priority == priority {
                add(entity, &mut vision);
            }
        }
    }
    vision
}

fn vision_radius(network: &NetworkView, entity: &moba_net::NetEntity) -> f32 {
    match entity.kind {
        0 => network
            .client
            .heroes()
            .get(usize::from(entity.hero_pick))
            .map_or(35.0, |hero| Fix::from_bits(hero.vision_bits).to_f32()),
        1 => MELEE_MINION.vision_range.to_f32(),
        2 => RANGED_MINION.vision_range.to_f32(),
        3 => TOWER.vision_range.to_f32(),
        4 => NEXUS.vision_range.to_f32(),
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centre_cursor_maps_to_camera_target() {
        let camera = Camera::orbit(Vec3::ZERO, 46.0, -0.7, 0.95, 16.0 / 9.0);
        let ground = cursor_ground(&camera, Vec2::new(640.0, 360.0), 1280, 720)
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
