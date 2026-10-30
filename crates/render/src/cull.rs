//! Per-view frustum culling, terrain chunk runs and mesh LOD selection (R8, ADR-0018).
//!
//! Everything here is pure math on fixed-size data, unit-tested without a device. The scene
//! pass calls it while it buckets a view's instances ([`crate::passes::scene`]).
//!
//! **Exactness.** A sphere or box is dropped only when it lies wholly outside one plane, so a
//! culled frame draws a subset of the unculled frame's primitives that contains every primitive
//! that can touch a pixel, in the same order: the images are byte-identical (the golden
//! workflow checks it). Non-finite inputs are never culled.

use moba_core::math::{Mat4, Vec3, Vec4};

use crate::QualityTier;
use crate::mesh::MeshLodTable;

/// How the renderer culls ([`RenderConfig::culling`](crate::RenderConfig::culling)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CullMode {
    /// Draw everything (the reference the culled image must equal).
    Off,
    /// Per-view CPU frustum culling of instances and terrain chunks. The default. (A
    /// GPU-driven path is deferred until instance counts justify it; ADR-0018 §2.)
    #[default]
    Cpu,
}

impl CullMode {
    /// `off` or `cpu`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Cpu => "cpu",
        }
    }

    /// Parses `off` (also `0`, `none`) or `cpu` (also `on`, `1`), ignoring ASCII case and
    /// surrounding whitespace.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "none" => Some(Self::Off),
            "cpu" | "on" | "1" => Some(Self::Cpu),
            _ => None,
        }
    }
}

impl core::fmt::Display for CullMode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

/// Mesh level-of-detail selection ([`RenderConfig::mesh_lod`](crate::RenderConfig::mesh_lod)).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct LodConfig {
    /// Select levels at all; off draws every mesh at LOD0.
    pub enabled: bool,
    /// The largest projected error, in pixels, a drawn level may have.
    pub tolerance_px: f32,
}

impl LodConfig {
    /// Every mesh at LOD0.
    pub const OFF: Self = Self {
        enabled: false,
        tolerance_px: 0.0,
    };

    /// The tier's tolerance (`moba_assets::TIER_BUDGETS`): Low 2 px, Medium 1, High 0.75,
    /// Ultra 0.5.
    #[must_use]
    pub fn for_tier(tier: QualityTier) -> Self {
        Self {
            enabled: true,
            tolerance_px: tier.budget().lod_tolerance_px,
        }
    }

    /// Selection on, with `tolerance_px` (clamped to `0..=64`).
    #[must_use]
    pub fn with_tolerance(tolerance_px: f32) -> Self {
        Self {
            enabled: true,
            tolerance_px: if tolerance_px.is_finite() {
                tolerance_px.clamp(0.0, 64.0)
            } else {
                0.0
            },
        }
    }
}

/// What the main view culled and selected in the most recent frame
/// ([`Renderer::cull_stats`](crate::Renderer::cull_stats)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CullStats {
    /// The mode in use (after any fallback).
    pub mode: CullMode,
    /// Units the scene handed in.
    pub candidates: u32,
    /// Units in the camera list (drawn by the prepass and the scene).
    pub drawn: u32,
    /// Units in the shadow caster list (0 without shadows).
    pub casters: u32,
    /// Units a list would draw that did not fit the instance cap (`max_instances`), applied
    /// after culling; 0 unless the scene has more visible units than the renderer holds.
    pub dropped: u32,
    /// Terrain chunks of the drawn terrain, and how many the main view drew.
    pub terrain_chunks: u32,
    /// Terrain chunks drawn by the main view.
    pub terrain_chunks_drawn: u32,
    /// Camera-list units per mesh level (LOD0 first).
    pub lod_instances: [u32; crate::mesh::MAX_LODS],
}

/// Which planes of a view-projection a [`Frustum`] tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Planes {
    /// Left, right, bottom, top, near, far (a camera; Vulkan depth `0..1`).
    All,
    /// Left, right, bottom, top only: the shadow box, whose near and far casters are
    /// pancaked by `depthClamp` and still write the map (ADR-0011).
    Sides,
}

/// Up to six normalised planes `(n, d)` with `n·p + d >= 0` inside.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Frustum {
    planes: [Vec4; 6],
    count: usize,
}

impl Frustum {
    /// Accepts everything (no planes).
    pub(crate) const EVERYTHING: Self = Self {
        planes: [Vec4::ZERO; 6],
        count: 0,
    };

