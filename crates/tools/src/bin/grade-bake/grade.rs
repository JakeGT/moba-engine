//! The colour grade `grade-bake` evaluates at each LUT lattice point.
//!
//! Inputs and outputs are **display-encoded** RGB in `0..=1` (the tonemapper's `AgX` output,
//! roughly a 2.2 gamma), so every step below works in display space, in this order:
//!
//! 1. lift/gamma/gain per channel (ASC-CDL-like): `x = max(0, gain·(x + lift·(1 − x)))^(1/gamma)`;
//! 2. contrast around display mid-grey: `x = (x − 0.5)·contrast + 0.5`;
//! 3. saturation around Rec.709 luma `l = 0.2126 r + 0.7152 g + 0.0722 b`:
//!    `x = l + (x − l)·saturation`;
//! 4. white balance as per-channel gains: `r ·= 1 + 0.1·temperature`,
//!    `b ·= 1 − 0.1·temperature`, `g ·= 1 − 0.1·tint` (temperature > 0 is warmer, tint > 0
//!    is more magenta, each in `-1..=1`, so at most a 10% gain change);
//! 5. clamp to `0..=1`.
//!
//! The defaults (lift 0, gamma 1, gain 1, contrast 1, saturation 1, temperature 0, tint 0)
//! are the identity.

/// Preset names, in `--preset` order.
pub(crate) const PRESETS: [&str; 4] = ["identity", "ember", "frost", "meadow"];
/// Gain change per unit of temperature or tint.
const WHITE_BALANCE_SCALE: f32 = 0.1;

/// One grade (see the module docs for the formula).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Grade {
    pub(crate) lift: [f32; 3],
    pub(crate) gamma: [f32; 3],
    pub(crate) gain: [f32; 3],
    pub(crate) contrast: f32,
    pub(crate) saturation: f32,
    pub(crate) temperature: f32,
    pub(crate) tint: f32,
}

impl Grade {
    /// The identity grade.
    pub(crate) const IDENTITY: Self = Self {
        lift: [0.0; 3],
        gamma: [1.0; 3],
        gain: [1.0; 3],
        contrast: 1.0,
        saturation: 1.0,
        temperature: 0.0,
        tint: 0.0,
    };

    /// A named preset. Every preset is gentle: team red and blue keep their dominant
    /// channel and hue (ADR-0011 readability rule 1), which the tests check.
    pub(crate) fn preset(name: &str) -> Option<Self> {
        Some(match name {
            "identity" => Self::IDENTITY,
            // Warm, slightly lifted shadows (black lands near [0.03, 0.02, 0.01] after the
            // contrast), gentle contrast. The temperature stays low so blue units lose at
            // most ~3% blue.
            "ember" => Self {
                lift: [0.05, 0.035, 0.025],
                contrast: 1.04,
                temperature: 0.3,
                ..Self::IDENTITY
            },
            // Cool, slightly desaturated, crisp.
            "frost" => Self {
                lift: [0.0, 0.005, 0.015],
                contrast: 1.08,
                saturation: 0.92,
                temperature: -0.3,
                ..Self::IDENTITY
            },
            // Neutral-warm with a touch more saturation.
            "meadow" => Self {
                lift: [0.01, 0.01, 0.0],
                contrast: 1.03,
                saturation: 1.08,
                temperature: 0.1,
                ..Self::IDENTITY
            },
            _ => return None,
        })
    }

    /// The graded colour of display-encoded `rgb`.
    pub(crate) fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mut c = [0.0f32; 3];
        for (channel, out) in c.iter_mut().enumerate() {
            let x = rgb[channel];
            let lifted = self.gain[channel] * (x + self.lift[channel] * (1.0 - x));
            *out = lifted.max(0.0).powf(1.0 / self.gamma[channel]);
        }
        let c = c.map(|x| (x - 0.5) * self.contrast + 0.5);
        let luma = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let c = c.map(|x| luma + (x - luma) * self.saturation);
        let balance = [
            1.0 + WHITE_BALANCE_SCALE * self.temperature,
            1.0 - WHITE_BALANCE_SCALE * self.tint,
            1.0 - WHITE_BALANCE_SCALE * self.temperature,
        ];
        [
            (c[0] * balance[0]).clamp(0.0, 1.0),
            (c[1] * balance[1]).clamp(0.0, 1.0),
            (c[2] * balance[2]).clamp(0.0, 1.0),
        ]
    }

    /// One line per parameter, for the `.cube` header comments.
    pub(crate) fn describe(&self) -> Vec<String> {
        let v3 = |v: [f32; 3]| format!("{},{},{}", v[0], v[1], v[2]);
        vec![
            format!(
                "lift = {}, gamma = {}, gain = {}",
                v3(self.lift),
                v3(self.gamma),
                v3(self.gain)
            ),
            format!(
                "contrast = {}, saturation = {}, temperature = {}, tint = {}",
                self.contrast, self.saturation, self.temperature, self.tint
            ),
        ]
    }
}

