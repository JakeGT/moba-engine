//! The ghost-energy probe (ADR-0021 verification gate 3): how much of a fast unit's wake
//! still shows it, frame by frame.
//!
//! A hero-sized box dashes at 10 m per 0.2 s at 120 fps (0.42 m per frame), a small box
//! projectile and a particle projectile (ADR-0021 Q3) cross the screen at 0.7 m per frame,
//! all writing their own motion, over a static camera. The hero is three slabs, dark, light,
//! dark, over mid-tone tiles, so its neighbourhood spans the backdrop's colours and TAAU's
//! clip cannot hide a wrong history fetch on it. The sequence is rendered twice from the same
//! jitter phase: once with history, and once with every frame a history reset (the no-history
//! reference, the same frame with nothing accumulated). On each of the last [`GHOST_FRAMES`]
//! frames the probe measures three shares of pixels that differ from the reference by more
//! than [`GHOST_THRESHOLD`] (4/255) in a channel:
//!
//! - **wake**: every output pixel a moving object covered on one of the previous
//!   [`WAKE_FRAMES`] frames and more than [`EDGE_MARGIN_PX`] from where it is now. Wake
//!   pixels where antialiasing, not ghosting, moves the image are left out and counted:
//!   edges of the still scene (a 3x3 neighbourhood spanning more than [`EDGE_RANGE`]) and
//!   pixels where the same sequence without the movers already differs from its own
//!   no-history render. Bound [`GHOST_BOUND`].
//! - **hero smear**: the hero's interior, off its silhouette edge and its stripes' edges. A
//!   missing or wrong motion vector fetches another stripe or the tiles. Bound
//!   [`SMEAR_BOUND`].
//! - **particle smear**: the particle's bright core. Bound [`PARTICLE_BOUND`].
//!
//! The bounds are regression tripwires above TAAU's measurements, checked against mutations
//! (motion removed), not perceptual thresholds. **The probe must be run on a real GPU as well
//! as lavapipe** (ADR-0021 amendment 4): N1a's particle motion relied on a blend that lavapipe
//! did not apply as the spec says, so lavapipe passed what the RTX 5070 Ti and RADV failed.
//! CI's lavapipe run is a tripwire; the hardware numbers are in perf/0033.
//!
//! Diagnostic only, like the motion probe: run it on a fresh renderer that then exits (the
//! client's `MOBA_GHOST_PROBE=1`), with fixed exposure (`MOBA_EXPOSURE=fixed`).

use core::fmt;

use moba_core::math::{Mat4, Vec2, Vec3, Vec4};

use super::Renderer;
use crate::error::RenderResult;
use crate::scene::{Camera, Scene, UnitInstance};
use crate::temporal::{Extent, clip_to_pixels};
use crate::vfx::{ParticleBlend, ParticleSpawn, VfxFrame};

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
/// The largest ghosting share of the wake allowed on any measured frame (ADR-0021 amendment
/// 2). A regression tripwire, not a perceptual threshold: set about 25 % above TAAU's
/// measurement on lavapipe when it was chosen (2.39 % at Quality, 1.20 % at native, solid
/// hero); with the striped hero and the projectiles it measures 0.57 % native, 1.14 %
/// Quality, 1.33 % Balanced, 1.98 % Performance. The residue is the trailing edge of the
/// hero's moving shadow (shadows have no motion of their own, and a still pixel's clip box
/// is the wider γ 1.25) and faint grid lines re-converging after the disocclusion. The wake
/// alone does not see a missing motion vector (the clip rescues it): the on-object measures
/// below do.
pub(crate) const GHOST_BOUND: f32 = 0.03;
/// The largest smeared share of the hero's interior on any measured frame: a tripwire about
/// 25 % above TAAU's worst on lavapipe (0.00 % native, 0.73 % Quality, 1.02 % Balanced,
/// 1.37 % Performance). With the hero's motion keys removed (no motion vectors) it measures
/// 2.59 % Quality and 7.30 % Performance, which fail; native barely smears either way
/// (0.14 %), so native does not discriminate.
pub(crate) const SMEAR_BOUND: f32 = 0.0175;
/// The largest smeared share of the particle's core on any measured frame: a tripwire above
/// TAAU's worst on lavapipe, the RTX 5070 Ti and RADV (1.40 % native, 3.77 % Quality, 5.70 %
/// Balanced, 6.56 % Performance; ADR-0021 amendment 4). With the particles' motion lost it
/// measures 29.96–32.23 % at Quality and 18.36–20.72 % native on the same three, which fail.
pub(crate) const PARTICLE_BOUND: f32 = 0.075;
/// FSR 3.1's particle tripwire at Performance and below (ADR-0021 amendment 4): about 25 %
/// above its worst Performance measurement on the same three devices (12.33 %, RADV). FSR 3.1
/// smears the particle's core more than TAAU as the ratio grows (Quality 6.63–6.82 %,
/// Balanced 6.96–7.35 %, Performance 11.51–12.33 %, Ultra Performance 21.78–22.34 %), so
/// Quality, Balanced and native keep [`PARTICLE_BOUND`]. Why is not known: the review of
/// amendment 4 found the smear unchanged with FSR's motion-vector dilation reduced to the
/// centre pixel, and the reactive mask is wired and used (zeroing it smears more). This is
/// why FSR 3.1 does not take `auto`'s slot (Q1). Losing the particles' motion fails it
/// (54.41–57.97 % at Performance).
pub(crate) const FSR3_PARTICLE_BOUND: f32 = 0.15;