    /// The planes of `view_proj` (Gribb–Hartmann). A non-finite or degenerate matrix gives
    /// [`Self::EVERYTHING`], so a bad camera never culls anything.
    pub(crate) fn new(view_proj: Mat4, planes: Planes) -> Self {
        if !view_proj.is_finite() {
            return Self::EVERYTHING;
        }
        let [r0, r1, r2, r3] = [0, 1, 2, 3].map(|row| view_proj.row(row));
        let all = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2];
        let count = match planes {
            Planes::All => 6,
            Planes::Sides => 4,
        };
        let mut out = [Vec4::ZERO; 6];
        for (slot, plane) in out.iter_mut().zip(&all[..count]) {
            let length = plane.truncate().length();
            if !length.is_finite() || length < 1e-12 {
                return Self::EVERYTHING;
            }
            *slot = *plane / length;
        }
        Self { planes: out, count }
    }

    /// Whether any part of the sphere can be inside. Non-finite spheres are kept.
    pub(crate) fn sphere_visible(&self, center: Vec3, radius: f32) -> bool {
        if !center.is_finite() || !radius.is_finite() {
            return true;
        }
        self.planes[..self.count]
            .iter()
            .all(|plane| plane.truncate().dot(center) + plane.w >= -radius)
    }

    /// Whether any part of the axis-aligned box can be inside (the box's corner furthest
    /// along each plane's normal is tested).
    pub(crate) fn aabb_visible(&self, min: [f32; 3], max: [f32; 3]) -> bool {
        let (min, max) = (Vec3::from_array(min), Vec3::from_array(max));
        if !min.is_finite() || !max.is_finite() {
            return true;
        }
        self.planes[..self.count].iter().all(|plane| {
            let normal = plane.truncate();
            let corner = Vec3::select(normal.cmpge(Vec3::ZERO), max, min);
            normal.dot(corner) + plane.w >= 0.0
        })
    }
}

/// A mesh's model-space sphere `[x, y, z, r]` through `transform`: the transformed centre and
/// the radius times the largest axis scale.
pub(crate) fn world_sphere(transform: &Mat4, local: [f32; 4]) -> (Vec3, f32) {
    let center = transform.transform_point3(Vec3::new(local[0], local[1], local[2]));
    let scale = max_axis_scale(transform);
    (center, local[3] * scale)
}

/// The largest column length of the upper 3×3 (an upper bound of the stretch along any axis).
pub(crate) fn max_axis_scale(transform: &Mat4) -> f32 {
    transform
        .x_axis
        .truncate()
        .length()
        .max(transform.y_axis.truncate().length())
        .max(transform.z_axis.truncate().length())
}

/// Projects object-space errors to pixels for one view (perspective or orthographic).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LodMetric {
    /// `w` of a point: `row3 · (p, 1)` (view depth for a perspective camera, 1 for ortho).
    row3: Vec4,
    /// Pixels per unit of clip-space `y`/`w`: half the view height times `|row1.xyz|`.
    pixels_per_unit: f32,
    tolerance_px: f32,
    enabled: bool,
}

impl LodMetric {
    /// Always LOD0.
    pub(crate) const OFF: Self = Self {
        row3: Vec4::ZERO,
        pixels_per_unit: 0.0,
        tolerance_px: 0.0,
        enabled: false,
    };

    /// The metric of a view `height` pixels tall drawn with `view_proj`.
    pub(crate) fn new(view_proj: Mat4, height: u32, config: LodConfig) -> Self {
        if !config.enabled || !view_proj.is_finite() {
            return Self::OFF;
        }
        let row1 = view_proj.row(1).truncate().length();
        Self {
            row3: view_proj.row(3),
            pixels_per_unit: 0.5 * height as f32 * row1,
            tolerance_px: config.tolerance_px,
            enabled: true,
        }
    }

    /// Pixels `error` model units span at `center` for an instance scaled by `scale`.
    pub(crate) fn pixels(&self, center: Vec3, scale: f32, error: f32) -> f32 {
        // Points at or behind the eye (shadow-only casters) get a small depth: a fine level.
        let w = self.row3.dot(center.extend(1.0)).max(1e-2);
        error * scale * self.pixels_per_unit / w
    }

    /// The coarsest level of `lods` whose projected error is within the tolerance, at least
    /// `finest` (the tier's triangle budget, ADR-0018 §4), clamped to the levels there are.
    pub(crate) fn select(&self, lods: &MeshLodTable, finest: u8, center: Vec3, scale: f32) -> u8 {
        let count = lods.count.max(1);
        let finest = finest.min(count - 1);
        if !self.enabled || count == 1 {
            return finest;
        }
        for level in (usize::from(finest) + 1..usize::from(count)).rev() {
            let error = lods.levels[level].error;
            let pixels = self.pixels(center, scale, error);
            if pixels.is_finite() && pixels <= self.tolerance_px {
                return u8::try_from(level).unwrap_or(finest);
            }
        }
        finest
    }
}

