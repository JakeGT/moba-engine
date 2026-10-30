//! Decals, foliage and water (R3b, ADR-0017).
//!
//! - [`EnvironmentConfig`] holds the renderer-lifetime, tiered features
//!   ([`EnvironmentConfig::for_tier`]) and their off switches.
//! - [`Decal`] is one projected ground decal of this frame ([`Scene::decals`](crate::Scene::decals)):
//!   a telegraph, a zone, a scorch mark or a splat. The renderer binds up to [`MAX_DECALS`]
//!   into a storage buffer with a 32 × 32 grid of per-cell masks, and the terrain, water and
//!   foliage shaders project them; enemy telegraphs are kept first and get guaranteed contrast.
//! - [`FoliageData`] and [`WaterData`] are a map's foliage instances and water mesh, uploaded
//!   once per map ([`Renderer::set_foliage`](crate::Renderer::set_foliage),
//!   [`Renderer::set_water`](crate::Renderer::set_water)); [`EnvironmentFrame`] carries the
//!   frame's time and the foliage displacers.
//!
//! Everything here is client-local and cosmetic: decals come only from effects the client
//! plays, displacers only from units it draws, and nothing reaches the simulation or the wire.

use bytemuck::{Pod, Zeroable};
use moba_core::math::{Mat4, Vec2, Vec3, Vec4};

use crate::quality::QualityTier;

/// Most decals bound per frame (enemy telegraphs first; the rest by the tier's cap).
pub const MAX_DECALS: usize = 128;
/// Cells per edge of the decal binning grid.
pub(crate) const DECAL_GRID: usize = 32;
/// Most foliage displacers per frame ([`EnvironmentFrame::displacers`]).
pub const MAX_FOLIAGE_DISPLACERS: usize = 32;
/// Most foliage instances one map may upload ([`Renderer::set_foliage`](crate::Renderer::set_foliage)).
pub const MAX_FOLIAGE_INSTANCES: usize = 65_536;
/// Most water bodies one map may upload (each [`WaterVertexData::body`] indexes
/// [`WaterData::styles`]).
pub const MAX_WATER_BODIES: usize = 8;
/// Most water vertices one map may upload (`moba_assets::MAX_WATER_MESH_VERTICES`, which
/// the map consistency check holds every terrain grid to).
pub const MAX_WATER_VERTICES: usize = moba_assets::MAX_WATER_MESH_VERTICES;
/// Edge of a foliage culling chunk, metres.
pub(crate) const FOLIAGE_CHUNK: f32 = 16.0;
/// Vertices per foliage blade (two segments: three triangles).
pub(crate) const FOLIAGE_BLADE_VERTICES: u32 = 9;
/// Most blades per foliage clump.
pub(crate) const MAX_FOLIAGE_BLADES: u32 = 8;

/// How water is drawn (ADR-0017 §3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WaterMode {
    /// No water surface (the carved bed still shows).
    Off,
    /// Alpha-blended over the bed at the end of the scene pass (Low, Medium).
    Simple,
    /// Plus screen-space refraction and contact foam in a pass after the scene (High, Ultra;
    /// [`Simple`](Self::Simple) on portability-subset devices).
    #[default]
    Full,
}

impl WaterMode {
    /// Every mode.
    pub const ALL: &'static [Self] = &[Self::Off, Self::Simple, Self::Full];

    /// Lower-case name, as accepted by [`Self::from_name`].
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Simple => "simple",
            Self::Full => "full",
        }
    }

    /// Parses a mode name, ignoring ASCII case and surrounding whitespace.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        Self::ALL
            .iter()
            .copied()
            .find(|mode| mode.name().eq_ignore_ascii_case(name))
    }
}

/// Renderer-lifetime decal, foliage and water features ([`RenderConfig::environment`](crate::RenderConfig::environment)).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct EnvironmentConfig {
    /// Projected decals on (`MOBA_DECALS=off` turns them off).
    pub decals: bool,
    /// Most decals bound per frame besides enemy telegraphs (which may use all
    /// [`MAX_DECALS`]); clamped to `0..=MAX_DECALS`.
    pub max_decals: u32,
    /// Foliage drawn (`MOBA_FOLIAGE=off`).
    pub foliage: bool,
    /// Multiplier on each map's foliage density, `0..=1` (the client generates with it).
    pub foliage_density: f32,
    /// Most foliage instances uploaded; clamped to [`MAX_FOLIAGE_INSTANCES`].
    pub max_foliage: u32,
    /// Blades per clump at close range, `1..=8`.
    pub foliage_blades: u32,
    /// Foliage draw distance, metres, from the eye or the camera's focus on the ground,
    /// whichever is nearer.
    pub foliage_distance: f32,
    /// Most foliage displacers used per frame, `0..=`[`MAX_FOLIAGE_DISPLACERS`].
    pub max_displacers: u32,
    /// Water mode (`MOBA_WATER=off|simple|full`).
    pub water: WaterMode,
    /// Water normal octaves, `1..=2`.
    pub water_octaves: u32,
}

impl EnvironmentConfig {
    /// The defaults of `tier` (ADR-0017).
    #[must_use]
    pub const fn for_tier(tier: QualityTier) -> Self {
        let (max_decals, density, max_foliage, blades, distance, displacers) = match tier {
            QualityTier::Low => (48, 0.3, 8_192, 3, 60.0, 32),
            QualityTier::Medium => (96, 0.6, 24_576, 4, 90.0, 32),
            QualityTier::High => (128, 1.0, 49_152, 5, 130.0, 32),
            QualityTier::Ultra => (128, 1.0, 65_536, 6, 180.0, 32),
        };
        let (water, water_octaves) = match tier {
            QualityTier::Low => (WaterMode::Simple, 1),
            QualityTier::Medium => (WaterMode::Simple, 2),
            QualityTier::High | QualityTier::Ultra => (WaterMode::Full, 2),
        };
        Self {
            decals: true,
            max_decals,
            foliage: true,
            foliage_density: density,
            max_foliage,
            foliage_blades: blades,
            foliage_distance: distance,
            max_displacers: displacers,
            water,
            water_octaves,
        }
    }

    /// Turns decals on or off.
    #[must_use]
    pub const fn with_decals(mut self, decals: bool) -> Self {
        self.decals = decals;
        self
    }

    /// Turns foliage on or off.
    #[must_use]
    pub const fn with_foliage(mut self, foliage: bool) -> Self {
        self.foliage = foliage;
        self
    }

    /// Sets the water mode.
    #[must_use]
    pub const fn with_water(mut self, water: WaterMode) -> Self {
        self.water = water;
        self
    }

    /// Sets the decal cap.
    #[must_use]
    pub const fn with_max_decals(mut self, max_decals: u32) -> Self {
        self.max_decals = max_decals;
        self
    }

    /// The config with every field in range.
    pub(crate) fn sanitized(self) -> Self {
        let finite = |value: f32, low: f32, high: f32| {
            if value.is_finite() {
                value.clamp(low, high)
            } else {
                low
            }
        };
        Self {
            max_decals: self.max_decals.min(MAX_DECALS as u32),
            foliage_density: finite(self.foliage_density, 0.0, 1.0),
            max_foliage: self.max_foliage.min(MAX_FOLIAGE_INSTANCES as u32),
            foliage_blades: self.foliage_blades.clamp(1, MAX_FOLIAGE_BLADES),
            foliage_distance: finite(self.foliage_distance, 10.0, 1_000.0),
            max_displacers: self.max_displacers.min(MAX_FOLIAGE_DISPLACERS as u32),
            water_octaves: self.water_octaves.clamp(1, 2),
            ..self
        }
    }
}

