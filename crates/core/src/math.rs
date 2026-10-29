//! Presentation-space math and small geometric helpers.
//!
//! Authoritative gameplay uses [`crate::fix`]; rendering, cameras, UI and effects use the
//! `glam` types re-exported here. Keeping the two worlds separate means a stray `f32`
//! cannot silently leak into the simulation. [`FixVec2::to_vec2`](crate::fix::FixVec2)
//! and the conversions below are the explicit boundary.

use crate::fix::FixVec2;

pub use glam::{Mat3, Mat4, Quat, Vec2, Vec3, Vec4};

/// Full turn in radians.
pub const TAU: f32 = core::f32::consts::TAU;
/// Half turn in radians.
pub const PI: f32 = core::f32::consts::PI;

/// Right-handed look-at view matrix (Vulkan-compatible logic).
///
/// Implemented here rather than relying on platform-specific glam backends: newer glam
/// NEON builds do not expose `Mat4::look_at_rh` on every target.
#[must_use]
pub fn look_at_rh(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    let forward = (target - eye).normalize();
    let side = forward.cross(up).normalize();
    let up = side.cross(forward);
    Mat4::from_cols(
        Vec4::new(side.x, up.x, -forward.x, 0.0),
        Vec4::new(side.y, up.y, -forward.y, 0.0),
        Vec4::new(side.z, up.z, -forward.z, 0.0),
        Vec4::new(-side.dot(eye), -up.dot(eye), forward.dot(eye), 1.0),
    )
}

/// Right-handed perspective projection with Vulkan clip-space depth in `[0, 1]`.
#[must_use]
pub fn perspective_rh(fov_y_radians: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fov_y_radians * 0.5).tan();
    let range = near - far;
    Mat4::from_cols(
        Vec4::new(f / aspect, 0.0, 0.0, 0.0),
        Vec4::new(0.0, f, 0.0, 0.0),
        Vec4::new(0.0, 0.0, far / range, -1.0),
        Vec4::new(0.0, 0.0, (far * near) / range, 0.0),
    )
}

