//! GPU particle VFX: the CPU-side spawn description, per-frame input, tier budgets and
//! statistics (ADR-0013). The passes live in `passes::vfx`.
//!
//! The application emits [`ParticleSpawn`]s (its emitters, event bindings and budgets run on
//! the CPU); the renderer allocates each one a slot in a fixed-capacity GPU particle buffer
//! ([`VfxBudget::max_particles`]) and a compute pass ages, integrates and compacts every live
//! particle each frame. Nothing here allocates per frame, and nothing feeds back into the
//! simulation: VFX are presentation only.

use moba_core::math::{Vec3, Vec4};

/// Mesh slots for mesh particles ([`ParticleShape::Mesh`], [`crate::Renderer::set_vfx_mesh`]).
pub const VFX_MESH_SLOTS: usize = 4;

/// How a particle is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParticleShape {
    /// A camera-facing quad (sprites and flipbooks), rotated by its rotation.
    #[default]
    Billboard,
    /// A camera-facing ribbon segment from the position to position +
    /// [`ParticleSpawn::segment`], `size` wide (trails and beams).
    Segment,
    /// A quad flat on the ground plane, rotated about +Y (rings and area telegraphs).
    Ground,
    /// The mesh bound to [`ParticleSpawn::mesh_slot`], scaled by `size` and tumbling by its
    /// spin. Drawn as a billboard where mesh particles are off (Low) or the slot is empty.
    Mesh,
}

/// How a particle blends into the HDR scene.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParticleBlend {
    /// Added to the scene (order-independent).
    #[default]
    Additive,
    /// Alpha blended: weighted blended order-independent transparency where the tier
    /// enables it ([`VfxBudget::oit`]), unsorted premultiplied blending otherwise.
    Alpha,
}

/// Which readability rule (AAA roadmap §2) applies to a particle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParticleRole {
    /// No rule: the particle draws as authored.
    #[default]
    Neutral,
    /// An allied (not the viewer's own) effect: scaled by [`VfxFrame::ally_opacity`].
    Ally,
    /// An enemy area telegraph: keeps at least [`VfxBudget::telegraph_floor`] opacity until
    /// the last 15 % of its life.
    EnemyTelegraph,
}

/// One particle to spawn this frame. Built with [`ParticleSpawn::new`] and the `with_*`
/// methods; the renderer simulates it on the GPU until its lifetime ends.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct ParticleSpawn {
    /// World position, metres.
    pub position: Vec3,
    /// Initial velocity, metres per second.
    pub velocity: Vec3,
    /// Lifetime, seconds (clamped to `0.01..=10`).
    pub lifetime: f32,
    /// Start size (diameter, or width for segments), metres.
    pub size: f32,
    /// End-of-life size, metres.
    pub size_end: f32,
    /// Start colour, linear RGBA (RGB may exceed 1 for HDR glow).
    pub color: Vec4,
    /// End-of-life colour, linear RGBA.
    pub color_end: Vec4,
    /// Initial rotation, radians.
    pub rotation: f32,
    /// Rotation speed, radians per second.
    pub spin: f32,
    /// Acceleration along +Y, m/s².
    pub gravity: f32,
    /// Linear drag, 1/s.
    pub drag: f32,
    /// How it is drawn.
    pub shape: ParticleShape,
    /// For [`ParticleShape::Segment`]: the other end, relative to `position`.
    pub segment: Vec3,
    /// For [`ParticleShape::Mesh`]: the mesh slot, `0..VFX_MESH_SLOTS`.
    pub mesh_slot: u8,
    /// How it blends.
    pub blend: ParticleBlend,
    /// Readability rule.
    pub role: ParticleRole,
    /// First atlas cell (row-major in the 8×8 atlas grid).
    pub cell: u8,
    /// Flipbook frames from `cell` (1 for a still sprite).
    pub frames: u8,
    /// Flipbook frames per second; `0` spreads the frames over the lifetime.
    pub fps: f32,
}

impl ParticleSpawn {
    /// A still, additive, billboard particle with no velocity that fades to transparent.
    #[must_use]
    pub fn new(position: Vec3, lifetime: f32, size: f32, color: Vec4) -> Self {
        Self {
            position,
            velocity: Vec3::ZERO,
            lifetime,
            size,
            size_end: size,
            color,
            color_end: color.truncate().extend(0.0),
            rotation: 0.0,
            spin: 0.0,
            gravity: 0.0,
            drag: 0.0,
            shape: ParticleShape::Billboard,
            segment: Vec3::ZERO,
            mesh_slot: 0,
            blend: ParticleBlend::Additive,
            role: ParticleRole::Neutral,
            cell: 0,
            frames: 1,
            fps: 0.0,
        }
    }

    /// Sets the initial velocity.
    #[must_use]
    pub const fn with_velocity(mut self, velocity: Vec3) -> Self {
        self.velocity = velocity;
        self
    }

    /// Sets the end-of-life size.
    #[must_use]
    pub const fn with_size_end(mut self, size_end: f32) -> Self {
        self.size_end = size_end;
        self
    }

    /// Sets the end-of-life colour.
    #[must_use]
    pub const fn with_color_end(mut self, color_end: Vec4) -> Self {
        self.color_end = color_end;
        self
    }

