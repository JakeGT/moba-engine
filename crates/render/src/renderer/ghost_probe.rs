//! The ghost-energy probe (ADR-0021 verification gate 3): how much of a fast unit's wake
//! still shows it, frame by frame.
//!
//! A hero-sized box dashes at 10 m per 0.2 s at 120 fps (0.42 m per frame) and a small
//! projectile crosses the screen at 1 m per frame, both keyed so they write their own motion,
//! over a static camera. The sequence is rendered twice from the same jitter phase: once with
//! history, and once with every frame a history reset (the no-history reference, the same
//! frame with nothing accumulated). On each of the last [`GHOST_FRAMES`] frames, the **wake**
//! is every output pixel either moving object covered on one of the previous
//! [`WAKE_FRAMES`] frames and is more than [`EDGE_MARGIN_PX`] from where it is now. A wake
//! pixel **ghosts** when any channel of the history render differs from the reference by more
//! than [`GHOST_THRESHOLD`] (4/255). Wake pixels where antialiasing, not ghosting, moves the
//! image are left out and counted: edges of the still scene (a 3x3 neighbourhood spanning
//! more than [`EDGE_RANGE`]) and pixels where the same sequence without the movers already
//! differs from its own no-history render. The probe passes when the ghosting share of the
//! wake stays at most [`GHOST_BOUND`] on every measured frame.
//!
//! Diagnostic only, like the motion probe: run it on a fresh renderer that then exits (the
//! client's `MOBA_GHOST_PROBE=1`), with fixed exposure (`MOBA_EXPOSURE=fixed`).

use core::fmt;

use moba_core::math::{Mat4, Vec2, Vec3, Vec4};

use super::Renderer;
use crate::error::RenderResult;
use crate::scene::{Camera, Scene, UnitInstance};
use crate::temporal::{Extent, clip_to_pixels};

/// Frames measured (ADR-0021 gate 3: "over 20 frames").
pub(crate) const GHOST_FRAMES: usize = 20;
/// Frames rendered before the first measured one, so the wake has formed.
const LEAD_IN: usize = 4;
/// How far back the wake reaches, frames (history at weight 0.9 is below 4/255 of a full
/// contrast step after about 50 frames, but a visible trail is the last few).
pub(crate) const WAKE_FRAMES: usize = 12;
/// Wake pixels this close to the object's current silhouette are its antialiased edge.
pub(crate) const EDGE_MARGIN_PX: f32 = 2.0;
/// A still-scene pixel whose 3x3 neighbourhood spans more than this in a channel is an
/// edge, where antialiasing (not ghosting) moves pixels; such wake pixels are left out.
const EDGE_RANGE: u8 = 8;
/// A pixel ghosts when a channel differs from the no-history reference by more than this.
pub(crate) const GHOST_THRESHOLD: u8 = 4;
/// The largest ghosting share of the wake allowed on any measured frame (ADR-0021 amendment:
/// the gate's fixed bound). TAAU measured 2.39 % at Quality and 1.20 % at native on
/// lavapipe when it was set; the residue is the trailing edge of the hero's moving shadow
/// (shadows have no motion of their own, and a still pixel's clip box is the wider γ 1.25)
/// and faint grid lines re-converging after the disocclusion.
pub(crate) const GHOST_BOUND: f32 = 0.03;

/// The hero's dash per frame: 10 m in 0.2 s at 120 fps.
const DASH_STEP: f32 = 10.0 / 0.2 / 120.0;
/// The projectile's step per frame.
const PROJECTILE_STEP: f32 = 1.0;

/// One measured frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct GhostFrame {
    /// Wake pixels.
    pub wake: u32,
    /// Wake pixels that differ from the no-history reference by more than 4/255.
    pub ghost: u32,
    /// Pixels anywhere that differ by more than 4/255 (the antialiasing's own share
    /// included), for context.
    pub changed: u32,
    /// Wake pixels left out as antialiasing: an edge of the still scene, or a pixel the
    /// antialiasing alone moves by more than 4/255 (the same frame without the movers
    /// differs from its no-history render there).
    pub antialiasing: u32,
}

