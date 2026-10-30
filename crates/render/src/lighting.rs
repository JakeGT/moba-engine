//! Lighting (R2a, ADR-0011): tiered shadow and IBL settings, per-frame art-direction
//! parameters, the directional shadow-map fit and the split-sum BRDF LUT.
//!
//! The pure parts (configuration, [`fit_shadow`], [`integrate_brdf`]) are unit-tested headless;
//! [`LightingResources`] owns the scene lighting descriptor set (set 3 of every scene
//! pipeline): the shadow map with a comparison and a plain sampler, the prefiltered specular
//! environment cube, the BRDF LUT and the stylisation ramp. The set is written at creation, on
//! resize (the shadow map is a render-graph transient) and when an environment or ramp is set
//! (both load time, after a device-idle wait), never per frame. A second set with the same
//! layout serves the offscreen views (R7b portraits and minimap): its shadow binding is a
//! persistent default image, so those passes, which run before the shadow pass, never touch
//! the shadow transient, and their camera uniform turns shadows off
//! ([`FrameLighting::offscreen`]).
//!
//! R2b (ADR-0016) adds the dynamic-light buffers (bindings 5 and 6, host-written per frame
//! in flight, so both sets exist once per frame), the baked AO map (7) and the main view's
//! contact-shadow target (8; the offscreen set names a white default), plus the tier
//! switches for lights, outlines, baked AO and contact shadows.

use ash::vk;
use moba_core::math::{Mat4, Vec3, Vec4, look_at_rh};

use crate::device::DeviceContext;
use crate::error::{RenderError, RenderResult};
use crate::quality::QualityTier;
use crate::resources::{DescriptorPool, DescriptorSetLayout, GpuBuffer, GpuImage, Sampler};
use crate::texture;

/// How the directional shadow map is filtered (ADR-0011).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ShadowFilter {
    /// No shadow pass: the scene is lit without shadows.
    Off,
    /// One hardware 2×2 PCF tap (the `Low` tier).
    Hard,
    /// 3×3 hardware PCF taps: a 4×4-texel tent (`Medium`).
    #[default]
    Pcf,
    /// PCSS-lite: a 16-tap blocker search and a penumbra-scaled PCF disk (`High`, `Ultra`).
    Pcss,
}

impl ShadowFilter {
    /// Every filter, cheapest first.
    pub const ALL: &'static [Self] = &[Self::Off, Self::Hard, Self::Pcf, Self::Pcss];

    /// Lower-case name, as accepted by [`Self::from_name`].
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Hard => "hard",
            Self::Pcf => "pcf",
            Self::Pcss => "pcss",
        }
    }

    /// Parses a filter name, ignoring ASCII case and surrounding whitespace.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        Self::ALL
            .iter()
            .copied()
            .find(|filter| filter.name().eq_ignore_ascii_case(name))
    }

    /// The shader's `shadow_params.x` value (`MOBA_SHADOW_*` in `camera.glsl`).
    const fn shader_mode(self) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::Hard => 1.0,
            Self::Pcf => 2.0,
            Self::Pcss => 3.0,
        }
    }
}

impl core::fmt::Display for ShadowFilter {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

/// How team and selection outlines are drawn (ADR-0016).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OutlineMode {
    /// No outlines.
    Off,
    /// Inverted hulls in the scene pass (`Low`, `Medium`).
    #[default]
    Hull,
    /// A screen-space pass over an ID target from the depth pre-pass (`High`, `Ultra`).
    Screen,
}

impl OutlineMode {
    /// Every mode.
    pub const ALL: &'static [Self] = &[Self::Off, Self::Hull, Self::Screen];

    /// Lower-case name, as accepted by [`Self::from_name`].
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Hull => "hull",
            Self::Screen => "screen",
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

impl core::fmt::Display for OutlineMode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

/// Most contact-shadow ray steps.
pub const MAX_CONTACT_SHADOW_STEPS: u32 = 32;
/// Contact-shadow ray length towards the sun, metres.
const CONTACT_RAY_LENGTH: f32 = 0.8;
/// Thickness assumed behind each depth-buffer surface, metres.
const CONTACT_THICKNESS: f32 = 0.6;
/// Distance from the eye at which contact shadows have faded out, metres.
const CONTACT_FADE_END: f32 = 120.0;

/// Smallest and largest accepted shadow-map edge.
pub const MIN_SHADOW_RESOLUTION: u32 = 256;
/// Largest accepted shadow-map edge.
pub const MAX_SHADOW_RESOLUTION: u32 = 8192;

/// Renderer-lifetime lighting features (startup only; see [`RenderConfig::lighting`]).
///
/// [`LightingConfig::for_tier`] gives each [`QualityTier`]'s defaults; the `with_*` methods
/// are the off switches and overrides (the client exposes them as `MOBA_SHADOWS`,
/// `MOBA_SHADOW_SIZE` and `MOBA_IBL`).
///
/// [`RenderConfig::lighting`]: crate::RenderConfig::lighting
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LightingConfig {
    /// Shadow filter; [`ShadowFilter::Off`] removes the shadow pass from the frame graph.
    pub shadows: ShadowFilter,
    /// Shadow-map edge in texels, clamped to
    /// [`MIN_SHADOW_RESOLUTION`]`..=`[`MAX_SHADOW_RESOLUTION`] and the device limit.
    pub shadow_resolution: u32,
    /// Sample the prefiltered specular environment cube. Without it (`Low`) specular ambient
    /// comes from the SH irradiance in the reflection direction, saving a cube fetch.
    pub specular_ibl: bool,
    /// Use image-based lighting when an environment is set. `false` keeps the hemispheric
    /// ambient everywhere (the off switch).
    pub ibl: bool,
    /// PCSS-lite PCF taps (the blocker search is always 16), clamped to 4..=64: 16 on `High`,
    /// 32 on `Ultra`. Ignored by the other filters.
    pub pcss_taps: u32,
    /// Dynamic point lights uploaded per frame (ADR-0016), at most
    /// [`MAX_LIGHTS`](crate::MAX_LIGHTS); `0` turns dynamic lights off.
    pub max_lights: u32,
    /// Lights per grid column, at most [`MAX_LIGHTS_PER_CELL`](crate::MAX_LIGHTS_PER_CELL).
    pub lights_per_cell: u32,
    /// How outlines are drawn.
    pub outlines: OutlineMode,
    /// Apply a map's baked ambient occlusion ([`Renderer::set_baked_ao`](crate::Renderer)).
    pub baked_ao: bool,
    /// Screen-space contact-shadow ray steps (`0` = off, at most
    /// [`MAX_CONTACT_SHADOW_STEPS`]). On, they add the depth pre-pass.
    pub contact_shadow_steps: u32,
}

impl LightingConfig {
    /// The defaults of `tier` (ADR-0011, ADR-0016):
    ///
    /// | Tier | Shadows | Map | Specular IBL | Lights (global / column) | Outlines | Contact |
    /// |---|---|---|---|---|---|---|
    /// | `Low` | hard (1 tap) | 1024² | off (SH only) | 16 / 8 | hull | off |
    /// | `Medium` | 3×3 PCF | 2048² | on | 64 / 12 | hull | off |
    /// | `High` | PCSS-lite, 16 + 16 taps | 2048² | on | 128 / 16 | screen | 8 steps |
    /// | `Ultra` | PCSS-lite, 16 + 32 taps | 4096² | on | 256 / 16 | screen | 16 steps |
    ///
    /// Baked AO is on for every tier.
    #[must_use]
    pub const fn for_tier(tier: QualityTier) -> Self {
        let (shadows, shadow_resolution, specular_ibl, pcss_taps) = match tier {
            QualityTier::Low => (ShadowFilter::Hard, 1024, false, 16),
            QualityTier::Medium => (ShadowFilter::Pcf, 2048, true, 16),
            QualityTier::High => (ShadowFilter::Pcss, 2048, true, 16),
            QualityTier::Ultra => (ShadowFilter::Pcss, 4096, true, 32),
        };
        let (max_lights, lights_per_cell, outlines, contact_shadow_steps) = match tier {
            // Per-column caps: a column is about 4.6 m at the default camera, so a teamfight's
            // overlapping effects put several lights in one (perf/0020).
            QualityTier::Low => (16, 8, OutlineMode::Hull, 0),
            QualityTier::Medium => (64, 12, OutlineMode::Hull, 0),
            QualityTier::High => (128, 16, OutlineMode::Screen, 8),
            QualityTier::Ultra => (256, 16, OutlineMode::Screen, 16),
        };
        Self {
            shadows,
            shadow_resolution,
            specular_ibl,
            ibl: true,
            pcss_taps,
            max_lights,
            lights_per_cell,
            outlines,
            baked_ao: true,
            contact_shadow_steps,
        }
    }

