//! Gameplay screen effects (R6, ADR-0015 §7): death desaturation, a damage vignette and a
//! low-health pulse, from the local hero's health in the snapshots the client already has.
//!
//! Each effect has its own accessibility setting; the renderer applies the result in the
//! composite, before the HUD. Fixed-size state, updated per snapshot and per frame without
//! allocating. Cosmetic and client-local: nothing here reaches the simulation or the wire.

use moba_render::ScreenEffects;

/// Health fraction below which the low-health pulse runs.
pub(crate) const LOW_HEALTH: f32 = 0.3;
/// Pulse frequency, Hz.
const PULSE_HZ: f32 = 1.1;
/// Damage flash decay per second (a full flash fades in ~0.4 s).
const FLASH_DECAY: f32 = 2.5;
/// Death fade rates per second: towards grey while dead, back to colour on respawn.
const DEATH_FADE_IN: f32 = 1.5;
const DEATH_FADE_OUT: f32 = 4.0;
/// Greyest the dead view gets (team colours stay faintly visible).
const DEATH_DESATURATION: f32 = 0.85;
/// The vignette's colour: a dark red, in display space.
const VIGNETTE_COLOR: [f32; 3] = [0.55, 0.02, 0.02];

/// Which effects the player allows (client settings).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // One accessibility switch per effect.
pub(crate) struct ScreenFxSettings {
    pub(crate) death_desaturation: bool,
    pub(crate) damage_vignette: bool,
    pub(crate) low_health_pulse: bool,
}

impl ScreenFxSettings {
    /// Every effect on.
    #[cfg(test)]
    pub(crate) const ALL: Self = Self {
        death_desaturation: true,
        damage_vignette: true,
        low_health_pulse: true,
    };
}

/// A fixed effect for captures and screenshots (`MOBA_SCREEN_EFFECTS_PREVIEW`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Preview {
    Death,
    Damage,
    LowHealth,
}

impl Preview {
    /// Parses `death`, `damage` or `lowhealth`.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "death" => Some(Self::Death),
            "damage" => Some(Self::Damage),
            "lowhealth" | "low-health" | "low_health" => Some(Self::LowHealth),
            _ => None,
        }
    }
}

/// The local hero's screen-effect state.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ScreenFx {
    settings: ScreenFxSettings,
    preview: Option<Preview>,
    last_health: Option<u16>,
    health_fraction: f32,
    dead: bool,
    /// Damage flash, 0..1.
    flash: f32,
    /// Death fade, 0..1.
    death: f32,
    /// Seconds of pulse phase.
    time: f32,
}

impl ScreenFx {
    pub(crate) const fn new(settings: ScreenFxSettings, preview: Option<Preview>) -> Self {
        Self {
            settings,
            preview,
            last_health: None,
            health_fraction: 1.0,
            dead: false,
            flash: 0.0,
            death: 0.0,
            time: 0.0,
        }
    }

    /// Forgets the hero (a new match or reconnect): no flash from the first snapshot.
    pub(crate) fn reset(&mut self) {
        *self = Self::new(self.settings, self.preview);
    }

    /// Feeds the local hero's state from a new snapshot.
    pub(crate) fn observe(&mut self, health: u16, max_health: u16, dead: bool) {
        if let Some(previous) = self.last_health
            && health < previous
            && !dead
        {
            let lost = f32::from(previous - health) / f32::from(max_health.max(1));
            self.flash = self.flash.max((0.35 + 2.0 * lost).min(1.0));
        }
        self.last_health = Some(health);
        self.health_fraction = f32::from(health) / f32::from(max_health.max(1));
        self.dead = dead;
    }

