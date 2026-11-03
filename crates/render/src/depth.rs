//! The main view's depth convention (ADR-0023 §1c).
//!
//! **Compat** keeps the convention every pass was written for: depth 0 at the near plane and 1
//! at the far plane, cleared to 1 and tested `LESS_OR_EQUAL`. **Tiered** uses **reversed-Z**
//! from N2a: the near plane at 1 and the finite far plane at 0, cleared to 0 and tested
//! `GREATER_OR_EQUAL`. A `D32_SFLOAT` depth then keeps its floating-point precision where the
//! perspective division spends it, far from the camera.
//!
//! One convention holds for every view a renderer draws (the main view, the overlay prepass,
//! the offscreen portraits and minimap), because they share pipelines. Only the GPU sees it:
//! the client's [`Camera`](crate::Camera), CPU culling, the shadow fit, the light grid and
//! picking keep the forward matrix, and the renderer converts the matrices it uploads with
//! [`DepthConvention::view_proj`]. Shadow maps are their own images with their own
//! (forward) convention and comparison sampler, and are not affected.

use ash::vk;
use moba_core::math::{Mat4, Vec4};

use crate::caps::RenderPath;

/// Which way the main view's depth runs (ADR-0023 §1c).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum DepthConvention {
    /// Compat: 0 near, 1 far, `LESS_OR_EQUAL`.
    #[default]
    Forward,
    /// Tiered: 1 near, 0 far (finite), `GREATER_OR_EQUAL`.
    Reversed,
}

impl DepthConvention {
    /// The convention of a render path: reversed on Tiered, forward on Compat.
    #[must_use]
    pub(crate) const fn of(path: RenderPath) -> Self {
        match path {
            RenderPath::Tiered => Self::Reversed,
            RenderPath::Compat => Self::Forward,
        }
    }

    /// Whether depth is reversed.
    #[must_use]
    pub(crate) const fn is_reversed(self) -> bool {
        matches!(self, Self::Reversed)
    }

    /// The far plane's depth, which is also the clear value: 1 forward, 0 reversed.
    #[must_use]
    pub(crate) const fn far(self) -> f32 {
        match self {
            Self::Forward => 1.0,
            Self::Reversed => 0.0,
        }
    }

    /// The depth clear value of every main-view depth attachment.
    pub(crate) const fn clear(self) -> vk::ClearDepthStencilValue {
        vk::ClearDepthStencilValue {
            depth: self.far(),
            stencil: 0,
        }
    }

    /// The test of every depth-tested main-view pipeline: a fragment at the same depth or
    /// nearer passes (`LESS_OR_EQUAL` forward, `GREATER_OR_EQUAL` reversed). The prepass and
    /// the scene reproduce the same depths (`invariant gl_Position`), so the scene's test
    /// passes exactly where the prepass drew, in either convention.
    #[must_use]
    pub(crate) const fn compare(self) -> vk::CompareOp {
        match self {
            Self::Forward => vk::CompareOp::LESS_OR_EQUAL,
            Self::Reversed => vk::CompareOp::GREATER_OR_EQUAL,
        }
    }

    /// The sky backdrop's vertex shader: a triangle on this convention's far plane.
    #[must_use]
    pub(crate) const fn sky_vertex(self) -> &'static str {
        match self {
            Self::Forward => "sky.vert",
            Self::Reversed => "sky_reversed.vert",
        }
    }

    /// Whether depth `a` is nearer the camera than depth `b`.
    #[must_use]
    pub(crate) fn nearer(self, a: f32, b: f32) -> bool {
        match self {
            Self::Forward => a < b,
            Self::Reversed => a > b,
        }
    }

    /// The GPU's view-projection for a forward (`0..1`) `view_proj` built with the near and
    /// far planes `near` and `far` (0 for an orthographic camera): unchanged forward;
    /// reversed, the clip-space `z' = w - z`, so the near plane lands at 1 and the far at 0.
    ///
    /// For a perspective camera ([`Camera::perspective`](crate::Camera::perspective) and
    /// `orbit`), the depth row is rebuilt from the `w` row and the planes as
    /// `z' = n f/(f-n) - n/(f-n) w_row` instead of subtracting two rows: `z_row` and `w_row`
    /// are nearly equal, and their difference would cancel most of `f32`'s precision, which
    /// is what reversed-Z is for. A matrix that is not that projection (planes that do not
    /// match it, an orthographic view) takes the subtraction, which is exact enough for a
    /// linear depth.
    #[must_use]
    pub(crate) fn view_proj(self, view_proj: Mat4, near: f32, far: f32) -> Mat4 {
        match self {
            Self::Forward => view_proj,
            Self::Reversed => reverse(view_proj, near, far),
        }
    }

    /// [`Self::view_proj`] of a camera's matrix with its own planes.
    #[must_use]
    pub(crate) fn camera(self, camera: &crate::Camera) -> Mat4 {
        self.view_proj(camera.view_proj, camera.near, camera.far)
    }
}

