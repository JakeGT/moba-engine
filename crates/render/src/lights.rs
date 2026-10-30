//! Dynamic point lights (R2b, ADR-0016): the public light type and the CPU binning into a
//! ground-aligned cluster grid.
//!
//! A MOBA camera looks down at one board, so the "clusters" are columns of a 32 × 32 grid
//! laid over the camera's ground footprint: the four frustum corner rays cut by the
//! `0..`[`RECEIVER_HEIGHT`](crate::lighting::RECEIVER_HEIGHT) slab, at most
//! [`MAX_LIGHT_DISTANCE`] from the eye, padded by [`GRID_PAD`]. Each light goes into every
//! column its sphere's xz bounds touch (conservative for any height); the shader applies the
//! exact falloff. Binning writes straight into the frame's mapped buffers with no scratch
//! allocation.

use moba_core::math::{Mat4, Vec3};

use crate::lighting::RECEIVER_HEIGHT;

/// Most lights one frame can hold on any tier (the light buffer's capacity).
pub const MAX_LIGHTS: usize = 256;
/// Most lights one grid column can hold on any tier.
pub const MAX_LIGHTS_PER_CELL: usize = 16;
/// Grid columns along x and along z.
pub(crate) const GRID: usize = 32;
/// Words per column in the grid buffer: a count, then the light indices.
pub(crate) const CELL_WORDS: usize = 1 + MAX_LIGHTS_PER_CELL;
/// `vec4` words per light in the light buffer: position + radius, colour.
pub(crate) const LIGHT_VEC4S: usize = 2;
/// Farthest ground point from the eye the grid covers; the terrain fog is mostly opaque
/// past it, and a larger footprint would make the columns coarse.
pub(crate) const MAX_LIGHT_DISTANCE: f32 = 300.0;
/// Margin around the footprint, metres: tall surfaces near the edge stay inside.
pub(crate) const GRID_PAD: f32 = 4.0;
/// Largest accepted light radius, metres.
pub(crate) const MAX_LIGHT_RADIUS: f32 = 64.0;
/// Largest accepted light colour channel (intensity premultiplied).
pub(crate) const MAX_LIGHT_COLOR: f32 = 256.0;

/// One dynamic point light ([`Scene::lights`](crate::Scene::lights)).
///
/// `color` is linear RGB with the intensity premultiplied, in the same units as the sun:
/// a white Lambert surface facing the light at 1 m (and well inside `radius`) shows about
/// `albedo × color`. The light reaches zero at `radius` (a windowed inverse-square falloff).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct PointLight {
    /// World position.
    pub position: Vec3,
    /// Range in metres; the light is exactly zero at and past it.
    pub radius: f32,
    /// Linear RGB times intensity.
    pub color: Vec3,
}

impl PointLight {
    /// A light at `position` reaching `radius` metres with `color` (intensity included).
    #[must_use]
    pub const fn new(position: Vec3, radius: f32, color: Vec3) -> Self {
        Self {
            position,
            radius,
            color,
        }
    }

    /// The light if every value is usable: finite, a positive radius (clamped to 64 m) and
    /// a non-negative colour (clamped), with some light in it.
    fn sanitized(self) -> Option<Self> {
        if !self.position.is_finite()
            || !self.radius.is_finite()
            || self.radius <= 0.0
            || !self.color.is_finite()
        {
            return None;
        }
        let color = self.color.clamp(Vec3::ZERO, Vec3::splat(MAX_LIGHT_COLOR));
        (color.max_element() > 0.0).then_some(Self {
            position: self.position,
            radius: self.radius.min(MAX_LIGHT_RADIUS),
            color,
        })
    }
}

/// What happened to the most recent frame's lights ([`Renderer::light_stats`]).
///
/// [`Renderer::light_stats`]: crate::Renderer::light_stats
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct LightStats {
    /// Lights in [`Scene::lights`](crate::Scene::lights).
    pub submitted: u32,
    /// Lights uploaded (inside the grid, under the global cap).
    pub binned: u32,
    /// Lights dropped by the tier's global cap (list order).
    pub dropped: u32,
    /// Light-column pairs dropped by the per-column cap.
    pub cell_overflows: u32,
    /// Lights skipped as invalid or outside the grid.
    pub culled: u32,
}