impl Default for EnvironmentConfig {
    fn default() -> Self {
        Self::for_tier(QualityTier::default())
    }
}

/// A decal's footprint (ADR-0017 §1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DecalShape {
    /// A filled circle of [`Decal::radius`] with a rim.
    #[default]
    Disc,
    /// A band of [`Decal::width`] centred on [`Decal::radius`].
    Ring,
    /// A circular sector of [`Decal::radius`] and full angle [`Decal::angle`], pointing along
    /// [`Decal::yaw`].
    Cone,
    /// A rectangle from the position along [`Decal::yaw`], [`Decal::radius`] long and
    /// [`Decal::width`] wide.
    Line,
    /// An irregular blot of about [`Decal::radius`], its outline from [`Decal::seed`].
    Splat,
}

impl DecalShape {
    const fn index(self) -> f32 {
        match self {
            Self::Disc => 0.0,
            Self::Ring => 1.0,
            Self::Cone => 2.0,
            Self::Line => 3.0,
            Self::Splat => 4.0,
        }
    }
}

/// How a decal blends (ADR-0017 §1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DecalStyle {
    /// Over the shaded, fogged surface: telegraphs and zones (never dimmed by lighting).
    #[default]
    Glow,
    /// Into the surface's albedo before lighting: scorch marks, blood, goo.
    Paint,
}

/// Whose decal it is, for culling and contrast (ADR-0013's telegraph rules).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DecalRole {
    /// Cosmetic: dropped first.
    #[default]
    Cosmetic,
    /// An ally's telegraph or zone.
    Ally,
    /// An enemy telegraph: never dropped before any other decal, its opacity floored at
    /// [`VfxBudget::telegraph_floor`](crate::VfxBudget::telegraph_floor) and drawn with a
    /// contrast rim.
    EnemyTelegraph,
}

/// One projected ground decal of this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Decal {
    /// Centre (the line's and the cone's origin), world space. Receivers within 4 m of its
    /// height take it.
    pub position: Vec3,
    /// Direction of cones and lines, radians about +y from +x towards +z.
    pub yaw: f32,
    /// Footprint.
    pub shape: DecalShape,
    /// Radius in metres (a line's length).
    pub radius: f32,
    /// Ring band and line width, metres.
    pub width: f32,
    /// Cone full angle, radians.
    pub angle: f32,
    /// Linear HDR colour; `w` is the opacity, `0..=1`.
    pub color: Vec4,
    /// Blend.
    pub style: DecalStyle,
    /// Role.
    pub role: DecalRole,
    /// A cast timer's fill `0..=1` (from the centre to the edge), or `None`.
    pub progress: Option<f32>,
    /// Fade multiplier `0..=1` (fade in and out); applied after the telegraph floor.
    pub fade: f32,
    /// Outline seed of [`DecalShape::Splat`].
    pub seed: u32,
}

impl Decal {
    /// A glow disc at `position`.
    #[must_use]
    pub const fn new(position: Vec3, shape: DecalShape, radius: f32, color: Vec4) -> Self {
        Self {
            position,
            yaw: 0.0,
            shape,
            radius,
            width: 0.5,
            angle: core::f32::consts::FRAC_PI_2,
            color,
            style: DecalStyle::Glow,
            role: DecalRole::Cosmetic,
            progress: None,
            fade: 1.0,
            seed: 0,
        }
    }

    /// Sets the yaw.
    #[must_use]
    pub const fn with_yaw(mut self, yaw: f32) -> Self {
        self.yaw = yaw;
        self
    }

    /// Sets the band width.
    #[must_use]
    pub const fn with_width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Sets the cone angle.
    #[must_use]
    pub const fn with_angle(mut self, angle: f32) -> Self {
        self.angle = angle;
        self
    }

    /// Sets the style.
    #[must_use]
    pub const fn with_style(mut self, style: DecalStyle) -> Self {
        self.style = style;
        self
    }

    /// Sets the role.
    #[must_use]
    pub const fn with_role(mut self, role: DecalRole) -> Self {
        self.role = role;
        self
    }

    /// Sets the progress fill.
    #[must_use]
    pub const fn with_progress(mut self, progress: Option<f32>) -> Self {
        self.progress = progress;
        self
    }

    /// Sets the fade.
    #[must_use]
    pub const fn with_fade(mut self, fade: f32) -> Self {
        self.fade = fade;
        self
    }

    /// Sets the splat seed.
    #[must_use]
    pub const fn with_seed(mut self, seed: u32) -> Self {
        self.seed = seed;
        self
    }

    /// Whether every field is finite and the decal covers some ground.
    fn usable(&self) -> bool {
        self.position.is_finite()
            && self.yaw.is_finite()
            && self.radius.is_finite()
            && self.radius > 0.0
            && self.width.is_finite()
            && self.angle.is_finite()
            && self.color.is_finite()
            && self.fade.is_finite()
            && self.fade > 0.0
            && self.color.w > 0.0
            && self.progress.is_none_or(f32::is_finite)
    }

    /// Half the side of the square around `position` that holds the footprint.
    fn reach(&self) -> f32 {
        let width = self.width.clamp(0.0, 64.0);
        let radius = self.radius.min(256.0);
        match self.shape {
            DecalShape::Ring | DecalShape::Line => radius + width * 0.5,
            _ => radius,
        }
    }
}

/// Glow-decal flag bits (`params.y`), shared with `decals.glsl`.
const FLAG_GLOW: u32 = 1;
const FLAG_ENEMY: u32 = 2;
/// Height band within which a receiver takes a decal (`MOBA_DECAL_REACH` in `decals.glsl`).
const DECAL_REACH: f32 = 4.0;
const FLAG_PROGRESS: u32 = 4;

/// One decal on the GPU (`DecalRecord` in `decals.glsl`, std430, 64 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub(crate) struct GpuDecal {
    /// x, z of the centre; cos and sin of the yaw.
    placement: [f32; 4],
    /// Radius (length), width, half the cone angle, height.
    size: [f32; 4],
    /// Linear colour and opacity (the enemy floor already applied, before `fade`).
    color: [f32; 4],
    /// Shape, flag bits (as a float), progress, fade.
    params: [f32; 4],
}

/// The decal buffer's header (`decals.glsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct DecalHeader {
    /// xz of the grid's corner, cells per metre (x, z).
    rect: [f32; 4],
    /// Decals, grid edge, enemy telegraphs, splat seed salt.
    counts: [u32; 4],
}

/// Everything the decal storage buffer holds, built on the CPU each frame without
/// allocation (boxed once at creation).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct DecalUpload {
    header: DecalHeader,
    decals: [GpuDecal; MAX_DECALS],
    /// Per grid cell, a 128-bit mask of the decals overlapping it.
    cells: [[u32; 4]; DECAL_GRID * DECAL_GRID],
}

/// Bytes of the decal storage buffer.
pub(crate) const DECAL_BUFFER_SIZE: u64 = core::mem::size_of::<DecalUpload>() as u64;