impl GhostFrame {
    /// The ghosting share of the wake.
    #[must_use]
    pub fn share(&self) -> f32 {
        if self.wake == 0 {
            0.0
        } else {
            self.ghost as f32 / self.wake as f32
        }
    }
}

/// The result of [`Renderer::ghost_probe`].
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct GhostProbeReport {
    /// Output resolution.
    pub output: [u32; 2],
    /// Render resolution.
    pub render: [u32; 2],
    /// The measured frames.
    pub frames: Vec<GhostFrame>,
}

impl GhostProbeReport {
    /// The largest ghosting share of the wake.
    #[must_use]
    pub fn worst(&self) -> f32 {
        self.frames
            .iter()
            .map(GhostFrame::share)
            .fold(0.0, f32::max)
    }

    /// Every measured frame has a wake and stays within `GHOST_BOUND` (3 %).
    #[must_use]
    pub fn passed(&self) -> bool {
        self.frames.len() == GHOST_FRAMES
            && self
                .frames
                .iter()
                .all(|frame| frame.wake > 0 && frame.share() <= GHOST_BOUND)
    }
}

impl fmt::Display for GhostProbeReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "ghost probe: output {}x{}, render {}x{}, wake {} frames, edge {} px, threshold \
             {}/255, bound {:.1} %",
            self.output[0],
            self.output[1],
            self.render[0],
            self.render[1],
            WAKE_FRAMES,
            EDGE_MARGIN_PX,
            GHOST_THRESHOLD,
            GHOST_BOUND * 100.0
        )?;
        for (index, frame) in self.frames.iter().enumerate() {
            writeln!(
                f,
                "  frame {:>2}: wake={:<6} ghost={:<5} share={:.3} % (antialiasing {} left \
                 out, {} changed in the frame)",
                index + LEAD_IN,
                frame.wake,
                frame.ghost,
                frame.share() * 100.0,
                frame.antialiasing,
                frame.changed
            )?;
        }
        write!(
            f,
            "ghost probe: worst {:.3} % ({})",
            self.worst() * 100.0,
            if self.passed() { "passed" } else { "FAILED" }
        )
    }
}

/// The moving objects at frame `frame`: (transform, colour, motion key).
fn movers(frame: usize) -> [(Mat4, Vec4, u32); 2] {
    let step = frame as f32;
    let hero = Mat4::from_scale_rotation_translation(
        Vec3::new(0.9, 1.8, 0.9),
        moba_core::math::Quat::IDENTITY,
        Vec3::new(-5.0 + DASH_STEP * step, 0.9, 0.5),
    );
    let projectile = Mat4::from_scale_rotation_translation(
        Vec3::splat(0.35),
        moba_core::math::Quat::IDENTITY,
        Vec3::new(11.0 - PROJECTILE_STEP * step, 1.0, -3.0),
    );
    [
        (hero, Vec4::new(0.8, 0.15, 0.1, 1.0), 1),
        (projectile, Vec4::new(1.0, 0.9, 0.2, 1.0), 2),
    ]
}

/// The convex hull of a box's projected corners, counter-clockwise in pixels (y down), or
/// empty when a corner is behind the eye.
fn silhouette(view_proj: Mat4, transform: Mat4, output: Extent) -> Vec<Vec2> {
    let mut points = Vec::with_capacity(8);
    for corner in 0..8u8 {
        let local = Vec3::new(
            if corner & 1 == 0 { -0.5 } else { 0.5 },
            if corner & 2 == 0 { -0.5 } else { 0.5 },
            if corner & 4 == 0 { -0.5 } else { 0.5 },
        );
        let world = transform.transform_point3(local);
        let Some(pixel) = clip_to_pixels(view_proj * world.extend(1.0), output) else {
            return Vec::new();
        };
        points.push(Vec2::from_array(pixel));
    }
    convex_hull(points)
}