/// The grid's placement, copied into the camera uniform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GridParams {
    /// `[min x, min z, 1 / cell x, 1 / cell z]`.
    pub(crate) grid: [f32; 4],
    /// `[columns x, columns z, words per column, lights]`; lights 0 = none.
    pub(crate) info: [u32; 4],
}

impl GridParams {
    /// No lights.
    pub(crate) const NONE: Self = Self {
        grid: [0.0, 0.0, 1.0, 1.0],
        info: [GRID as u32, GRID as u32, CELL_WORDS as u32, 0],
    };
}

/// The xz bounds `[min x, min z, max x, max z]` of what `view_proj` shows of the receiver
/// slab within [`MAX_LIGHT_DISTANCE`], padded by [`GRID_PAD`]; `None` for a degenerate
/// camera or one that sees no ground.
///
/// Rays through points along the four frustum edges (and the centre) are cut by the slab.
/// A ray at or above the horizon (a shallow camera's upper corners) is tilted just below it
/// first, so the ground the screen's sides show out to the horizon is still covered: the
/// frustum's side planes meet the ground there, not where the corner rays would.
pub(crate) fn ground_footprint(view_proj: Mat4) -> Option<[f32; 4]> {
    let inverse = view_proj.inverse();
    if !inverse.is_finite() {
        return None;
    }
    // Five points along each edge (corners shared), plus the centre.
    let mut samples = [(0.0f32, 0.0f32); 17];
    for (index, step) in [-1.0f32, -0.5, 0.0, 0.5].into_iter().enumerate() {
        samples[index] = (step, -1.0);
        samples[4 + index] = (1.0, step);
        samples[8 + index] = (-step, 1.0);
        samples[12 + index] = (-1.0, -step);
    }
    let mut rays = [(Vec3::ZERO, Vec3::ZERO, 0.0f32); 17];
    let mut sees_down = false;
    for (ray, &(x, y)) in rays.iter_mut().zip(&samples) {
        let near = inverse.project_point3(Vec3::new(x, y, 0.0));
        let far = inverse.project_point3(Vec3::new(x, y, 1.0));
        let span = far - near;
        let length = span.length();
        if !near.is_finite() || !length.is_finite() || length < 1e-6 {
            return None;
        }
        let direction = span / length;
        sees_down |= direction.y < -1e-6;
        *ray = (near, direction, length.min(MAX_LIGHT_DISTANCE));
    }
    let mut bounds = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    let mut any = false;
    for (near, direction, reach) in rays {
        let (direction, reach) = if sees_down && direction.y > -HORIZON_TILT {
            // Just below the horizon, in the ray's own heading.
            let flat = Vec3::new(direction.x, 0.0, direction.z);
            let heading = if flat.length() > 1e-6 {
                flat.normalize()
            } else {
                continue;
            };
            let horizontal = (1.0 - HORIZON_TILT * HORIZON_TILT).sqrt();
            (
                heading * horizontal - Vec3::Y * HORIZON_TILT,
                MAX_LIGHT_DISTANCE,
            )
        } else {
            (direction, reach)
        };
        let (mut enter, mut exit) = (0.0f32, reach);
        if direction.y.abs() > 1e-6 {
            let to_ground = -near.y / direction.y;
            let to_ceiling = (RECEIVER_HEIGHT - near.y) / direction.y;
            enter = enter.max(to_ground.min(to_ceiling));
            exit = exit.min(to_ground.max(to_ceiling));
        } else if !(0.0..=RECEIVER_HEIGHT).contains(&near.y) {
            continue;
        }
        if !enter.is_finite() || !exit.is_finite() || enter > exit {
            continue;
        }
        for t in [enter, exit] {
            bounds = include(bounds, near + direction * t);
            any = true;
        }
    }
    any.then(|| {
        [
            bounds[0] - GRID_PAD,
            bounds[1] - GRID_PAD,
            bounds[2] + GRID_PAD,
            bounds[3] + GRID_PAD,
        ]
    })
}

/// How far below the horizon a ray at or above it is tilted ([`ground_footprint`]): the
/// sine of about 1.1°.
const HORIZON_TILT: f32 = 0.02;