impl DecalUpload {
    /// An empty upload (no decals).
    pub(crate) fn boxed() -> Box<Self> {
        Box::new(Zeroable::zeroed())
    }

    /// Bytes to copy for the current contents: the header, the records in use and the grid
    /// (always whole, so stale masks never survive).
    pub(crate) fn bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }

    /// Fills the upload from `decals`: enemy telegraphs first (all of them, up to
    /// [`MAX_DECALS`]), then the others in order up to `cap` in total, then bins them into
    /// the grid. Opacity of enemy telegraphs is floored at `floor`. With `view`, the xz
    /// bounds `[min x, min z, max x, max z]` of the ground the main camera shows, decals whose
    /// footprint lies outside it are skipped: they cannot be seen, and an off-screen one (a
    /// distant or global telegraph) would otherwise stretch the grid and coarsen every cell.
    /// Returns how many usable, visible decals did not fit.
    pub(crate) fn build(
        &mut self,
        decals: &[Decal],
        cap: usize,
        floor: f32,
        view: Option<[f32; 4]>,
    ) -> u32 {
        let floor = if floor.is_finite() {
            floor.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let cap = cap.min(MAX_DECALS);
        let mut count = 0usize;
        let mut enemies = 0u32;
        let mut dropped = 0u32;
        let mut min = [f32::INFINITY; 2];
        let mut max = [f32::NEG_INFINITY; 2];
        for pass in [true, false] {
            for decal in decals {
                let enemy = decal.role == DecalRole::EnemyTelegraph;
                if enemy != pass || !decal.usable() {
                    continue;
                }
                let reach = decal.reach();
                if let Some([x0, z0, x1, z1]) = view
                    && (decal.position.x + reach < x0
                        || decal.position.x - reach > x1
                        || decal.position.z + reach < z0
                        || decal.position.z - reach > z1)
                {
                    continue;
                }
                let limit = if enemy { MAX_DECALS } else { cap };
                if count >= limit {
                    dropped += 1;
                    continue;
                }
                min[0] = min[0].min(decal.position.x - reach);
                min[1] = min[1].min(decal.position.z - reach);
                max[0] = max[0].max(decal.position.x + reach);
                max[1] = max[1].max(decal.position.z + reach);
                self.decals[count] = gpu_decal(decal, floor);
                count += 1;
                enemies += u32::from(enemy);
            }
        }
        self.cells = [[0; 4]; DECAL_GRID * DECAL_GRID];
        if count == 0 {
            self.header = DecalHeader::default();
            return dropped;
        }
        // At least 1 m per cell edge, so a tiny cluster still gets a sensible grid.
        let span = [
            (max[0] - min[0]).max(DECAL_GRID as f32),
            (max[1] - min[1]).max(DECAL_GRID as f32),
        ];
        let per_metre = [DECAL_GRID as f32 / span[0], DECAL_GRID as f32 / span[1]];
        self.header = DecalHeader {
            rect: [min[0], min[1], per_metre[0], per_metre[1]],
            counts: [count as u32, DECAL_GRID as u32, enemies, 0],
        };
        let cell = |value: f32, origin: f32, scale: f32| -> usize {
            let index = ((value - origin) * scale).floor();
            if index <= 0.0 {
                0
            } else {
                (index as usize).min(DECAL_GRID - 1)
            }
        };
        for (index, decal) in self.decals[..count].iter().enumerate() {
            let reach = decal_reach(decal);
            let (x, z) = (decal.placement[0], decal.placement[1]);
            let x0 = cell(x - reach, min[0], per_metre[0]);
            let x1 = cell(x + reach, min[0], per_metre[0]);
            let z0 = cell(z - reach, min[1], per_metre[1]);
            let z1 = cell(z + reach, min[1], per_metre[1]);
            let (word, bit) = (index / 32, 1u32 << (index % 32));
            for row in z0..=z1 {
                for column in x0..=x1 {
                    self.cells[row * DECAL_GRID + column][word] |= bit;
                }
            }
        }
        dropped
    }

    /// Decals in the upload.
    #[cfg(test)]
    fn count(&self) -> usize {
        self.header.counts[0] as usize
    }
}

fn gpu_decal(decal: &Decal, floor: f32) -> GpuDecal {
    let enemy = decal.role == DecalRole::EnemyTelegraph;
    let mut flags = 0;
    if decal.style == DecalStyle::Glow {
        flags |= FLAG_GLOW;
    }
    if enemy {
        flags |= FLAG_ENEMY;
    }
    if decal.progress.is_some() {
        flags |= FLAG_PROGRESS;
    }
    let mut alpha = decal.color.w.clamp(0.0, 1.0);
    if enemy {
        alpha = alpha.max(floor);
    }
    let rgb = decal.color.truncate().clamp(Vec3::ZERO, Vec3::splat(64.0));
    let (sin, cos) = decal.yaw.sin_cos();
    let half_angle = (decal.angle * 0.5).clamp(0.01, core::f32::consts::PI);
    let seed = (decal.seed % 1024) as f32;
    GpuDecal {
        placement: [decal.position.x, decal.position.z, cos, sin],
        size: [
            decal.radius.min(256.0),
            decal.width.clamp(0.02, 64.0),
            half_angle,
            decal.position.y,
        ],
        color: [rgb.x, rgb.y, rgb.z, alpha],
        params: [
            decal.shape.index() + seed * 8.0,
            flags as f32,
            decal.progress.unwrap_or(0.0).clamp(0.0, 1.0),
            decal.fade.clamp(0.0, 1.0),
        ],
    }
}

fn decal_reach(decal: &GpuDecal) -> f32 {
    // `params.x` is `shape + 8 * seed`, an exact small integer in f32.
    let shape = (decal.params[0] as u32) % 8;
    if shape == 1 || shape == 3 {
        decal.size[0] + decal.size[1] * 0.5
    } else {
        decal.size[0]
    }
}

/// Relative luminance of a linear colour.
fn luminance(color: Vec3) -> f32 {
    color.dot(Vec3::new(0.2126, 0.7152, 0.0722))
}

/// GLSL `smoothstep`.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `decals.glsl`'s `decal_hash` (GLSL `fract` is `x - floor(x)`).
#[allow(clippy::excessive_precision)] // The shader's constant, written as it is there.
fn decal_hash(n: f32) -> f32 {
    let x = n.sin() * 43758.5453;
    x - x.floor()
}