    /// Returns this config with [`LightingConfig::max_lights`] set (`0` = no dynamic lights).
    #[must_use]
    pub const fn with_max_lights(mut self, max_lights: u32) -> Self {
        self.max_lights = max_lights;
        self
    }

    /// Returns this config with [`LightingConfig::lights_per_cell`] set.
    #[must_use]
    pub const fn with_lights_per_cell(mut self, lights: u32) -> Self {
        self.lights_per_cell = lights;
        self
    }

    /// Returns this config with [`LightingConfig::outlines`] set.
    #[must_use]
    pub const fn with_outlines(mut self, outlines: OutlineMode) -> Self {
        self.outlines = outlines;
        self
    }

    /// Returns this config with [`LightingConfig::baked_ao`] set.
    #[must_use]
    pub const fn with_baked_ao(mut self, baked_ao: bool) -> Self {
        self.baked_ao = baked_ao;
        self
    }

    /// Returns this config with [`LightingConfig::contact_shadow_steps`] set (`0` = off).
    #[must_use]
    pub const fn with_contact_shadow_steps(mut self, steps: u32) -> Self {
        self.contact_shadow_steps = steps;
        self
    }

    /// The global light cap actually used.
    pub(crate) fn light_cap(self) -> usize {
        (self.max_lights as usize).min(crate::lights::MAX_LIGHTS)
    }

    /// The per-column light cap actually used.
    pub(crate) fn cell_cap(self) -> usize {
        (self.lights_per_cell as usize).min(crate::lights::MAX_LIGHTS_PER_CELL)
    }

    /// Contact-shadow steps actually used.
    pub(crate) fn contact_steps(self) -> u32 {
        self.contact_shadow_steps.min(MAX_CONTACT_SHADOW_STEPS)
    }

    /// Returns this config with [`LightingConfig::pcss_taps`] set.
    #[must_use]
    pub const fn with_pcss_taps(mut self, taps: u32) -> Self {
        self.pcss_taps = taps;
        self
    }

    /// Returns this config with [`LightingConfig::shadows`] set.
    #[must_use]
    pub const fn with_shadows(mut self, shadows: ShadowFilter) -> Self {
        self.shadows = shadows;
        self
    }

    /// Returns this config with [`LightingConfig::shadow_resolution`] set.
    #[must_use]
    pub const fn with_shadow_resolution(mut self, resolution: u32) -> Self {
        self.shadow_resolution = resolution;
        self
    }

    /// Returns this config with [`LightingConfig::specular_ibl`] set.
    #[must_use]
    pub const fn with_specular_ibl(mut self, specular_ibl: bool) -> Self {
        self.specular_ibl = specular_ibl;
        self
    }

    /// Returns this config with [`LightingConfig::ibl`] set.
    #[must_use]
    pub const fn with_ibl(mut self, ibl: bool) -> Self {
        self.ibl = ibl;
        self
    }

    /// The resolution actually used: a power of two in range, at most `device_limit`.
    pub(crate) fn clamped_resolution(self, device_limit: u32) -> u32 {
        let high = MAX_SHADOW_RESOLUTION.min(device_limit.max(MIN_SHADOW_RESOLUTION));
        let wanted = self
            .shadow_resolution
            .clamp(MIN_SHADOW_RESOLUTION, high)
            .next_power_of_two();
        if wanted > high { wanted / 2 } else { wanted }
    }

    /// PCF/PCSS taps, blocker-search radius and maximum penumbra radius (texels). The PCSS
    /// tap count is the tier's ([`Self::pcss_taps`]); the radii follow it, so a 32-tap disk
    /// may spread wider without banding.
    fn filter_taps(self) -> (f32, f32, f32) {
        match self.shadows {
            ShadowFilter::Off | ShadowFilter::Hard => (1.0, 0.0, 1.0),
            ShadowFilter::Pcf => (9.0, 0.0, 1.0),
            ShadowFilter::Pcss => {
                let taps = self.pcss_taps.clamp(4, 64);
                let radius = if taps >= 32 { 16.0 } else { 10.0 };
                (taps as f32, radius, radius)
            }
        }
    }
}

impl Default for LightingConfig {
    fn default() -> Self {
        Self::for_tier(QualityTier::default())
    }
}

/// Per-frame lighting art direction ([`Scene::lighting`](crate::Scene)). The key light's
/// direction and intensity and the hemispheric sky ambient stay on [`Camera`](crate::Camera)
/// (`light_direction`, `light_intensity`, `ambient`); this holds the rest (ADR-0011). Maps set
/// it from their visuals descriptor's `[lighting]` table.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct SceneLighting {
    /// Linear RGB colour of the key light (times `Camera::light_intensity`).
    pub sun_color: Vec3,
    /// Hemispheric ground ambient, used without an environment (the sky half is
    /// `Camera::ambient`).
    pub ambient_ground: Vec3,
    /// How much shadow removes direct light, `0` (no shadows) to at most
    /// [`MAX_SHADOW_STRENGTH`](moba_assets::MAX_SHADOW_STRENGTH) (0.9; larger values are
    /// clamped), so units in shadow keep a floor of direct light (ADR-0011 rule 2).
    pub shadow_strength: f32,
    /// Sun angular radius in radians; scales the PCSS penumbra.
    pub shadow_softness: f32,
    /// Environment intensity multiplier.
    pub ibl_intensity: f32,
    /// Wrap lighting (`0` = Lambert, `1` = fully wrapped).
    pub wrap: f32,
    /// Rim strength.
    pub rim: f32,
    /// Rim colour.
    pub rim_color: Vec3,
    /// Strength of the ramp texture ([`Renderer::set_lighting_ramp`](crate::Renderer)).
    pub ramp: f32,
    /// Painted-specular tint.
    pub specular_tint: Vec3,
    /// Specular scale.
    pub specular: f32,
    /// Roughness of units without a material (vertex colour).
    pub unit_roughness: f32,
    /// Terrain roughness.
    pub terrain_roughness: f32,
}

impl SceneLighting {
    /// The default look: white sun, the pre-R2 ground ambient ratio, strong but not black
    /// shadows, a little wrap and a white rim.
    pub const DEFAULT: Self = Self {
        sun_color: Vec3::new(1.0, 0.97, 0.92),
        ambient_ground: Vec3::new(0.176, 0.21, 0.294),
        shadow_strength: 0.78,
        shadow_softness: 0.03,
        ibl_intensity: 1.0,
        wrap: 0.2,
        rim: 0.25,
        rim_color: Vec3::ONE,
        ramp: 0.0,
        specular_tint: Vec3::ONE,
        specular: 1.0,
        unit_roughness: moba_assets::DEFAULT_UNIT_ROUGHNESS,
        terrain_roughness: moba_assets::DEFAULT_TERRAIN_ROUGHNESS,
    };

    /// Returns this lighting with [`SceneLighting::sun_color`] set.
    #[must_use]
    pub const fn with_sun_color(mut self, color: Vec3) -> Self {
        self.sun_color = color;
        self
    }

    /// Returns this lighting with [`SceneLighting::ambient_ground`] set.
    #[must_use]
    pub const fn with_ambient_ground(mut self, color: Vec3) -> Self {
        self.ambient_ground = color;
        self
    }

