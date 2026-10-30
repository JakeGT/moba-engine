//! Heat-haze distortion (ADR-0019): the per-tier, per-resolution offset cap, the CPU twin of
//! the fragment shader's readability rules (`vfx_distort.frag`) and the CPU-side bounds of
//! the live haze, which pick the frame's copy and draw region (or no haze passes at all).
//!
//! A distortion particle ([`ParticleBlend::Distortion`](crate::ParticleBlend::Distortion))
//! refracts the copy of the HDR target taken after the water and before the particles. Per
//! fragment the shader (and [`refract_tap`]):
//!
//! 1. keeps the pixel where a unit is (the prepass ID target is nonzero: an outlined unit's
//!    slot, or [`UNIT_ID_PLAIN`] for every other unit) or a telegraph's rim or halo band is
//!    (the scene pass wrote [`TELEGRAPH_MARK_BELOW`]-or-less into the HDR alpha there);
//! 2. scales the noise direction by the strength and fades and clamps the offset to the cap
//!    ([`max_offset_pixels`]);
//! 3. rejects a tap whose footprint touches a unit, a marked telegraph band or geometry nearer
//!    the camera than the sprite, keeping the pixel: nothing in front is pulled into the haze
//!    and no telegraph edge moves or bleeds.
//!
//! The sprite itself is depth-tested, so anything in front of it is never touched; the
//! particles (telegraphs, projectiles) draw after it and the outlines and UI after the post
//! chain, so none of them is ever displaced.

use moba_core::math::{Mat4, Vec3, Vec4};

use crate::vfx::VfxBudget;

/// The prepass ID of a unit without an outline (minions, structures, walls, props). Outlined
/// units write their palette slot (1–7); the terrain writes nothing (0). The outline pass
/// treats this value like 0, so the outlines are unchanged.
pub const UNIT_ID_PLAIN: u8 = 8;
/// An HDR alpha below this marks a telegraph band (`1 - coverage` of the band, written by the
/// terrain, foliage and water shaders); every other scene pixel has alpha 1.
pub const TELEGRAPH_MARK_BELOW: f32 = 0.999;
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
/// Haze clusters the CPU tracks; a spawn joins a nearby cluster or the one it grows least.
pub(crate) const HAZE_CLUSTERS: usize = 8;
/// A cluster grows by joining a spawn only while it stays this small on every axis, metres.
const CLUSTER_EXTENT: f32 = 12.0;
/// Pixels added around the draw region for the bilinear tap and rounding.
const COPY_MARGIN: u32 = 2;

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
    /// Leaves the pixel as it is (a unit, a telegraph band, a rejected tap, or no strength).
    Keep,
    /// Writes the copy at the pixel plus this offset (pixels, at most the cap long).
    Offset([f32; 2]),
}

/// What the scene holds where one distortion fragment lands and where its tap lands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TapScene {
    /// The fragment's own pixel is a unit (nonzero ID).
    pub own_unit: bool,
    /// The fragment's own HDR alpha (below [`TELEGRAPH_MARK_BELOW`]: a telegraph band).
    pub own_alpha: f32,
    /// The tap's 2×2 footprint touches a unit.
    pub tap_unit: bool,
    /// The lowest HDR alpha in the tap's 2×2 footprint.
    pub tap_alpha: f32,
    /// The nearest linear view depth in the tap's footprint, metres.
    pub tap_depth: f32,
    /// The sprite's linear view depth at the fragment, metres.
    pub particle_depth: f32,
}

/// Whether an HDR alpha marks a telegraph band (a NaN counts as marked).
#[must_use]
pub fn telegraph_marked(alpha: f32) -> bool {
    !matches!(
        alpha.partial_cmp(&TELEGRAPH_MARK_BELOW),
        Some(core::cmp::Ordering::Greater | core::cmp::Ordering::Equal)
    )
}