/// The CPU twin of `decals.glsl`'s `decal_coverage`: x the washed area, y the rim band, z the
/// progress fill, w the halo just inside the rim, for `decal` at `world` with antialiasing
/// width `aa` (metres).
#[allow(clippy::many_single_char_names)] // The shader's names, so the two read side by side.
fn decal_coverage(decal: &GpuDecal, world: Vec3, aa: f32) -> Vec4 {
    if (world.y - decal.size[3]).abs() > DECAL_REACH {
        return Vec4::ZERO;
    }
    let offset = Vec2::new(world.x - decal.placement[0], world.z - decal.placement[1]);
    let axis = Vec2::new(decal.placement[2], decal.placement[3]);
    let p = Vec2::new(offset.dot(axis), offset.dot(Vec2::new(-axis.y, axis.x)));
    let shape = decal.params[0].rem_euclid(8.0);
    let seed = (decal.params[0] / 8.0).floor();
    let radius = decal.size[0];
    let mut rim_width = (radius * 0.08).clamp(0.12, 0.35);
    let (sd, along) = if shape < 0.5 {
        (p.length() - radius, p.length() / radius)
    } else if shape < 1.5 {
        rim_width = decal.size[1];
        (p.length() - (radius + 0.5 * rim_width), p.length() / radius)
    } else if shape < 2.5 {
        let q = Vec2::new(p.y.abs(), p.x);
        let c = Vec2::new(decal.size[2].sin(), decal.size[2].cos());
        let l = q.length() - radius;
        let m = (q - c * q.dot(c).clamp(0.0, radius)).length();
        let side = c.y * q.x - c.x * q.y;
        // GLSL `sign` is 0 at 0.
        let sign = if side > 0.0 {
            1.0
        } else if side < 0.0 {
            -1.0
        } else {
            0.0
        };
        (l.max(m * sign), p.length() / radius)
    } else if shape < 3.5 {
        let half = Vec2::new(radius * 0.5, decal.size[1] * 0.5);
        let b = (p - Vec2::new(half.x, 0.0)).abs() - half;
        rim_width = (decal.size[1] * 0.15).clamp(0.08, 0.3);
        (
            b.max(Vec2::ZERO).length() + b.x.max(b.y).min(0.0),
            (p.x / radius.max(1e-3)).clamp(0.0, 1.0),
        )
    } else {
        let angle = p.y.atan2(p.x);
        let wobble = 0.78
            + 0.12 * (5.0 * angle + seed).sin()
            + 0.07 * (11.0 * angle + seed * 1.7).sin()
            + 0.04 * (23.0 * angle + seed * 3.1).sin();
        let mut sd = p.length() - radius * wobble;
        for i in 0..3 {
            let i = i as f32;
            let a = decal_hash(seed + i * 7.0) * core::f32::consts::TAU;
            let r = radius * (1.05 + 0.25 * decal_hash(seed + i * 13.0));
            let s = radius * (0.08 + 0.08 * decal_hash(seed + i * 19.0));
            sd = sd.min((p - Vec2::new(a.cos(), a.sin()) * r).length() - s);
        }
        (sd, 1.0)
    };
    let fill = 1.0 - smoothstep(-aa, aa, sd);
    if fill <= 0.0 {
        return Vec4::ZERO;
    }
    let rim = fill * (1.0 - smoothstep(rim_width - aa, rim_width + aa, -sd));
    let halo =
        fill * (1.0 - rim) * (1.0 - smoothstep(2.0 * rim_width - aa, 2.0 * rim_width + aa, -sd));
    let mut progress = 0.0;
    if (decal.params[1] as u32) & FLAG_PROGRESS != 0 {
        let sweep = decal.params[2];
        progress = fill * (1.0 - smoothstep(sweep - 0.01, sweep + 0.01, along));
    }
    let wash = if shape > 0.5 && shape < 1.5 {
        rim
    } else {
        fill
    };
    Vec4::new(wash, rim, progress, halo)
}

/// The CPU twin of one step of `decals.glsl`'s `apply_glow_decals`.
fn glow_over(color: Vec3, decal: &GpuDecal, coverage: Vec4) -> Vec3 {
    if coverage.x + coverage.z + coverage.w <= 0.0 {
        return color;
    }
    let alpha = (decal.color[3] * decal.params[3]).clamp(0.0, 1.0);
    let band = (2.0 * alpha).min(1.0);
    let tint = Vec3::new(decal.color[0], decal.color[1], decal.color[2]);
    let mut color = color.lerp(tint, alpha * (0.3 * coverage.x + 0.35 * coverage.z));
    let mut rim = tint;
    if (decal.params[1] as u32) & FLAG_ENEMY != 0 {
        let ground = luminance(color);
        rim *= ((1.8 * ground + 0.05) / luminance(tint).max(1e-3)).max(1.0);
        color = color.lerp(color * 0.2, band * coverage.w);
    }
    color.lerp(rim, band * coverage.y)
}

/// The CPU twin of `decals.glsl` for one glow decal (ADR-0017 §1): the colour `decal` leaves
/// over a shaded ground of `background` at `point`, with a pixel `pixel` metres wide (the
/// edges' antialiasing) and enemy telegraphs' opacity floored at `floor` as the renderer does.
/// The same shape coverage (rim band, halo, cast-timer fill and wash) as the shader, so tests
/// can check the telegraph contrast per shape. A paint decal returns `background`.
#[must_use]
pub fn glow_reference(
    background: Vec3,
    decal: &Decal,
    floor: f32,
    point: Vec3,
    pixel: f32,
) -> Vec3 {
    if decal.style != DecalStyle::Glow || !decal.usable() {
        return background;
    }
    let record = gpu_decal(decal, floor.clamp(0.0, 1.0));
    glow_over(
        background,
        &record,
        decal_coverage(&record, point, pixel.max(1e-4)),
    )
}

/// A map's foliage, uploaded once ([`Renderer::set_foliage`](crate::Renderer::set_foliage)).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct FoliageData<'a> {
    /// The instances (at most [`MAX_FOLIAGE_INSTANCES`] and the config's cap; more are
    /// ignored).
    pub instances: &'a [FoliageInstanceData],
    /// Linear colour at the blades' base.
    pub base_color: [f32; 3],
    /// Linear colour at the tips.
    pub tip_color: [f32; 3],
    /// Linear colour of flower heads.
    pub flower_color: [f32; 3],
    /// Wind direction times strength (world x, z).
    pub wind: [f32; 2],
    /// Gust frequency, cycles per second.
    pub wind_speed: f32,
}

impl<'a> FoliageData<'a> {
    /// Foliage with the default colours and wind.
    #[must_use]
    pub const fn new(instances: &'a [FoliageInstanceData]) -> Self {
        Self {
            instances,
            base_color: [0.05, 0.16, 0.04],
            tip_color: [0.35, 0.55, 0.18],
            flower_color: [0.9, 0.75, 0.3],
            wind: [0.8, 0.3],
            wind_speed: 1.2,
        }
    }

    /// Sets the base, tip and flower colours.
    #[must_use]
    pub const fn with_colors(mut self, base: [f32; 3], tip: [f32; 3], flower: [f32; 3]) -> Self {
        self.base_color = base;
        self.tip_color = tip;
        self.flower_color = flower;
        self
    }

    /// Sets the wind.
    #[must_use]
    pub const fn with_wind(mut self, wind: [f32; 2], speed: f32) -> Self {
        self.wind = wind;
        self.wind_speed = speed;
        self
    }
}

/// One foliage clump.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct FoliageInstanceData {
    /// Base position, world space.
    pub position: [f32; 3],
    /// Rotation about +y, radians.
    pub yaw: f32,
    /// Clump height, metres (`0.05..=2.5`).
    pub height: f32,
    /// Kind: 0 grass, 1 brush, 2 reeds, 3 flowers.
    pub kind: u32,
    /// Colour variation, `0.5..=1.5`.
    pub tint: f32,
}

impl FoliageInstanceData {
    /// A clump.
    #[must_use]
    pub const fn new(position: [f32; 3], yaw: f32, height: f32, kind: u32, tint: f32) -> Self {
        Self {
            position,
            yaw,
            height,
            kind,
            tint,
        }
    }