/// `view_proj` with `z' = w - z` (see [`DepthConvention::view_proj`]).
fn reverse(view_proj: Mat4, near: f32, far: f32) -> Mat4 {
    let z = view_proj.row(2);
    let w = view_proj.row(3);
    let subtracted = w - z;
    let exact = (near.is_finite() && far.is_finite() && near > 0.0 && far > near)
        .then(|| {
            let range = far - near;
            // The forward row this camera's projection has: `f/(n-f) (-w_row) + n f/(n-f)`.
            let forward = -w * (far / (near - far)) + Vec4::W * (far * near / (near - far));
            let scale = z.abs().max_element().max(w.abs().max_element()).max(1e-6);
            ((forward - z).abs().max_element() <= 1e-4 * scale)
                .then(|| -w * (near / range) + Vec4::W * (far * near / range))
        })
        .flatten();
    let row = exact.unwrap_or(subtracted);
    if !row.is_finite() {
        return view_proj;
    }
    // Row 2 is the third component of every column (glam is column-major).
    let mut columns = view_proj.to_cols_array_2d();
    columns[0][2] = row.x;
    columns[1][2] = row.y;
    columns[2][2] = row.z;
    columns[3][2] = row.w;
    Mat4::from_cols_array_2d(&columns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use moba_core::math::{Vec3, look_at_rh, orthographic_rh, perspective_rh};

    fn camera() -> (Mat4, f32, f32) {
        let (near, far) = (0.1, 1_000.0);
        let view = look_at_rh(
            Vec3::new(4.0, 40.0, 26.0),
            Vec3::new(1.0, 0.0, -3.0),
            Vec3::Y,
        );
        (
            perspective_rh(1.05, 16.0 / 9.0, near, far) * view,
            near,
            far,
        )
    }

    fn ndc(view_proj: Mat4, point: Vec3) -> Vec3 {
        let clip = view_proj * point.extend(1.0);
        clip.truncate() / clip.w
    }

    #[test]
    fn the_paths_take_their_conventions() {
        assert_eq!(
            DepthConvention::of(RenderPath::Tiered),
            DepthConvention::Reversed
        );
        assert_eq!(
            DepthConvention::of(RenderPath::Compat),
            DepthConvention::Forward
        );
        assert_eq!(DepthConvention::Forward.clear().depth, 1.0);
        assert_eq!(DepthConvention::Reversed.clear().depth, 0.0);
        assert_eq!(
            DepthConvention::Forward.compare(),
            vk::CompareOp::LESS_OR_EQUAL
        );
        assert_eq!(
            DepthConvention::Reversed.compare(),
            vk::CompareOp::GREATER_OR_EQUAL
        );
        assert!(DepthConvention::Forward.nearer(0.25, 0.5));
        assert!(DepthConvention::Reversed.nearer(0.5, 0.25));
    }

    #[test]
    fn forward_is_the_identity() {
        let (view_proj, near, far) = camera();
        assert_eq!(
            DepthConvention::Forward.view_proj(view_proj, near, far),
            view_proj
        );
    }

    #[test]
    fn reversed_puts_the_near_plane_at_one_and_the_far_at_zero() {
        let (view_proj, near, far) = camera();
        let reversed = DepthConvention::Reversed.view_proj(view_proj, near, far);
        let inverse = view_proj.inverse();
        for (x, y) in [(0.0, 0.0), (0.7, -0.4), (-0.9, 0.9)] {
            let at = |depth: f32| {
                let world = inverse * Vec4::new(x, y, depth, 1.0);
                world.truncate() / world.w
            };
            let near_point = ndc(reversed, at(0.0));
            let far_point = ndc(reversed, at(1.0));
            // Through the forward inverse, which costs a few ulps of f32.
            assert!((near_point.z - 1.0).abs() < 1e-4, "{near_point}");
            assert!(far_point.z.abs() < 1e-4, "{far_point}");
            // x and y are untouched.
            assert!((near_point.x - x).abs() < 1e-4 && (near_point.y - y).abs() < 1e-4);
            // Every depth in between maps to 1 - d.
            for depth in [0.25f32, 0.5, 0.9, 0.999] {
                let point = ndc(reversed, at(depth));
                assert!((point.z - (1.0 - depth)).abs() < 2e-4, "{depth}: {point}");
            }
        }
    }

    #[test]
    fn the_rebuilt_row_keeps_the_precision_the_subtraction_loses() {
        // A point 46 m from the eye (the gameplay camera) and one 1 mm behind it: the
        // rebuilt matrix separates them by the analytic step, the subtraction of two nearly
        // equal rows by far less reliably.
        let (view_proj, near, far) = camera();
        let reversed = DepthConvention::Reversed.view_proj(view_proj, near, far);
        let eye = Vec3::new(4.0, 40.0, 26.0);
        let direction = (Vec3::new(1.0, 0.0, -3.0) - eye).normalize();
        let a = eye + direction * 46.0;
        let b = eye + direction * 46.001;
        let (da, db) = (ndc(reversed, a).z, ndc(reversed, b).z);
        // Reversed: farther is smaller. Analytic depth: n f / (f - n) / z + ... ~ n / z.
        let analytic = |z: f32| near * (far - z) / (z * (far - near));
        assert!(da > db, "{da} {db}");
        let step = analytic(46.0) - analytic(46.001);
        assert!(
            ((da - db) - step).abs() < 0.05 * step,
            "{} vs {step}",
            da - db
        );
        assert!((da - analytic(46.0)).abs() < 1e-3 * analytic(46.0));
    }

    #[test]
    fn linear_depth_parameters_hold_for_the_reversed_matrix() {
        // `passes::vfx::depth_params` reads `(P22, P23)` from any perspective matrix, and
        // `P23 / (d + P22)` is the view depth in either convention.
        let (view_proj, near, far) = camera();
        let reversed = DepthConvention::Reversed.view_proj(view_proj, near, far);
        let (p22, p23) = crate::passes::vfx::depth_params(reversed).expect("perspective");
        let eye = Vec3::new(4.0, 40.0, 26.0);
        for distance in [0.5f32, 10.0, 46.0, 300.0] {
            let direction = (Vec3::new(1.0, 0.0, -3.0) - eye).normalize();
            let point = eye + direction * distance;
            let depth = ndc(reversed, point).z;
            let linear = p23 / (depth + p22);
            let view_z =
                -(look_at_rh(eye, Vec3::new(1.0, 0.0, -3.0), Vec3::Y) * point.extend(1.0)).z;
            assert!(
                (linear - view_z).abs() < 1e-3 * view_z,
                "{distance}: {linear} {view_z}"
            );
        }
    }

    #[test]
    fn jitter_commutes_with_the_reversal() {
        let (view_proj, near, far) = camera();
        let render = vk::Extent2D {
            width: 1280,
            height: 720,
        };
        let jitter = [0.375, -0.125];
        let a = DepthConvention::Reversed.view_proj(
            crate::temporal::jittered(view_proj, jitter, render),
            near,
            far,
        );
        let b = crate::temporal::jittered(
            DepthConvention::Reversed.view_proj(view_proj, near, far),
            jitter,
            render,
        );
        let difference = (a - b).abs();
        assert!(
            difference.to_cols_array().iter().all(|&v| v < 1e-6),
            "{a} {b}"
        );
    }

    #[test]
    fn orthographic_and_mismatched_matrices_subtract_the_rows() {
        let view = look_at_rh(Vec3::new(0.0, 80.0, 0.1), Vec3::ZERO, Vec3::Y);
        let ortho = orthographic_rh(-20.0, 20.0, -10.0, 10.0, 1.0, 200.0) * view;
        // An orthographic camera carries no planes (0, 0).
        let reversed = DepthConvention::Reversed.view_proj(ortho, 0.0, 0.0);
        for depth in [0.0f32, 0.3, 1.0] {
            let world = ortho.inverse() * Vec4::new(0.2, -0.5, depth, 1.0);
            let point = world.truncate() / world.w;
            assert!((ndc(reversed, point).z - (1.0 - depth)).abs() < 1e-5);
        }
        // Planes that do not describe the matrix are not trusted.
        let (view_proj, _, _) = camera();
        let reversed = DepthConvention::Reversed.view_proj(view_proj, 1.0, 50.0);
        let world = view_proj.inverse() * Vec4::new(0.0, 0.0, 0.5, 1.0);
        let point = world.truncate() / world.w;
        assert!((ndc(reversed, point).z - 0.5).abs() < 1e-4);
    }

    #[test]
    fn frustum_planes_are_the_same_set() {
        // Gribb-Hartmann: the near plane is `z_row` and the far `w_row - z_row` in `0..1`;
        // reversed, the two swap, so CPU culling may use either matrix.
        let (view_proj, near, far) = camera();
        let reversed = DepthConvention::Reversed.view_proj(view_proj, near, far);
        let near_forward = view_proj.row(2);
        let far_reversed = reversed.row(3) - reversed.row(2);
        let scale = near_forward.length();
        assert!((near_forward - far_reversed).length() < 1e-3 * scale);
    }
}