/// The CPU twin of `vfx_distort.frag`'s steps 2–4: `direction` is the noise direction in
/// `[-1, 1]²`, `weight` the strength times every fade (`0..=1`) and `cap` the offset cap
/// ([`max_offset_pixels`]).
#[must_use]
pub fn refract_tap(direction: [f32; 2], weight: f32, cap: f32, scene: &TapScene) -> DistortionTap {
    if scene.own_unit || telegraph_marked(scene.own_alpha) {
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
    if scene.tap_unit || telegraph_marked(scene.tap_alpha) || in_front || weight <= 0.0 {
        return DistortionTap::Keep;
    }
    DistortionTap::Offset(offset)
}

/// A pixel rectangle of the render target: `[x0, x1) × [y0, y1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PixelRect {
    pub(crate) x0: u32,
    pub(crate) y0: u32,
    pub(crate) x1: u32,
    pub(crate) y1: u32,
}

impl PixelRect {
    pub(crate) const fn width(self) -> u32 {
        self.x1 - self.x0
    }

    pub(crate) const fn height(self) -> u32 {
        self.y1 - self.y0
    }

    /// Grown by `by` pixels on every side, within `[0, width) × [0, height)`.
    fn grown(self, by: u32, width: u32, height: u32) -> Self {
        Self {
            x0: self.x0.saturating_sub(by),
            y0: self.y0.saturating_sub(by),
            x1: self.x1.saturating_add(by).min(width),
            y1: self.y1.saturating_add(by).min(height),
        }
    }
}

/// This frame's haze passes: where the sprites may draw (the scissor) and the part of the
/// HDR target the copy holds (the draw region plus the cap and a margin, so every tap of a
/// fragment inside `draw` lands in `copy`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HazeRegion {
    pub(crate) draw: PixelRect,
    pub(crate) copy: PixelRect,
}

/// A world-space box holding every position a haze sprite can cover until `until`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HazeBox {
    min: Vec3,
    max: Vec3,
    until: f64,
}

/// The CPU's bounds of the live heat haze (ADR-0019): up to [`HAZE_CLUSTERS`] world boxes,
/// each grown by every distortion spawn that joins it and live until its last sprite can
/// have expired (the slot allocator's clock). A frame's changes are a transaction, like the
/// slot allocator's: [`Self::begin`] rolls back a frame that was never
/// [committed](Self::commit). Fixed size, allocation-free; pure logic, unit-tested headless.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HazeBounds {
    boxes: [Option<HazeBox>; HAZE_CLUSTERS],
    /// The boxes before the open transaction.
    saved: Option<[Option<HazeBox>; HAZE_CLUSTERS]>,
}

/// The world box a sprite spawned at `position` can cover over `lifetime` seconds: its
/// path under `velocity` (drag only slows it) and `gravity` (along +y), grown by the
/// billboard's half diagonal at its largest size.
pub(crate) fn sprite_bounds(
    position: Vec3,
    velocity: Vec3,
    gravity: f32,
    lifetime: f32,
    size: f32,
) -> (Vec3, Vec3) {
    let life = lifetime.max(0.0);
    let travel = velocity * life;
    // Per-frame integration adds at most one 0.1 s step of gravity on top of the exact term.
    let fall = gravity.abs() * life * (life + 0.1);
    let reach = size.max(0.0) * core::f32::consts::FRAC_1_SQRT_2 + 0.05;
    let low = position + travel.min(Vec3::ZERO) - Vec3::new(0.0, fall, 0.0);
    let high = position + travel.max(Vec3::ZERO) + Vec3::new(0.0, fall, 0.0);
    (low - Vec3::splat(reach), high + Vec3::splat(reach))
}

impl HazeBounds {
    pub(crate) const fn new() -> Self {
        Self {
            boxes: [None; HAZE_CLUSTERS],
            saved: None,
        }
    }

    /// Opens this frame's transaction, rolling back an uncommitted previous one.
    pub(crate) fn begin(&mut self) {
        if let Some(saved) = self.saved.take() {
            self.boxes = saved;
        }
        self.saved = Some(self.boxes);
    }

    /// Keeps the open transaction (the frame was submitted).
    pub(crate) fn commit(&mut self) {
        self.saved = None;
    }

    /// Drops every box whose sprites have all expired by `clock`.
    pub(crate) fn expire(&mut self, clock: f64) {
        for slot in &mut self.boxes {
            if slot.is_some_and(|haze| haze.until <= clock) {
                *slot = None;
            }
        }
    }

    /// Whether any haze may be live.
    pub(crate) fn any(&self) -> bool {
        self.boxes.iter().any(Option::is_some)
    }