/// The particle bound for the back end and mode the probe ran: TAAU's tripwire everywhere,
/// FSR 3.1's own from Performance (ratio 2) down.
#[must_use]
pub(crate) fn particle_bound(kind: crate::post::UpscalerKind, ratio: f32) -> f32 {
    if kind == crate::post::UpscalerKind::Fsr3 && ratio >= 2.0 {
        FSR3_PARTICLE_BOUND
    } else {
        PARTICLE_BOUND
    }
}

/// A particle core pixel: within its disc, and its no-history render differs from the same
/// frame without the movers by more than this in a channel (the glow's bright centre; a soft
/// sprite's rim is mostly backdrop).
const PARTICLE_CORE: u8 = 40;

/// The hero's dash per frame: 10 m in 0.2 s at 120 fps (0.42 m).
const DASH_STEP: f32 = 10.0 / 0.2 / 120.0;
/// The projectiles' step per frame (84 m/s: across the view, and in it, for all frames).
const PROJECTILE_STEP: f32 = 0.7;

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
    /// The hero's interior pixels off its stripe edges (the on-object measure).
    pub object: u32,
    /// Of those, pixels that differ from the no-history reference by more than 4/255.
    pub smear: u32,
    /// The particle projectile's core pixels.
    pub particle: u32,
    /// Of those, pixels that differ from the no-history reference by more than 4/255.
    pub particle_smear: u32,
}

impl GhostFrame {
    /// The smeared share of the hero's interior.
    #[must_use]
    pub fn smear_share(&self) -> f32 {
        share(self.smear, self.object)
    }

    /// The smeared share of the particle's core.
    #[must_use]
    pub fn particle_share(&self) -> f32 {
        share(self.particle_smear, self.particle)
    }

    /// The ghosting share of the wake.
    #[must_use]
    pub fn share(&self) -> f32 {
        share(self.ghost, self.wake)
    }
}

fn share(part: u32, whole: u32) -> f32 {
    if whole == 0 {
        0.0
    } else {
        part as f32 / whole as f32
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
    /// The back end that ran (`MOBA_UPSCALER`'s name).
    pub upscaler: &'static str,
    /// The particle core's bound for that back end and mode (TAAU's 7.5 % tripwire unless
    /// FSR 3.1 ran at Performance or below, where it is 15 %: ADR-0021 amendment 4).
    pub particle_bound: f32,
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

    /// The largest smeared share of the hero's interior.
    #[must_use]
    pub fn worst_smear(&self) -> f32 {
        self.frames
            .iter()
            .map(GhostFrame::smear_share)
            .fold(0.0, f32::max)
    }

    /// The largest smeared share of the particle's core.
    #[must_use]
    pub fn worst_particle(&self) -> f32 {
        self.frames
            .iter()
            .map(GhostFrame::particle_share)
            .fold(0.0, f32::max)
    }

    /// Every measured frame has a wake, a hero interior and a particle core, and stays within
    /// `GHOST_BOUND`, `SMEAR_BOUND` and the report's particle bound.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.frames.len() == GHOST_FRAMES
            && self.frames.iter().all(|frame| {
                frame.wake > 0
                    && frame.object > 0
                    && frame.particle > 0
                    && frame.share() <= GHOST_BOUND
                    && frame.smear_share() <= SMEAR_BOUND
                    && frame.particle_share() <= self.particle_bound
            })
    }
}

impl fmt::Display for GhostProbeReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "ghost probe: upscaler {}, output {}x{}, render {}x{}, wake {} frames, edge {} px, \
             threshold {}/255, bound {:.1} %",
            self.upscaler,
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
                 out, {} changed in the frame); hero {}/{} = {:.2} %, particle {}/{} = {:.2} %",
                index + LEAD_IN,
                frame.wake,
                frame.ghost,
                frame.share() * 100.0,
                frame.antialiasing,
                frame.changed,
                frame.smear,
                frame.object,
                frame.smear_share() * 100.0,
                frame.particle_smear,
                frame.particle,
                frame.particle_share() * 100.0
            )?;
        }
        write!(
            f,
            "ghost probe: worst wake {:.3} % (bound {:.1} %), hero smear {:.2} % (bound {:.2} %), \
             particle smear {:.2} % (bound {:.1} %) ({})",
            self.worst() * 100.0,
            GHOST_BOUND * 100.0,
            self.worst_smear() * 100.0,
            SMEAR_BOUND * 100.0,
            self.worst_particle() * 100.0,
            self.particle_bound * 100.0,
            if self.passed() { "passed" } else { "FAILED" }
        )
    }
}