    /// Advances by `dt` seconds and returns this frame's effects.
    pub(crate) fn update(&mut self, dt: f32) -> ScreenEffects {
        let dt = if dt.is_finite() {
            dt.clamp(0.0, 0.25)
        } else {
            0.0
        };
        self.time = (self.time + dt) % 1_000.0;
        self.flash = (self.flash - FLASH_DECAY * dt).max(0.0);
        self.death = if self.dead {
            (self.death + DEATH_FADE_IN * dt).min(1.0)
        } else {
            (self.death - DEATH_FADE_OUT * dt).max(0.0)
        };
        let (death, flash, low) = match self.preview {
            Some(Preview::Death) => (1.0, 0.0, None),
            Some(Preview::Damage) => (0.0, 1.0, None),
            Some(Preview::LowHealth) => (0.0, 0.0, Some(0.12)),
            None => (
                self.death,
                self.flash,
                (!self.dead && self.health_fraction < LOW_HEALTH).then_some(self.health_fraction),
            ),
        };
        let settings = self.settings;
        let desaturation = if settings.death_desaturation {
            DEATH_DESATURATION * death
        } else {
            0.0
        };
        let damage = if settings.damage_vignette {
            0.6 * flash
        } else {
            0.0
        };
        let pulse = match low {
            Some(fraction) if settings.low_health_pulse => {
                // Stronger the lower the health, never fully off while low; a captured
                // preview holds the peak so the frame is deterministic.
                let depth = 0.2 + 0.3 * (LOW_HEALTH - fraction).max(0.0) / LOW_HEALTH;
                let wave = if self.preview.is_some() {
                    1.0
                } else {
                    0.5 + 0.5 * (core::f32::consts::TAU * PULSE_HZ * self.time).sin()
                };
                depth * (0.35 + 0.65 * wave)
            }
            _ => 0.0,
        };
        ScreenEffects::NONE
            .with_desaturation(desaturation)
            .with_vignette(damage.max(pulse), VIGNETTE_COLOR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_flashes_then_fades() {
        let mut fx = ScreenFx::new(ScreenFxSettings::ALL, None);
        fx.observe(1_000, 1_000, false);
        assert_eq!(
            fx.update(0.016).vignette,
            0.0,
            "the first snapshot never flashes"
        );
        fx.observe(800, 1_000, false);
        let hit = fx.update(0.0);
        assert!(hit.vignette > 0.4, "{hit:?}");
        assert_eq!(hit.desaturation, 0.0);
        // Frame steps are capped at 0.25 s, so a hitch cannot skip the flash entirely.
        assert!(fx.update(1.0).vignette > 0.0);
        let later = (0..4).fold(ScreenEffects::NONE, |_, _| fx.update(0.25));
        assert_eq!(later.vignette, 0.0);
        // Healing never flashes.
        fx.observe(900, 1_000, false);
        assert_eq!(fx.update(0.0).vignette, 0.0);
    }

    #[test]
    fn death_desaturates_and_respawn_restores_colour() {
        let mut fx = ScreenFx::new(ScreenFxSettings::ALL, None);
        fx.observe(500, 1_000, false);
        fx.observe(0, 1_000, true);
        let dead = (0..60).fold(ScreenEffects::NONE, |_, _| fx.update(1.0 / 30.0));
        assert!(
            (dead.desaturation - DEATH_DESATURATION).abs() < 1e-4,
            "{dead:?}"
        );
        assert_eq!(dead.vignette, 0.0, "no pulse or flash while dead");
        fx.observe(1_000, 1_000, false);
        let alive = (0..30).fold(ScreenEffects::NONE, |_, _| fx.update(1.0 / 30.0));
        assert_eq!(alive.desaturation, 0.0);
    }

    #[test]
    fn low_health_pulses_and_each_setting_turns_its_effect_off() {
        let mut fx = ScreenFx::new(ScreenFxSettings::ALL, None);
        fx.observe(100, 1_000, false);
        let samples: Vec<f32> = (0..60).map(|_| fx.update(1.0 / 60.0).vignette).collect();
        let (low, high) = samples
            .iter()
            .fold((f32::MAX, 0.0f32), |(low, high), &value| {
                (low.min(value), high.max(value))
            });
        assert!(low > 0.0 && high > low + 0.1, "{low} {high}");

        let off = ScreenFxSettings {
            death_desaturation: false,
            damage_vignette: false,
            low_health_pulse: false,
        };
        let mut quiet = ScreenFx::new(off, None);
        quiet.observe(1_000, 1_000, false);
        quiet.observe(50, 1_000, false);
        let effects = quiet.update(0.0);
        assert_eq!((effects.vignette, effects.desaturation), (0.0, 0.0));
        quiet.observe(0, 1_000, true);
        assert_eq!(quiet.update(5.0).desaturation, 0.0);
    }

    #[test]
    fn previews_are_constant() {
        for (name, preview) in [
            ("death", Preview::Death),
            ("damage", Preview::Damage),
            ("LowHealth", Preview::LowHealth),
        ] {
            assert_eq!(Preview::from_name(name), Some(preview));
            let mut fx = ScreenFx::new(ScreenFxSettings::ALL, Some(preview));
            let first = fx.update(0.3);
            let second = fx.update(0.7);
            assert_eq!(first, second, "{name}");
            assert!(first.vignette > 0.0 || first.desaturation > 0.0);
        }
        assert_eq!(Preview::from_name("blood"), None);
    }
}