    /// The 16-byte GPU record: position and a packed word (yaw 8 bits, height 10 bits in
    /// 2.5 mm steps, kind 2 bits, tint 8 bits).
    pub(crate) fn pack(&self) -> Option<[u32; 4]> {
        if !(self.position.iter().all(|value| value.is_finite())
            && self.yaw.is_finite()
            && self.height.is_finite()
            && self.tint.is_finite())
        {
            return None;
        }
        let turns = (self.yaw / core::f32::consts::TAU).rem_euclid(1.0);
        let yaw = ((turns * 256.0) as u32).min(255);
        let height = ((self.height.clamp(0.0, 2.5) / 2.5 * 1023.0).round() as u32).min(1023);
        let kind = self.kind.min(3);
        let tint = (((self.tint.clamp(0.5, 1.5) - 0.5) * 255.0).round() as u32).min(255);
        Some([
            self.position[0].to_bits(),
            self.position[1].to_bits(),
            self.position[2].to_bits(),
            yaw | (height << 8) | (kind << 18) | (tint << 20),
        ])
    }
}

/// One unit bending the foliage this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct FoliageDisplacer {
    /// Ground position (world x, z).
    pub position: [f32; 2],
    /// Radius of influence, metres.
    pub radius: f32,
    /// Push strength, `0..=1`.
    pub strength: f32,
}

impl FoliageDisplacer {
    /// A displacer.
    #[must_use]
    pub const fn new(position: [f32; 2], radius: f32, strength: f32) -> Self {
        Self {
            position,
            radius,
            strength,
        }
    }
}

/// This frame's environment input ([`Scene::with_environment`](crate::Scene::with_environment)).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct EnvironmentFrame<'a> {
    /// Seconds of animation time (wind, water flow); the caller keeps it deterministic in
    /// captures.
    pub time: f32,
    /// Units bending the foliage: **only units the client draws this frame** (ADR-0017 §2).
    /// The first [`EnvironmentConfig::max_displacers`] are used.
    pub displacers: &'a [FoliageDisplacer],
}

impl<'a> EnvironmentFrame<'a> {
    /// The frame at `time` with `displacers`.
    #[must_use]
    pub const fn new(time: f32, displacers: &'a [FoliageDisplacer]) -> Self {
        Self { time, displacers }
    }
}

/// A map's water surface, uploaded once ([`Renderer::set_water`](crate::Renderer::set_water)).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct WaterData<'a> {
    /// Surface vertices, world space.
    pub vertices: &'a [WaterVertexData],
    /// Triangle list (counter-clockwise seen from above).
    pub indices: &'a [u32],
    /// One style per body ([`WaterVertexData::body`]), at most [`MAX_WATER_BODIES`].
    pub styles: &'a [WaterStyle],
}

impl<'a> WaterData<'a> {
    /// A water mesh.
    #[must_use]
    pub const fn new(
        vertices: &'a [WaterVertexData],
        indices: &'a [u32],
        styles: &'a [WaterStyle],
    ) -> Self {
        Self {
            vertices,
            indices,
            styles,
        }
    }
}

/// One water vertex (32 bytes on the GPU).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct WaterVertexData {
    /// Surface position, world space.
    pub position: [f32; 3],
    /// Bed depth below the surface, metres (negative over dry ground: discarded).
    pub depth: f32,
    /// Flow velocity (world x, z), metres per second.
    pub flow: [f32; 2],
    /// Body index into [`WaterData::styles`].
    pub body: u32,
    /// Reserved, zero.
    pub reserved: u32,
}

impl WaterVertexData {
    /// A vertex.
    #[must_use]
    pub const fn new(position: [f32; 3], depth: f32, flow: [f32; 2], body: u32) -> Self {
        Self {
            position,
            depth,
            flow,
            body,
            reserved: 0,
        }
    }
}

/// One water body's look.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct WaterStyle {
    /// Linear colour of deep water.
    pub deep: [f32; 3],
    /// Linear colour of shallow water.
    pub shallow: [f32; 3],
    /// Depth, metres, at which the water is about 63 % opaque.
    pub clarity: f32,
    /// Shoreline and contact foam, `0..=1`.
    pub foam: f32,
}

impl WaterStyle {
    /// A style.
    #[must_use]
    pub const fn new(deep: [f32; 3], shallow: [f32; 3], clarity: f32, foam: f32) -> Self {
        Self {
            deep,
            shallow,
            clarity,
            foam,
        }
    }
}

impl Default for WaterStyle {
    fn default() -> Self {
        Self::new([0.02, 0.12, 0.16], [0.10, 0.30, 0.30], 0.35, 0.6)
    }
}

/// The environment uniform shared by the foliage and water pipelines (`EnvironmentBlock` in
/// `environment.glsl`, std140, `vec4`s only).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct EnvironmentBlock {
    /// Inverse view-projection (water depth reconstruction).
    pub(crate) inverse_view_proj: [[f32; 4]; 4],
    /// x time, y gust speed, z blades per clump, w draw distance.
    pub(crate) time: [f32; 4],
    /// xy wind (world x, z), z displacers, w water normal octaves.
    pub(crate) wind: [f32; 4],
    pub(crate) base_color: [f32; 4],
    pub(crate) tip_color: [f32; 4],
    pub(crate) flower_color: [f32; 4],
    /// xy 1 / target size, z refraction strength, w 1 = full water.
    pub(crate) screen: [f32; 4],
    /// xyz the camera's focus on the ground ([`ground_focus`]), for the foliage distance.
    pub(crate) focus: [f32; 4],
    /// xz, radius, strength.
    pub(crate) displacers: [[f32; 4]; MAX_FOLIAGE_DISPLACERS],
    /// rgb deep, w clarity.
    pub(crate) water_deep: [[f32; 4]; MAX_WATER_BODIES],
    /// rgb shallow, w foam.
    pub(crate) water_shallow: [[f32; 4]; MAX_WATER_BODIES],
}

impl EnvironmentBlock {
    pub(crate) fn zeroed() -> Self {
        Zeroable::zeroed()
    }
}

/// The map's foliage look, kept for the per-frame block.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FoliageLook {
    pub(crate) base_color: [f32; 3],
    pub(crate) tip_color: [f32; 3],
    pub(crate) flower_color: [f32; 3],
    pub(crate) wind: [f32; 2],
    pub(crate) wind_speed: f32,
}

impl FoliageLook {
    pub(crate) fn from_data(data: &FoliageData<'_>) -> Self {
        let colour = |rgb: [f32; 3]| rgb.map(|value| sanitize(value, 0.0, 4.0));
        Self {
            base_color: colour(data.base_color),
            tip_color: colour(data.tip_color),
            flower_color: colour(data.flower_color),
            wind: data.wind.map(|value| sanitize(value, -4.0, 4.0)),
            wind_speed: sanitize(data.wind_speed, 0.0, 8.0),
        }
    }
}

fn sanitize(value: f32, low: f32, high: f32) -> f32 {
    if value.is_finite() {
        value.clamp(low, high)
    } else {
        low
    }
}

/// One culling chunk of the uploaded foliage: a contiguous instance range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FoliageChunk {
    pub(crate) first: u32,
    pub(crate) count: u32,
    pub(crate) min: Vec3,
    pub(crate) max: Vec3,
}