/// HSV hue in degrees (`0..360`) of `rgb`.
#[cfg(test)]
pub(crate) fn hue(rgb: [f32; 3]) -> f32 {
    let [r, g, b] = rgb;
    let max = r.max(g).max(b);
    let delta = max - r.min(g).min(b);
    if delta <= 0.0 {
        return 0.0;
    }
    let sector = if r >= g && r >= b {
        ((g - b) / delta).rem_euclid(6.0)
    } else if g >= b {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    sector * 60.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Display-encoded team colours (approximate red and blue unit tints).
    const RED: [f32; 3] = [0.85, 0.2, 0.2];
    const BLUE: [f32; 3] = [0.2, 0.4, 0.9];

    fn dominant(rgb: [f32; 3]) -> usize {
        (0..3)
            .max_by(|&a, &b| rgb[a].total_cmp(&rgb[b]))
            .unwrap_or(0)
    }

    fn hue_distance(a: f32, b: f32) -> f32 {
        let d = (a - b).abs() % 360.0;
        d.min(360.0 - d)
    }

    #[test]
    fn identity_is_identity() {
        for rgb in [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], RED, BLUE, [0.3, 0.7, 0.1]] {
            let out = Grade::IDENTITY.apply(rgb);
            assert!(out.iter().zip(rgb).all(|(a, b)| (a - b).abs() < 1e-6));
        }
    }

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn each_step_follows_the_documented_formula() {
        let lift = Grade {
            lift: [0.2; 3],
            ..Grade::IDENTITY
        };
        // x + lift·(1 − x): black rises to the lift, white stays white.
        assert!(near(lift.apply([0.0; 3]), [0.2; 3]));
        assert!(near(lift.apply([1.0; 3]), [1.0; 3]));
        let gamma = Grade {
            gamma: [2.0; 3],
            ..Grade::IDENTITY
        };
        assert!((gamma.apply([0.25; 3])[0] - 0.5).abs() < 1e-6);
        let gain = Grade {
            gain: [0.5, 1.0, 1.0],
            ..Grade::IDENTITY
        };
        assert!(near(gain.apply([0.8; 3]), [0.4, 0.8, 0.8]));
        let contrast = Grade {
            contrast: 2.0,
            ..Grade::IDENTITY
        };
        assert!(near(contrast.apply([0.5; 3]), [0.5; 3]));
        assert!(near(contrast.apply([0.6; 3]), [0.7; 3]));
        let grey = Grade {
            saturation: 0.0,
            ..Grade::IDENTITY
        }
        .apply([1.0, 0.0, 0.0]);
        assert!(grey.iter().all(|&c| (c - 0.2126).abs() < 1e-6), "{grey:?}");
        let warm = Grade {
            temperature: 1.0,
            tint: 1.0,
            ..Grade::IDENTITY
        }
        .apply([0.5; 3]);
        assert!((warm[0] - 0.55).abs() < 1e-6 && (warm[1] - 0.45).abs() < 1e-6);
        assert!((warm[2] - 0.45).abs() < 1e-6);
        // Output is always clamped.
        let hot = Grade {
            gain: [2.0; 3],
            contrast: 2.0,
            ..Grade::IDENTITY
        };
        assert_eq!(hot.apply([1.0; 3]), [1.0; 3]);
        assert_eq!(hot.apply([0.0; 3]), [0.0; 3]);
    }

    #[test]
    fn presets_keep_team_colours_readable() {
        for name in PRESETS {
            let grade = Grade::preset(name).expect("preset");
            let lut = crate::bake_lut(&grade, 17).expect("bakes");
            for team in [RED, BLUE] {
                // Both the exact grade and the baked, interpolated LUT.
                for out in [grade.apply(team), lut.sample(team)] {
                    assert_eq!(dominant(out), dominant(team), "{name} {team:?} -> {out:?}");
                    let shift = hue_distance(hue(out), hue(team));
                    assert!(shift < 15.0, "{name} {team:?} -> {out:?}: {shift}°");
                }
            }
            // Blue units keep nearly all of their blue (ember is the warmest preset).
            assert!(grade.apply(BLUE)[2] > 0.85, "{name}");
            // Gentle: no lattice point moves more than 0.12 from the identity.
            let worst = lut
                .texels()
                .iter()
                .zip(moba_assets::GradingLut::identity(17).texels())
                .flat_map(|(a, b)| (0..3).map(move |c| (a[c] - b[c]).abs()))
                .fold(0.0f32, f32::max);
            assert!(worst < 0.12, "{name}: {worst}");
        }
        assert!(Grade::preset("lava").is_none());
        assert!((hue(BLUE) - 222.857).abs() < 0.01);
        assert_eq!(hue([0.5; 3]), 0.0);
    }
}
