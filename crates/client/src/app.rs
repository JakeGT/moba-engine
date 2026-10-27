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
    frame_log_path: Option<PathBuf>,
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
        let mut app = Self {
            renderer: None,
            simulation: None,
            network: None,
            local_audio: None,
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
            frame_log,
            frame_log_path,
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
        let config = RenderConfig {
            validation,
            max_instances: 1_024,
            vsync: std::env::var("MOBA_VSYNC").map_or(true, |value| value != "0"),
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
        if let Some(samples) = self.frame_log.as_mut()
            && samples.len() < 10_000
        {
            samples.push(raw_ms);
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
                            }
                            ClientEvent::Snapshot { .. } => {}
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
                        let tint = map_theme(network.client.map().theme).0;
                        let scene =
                            Scene::with_vision(camera, units, vision).with_terrain_tint(tint);
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
                        push_hero_rig(
                            units,
                            position.0,
                            *team,
                            fraction,
                            center,
                            simulation.current_tick().get() as f32 / 30.0,
                            entity.index() as f32,
                            0.0,
                            1.0,
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
                    let tint = map_theme(simulation.content().map.theme.id()).0;
                    let scene = Scene::new(camera, units).with_terrain_tint(tint);
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

mod launch;
mod scene_build;

pub(crate) use launch::{
    capture_and_exit, connect_network, frames_is_capture, parse_demo_move, run,
};
pub(crate) use scene_build::{
    map_theme, network_camera, push_hero_rig, push_unit, rebuild_network_units,
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
