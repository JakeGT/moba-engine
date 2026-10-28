//! Persisted client settings: key bindings, UI scale and rendering quality.

use std::path::{Path, PathBuf};

use moba_platform::Key;
use serde::{Deserialize, Serialize};

/// Default settings file name when `MOBA_SETTINGS` is not set.
pub(crate) const DEFAULT_PATH: &str = "moba-settings.toml";
/// UI scale bounds.
pub(crate) const UI_SCALE_MIN: f32 = 0.5;
/// UI scale bounds.
pub(crate) const UI_SCALE_MAX: f32 = 3.0;
/// Camera sensitivity bounds.
pub(crate) const SENSITIVITY_MIN: f32 = 0.25;
/// Camera sensitivity bounds.
pub(crate) const SENSITIVITY_MAX: f32 = 3.0;
/// Spectator zoom bounds.
pub(crate) const CAMERA_ZOOM_MIN: f32 = 70.0;
/// Spectator zoom bounds.
pub(crate) const CAMERA_ZOOM_MAX: f32 = 420.0;

/// Mouse button used for attack orders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MouseBinding {
    Left,
    Right,
}

/// A key binding name that can be parsed and validated.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Controls {
    pub(crate) move_up: String,
    pub(crate) move_down: String,
    pub(crate) move_left: String,
    pub(crate) move_right: String,
    pub(crate) cast_q: String,
    pub(crate) cast_w: String,
    pub(crate) cast_e: String,
    pub(crate) stop: String,
    pub(crate) shop: String,
    pub(crate) scoreboard: String,
    pub(crate) ready: String,
    pub(crate) surrender: String,
    pub(crate) attack_button: String,
    pub(crate) camera_left: String,
    pub(crate) camera_right: String,
    pub(crate) camera_up: String,
    pub(crate) camera_down: String,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            move_up: String::from("W"),
            move_down: String::from("S"),
            move_left: String::from("A"),
            move_right: String::from("D"),
            cast_q: String::from("Q"),
            cast_w: String::from("W"),
            cast_e: String::from("E"),
            stop: String::from("Space"),
            shop: String::from("B"),
            scoreboard: String::from("Tab"),
            ready: String::from("R"),
            surrender: String::from("F5"),
            attack_button: String::from("left"),
            camera_left: String::from("Left"),
            camera_right: String::from("Right"),
            camera_up: String::from("Up"),
            camera_down: String::from("Down"),
        }
    }
}

/// Resolved bindings used by the client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Bindings {
    pub(crate) move_up: Key,
    pub(crate) move_down: Key,
    pub(crate) move_left: Key,
    pub(crate) move_right: Key,
    pub(crate) cast_q: Key,
    pub(crate) cast_w: Key,
    pub(crate) cast_e: Key,
    pub(crate) stop: Key,
    pub(crate) shop: Key,
    pub(crate) scoreboard: Key,
    pub(crate) ready: Key,
    pub(crate) surrender: Key,
    pub(crate) attack_button: MouseBinding,
    pub(crate) camera_left: Key,
    pub(crate) camera_right: Key,
    pub(crate) camera_up: Key,
    pub(crate) camera_down: Key,
}

impl Bindings {
    #[allow(clippy::too_many_arguments)]
    const fn new(
        move_up: Key,
        move_down: Key,
        move_left: Key,
        move_right: Key,
        cast_q: Key,
        cast_w: Key,
        cast_e: Key,
        stop: Key,
        shop: Key,
        scoreboard: Key,
        ready: Key,
        surrender: Key,
        camera_left: Key,
        camera_right: Key,
        camera_up: Key,
        camera_down: Key,
    ) -> Self {
        Self {
            move_up,
            move_down,
            move_left,
            move_right,
            cast_q,
            cast_w,
            cast_e,
            stop,
            shop,
            scoreboard,
            ready,
            surrender,
            attack_button: MouseBinding::Left,
            camera_left,
            camera_right,
            camera_up,
            camera_down,
        }
    }
}

/// User settings persisted as TOML.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    pub(crate) ui_scale: f32,
    /// Rendering quality tier: `low`, `medium`, `high` or `ultra` (`MOBA_QUALITY` overrides).
    pub(crate) quality: String,
    pub(crate) audio_volume: f32,
    pub(crate) audio_mute: bool,
    pub(crate) camera_sensitivity: f32,
    pub(crate) camera_zoom: f32,
    pub(crate) controls: Controls,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ui_scale: 1.0,
            quality: String::from(moba_render::QualityTier::default().name()),
            audio_volume: 0.7,
            audio_mute: false,
            camera_sensitivity: 1.0,
            camera_zoom: 240.0,
            controls: Controls::default(),
        }
    }
}

impl Settings {
    /// Loads settings from `path`, falling back to defaults and collecting warnings.
    #[must_use]
    pub(crate) fn load(path: &Path) -> (Self, Vec<String>) {
        let Ok(text) = std::fs::read_to_string(path) else {
            return (Self::default(), Vec::new());
        };
        match toml::from_str::<Self>(&text) {
            Ok(mut settings) => {
                let warnings = settings.sanitize();
                (settings, warnings)
            }
            Err(error) => (
                Self::default(),
                vec![format!("settings: {error}; using defaults")],
            ),
        }
    }