/// Andrew's monotone chain.
fn convex_hull(mut points: Vec<Vec2>) -> Vec<Vec2> {
    points.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
    let mut hull: Vec<Vec2> = Vec::with_capacity(points.len() * 2);
    for pass in 0..2 {
        let start = hull.len();
        let ordered: Box<dyn Iterator<Item = &Vec2>> = if pass == 0 {
            Box::new(points.iter())
        } else {
            Box::new(points.iter().rev())
        };
        for &point in ordered {
            while hull.len() >= start + 2
                && cross(hull[hull.len() - 2], hull[hull.len() - 1], point) <= 0.0
            {
                hull.pop();
            }
            hull.push(point);
        }
        hull.pop();
    }
    hull
}

/// The smallest signed distance from `point` to the hull's edges (positive inside).
fn inside_distance(hull: &[Vec2], point: Vec2) -> f32 {
    if hull.len() < 3 {
        return f32::NEG_INFINITY;
    }
    let mut distance = f32::INFINITY;
    for (index, &a) in hull.iter().enumerate() {
        let b = hull[(index + 1) % hull.len()];
        let edge = b - a;
        let length = edge.length();
        if length <= 0.0 {
            continue;
        }
        distance = distance.min(edge.perp_dot(point - a) / length);
    }
    distance
}

impl Renderer {
    /// Runs the ghost-energy probe (ADR-0021 verification gate 3; module docs of
    /// `renderer/ghost_probe.rs`). Dynamic resolution is held off while it runs.
    ///
    /// # Errors
    /// When temporal anti-aliasing is off, or from rendering and the readbacks.
    pub fn ghost_probe(&mut self) -> RenderResult<GhostProbeReport> {
        if self.temporal.is_none() {
            return Err(crate::error::RenderError::NoSuitableDevice(String::from(
                "ghost probe: temporal anti-aliasing is off",
            )));
        }
        let dynres = self.dynres.take();
        let result = self.ghost_probe_runs();
        self.dynres = dynres;
        result
    }