    /// Returns this lighting with [`SceneLighting::shadow_strength`] set.
    #[must_use]
    pub const fn with_shadow_strength(mut self, strength: f32) -> Self {
        self.shadow_strength = strength;
        self
    }

    /// Returns this lighting with [`SceneLighting::shadow_softness`] set.
    #[must_use]
    pub const fn with_shadow_softness(mut self, softness: f32) -> Self {
        self.shadow_softness = softness;
        self
    }

    /// Returns this lighting with [`SceneLighting::ibl_intensity`] set.
    #[must_use]
    pub const fn with_ibl_intensity(mut self, intensity: f32) -> Self {
        self.ibl_intensity = intensity;
        self
    }

    /// Returns this lighting with [`SceneLighting::wrap`] set.
    #[must_use]
    pub const fn with_wrap(mut self, wrap: f32) -> Self {
        self.wrap = wrap;
        self
    }

    /// Returns this lighting with the rim strength and colour set.
    #[must_use]
    pub const fn with_rim(mut self, strength: f32, color: Vec3) -> Self {
        self.rim = strength;
        self.rim_color = color;
        self
    }

    /// Returns this lighting with [`SceneLighting::ramp`] set.
    #[must_use]
    pub const fn with_ramp(mut self, strength: f32) -> Self {
        self.ramp = strength;
        self
    }

    /// Returns this lighting with the painted-specular tint and scale set.
    #[must_use]
    pub const fn with_specular(mut self, tint: Vec3, scale: f32) -> Self {
        self.specular_tint = tint;
        self.specular = scale;
        self
    }

    /// Returns this lighting with the vertex-colour unit and terrain roughness set.
    #[must_use]
    pub const fn with_roughness(mut self, unit: f32, terrain: f32) -> Self {
        self.unit_roughness = unit;
        self.terrain_roughness = terrain;
        self
    }
}

impl Default for SceneLighting {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Clamps to `low..=high`, mapping NaN to `low` (uniform inputs are untrusted map data).
pub(crate) fn sanitize(value: f32, low: f32, high: f32) -> f32 {
    if value.is_finite() {
        value.clamp(low, high)
    } else {
        low
    }
}

fn sanitize_rgb(color: Vec3, high: f32) -> [f32; 3] {
    [
        sanitize(color.x, 0.0, high),
        sanitize(color.y, 0.0, high),
        sanitize(color.z, 0.0, high),
    ]
}

/// Tallest shadow caster the fit plans for (walls and props are at most 32 m): the map is
/// pushed back along the light far enough to hold one this tall above any receiver.
pub(crate) const CASTER_HEIGHT: f32 = 32.0;
/// Height of the receiver slab the camera's view is cut by: the ground plus unit bodies
/// (heroes ~5 m, towers 9 m). Taller surfaces near the eye may fall outside the map and are
/// then lit; using [`CASTER_HEIGHT`] here would make the player camera's map 2.5× coarser.
pub(crate) const RECEIVER_HEIGHT: f32 = 10.0;
/// Farthest receiver from the eye that gets shadows; the terrain fog is opaque by 520 m.
pub(crate) const MAX_SHADOW_DISTANCE: f32 = 520.0;
/// The fitted radius is rounded up to this step, so small zoom changes do not resize texels.
const RADIUS_STEP: f32 = 2.0;

/// A fitted directional shadow map for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ShadowFit {
    /// World → shadow clip space (xy −1..1, z 0..1), texel-snapped.
    pub(crate) view_proj: Mat4,
    /// World-space size of one shadow texel.
    pub(crate) texel_world: f32,
    /// World distance covered by the depth range (z 0..1).
    pub(crate) depth_range: f32,
}

/// Fits one orthographic shadow map around the camera's view of the board (ADR-0011).
///
/// The four frustum corner rays are cut to the receiver slab (`0 <= y <=`
/// [`RECEIVER_HEIGHT`]), each at most [`MAX_SHADOW_DISTANCE`] from the eye; the map covers
/// the bounding sphere of those points (a sphere, so rotating the camera never resizes it),
/// its radius rounded up to a 2 m step, and is pushed back along the light far enough to hold
/// every caster up to [`CASTER_HEIGHT`] above a receiver. The projection is then snapped to whole texels so a
/// panning camera does not make shadow edges shimmer. `None` for a degenerate camera.
pub(crate) fn fit_shadow(
    view_proj: Mat4,
    light_direction: Vec3,
    resolution: u32,
) -> Option<ShadowFit> {
    let inverse = view_proj.inverse();
    if !inverse.is_finite() {
        return None;
    }
    let light = light_direction.try_normalize()?;
    let mut points = [Vec3::ZERO; 8];
    let mut count = 0;
    for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let near = inverse.project_point3(Vec3::new(x, y, 0.0));
        let far = inverse.project_point3(Vec3::new(x, y, 1.0));
        let ray = far - near;
        let length = ray.length();
        if !near.is_finite() || !length.is_finite() || length < 1e-6 {
            return None;
        }
        let t_max = (MAX_SHADOW_DISTANCE / length).min(1.0);
        // The part of the ray inside the slab 0 <= y <= RECEIVER_HEIGHT, within t_max.
        let (mut enter, mut exit) = (0.0f32, t_max);
        if ray.y.abs() > 1e-6 {
            let to_ground = -near.y / ray.y;
            let to_ceiling = (RECEIVER_HEIGHT - near.y) / ray.y;
            enter = enter.max(to_ground.min(to_ceiling));
            exit = exit.min(to_ground.max(to_ceiling));
        } else if !(0.0..=RECEIVER_HEIGHT).contains(&near.y) {
            continue;
        }
        if !enter.is_finite() || !exit.is_finite() || enter > exit {
            continue; // The ray never crosses the slab (looking at the sky).
        }
        for t in [enter, exit] {
            let mut point = near + ray * t;
            point.y = point.y.clamp(0.0, RECEIVER_HEIGHT);
            points[count] = point;
            count += 1;
        }
    }
    if count == 0 {
        return None;
    }
    let points = &points[..count];
    let center = points.iter().copied().sum::<Vec3>() / count as f32;
    let radius = points
        .iter()
        .map(|point| point.distance(center))
        .fold(1.0f32, f32::max);
    let radius = (radius / RADIUS_STEP).ceil() * RADIUS_STEP;
    // Every caster up to the ceiling above a receiver lies at most this far towards the light.
    // Casters lower than the sun's direction allows are clipped only below y = 0.05 (maps
    // require y >= 0.2); with depthClamp (pancaking) they still write the near plane.
    let lift = CASTER_HEIGHT / light.y.max(0.05);
    let eye = center + light * (radius + lift);
    let up = if light.y.abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let view = look_at_rh(eye, center, up);
    let depth_range = 2.0 * radius + lift;
    let projection = orthographic(radius, depth_range);
    let unsnapped = projection * view;
    // Snap the world origin to a texel centre in light space.
    let texel_ndc = 2.0 / resolution.max(1) as f32;
    let origin = unsnapped * Vec4::new(0.0, 0.0, 0.0, 1.0);
    let snapped_x = (origin.x / texel_ndc).round() * texel_ndc;
    let snapped_y = (origin.y / texel_ndc).round() * texel_ndc;
    let snap = Mat4::from_translation(Vec3::new(snapped_x - origin.x, snapped_y - origin.y, 0.0));
    let view_proj = snap * unsnapped;
    view_proj.is_finite().then_some(ShadowFit {
        view_proj,
        texel_world: 2.0 * radius / resolution.max(1) as f32,
        depth_range,
    })
}

/// Right-handed orthographic projection of the square `-radius..radius` and depths
/// `0..far` in front of the eye, to Vulkan clip space (z in `[0, 1]`).
fn orthographic(radius: f32, far: f32) -> Mat4 {
    Mat4::from_cols(
        Vec4::new(1.0 / radius, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0 / radius, 0.0, 0.0),
        Vec4::new(0.0, 0.0, -1.0 / far, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 1.0),
    )
}