    /// Writes settings to `path` atomically enough for a local preference file.
    pub(crate) fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }

    /// Resolves and validates bindings, replacing invalid/duplicate entries with defaults.
    #[must_use]
    pub(crate) fn bindings(&self) -> (Bindings, Vec<String>) {
        let defaults = Controls::default();
        let mut warnings = Vec::new();
        let keys = [
            (&self.controls.move_up, &defaults.move_up, "move_up"),
            (&self.controls.move_down, &defaults.move_down, "move_down"),
            (&self.controls.move_left, &defaults.move_left, "move_left"),
            (
                &self.controls.move_right,
                &defaults.move_right,
                "move_right",
            ),
            (&self.controls.cast_q, &defaults.cast_q, "cast_q"),
            (&self.controls.cast_w, &defaults.cast_w, "cast_w"),
            (&self.controls.cast_e, &defaults.cast_e, "cast_e"),
            (&self.controls.stop, &defaults.stop, "stop"),
            (&self.controls.shop, &defaults.shop, "shop"),
            (
                &self.controls.scoreboard,
                &defaults.scoreboard,
                "scoreboard",
            ),
            (&self.controls.ready, &defaults.ready, "ready"),
            (&self.controls.surrender, &defaults.surrender, "surrender"),
            (
                &self.controls.camera_left,
                &defaults.camera_left,
                "camera_left",
            ),
            (
                &self.controls.camera_right,
                &defaults.camera_right,
                "camera_right",
            ),
            (&self.controls.camera_up, &defaults.camera_up, "camera_up"),
            (
                &self.controls.camera_down,
                &defaults.camera_down,
                "camera_down",
            ),
        ];
        let mut resolved = [Key::W; 16];
        for (index, (value, fallback, name)) in keys.into_iter().enumerate() {
            resolved[index] = parse_key(value).unwrap_or_else(|| {
                warnings.push(format!("settings: invalid binding {name}={value:?}"));
                parse_key(fallback).unwrap_or(Key::W)
            });
        }
        let mut bindings = Bindings::new(
            resolved[0],
            resolved[1],
            resolved[2],
            resolved[3],
            resolved[4],
            resolved[5],
            resolved[6],
            resolved[7],
            resolved[8],
            resolved[9],
            resolved[10],
            resolved[11],
            resolved[12],
            resolved[13],
            resolved[14],
            resolved[15],
        );
        bindings.attack_button = parse_mouse(&self.controls.attack_button).unwrap_or_else(|| {
            warnings.push(format!(
                "settings: invalid attack_button={:?}",
                self.controls.attack_button
            ));
            MouseBinding::Left
        });
        if bindings.move_up == bindings.move_down {
            warnings.push(String::from("settings: duplicate movement binding reset"));
            bindings = Bindings::new(
                Key::W,
                Key::S,
                Key::A,
                Key::D,
                Key::Q,
                Key::W,
                Key::E,
                Key::Space,
                Key::B,
                Key::Tab,
                Key::R,
                Key::F5,
                Key::ArrowLeft,
                Key::ArrowRight,
                Key::ArrowUp,
                Key::ArrowDown,
            );
        }
        (bindings, warnings)
    }

    /// The configured rendering quality tier (validated by [`Self::sanitize`]).
    #[must_use]
    pub(crate) fn quality_tier(&self) -> moba_render::QualityTier {
        moba_render::QualityTier::from_name(&self.quality).unwrap_or_default()
    }

    /// Clamps numeric settings, validates names and reports corrections.
    pub(crate) fn sanitize(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        if moba_render::QualityTier::from_name(&self.quality).is_none() {
            warnings.push(format!(
                "settings: unknown quality {:?} (low/medium/high/ultra); using high",
                self.quality
            ));
            self.quality = String::from(moba_render::QualityTier::default().name());
        }
        if !self.ui_scale.is_finite() {
            self.ui_scale = 1.0;
            warnings.push(String::from("settings: ui_scale reset"));
        } else if !(UI_SCALE_MIN..=UI_SCALE_MAX).contains(&self.ui_scale) {
            self.ui_scale = self.ui_scale.clamp(UI_SCALE_MIN, UI_SCALE_MAX);
            warnings.push(String::from("settings: ui_scale clamped"));
        }
        if !self.audio_volume.is_finite() {
            self.audio_volume = 0.7;
            warnings.push(String::from("settings: audio_volume reset"));
        } else if !(0.0..=1.0).contains(&self.audio_volume) {
            self.audio_volume = self.audio_volume.clamp(0.0, 1.0);
            warnings.push(String::from("settings: audio_volume clamped"));
        }
        if !self.camera_sensitivity.is_finite() {
            self.camera_sensitivity = 1.0;
            warnings.push(String::from("settings: camera_sensitivity reset"));
        } else if !(SENSITIVITY_MIN..=SENSITIVITY_MAX).contains(&self.camera_sensitivity) {
            self.camera_sensitivity = self
                .camera_sensitivity
                .clamp(SENSITIVITY_MIN, SENSITIVITY_MAX);
            warnings.push(String::from("settings: camera_sensitivity clamped"));
        }
        if !self.camera_zoom.is_finite() {
            self.camera_zoom = 240.0;
            warnings.push(String::from("settings: camera_zoom reset"));
        } else if !(CAMERA_ZOOM_MIN..=CAMERA_ZOOM_MAX).contains(&self.camera_zoom) {
            self.camera_zoom = self.camera_zoom.clamp(CAMERA_ZOOM_MIN, CAMERA_ZOOM_MAX);
            warnings.push(String::from("settings: camera_zoom clamped"));
        }
        warnings
    }

    /// Default settings path, overridable with `MOBA_SETTINGS`.
    #[must_use]
    pub(crate) fn path() -> PathBuf {
        std::env::var_os("MOBA_SETTINGS").map_or_else(|| PathBuf::from(DEFAULT_PATH), PathBuf::from)
    }
}