/// Merges the visible chunks of `chunks` (in index-buffer order) into contiguous
/// `(first_index, index_count)` runs written to `runs` (cleared first, never grown past its
/// capacity: a run that does not fit is merged into the last one, which only draws more).
/// Returns the number of visible chunks.
pub(crate) fn terrain_runs(
    chunks: &[crate::terrain::TerrainChunk],
    frustum: &Frustum,
    runs: &mut Vec<(u32, u32)>,
) -> u32 {
    runs.clear();
    let mut visible = 0u32;
    for chunk in chunks {
        if chunk.index_count == 0 || !frustum.aabb_visible(chunk.min, chunk.max) {
            continue;
        }
        visible += 1;
        let full = runs.len() == runs.capacity();
        match runs.last_mut() {
            Some((first, count)) if *first + *count == chunk.first_index => {
                *count += chunk.index_count;
            }
            Some((first, count)) if full => {
                // Out of scratch: extend the last run over the gap (draws hidden chunks too).
                *count = chunk.first_index + chunk.index_count - *first;
            }
            _ => runs.push((chunk.first_index, chunk.index_count)),
        }
    }
    visible
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Camera;
    use crate::terrain::TerrainChunk;

    fn camera() -> Mat4 {
        Camera::perspective(
            Vec3::new(0.0, 10.0, 10.0),
            Vec3::ZERO,
            Vec3::Y,
            60f32.to_radians(),
            16.0 / 9.0,
            0.1,
            500.0,
        )
        .view_proj
    }

    #[test]
    fn spheres_are_culled_only_when_wholly_outside_a_plane() {
        let frustum = Frustum::new(camera(), Planes::All);
        assert!(frustum.sphere_visible(Vec3::ZERO, 0.5), "the target");
        assert!(
            !frustum.sphere_visible(Vec3::new(0.0, 10.0, 30.0), 1.0),
            "behind the eye"
        );
        assert!(
            !frustum.sphere_visible(Vec3::new(200.0, 0.0, 0.0), 1.0),
            "far to the side"
        );
        assert!(
            frustum.sphere_visible(Vec3::new(200.0, 0.0, 0.0), 500.0),
            "huge sphere"
        );
        assert!(
            !frustum.sphere_visible(Vec3::new(0.0, -2000.0, -2000.0), 1.0),
            "past far"
        );
        // A sphere straddling the right edge is kept; moved out far enough that its distance
        // to the slanted right plane (normal ≈ 0.7 along x at this field of view) passes its
        // radius, it is not.
        let mut x = 0.0f32;
        while frustum.sphere_visible(Vec3::new(x, 0.0, 0.0), 0.0) {
            x += 0.01;
        }
        assert!(frustum.sphere_visible(Vec3::new(x + 0.9, 0.0, 0.0), 1.0));
        assert!(!frustum.sphere_visible(Vec3::new(x + 1.6, 0.0, 0.0), 1.0));
        // NaN never culls; neither does a broken camera.
        assert!(frustum.sphere_visible(Vec3::NAN, 1.0));
        let broken = Frustum::new(Mat4::from_cols_array(&[f32::NAN; 16]), Planes::All);
        assert!(broken.sphere_visible(Vec3::new(1e6, 0.0, 0.0), 0.0));
    }

    #[test]
    fn the_shadow_box_keeps_casters_beyond_its_near_and_far_planes() {
        let light = moba_core::math::orthographic_rh(-10.0, 10.0, -10.0, 10.0, 0.0, 20.0)
            * moba_core::math::look_at_rh(Vec3::new(0.0, 10.0, 0.0), Vec3::ZERO, Vec3::Z);
        let sides = Frustum::new(light, Planes::Sides);
        let all = Frustum::new(light, Planes::All);
        let above = Vec3::new(0.0, 50.0, 0.0);
        assert!(!all.sphere_visible(above, 1.0));
        assert!(
            sides.sphere_visible(above, 1.0),
            "pancaked onto the near plane"
        );
        assert!(
            !sides.sphere_visible(Vec3::new(30.0, 0.0, 0.0), 1.0),
            "outside the box"
        );
    }

    #[test]
    fn boxes_test_their_furthest_corner() {
        let frustum = Frustum::new(camera(), Planes::All);
        assert!(frustum.aabb_visible([-1.0, 0.0, -1.0], [1.0, 1.0, 1.0]));
        assert!(!frustum.aabb_visible([300.0, 0.0, 0.0], [310.0, 1.0, 10.0]));
        assert!(frustum.aabb_visible([-1000.0, -1.0, -1000.0], [1000.0, 1.0, 1000.0]));
        assert!(Frustum::EVERYTHING.aabb_visible([1e9; 3], [1e9; 3]));
    }

    #[test]
    fn world_spheres_follow_the_largest_axis_scale() {
        let transform = Mat4::from_scale_rotation_translation(
            Vec3::new(1.0, 3.0, 2.0),
            moba_core::math::Quat::from_rotation_y(0.7),
            Vec3::new(5.0, 0.0, -1.0),
        );
        let (center, radius) = world_sphere(&transform, [0.0, 1.0, 0.0, 2.0]);
        assert!((center - Vec3::new(5.0, 3.0, -1.0)).length() < 1e-5);
        assert!((radius - 6.0).abs() < 1e-5);
    }

    fn lods(errors: &[f32]) -> MeshLodTable {
        let mut table = MeshLodTable::single(300);
        for &error in errors {
            let slot = usize::from(table.count);
            table.levels[slot].error = error;
            table.levels[slot].index_count = 3;
            table.count += 1;
        }
        table
    }

    #[test]
    fn levels_are_the_coarsest_within_the_pixel_tolerance() {
        let metric = LodMetric::new(camera(), 1080, LodConfig::with_tolerance(1.0));
        let table = lods(&[0.01, 0.05, 0.2]);
        // Pixels per metre at the origin, ≈ 14 m from the eye.
        let near = metric.pixels(Vec3::ZERO, 1.0, 1.0);
        assert!(near > 50.0 && near < 80.0, "{near}");
        assert_eq!(
            metric.select(&table, 0, Vec3::ZERO, 1.0),
            1,
            "0.01 m ≈ 0.7 px"
        );
        let far = Vec3::new(0.0, 0.0, -400.0);
        assert_eq!(
            metric.select(&table, 0, far, 1.0),
            3,
            "far away: the coarsest"
        );
        assert_eq!(
            metric.select(&table, 0, Vec3::ZERO, 10.0),
            0,
            "a big instance"
        );
        assert_eq!(
            metric.select(&table, 2, Vec3::ZERO, 10.0),
            2,
            "the tier's finest level"
        );
        assert_eq!(metric.select(&table, 9, Vec3::ZERO, 1.0), 3, "clamped");
        assert_eq!(LodMetric::OFF.select(&table, 0, far, 1.0), 0);
        assert_eq!(metric.select(&MeshLodTable::single(3), 0, far, 1.0), 0);
        // Orthographic: w = 1, so the error scales by half the height over the half extent.
        let ortho = moba_core::math::orthographic_rh(-100.0, 100.0, -100.0, 100.0, 0.0, 50.0);
        let metric = LodMetric::new(ortho, 512, LodConfig::with_tolerance(1.0));
        assert!((metric.pixels(Vec3::ZERO, 1.0, 1.0) - 2.56).abs() < 1e-4);
    }

    #[test]
    fn visible_chunks_merge_into_runs_without_growing_the_scratch() {
        let chunk = |first: u32, x: f32| TerrainChunk {
            first_index: first,
            index_count: 6,
            min: [x, 0.0, -1.0],
            max: [x + 1.0, 1.0, 1.0],
        };
        let chunks = [
            chunk(0, 0.0),
            chunk(6, 1.0),
            chunk(12, 500.0),
            chunk(18, 2.0),
            chunk(24, 3.0),
        ];
        let frustum = Frustum::new(camera(), Planes::All);
        let mut runs = Vec::with_capacity(4);
        assert_eq!(terrain_runs(&chunks, &frustum, &mut runs), 4);
        assert_eq!(runs, [(0, 12), (18, 12)]);
        assert_eq!(terrain_runs(&chunks, &Frustum::EVERYTHING, &mut runs), 5);
        assert_eq!(runs, [(0, 30)]);
        let mut tight = Vec::with_capacity(1);
        assert_eq!(terrain_runs(&chunks, &frustum, &mut tight), 4);
        assert_eq!(tight, [(0, 30)], "out of scratch: one run over the gap");
        assert_eq!(tight.capacity(), 1);
    }

    #[test]
    fn modes_round_trip_by_name() {
        for mode in [CullMode::Off, CullMode::Cpu] {
            assert_eq!(CullMode::from_name(mode.name()), Some(mode));
        }
        assert_eq!(CullMode::from_name(" ON "), Some(CullMode::Cpu));
        assert_eq!(CullMode::from_name("gpu"), None, "no GPU-driven path yet");
        assert_eq!(CullMode::from_name("maybe"), None);
        assert_eq!(CullMode::default(), CullMode::Cpu);
    }
}
