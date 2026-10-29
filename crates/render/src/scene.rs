//! Presentation-side scene data: camera and per-instance transforms.

use bytemuck::{Pod, Zeroable};
use moba_assets::JointMatrix;
use moba_core::math::{Mat4, Quat, Vec3, Vec4, look_at_rh, orthographic_rh, perspective_rh};

use crate::ui::UiImage;

/// A pinhole camera with Vulkan clip-space conventions (`z` in `[0, 1]`).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
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
        let view = look_at_rh(eye, target, up);
        let projection = perspective_rh(fov_y_radians, aspect.max(1e-4), near, far);
        Self {
            view_proj: projection * view,
            position: eye,
            light_direction: Vec3::new(0.45, 1.0, 0.35).normalize(),
            light_intensity: 1.15,
            ambient: Vec3::new(0.32, 0.35, 0.42),
        }
    }

    /// Builds an orthographic camera looking from `eye` at `target`: the view box is
    /// `half_width` x `half_height` around the view axis and `near..far` along it. Used for
    /// offscreen views (R7b): the top-down minimap and the portrait studio.
    #[must_use]
    pub fn orthographic(
        eye: Vec3,
        target: Vec3,
        up: Vec3,
        half_width: f32,
        half_height: f32,
        near: f32,
        far: f32,
    ) -> Self {
        let view = look_at_rh(eye, target, up);
        let half_width = half_width.max(1e-4);
        let half_height = half_height.max(1e-4);
        let projection = orthographic_rh(
            -half_width,
            half_width,
            -half_height,
            half_height,
            near,
            far,
        );
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

/// Handle to a mesh owned by the renderer.
///
/// [`MeshId::CUBE`] (id 0) is the built-in unit cube used by every procedural shape. Other
/// ids come from [`Renderer::register_mesh`](crate::Renderer::register_mesh) and stay valid
/// for the renderer's lifetime. An id the renderer does not know draws as the cube.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MeshId(u16);

impl MeshId {
    /// The built-in unit cube.
    pub const CUBE: Self = Self(0);

    pub(crate) const fn from_index(index: u16) -> Self {
        Self(index)
    }

    /// Slot index inside the renderer's mesh table.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// The joints of one skinned instance inside [`Scene::bones`].
///
/// [`BoneRange::NONE`] (the default) means the instance is not skinned. A skinned mesh
/// instance without a valid range (empty, or past the bones uploaded this frame) is not
/// drawn and is counted in [`skinned_instances_dropped`](crate::skinned_instances_dropped).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoneRange {
    /// Index of the instance's first joint matrix in [`Scene::bones`].
    pub first: u32,
    /// Number of joint matrices (the skeleton's joint count).
    pub count: u32,
}

impl BoneRange {
    /// No bones: a static instance.
    pub const NONE: Self = Self { first: 0, count: 0 };

    /// `count` joint matrices starting at `first`.
    #[must_use]
    pub const fn new(first: u32, count: u32) -> Self {
        Self { first, count }
    }
}

/// One drawable unit: a model matrix, a colour and the mesh to draw.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct UnitInstance {
    /// Model-to-world transform.
    pub transform: Mat4,
    /// Linear-space RGBA colour.
    pub color: Vec4,
    /// Mesh drawn for this instance ([`MeshId::CUBE`] by default).
    pub mesh: MeshId,
    /// Joint matrices for a skinned mesh ([`BoneRange::NONE`] for static meshes).
    pub bones: BoneRange,
}

impl UnitInstance {
    /// Creates a cube instance from a transform and colour.
    #[must_use]
    pub const fn new(transform: Mat4, color: Vec4) -> Self {
        Self {
            transform,
            color,
            mesh: MeshId::CUBE,
            bones: BoneRange::NONE,
        }
    }

    /// Returns this instance drawn with `mesh` instead.
    #[must_use]
    pub const fn with_mesh(mut self, mesh: MeshId) -> Self {
        self.mesh = mesh;
        self
    }

