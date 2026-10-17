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

#![forbid(unsafe_code)]

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moba_core::math::{Vec2, Vec3, Vec4};
use moba_core::{Fix, FixVec2};
use moba_game::{HEROES, Team};
use moba_net::protocol::{WireCommand, dequantize_position, quantize_position};
use moba_net::{ClientEvent, Interpolator, NetClient, PredictionState};
use moba_platform::{
    App, Frame, FrameOutcome, InputState, Key, MouseButton, Window, WindowConfig, run,
};
use moba_render::{
    Camera, CapturedFrame, RenderConfig, Renderer, Scene, UnitInstance, ValidationMode,
};
use moba_sim::{SimConfig, Simulation, UnitKind};

// Four ticks (133 ms) covers two lost 30 Hz snapshots plus 30 ms of jitter.
const SNAPSHOT_DELAY_TICKS: f32 = 4.0;

struct NetworkView {
    client: NetClient,
    prediction: PredictionState,
    interpolator: Interpolator,
    own_slot: u8,
    own_net_id: Option<u16>,
    last_snapshot_seq: u32,
    demo_move: Option<FixVec2>,
    demo_sent: bool,
}

struct ClientApp {
    renderer: Option<Renderer>,
    simulation: Option<Simulation>,
    network: Option<NetworkView>,
    units: Vec<UnitInstance>,
    frames: u64,
    quit_after_frames: u64,
    capture_path: Option<PathBuf>,
    capture_frame: u64,
    startup_error: Option<String>,
    error: Arc<Mutex<Option<String>>>,
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
            frames: 0,
            quit_after_frames,
            capture_path,
            capture_frame,
            startup_error: None,
            error,
        };

        match std::env::var("MOBA_SERVER") {
            Ok(spec) if !spec.is_empty() => match connect_network(&spec) {
                Ok(client) => {
                    let slot = client.player_slot();
                    let hero = usize::from(client.hero_pick()) % HEROES.len();
                    let prediction = PredictionState::new(HEROES[hero].move_speed_per_sec);
                    let interpolator =
                        Interpolator::new(SNAPSHOT_DELAY_TICKS, f32::from(client.tick_rate()));
                    app.network = Some(NetworkView {
                        client,
                        prediction,
                        interpolator,
                        own_slot: slot,
                        own_net_id: None,
                        last_snapshot_seq: 0,
                        demo_move: parse_demo_move(),
                        demo_sent: false,
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
                    format!(
                        "MOBA — online slot {} (right-click move, Q/W/E cast)",
                        network.own_slot
                    )
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
        {
            let ClientApp {
                network,
                simulation,
                units,
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
                            network.interpolator.push(snapshot.clone());
                            if let Some(own) = snapshot.entities.iter().find(|entity| {
                                entity.hero_slot == network.own_slot
                                    && entity.team == network.client.team().index() as u8
                            }) {
                                network.own_net_id = Some(own.net_id);
                                network.prediction.set_dead(own.is_dead());
                                network.prediction.reconcile(own.position());
                                if let Some(target) =
                                    network.demo_move.filter(|_| !network.demo_sent)
                                {
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
                    for _ in 0..frame.ticks() {
                        network.prediction.step();
                    }
                    rebuild_network_units(network, units);
                    if width > 0 && height > 0 {
                        let aspect = width as f32 / height as f32;
                        let camera = network_camera(network, aspect);
                        handle_network_input(network, frame.input(), &camera, width, height);
                        let scene = Scene::new(camera, units);
                        if let Some(renderer) = renderer.as_mut() {
                            if frames_is_capture(*frames, *capture_frame, capture_path.as_deref()) {
                                capture_and_exit(
                                    renderer,
                                    &scene,
                                    capture_path.as_deref(),
                                    &mut failure,
                                );
                                exit_after_capture = true;
                            } else if let Err(error) = renderer.render(&scene) {
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
                    push_unit(units, position.0, *kind, *team, fraction);
                }
                if width > 0 && height > 0 {
                    let aspect = width as f32 / height as f32;
                    let yaw = frame.elapsed_secs() as f32 * 0.035;
                    let camera = Camera::orbit(Vec3::ZERO, 235.0, yaw, 0.92, aspect);
                    let scene = Scene::new(camera, units);
                    if let Some(renderer) = renderer.as_mut() {
                        if frames_is_capture(*frames, *capture_frame, capture_path.as_deref()) {
                            capture_and_exit(
                                renderer,
                                &scene,
                                capture_path.as_deref(),
                                &mut failure,
                            );
                            exit_after_capture = true;
                        } else if let Err(error) = renderer.render(&scene) {
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
    capture_path: Option<&Path>,
    failure: &mut Option<String>,
) {
    let Some(path) = capture_path else {
        return;
    };
    match renderer.capture_frame(scene) {
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

fn connect_network(spec: &str) -> Result<NetClient, String> {
    if spec == "local" {
        let mut server = moba_net::NetServer::bind(moba_net::ServerConfig {
            bind: "127.0.0.1:0".parse().expect("loopback address"),
            sim: SimConfig::default(),
            snapshot_interval: 1,
            max_clients: 10,
        })
        .map_err(|error| format!("local server failed: {error}"))?;
        let addr = server
            .local_addr()
            .map_err(|error| format!("local server address: {error}"))?;
        std::thread::spawn(move || {
            let _ = server.run_for(Duration::from_secs(3_600));
        });
        NetClient::connect(addr, Duration::from_secs(5))
            .map_err(|error| format!("local connection failed: {error}"))
    } else {
        let addr = spec
            .parse()
            .map_err(|error| format!("invalid MOBA_SERVER {spec}: {error}"))?;
        NetClient::connect(addr, Duration::from_secs(5))
            .map_err(|error| format!("connection to {spec} failed: {error}"))
    }
}

fn rebuild_network_units(network: &NetworkView, units: &mut Vec<UnitInstance>) {
    units.clear();
    let Some(latest) = network.interpolator.latest() else {
        return;
    };
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
        let position = if Some(entity.net_id) == network.own_net_id {
            network.prediction.position()
        } else {
            network
                .interpolator
                .position(entity.net_id, render_tick)
                .unwrap_or_else(|| dequantize_position(entity.x, entity.y))
        };
        let fraction = if entity.max_health == 0 {
            1.0
        } else {
            f32::from(entity.health) / f32::from(entity.max_health)
        };
        push_unit(units, position, kind, team, fraction);
    }
}

fn push_unit(
    units: &mut Vec<UnitInstance>,
    position: FixVec2,
    kind: UnitKind,
    team: Team,
    health_fraction: f32,
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

fn network_camera(network: &NetworkView, aspect: f32) -> Camera {
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
) {
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
