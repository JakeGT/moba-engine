//! Game client: rendering, prediction, interpolation and UI.
//!
//! Milestone M3 renders a demo scene (terrain + 500 instanced units) through the Vulkan
//! renderer. Simulation and networking attach in M4/M5.
//!
//! Environment:
//! - `MOBA_SMOKE_FRAMES=<n>` exits after `n` frames (CI smoke runs set this).
//! - `MOBA_VULKAN_VALIDATION=required|off` controls the Khronos validation layer.

#![forbid(unsafe_code)]

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use moba_core::math::{Vec3, Vec4};
use moba_game::Team;
use moba_platform::{App, Frame, FrameOutcome, Key, Window, WindowConfig, run};
use moba_render::{
    Camera, CapturedFrame, RenderConfig, Renderer, Scene, UnitInstance, ValidationMode,
};
use moba_sim::{SimConfig, Simulation, UnitKind};

struct ClientApp {
    renderer: Option<Renderer>,
    simulation: Simulation,
    units: Vec<UnitInstance>,
    frames: u64,
    quit_after_frames: u64,
    capture_path: Option<PathBuf>,
    capture_frame: u64,
    error: Arc<Mutex<Option<String>>>,
}

impl ClientApp {
    fn new(
        quit_after_frames: u64,
        capture_path: Option<PathBuf>,
        capture_frame: u64,
        error: Arc<Mutex<Option<String>>>,
    ) -> Self {
        let mut simulation = Simulation::new(SimConfig::default());
        let warmup: u32 = std::env::var("MOBA_WARMUP_TICKS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        for _ in 0..warmup {
            simulation.tick_with_bots();
        }
        simulation.clear_events();
        Self {
            renderer: None,
            simulation,
            units: Vec::with_capacity(moba_sim::MAX_UNITS),
            frames: 0,
            quit_after_frames,
            capture_path,
            capture_frame,
            error,
        }
    }

    fn fail(&self, message: String) {
        if let Ok(mut slot) = self.error.lock() {
            if slot.is_none() {
                *slot = Some(message);
            }
        }
    }

    fn rebuild_units(&mut self) {
        self.units.clear();
        let world = self.simulation.world();
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
            let health_fraction = health.current as f32 / health.max as f32;
            let brightness = 0.55 + 0.45 * health_fraction;
            color.x *= brightness;
            color.y *= brightness;
            color.z *= brightness;
            let world_position = Vec3::new(
                position.0.x.to_f32() - 128.0,
                0.0,
                position.0.y.to_f32() - 128.0,
            );
            self.units
                .push(UnitInstance::at(world_position, height, color));
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
                window.set_title("MOBA — M3 instanced demo");
                self.renderer = Some(renderer);
            }
            Err(error) => {
                self.fail(format!("renderer initialisation failed: {error}"));
            }
        }
    }

    fn on_frame(&mut self, frame: &mut Frame<'_>) -> FrameOutcome {
        self.frames += 1;
        if self.renderer.is_some() {
            for _ in 0..frame.ticks() {
                self.simulation.tick_with_bots();
                self.simulation.clear_events();
            }
            self.rebuild_units();

            let [width, height] = frame.window().size();
            if width > 0 && height > 0 {
                let aspect = width as f32 / height as f32;
                let yaw = frame.elapsed_secs() as f32 * 0.035;
                let camera = Camera::orbit(Vec3::ZERO, 235.0, yaw, 0.92, aspect);
                let scene = Scene::new(camera, &self.units);
                if let Some(renderer) = self.renderer.as_mut() {
                    if let Some(path) = &self.capture_path {
                        if self.frames >= self.capture_frame {
                            match renderer.capture_frame(&scene) {
                                Ok(frame) => {
                                    if let Err(error) = write_ppm(path, &frame) {
                                        self.fail(format!("capture write failed: {error}"));
                                    }
                                }
                                Err(error) => self.fail(format!("capture failed: {error}")),
                            }
                            return FrameOutcome::Exit;
                        }
                    }
                    if let Err(error) = renderer.render(&scene) {
                        self.fail(format!("render failed: {error}"));
                        return FrameOutcome::Exit;
                    }
                }
            }
        } else if self.error.lock().is_ok_and(|slot| slot.is_some()) {
            return FrameOutcome::Exit;
        }

        if frame.input().was_key_pressed(Key::Escape) || self.frames >= self.quit_after_frames {
            return FrameOutcome::Exit;
        }
        FrameOutcome::Continue
    }
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