    /// Adds a sprite covering `[min, max]` until `until`: into a live box it keeps under
    /// [`CLUSTER_EXTENT`], else a free slot, else the box it grows least.
    pub(crate) fn add(&mut self, min: Vec3, max: Vec3, until: f64) {
        if !(min.is_finite() && max.is_finite() && until.is_finite()) {
            return;
        }
        let union = |haze: &HazeBox| (haze.min.min(min), haze.max.max(max));
        let fits = |haze: &HazeBox| {
            let (low, high) = union(haze);
            (high - low).max_element() <= CLUSTER_EXTENT
        };
        let volume = |low: Vec3, high: Vec3| {
            let size = (high - low).max(Vec3::ZERO);
            size.x * size.y * size.z
        };
        let target = self
            .boxes
            .iter()
            .position(|slot| slot.as_ref().is_some_and(fits))
            .or_else(|| self.boxes.iter().position(Option::is_none))
            .or_else(|| {
                (0..HAZE_CLUSTERS).min_by(|&a, &b| {
                    let growth = |index: usize| {
                        self.boxes[index].map_or(0.0, |haze| {
                            let (low, high) = union(&haze);
                            volume(low, high) - volume(haze.min, haze.max)
                        })
                    };
                    growth(a).total_cmp(&growth(b))
                })
            });
        let Some(index) = target else {
            return;
        };
        self.boxes[index] = Some(match self.boxes[index] {
            Some(haze) => {
                let (low, high) = union(&haze);
                HazeBox {
                    min: low,
                    max: high,
                    until: haze.until.max(until),
                }
            }
            None => HazeBox { min, max, until },
        });
    }

    /// The frame's haze region in a `width × height` render target seen through `view_proj`
    /// (the engine's projection with a flipped viewport: NDC +y is the top), padded for a
    /// `cap`-pixel offset; `None` when no live box is in view within the fade distance.
    pub(crate) fn region(
        &self,
        view_proj: Mat4,
        width: u32,
        height: u32,
        cap: f32,
    ) -> Option<HazeRegion> {
        if width == 0 || height == 0 {
            return None;
        }
        let full = PixelRect {
            x0: 0,
            y0: 0,
            x1: width,
            y1: height,
        };
        let mut draw: Option<PixelRect> = None;
        let mut join = |rect: PixelRect| {
            draw = Some(draw.map_or(rect, |old| PixelRect {
                x0: old.x0.min(rect.x0),
                y0: old.y0.min(rect.y0),
                x1: old.x1.max(rect.x1),
                y1: old.y1.max(rect.y1),
            }));
        };
        for haze in self.boxes.iter().flatten() {
            let mut low = [f32::INFINITY; 2];
            let mut high = [f32::NEG_INFINITY; 2];
            let mut nearest = f32::INFINITY;
            let mut behind = 0;
            for corner in 0..8 {
                let point = Vec3::new(
                    if corner & 1 == 0 {
                        haze.min.x
                    } else {
                        haze.max.x
                    },
                    if corner & 2 == 0 {
                        haze.min.y
                    } else {
                        haze.max.y
                    },
                    if corner & 4 == 0 {
                        haze.min.z
                    } else {
                        haze.max.z
                    },
                );
                let clip = view_proj * Vec4::new(point.x, point.y, point.z, 1.0);
                if clip.w <= 1e-3 || !clip.is_finite() {
                    behind += 1;
                    continue;
                }
                nearest = nearest.min(clip.w);
                let ndc = [clip.x / clip.w, clip.y / clip.w];
                for axis in 0..2 {
                    low[axis] = low[axis].min(ndc[axis]);
                    high[axis] = high[axis].max(ndc[axis]);
                }
            }
            if behind == 8 || (behind == 0 && nearest > FADE_END) {
                continue; // Behind the camera, or faded out entirely.
            }
            if behind > 0 {
                // Behind or around the camera: the whole target, conservatively.
                join(full);
                continue;
            }
            if high[0] < -1.0 || low[0] > 1.0 || high[1] < -1.0 || low[1] > 1.0 {
                continue; // Off screen.
            }
            let to_x = |ndc: f32| (ndc.clamp(-1.0, 1.0) * 0.5 + 0.5) * width as f32;
            let to_y = |ndc: f32| (0.5 - ndc.clamp(-1.0, 1.0) * 0.5) * height as f32;
            let rect = PixelRect {
                x0: to_x(low[0]).floor() as u32,
                y0: to_y(high[1]).floor() as u32,
                x1: (to_x(high[0]).ceil() as u32 + 1).min(width),
                y1: (to_y(low[1]).ceil() as u32 + 1).min(height),
            };
            if rect.x0 < rect.x1 && rect.y0 < rect.y1 {
                join(rect);
            }
        }
        let draw = draw?;
        let pad = if cap.is_finite() {
            cap.clamp(0.0, 64.0).ceil() as u32
        } else {
            0
        };
        let copy = draw.grown(pad + COPY_MARGIN, width, height);
        // The shader clamps taps to `[first + 1, last]`: a region under 2 pixels is no haze.
        (copy.width() >= 2 && copy.height() >= 2).then_some(HazeRegion { draw, copy })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DistortionTap, HAZE_CLUSTERS, HazeBounds, PixelRect, TapScene, UNIT_ID_PLAIN,
        max_offset_pixels, refract_tap, sprite_bounds, telegraph_marked,
    };
    use crate::QualityTier;
    use moba_core::math::{Mat4, Vec3, Vec4};