    /// Returns this instance posed by `bones` (for a mesh from
    /// [`Renderer::register_skinned_mesh`](crate::Renderer::register_skinned_mesh)).
    #[must_use]
    pub const fn with_bones(mut self, bones: BoneRange) -> Self {
        self.bones = bones;
        self
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
            mesh: MeshId::CUBE,
            bones: BoneRange::NONE,
        }
    }
}

/// Everything the renderer draws in one frame.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Scene<'a> {
    /// Camera for this frame.
    pub camera: Camera,
    /// Units to draw, in any order.
    pub units: &'a [UnitInstance],
    /// Allied vision circles; an empty set disables fog darkening.
    pub vision: VisionCircles,
    /// Terrain tint: RGB (`1.0` = default grass palette) and, in alpha, the strength of the
    /// terrain's grid lines (`1.0` = the default; R7b's baked minimap uses a fainter grid).
    pub terrain_tint: [f32; 4],
    /// Joint matrices for skinned instances, indexed by [`UnitInstance::bones`]. Up to
    /// [`RenderConfig::max_bones`](crate::RenderConfig::max_bones) are uploaded per frame.
    pub bones: &'a [JointMatrix],
}

/// A rectangle of an offscreen image cleared to its own colour before anything is drawn
/// (R7b: the team-coloured backdrop of each hero portrait).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClearRect {
    /// `[x, y, width, height]` in the image's pixels, top-left origin; clamped to the image.
    pub rect: [u32; 4],
    /// Linear HDR RGBA colour (tonemapped with the image when the UI samples it).
    pub color: [f32; 4],
}

impl ClearRect {
    /// A rectangle of `color`.
    #[must_use]
    pub const fn new(rect: [u32; 4], color: [f32; 4]) -> Self {
        Self { rect, color }
    }
}

/// One render into an offscreen [`UiImage`] this frame (R7b), for
/// [`Renderer::render_with_offscreen`](crate::Renderer::render_with_offscreen).
///
/// The scene is drawn with the main scene's pipelines, meshes and materials into the image's
/// colour target (the HDR format) and its own depth buffer, through its own render-graph
/// pass. Its camera, units and bones are uploaded to the image's own buffers, which hold up
/// to [`OFFSCREEN_MAX_INSTANCES`] units and [`OFFSCREEN_MAX_BONES`] joint matrices (the
/// rest are dropped). The image keeps the result until the next render into it.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct OffscreenRender<'a> {
    /// The image to render into.
    pub image: UiImage,
    /// Camera, units, bones and terrain tint. Pass empty vision for no fog darkening.
    pub scene: Scene<'a>,
    /// Draw the terrain plane (the minimap does; portraits do not). Default `false`.
    pub terrain: bool,
    /// Linear HDR colour the whole image is cleared to first. Default opaque black.
    pub clear_color: [f32; 4],
    /// Rectangles then cleared to their own colours (up to 32). Default none.
    pub backdrops: &'a [ClearRect],
}

/// Units uploaded per [`OffscreenRender`]; more are dropped in scene order.
pub const OFFSCREEN_MAX_INSTANCES: usize = 1_024;
/// Joint matrices uploaded per [`OffscreenRender`] (16 skinned heroes × 64 joints).
pub const OFFSCREEN_MAX_BONES: usize = 16 * 64;

impl<'a> OffscreenRender<'a> {
    /// Renders `scene` into `image`, without terrain, over opaque black.
    #[must_use]
    pub const fn new(image: UiImage, scene: Scene<'a>) -> Self {
        Self {
            image,
            scene,
            terrain: false,
            clear_color: [0.0, 0.0, 0.0, 1.0],
            backdrops: &[],
        }
    }

    /// Sets whether the terrain plane is drawn.
    #[must_use]
    pub const fn with_terrain(mut self, terrain: bool) -> Self {
        self.terrain = terrain;
        self
    }

    /// Sets the colour the image is cleared to.
    #[must_use]
    pub const fn with_clear_color(mut self, color: [f32; 4]) -> Self {
        self.clear_color = color;
        self
    }

    /// Sets the backdrop rectangles cleared after the full clear.
    #[must_use]
    pub const fn with_backdrops(mut self, backdrops: &'a [ClearRect]) -> Self {
        self.backdrops = backdrops;
        self
    }
}

