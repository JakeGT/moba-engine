//! Temporal presentation state (ADR-0021 §1, §2c): the sub-pixel jitter, the per-frame camera
//! matrices that the tiered path's temporal passes read, and the automatic history resets.
//!
//! Everything here is presentation state inside `moba-render` (ADR-0021 §8). The client's
//! [`Camera`](crate::Camera) is never jittered and never mutated: picking, the HUD's world
//! anchors and CPU culling keep reading its unjittered `view_proj`. Only the GPU uniforms of
//! the main view get the jittered matrix.
//!
//! - **The sequence** is Halton (2, 3), index from 1, offsets in render pixels in
//!   `[-0.5, 0.5)`. The phase count is `ceil(8 r²)`, `r` the output ÷ render ratio rounded to
//!   three decimals ([`jitter_phases`]); the index runs on and is taken modulo the current
//!   count, so a dynamic-resolution step does not restart the pattern ([`Jitter`]).
//! - **Application** is a clip-space translation of the rasterised projection
//!   ([`jittered`]): `+2 jx / render_w` and `-2 jy / render_h`. The scene draws through a
//!   flipped viewport (NDC +Y is the top of the image), so that moves every sample by
//!   `(+jx, +jy)` render pixels, x right and y down (tested below).
//! - **Motion vectors** are in render pixels, x right and y down, from the current sample to
//!   its position in the previous frame, without jitter ([`motion_pixels`], the CPU twin of
//!   the prepass shaders).

use moba_core::math::{Mat4, Vec3, Vec4};

/// The render or output size of one frame, in pixels.
pub(crate) type Extent = ash::vk::Extent2D;

/// The radical inverse of `index` in `base` (`index >= 1` for the Halton sequence).
#[must_use]
pub(crate) fn halton(mut index: u32, base: u32) -> f32 {
    let mut fraction = 1.0f64;
    let mut result = 0.0f64;
    let base_f = f64::from(base);
    while index > 0 {
        fraction /= base_f;
        result += fraction * f64::from(index % base);
        index /= base;
    }
    result as f32
}

/// `round(output / render, 3 decimals)` per axis, in thousandths; the larger axis (they are
/// equal up to rounding: the controller scales both together, ADR-0021 §4c).
fn ratio_milli(render: Extent, output: Extent) -> u64 {
    let axis = |output: u32, render: u32| {
        let render = u64::from(render.max(1));
        // round(1000 * output / render) in integers.
        (1000 * u64::from(output) + render / 2) / render
    };
    axis(output.width, render.width).max(axis(output.height, render.height))
}

/// Jitter phases for a frame rendered at `render` and shown at `output`: `ceil(8 r²)`, `r`
/// rounded to three decimals first (ADR-0021 §1). 8 at native, 18 at 1.5×, 24 at 1.7× and
/// DLSS's 1.724×, 32 at 2×, 72 at 3×. Exact integer arithmetic, so a ratio that squares to
/// a whole number never rounds up by a floating-point hair.
#[must_use]
pub(crate) fn jitter_phases(render: Extent, output: Extent) -> u32 {
    let milli = ratio_milli(render, output).max(1000);
    let scaled = 8 * milli * milli;
    let phases = scaled.div_ceil(1_000_000);
    u32::try_from(phases).unwrap_or(u32::MAX)
}

/// The jitter of a presented frame: a presentation counter (never the tick or the wall
/// clock), advanced once per presented main-view frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Jitter {
    index: u64,
}

impl Jitter {
    /// The offset in render pixels, `[-0.5, 0.5)` per axis, for this frame with `phases`
    /// phases: Halton (2, 3) at `1 + index mod phases`.
    #[must_use]
    pub(crate) fn offset(self, phases: u32) -> [f32; 2] {
        let phases = u64::from(phases.max(1));
        let sample = u32::try_from(self.index % phases).unwrap_or(0) + 1;
        [halton(sample, 2) - 0.5, halton(sample, 3) - 0.5]
    }

    /// Moves on to the next presented frame.
    pub(crate) fn advance(&mut self) {
        self.index = self.index.wrapping_add(1);
    }

    /// Back to the first phase (captures, ADR-0021 §1).
    pub(crate) fn reset(&mut self) {
        self.index = 0;
    }

    /// The counter, for tests and logs.
    #[cfg(test)]
    pub(crate) fn index(self) -> u64 {
        self.index
    }
}

