//! Heat-haze distortion (ADR-0019): the per-tier, per-resolution offset cap and the CPU twin
//! of the fragment shader's readability rules (`vfx_distort.frag`), unit-tested here.
//!
//! A distortion particle ([`ParticleBlend::Distortion`](crate::ParticleBlend::Distortion))
//! refracts the copy of the HDR target taken after the water and before the particles. Per
//! fragment the shader (and [`refract_tap`]):
//!
//! 1. keeps the pixel where a unit is (the prepass ID target is nonzero: an outlined unit's
//!    slot, or [`UNIT_ID_PLAIN`] for every other unit);
//! 2. scales the noise direction by the strength and fades and clamps the offset to the cap
//!    ([`max_offset_pixels`]);
//! 3. rejects a tap whose footprint touches a unit or geometry nearer the camera than the
//!    sprite, keeping the pixel: nothing in front is pulled into the haze.
//!
//! The sprite itself is depth-tested, so anything in front of it is never touched; the
//! particles (telegraphs, projectiles) draw after it and the outlines and UI after the post
//! chain, so none of them is ever displaced.

use crate::vfx::VfxBudget;

/// The prepass ID of a unit without an outline (minions, structures, walls, props). Outlined
/// units write their palette slot (1–7); the terrain writes nothing (0). The outline pass
/// treats this value like 0, so the outlines are unchanged.
pub const UNIT_ID_PLAIN: u8 = 8;
/// The render height [`VfxBudget::distortion_pixels`] is given for.
pub const REFERENCE_HEIGHT: f32 = 1080.0;
/// The cap scales with the render height within `[0.5, 2]` of the 1080p value.
const HEIGHT_SCALE: (f32, f32) = (0.5, 2.0);
/// Largest per-tier cap accepted at 1080p, pixels (a hostile or mistaken budget is clamped).
const MAX_BASE_PIXELS: f32 = 16.0;
/// Soft-depth fade distance, metres: the haze fades out where it meets the scene.
pub(crate) const SOFT_DISTANCE: f32 = 1.0;
/// View distance at which the haze starts to fade, metres.
pub(crate) const FADE_START: f32 = 70.0;
/// View distance at which the haze is gone, metres.
pub(crate) const FADE_END: f32 = 110.0;

/// The longest offset a distortion fragment may apply, in pixels of the render target, for a
/// frame rendered `render_height` pixels tall (the render sub-rectangle's height, so dynamic
/// resolution shrinks it): the budget's 1080p cap × `render_height / 1080`, the ratio clamped
/// to `[0.5, 2]`. Zero where the budget has no distortion.
#[must_use]
pub fn max_offset_pixels(budget: &VfxBudget, render_height: u32) -> f32 {
    if !budget.distortion || !budget.distortion_pixels.is_finite() {
        return 0.0;
    }
    let base = budget.distortion_pixels.clamp(0.0, MAX_BASE_PIXELS);
    let ratio = (render_height as f32 / REFERENCE_HEIGHT).clamp(HEIGHT_SCALE.0, HEIGHT_SCALE.1);
    base * ratio
}

/// What a distortion fragment does with its pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DistortionTap {
    /// Leaves the pixel as it is (a unit, a rejected tap, or no strength).
    Keep,
    /// Writes the copy at the pixel plus this offset (pixels, at most the cap long).
    Offset([f32; 2]),
}

/// What the scene holds where one distortion fragment lands and where its tap lands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TapScene {
    /// The fragment's own pixel is a unit (nonzero ID).
    pub own_unit: bool,
    /// The tap's 2×2 footprint touches a unit.
    pub tap_unit: bool,
    /// The nearest linear view depth in the tap's footprint, metres.
    pub tap_depth: f32,
    /// The sprite's linear view depth at the fragment, metres.
    pub particle_depth: f32,
}