/// Edge of the split-sum BRDF LUT (`R16G16_SFLOAT`, 4 KiB).
pub(crate) const BRDF_LUT_SIZE: u32 = 32;
const BRDF_LUT_SAMPLES: u32 = 128;

fn radical_inverse(bits: u32) -> f32 {
    bits.reverse_bits() as f32 * (1.0 / 4_294_967_296.0)
}

/// Height-correlated Smith-GGX visibility, as in `lighting.glsl`.
fn visibility_smith_ggx(n_dot_v: f32, n_dot_l: f32, alpha: f32) -> f32 {
    let alpha2 = alpha * alpha;
    let ggx_v = n_dot_l * (n_dot_v * n_dot_v * (1.0 - alpha2) + alpha2).sqrt();
    let ggx_l = n_dot_v * (n_dot_l * n_dot_l * (1.0 - alpha2) + alpha2).sqrt();
    0.5 / (ggx_v + ggx_l).max(1e-5)
}

/// Split-sum environment BRDF (Karis 2013, height-correlated visibility as in Filament):
/// `(scale, bias)` with `specular = prefiltered * (F0 * scale + bias)`.
pub(crate) fn integrate_brdf(n_dot_v: f32, roughness: f32, samples: u32) -> (f32, f32) {
    let n_dot_v = n_dot_v.clamp(1e-4, 1.0);
    let alpha = (roughness * roughness).max(1e-4);
    // View vector in tangent space (N = +Z).
    let view = Vec3::new((1.0 - n_dot_v * n_dot_v).sqrt(), 0.0, n_dot_v);
    let (mut scale, mut bias) = (0.0f32, 0.0f32);
    for index in 0..samples {
        let u = (index as f32 + 0.5) / samples as f32;
        let v = radical_inverse(index);
        // GGX importance sample of the half vector.
        let phi = 2.0 * core::f32::consts::PI * u;
        let cos_theta = ((1.0 - v) / (1.0 + (alpha * alpha - 1.0) * v)).sqrt();
        let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
        let half = Vec3::new(sin_theta * phi.cos(), sin_theta * phi.sin(), cos_theta);
        let light = half * (2.0 * view.dot(half)) - view;
        let n_dot_l = light.z;
        if n_dot_l <= 0.0 {
            continue;
        }
        let n_dot_h = half.z.max(0.0);
        let v_dot_h = view.dot(half).max(0.0);
        let g = visibility_smith_ggx(n_dot_v, n_dot_l, alpha) * 4.0 * n_dot_l * v_dot_h
            / n_dot_h.max(1e-5);
        let fresnel = (1.0 - v_dot_h).powi(5);
        scale += (1.0 - fresnel) * g;
        bias += fresnel * g;
    }
    (scale / samples as f32, bias / samples as f32)
}

/// The LUT texels, `R16G16_SFLOAT` row-major: `u` = N·V, `v` = roughness (texel centres).
pub(crate) fn brdf_lut() -> Vec<u8> {
    let mut bytes = Vec::with_capacity((BRDF_LUT_SIZE * BRDF_LUT_SIZE * 4) as usize);
    for row in 0..BRDF_LUT_SIZE {
        let roughness = (row as f32 + 0.5) / BRDF_LUT_SIZE as f32;
        for column in 0..BRDF_LUT_SIZE {
            let n_dot_v = (column as f32 + 0.5) / BRDF_LUT_SIZE as f32;
            let (scale, bias) = integrate_brdf(n_dot_v, roughness, BRDF_LUT_SAMPLES);
            bytes.extend_from_slice(&moba_assets::f16_from_f32(scale).to_le_bytes());
            bytes.extend_from_slice(&moba_assets::f16_from_f32(bias).to_le_bytes());
        }
    }
    bytes
}

/// Image-based lighting state copied into the camera uniform every frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EnvironmentState {
    /// SH-L2 irradiance / π, linear RGB.
    pub(crate) sh: [[f32; 3]; 9],
    /// Environment intensity (0 = no environment: hemispheric ambient).
    pub(crate) intensity: f32,
    /// Highest mip of the specular cube (roughness 1).
    pub(crate) max_lod: f32,
    /// Whether a specular cube is bound and sampled.
    pub(crate) specular: bool,
}

impl EnvironmentState {
    pub(crate) const NONE: Self = Self {
        sh: [[0.0; 3]; 9],
        intensity: 0.0,
        max_lod: 0.0,
        specular: false,
    };
}

/// A map's baked ambient occlusion placement ([`Renderer::set_baked_ao`], ADR-0016).
///
/// The AO texture (`R8`, 1 = open) covers the square `origin .. origin + size` of the
/// world's xz plane (`u` along +x, `v` along +z). Surfaces sample it at their xz and fade
/// it out over `height` metres above the ground; `strength` scales it (`0` = off).
///
/// [`Renderer::set_baked_ao`]: crate::Renderer::set_baked_ao
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct BakedAo {
    /// World xz of the texture's `(0, 0)` corner.
    pub origin: [f32; 2],
    /// World size of the square the texture covers, metres.
    pub size: f32,
    /// Height above the ground over which the occlusion fades out, metres.
    pub height: f32,
    /// Strength, `0..=1`.
    pub strength: f32,
}

impl BakedAo {
    /// AO over `origin .. origin + size`, a 3 m falloff, full strength.
    #[must_use]
    pub const fn new(origin: [f32; 2], size: f32) -> Self {
        Self {
            origin,
            size,
            height: 3.0,
            strength: 1.0,
        }
    }

    /// Returns this placement with [`BakedAo::height`] set.
    #[must_use]
    pub const fn with_height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    /// Returns this placement with [`BakedAo::strength`] set.
    #[must_use]
    pub const fn with_strength(mut self, strength: f32) -> Self {
        self.strength = strength;
        self
    }

    /// The camera-uniform values: `([origin x, origin z, 1 / size, height], strength)`;
    /// `None` when unusable.
    pub(crate) fn uniform(self) -> Option<([f32; 4], f32)> {
        let finite = self.origin.iter().all(|value| value.is_finite())
            && self.size.is_finite()
            && self.size > 0.0;
        finite.then(|| {
            (
                [
                    self.origin[0],
                    self.origin[1],
                    1.0 / self.size,
                    sanitize(self.height, 0.1, 64.0),
                ],
                sanitize(self.strength, 0.0, 1.0),
            )
        })
    }
}

/// The lighting values of one frame, packed into the camera uniform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FrameLighting {
    pub(crate) light_view_proj: [[f32; 4]; 4],
    pub(crate) sun_color: [f32; 4],
    pub(crate) shadow_params: [f32; 4],
    pub(crate) shadow_extra: [f32; 4],
    pub(crate) ibl_params: [f32; 4],
    pub(crate) style: [f32; 4],
    pub(crate) rim_color: [f32; 4],
    pub(crate) specular_tint: [f32; 4],
    pub(crate) ambient_ground: [f32; 4],
    pub(crate) sh: [[f32; 4]; 9],
    // R2b (ADR-0016).
    pub(crate) light_grid: [f32; 4],
    pub(crate) light_info: [u32; 4],
    pub(crate) occlusion: [f32; 4],
    pub(crate) baked_ao: [f32; 4],
    pub(crate) contact: [f32; 4],
}

impl FrameLighting {
    /// Adds this frame's binned dynamic lights.
    pub(crate) fn with_lights(mut self, grid: crate::lights::GridParams) -> Self {
        self.light_grid = grid.grid;
        self.light_info = grid.info;
        self
    }

    /// Adds the map's baked AO (`None`, or `config.baked_ao` off: none).
    pub(crate) fn with_baked_ao(mut self, ao: Option<BakedAo>, config: LightingConfig) -> Self {
        match ao.filter(|_| config.baked_ao).and_then(BakedAo::uniform) {
            Some((placement, strength)) => {
                self.baked_ao = placement;
                self.occlusion[0] = strength;
            }
            None => self.occlusion[0] = 0.0,
        }
        self
    }