/// `view_proj` translated in clip space by `jitter` render pixels (x right, y down) for a
/// `render` sized frame drawn through the flipped viewport.
#[must_use]
pub(crate) fn jittered(view_proj: Mat4, jitter: [f32; 2], render: Extent) -> Mat4 {
    let dx = 2.0 * jitter[0] / render.width.max(1) as f32;
    let dy = -2.0 * jitter[1] / render.height.max(1) as f32;
    // clip' = T * clip with T adding (dx, dy) * w to x and y: an NDC translation.
    let translate = Mat4::from_cols(
        Vec4::new(1.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(dx, dy, 0.0, 1.0),
    );
    translate * view_proj
}

/// The pixel position (x right, y down, pixel centres at `+0.5`) of clip-space `clip` in a
/// `render` sized frame drawn through the flipped viewport, or `None` behind the eye.
#[must_use]
pub(crate) fn clip_to_pixels(clip: Vec4, render: Extent) -> Option<[f32; 2]> {
    if clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    let width = render.width as f32;
    let height = render.height as f32;
    // Flipped viewport (`passes::flipped_viewport`): NDC +Y is the top row.
    Some([(ndc.x * 0.5 + 0.5) * width, (0.5 - ndc.y * 0.5) * height])
}

/// The CPU twin of the prepass's motion output: the render-pixel offset from the current
/// position of `world` to its previous one (`previous_world`, under `previous_view_proj`),
/// both unjittered. `None` when either is behind the eye.
#[must_use]
pub(crate) fn motion_pixels(
    view_proj: Mat4,
    world: Vec3,
    previous_view_proj: Mat4,
    previous_world: Vec3,
    render: Extent,
) -> Option<[f32; 2]> {
    let current = clip_to_pixels(view_proj * world.extend(1.0), render)?;
    let previous = clip_to_pixels(previous_view_proj * previous_world.extend(1.0), render)?;
    Some([previous[0] - current[0], previous[1] - current[1]])
}

/// Which mip-bias formula a back end documents (ADR-0021 §1, "Texture LOD bias").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // `Off` documents temporal off.
pub(crate) enum MipBiasRule {
    /// DLSS, FSR 3.1, FSR 4 and TAAU: `log2(render_w / output_w) - 1`.
    Standard,
    /// XeSS: `log2(render_w / output_w)`.
    Xess,
    /// Temporal off: 0.
    Off,
}

impl MipBiasRule {
    /// The rule the back end `kind` documents (N1c: XeSS's differs).
    #[must_use]
    pub(crate) fn for_kind(kind: crate::post::UpscalerKind) -> Self {
        match kind {
            crate::post::UpscalerKind::Xess => Self::Xess,
            _ => Self::Standard,
        }
    }
}

/// The material texture LOD bias for this frame.
#[must_use]
pub(crate) fn mip_bias(rule: MipBiasRule, render_width: u32, output_width: u32) -> f32 {
    let ratio = render_width.max(1) as f32 / output_width.max(1) as f32;
    match rule {
        MipBiasRule::Standard => ratio.log2() - 1.0,
        MipBiasRule::Xess => ratio.log2(),
        MipBiasRule::Off => 0.0,
    }
}

/// Eye travel in one frame that resets every temporal history (ADR-0021 §2c), metres.
pub(crate) const RESET_EYE_DISTANCE: f32 = 8.0;
/// View rotation in one frame that resets every temporal history, degrees.
pub(crate) const RESET_VIEW_DEGREES: f32 = 15.0;

/// The main view of the previous presented frame, for motion and the automatic resets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PreviousView {
    /// The unjittered view-projection.
    pub(crate) view_proj: Mat4,
    /// The eye position.
    pub(crate) eye: Vec3,
    /// The view direction (unit), from the inverse view-projection.
    pub(crate) forward: Vec3,
    /// The jitter in render pixels.
    pub(crate) jitter: [f32; 2],
}

/// The unit view direction of `view_proj` (through the centre of the image).
#[must_use]
pub(crate) fn view_forward(view_proj: Mat4) -> Vec3 {
    let inverse = view_proj.inverse();
    let near = inverse * Vec4::new(0.0, 0.0, 0.0, 1.0);
    let far = inverse * Vec4::new(0.0, 0.0, 1.0, 1.0);
    if near.w.abs() < f32::EPSILON || far.w.abs() < f32::EPSILON {
        return Vec3::NEG_Z;
    }
    let direction = far.truncate() / far.w - near.truncate() / near.w;
    let length = direction.length();
    if length.is_finite() && length > 0.0 {
        direction / length
    } else {
        Vec3::NEG_Z
    }
}