    const OPEN: TapScene = TapScene {
        own_unit: false,
        own_alpha: 1.0,
        tap_unit: false,
        tap_alpha: 1.0,
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
        // Never more than 0.75 % of the render height at 1080p and up (readability).
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
                let tap = refract_tap(direction, weight, cap, &OPEN);
                assert!(length(tap) <= cap + 1e-5, "{direction:?} {weight}: {tap:?}");
            }
        }
        assert_eq!(
            refract_tap([1.0, 0.0], 0.5, cap, &OPEN),
            DistortionTap::Offset([3.0, 0.0])
        );
        // The diagonal is clamped to the cap's length, not its per-axis value.
        assert!((length(refract_tap([1.0, 1.0], 1.0, cap, &OPEN)) - cap).abs() < 1e-5);
        // No strength (an edge, a far haze, an allied haze at 0 opacity): nothing moves.
        assert_eq!(
            refract_tap([1.0, 1.0], 0.0, cap, &OPEN),
            DistortionTap::Keep
        );
        assert_eq!(
            refract_tap([1.0, 1.0], 1.0, f32::NAN, &OPEN),
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
            refract_tap([1.0, 0.0], 1.0, cap, &on_unit),
            DistortionTap::Keep
        );
        // A tap that would pull a unit's colour out of its silhouette is rejected.
        let next_to_unit = TapScene {
            tap_unit: true,
            ..OPEN
        };
        assert_eq!(
            refract_tap([1.0, 0.0], 1.0, cap, &next_to_unit),
            DistortionTap::Keep
        );
        // Geometry nearer than the sprite (a unit without an ID, a wall edge) is rejected
        // by depth alone; so is a NaN depth.
        for depth in [39.0, f32::NAN] {
            let in_front = TapScene {
                tap_depth: depth,
                ..OPEN
            };
            assert_eq!(
                refract_tap([1.0, 0.0], 1.0, cap, &in_front),
                DistortionTap::Keep
            );
        }
        // Ground behind the haze bends.
        assert!(matches!(
            refract_tap([0.0, -1.0], 1.0, cap, &OPEN),
            DistortionTap::Offset([_, y]) if (y + cap).abs() < 1e-5
        ));
        // The ID value for unoutlined units is past the outline palette's slots (1..=7).
        assert!(usize::from(UNIT_ID_PLAIN) >= crate::OUTLINE_SLOTS);
    }

    /// The telegraph mask: a pixel in a telegraph's rim or halo band keeps its colour and a
    /// tap touching one is rejected, so a ring's edge neither moves nor bleeds into the haze;
    /// the mark is exactly what `crate::telegraph_mark` (the decal shader's twin) writes.
    #[test]
    fn telegraph_bands_are_never_displaced_or_pulled_in() {
        use crate::{Decal, DecalRole, DecalShape};
        let cap = 8.0;
        let ring = Decal::new(
            Vec3::ZERO,
            DecalShape::Ring,
            3.0,
            Vec4::new(2.2, 0.8, 0.2, 0.45),
        )
        .with_width(0.3)
        .with_role(DecalRole::EnemyTelegraph);
        let pixel = 0.05;
        // On the band, inside the ring (plain ground) and far outside.
        let band = 1.0 - crate::telegraph_mark(&ring, Vec3::new(3.0, 0.0, 0.0), pixel);
        let inside = 1.0 - crate::telegraph_mark(&ring, Vec3::new(1.0, 0.0, 0.0), pixel);
        let outside = 1.0 - crate::telegraph_mark(&ring, Vec3::new(6.0, 0.0, 0.0), pixel);
        assert!(telegraph_marked(band), "{band}");
        assert!(!telegraph_marked(inside), "{inside}");
        assert!(!telegraph_marked(outside), "{outside}");
        // Allied telegraphs are marked too; cosmetic decals and paint never are.
        let ally = ring.with_role(DecalRole::Ally);
        assert!(crate::telegraph_mark(&ally, Vec3::new(3.0, 0.0, 0.0), pixel) > 0.0);
        let cosmetic = ring.with_role(DecalRole::Cosmetic);
        assert_eq!(
            crate::telegraph_mark(&cosmetic, Vec3::new(3.0, 0.0, 0.0), pixel),
            0.0
        );
        // The band's own pixel keeps its colour; a tap landing on it is rejected; plain
        // ground next to it still bends.
        let on_band = TapScene {
            own_alpha: band,
            ..OPEN
        };
        assert_eq!(
            refract_tap([1.0, 0.0], 1.0, cap, &on_band),
            DistortionTap::Keep
        );
        let towards_band = TapScene {
            tap_alpha: band,
            ..OPEN
        };
        assert_eq!(
            refract_tap([1.0, 0.0], 1.0, cap, &towards_band),
            DistortionTap::Keep
        );
        let ground = TapScene {
            own_alpha: inside,
            tap_alpha: inside,
            ..OPEN
        };
        assert!(matches!(
            refract_tap([1.0, 0.0], 1.0, cap, &ground),
            DistortionTap::Offset(_)
        ));
        // Alpha 1 (every unmarked scene pixel) is never marked; NaN is.
        assert!(!telegraph_marked(1.0));
        assert!(telegraph_marked(0.5));
        assert!(telegraph_marked(f32::NAN));
    }

    fn camera() -> Mat4 {
        // The engine's camera (`Camera::perspective`), looking down at the origin.
        crate::Camera::perspective(
            Vec3::new(0.0, 20.0, 12.0),
            Vec3::ZERO,
            Vec3::Y,
            0.8,
            16.0 / 9.0,
            0.5,
            400.0,
        )
        .view_proj
    }

    #[test]
    fn haze_bounds_cover_every_sprite_and_expire() {
        // A sprite rising at 1 m/s for 2 s, 1.5 m wide: its path and half diagonal.
        let (low, high) = sprite_bounds(Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0), 0.0, 2.0, 1.5);
        assert!(low.y <= -1.06 && high.y >= 3.06, "{low} {high}");
        assert!(low.x <= -1.06 && high.x >= 1.06);
        // Gravity both ways, drag never widens it.
        let (low, high) = sprite_bounds(Vec3::ZERO, Vec3::ZERO, -9.8, 1.0, 0.0);
        assert!(low.y <= -4.9 && high.y >= 4.9);

        let mut bounds = HazeBounds::new();
        assert!(!bounds.any());
        bounds.add(Vec3::ZERO, Vec3::ONE, 3.0);
        // A nearby spawn joins the box, a far one takes another slot.
        bounds.add(Vec3::splat(1.0), Vec3::splat(2.0), 5.0);
        bounds.add(Vec3::splat(100.0), Vec3::splat(101.0), 4.0);
        assert_eq!(bounds.boxes.iter().flatten().count(), 2);
        bounds.expire(4.0);
        assert_eq!(
            bounds.boxes.iter().flatten().count(),
            1,
            "the far box expired"
        );
        bounds.expire(5.0);
        assert!(!bounds.any());
        // More clusters than slots: the extra spawn grows a box, none is lost.
        for index in 0..HAZE_CLUSTERS + 3 {
            let at = Vec3::new(index as f32 * 50.0, 0.0, 0.0);
            bounds.add(at, at + Vec3::ONE, 9.0);
        }
        assert_eq!(bounds.boxes.iter().flatten().count(), HAZE_CLUSTERS);
        for index in 0..HAZE_CLUSTERS + 3 {
            let at = Vec3::new(index as f32 * 50.0 + 0.5, 0.5, 0.5);
            assert!(
                bounds
                    .boxes
                    .iter()
                    .flatten()
                    .any(|haze| { at.cmpge(haze.min).all() && at.cmple(haze.max).all() }),
                "{index}"
            );
        }
        // Non-finite input is ignored.
        let before = bounds;
        bounds.add(Vec3::splat(f32::NAN), Vec3::ONE, 1.0);
        assert_eq!(bounds, before);
    }

    /// A frame that fails after `prepare` (never submitted) leaves no haze behind: the next
    /// frame starts from the last committed bounds, as the slot allocator does.
    #[test]
    fn haze_bounds_roll_back_a_frame_that_was_not_submitted() {
        let mut bounds = HazeBounds::new();
        bounds.begin();
        bounds.add(Vec3::ZERO, Vec3::ONE, 5.0);
        bounds.commit();
        let committed = bounds.boxes;
        bounds.begin();
        bounds.add(Vec3::splat(100.0), Vec3::splat(101.0), 9.0);
        assert_eq!(bounds.boxes.iter().flatten().count(), 2);
        bounds.begin();
        assert_eq!(bounds.boxes, committed, "the unsubmitted spawn is gone");
        bounds.expire(5.0);
        assert!(!bounds.any());
        bounds.commit();
        bounds.begin();
        assert!(!bounds.any(), "the committed expiry stays");
    }

    #[test]
    fn the_region_covers_the_projected_haze_plus_the_cap() {
        let view_proj = camera();
        let (width, height) = (1920, 1080);
        let mut bounds = HazeBounds::new();
        assert_eq!(
            bounds.region(view_proj, width, height, 8.0),
            None,
            "no haze"
        );
        let (low, high) = (Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 2.0, 1.0));
        bounds.add(low, high, 10.0);
        let region = bounds
            .region(view_proj, width, height, 8.0)
            .expect("in view");
        // Every corner of the box projects inside the draw region (flipped viewport).
        for corner in 0..8 {
            let point = Vec3::new(
                if corner & 1 == 0 { low.x } else { high.x },
                if corner & 2 == 0 { low.y } else { high.y },
                if corner & 4 == 0 { low.z } else { high.z },
            );
            let clip = view_proj * point.extend(1.0);
            let x = (clip.x / clip.w * 0.5 + 0.5) * width as f32;
            let y = (0.5 - clip.y / clip.w * 0.5) * height as f32;
            let draw = region.draw;
            assert!(x >= draw.x0 as f32 && x <= draw.x1 as f32, "{x} {draw:?}");
            assert!(y >= draw.y0 as f32 && y <= draw.y1 as f32, "{y} {draw:?}");
        }
        // A small haze is a small part of the frame; the copy adds the cap and the margin.
        let draw = region.draw;
        assert!(
            draw.width() * draw.height() < width * height / 8,
            "{draw:?}"
        );
        assert_eq!(region.copy.x0, draw.x0.saturating_sub(10));
        assert_eq!(region.copy.x1, (draw.x1 + 10).min(width));
        // Off screen, behind the camera or past the fade distance: no haze passes.
        for at in [
            Vec3::new(200.0, 0.0, 0.0),
            Vec3::new(0.0, 40.0, 40.0),
            Vec3::new(0.0, -151.0, -91.0),
        ] {
            let mut away = HazeBounds::new();
            away.add(at, at + Vec3::ONE, 10.0);
            assert_eq!(away.region(view_proj, width, height, 8.0), None, "{at}");
        }
        // Around the camera: the whole target.
        let mut around = HazeBounds::new();
        around.add(Vec3::new(-1.0, 15.0, 8.0), Vec3::new(1.0, 25.0, 16.0), 10.0);
        let whole = around.region(view_proj, width, height, 8.0).expect("whole");
        assert_eq!(
            whole.copy,
            PixelRect {
                x0: 0,
                y0: 0,
                x1: width,
                y1: height
            }
        );
        // An empty target never has a region.
        assert_eq!(bounds.region(view_proj, 0, height, 8.0), None);
    }
}