/// The CPU twin of `vfx_distort.frag`'s steps 2–4: `direction` is the noise direction in
/// `[-1, 1]²`, `weight` the strength times every fade (`0..=1`), `cap` the offset cap
/// ([`max_offset_pixels`]) and `mask` whether the unit ID target is used.
#[must_use]
pub fn refract_tap(
    direction: [f32; 2],
    weight: f32,
    cap: f32,
    mask: bool,
    scene: &TapScene,
) -> DistortionTap {
    if mask && scene.own_unit {
        return DistortionTap::Keep;
    }
    let weight = if weight.is_finite() {
        weight.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let cap = if cap.is_finite() { cap.max(0.0) } else { 0.0 };
    let mut offset = direction.map(|axis| {
        let axis = if axis.is_finite() { axis } else { 0.0 };
        axis.clamp(-1.0, 1.0) * weight * cap
    });
    let length = offset[0].hypot(offset[1]);
    if length > cap && length > 0.0 {
        offset = offset.map(|axis| axis * cap / length);
    }
    // A NaN depth counts as in front: rejected.
    let in_front = scene
        .tap_depth
        .partial_cmp(&scene.particle_depth)
        .is_none_or(|order| order == core::cmp::Ordering::Less);
    if (mask && scene.tap_unit) || in_front || weight <= 0.0 {
        return DistortionTap::Keep;
    }
    DistortionTap::Offset(offset)
}

#[cfg(test)]
mod tests {
    use super::{DistortionTap, TapScene, UNIT_ID_PLAIN, max_offset_pixels, refract_tap};
    use crate::QualityTier;

    const OPEN: TapScene = TapScene {
        own_unit: false,
        tap_unit: false,
        tap_depth: 50.0,
        particle_depth: 40.0,
    };

    fn length(tap: DistortionTap) -> f32 {
        match tap {
            DistortionTap::Offset([x, y]) => x.hypot(y),
            DistortionTap::Keep => 0.0,
        }
    }

    #[test]
    fn the_cap_is_per_tier_and_scales_with_the_render_height() {
        let low = QualityTier::Low.vfx_budget();
        let medium = QualityTier::Medium.vfx_budget();
        let high = QualityTier::High.vfx_budget();
        let ultra = QualityTier::Ultra.vfx_budget();
        assert_eq!(max_offset_pixels(&low, 1080), 0.0);
        assert_eq!(max_offset_pixels(&medium, 2160), 0.0);
        assert_eq!(max_offset_pixels(&high, 1080), 6.0);
        assert_eq!(max_offset_pixels(&ultra, 1080), 8.0);
        // 4K doubles it, 1440p by 4/3, a 0.75 render scale of 1080p shrinks it.
        assert_eq!(max_offset_pixels(&ultra, 2160), 16.0);
        assert!((max_offset_pixels(&high, 1440) - 8.0).abs() < 1e-5);
        assert!((max_offset_pixels(&high, 810) - 4.5).abs() < 1e-5);
        // Clamped to half and twice the 1080p cap, whatever the target.
        assert_eq!(max_offset_pixels(&high, 100), 3.0);
        assert_eq!(max_offset_pixels(&high, 8640), 12.0);
        assert_eq!(max_offset_pixels(&high, 0), 3.0);
        // Never more than 0.6 % of the render height at 1080p and up (readability).
        for &tier in QualityTier::ALL {
            for height in [1080u32, 1440, 2160] {
                let cap = max_offset_pixels(&tier.vfx_budget(), height);
                assert!(
                    cap <= 0.0075 * height as f32 + 1e-4,
                    "{tier} {height}: {cap}"
                );
            }
        }
        // A switched-off or hostile budget gives no offset or a clamped one.
        let mut off = ultra;
        off.distortion = false;
        assert_eq!(max_offset_pixels(&off, 1080), 0.0);
        let mut hostile = ultra;
        hostile.distortion_pixels = f32::NAN;
        assert_eq!(max_offset_pixels(&hostile, 1080), 0.0);
        hostile.distortion_pixels = 1e9;
        assert_eq!(max_offset_pixels(&hostile, 1080), 16.0);
    }

    #[test]
    fn offsets_never_exceed_the_cap() {
        let cap = 6.0;
        for direction in [
            [1.0, 1.0],
            [-1.0, 1.0],
            [1.0, 0.0],
            [9.0, -9.0],
            [f32::NAN, 1.0],
        ] {
            for weight in [0.25, 1.0, 4.0, f32::INFINITY] {
                let tap = refract_tap(direction, weight, cap, true, &OPEN);
                assert!(length(tap) <= cap + 1e-5, "{direction:?} {weight}: {tap:?}");
            }
        }
        assert_eq!(
            refract_tap([1.0, 0.0], 0.5, cap, true, &OPEN),
            DistortionTap::Offset([3.0, 0.0])
        );
        // The diagonal is clamped to the cap's length, not its per-axis value.
        assert!((length(refract_tap([1.0, 1.0], 1.0, cap, true, &OPEN)) - cap).abs() < 1e-5);
        // No strength (an edge, a far haze, an allied haze at 0 opacity): nothing moves.
        assert_eq!(
            refract_tap([1.0, 1.0], 0.0, cap, true, &OPEN),
            DistortionTap::Keep
        );
        assert_eq!(
            refract_tap([1.0, 1.0], 1.0, f32::NAN, true, &OPEN),
            DistortionTap::Offset([0.0, 0.0])
        );
    }

    #[test]
    fn units_and_nearer_geometry_are_never_displaced_or_pulled_in() {
        let cap = 8.0;
        // A unit's own pixel keeps its colour.
        let on_unit = TapScene {
            own_unit: true,
            ..OPEN
        };
        assert_eq!(
            refract_tap([1.0, 0.0], 1.0, cap, true, &on_unit),
            DistortionTap::Keep
        );
        // A tap that would pull a unit's colour out of its silhouette is rejected.
        let next_to_unit = TapScene {
            tap_unit: true,
            ..OPEN
        };
        assert_eq!(
            refract_tap([1.0, 0.0], 1.0, cap, true, &next_to_unit),
            DistortionTap::Keep
        );
        // Geometry nearer than the sprite (a unit without an ID, a wall edge) is rejected
        // by depth alone, mask or not; so is a NaN depth.
        for depth in [39.0, f32::NAN] {
            let in_front = TapScene {
                tap_depth: depth,
                ..OPEN
            };
            for mask in [false, true] {
                assert_eq!(
                    refract_tap([1.0, 0.0], 1.0, cap, mask, &in_front),
                    DistortionTap::Keep
                );
            }
        }
        // Ground behind the haze bends.
        assert!(matches!(
            refract_tap([0.0, -1.0], 1.0, cap, true, &OPEN),
            DistortionTap::Offset([_, y]) if (y + cap).abs() < 1e-5
        ));
        // The ID value for unoutlined units is past the outline palette's slots (1..=7).
        assert!(usize::from(UNIT_ID_PLAIN) >= crate::OUTLINE_SLOTS);
    }
}