    #[allow(clippy::too_many_lines)] // Four runs, then the per-pixel measure, in order.
    fn ghost_probe_runs(&mut self) -> RenderResult<GhostProbeReport> {
        let output = self.swapchain.extent;
        let aspect = output.width as f32 / output.height.max(1) as f32;
        let camera = Camera::perspective(
            Vec3::new(0.0, 13.0, 9.0),
            Vec3::new(0.0, 0.0, 0.5),
            Vec3::Y,
            0.8,
            aspect,
            1.0,
            60.0,
        );
        let total = LEAD_IN + GHOST_FRAMES;
        let backdrop: Vec<UnitInstance> = (0..9)
            .map(|index| {
                let x = -8.0 + 2.0 * index as f32;
                let z = if index % 2 == 0 { -1.5 } else { 2.5 };
                UnitInstance::new(
                    Mat4::from_translation(Vec3::new(x, 0.5, z)),
                    Vec4::new(0.25, 0.3, 0.45, 1.0),
                )
            })
            .collect();
        // History with the movers, its no-history reference, and the same two without them
        // (the antialiasing's own difference, which is not ghosting).
        let mut runs: [Vec<Vec<u8>>; 4] = core::array::from_fn(|_| Vec::with_capacity(total));
        for (run, frames) in runs.iter_mut().enumerate() {
            let reference = run % 2 == 1;
            let with_movers = run < 2;
            self.reset_temporal();
            for frame in 0..total {
                let mut units = backdrop.clone();
                if with_movers {
                    for (transform, colour, key) in movers(frame) {
                        units.push(
                            UnitInstance::new(transform, colour).with_motion(key, frame == 0),
                        );
                    }
                }
                let scene = Scene::new(camera, &units).with_history_reset(reference);
                // The sequence is the history: no capture warm-up between its frames.
                self.capture_warmed = true;
                let captured = self.capture_frame_internal(&scene, None, &[])?;
                frames.push(captured.rgba);
            }
        }
        let render = self.render_extent;
        let silhouettes: Vec<Vec<Vec<Vec2>>> = (0..total)
            .map(|frame| {
                movers(frame)
                    .iter()
                    .map(|(transform, _, _)| silhouette(camera.view_proj, *transform, output))
                    .collect()
            })
            .collect();
        let [history, reference, still_history, still_reference] = &runs;
        let differs = |a: &[u8], b: &[u8], offset: usize| {
            a[offset..offset + 3]
                .iter()
                .zip(&b[offset..offset + 3])
                .any(|(a, b)| a.abs_diff(*b) > GHOST_THRESHOLD)
        };
        let (width, height) = (output.width as usize, output.height as usize);
        // An edge of the still scene: its 3x3 neighbourhood spans more than EDGE_RANGE in a
        // channel. TAA's own antialiasing and re-convergence move those pixels.
        let edge = |image: &[u8], x: usize, y: usize| {
            (0..3).any(|channel| {
                let (mut low, mut high) = (u8::MAX, u8::MIN);
                for ny in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                    for nx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                        let value = image[(ny * width + nx) * 4 + channel];
                        low = low.min(value);
                        high = high.max(value);
                    }
                }
                high - low > EDGE_RANGE
            })
        };
        let mut frames = Vec::with_capacity(GHOST_FRAMES);
        for frame in LEAD_IN..total {
            let mut measured = GhostFrame {
                wake: 0,
                ghost: 0,
                changed: 0,
                antialiasing: 0,
            };
            let first = frame.saturating_sub(WAKE_FRAMES);
            for y in 0..height {
                for x in 0..width {
                    let offset = (y * width + x) * 4;
                    let changed = differs(&history[frame], &reference[frame], offset);
                    measured.changed += u32::from(changed);
                    let point = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let current = silhouettes[frame]
                        .iter()
                        .any(|hull| inside_distance(hull, point) > -EDGE_MARGIN_PX);
                    let wake = !current
                        && silhouettes[first..frame]
                            .iter()
                            .flatten()
                            .any(|hull| inside_distance(hull, point) > 0.0);
                    if !wake {
                        continue;
                    }
                    if differs(&still_history[frame], &still_reference[frame], offset)
                        || edge(&still_reference[frame], x, y)
                    {
                        measured.antialiasing += 1;
                        continue;
                    }
                    measured.wake += 1;
                    measured.ghost += u32::from(changed);
                }
            }
            frames.push(measured);
        }
        Ok(GhostProbeReport {
            output: [output.width, output.height],
            render: [render.width, render.height],
            frames,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hulls_contain_their_centre_and_not_far_points() {
        let square = convex_hull(vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(4.0, 0.0),
            Vec2::new(4.0, 4.0),
            Vec2::new(0.0, 4.0),
            Vec2::new(2.0, 2.0),
        ]);
        assert_eq!(square.len(), 4);
        assert!((inside_distance(&square, Vec2::new(2.0, 2.0)) - 2.0).abs() < 1e-6);
        assert!(inside_distance(&square, Vec2::new(7.0, 2.0)) < -2.0);
        assert!(inside_distance(&[], Vec2::ZERO) < 0.0);
    }

    #[test]
    fn the_dash_is_eleven_pixels_a_frame_class() {
        // 10 m in 0.2 s at 120 fps (ADR-0021 §3's example).
        assert!((DASH_STEP - 0.416_666_7).abs() < 1e-5);
        let frame = GhostFrame {
            wake: 200,
            ghost: 3,
            changed: 9,
            antialiasing: 4,
        };
        assert!((frame.share() - 0.015).abs() < 1e-6);
    }
}