fn include(bounds: [f32; 4], point: Vec3) -> [f32; 4] {
    [
        bounds[0].min(point.x),
        bounds[1].min(point.z),
        bounds[2].max(point.x),
        bounds[3].max(point.z),
    ]
}

/// Per-column light counts of one binning, kept in CPU memory (see [`bin`]).
pub(crate) type CellCounts = [u8; GRID * GRID];

/// Bins `lights` over `footprint` into `light_words` (two `vec4` per light) and
/// `cell_words` ([`GRID`]² columns of [`CELL_WORDS`]), keeping at most `max_lights` lights
/// and `per_cell` per column, in list order. `light_words` and `cell_words` are the frame's
/// mapped buffers; only the column counts and the entries below them are written, which is
/// all the shader reads.
///
/// The mapped buffers are **only written**, never read: host-visible device memory is often
/// write-combined (or BAR memory over the PCI Express bus), where every read is an uncached round trip.
/// The running per-column counts live in `counts` (CPU scratch, reset here) and are written
/// to the count words once at the end.
pub(crate) fn bin(
    lights: &[PointLight],
    footprint: Option<[f32; 4]>,
    max_lights: usize,
    per_cell: usize,
    light_words: &mut [[f32; 4]],
    cell_words: &mut [u32],
    counts: &mut CellCounts,
) -> (GridParams, LightStats) {
    let mut stats = LightStats {
        submitted: u32::try_from(lights.len()).unwrap_or(u32::MAX),
        ..LightStats::default()
    };
    let capacity = (light_words.len() / LIGHT_VEC4S).min(MAX_LIGHTS);
    let max_lights = max_lights.min(capacity);
    let per_cell = per_cell.min(MAX_LIGHTS_PER_CELL);
    let columns = (cell_words.len() / CELL_WORDS).min(GRID * GRID);
    let Some(footprint) =
        footprint.filter(|_| max_lights > 0 && per_cell > 0 && columns == GRID * GRID)
    else {
        stats.culled = stats.submitted;
        return (GridParams::NONE, stats);
    };
    let [min_x, min_z, max_x, max_z] = footprint;
    let cell_x = ((max_x - min_x) / GRID as f32).max(0.25);
    let cell_z = ((max_z - min_z) / GRID as f32).max(0.25);
    let (inverse_x, inverse_z) = (1.0 / cell_x, 1.0 / cell_z);
    counts.fill(0);
    let column_of = |value: f32, min: f32, inverse: f32| -> usize {
        // Saturating float-to-int casts clamp below; `min` clamps above.
        (((value - min) * inverse).floor().max(0.0) as usize).min(GRID - 1)
    };
    let mut count = 0usize;
    for light in lights {
        let Some(light) = light.sanitized() else {
            stats.culled += 1;
            continue;
        };
        let p = light.position;
        let r = light.radius;
        if p.x + r < min_x || p.x - r > max_x || p.z + r < min_z || p.z - r > max_z {
            stats.culled += 1;
            continue;
        }
        if count >= max_lights {
            stats.dropped += 1;
            continue;
        }
        let index = u32::try_from(count).unwrap_or(u32::MAX);
        light_words[count * LIGHT_VEC4S] = [p.x, p.y, p.z, r];
        light_words[count * LIGHT_VEC4S + 1] = [light.color.x, light.color.y, light.color.z, 0.0];
        count += 1;
        let (x0, x1) = (
            column_of(p.x - r, min_x, inverse_x),
            column_of(p.x + r, min_x, inverse_x),
        );
        let (z0, z1) = (
            column_of(p.z - r, min_z, inverse_z),
            column_of(p.z + r, min_z, inverse_z),
        );
        for z in z0..=z1 {
            for x in x0..=x1 {
                let column = z * GRID + x;
                let used = usize::from(counts[column]);
                if used >= per_cell {
                    stats.cell_overflows += 1;
                    continue;
                }
                cell_words[column * CELL_WORDS + 1 + used] = index;
                // `used < per_cell <= MAX_LIGHTS_PER_CELL` (16), so this fits a byte.
                counts[column] = (used + 1) as u8;
            }
        }
    }
    for (column, &used) in counts.iter().enumerate() {
        cell_words[column * CELL_WORDS] = u32::from(used);
    }
    stats.binned = u32::try_from(count).unwrap_or(u32::MAX);
    let params = GridParams {
        grid: [min_x, min_z, inverse_x, inverse_z],
        info: [GRID as u32, GRID as u32, CELL_WORDS as u32, stats.binned],
    };
    (params, stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use moba_core::math::{look_at_rh, perspective_rh};

    fn player_camera() -> Mat4 {
        let eye = Vec3::new(0.0, 40.0, 23.0);
        perspective_rh(1.05, 16.0 / 9.0, 0.1, 1000.0) * look_at_rh(eye, Vec3::ZERO, Vec3::Y)
    }

    fn buffers() -> (Vec<[f32; 4]>, Vec<u32>) {
        (
            vec![[0.0; 4]; MAX_LIGHTS * LIGHT_VEC4S],
            vec![u32::MAX; GRID * GRID * CELL_WORDS],
        )
    }

    fn column(params: &GridParams, x: f32, z: f32) -> usize {
        let cx = (((x - params.grid[0]) * params.grid[2]).floor() as usize).min(GRID - 1);
        let cz = (((z - params.grid[1]) * params.grid[3]).floor() as usize).min(GRID - 1);
        cz * GRID + cx
    }

    #[test]
    fn the_footprint_covers_the_ground_the_camera_sees() {
        let footprint = ground_footprint(player_camera()).expect("sees the ground");
        let [min_x, min_z, max_x, max_z] = footprint;
        assert!(min_x < -10.0 && max_x > 10.0 && min_z < -10.0 && max_z > 10.0);
        // Capped by the light distance, padded.
        assert!(max_x - min_x <= 2.0 * (MAX_LIGHT_DISTANCE + GRID_PAD));
        // Looking straight up sees no ground.
        let sky = perspective_rh(1.0, 1.0, 0.1, 100.0)
            * look_at_rh(
                Vec3::new(0.0, 20.0, 0.0),
                Vec3::new(0.0, 40.0, 0.1),
                Vec3::Y,
            );
        assert!(ground_footprint(sky).is_none());
    }

    #[test]
    fn the_default_camera_grid_cells_are_a_few_metres() {
        // The 46 m, 0.92 rad player camera (`Camera::orbit`, 60 degree fov) at 16:9.
        let camera = crate::Camera::orbit(Vec3::ZERO, 46.0, -0.7, 0.92, 16.0 / 9.0);
        let [min_x, min_z, max_x, max_z] =
            ground_footprint(camera.view_proj).expect("sees the ground");
        let (cell_x, cell_z) = ((max_x - min_x) / GRID as f32, (max_z - min_z) / GRID as f32);
        eprintln!("default camera cells {cell_x:.2} x {cell_z:.2} m");
        assert!((1.0..6.0).contains(&cell_x) && (1.0..6.0).contains(&cell_z));
    }

    #[test]
    fn a_shallow_camera_covers_the_ground_out_to_its_horizon() {
        // Eye 15 m up, pitched about 3 degrees down: the upper corners look at the sky, but
        // the screen's sides show ground out to about +-100 m at the horizon.
        let eye = Vec3::new(0.0, 15.0, 60.0);
        let target = eye + Vec3::new(0.0, -(3.0f32.to_radians()).tan(), -1.0);
        let camera =
            perspective_rh(1.05, 16.0 / 9.0, 0.1, 1000.0) * look_at_rh(eye, target, Vec3::Y);
        let [min_x, min_z, max_x, max_z] = ground_footprint(camera).expect("sees the ground");
        assert!(min_x < -90.0 && max_x > 90.0, "{min_x} {max_x}");
        // Still bounded by the light distance from the eye (padded).
        assert!(max_x - min_x <= 2.0 * (MAX_LIGHT_DISTANCE + GRID_PAD));
        assert!(
            min_z >= eye.z - MAX_LIGHT_DISTANCE - GRID_PAD - 1.0 && max_z <= eye.z + GRID_PAD + 1.0,
            "{min_z} {max_z}"
        );
    }

    #[test]
    fn lights_land_in_every_column_they_touch_and_nowhere_else() {
        let (mut lights_buf, mut cells) = buffers();
        let footprint = ground_footprint(player_camera());
        let lights = [
            PointLight::new(Vec3::new(0.0, 1.0, 0.0), 3.0, Vec3::new(4.0, 2.0, 1.0)),
            PointLight::new(Vec3::new(12.0, 1.0, -6.0), 1.0, Vec3::ONE),
        ];
        let (params, stats) = bin(
            &lights,
            footprint,
            64,
            8,
            &mut lights_buf,
            &mut cells,
            &mut [0; GRID * GRID],
        );
        assert_eq!((stats.binned, stats.dropped, stats.culled), (2, 0, 0));
        assert_eq!(params.info[3], 2);
        assert_eq!(lights_buf[0], [0.0, 1.0, 0.0, 3.0]);
        assert_eq!(lights_buf[3], [1.0, 1.0, 1.0, 0.0]);
        let centre = column(&params, 0.0, 0.0) * CELL_WORDS;
        assert_eq!(cells[centre], 1);
        assert_eq!(cells[centre + 1], 0);
        let edge = column(&params, 2.9, 0.0) * CELL_WORDS;
        assert_eq!(&cells[edge..edge + 2], &[1, 0]);
        let second = column(&params, 12.0, -6.0) * CELL_WORDS;
        assert_eq!(&cells[second..second + 2], &[1, 1]);
        let far = column(&params, -20.0, 15.0) * CELL_WORDS;
        assert_eq!(cells[far], 0, "untouched columns are cleared");
    }

    #[test]
    fn caps_drop_in_list_order_and_invalid_lights_are_skipped() {
        let (mut lights_buf, mut cells) = buffers();
        let footprint = ground_footprint(player_camera());
        let mut lights = vec![PointLight::new(Vec3::ZERO, 2.0, Vec3::ONE); 6];
        lights.push(PointLight::new(Vec3::splat(f32::NAN), 2.0, Vec3::ONE));
        lights.push(PointLight::new(Vec3::new(5000.0, 0.0, 0.0), 2.0, Vec3::ONE));
        lights.push(PointLight::new(Vec3::ZERO, 2.0, Vec3::ZERO));
        let (params, stats) = bin(
            &lights,
            footprint,
            4,
            3,
            &mut lights_buf,
            &mut cells,
            &mut [0; GRID * GRID],
        );
        assert_eq!(stats.submitted, 9);
        assert_eq!(stats.binned, 4, "the global cap keeps the first four");
        assert_eq!(stats.dropped, 2);
        assert_eq!(stats.culled, 3, "NaN, far away and black");
        let centre = column(&params, 0.0, 0.0) * CELL_WORDS;
        assert_eq!(
            &cells[centre..centre + 4],
            &[3, 0, 1, 2],
            "per-column cap, list order"
        );
        assert!(stats.cell_overflows > 0);
    }

    #[test]
    fn no_footprint_or_a_zero_cap_uploads_nothing() {
        let (mut lights_buf, mut cells) = buffers();
        let lights = [PointLight::new(Vec3::ZERO, 2.0, Vec3::ONE)];
        let (params, stats) = bin(
            &lights,
            None,
            64,
            8,
            &mut lights_buf,
            &mut cells,
            &mut [0; GRID * GRID],
        );
        assert_eq!((params.info[3], stats.binned, stats.culled), (0, 0, 1));
        let footprint = ground_footprint(player_camera());
        let (params, _) = bin(
            &lights,
            footprint,
            0,
            8,
            &mut lights_buf,
            &mut cells,
            &mut [0; GRID * GRID],
        );
        assert_eq!(params, GridParams::NONE);
        // Huge radii are clamped.
        let big = [PointLight::new(Vec3::ZERO, 1.0e9, Vec3::splat(1.0e9))];
        let (_, stats) = bin(
            &big,
            footprint,
            64,
            8,
            &mut lights_buf,
            &mut cells,
            &mut [0; GRID * GRID],
        );
        assert_eq!(stats.binned, 1);
        assert_eq!(lights_buf[0][3], MAX_LIGHT_RADIUS);
        assert_eq!(lights_buf[1][0], MAX_LIGHT_COLOR);
    }
}