/// Packs `instances` (up to `cap`) sorted into [`FOLIAGE_CHUNK`] chunks, row-major, and the
/// chunks. Load time only.
pub(crate) fn chunk_foliage(
    instances: &[FoliageInstanceData],
    cap: usize,
) -> (Vec<[u32; 4]>, Vec<FoliageChunk>) {
    /// Chunk row, column, packed record, position and height.
    type Keyed = (i32, i32, [u32; 4], [f32; 3], f32);
    let mut keyed: Vec<Keyed> = instances
        .iter()
        .take(cap.min(MAX_FOLIAGE_INSTANCES))
        .filter_map(|instance| {
            let packed = instance.pack()?;
            let [x, _, z] = instance.position;
            let key = |value: f32| (value / FOLIAGE_CHUNK).floor().clamp(-4096.0, 4096.0) as i32;
            Some((key(z), key(x), packed, instance.position, instance.height))
        })
        .collect();
    keyed.sort_by_key(|entry| (entry.0, entry.1));
    let mut packed = Vec::with_capacity(keyed.len());
    let mut chunks: Vec<FoliageChunk> = Vec::new();
    let mut current: Option<(i32, i32)> = None;
    for (row, column, record, position, height) in keyed {
        let index = u32::try_from(packed.len()).expect("bounded by MAX_FOLIAGE_INSTANCES");
        packed.push(record);
        let point = Vec3::from_array(position);
        // Brush leans out up to its height; the bound grows by it.
        let reach = Vec3::new(height.max(0.5), height.max(0.0) + 0.5, height.max(0.5));
        if current == Some((row, column))
            && let Some(chunk) = chunks.last_mut()
        {
            chunk.count += 1;
            chunk.min = chunk.min.min(point - reach);
            chunk.max = chunk.max.max(point + reach);
        } else {
            current = Some((row, column));
            chunks.push(FoliageChunk {
                first: index,
                count: 1,
                min: point - reach,
                max: point + reach,
            });
        }
    }
    (packed, chunks)
}

/// The six frustum planes of `view_proj` (normal xyz, distance w; inside is positive).
pub(crate) fn frustum_planes(view_proj: Mat4) -> [Vec4; 6] {
    let rows = [
        view_proj.row(0),
        view_proj.row(1),
        view_proj.row(2),
        view_proj.row(3),
    ];
    let planes = [
        rows[3] + rows[0],
        rows[3] - rows[0],
        rows[3] + rows[1],
        rows[3] - rows[1],
        // Vulkan depth 0..1: near is row 2 alone.
        rows[2],
        rows[3] - rows[2],
    ];
    planes.map(|plane| {
        let length = plane.truncate().length();
        if length > 0.0 { plane / length } else { plane }
    })
}

/// Whether the box `min..max` is at least partly inside every plane.
pub(crate) fn box_visible(planes: &[Vec4; 6], min: Vec3, max: Vec3) -> bool {
    planes.iter().all(|plane| {
        let corner = Vec3::new(
            if plane.x >= 0.0 { max.x } else { min.x },
            if plane.y >= 0.0 { max.y } else { min.y },
            if plane.z >= 0.0 { max.z } else { min.z },
        );
        plane.truncate().dot(corner) + plane.w >= 0.0
    })
}

/// Where the camera looks on the ground: the screen centre's ray met with the `y = 0`
/// plane, or `eye` when that ray does not reach the ground (a level or upward view).
/// ADR-0017 §2: foliage distance is measured from here as well as from the eye, so a high
/// MOBA camera (46 m back by default) keeps foliage to the top of the screen.
pub(crate) fn ground_focus(view_proj: Mat4, eye: Vec3) -> Vec3 {
    let inverse = view_proj.inverse();
    let near = inverse.project_point3(Vec3::new(0.0, 0.0, 0.0));
    let far = inverse.project_point3(Vec3::new(0.0, 0.0, 1.0));
    let direction = far - near;
    if !near.is_finite() || !far.is_finite() || direction.y >= -1e-4 {
        return eye;
    }
    let t = -near.y / direction.y;
    if (0.0..=1.0).contains(&t) {
        near + direction * t
    } else {
        eye
    }
}