    /// Turns on sampling the main view's contact-shadow target (`config`'s steps > 0).
    pub(crate) fn with_contact_shadows(mut self, config: LightingConfig) -> Self {
        let steps = config.contact_steps();
        if steps > 0 {
            self.occlusion[1] = 1.0;
            self.contact = [
                steps as f32,
                CONTACT_RAY_LENGTH,
                CONTACT_THICKNESS,
                CONTACT_FADE_END,
            ];
        }
        self
    }

    /// The lighting of an offscreen view (R7b portraits, the minimap): shadows **off** (the
    /// map is fitted to, and rendered after, the main view; a baked minimap must not keep
    /// view-dependent shadows), the scene's art direction and the environment kept.
    pub(crate) fn offscreen(
        scene: &SceneLighting,
        config: LightingConfig,
        environment: &EnvironmentState,
    ) -> Self {
        Self::new(
            scene,
            config.with_shadows(ShadowFilter::Off),
            config.shadow_resolution,
            None,
            environment,
        )
    }

    /// Builds the frame's lighting from the scene, the renderer config and the environment.
    /// `fit` is `None` when shadows are off (or the camera is degenerate): the shader then
    /// skips shadowing.
    pub(crate) fn new(
        scene: &SceneLighting,
        config: LightingConfig,
        resolution: u32,
        fit: Option<ShadowFit>,
        environment: &EnvironmentState,
    ) -> Self {
        let (mode, light_view_proj, texel_world, depth_range) = match fit {
            Some(fit) if config.shadows != ShadowFilter::Off => (
                config.shadows.shader_mode(),
                fit.view_proj,
                fit.texel_world,
                fit.depth_range,
            ),
            _ => (0.0, Mat4::IDENTITY, 1.0, 1.0),
        };
        let (taps, search, max_penumbra) = config.filter_taps();
        let softness = sanitize(scene.shadow_softness, 0.0, 0.2);
        // Penumbra radius in texels per unit of light-space depth difference.
        let penumbra_scale = depth_range * softness / texel_world.max(1e-6);
        let ibl_on = config.ibl && environment.intensity > 0.0;
        let intensity = if ibl_on {
            environment.intensity * sanitize(scene.ibl_intensity, 0.0, 8.0)
        } else {
            0.0
        };
        let mut sh = [[0.0; 4]; 9];
        for (destination, source) in sh.iter_mut().zip(environment.sh) {
            *destination = [source[0], source[1], source[2], 0.0];
        }
        let sun = sanitize_rgb(scene.sun_color, 4.0);
        let rim = sanitize_rgb(scene.rim_color, 4.0);
        let tint = sanitize_rgb(scene.specular_tint, 1.0);
        let ground = sanitize_rgb(scene.ambient_ground, 4.0);
        Self {
            light_view_proj: light_view_proj.to_cols_array_2d(),
            sun_color: [
                sun[0],
                sun[1],
                sun[2],
                // Never 1: shadowed units keep some sun (ADR-0011 rule 2).
                sanitize(scene.shadow_strength, 0.0, moba_assets::MAX_SHADOW_STRENGTH),
            ],
            shadow_params: [
                mode,
                1.0 / resolution.max(1) as f32,
                penumbra_scale,
                1.5 * texel_world,
            ],
            shadow_extra: [taps, search, max_penumbra, 0.0],
            ibl_params: [
                intensity,
                environment.max_lod,
                f32::from(u8::from(config.specular_ibl && environment.specular)),
                0.0,
            ],
            style: [
                sanitize(scene.wrap, 0.0, 1.0),
                sanitize(scene.rim, 0.0, 2.0),
                sanitize(scene.ramp, 0.0, 1.0),
                sanitize(scene.unit_roughness, 0.0, 1.0),
            ],
            rim_color: [
                rim[0],
                rim[1],
                rim[2],
                sanitize(scene.terrain_roughness, 0.0, 1.0),
            ],
            specular_tint: [
                tint[0],
                tint[1],
                tint[2],
                sanitize(scene.specular, 0.0, 2.0),
            ],
            ambient_ground: [ground[0], ground[1], ground[2], 0.0],
            sh,
            light_grid: crate::lights::GridParams::NONE.grid,
            light_info: crate::lights::GridParams::NONE.info,
            occlusion: [0.0; 4],
            baked_ao: [0.0, 0.0, 1.0, 1.0],
            contact: [0.0; 4],
        }
    }
}

/// Bindings of the lighting set (set 3), as in `lighting.glsl`.
const BINDINGS: u32 = 9;
/// Combined image samplers in the lighting set: every binding but the two light buffers.
pub(crate) const LIGHTING_SAMPLERS: u32 = BINDINGS - 2;
/// Fragment-stage combined image samplers a scene pipeline binds: the material set's
/// textures (set 2) and the lighting set's (set 3). Sets 0/1 hold buffers only and the VAT
/// set's one sampler is vertex-stage. Each counts once against both
/// `maxPerStageDescriptorSamplers` and `maxPerStageDescriptorSampledImages`.
pub(crate) const SCENE_FRAGMENT_SAMPLERS: u32 =
    moba_assets::TextureSlot::COUNT as u32 + LIGHTING_SAMPLERS;
/// Vulkan's required minimum of both per-stage limits (also `MoltenVK`'s value): the
/// budget [`SCENE_FRAGMENT_SAMPLERS`] must stay within so every conformant device runs the
/// scene. Device selection still checks the real limits.
pub(crate) const MIN_PER_STAGE_SAMPLERS: u32 = 16;
const _: () = assert!(SCENE_FRAGMENT_SAMPLERS <= MIN_PER_STAGE_SAMPLERS);
/// The storage-buffer bindings (dynamic lights and their grid); the rest are images.
const LIGHT_LIST_BINDING: u32 = 5;
const LIGHT_GRID_BINDING: u32 = 6;
const LIGHT_LIST_BYTES: u64 = (crate::lights::MAX_LIGHTS
    * crate::lights::LIGHT_VEC4S
    * core::mem::size_of::<[f32; 4]>()) as u64;
const LIGHT_GRID_WORDS: usize =
    crate::lights::GRID * crate::lights::GRID * crate::lights::CELL_WORDS;

/// One frame in flight's light buffers and the two sets that name them.
struct LightFrame {
    lights: GpuBuffer,
    grid: GpuBuffer,
    /// The main view's set (the shadow map and the contact target).
    set: vk::DescriptorSet,
    /// The offscreen views' set: the same bindings, default shadow and contact images.
    offscreen_set: vk::DescriptorSet,
}

/// The scene lighting descriptor sets (one pair per frame in flight) and the images and
/// buffers they name besides the graph transients.
pub(crate) struct LightingResources {
    frames: Vec<LightFrame>,
    _pool: DescriptorPool,
    set_layout: DescriptorSetLayout,
    shadow_compare: Sampler,
    shadow_plain: Sampler,
    linear_clamp: Sampler,
    /// 1×1 depth image at depth 1 (fully lit), bound when shadows are off.
    default_shadow: GpuImage,
    /// 1×1 black cube, bound without an environment.
    default_environment: GpuImage,
    environment: Option<GpuImage>,
    brdf_lut: GpuImage,
    /// 1×1 white, bound without a ramp.
    default_ramp: GpuImage,
    /// 1×1 white `R8`: no baked AO, and the contact target of views without one.
    default_white: GpuImage,
    /// The ramp's view (owned by the material texture table), or `None`.
    ramp: Option<vk::ImageView>,
    /// The bound shadow-map view: the graph transient, or the default.
    shadow_view: vk::ImageView,
    /// The main view's contact-shadow transient, when the frame has the pass.
    contact_view: Option<vk::ImageView>,
    /// The map's baked AO texture and placement.
    baked_ao: Option<(GpuImage, BakedAo)>,
    state: EnvironmentState,
    light_stats: crate::lights::LightStats,
    /// CPU scratch of the light binning's per-column counts (the mapped grid is write-only).
    cell_counts: crate::lights::CellCounts,
}