/// Up to 16 allied vision circles used to darken unseen terrain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VisionCircles {
    /// World-space circles: `(x, z, radius, unused)`.
    pub circles: [Vec4; VISION_CIRCLES],
    /// Number of active circles.
    pub count: u32,
    /// Vision-blocking segments: `(x1, z1, x2, z2)`.
    pub walls: [Vec4; VISION_WALLS],
    /// Number of active walls.
    pub wall_count: u32,
}

/// Maximum vision circles uploaded per frame.
pub const VISION_CIRCLES: usize = 16;
/// Maximum vision-blocking walls uploaded per frame.
pub const VISION_WALLS: usize = 32;

impl Default for VisionCircles {
    fn default() -> Self {
        Self {
            circles: [Vec4::ZERO; VISION_CIRCLES],
            count: 0,
            walls: [Vec4::ZERO; VISION_WALLS],
            wall_count: 0,
        }
    }
}

impl VisionCircles {
    /// Adds a world-space circle, saturating at [`VISION_CIRCLES`].
    pub fn add(&mut self, x: f32, z: f32, radius: f32) {
        if self.count as usize >= VISION_CIRCLES {
            return;
        }
        self.circles[self.count as usize] = Vec4::new(x, z, radius.max(0.0), 0.0);
        self.count += 1;
    }

    /// Adds a vision-blocking wall segment, saturating at [`VISION_WALLS`].
    pub fn add_wall(&mut self, x1: f32, z1: f32, x2: f32, z2: f32) {
        if self.wall_count as usize >= VISION_WALLS {
            return;
        }
        self.walls[self.wall_count as usize] = Vec4::new(x1, z1, x2, z2);
        self.wall_count += 1;
    }
}

impl<'a> Scene<'a> {
    /// Creates a scene without fog-of-war darkening.
    #[must_use]
    pub const fn new(camera: Camera, units: &'a [UnitInstance]) -> Self {
        Self {
            camera,
            units,
            vision: VisionCircles {
                circles: [Vec4::ZERO; VISION_CIRCLES],
                count: 0,
                walls: [Vec4::ZERO; VISION_WALLS],
                wall_count: 0,
            },
            terrain_tint: [0.10, 0.16, 0.11, 1.0],
            bones: &[],
        }
    }

    /// Creates a scene with allied vision circles for terrain fog.
    #[must_use]
    pub const fn with_vision(
        camera: Camera,
        units: &'a [UnitInstance],
        vision: VisionCircles,
    ) -> Self {
        Self {
            camera,
            units,
            vision,
            terrain_tint: [0.10, 0.16, 0.11, 1.0],
            bones: &[],
        }
    }

    /// Sets the terrain tint.
    #[must_use]
    pub const fn with_terrain_tint(mut self, tint: [f32; 4]) -> Self {
        self.terrain_tint = tint;
        self
    }

    /// Sets the joint matrices that skinned instances index.
    #[must_use]
    pub const fn with_bones(mut self, bones: &'a [JointMatrix]) -> Self {
        self.bones = bones;
        self
    }

    /// Creates an empty scene with a camera (used for clear-only frames and tests).
    #[must_use]
    pub const fn empty(camera: Camera) -> Self {
        Self::new(camera, &[])
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
    terrain: [f32; 4],
    vision_circles: [[f32; 4]; VISION_CIRCLES],
    vision_count: [u32; 4],
    vision_walls: [[f32; 4]; VISION_WALLS],
    wall_count: [u32; 4],
}

impl CameraUniform {
    pub(crate) fn new(camera: Camera, vision: &VisionCircles, terrain: [f32; 4]) -> Self {
        let mut circles = [[0.0; 4]; VISION_CIRCLES];
        for (destination, source) in circles.iter_mut().zip(vision.circles) {
            *destination = source.to_array();
        }
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
            terrain,
            vision_circles: circles,
            vision_count: [vision.count, 0, 0, 0],
            vision_walls: vision.walls.map(|wall| wall.to_array()),
            wall_count: [vision.wall_count, 0, 0, 0],
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
