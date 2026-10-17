//! Presentation-side scene data: camera and per-instance transforms.

use bytemuck::{Pod, Zeroable};
use moba_core::math::{Mat4, Quat, Vec3, Vec4};

/// A pinhole camera with Vulkan clip-space conventions (`z` in `[0, 1]`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Combined view-projection matrix.
    pub view_proj: Mat4,
    /// Eye position in world space.
    pub position: Vec3,
    /// Normalised direction towards the key light.
    pub light_direction: Vec3,
    /// Key-light intensity multiplier.
    pub light_intensity: f32,
    /// Ambient light colour.
    pub ambient: Vec3,
}

impl Camera {
    /// Builds a perspective camera looking at `target`.
    #[must_use]
    pub fn perspective(
        eye: Vec3,
        target: Vec3,
        up: Vec3,
        fov_y_radians: f32,
        aspect: f32,
        near: f32,
        far: f32,
    ) -> Self {
        let view = Mat4::look_at_rh(eye, target, up);
        let projection = Mat4::perspective_rh(fov_y_radians, aspect.max(1e-4), near, far);
        Self {
            view_proj: projection * view,
            position: eye,
            light_direction: Vec3::new(0.45, 1.0, 0.35).normalize(),
            light_intensity: 1.15,
            ambient: Vec3::new(0.32, 0.35, 0.42),
        }
    }

    /// Orbits a target at `distance`, with `yaw` and `pitch` in radians.
    #[must_use]
    pub fn orbit(target: Vec3, distance: f32, yaw: f32, pitch: f32, aspect: f32) -> Self {
        let pitch = pitch.clamp(0.08, 1.45);
        let offset = Vec3::new(
            yaw.cos() * pitch.cos(),
            pitch.sin(),
            yaw.sin() * pitch.cos(),
        ) * distance;
        Self::perspective(
            target + offset,
            target,
            Vec3::Y,
            60.0f32.to_radians(),
            aspect,
            0.1,
            1_000.0,
        )
    }
}

/// One drawable unit: a model matrix and a team colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnitInstance {
    /// Model-to-world transform.
    pub transform: Mat4,
    /// Linear-space RGBA colour.
    pub color: Vec4,
}

impl UnitInstance {
    /// Creates an instance from a transform and colour.
    #[must_use]
    pub const fn new(transform: Mat4, color: Vec4) -> Self {
        Self { transform, color }
    }

    /// Convenience constructor for an upright unit placed on the ground plane.
    #[must_use]
    pub fn at(position: Vec3, height: f32, color: Vec4) -> Self {
        Self {
            transform: Mat4::from_scale_rotation_translation(
                Vec3::new(height * 0.45, height, height * 0.45),
                Quat::IDENTITY,
                position + Vec3::Y * (height * 0.5),
            ),
            color,
        }
    }
}

/// Everything the renderer draws in one frame.
#[derive(Clone, Copy, Debug)]
pub struct Scene<'a> {
    /// Camera for this frame.
    pub camera: Camera,
    /// Units to draw, in any order.
    pub units: &'a [UnitInstance],
}

impl<'a> Scene<'a> {
    /// Creates a scene.
    #[must_use]
    pub const fn new(camera: Camera, units: &'a [UnitInstance]) -> Self {
        Self { camera, units }
    }

    /// Creates an empty scene with a camera (used for clear-only frames and tests).
    #[must_use]
    pub const fn empty(camera: Camera) -> Self {
        Self { camera, units: &[] }
    }
}

/// GPU layout of the per-frame camera uniform. Must match `shaders/unit.vert`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    light_direction: [f32; 4],
    ambient: [f32; 4],
}

impl From<Camera> for CameraUniform {
    fn from(camera: Camera) -> Self {
        Self {
            view_proj: camera.view_proj.to_cols_array_2d(),
            camera_pos: [camera.position.x, camera.position.y, camera.position.z, 1.0],
            light_direction: [
                camera.light_direction.x,
                camera.light_direction.y,
                camera.light_direction.z,
                camera.light_intensity,
            ],
            ambient: [camera.ambient.x, camera.ambient.y, camera.ambient.z, 0.0],
        }
    }
}

/// GPU layout of one instance. Must match `shaders/unit.vert`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct GpuInstance {
    model: [[f32; 4]; 4],
    color: [f32; 4],
}

impl From<UnitInstance> for GpuInstance {
    fn from(instance: UnitInstance) -> Self {
        Self {
            model: instance.transform.to_cols_array_2d(),
            color: instance.color.to_array(),
        }
    }
}