    /// Sets the initial rotation and spin (radians, radians per second).
    #[must_use]
    pub const fn with_rotation(mut self, rotation: f32, spin: f32) -> Self {
        self.rotation = rotation;
        self.spin = spin;
        self
    }

    /// Sets gravity (m/s² along +Y) and linear drag (1/s).
    #[must_use]
    pub const fn with_forces(mut self, gravity: f32, drag: f32) -> Self {
        self.gravity = gravity;
        self.drag = drag;
        self
    }

    /// Draws the particle as `shape`.
    #[must_use]
    pub const fn with_shape(mut self, shape: ParticleShape) -> Self {
        self.shape = shape;
        self
    }

    /// Draws the particle as a segment to `position + end`.
    #[must_use]
    pub const fn with_segment(mut self, end: Vec3) -> Self {
        self.shape = ParticleShape::Segment;
        self.segment = end;
        self
    }

    /// Draws the particle as the mesh in `slot`.
    #[must_use]
    pub const fn with_mesh(mut self, slot: u8) -> Self {
        self.shape = ParticleShape::Mesh;
        self.mesh_slot = slot;
        self
    }

    /// Sets the blend mode.
    #[must_use]
    pub const fn with_blend(mut self, blend: ParticleBlend) -> Self {
        self.blend = blend;
        self
    }

    /// Sets the readability role.
    #[must_use]
    pub const fn with_role(mut self, role: ParticleRole) -> Self {
        self.role = role;
        self
    }

    /// Sets the atlas sprite: first cell, flipbook frames and frames per second.
    #[must_use]
    pub const fn with_sprite(mut self, cell: u8, frames: u8, fps: f32) -> Self {
        self.cell = cell;
        self.frames = frames;
        self.fps = fps;
        self
    }
}

/// This frame's VFX input ([`crate::Scene::with_vfx`]).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct VfxFrame<'a> {
    /// Particles to spawn. At most [`VfxBudget::max_spawns_per_frame`] are taken, in order
    /// (so put the ones that must not drop, such as enemy telegraphs, first); the rest, and
    /// any that find no free slot, are dropped and counted ([`VfxStats::dropped`]).
    pub spawns: &'a [ParticleSpawn],
    /// Seconds to advance every live particle (`0` freezes them, as a pinned capture does).
    pub dt: f32,
    /// Opacity multiplier for [`ParticleRole::Ally`] particles, `0..=1` (the allied-VFX
    /// dimming setting). Default `1`.
    pub ally_opacity: f32,
}

impl<'a> VfxFrame<'a> {
    /// No spawns and no time step: live particles stay frozen.
    pub const EMPTY: VfxFrame<'static> = VfxFrame {
        spawns: &[],
        dt: 0.0,
        ally_opacity: 1.0,
    };

    /// Spawns `spawns` and advances live particles by `dt` seconds.
    #[must_use]
    pub const fn new(spawns: &'a [ParticleSpawn], dt: f32) -> Self {
        Self {
            spawns,
            dt,
            ally_opacity: 1.0,
        }
    }

    /// Sets the allied-VFX opacity.
    #[must_use]
    pub const fn with_ally_opacity(mut self, ally_opacity: f32) -> Self {
        self.ally_opacity = ally_opacity;
        self
    }
}

impl Default for VfxFrame<'_> {
    fn default() -> Self {
        VfxFrame::EMPTY
    }
}

/// Per-tier VFX budgets ([`crate::QualityTier::vfx_budget`]).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct VfxBudget {
    /// GPU particle capacity: the hard cap, allocated once at renderer creation.
    pub max_particles: u32,
    /// Most spawns accepted per frame (the per-frame upload buffer's capacity).
    pub max_spawns_per_frame: u32,
    /// Overdraw budget: particle pixels per screen pixel. The application culls
    /// low-importance effects past it (the renderer reports the measured value in
    /// [`VfxStats::overdraw`]).
    pub max_overdraw: f32,
    /// Soft particles fade against the scene depth.
    pub soft_particles: bool,
    /// Alpha particles use weighted blended OIT (two extra screen-sized targets).
    pub oit: bool,
    /// Mesh particles draw their meshes (else a shard billboard).
    pub mesh_particles: bool,
    /// Effects farther than this from the camera eye are culled (metres), telegraphs excepted.
    pub max_distance: f32,
    /// Minimum opacity of enemy telegraphs.
    pub telegraph_floor: f32,
}

/// Measured VFX statistics of the latest completed frame ([`crate::Renderer::vfx_stats`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct VfxStats {
    /// Sequence number of the measured frame (as [`crate::GpuTimings::frame`]).
    pub frame: u64,
    /// Live particles drawn.
    pub alive: u32,
    /// Of which sprites, flipbooks, segments and ground quads.
    pub sprites: u32,
    /// Of which mesh particles.
    pub meshes: u32,
    /// Particles spawned that frame.
    pub spawned: u32,
    /// Spawns dropped that frame (over the per-frame cap or no free slot).
    pub dropped: u32,
    /// Sum of the live particles' projected screen areas, pixels (computed on the GPU).
    pub pixels: u64,
    /// `pixels` per screen pixel: the fill-rate (overdraw) estimate.
    pub overdraw: f32,
}