/// Parses a small, display-friendly key name.
#[must_use]
pub(crate) fn parse_key(name: &str) -> Option<Key> {
    Some(match name.to_ascii_uppercase().as_str() {
        "W" => Key::W,
        "A" => Key::A,
        "S" => Key::S,
        "D" => Key::D,
        "Q" => Key::Q,
        "E" => Key::E,
        "R" => Key::R,
        "F" => Key::F,
        "B" => Key::B,
        "M" => Key::M,
        "SPACE" => Key::Space,
        "TAB" => Key::Tab,
        "ESCAPE" => Key::Escape,
        "ENTER" => Key::Enter,
        "UP" => Key::ArrowUp,
        "DOWN" => Key::ArrowDown,
        "LEFT" => Key::ArrowLeft,
        "RIGHT" => Key::ArrowRight,
        "F1" => Key::F1,
        "F2" => Key::F2,
        "F3" => Key::F3,
        "F4" => Key::F4,
        "F5" => Key::F5,
        "1" => Key::Digit1,
        "2" => Key::Digit2,
        "3" => Key::Digit3,
        "4" => Key::Digit4,
        "5" => Key::Digit5,
        "6" => Key::Digit6,
        "7" => Key::Digit7,
        "8" => Key::Digit8,
        _ => return None,
    })
}

fn parse_mouse(name: &str) -> Option<MouseBinding> {
    match name.to_ascii_lowercase().as_str() {
        "left" => Some(MouseBinding::Left),
        "right" => Some(MouseBinding::Right),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_invalid_bindings_fall_back() {
        let settings = Settings::default();
        let (bindings, warnings) = settings.bindings();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(bindings.move_up, Key::W);
        assert_eq!(bindings.cast_q, Key::Q);

        let broken = Settings {
            controls: Controls {
                move_up: String::from("NOPE"),
                ..Controls::default()
            },
            ..Settings::default()
        };
        let (bindings, warnings) = broken.bindings();
        assert_eq!(bindings.move_up, Key::W);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn camera_and_attack_settings_validate() {
        let mut settings = Settings {
            camera_sensitivity: 99.0,
            camera_zoom: 1.0,
            quality: String::from("epic"),
            ..Settings::default()
        };
        let warnings = settings.sanitize();
        assert_eq!(warnings.len(), 3);
        assert_eq!(settings.camera_sensitivity, SENSITIVITY_MAX);
        assert_eq!(settings.camera_zoom, CAMERA_ZOOM_MIN);
        assert_eq!(settings.quality_tier(), moba_render::QualityTier::High);
        settings.quality = String::from("Low");
        assert_eq!(settings.sanitize(), Vec::<String>::new());
        assert_eq!(settings.quality_tier(), moba_render::QualityTier::Low);

        settings.controls.attack_button = String::from("right");
        let (bindings, warnings) = settings.bindings();
        assert_eq!(warnings.len(), 0);
        assert_eq!(bindings.attack_button, MouseBinding::Right);
    }

    #[test]
    fn settings_save_and_load_round_trip() {
        let path = std::env::temp_dir().join(format!("moba-settings-{}.toml", std::process::id()));
        let mut settings = Settings::default();
        settings.controls.scoreboard = String::from("F2");
        settings.save(&path).expect("save");
        let (loaded, warnings) = Settings::load(&path);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(loaded.controls.scoreboard, "F2");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn settings_round_trip_and_scale_clamp() {
        let mut settings = Settings {
            ui_scale: 99.0,
            controls: Controls {
                shop: String::from("M"),
                ..Controls::default()
            },
            ..Settings::default()
        };
        let warnings = settings.sanitize();
        assert_eq!(settings.ui_scale, UI_SCALE_MAX);
        assert_eq!(warnings.len(), 1);
        let text = toml::to_string_pretty(&settings).expect("serialize");
        let parsed: Settings = toml::from_str(&text).expect("parse");
        assert_eq!(parsed.ui_scale, UI_SCALE_MAX);
        assert_eq!(parsed.controls.shop, "M");
        assert_eq!(parsed.bindings().0.shop, Key::M);
    }
}