/// The hero's box at frame `frame` (its three slabs together).
fn hero_box(frame: usize) -> Mat4 {
    Mat4::from_scale_rotation_translation(
        Vec3::new(0.9, 1.8, 0.9),
        moba_core::math::Quat::IDENTITY,
        Vec3::new(-5.0 + DASH_STEP * frame as f32, 0.9, 0.5),
    )
}

/// The moving objects at frame `frame`: (transform, colour, motion key). The hero is three
/// 0.3 m slabs across its direction of travel, dark, light, dark: its neighbourhood spans the
/// backdrop's colours, so TAAU's clip cannot hide a wrong history fetch on it (a missing or
/// wrong motion vector smears the backdrop or the next slab into it).
fn movers(frame: usize) -> [(Mat4, Vec4, u32); 4] {
    let step = frame as f32;
    let slab = |index: usize| {
        Mat4::from_scale_rotation_translation(
            Vec3::new(0.3, 1.8, 0.9),
            moba_core::math::Quat::IDENTITY,
            Vec3::new(-5.0 + DASH_STEP * step - 0.3 + 0.3 * index as f32, 0.9, 0.5),
        )
    };
    let dark = Vec4::new(0.05, 0.04, 0.04, 1.0);
    let light = Vec4::new(0.95, 0.85, 0.8, 1.0);
    let projectile = Mat4::from_scale_rotation_translation(
        Vec3::splat(0.35),
        moba_core::math::Quat::IDENTITY,
        Vec3::new(9.0 - PROJECTILE_STEP * step, 1.0, -3.0),
    );
    [
        (slab(0), dark, 1),
        (slab(1), light, 2),
        (slab(2), dark, 3),
        (projectile, Vec4::new(1.0, 0.9, 0.2, 1.0), 4),
    ]
}

/// Where the particle projectile spawns on frame 0.
const PARTICLE_SPAWN: Vec3 = Vec3::new(8.5, 1.5, 3.5);

/// The particle projectile's centre as drawn on frame `frame`, flying [`PROJECTILE_STEP`]
/// metres a frame (ADR-0021 Q3: its fragments from 0.1 coverage write their own motion).
/// The simulation integrates a spawn once before its first draw, hence `frame + 1`.
fn particle_centre(frame: usize) -> Vec3 {
    PARTICLE_SPAWN - Vec3::X * (PROJECTILE_STEP * (frame + 1) as f32)
}

/// The particle's diameter, metres.
const PARTICLE_SIZE: f32 = 0.9;
/// Seconds per probe frame (120 fps).
const FRAME_SECONDS: f32 = 1.0 / 120.0;