impl PreviousView {
    /// Whether moving from `self` to (`eye`, `forward`) is a camera cut: more than
    /// [`RESET_EYE_DISTANCE`] of travel or [`RESET_VIEW_DEGREES`] of rotation.
    #[must_use]
    pub(crate) fn is_cut(&self, eye: Vec3, forward: Vec3) -> bool {
        let travel = (eye - self.eye).length();
        let cosine = self.forward.dot(forward).clamp(-1.0, 1.0);
        !travel.is_finite()
            || travel > RESET_EYE_DISTANCE
            || cosine < RESET_VIEW_DEGREES.to_radians().cos()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moba_core::math::{look_at_rh, perspective_rh};

    fn extent(width: u32, height: u32) -> Extent {
        Extent { width, height }
    }

    #[test]
    fn halton_values_match_the_sequence() {
        let base2: Vec<f32> = (1..=8).map(|i| halton(i, 2)).collect();
        assert_eq!(base2, [0.5, 0.25, 0.75, 0.125, 0.625, 0.375, 0.875, 0.0625]);
        let base3: Vec<f32> = (1..=4).map(|i| halton(i, 3)).collect();
        let expected = [1.0 / 3.0, 2.0 / 3.0, 1.0 / 9.0, 4.0 / 9.0];
        for (value, want) in base3.iter().zip(expected) {
            assert!((value - want).abs() < 1e-7, "{value} vs {want}");
        }
    }

    #[test]
    fn offsets_stay_in_the_half_open_pixel() {
        let mut jitter = Jitter::default();
        for _ in 0..1000 {
            let [x, y] = jitter.offset(72);
            assert!((-0.5..0.5).contains(&x) && (-0.5..0.5).contains(&y));
            jitter.advance();
        }
    }

    #[test]
    fn phase_counts_follow_the_ceil_rule() {
        // Native, Quality 1.5x (2560 / 1707 rounds to 1.500), 1.7x, DLSS Balanced 1.724x,
        // Performance 2x and Ultra Performance 3x.
        assert_eq!(jitter_phases(extent(1920, 1080), extent(1920, 1080)), 8);
        assert_eq!(jitter_phases(extent(1707, 960), extent(2560, 1440)), 18);
        assert_eq!(jitter_phases(extent(1280, 720), extent(1920, 1080)), 18);
        assert_eq!(jitter_phases(extent(1506, 847), extent(2560, 1440)), 24);
        assert_eq!(jitter_phases(extent(1000, 1000), extent(1700, 1700)), 24);
        assert_eq!(jitter_phases(extent(1000, 1000), extent(1724, 1724)), 24);
        assert_eq!(jitter_phases(extent(1280, 720), extent(2560, 1440)), 32);
        assert_eq!(jitter_phases(extent(1280, 720), extent(3840, 2160)), 72);
        // A render size above the output never drops below the native count.
        assert_eq!(jitter_phases(extent(4000, 4000), extent(2000, 2000)), 8);
    }

    #[test]
    fn the_index_survives_a_scale_step() {
        let mut jitter = Jitter::default();
        for _ in 0..30 {
            jitter.advance();
        }
        // At 18 phases frame 30 is phase 12 (Halton index 13); after a dynamic-resolution step
        // to 32 phases it is phase 30 (index 31): the counter runs on, it does not restart.
        assert_eq!(jitter.index(), 30);
        assert_eq!(
            jitter.offset(18),
            [halton(13, 2) - 0.5, halton(13, 3) - 0.5]
        );
        assert_eq!(
            jitter.offset(32),
            [halton(31, 2) - 0.5, halton(31, 3) - 0.5]
        );
        jitter.advance();
        assert_eq!(
            jitter.offset(32),
            [halton(32, 2) - 0.5, halton(32, 3) - 0.5]
        );
        jitter.reset();
        assert_eq!(jitter.offset(32), [0.0, halton(1, 3) - 0.5]);
    }

    /// The sign convention (ADR-0021 §1): the jittered matrix moves every sample by
    /// `(+jx, +jy)` render pixels, x right and y down, through the flipped viewport.
    #[test]
    fn jitter_moves_samples_right_and_down_in_pixels() {
        let render = extent(640, 360);
        let view_proj = perspective_rh(1.0, 640.0 / 360.0, 0.1, 100.0)
            * look_at_rh(Vec3::new(3.0, 8.0, 6.0), Vec3::ZERO, Vec3::Y);
        let jitter = [0.25, -0.375];
        let shifted = jittered(view_proj, jitter, render);
        for point in [
            Vec3::ZERO,
            Vec3::new(1.0, 0.5, -2.0),
            Vec3::new(-3.0, 1.0, 2.0),
        ] {
            let plain = clip_to_pixels(view_proj * point.extend(1.0), render).unwrap();
            let moved = clip_to_pixels(shifted * point.extend(1.0), render).unwrap();
            assert!(
                (moved[0] - plain[0] - jitter[0]).abs() < 1e-3,
                "{plain:?} {moved:?}"
            );
            assert!(
                (moved[1] - plain[1] - jitter[1]).abs() < 1e-3,
                "{plain:?} {moved:?}"
            );
        }
        // Depth is untouched.
        let point = Vec3::new(1.0, 0.5, -2.0).extend(1.0);
        let (a, b) = (view_proj * point, shifted * point);
        assert!((a.z / a.w - b.z / b.w).abs() < 1e-7);
        // The world's +Y (up the screen at this camera) is the top of the image: y down.
        let low = clip_to_pixels(view_proj * Vec3::ZERO.extend(1.0), render).unwrap();
        let high = clip_to_pixels(view_proj * Vec3::Y.extend(1.0), render).unwrap();
        assert!(high[1] < low[1]);
    }

    #[test]
    fn motion_points_to_the_previous_position() {
        let render = extent(800, 600);
        let view_proj = perspective_rh(1.0, 800.0 / 600.0, 0.1, 100.0)
            * look_at_rh(Vec3::new(0.0, 10.0, 10.0), Vec3::ZERO, Vec3::Y);
        // A unit that moved +x since the last frame points back towards -x.
        let motion = motion_pixels(
            view_proj,
            Vec3::new(1.0, 0.0, 0.0),
            view_proj,
            Vec3::ZERO,
            render,
        )
        .unwrap();
        assert!(motion[0] < 0.0 && motion[1].abs() < 1e-3, "{motion:?}");
        // A static point under a static camera does not move.
        assert_eq!(
            motion_pixels(view_proj, Vec3::ONE, view_proj, Vec3::ONE, render),
            Some([0.0, 0.0])
        );
    }

    #[test]
    fn mip_bias_follows_each_back_end() {
        use crate::post::UpscalerKind;
        let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
        assert!(close(mip_bias(MipBiasRule::Standard, 1707, 2560), -1.585));
        assert!(close(mip_bias(MipBiasRule::Standard, 1280, 2560), -2.0));
        assert!(close(mip_bias(MipBiasRule::Standard, 1280, 3840), -2.585));
        assert!(close(mip_bias(MipBiasRule::Standard, 2560, 2560), -1.0));
        assert!(close(mip_bias(MipBiasRule::Xess, 1280, 2560), -1.0));
        assert!(close(mip_bias(MipBiasRule::Xess, 1113, 2560), -1.202));
        assert_eq!(mip_bias(MipBiasRule::Off, 1280, 2560), 0.0);
        assert_eq!(MipBiasRule::for_kind(UpscalerKind::Xess), MipBiasRule::Xess);
        for kind in [
            UpscalerKind::Taau,
            UpscalerKind::Fsr3,
            UpscalerKind::Dlss,
            UpscalerKind::Fsr4,
        ] {
            assert_eq!(MipBiasRule::for_kind(kind), MipBiasRule::Standard);
        }
    }

    #[test]
    fn camera_cuts_reset_the_history() {
        let view_proj = perspective_rh(1.0, 1.5, 0.1, 100.0)
            * look_at_rh(Vec3::new(0.0, 10.0, 10.0), Vec3::ZERO, Vec3::Y);
        let previous = PreviousView {
            view_proj,
            eye: Vec3::new(0.0, 10.0, 10.0),
            forward: view_forward(view_proj),
            jitter: [0.0; 2],
        };
        assert!(!previous.is_cut(previous.eye + Vec3::X * 7.9, previous.forward));
        assert!(previous.is_cut(previous.eye + Vec3::X * 8.1, previous.forward));
        let turned = |degrees: f32| {
            let axis = previous.forward.cross(Vec3::Y).normalize();
            let rotation = moba_core::math::Quat::from_axis_angle(axis, degrees.to_radians());
            rotation * previous.forward
        };
        assert!(!previous.is_cut(previous.eye, turned(14.0)));
        assert!(previous.is_cut(previous.eye, turned(16.0)));
        let expected = (Vec3::ZERO - previous.eye).normalize();
        assert!((previous.forward - expected).length() < 1e-4);
    }
}