impl LightingResources {
    /// The set layout, samplers, default images, the BRDF LUT, `frames` light buffers and
    /// their sets. `shadow_format` is the shadow map's format (or the depth format when
    /// shadows are off) and `shadow_filter_linear` whether it can be sampled with linear
    /// filtering.
    #[allow(clippy::too_many_lines)] // Layout, samplers, default images and buffers, in order.
    pub(crate) fn new(
        device: &DeviceContext,
        frames: usize,
        shadow_format: vk::Format,
        shadow_filter_linear: bool,
    ) -> RenderResult<Self> {
        let bindings: [vk::DescriptorSetLayoutBinding<'_>; BINDINGS as usize] =
            core::array::from_fn(|index| {
                let index = index as u32;
                let kind = if index == LIGHT_LIST_BINDING || index == LIGHT_GRID_BINDING {
                    vk::DescriptorType::STORAGE_BUFFER
                } else {
                    vk::DescriptorType::COMBINED_IMAGE_SAMPLER
                };
                vk::DescriptorSetLayoutBinding::default()
                    .binding(index)
                    .descriptor_type(kind)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            });
        let set_layout = DescriptorSetLayout::new(device.device.clone(), &bindings)?;
        let sets_total = u32::try_from(2 * frames).expect("frames in flight fit u32");
        let pool_sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(sets_total * (BINDINGS - 2)),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(sets_total * 2),
        ];
        let pool = DescriptorPool::new(device.device.clone(), &pool_sizes, sets_total)?;
        let layouts = vec![set_layout.handle(); 2 * frames];
        let sets = pool.allocate(&device.device, &layouts)?;
        let filter = if shadow_filter_linear {
            vk::Filter::LINEAR
        } else {
            vk::Filter::NEAREST
        };
        let clamp = vk::SamplerAddressMode::CLAMP_TO_EDGE;
        let shadow_compare = Sampler::new(
            device.device.clone(),
            &vk::SamplerCreateInfo::default()
                .mag_filter(filter)
                .min_filter(filter)
                .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                .address_mode_u(clamp)
                .address_mode_v(clamp)
                .address_mode_w(clamp)
                // Without `mutableComparisonSamplers` (portability) shadows are off and this
                // sampler is never used; it must not enable comparison then.
                .compare_enable(device.comparison_samplers)
                .compare_op(vk::CompareOp::LESS_OR_EQUAL)
                .max_lod(0.0),
        )?;
        let shadow_plain = Sampler::new(
            device.device.clone(),
            &vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::NEAREST)
                .min_filter(vk::Filter::NEAREST)
                .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                .address_mode_u(clamp)
                .address_mode_v(clamp)
                .address_mode_w(clamp)
                .max_lod(0.0),
        )?;
        let linear_clamp = Sampler::new(
            device.device.clone(),
            &vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::LINEAR)
                .min_filter(vk::Filter::LINEAR)
                .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                .address_mode_u(clamp)
                .address_mode_v(clamp)
                .address_mode_w(clamp)
                .min_lod(0.0)
                .max_lod(vk::LOD_CLAMP_NONE),
        )?;
        let default_shadow = texture::default_depth(device, shadow_format)?;
        let default_environment = texture::upload_raw(
            device,
            "default-environment",
            &texture::RawImage {
                format: vk::Format::R8G8B8A8_UNORM,
                extent: vk::Extent2D {
                    width: 1,
                    height: 1,
                },
                cube: true,
                levels: &[(0, 24)],
                bytes: &[0, 0, 0, 255].repeat(6),
            },
        )?;
        let lut = brdf_lut();
        let brdf_lut = texture::upload_raw(
            device,
            "brdf-lut",
            &texture::RawImage {
                format: vk::Format::R16G16_SFLOAT,
                extent: vk::Extent2D {
                    width: BRDF_LUT_SIZE,
                    height: BRDF_LUT_SIZE,
                },
                cube: false,
                levels: &[(0, lut.len())],
                bytes: &lut,
            },
        )?;
        let default_ramp = texture::upload_rgba(device, "default-ramp", 1, 1, &[255; 4])?;
        let default_white = texture::upload_raw(
            device,
            "default-occlusion",
            &texture::RawImage {
                format: vk::Format::R8_UNORM,
                extent: vk::Extent2D {
                    width: 1,
                    height: 1,
                },
                cube: false,
                levels: &[(0, 1)],
                bytes: &[255],
            },
        )?;
        let mut light_frames = Vec::with_capacity(frames);
        for frame in 0..frames {
            let buffer = |name, size| {
                GpuBuffer::new(
                    device.device.clone(),
                    device.allocator(),
                    name,
                    size,
                    vk::BufferUsageFlags::STORAGE_BUFFER,
                    gpu_allocator::MemoryLocation::CpuToGpu,
                )
            };
            let lights = buffer("dynamic-lights", LIGHT_LIST_BYTES)?;
            let mut grid = buffer("light-grid", (LIGHT_GRID_WORDS * 4) as u64)?;
            // Every column starts empty; frames only rewrite the counts and used entries.
            if let Some(words) = grid.mapped_slice_mut::<u32>(LIGHT_GRID_WORDS) {
                words.fill(0);
            }
            light_frames.push(LightFrame {
                lights,
                grid,
                set: sets[2 * frame],
                offscreen_set: sets[2 * frame + 1],
            });
        }
        let shadow_view = default_shadow.view();
        let resources = Self {
            frames: light_frames,
            _pool: pool,
            set_layout,
            shadow_compare,
            shadow_plain,
            linear_clamp,
            default_shadow,
            default_environment,
            environment: None,
            brdf_lut,
            default_ramp,
            default_white,
            ramp: None,
            shadow_view,
            contact_view: None,
            baked_ao: None,
            state: EnvironmentState::NONE,
            light_stats: crate::lights::LightStats::default(),
            cell_counts: [0; crate::lights::GRID * crate::lights::GRID],
        };
        resources.write(device);
        Ok(resources)
    }

    pub(crate) fn set_layout(&self) -> vk::DescriptorSetLayout {
        self.set_layout.handle()
    }

    /// The main view's lighting set of frame `frame` (names the shadow map, the contact
    /// target and that frame's light buffers).
    pub(crate) fn set(&self, frame: usize) -> vk::DescriptorSet {
        self.frames[frame].set
    }

    /// The offscreen views' lighting set of frame `frame` (default shadow and contact
    /// images; their uniform has no lights).
    pub(crate) fn offscreen_set(&self, frame: usize) -> vk::DescriptorSet {
        self.frames[frame].offscreen_set
    }

    pub(crate) fn state(&self) -> &EnvironmentState {
        &self.state
    }

    /// The bound baked AO placement, if any.
    pub(crate) fn baked_ao(&self) -> Option<BakedAo> {
        self.baked_ao.as_ref().map(|(_, ao)| *ao)
    }

    /// What happened to the most recent frame's lights.
    pub(crate) fn light_stats(&self) -> crate::lights::LightStats {
        self.light_stats
    }

    /// Bins `lights` for a camera `view_proj` into frame `frame`'s buffers under `config`'s
    /// caps and returns the grid placement for the uniform. The frame's fence must have been
    /// waited on. Never allocates.
    pub(crate) fn upload_lights(
        &mut self,
        frame: usize,
        lights: &[crate::lights::PointLight],
        view_proj: Mat4,
        config: LightingConfig,
    ) -> crate::lights::GridParams {
        let footprint = if config.light_cap() > 0 && !lights.is_empty() {
            crate::lights::ground_footprint(view_proj)
        } else {
            None
        };
        let LightFrame {
            lights: list, grid, ..
        } = &mut self.frames[frame];
        let words = crate::lights::MAX_LIGHTS * crate::lights::LIGHT_VEC4S;
        let (Some(list), Some(cells)) = (
            list.mapped_slice_mut::<[f32; 4]>(words),
            grid.mapped_slice_mut::<u32>(LIGHT_GRID_WORDS),
        ) else {
            return crate::lights::GridParams::NONE;
        };
        let (params, stats) = crate::lights::bin(
            lights,
            footprint,
            config.light_cap(),
            config.cell_cap(),
            list,
            cells,
            &mut self.cell_counts,
        );
        self.light_stats = stats;
        params
    }

    /// Points the main view's sets at `shadow` (the shadow transient) and `contact` (the
    /// contact-shadow transient), or the defaults. The device must be idle or the sets unused
    /// by pending work (creation, resize).
    pub(crate) fn bind_transients(
        &mut self,
        device: &DeviceContext,
        shadow: Option<vk::ImageView>,
        contact: Option<vk::ImageView>,
    ) {
        self.shadow_view = shadow.unwrap_or(self.default_shadow.view());
        self.contact_view = contact;
        self.write(device);
    }

    /// Replaces the environment. The device must be idle.
    pub(crate) fn set_environment(
        &mut self,
        device: &DeviceContext,
        image: Option<GpuImage>,
        state: EnvironmentState,
    ) {
        self.environment = image;
        self.state = state;
        self.write(device);
    }

    /// Replaces the baked AO texture and placement. The device must be idle.
    pub(crate) fn set_baked_ao(&mut self, device: &DeviceContext, ao: Option<(GpuImage, BakedAo)>) {
        self.baked_ao = ao;
        self.write(device);
    }

    /// Replaces the ramp view (owned elsewhere for the renderer's lifetime). The device must
    /// be idle.
    pub(crate) fn set_ramp(&mut self, device: &DeviceContext, ramp: Option<vk::ImageView>) {
        self.ramp = ramp;
        self.write(device);
    }

    fn write(&self, device: &DeviceContext) {
        let white = self.default_white.view();
        for frame in &self.frames {
            self.write_set(
                device,
                frame,
                frame.set,
                self.shadow_view,
                self.contact_view.unwrap_or(white),
            );
            self.write_set(
                device,
                frame,
                frame.offscreen_set,
                self.default_shadow.view(),
                white,
            );
        }
    }

    fn write_set(
        &self,
        device: &DeviceContext,
        frame: &LightFrame,
        set: vk::DescriptorSet,
        shadow: vk::ImageView,
        contact: vk::ImageView,
    ) {
        let environment = self
            .environment
            .as_ref()
            .map_or(self.default_environment.view(), GpuImage::view);
        let baked = self
            .baked_ao
            .as_ref()
            .map_or(self.default_white.view(), |(image, _)| image.view());
        // Bindings 0-4, 7 and 8 are images; 5 and 6 the light buffers.
        let images = [
            (0, self.shadow_compare.handle(), shadow),
            (1, self.shadow_plain.handle(), shadow),
            (2, self.linear_clamp.handle(), environment),
            (3, self.linear_clamp.handle(), self.brdf_lut.view()),
            (
                4,
                self.linear_clamp.handle(),
                self.ramp.unwrap_or(self.default_ramp.view()),
            ),
            (7, self.linear_clamp.handle(), baked),
            (8, self.shadow_plain.handle(), contact),
        ];
        let image_infos: [[vk::DescriptorImageInfo; 1]; 7] = core::array::from_fn(|index| {
            [vk::DescriptorImageInfo::default()
                .sampler(images[index].1)
                .image_view(images[index].2)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)]
        });
        let buffer_infos = [
            [vk::DescriptorBufferInfo::default()
                .buffer(frame.lights.handle())
                .offset(0)
                .range(vk::WHOLE_SIZE)],
            [vk::DescriptorBufferInfo::default()
                .buffer(frame.grid.handle())
                .offset(0)
                .range(vk::WHOLE_SIZE)],
        ];
        let mut writes = [vk::WriteDescriptorSet::default(); BINDINGS as usize];
        for (write, (info, image)) in writes.iter_mut().zip(image_infos.iter().zip(images)) {
            *write = vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(image.0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(info);
        }
        writes[7] = vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(LIGHT_LIST_BINDING)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&buffer_infos[0]);
        writes[8] = vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(LIGHT_GRID_BINDING)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&buffer_infos[1]);
        // SAFETY: the set, samplers, views and buffers are live, and the callers guarantee
        // no pending command buffer uses the set (creation, or after a device-idle wait).
        unsafe { device.device.update_descriptor_sets(&writes, &[]) };
    }
}