/// A disc in pixels (centre, radius) for the particle at `frame`.
fn particle_disc(view_proj: Mat4, frame: usize, output: Extent) -> Option<(Vec2, f32)> {
    let centre = particle_centre(frame);
    let here = clip_to_pixels(view_proj * centre.extend(1.0), output)?;
    // A billboard faces the camera; its screen radius is close to a horizontal offset's.
    let edge = clip_to_pixels(
        view_proj * (centre + Vec3::X * (0.5 * PARTICLE_SIZE)).extend(1.0),
        output,
    )?;
    let here = Vec2::from_array(here);
    Some((here, here.distance(Vec2::from_array(edge))))
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
        let mut backdrop: Vec<UnitInstance> = (0..9)
            .map(|index| {
                let x = -8.0 + 2.0 * index as f32;
                let z = if index % 2 == 0 { -1.5 } else { 2.5 };
                UnitInstance::new(
                    Mat4::from_translation(Vec3::new(x, 0.5, z)),
                    Vec4::new(0.25, 0.3, 0.45, 1.0),
                )
            })
            .collect();
        // Mid-tone tiles under the hero's path: inside the striped hero's colour range, so a
        // history fetch that lands on them survives the clip.
        backdrop.extend((0..28).map(|index| {
            let shade = if index % 2 == 0 { 0.3 } else { 0.6 };
            UnitInstance::new(
                Mat4::from_scale_rotation_translation(
                    Vec3::new(0.5, 0.04, 1.6),
                    moba_core::math::Quat::IDENTITY,
                    Vec3::new(-6.75 + 0.5 * index as f32, 0.02, 0.5),
                ),
                Vec4::new(shade, shade * 0.95, shade * 0.9, 1.0),
            )
        }));
        let particle = [ParticleSpawn::new(
            PARTICLE_SPAWN,
            (total as f32 + 1.0) * FRAME_SECONDS,
            PARTICLE_SIZE,
            Vec4::new(0.2, 0.9, 1.0, 1.0),
        )
        .with_color_end(Vec4::new(0.2, 0.9, 1.0, 1.0))
        .with_velocity(Vec3::new(-PROJECTILE_STEP / FRAME_SECONDS, 0.0, 0.0))
        .with_blend(ParticleBlend::Alpha)];
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
                let spawns: &[ParticleSpawn] = if with_movers && frame == 0 {
                    &particle
                } else {
                    &[]
                };
                let scene = Scene::new(camera, &units)
                    .with_history_reset(reference)
                    .with_vfx(VfxFrame::new(spawns, FRAME_SECONDS));
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
        let heroes: Vec<Vec<Vec2>> = (0..total)
            .map(|frame| silhouette(camera.view_proj, hero_box(frame), output))
            .collect();
        let discs: Vec<Option<(Vec2, f32)>> = (0..total)
            .map(|frame| particle_disc(camera.view_proj, frame, output))
            .collect();
        let in_disc = |disc: Option<(Vec2, f32)>, point: Vec2, scale: f32, margin: f32| {
            disc.is_some_and(|(centre, radius)| point.distance(centre) < radius * scale + margin)
        };
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
                object: 0,
                smear: 0,
                particle: 0,
                particle_smear: 0,
            };
            let first = frame.saturating_sub(WAKE_FRAMES);
            for y in 0..height {
                for x in 0..width {
                    let offset = (y * width + x) * 4;
                    let changed = differs(&history[frame], &reference[frame], offset);
                    measured.changed += u32::from(changed);
                    let point = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    // On the object: the hero's interior off its own stripe edges, and the
                    // particle's core. Wrong or missing motion fetches another stripe or the
                    // tiles there (ADR-0021 gate 3's on-object smear).
                    if inside_distance(&heroes[frame], point) > EDGE_MARGIN_PX
                        && !edge(&reference[frame], x, y)
                    {
                        measured.object += 1;
                        measured.smear += u32::from(changed);
                        continue;
                    }
                    if in_disc(discs[frame], point, 1.0, 0.0)
                        && reference[frame][offset..offset + 3]
                            .iter()
                            .zip(&still_reference[frame][offset..offset + 3])
                            .any(|(a, b)| a.abs_diff(*b) > PARTICLE_CORE)
                    {
                        measured.particle += 1;
                        measured.particle_smear += u32::from(changed);
                        continue;
                    }
                    let current = silhouettes[frame]
                        .iter()
                        .any(|hull| inside_distance(hull, point) > -EDGE_MARGIN_PX)
                        || in_disc(discs[frame], point, 1.0, EDGE_MARGIN_PX);
                    let wake = !current
                        && (silhouettes[first..frame]
                            .iter()
                            .flatten()
                            .any(|hull| inside_distance(hull, point) > 0.0)
                            || discs[first..frame]
                                .iter()
                                .any(|&disc| in_disc(disc, point, 1.0, 0.0)));
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
        let (upscaler, ratio) = self
            .temporal
            .as_ref()
            .map_or((crate::post::UpscalerKind::Taau, 1.0), |temporal| {
                (temporal.upscaler.kind(), temporal.mode.ratio())
            });
        Ok(GhostProbeReport {
            output: [output.width, output.height],
            render: [render.width, render.height],
            frames,
            upscaler: upscaler.name(),
            particle_bound: particle_bound(upscaler, ratio),
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

    /// TAAU keeps its tripwire in every mode; FSR 3.1 only from Performance down (amendment 4).
    #[test]
    fn particle_bounds_by_back_end() {
        use crate::post::{UpscaleMode, UpscalerKind};
        for &mode in UpscaleMode::ALL {
            assert_eq!(
                particle_bound(UpscalerKind::Taau, mode.ratio()),
                PARTICLE_BOUND
            );
        }
        for (mode, bound) in [
            (UpscaleMode::Native, PARTICLE_BOUND),
            (UpscaleMode::Quality, PARTICLE_BOUND),
            (UpscaleMode::Balanced, PARTICLE_BOUND),
            (UpscaleMode::Performance, FSR3_PARTICLE_BOUND),
            (UpscaleMode::UltraPerformance, FSR3_PARTICLE_BOUND),
        ] {
            assert_eq!(particle_bound(UpscalerKind::Fsr3, mode.ratio()), bound);
        }
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
            object: 50,
            smear: 5,
            particle: 0,
            particle_smear: 0,
        };
        assert!((frame.share() - 0.015).abs() < 1e-6);
        assert!((frame.smear_share() - 0.1).abs() < 1e-6);
        assert_eq!(frame.particle_share(), 0.0);
    }
}