/// Right-handed orthographic projection with Vulkan clip-space depth in `[0, 1]`: view-space
/// `x` in `[left, right]` and `y` in `[bottom, top]` map to NDC `[-1, 1]`, and `-z` in
/// `[near, far]` to depth `[0, 1]`.
#[must_use]
pub fn orthographic_rh(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> Mat4 {
    let width = right - left;
    let height = top - bottom;
    let depth = far - near;
    Mat4::from_cols(
        Vec4::new(2.0 / width, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 / height, 0.0, 0.0),
        Vec4::new(0.0, 0.0, -1.0 / depth, 0.0),
        Vec4::new(
            -(right + left) / width,
            -(top + bottom) / height,
            -near / depth,
            1.0,
        ),
    )
}

/// Converts an authoritative ground-plane position to presentation space (X, 0, Z).
#[inline]
#[must_use]
pub fn fix_to_world(position: FixVec2) -> Vec3 {
    Vec3::new(position.x.to_f32(), 0.0, position.y.to_f32())
}

/// Converts a presentation-space ground position to the authoritative plane.
#[inline]
#[must_use]
pub fn world_to_fix(position: Vec3) -> FixVec2 {
    FixVec2::from_vec2(Vec2::new(position.x, position.z))
}

/// Linear interpolation between `a` and `b`.
#[inline]
#[must_use]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Position of `value` between `a` and `b`; the result is not clamped.
#[inline]
#[must_use]
pub fn inverse_lerp(a: f32, b: f32, value: f32) -> f32 {
    debug_assert!((b - a).abs() > f32::EPSILON, "inverse_lerp needs a range");
    (value - a) / (b - a)
}

/// Hermite smoothstep between two edges.
#[must_use]
pub fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    if (edge1 - edge0).abs() <= f32::EPSILON {
        return if value < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Moves `current` toward `target` by at most `max_delta`.
#[inline]
#[must_use]
pub fn move_towards(current: f32, target: f32, max_delta: f32) -> f32 {
    let delta = target - current;
    if delta.abs() <= max_delta {
        target
    } else {
        current + max_delta.copysign(delta)
    }
}

/// Frame-rate independent exponential smoothing.
///
/// `lambda` is the rate at which the remaining error decays (larger is faster).
#[inline]
#[must_use]
pub fn damp(current: f32, target: f32, lambda: f32, dt: f32) -> f32 {
    target + (current - target) * (-lambda * dt).exp()
}

/// Axis-aligned bounding box.
///
/// An empty box is represented by `min = +inf`, `max = -inf`, which makes the union and
/// point-expansion operations work without special cases.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Aabb {
    /// Minimum corner.
    pub min: Vec3,
    /// Maximum corner.
    pub max: Vec3,
}

impl Aabb {
    /// An empty box that absorbs any other box in a union.
    pub const EMPTY: Self = Self {
        min: Vec3::splat(f32::INFINITY),
        max: Vec3::splat(f32::NEG_INFINITY),
    };

    /// Creates a box from its corners.
    #[inline]
    #[must_use]
    pub const fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    /// Creates a box from a centre and half extents.
    #[inline]
    #[must_use]
    pub fn from_center_half_extents(center: Vec3, half_extents: Vec3) -> Self {
        Self {
            min: center - half_extents,
            max: center + half_extents,
        }
    }

    /// Creates the tightest box containing every point.
    #[must_use]
    pub fn from_points(points: impl IntoIterator<Item = Vec3>) -> Self {
        let mut bounds = Self::EMPTY;
        for point in points {
            bounds.expand_to_include(point);
        }
        bounds
    }

    /// Returns `true` if the box contains no volume.
    #[inline]
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z
    }

    /// Box centre.
    #[inline]
    #[must_use]
    pub fn center(self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    /// Box half extents.
    #[inline]
    #[must_use]
    pub fn half_extents(self) -> Vec3 {
        (self.max - self.min) * 0.5
    }

    /// Box size along each axis.
    #[inline]
    #[must_use]
    pub fn size(self) -> Vec3 {
        self.max - self.min
    }

    /// Returns `true` if `point` lies inside or on the boundary.
    #[inline]
    #[must_use]
    pub fn contains_point(self, point: Vec3) -> bool {
        point.cmpge(self.min).all() && point.cmple(self.max).all()
    }

    /// Returns `true` if two boxes overlap or touch.
    #[inline]
    #[must_use]
    pub fn intersects(self, other: Self) -> bool {
        self.min.cmple(other.max).all() && self.max.cmpge(other.min).all()
    }

    /// Smallest box containing both inputs.
    #[inline]
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    /// Grows the box to include `point`.
    #[inline]
    pub fn expand_to_include(&mut self, point: Vec3) {
        self.min = self.min.min(point);
        self.max = self.max.max(point);
    }

    /// Grows the box outward by `amount` on every axis.
    #[inline]
    #[must_use]
    pub fn expanded(self, amount: f32) -> Self {
        Self {
            min: self.min - Vec3::splat(amount),
            max: self.max + Vec3::splat(amount),
        }
    }

    /// Returns the box transformed by an affine matrix, re-fitted to the eight corners.
    #[must_use]
    pub fn transformed(self, transform: Mat4) -> Self {
        if self.is_empty() {
            return self;
        }
        let corners = [
            Vec3::new(self.min.x, self.min.y, self.min.z),
            Vec3::new(self.max.x, self.min.y, self.min.z),
            Vec3::new(self.min.x, self.max.y, self.min.z),
            Vec3::new(self.max.x, self.max.y, self.min.z),
            Vec3::new(self.min.x, self.min.y, self.max.z),
            Vec3::new(self.max.x, self.min.y, self.max.z),
            Vec3::new(self.min.x, self.max.y, self.max.z),
            Vec3::new(self.max.x, self.max.y, self.max.z),
        ];
        Self::from_points(corners.map(|corner| transform.transform_point3(corner)))
    }

    /// Longest axis length.
    #[inline]
    #[must_use]
    pub fn longest_axis(self) -> f32 {
        self.size().max_element()
    }
}

impl Default for Aabb {
    fn default() -> Self {
        Self::EMPTY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_behave() {
        assert!((lerp(2.0, 4.0, 0.25) - 2.5).abs() < 1e-6);
        assert!((inverse_lerp(2.0, 4.0, 2.5) - 0.25).abs() < 1e-6);
        assert_eq!(smoothstep(0.0, 1.0, -1.0), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, 2.0), 1.0);
        assert!((smoothstep(0.0, 1.0, 0.5) - 0.5).abs() < 1e-6);
        assert_eq!(move_towards(0.0, 10.0, 3.0), 3.0);
        assert_eq!(move_towards(0.0, 10.0, 30.0), 10.0);
        assert_eq!(move_towards(0.0, -10.0, 3.0), -3.0);
        assert!((damp(10.0, 0.0, 5.0, 1.0) - 10.0 * (-5.0f32).exp()).abs() < 1e-5);
    }

    #[test]
    fn camera_matrices_use_vulkan_depth() {
        let view = look_at_rh(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO, Vec3::Y);
        let target = view * Vec4::new(0.0, 0.0, 0.0, 1.0);
        assert!((target.z + 10.0).abs() < 1e-4);
        assert!(target.x.abs() < 1e-6 && target.y.abs() < 1e-6);
        let projection = perspective_rh(core::f32::consts::FRAC_PI_2, 1.0, 0.1, 100.0);
        let near = projection * Vec4::new(0.0, 0.0, -0.1, 1.0);
        let far = projection * Vec4::new(0.0, 0.0, -100.0, 1.0);
        assert!((near.z / near.w).abs() < 1e-4);
        assert!((far.z / far.w - 1.0).abs() < 1e-4);
    }

    #[test]
    fn orthographic_maps_the_box_to_vulkan_clip_space() {
        let projection = orthographic_rh(-4.0, 6.0, -1.0, 3.0, 2.0, 12.0);
        let corner = projection * Vec4::new(-4.0, -1.0, -2.0, 1.0);
        assert!((corner - Vec4::new(-1.0, -1.0, 0.0, 1.0)).length() < 1e-5);
        let far = projection * Vec4::new(6.0, 3.0, -12.0, 1.0);
        assert!((far - Vec4::new(1.0, 1.0, 1.0, 1.0)).length() < 1e-5);
    }

    #[test]
    fn fix_world_boundary_round_trips() {
        let position = FixVec2::from_ints(12, -7);
        let world = fix_to_world(position);
        assert_eq!(world, Vec3::new(12.0, 0.0, -7.0));
        assert_eq!(world_to_fix(world), position);
    }

    #[test]
    fn aabb_basics() {
        let a = Aabb::new(Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0));
        assert!(a.contains_point(Vec3::ZERO));
        assert!(a.contains_point(Vec3::splat(1.0)));
        assert!(!a.contains_point(Vec3::splat(1.1)));
        assert_eq!(a.center(), Vec3::ZERO);
        assert_eq!(a.size(), Vec3::splat(2.0));
        assert_eq!(a.half_extents(), Vec3::ONE);

        let b = Aabb::new(Vec3::new(2.0, 0.0, 0.0), Vec3::new(4.0, 2.0, 2.0));
        assert!(!a.intersects(b));
        assert!(a.intersects(Aabb::new(Vec3::ZERO, Vec3::ONE)));
        assert_eq!(a.union(b).min, Vec3::splat(-1.0));
        assert_eq!(a.union(b).max, Vec3::new(4.0, 2.0, 2.0));
        assert!(!a.is_empty());
        assert!(Aabb::EMPTY.is_empty());
        assert!(!Aabb::EMPTY.union(a).is_empty());
    }

    #[test]
    fn aabb_from_points_and_transform() {
        let boxed = Aabb::from_points([
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(-1.0, 0.0, 5.0),
            Vec3::new(4.0, -2.0, 0.0),
        ]);
        assert_eq!(boxed.min, Vec3::new(-1.0, -2.0, 0.0));
        assert_eq!(boxed.max, Vec3::new(4.0, 2.0, 5.0));

        let transform = Mat4::from_translation(Vec3::X * 10.0);
        let moved = boxed.transformed(transform);
        assert_eq!(moved.min, Vec3::new(9.0, -2.0, 0.0));
        assert_eq!(moved.max, Vec3::new(14.0, 2.0, 5.0));
        assert!(!moved.is_empty());
    }
}