/// Appends the visible chunks of `chunks` to `draws` as merged `(first, count)` runs, within
/// `distance` of `eye` or of `focus` (the camera's focus on the ground, [`ground_focus`]).
/// `draws` must have capacity for every chunk (it never reallocates).
pub(crate) fn visible_runs(
    chunks: &[FoliageChunk],
    planes: &[Vec4; 6],
    eye: Vec3,
    focus: Vec3,
    distance: f32,
    draws: &mut Vec<(u32, u32)>,
) {
    draws.clear();
    for chunk in chunks {
        let from_eye = eye.clamp(chunk.min, chunk.max).distance(eye);
        let from_focus = focus.clamp(chunk.min, chunk.max).distance(focus);
        if from_eye.min(from_focus) > distance || !box_visible(planes, chunk.min, chunk.max) {
            continue;
        }
        if let Some(run) = draws.last_mut()
            && run.0 + run.1 == chunk.first
        {
            run.1 += chunk.count;
        } else if draws.len() < draws.capacity() {
            draws.push((chunk.first, chunk.count));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disc(x: f32, role: DecalRole) -> Decal {
        Decal::new(Vec3::new(x, 0.0, 0.0), DecalShape::Disc, 2.0, Vec4::ONE).with_role(role)
    }

    #[test]
    fn decal_records_match_the_std430_buffer() {
        assert_eq!(core::mem::size_of::<GpuDecal>(), 64);
        assert_eq!(
            core::mem::offset_of!(DecalUpload, decals),
            32,
            "header is a vec4 and a uvec4"
        );
        assert_eq!(
            core::mem::offset_of!(DecalUpload, cells),
            32 + 64 * MAX_DECALS
        );
        assert_eq!(DECAL_BUFFER_SIZE, 32 + 64 * 128 + 16 * 1024);
        assert_eq!(core::mem::size_of::<WaterVertexData>(), 32);
        assert_eq!(core::mem::size_of::<EnvironmentBlock>() % 16, 0);
    }

    #[test]
    fn enemy_telegraphs_go_first_and_are_never_dropped_for_others() {
        let mut upload = DecalUpload::boxed();
        let mut decals = vec![disc(0.0, DecalRole::Cosmetic); 10];
        decals.push(disc(5.0, DecalRole::EnemyTelegraph));
        decals.push(disc(6.0, DecalRole::Ally));
        let dropped = upload.build(&decals, 4, 0.35, None);
        assert_eq!(upload.count(), 4);
        assert_eq!(dropped, 8);
        // The enemy telegraph is record 0 and its opacity is floored.
        assert_eq!(upload.decals[0].placement[0], 5.0);
        assert_eq!(upload.header.counts[2], 1);
        let faint = disc(1.0, DecalRole::EnemyTelegraph);
        let faint = Decal {
            color: Vec4::new(1.0, 0.0, 0.0, 0.05),
            ..faint
        };
        let _ = upload.build(&[faint], 0, 0.35, None);
        assert_eq!(
            upload.count(),
            1,
            "a cap of zero still keeps enemy telegraphs"
        );
        assert!((upload.decals[0].color[3] - 0.35).abs() < 1e-6);
        // Ally telegraphs keep their own opacity.
        let ally = Decal {
            color: Vec4::new(1.0, 0.0, 0.0, 0.05),
            ..disc(1.0, DecalRole::Ally)
        };
        let _ = upload.build(&[ally], 8, 0.35, None);
        assert!((upload.decals[0].color[3] - 0.05).abs() < 1e-6);
    }

    #[test]
    fn many_enemy_telegraphs_fill_the_buffer_before_anything_else() {
        let mut upload = DecalUpload::boxed();
        let decals: Vec<Decal> = (0..200)
            .map(|index| disc(index as f32, DecalRole::EnemyTelegraph))
            .collect();
        let dropped = upload.build(&decals, 16, 0.35, None);
        assert_eq!(upload.count(), MAX_DECALS);
        assert_eq!(dropped, 200 - MAX_DECALS as u32);
    }

    /// A telegraph far off screen (a global ultimate, a fight 900 m away) is skipped instead
    /// of stretching the grid: the visible decals keep metre-scale cells.
    #[test]
    fn decals_outside_the_view_are_skipped_and_do_not_stretch_the_grid() {
        let mut upload = DecalUpload::boxed();
        let decals = [
            disc(-10.0, DecalRole::Cosmetic),
            disc(900.0, DecalRole::EnemyTelegraph),
            disc(10.0, DecalRole::Ally),
        ];
        let view = Some([-40.0, -40.0, 40.0, 40.0]);
        let dropped = upload.build(&decals, 8, 0.35, view);
        assert_eq!(dropped, 0, "an unseen decal is not a dropped one");
        assert_eq!(upload.count(), 2);
        let rect = upload.header.rect;
        assert!(
            rect[2] >= 1.0 && rect[3] >= 1.0,
            "cells stay a metre or finer: {rect:?}"
        );
        // A decal straddling the view edge is kept.
        let edge = [disc(41.5, DecalRole::Cosmetic)];
        let _ = upload.build(&edge, 8, 0.35, view);
        assert_eq!(upload.count(), 1);
        // Without a view (a degenerate camera) nothing is culled.
        let _ = upload.build(&decals, 8, 0.35, None);
        assert_eq!(upload.count(), 3);
    }

    #[test]
    fn binning_marks_exactly_the_cells_a_decal_covers() {
        let mut upload = DecalUpload::boxed();
        let decals = [
            disc(-50.0, DecalRole::Cosmetic),
            disc(50.0, DecalRole::Cosmetic),
        ];
        let _ = upload.build(&decals, 8, 0.35, None);
        let rect = upload.header.rect;
        let cell_of = |x: f32, z: f32| {
            let column = ((x - rect[0]) * rect[2]).floor() as usize;
            let row = ((z - rect[1]) * rect[3]).floor() as usize;
            upload.cells[row.min(31) * DECAL_GRID + column.min(31)]
        };
        assert_eq!(cell_of(-50.0, 0.0)[0], 1);
        assert_eq!(cell_of(50.0, 0.0)[0], 2);
        assert_eq!(cell_of(0.0, 0.0)[0], 0, "nothing between them");
        // A rebuild with nothing clears the grid and the count.
        let _ = upload.build(&[], 8, 0.35, None);
        assert_eq!(upload.count(), 0);
        assert!(upload.cells.iter().all(|cell| *cell == [0; 4]));
    }

    #[test]
    fn unusable_decals_are_skipped() {
        let mut upload = DecalUpload::boxed();
        let bad = [
            Decal::new(Vec3::NAN, DecalShape::Disc, 2.0, Vec4::ONE),
            Decal::new(Vec3::ZERO, DecalShape::Disc, 0.0, Vec4::ONE),
            disc(0.0, DecalRole::Cosmetic).with_fade(0.0),
            disc(0.0, DecalRole::Cosmetic).with_progress(Some(f32::NAN)),
        ];
        assert_eq!(upload.build(&bad, 8, 0.35, None), 0);
        assert_eq!(upload.count(), 0);
    }

    /// ADR-0017 §1: over any background from black to 16× white, every enemy telegraph shape
    /// at the floor opacity leaves a rim band at least 1.5× brighter than the ground or a
    /// halo just inside it at most 1/1.5 of it, the rim always lifts, and on any ground
    /// bright enough to darken the halo is there. Evaluated through the shader's coverage
    /// (`glow_reference`), cast-timer wash included, so a shape whose band and halo
    /// coincide fails here.
    #[test]
    fn enemy_telegraph_rims_contrast_with_any_background() {
        let colours = [
            Vec3::new(1.0, 0.1, 0.05),
            Vec3::new(0.02, 0.02, 0.02),
            Vec3::new(0.9, 0.9, 0.9),
            Vec3::new(0.1, 0.4, 1.0),
        ];
        let centre = Vec3::new(10.0, 0.0, -4.0);
        // (shape, radius, width, the band's middle and the halo's middle along +x).
        let shapes = [
            (DecalShape::Ring, 7.0, 0.35, 7.0, 7.0 - 0.35),
            (DecalShape::Ring, 3.0, 0.2, 3.0, 3.0 - 0.2),
            (DecalShape::Disc, 3.0, 0.5, 3.0 - 0.12, 3.0 - 0.36),
            (DecalShape::Cone, 6.0, 0.5, 6.0 - 0.17, 6.0 - 0.5),
            (DecalShape::Line, 8.0, 1.5, 8.0 - 0.11, 8.0 - 0.34),
        ];
        for (shape, radius, width, band_at, halo_at) in shapes {
            for progress in [None, Some(0.3), Some(0.97)] {
                for colour in colours {
                    // Faint (below the floor), so the floor sets the opacity.
                    let decal = Decal::new(centre, shape, radius, colour.extend(0.05))
                        .with_width(width)
                        .with_angle(1.2)
                        .with_role(DecalRole::EnemyTelegraph)
                        .with_progress(progress);
                    for step in 0..=64 {
                        let level = 16.0 * (step as f32 / 64.0).powi(2);
                        for tint in [
                            Vec3::ONE,
                            Vec3::new(0.3, 1.0, 0.2),
                            Vec3::new(1.0, 0.5, 0.1),
                        ] {
                            let background = tint * level;
                            let lb = luminance(background);
                            let at = |x: f32| {
                                glow_reference(background, &decal, 0.35, centre + Vec3::X * x, 0.01)
                            };
                            let rim = at(band_at);
                            let halo = at(halo_at);
                            let rim_ratio = (luminance(rim) + 0.05) / (lb + 0.05);
                            let halo_ratio = (lb + 0.05) / (luminance(halo) + 0.05);
                            let what =
                                format!("{shape:?} {progress:?} {colour:?} over {background:?}");
                            assert!(
                                rim_ratio >= 1.5 || halo_ratio >= 1.5,
                                "{what}: rim {rim_ratio}, halo {halo_ratio}"
                            );
                            assert!(rim_ratio >= 1.2, "{what}: the rim alone lifts: {rim_ratio}");
                            if lb >= 0.5 {
                                assert!(
                                    halo_ratio >= 1.5,
                                    "{what}: a dark halo inside the band: {halo_ratio}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// ADR-0017 §1: a telegraph's cast timer fills its inside from the centre out to
    /// `radius × progress`, rings included; without a timer a ring leaves its inside alone.
    #[test]
    fn telegraph_progress_fills_rings_and_discs() {
        let centre = Vec3::new(-3.0, 0.2, 5.0);
        let background = Vec3::splat(0.3);
        let colour = Vec4::new(1.0, 0.2, 0.1, 0.6);
        for shape in [DecalShape::Ring, DecalShape::Disc] {
            let timed = Decal::new(centre, shape, 6.0, colour)
                .with_width(0.3)
                .with_role(DecalRole::EnemyTelegraph)
                .with_progress(Some(0.5));
            let at = |decal: &Decal, x: f32| {
                glow_reference(background, decal, 0.35, centre + Vec3::Z * x, 0.01)
            };
            let filled = at(&timed, 1.5);
            let ahead = at(&timed, 4.2);
            assert!(
                (filled - background).length() > 0.05,
                "{shape:?}: the filled part is washed: {filled:?}"
            );
            assert!(
                (filled - ahead).length() > 0.02,
                "{shape:?}: the fill stops at the sweep: {filled:?} vs {ahead:?}"
            );
            if shape == DecalShape::Ring {
                let untimed = timed.with_progress(None);
                assert_eq!(
                    at(&untimed, 1.5),
                    background,
                    "an untimed ring's inside is clear"
                );
                assert_eq!(at(&untimed, 4.2), background);
                assert_ne!(at(&untimed, 6.0), background, "but its band shows");
            }
        }
    }

    #[test]
    fn foliage_packs_and_chunks_row_major() {
        let instances = [
            FoliageInstanceData::new([40.0, 0.0, 1.0], 1.0, 0.5, 0, 1.0),
            FoliageInstanceData::new([1.0, 0.1, 1.0], 0.0, 1.2, 1, 0.9),
            FoliageInstanceData::new([2.0, 0.0, 2.0], 7.0, 0.4, 3, 1.4),
            FoliageInstanceData::new([f32::NAN, 0.0, 2.0], 0.0, 0.4, 0, 1.0),
        ];
        let (packed, chunks) = chunk_foliage(&instances, 16);
        assert_eq!(packed.len(), 3, "the non-finite clump is dropped");
        assert_eq!(chunks.len(), 2);
        assert_eq!((chunks[0].first, chunks[0].count), (0, 2));
        assert_eq!((chunks[1].first, chunks[1].count), (2, 1));
        let word = packed[0][3];
        assert_eq!(
            (word >> 18) & 3,
            1,
            "brush first: chunk (0, 0) in input order"
        );
        let height = ((word >> 8) & 1023) as f32 / 1023.0 * 2.5;
        assert!((height - 1.2).abs() < 0.003);
        let (capped, _) = chunk_foliage(&instances, 1);
        assert_eq!(capped.len(), 1);
    }

    #[test]
    fn culling_merges_adjacent_visible_chunks() {
        let chunk = |first, x: f32| FoliageChunk {
            first,
            count: 10,
            min: Vec3::new(x, 0.0, 0.0),
            max: Vec3::new(x + 16.0, 1.0, 16.0),
        };
        let chunks = [chunk(0, 0.0), chunk(10, 16.0), chunk(20, 500.0)];
        let camera = crate::Camera::perspective(
            Vec3::new(16.0, 40.0, -20.0),
            Vec3::new(16.0, 0.0, 8.0),
            Vec3::Y,
            0.8,
            16.0 / 9.0,
            0.5,
            1_000.0,
        );
        let planes = frustum_planes(camera.view_proj);
        let focus = ground_focus(camera.view_proj, camera.position);
        assert!(
            focus.distance(Vec3::new(16.0, 0.0, 8.0)) < 1e-3,
            "{focus:?}"
        );
        let mut draws = Vec::with_capacity(chunks.len());
        visible_runs(&chunks, &planes, camera.position, focus, 130.0, &mut draws);
        assert_eq!(
            draws,
            [(0, 20)],
            "far chunk culled by distance, the rest merged"
        );
        visible_runs(&chunks, &planes, camera.position, focus, 5.0, &mut draws);
        assert_eq!(draws, [(0, 20)], "the chunks under the focus stay");
        visible_runs(
            &chunks,
            &planes,
            camera.position,
            camera.position,
            5.0,
            &mut draws,
        );
        assert_eq!(
            draws,
            [],
            "measured from the eye alone, 5 m reaches nothing"
        );
    }

    /// ADR-0017 §2: with the client's player camera (`Camera::orbit`, 46 m back at pitch
    /// 0.95), the ground at the top of the screen is over 90 m from the eye but under 60 m
    /// from the focus, so the Low tier's 60 m keeps foliage to the top of the screen.
    #[test]
    fn foliage_distance_counts_from_the_focus_too() {
        let target = Vec3::new(5.0, 0.0, 5.0);
        let camera = crate::Camera::orbit(target, 46.0, -0.7, 0.95, 16.0 / 9.0);
        let eye = camera.position;
        let focus = ground_focus(camera.view_proj, camera.position);
        assert!(focus.distance(target) < 1e-2, "{focus:?}");
        // The ground at the top edge of the screen.
        let inverse = camera.view_proj.inverse();
        let near = inverse.project_point3(Vec3::new(0.0, 1.0, 0.0));
        let far = inverse.project_point3(Vec3::new(0.0, 1.0, 1.0));
        let top = near + (far - near) * (-near.y / (far - near).y);
        assert!(top.distance(eye) > 90.0, "{}", top.distance(eye));
        assert!(top.distance(focus) < 58.0, "{}", top.distance(focus));
        let chunk = FoliageChunk {
            first: 0,
            count: 4,
            min: top - Vec3::new(1.0, 0.0, 1.0),
            max: top + Vec3::new(1.0, 1.0, 1.0),
        };
        let planes = frustum_planes(camera.view_proj);
        let mut draws = Vec::with_capacity(1);
        visible_runs(&[chunk], &planes, eye, focus, 60.0, &mut draws);
        assert_eq!(draws, [(0, 4)]);
        // A level camera (the horizon preview) has no ground focus: the eye is used.
        let level = crate::Camera::perspective(
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(0.0, 2.0, -10.0),
            Vec3::Y,
            0.8,
            1.0,
            0.5,
            400.0,
        );
        assert_eq!(
            ground_focus(level.view_proj, level.position),
            level.position
        );
    }

    #[test]
    fn tiers_scale_every_feature() {
        let low = EnvironmentConfig::for_tier(QualityTier::Low);
        let ultra = EnvironmentConfig::for_tier(QualityTier::Ultra);
        assert_eq!(low.water, WaterMode::Simple);
        assert_eq!(ultra.water, WaterMode::Full);
        assert!(low.max_foliage < ultra.max_foliage);
        assert!(low.foliage_distance < ultra.foliage_distance);
        assert!(low.max_decals < ultra.max_decals);
        let wild = EnvironmentConfig {
            max_decals: 9_999,
            foliage_blades: 99,
            foliage_distance: f32::NAN,
            ..ultra
        }
        .sanitized();
        assert_eq!(wild.max_decals, MAX_DECALS as u32);
        assert_eq!(wild.foliage_blades, MAX_FOLIAGE_BLADES);
        assert!(wild.foliage_distance.is_finite());
        assert_eq!(WaterMode::from_name(" Simple "), Some(WaterMode::Simple));
    }
}