/// Validates an environment's specular cube for upload.
pub(crate) fn check_environment_cube(data: &moba_assets::TextureData) -> RenderResult<()> {
    if !data.is_cube() {
        return Err(RenderError::InvalidEnvironment(
            "the specular map must be a cube map",
        ));
    }
    if data.format().is_srgb() {
        return Err(RenderError::InvalidEnvironment(
            "the specular map must hold linear radiance",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Sampler declarations (`uniform [u|i]sampler…`) in `set = <set>` of a GLSL source.
    fn samplers_in_set(source: &str, set: u32) -> u32 {
        let needle = format!("set = {set},");
        let count = source
            .lines()
            .map(str::trim_start)
            .filter(|line| !line.starts_with("//") && line.contains(&needle))
            .filter(|line| {
                ["uniform sampler", "uniform usampler", "uniform isampler"]
                    .iter()
                    .any(|kind| line.contains(kind))
            })
            .count();
        u32::try_from(count).expect("small")
    }

    #[test]
    fn the_scene_fragment_samplers_fit_the_vulkan_per_stage_minimum() {
        let lighting = include_str!("../../shaders/shaders/include/lighting.glsl");
        let material = include_str!("../../shaders/shaders/include/material.glsl");
        // The constants match what the shaders declare...
        assert_eq!(samplers_in_set(lighting, 3), super::LIGHTING_SAMPLERS);
        assert_eq!(
            samplers_in_set(material, 2),
            moba_assets::TextureSlot::COUNT as u32
        );
        // ...no scene vertex shader samples in sets 0/1, and the total is within 16.
        for source in [lighting, material] {
            assert_eq!(samplers_in_set(source, 0) + samplers_in_set(source, 1), 0);
        }
        let total = samplers_in_set(lighting, 3) + samplers_in_set(material, 2);
        assert_eq!(total, super::SCENE_FRAGMENT_SAMPLERS);
        assert!(
            total <= super::MIN_PER_STAGE_SAMPLERS,
            "{total} fragment samplers"
        );
    }

    use super::*;
    use moba_core::math::{look_at_rh, perspective_rh};

    #[test]
    fn tiers_pick_cheaper_shadows_lower_down() {
        let low = LightingConfig::for_tier(QualityTier::Low);
        assert_eq!(low.shadows, ShadowFilter::Hard);
        assert!(!low.specular_ibl);
        assert_eq!(low.clamped_resolution(16_384), 1024);
        assert_eq!(
            LightingConfig::for_tier(QualityTier::Ultra).clamped_resolution(16_384),
            4096
        );
        assert_eq!(
            LightingConfig::for_tier(QualityTier::Ultra).clamped_resolution(2048),
            2048
        );
        assert_eq!(
            low.with_shadow_resolution(3000).clamped_resolution(16_384),
            4096
        );
        assert_eq!(
            low.with_shadow_resolution(3000).clamped_resolution(3000),
            2048
        );
        assert_eq!(
            low.with_shadow_resolution(1).clamped_resolution(16_384),
            256
        );
        for &filter in ShadowFilter::ALL {
            assert_eq!(ShadowFilter::from_name(filter.name()), Some(filter));
        }
        assert_eq!(ShadowFilter::from_name(" PCSS "), Some(ShadowFilter::Pcss));
        assert_eq!(ShadowFilter::from_name("vsm"), None);
    }

    fn overview() -> Mat4 {
        let eye = Vec3::new(120.0, 180.0, 120.0);
        perspective_rh(1.05, 16.0 / 9.0, 0.1, 1000.0) * look_at_rh(eye, Vec3::ZERO, Vec3::Y)
    }

    #[test]
    fn shadow_fit_covers_the_visible_ground_and_casters() {
        let light = Vec3::new(0.45, 1.0, 0.35).normalize();
        let fit = fit_shadow(overview(), light, 2048).expect("fits");
        // The camera target and a 32 m caster above it land inside the map, with z in 0..1.
        for point in [
            Vec3::ZERO,
            Vec3::new(0.0, CASTER_HEIGHT, 0.0),
            Vec3::new(40.0, 0.0, -30.0),
        ] {
            let clip = fit.view_proj * point.extend(1.0);
            assert!(
                clip.x.abs() < 1.0 && clip.y.abs() < 1.0,
                "{point} -> {clip}"
            );
            assert!((0.0..=1.0).contains(&clip.z), "{point} -> {clip}");
        }
        // A caster is nearer the light than the ground under it.
        let ground = fit.view_proj * Vec4::new(0.0, 0.0, 0.0, 1.0);
        let top = fit.view_proj * (light * 10.0).extend(1.0);
        assert!(top.z < ground.z);
        assert!(
            fit.texel_world > 0.0 && fit.texel_world < 1.0,
            "{}",
            fit.texel_world
        );
    }

    #[test]
    fn shadow_fit_tightens_for_a_close_player_camera() {
        // The player camera: 18 m away, 54 degrees down, below the caster ceiling. Only the
        // slab part of each corner ray counts, so the map stays tight (not 520 m wide).
        let eye = Vec3::new(0.0, 14.6, 10.6);
        let close =
            perspective_rh(1.05, 16.0 / 9.0, 0.1, 1000.0) * look_at_rh(eye, Vec3::ZERO, Vec3::Y);
        let light = Vec3::new(0.45, 1.0, 0.35).normalize();
        let fit = fit_shadow(close, light, 2048).expect("fits");
        assert!(fit.texel_world < 0.05, "{}", fit.texel_world);
        let overview = fit_shadow(overview(), light, 2048).expect("fits");
        assert!(overview.texel_world > fit.texel_world * 2.0);
        // A camera that only sees sky has nothing to shadow.
        let sky = perspective_rh(0.5, 1.0, 0.1, 1000.0)
            * look_at_rh(
                Vec3::new(0.0, 50.0, 0.0),
                Vec3::new(0.0, 100.0, 1.0),
                Vec3::Y,
            );
        assert!(fit_shadow(sky, light, 2048).is_none());
    }

    /// The texel sizes ADR-0011 quotes, for the client's real cameras at 2048².
    #[test]
    fn shadow_texels_at_the_client_cameras() {
        let light = Vec3::new(0.45, 1.0, 0.35).normalize();
        let texel = |distance: f32, pitch: f32| {
            let camera = crate::Camera::orbit(Vec3::ZERO, distance, -0.7, pitch, 16.0 / 9.0);
            fit_shadow(camera.view_proj, light, 2048)
                .expect("fits")
                .texel_world
        };
        let (close, player, overview) = (texel(18.0, 0.95), texel(46.0, 0.95), texel(240.0, 0.92));
        eprintln!("texel m: close {close:.4} player {player:.4} overview {overview:.4}");
        assert!(close < 0.05 && player < 0.1 && overview < 0.4);
        assert!(close < player && player < overview);
    }

    #[test]
    fn shadow_fit_is_texel_stable_while_panning() {
        let light = Vec3::new(0.45, 1.0, 0.35).normalize();
        let base = fit_shadow(overview(), light, 2048).expect("fits");
        for step in 1..6 {
            let pan =
                Mat4::from_translation(Vec3::new(-0.37 * step as f32, 0.0, 0.21 * step as f32));
            let moved = fit_shadow(overview() * pan, light, 2048).expect("fits");
            assert!((moved.texel_world - base.texel_world).abs() < 1e-5);
            // A fixed world point lands on the same sub-texel position.
            let texel = 2.0 / 2048.0;
            let a = base.view_proj * Vec4::new(3.0, 0.0, 5.0, 1.0);
            let b = moved.view_proj * Vec4::new(3.0, 0.0, 5.0, 1.0);
            let fraction = |v: f32| (v / texel).rem_euclid(1.0);
            assert!((fraction(a.x) - fraction(b.x)).abs() < 2e-3, "x {step}");
            assert!((fraction(a.y) - fraction(b.y)).abs() < 2e-3, "y {step}");
        }
        assert!(fit_shadow(Mat4::ZERO, light, 2048).is_none());
        assert!(fit_shadow(overview(), Vec3::ZERO, 2048).is_none());
    }

    #[test]
    fn brdf_lut_matches_known_split_sum_values() {
        // Smooth and head-on: almost all energy in the scale term.
        let (scale, bias) = integrate_brdf(1.0, 0.05, 256);
        assert!(scale > 0.9 && scale < 1.05 && bias < 0.05, "{scale} {bias}");
        // Grazing: Fresnel moves energy into the bias term.
        let (scale, bias) = integrate_brdf(0.05, 0.3, 256);
        assert!(bias > 0.25 && scale < 0.7, "{scale} {bias}");
        // Rough surfaces lose energy (single scattering).
        let (scale, bias) = integrate_brdf(0.5, 1.0, 256);
        assert!(scale + bias < 0.9 && scale + bias > 0.3, "{scale} {bias}");
        let lut = brdf_lut();
        assert_eq!(lut.len(), (BRDF_LUT_SIZE * BRDF_LUT_SIZE * 4) as usize);
    }

    #[test]
    fn offscreen_views_never_sample_the_shadow_map() {
        let environment = EnvironmentState {
            intensity: 1.0,
            specular: true,
            max_lod: 6.0,
            ..EnvironmentState::NONE
        };
        for &tier in QualityTier::ALL {
            let config = LightingConfig::for_tier(tier);
            let view = FrameLighting::offscreen(&SceneLighting::DEFAULT, config, &environment);
            assert_eq!(view.shadow_params[0], 0.0, "{tier}: shadow filter off");
            assert_eq!(view.light_view_proj, Mat4::IDENTITY.to_cols_array_2d());
            // The environment is kept (portraits and the minimap stay lit like the map).
            assert_eq!(view.ibl_params[0], 1.0);
        }
    }

    #[test]
    fn pcss_taps_follow_the_tier() {
        let high = LightingConfig::for_tier(QualityTier::High);
        let ultra = LightingConfig::for_tier(QualityTier::Ultra);
        assert_eq!(high.filter_taps().0, 16.0);
        assert_eq!(ultra.filter_taps().0, 32.0);
        // Independent of the resolution: an Ultra map at 2048 keeps Ultra's disk.
        assert_eq!(ultra.with_shadow_resolution(2048).filter_taps().0, 32.0);
        assert_eq!(high.with_pcss_taps(1000).filter_taps().0, 64.0);
    }

    #[test]
    fn frame_lighting_sanitises_untrusted_parameters() {
        let scene = SceneLighting::DEFAULT
            .with_wrap(f32::NAN)
            .with_shadow_strength(7.0)
            .with_specular(Vec3::new(2.0, -1.0, 0.5), 9.0);
        let environment = EnvironmentState {
            intensity: 1.0,
            ..EnvironmentState::NONE
        };
        let config = LightingConfig::for_tier(QualityTier::High);
        let fit = fit_shadow(overview(), Vec3::Y, 2048);
        let frame = FrameLighting::new(&scene, config, 2048, fit, &environment);
        assert_eq!(frame.style[0], 0.0);
        assert_eq!(frame.sun_color[3], moba_assets::MAX_SHADOW_STRENGTH);
        assert_eq!(frame.specular_tint, [1.0, 0.0, 0.5, 2.0]);
        assert_eq!(frame.shadow_params[0], 3.0);
        assert_eq!(frame.ibl_params[0], 1.0);
        let off = FrameLighting::new(
            &scene,
            config.with_shadows(ShadowFilter::Off).with_ibl(false),
            2048,
            fit,
            &environment,
        );
        assert_eq!(off.shadow_params[0], 0.0);
        assert_eq!(
            off.ibl_params[0], 0.0,
            "IBL off keeps the hemispheric ambient"
        );
    }
}
