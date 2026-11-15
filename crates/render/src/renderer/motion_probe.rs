//! The motion-vector probe (ADR-0021 verification gate 2): a scripted two-frame scene whose
//! motion target is read back and compared, pixel by pixel, with a CPU twin.
//!
//! Frame A, then frame B with the camera panned and every object moved:
//!
//! - **camera**: the terrain (motion key 0);
//! - **static**: a keyed cube translated;
//! - **skinned**: a keyed one-joint skinned cube whose joint moves;
//! - **vat**: a keyed vertex-animation cube going from frame 0 to frame 1 of its texture;
//! - **foliage**: a patch in a steady wind at the same environment time in both frames (the
//!   prepass evaluates every blade again at the previous time: the blades bend, and only the
//!   camera moves them);
//! - **foliage-pushed**: a second patch with a displacer in frame B only. Its blades bend away
//!   from it in B, so their motion must differ from camera-only and point back towards the
//!   displacer. There is no per-blade CPU twin, so this class is checked by direction and
//!   share ([`PushCheck`]), not to 1/64 px: it fails when the prepass drops the second
//!   evaluation at the previous displacers (every pushed blade would then be camera-only);
//! - **reset**: a keyed cube moved with `motion_reset`, **absent-key** a cube with key 0
//!   moved, **new-key** a cube whose key frame A did not draw: all three camera-only.
//!
//! The twin reconstructs each pixel's world position from frame B's depth (through the
//! jittered matrix the depth was rasterised with), takes it back to frame A by the object's
//! known motion, and projects both through the unjittered matrices: exactly what
//! `include/motion.glsl` writes. Every classified pixel must match within
//! [`MOTION_PROBE_TOLERANCE_PX`] render pixels; pixels within a centimetre of an object's
//! boundary, where the class is ambiguous, are skipped and counted.
//!
//! **The G-buffer** (ADR-0023 amendment 1 item 9) is read back with frame B and checked per
//! draw class against what each surface writes in N2a: the unit, skinned and VAT cubes are
//! `STANDARD` with F0 0.04 and `UNIT | NO_FOG_OF_WAR`; the terrain is `STANDARD` with F0 0.04
//! and no flags; the foliage is `FOLIAGE` with no flags; the far plane keeps the clear value
//! (`UNLIT`, every bit 0). Every rim slot is 0. The pixels are classified as for the motion
//! (ground under a foliage patch, where blade bases stand, is left out of `terrain`), and
//! every classified pixel must match ([`GBufferCheck`]). Frames A and B look down on the
//! ground and show no sky, so a frame C, with the camera turned to the horizon, gives the
//! far-plane pixels of the `sky` class.
//!
//! Diagnostic only: the probe registers its meshes and replaces the foliage, so it is meant
//! for a fresh renderer that then exits (the client's `MOBA_MOTION_PROBE=1`).

use core::fmt;

use ash::vk;
use gpu_allocator::MemoryLocation;
use moba_assets::{
    JointMatrix, JointTransform, MeshSkin, Skeleton, f16_from_f32, f32_from_f16, mesh_hash,
    vat_extent,
};
use moba_core::math::{Mat4, Vec3, Vec4};

use super::Renderer;
use crate::environment::{EnvironmentFrame, FoliageData, FoliageDisplacer, FoliageInstanceData};
use crate::error::{RenderError, RenderResult};
use crate::frame_graph::FramePlan;
use crate::gbuffer::{FLAG_NO_FOG_OF_WAR, FLAG_UNIT, GBufferTexels, ShadingModel};
use crate::graph::ResourceId;
use crate::resources::{CommandPool, GpuBuffer};
use crate::scene::{BoneRange, Camera, Scene, UnitInstance};
use crate::temporal::{clip_to_pixels, jittered, motion_pixels};
use crate::{MeshData, TextureData, TextureFormat, VatDescriptor, VatPose, VatSample};

/// The largest difference allowed between a motion vector and its CPU twin, in render
/// pixels (ADR-0021 gate 2).
pub const MOTION_PROBE_TOLERANCE_PX: f32 = 1.0 / 64.0;
/// Pixels each class must cover for its check to count.
const MIN_PIXELS: u32 = 16;

/// The camera's move from frame A to frame B (no cut: ADR-0021 §2c's 8 m and 15°).
const CAMERA_PAN: Vec3 = Vec3::new(0.125, 0.0, -0.0625);
const STATIC_MOVE: Vec3 = Vec3::new(0.125, 0.0, -0.125);
const BONE_MOVE: Vec3 = Vec3::new(-0.125, 0.0, 0.0625);
/// Frame 1 of the probe's VAT minus frame 0 (exact in `f16`).
const VAT_MOVE: Vec3 = Vec3::new(0.0625, 0.0, 0.125);
const CAMERA_ONLY_MOVE: Vec3 = Vec3::new(0.125, 0.0, 0.0);
/// The environment time of both frames.
const FOLIAGE_TIME: f32 = 37.25;
const FOLIAGE_CENTRE: [f32; 2] = [-2.5, 2.5];
/// The pushed patch, and its displacer (frame B only) at the patch's centre.
const PUSHED_CENTRE: [f32; 2] = [-2.5, -2.5];
const PUSH_RADIUS: f32 = 1.5;
/// A pushed pixel's motion counts as moved when it differs from camera-only by this much.
const PUSH_MOVED_PX: f32 = 0.25;
/// Shares of the pushed patch's pixels that must have moved, and of those that must point
/// towards the displacer. Not every moved pixel does: a pushed blade also flattens, so its
/// previous (taller) position is partly *up* the screen, and near the displacer's centre the
/// two directions part (measured on lavapipe: 80 % moved, 81 % of those inward).
const PUSH_MOVED_SHARE: f32 = 0.3;
const PUSH_INWARD_SHARE: f32 = 0.7;
/// Half the side of the square in which a non-terrain, non-cube pixel counts as foliage.
const FOLIAGE_HALF: f32 = 1.75;
/// Distance from a cube's box within which a pixel is ambiguous.
const SHELL: f32 = 0.01;
/// Distance from a cube's box within which a pixel is on the cube.
const ON_BOX: f32 = 0.002;
/// Height under which a pixel off every cube is terrain.
const GROUND: f32 = 0.003;

/// One class of pixels against the twin.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct MotionCheck {
    /// The class (`camera`, `static`, `skinned`, `vat`, `foliage`, `reset`, `absent-key`,
    /// `new-key`).
    pub name: &'static str,
    /// Pixels compared.
    pub pixels: u32,
    /// The largest difference from the twin, render pixels.
    pub max_error: f32,
    /// The mean difference, render pixels.
    pub mean_error: f32,
    /// The twin's mean motion length, render pixels (the check is not vacuous).
    pub mean_motion: f32,
}

impl MotionCheck {
    /// Enough pixels, all within [`MOTION_PROBE_TOLERANCE_PX`].
    #[must_use]
    pub fn passed(&self) -> bool {
        self.pixels >= MIN_PIXELS && self.max_error <= MOTION_PROBE_TOLERANCE_PX
    }
}

/// The pushed foliage patch (frame B's displacer): how many of its pixels moved more than
/// camera-only, and how many of those point back towards the displacer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct PushCheck {
    /// Pixels of the pushed patch.
    pub pixels: u32,
    /// Pixels whose motion differs from camera-only by more than 0.25 px.
    pub moved: u32,
    /// Moved pixels whose extra motion points towards the displacer.
    pub inward: u32,
    /// The mean extra motion of the moved pixels, render pixels.
    pub mean_extra: f32,
}

impl PushCheck {
    /// Enough pixels, at least 30 % moved and at least 70 % of those inward.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.pixels >= MIN_PIXELS
            && self.moved as f32 >= PUSH_MOVED_SHARE * self.pixels as f32
            && self.inward as f32 >= PUSH_INWARD_SHARE * self.moved as f32
    }
}

/// One draw class's G-buffer against what N2a writes (ADR-0023 amendment 1 item 9).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct GBufferCheck {
    /// The class (`unit`, `skinned`, `vat`, `terrain`, `foliage`, `sky`).
    pub name: &'static str,
    /// Pixels compared.
    pub pixels: u32,
    /// Pixels whose model, flags, rim slot or F0 differ from the class's.
    pub mismatches: u32,
    /// The first mismatching pixel's texels, for the log.
    pub first_mismatch: Option<String>,
}

impl GBufferCheck {
    /// Enough pixels, every one as expected.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.pixels >= MIN_PIXELS && self.mismatches == 0
    }
}

/// The result of [`Renderer::motion_probe`].
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct MotionProbeReport {
    /// Frame B's render resolution.
    pub render: [u32; 2],
    /// Frame B's jitter, render pixels.
    pub jitter: [f32; 2],
    /// Per class.
    pub checks: Vec<MotionCheck>,
    /// The pushed foliage patch.
    pub pushed: PushCheck,
    /// The G-buffer per draw class.
    pub gbuffer: Vec<GBufferCheck>,
    /// Pixels skipped as ambiguous (a cube's boundary) or unclassified.
    pub skipped: u32,
}

impl MotionProbeReport {
    /// Every class passed.
    #[must_use]
    pub fn passed(&self) -> bool {
        !self.checks.is_empty()
            && self.checks.iter().all(MotionCheck::passed)
            && self.pushed.passed()
            && self.gbuffer.len() == GBUFFER_CLASSES.len()
            && self.gbuffer.iter().all(GBufferCheck::passed)
    }
}

impl fmt::Display for MotionProbeReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "motion probe: render {}x{}, jitter ({:+.4}, {:+.4}), tolerance {:.5} px, {} skipped",
            self.render[0],
            self.render[1],
            self.jitter[0],
            self.jitter[1],
            MOTION_PROBE_TOLERANCE_PX,
            self.skipped
        )?;
        for check in &self.checks {
            writeln!(
                f,
                "  {} {:<10} pixels={:<7} max_error={:.6} mean_error={:.6} mean_motion={:.3}",
                if check.passed() { "ok  " } else { "FAIL" },
                check.name,
                check.pixels,
                check.max_error,
                check.mean_error,
                check.mean_motion
            )?;
        }
        writeln!(
            f,
            "  {} foliage-pushed pixels={:<7} moved={} ({:.1} %, need {:.0} %) inward={} ({:.1} %, need {:.0} %) mean_extra={:.3}",
            if self.pushed.passed() { "ok  " } else { "FAIL" },
            self.pushed.pixels,
            self.pushed.moved,
            100.0 * self.pushed.moved as f32 / self.pushed.pixels.max(1) as f32,
            100.0 * PUSH_MOVED_SHARE,
            self.pushed.inward,
            100.0 * self.pushed.inward as f32 / self.pushed.moved.max(1) as f32,
            100.0 * PUSH_INWARD_SHARE,
            self.pushed.mean_extra
        )?;
        for check in &self.gbuffer {
            writeln!(
                f,
                "  {} gbuffer {:<8} pixels={:<7} mismatches={}{}",
                if check.passed() { "ok  " } else { "FAIL" },
                check.name,
                check.pixels,
                check.mismatches,
                check
                    .first_mismatch
                    .as_deref()
                    .map_or(String::new(), |first| format!(" first: {first}"))
            )?;
        }
        write!(
            f,
            "motion probe: {}",
            if self.passed() { "passed" } else { "FAILED" }
        )
    }
}

/// A cube of the probe in frame B, and what the twin expects of it.
#[derive(Clone, Copy)]
struct Cube {
    class: usize,
    centre: Vec3,
    /// Frame B minus frame A, world metres (zero for camera-only motion).
    expected: Vec3,
}

const CLASSES: [&str; 8] = [
    "camera",
    "static",
    "skinned",
    "vat",
    "foliage",
    "reset",
    "absent-key",
    "new-key",
];
const CAMERA: usize = 0;
const FOLIAGE: usize = 4;
/// The pushed patch: not a `CLASSES` entry, checked by [`PushCheck`].
const PUSHED: usize = usize::MAX;

/// What one draw class writes into the G-buffer (ADR-0023 amendment 1 item 9).
#[derive(Clone, Copy, Debug)]
struct GBufferExpect {
    name: &'static str,
    model: ShadingModel,
    flags: u32,
    /// F0 0.04 in the model data (`STANDARD`).
    f0: bool,
    /// The linear base colour `gb0` holds, when the probe knows it (the units' instance
    /// colour; untextured, so the albedo is that colour). Every class has metallic 0.
    base_color: Option<[f32; 3]>,
}

const fn expect(name: &'static str, model: ShadingModel, flags: u32, f0: bool) -> GBufferExpect {
    GBufferExpect {
        name,
        model,
        flags,
        f0,
        base_color: None,
    }
}

const fn unit_expect(name: &'static str) -> GBufferExpect {
    GBufferExpect {
        base_color: Some(UNIT_COLOUR),
        ..expect(name, ShadingModel::Standard, UNIT_FLAGS, true)
    }
}

/// Every probe unit's instance colour. Its blue lands in `gb0`'s blue channel, which the
/// lavapipe write-mask fault dropped (ADR-0023 amendment 1 item 16).
const UNIT_COLOUR: [f32; 3] = [0.75, 0.35, 0.2];
const UNIT_FLAGS: u32 = FLAG_UNIT | FLAG_NO_FOG_OF_WAR;
const GBUFFER_CLASSES: [GBufferExpect; 6] = [
    unit_expect("unit"),
    unit_expect("skinned"),
    unit_expect("vat"),
    expect("terrain", ShadingModel::Standard, 0, true),
    expect("foliage", ShadingModel::Foliage, 0, false),
    expect("sky", ShadingModel::Unlit, 0, false),
];
const GB_TERRAIN: usize = 3;
const GB_FOLIAGE: usize = 4;
const GB_SKY: usize = 5;
/// The F0 0.04 default stores 128 (`sqrt(F0 / 0.16)`), which decodes to 0.0403; one step is
/// 0.0006.
const F0_TOLERANCE: f32 = 0.0007;

/// The G-buffer class of a motion class (`PUSHED` is foliage).
fn gbuffer_class(class: usize) -> Option<usize> {
    match class {
        CAMERA => Some(GB_TERRAIN),
        1 | 5 | 6 | 7 => Some(0),
        2 => Some(1),
        3 => Some(2),
        FOLIAGE | PUSHED => Some(GB_FOLIAGE),
        _ => None,
    }
}

/// Whether `texels` hold what `expect` writes: model, flags, rim slot and F0 (`gb1`,
/// `gb2`), metallic 0 and any known base colour (`gb0`, each channel within one step, the
/// UNORM rounding the spec allows), and all zero for the sky.
fn gbuffer_matches(expect: &GBufferExpect, texels: GBufferTexels) -> bool {
    let Some(surface) = crate::gbuffer::decode(texels) else {
        return false;
    };
    let clear = texels == GBufferTexels::default();
    let base_color = expect.base_color.is_none_or(|base_color| {
        let want = crate::gbuffer::encode(&crate::gbuffer::SurfaceParams {
            base_color,
            ..surface
        });
        want.gb0[..3]
            .iter()
            .zip(&texels.gb0[..3])
            .all(|(&want, &got)| want.abs_diff(got) <= 1)
    });
    surface.model == expect.model
        && surface.flags == expect.flags
        && surface.rim_slot == 0
        && texels.gb0[3] == 0
        && base_color
        && (!expect.f0 || (surface.model_data[0] - 0.04).abs() <= F0_TOLERANCE)
        && (expect.model != ShadingModel::Unlit || clear)
}

#[derive(Clone, Copy, Default)]
struct GBufferTally {
    pixels: u32,
    mismatches: u32,
    first: Option<(usize, usize, GBufferTexels)>,
}

impl GBufferTally {
    fn add(&mut self, expect: &GBufferExpect, x: usize, y: usize, texels: GBufferTexels) {
        self.pixels += 1;
        if !gbuffer_matches(expect, texels) {
            self.mismatches += 1;
            self.first.get_or_insert((x, y, texels));
        }
    }
}

/// Frame B's targets as read back, row-major at the render resolution.
struct Readback {
    /// Render pixels.
    motion: Vec<[f32; 2]>,
    depth: Vec<f32>,
    /// `gb0`, `gb1` and `gb2`.
    gbuffer: Vec<GBufferTexels>,
}

#[derive(Clone, Copy, Default)]
struct Tally {
    pixels: u32,
    max_error: f32,
    sum_error: f64,
    sum_motion: f64,
}

/// A unit cube (24 vertices, one normal per face) wound as the built-in cube.
fn cube_mesh() -> (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<u32>) {
    let mut positions = Vec::with_capacity(24);
    let mut normals = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (normal, along) in [
        (Vec3::X, Vec3::Y),
        (-Vec3::X, Vec3::Y),
        (Vec3::Y, Vec3::Z),
        (-Vec3::Y, Vec3::Z),
        (Vec3::Z, Vec3::X),
        (-Vec3::Z, Vec3::X),
    ] {
        let across = normal.cross(along);
        let base = u32::try_from(positions.len()).expect("24 vertices");
        for (u, v) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let corner = (normal + across * u + along * v) * 0.5;
            positions.push(corner.to_array());
            normals.push(normal.to_array());
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (positions, normals, indices)
}

fn probe_error(message: &str) -> RenderError {
    RenderError::NoSuitableDevice(format!("motion probe: {message}"))
}

/// A translation as a joint matrix (three rows of a row-major affine).
fn joint_translation(offset: Vec3) -> JointMatrix {
    [
        [1.0, 0.0, 0.0, offset.x],
        [0.0, 1.0, 0.0, offset.y],
        [0.0, 0.0, 1.0, offset.z],
    ]
}

/// A two-frame VAT of `positions` whose frame 1 is frame 0 moved by [`VAT_MOVE`].
fn vat_assets(
    mesh: &MeshData,
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
) -> RenderResult<(TextureData, VatDescriptor)> {
    let vertex_count = u32::try_from(positions.len()).expect("24 vertices");
    let (width, height) = vat_extent(vertex_count, 2).ok_or_else(|| probe_error("VAT extent"))?;
    let texels = usize::try_from(width * height).expect("small");
    let mut bytes = Vec::with_capacity(texels * 8);
    let mut push = |value: [f32; 4]| {
        for lane in value {
            bytes.extend_from_slice(&f16_from_f32(lane).to_le_bytes());
        }
    };
    for frame in 0..2u8 {
        let offset = VAT_MOVE * f32::from(frame);
        for position in positions {
            let moved = Vec3::from_array(*position) + offset;
            push([moved.x, moved.y, moved.z, 1.0]);
        }
    }
    for _ in 0..2 {
        for normal in normals {
            push([normal[0], normal[1], normal[2], 0.0]);
        }
    }
    bytes.resize(texels * 8, 0);
    let texture = TextureData::new(TextureFormat::Rgba16Float, width, height, &[bytes])
        .map_err(|_| probe_error("VAT texture"))?;
    let descriptor = VatDescriptor::parse(&format!(
        "model = \"motion-probe\"\nvertex_count = {vertex_count}\nframe_count = 2\n\
         width = {width}\nheight = {height}\nfps = 10.0\nmesh_hash = \"0x{:016x}\"\n\n\
         [[clips]]\nname = \"idle\"\nfirst = 0\nframes = 2\nloop = true\n",
        mesh_hash(mesh)
    ))
    .map_err(|_| probe_error("VAT descriptor"))?;
    Ok((texture, descriptor))
}

/// Depth texel bytes as copied out of the `DEPTH` aspect.
fn depth_texel_bytes(format: vk::Format) -> u64 {
    match format {
        vk::Format::D16_UNORM | vk::Format::D16_UNORM_S8_UINT => 2,
        _ => 4,
    }
}

/// One depth texel as a `[0, 1]` depth.
fn decode_depth(format: vk::Format, bytes: &[u8]) -> f32 {
    match format {
        vk::Format::D16_UNORM | vk::Format::D16_UNORM_S8_UINT => {
            f32::from(u16::from_le_bytes([bytes[0], bytes[1]])) / 65535.0
        }
        vk::Format::D24_UNORM_S8_UINT | vk::Format::X8_D24_UNORM_PACK32 => {
            let raw = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) & 0x00ff_ffff;
            raw as f32 / 16_777_215.0
        }
        _ => f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
    }
}

/// The physical image of transient `resource` and the layout the frame left it in.
fn final_state(
    renderer: &Renderer,
    plan: &FramePlan,
    resource: ResourceId,
    what: &str,
) -> RenderResult<(vk::Image, vk::ImageLayout)> {
    let graph = &plan.graph;
    let slot = graph
        .slot_of(resource)
        .ok_or_else(|| probe_error(&format!("{what} is not a live transient")))?;
    let last = graph
        .passes()
        .iter()
        .rev()
        .flat_map(|pass| pass.barriers.iter().rev())
        .find(|barrier| barrier.image && graph.slot_of(barrier.resource) == Some(slot))
        .ok_or_else(|| probe_error(&format!("{what} has no barrier")))?;
    if last.resource != resource {
        return Err(probe_error(&format!(
            "{what} shares its memory with a later transient"
        )));
    }
    Ok((renderer.transients.image(slot).image(), last.new_layout))
}

fn layout_barrier(
    image: vk::Image,
    aspect: vk::ImageAspectFlags,
    from: (vk::ImageLayout, vk::PipelineStageFlags2, vk::AccessFlags2),
    to: (vk::ImageLayout, vk::PipelineStageFlags2, vk::AccessFlags2),
) -> vk::ImageMemoryBarrier2<'static> {
    vk::ImageMemoryBarrier2::default()
        .src_stage_mask(from.1)
        .src_access_mask(from.2)
        .dst_stage_mask(to.1)
        .dst_access_mask(to.2)
        .old_layout(from.0)
        .new_layout(to.0)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(aspect)
                .level_count(1)
                .layer_count(1),
        )
}

impl Renderer {
    /// Runs the motion-vector probe (ADR-0021 verification gate 2): renders the scripted
    /// frames A and B (module docs of `renderer/motion_probe.rs`), reads frame B's motion
    /// and depth back and compares them with the CPU twin.
    ///
    /// Diagnostic only: it registers its meshes and replaces the foliage, so run it on a
    /// fresh renderer that is then dropped.
    ///
    /// # Errors
    /// When temporal anti-aliasing is off (no motion target), the skinned or VAT pipeline is
    /// missing, a frame is skipped, or from resource creation and submission.
    #[allow(clippy::too_many_lines)] // Assets, two scenes, readback and the twin, in order.
    pub fn motion_probe(&mut self) -> RenderResult<MotionProbeReport> {
        if self.temporal.is_none() || self.plans[0].resources.temporal.is_none() {
            return Err(probe_error(
                "temporal anti-aliasing is off: no motion target",
            ));
        }
        if !self.frame_spec.readback {
            return Err(probe_error(
                "the renderer was created without RenderConfig::readback",
            ));
        }
        let (positions, normals, indices) = cube_mesh();
        let plain = MeshData::new(positions.clone(), normals.clone(), indices.clone())
            .map_err(|_| probe_error("cube mesh"))?;
        let skeleton = Skeleton::new(
            vec![None],
            vec![JointTransform::IDENTITY],
            vec![Mat4::IDENTITY],
            vec![Mat4::IDENTITY],
            None,
        )
        .map_err(|_| probe_error("skeleton"))?;
        let skin = MeshSkin::new(vec![[0; 4]; 24], vec![[255, 0, 0, 0]; 24], skeleton)
            .map_err(|_| probe_error("skin"))?;
        let skinned_data = plain
            .clone()
            .with_skin(skin)
            .map_err(|_| probe_error("skinned mesh"))?;
        let skinned = self.register_skinned_mesh(&skinned_data)?;
        let (texture, descriptor) = vat_assets(&plain, &positions, &normals)?;
        let vat = self.register_vat_mesh(&plain, &texture, &descriptor)?;

        let mut clumps = Vec::with_capacity(72);
        for centre in [FOLIAGE_CENTRE, PUSHED_CENTRE] {
            for row in 0..6u8 {
                for column in 0..6u8 {
                    clumps.push(FoliageInstanceData::new(
                        [
                            centre[0] - 0.875 + 0.35 * f32::from(column),
                            0.0,
                            centre[1] - 0.875 + 0.35 * f32::from(row),
                        ],
                        0.7 * f32::from(row * 6 + column),
                        0.9,
                        0,
                        1.0,
                    ));
                }
            }
        }
        self.set_foliage(&FoliageData::new(&clumps))?;

        let [width, height] = [self.swapchain.extent.width, self.swapchain.extent.height];
        let aspect = width as f32 / height.max(1) as f32;
        let camera = |pan: Vec3| {
            Camera::perspective(
                Vec3::new(0.0, 13.0, 9.0) + pan,
                Vec3::new(0.0, 0.0, 0.5) + pan,
                Vec3::Y,
                0.8,
                aspect,
                1.0,
                60.0,
            )
        };
        let (camera_a, camera_b) = (camera(Vec3::ZERO), camera(CAMERA_PAN));
        let colour = Vec3::from_array(UNIT_COLOUR).extend(1.0);
        let at = |centre: Vec3| UnitInstance::new(Mat4::from_translation(centre), colour);
        let static_a = Vec3::new(2.5, 0.5, 0.0);
        let skinned_at = Vec3::new(0.0, 0.5, -2.5);
        let vat_at = Vec3::new(-2.5, 0.5, 0.0);
        let reset_a = Vec3::new(2.5, 0.5, -2.5);
        let absent_a = Vec3::new(0.0, 0.5, 2.5);
        let new_key = Vec3::new(2.5, 0.5, 2.5);
        let pose = |frame: u32| VatPose::single(VatSample::new(frame, frame, 0.0));
        let units = |b: bool| {
            let moved = |start: Vec3, by: Vec3| if b { start + by } else { start };
            let mut units = vec![
                at(moved(static_a, STATIC_MOVE)).with_motion(1, false),
                at(skinned_at)
                    .with_mesh(skinned)
                    .with_bones(BoneRange::new(0, 1))
                    .with_motion(2, false),
                at(vat_at)
                    .with_mesh(vat)
                    .with_vat(pose(u32::from(b)))
                    .with_motion(3, false),
                at(moved(reset_a, CAMERA_ONLY_MOVE)).with_motion(4, b),
                at(moved(absent_a, -CAMERA_ONLY_MOVE)).with_motion(0, false),
            ];
            if b {
                units.push(at(new_key).with_motion(5, false));
            }
            units
        };
        let (units_a, units_b) = (units(false), units(true));
        let bones_a = [joint_translation(Vec3::ZERO)];
        let bones_b = [joint_translation(BONE_MOVE)];
        let scene_a = Scene::new(camera_a, &units_a)
            .with_bones(&bones_a)
            .with_environment(EnvironmentFrame::new(FOLIAGE_TIME, &[]));
        let push = [FoliageDisplacer::new(PUSHED_CENTRE, PUSH_RADIUS, 1.0)];
        let scene_b = Scene::new(camera_b, &units_b)
            .with_bones(&bones_b)
            .with_environment(EnvironmentFrame::new(FOLIAGE_TIME, &push));
        // Frame A a few times (the first frames build pipelines and may be skipped), then B.
        let mut drawn = 0;
        for _ in 0..8 {
            if self.render_internal(&scene_a, None, &[])? {
                drawn += 1;
                if drawn == 3 {
                    break;
                }
            }
        }
        let swapchain_a = (self.swapchain.handle(), self.render_extent);
        if drawn < 3 || !self.render_internal(&scene_b, None, &[])? {
            return Err(probe_error("a probe frame was skipped"));
        }
        // A present that re-created the swapchain would leave fresh transients and a reset
        // history behind; the readback would then compare against nothing frame B wrote.
        // Frame B uses plan 0 (no capture, no offscreen view, no haze in the probe scene), whose
        // final layouts the readback assumes.
        if (self.swapchain.handle(), self.render_extent) != swapchain_a {
            return Err(probe_error("the swapchain was re-created during the probe"));
        }
        let jitter = self
            .temporal
            .as_ref()
            .and_then(super::TemporalState::previous_jitter)
            .ok_or_else(|| probe_error("frame B left no jitter"))?;
        let render = self.render_extent;
        let Readback {
            motion,
            depth,
            gbuffer,
        } = self.read_targets()?;
        let mut gbuffer_tallies = [GBufferTally::default(); GBUFFER_CLASSES.len()];

        let cubes = [
            Cube {
                class: 1,
                centre: static_a + STATIC_MOVE,
                expected: STATIC_MOVE,
            },
            Cube {
                class: 2,
                centre: skinned_at + BONE_MOVE,
                expected: BONE_MOVE,
            },
            Cube {
                class: 3,
                centre: vat_at + VAT_MOVE,
                expected: VAT_MOVE,
            },
            Cube {
                class: 5,
                centre: reset_a + CAMERA_ONLY_MOVE,
                expected: Vec3::ZERO,
            },
            Cube {
                class: 6,
                centre: absent_a - CAMERA_ONLY_MOVE,
                expected: Vec3::ZERO,
            },
            Cube {
                class: 7,
                centre: new_key,
                expected: Vec3::ZERO,
            },
        ];
        // The matrix the depth was rasterised with, in the frame's depth convention (reversed
        // on Tiered, ADR-0023 §1c).
        let depth_convention = self.frame_spec.depth;
        let raster = depth_convention.view_proj(
            jittered(camera_b.view_proj, jitter, render),
            camera_b.near,
            camera_b.far,
        );
        let unproject = raster.inverse();
        let mut tallies = [Tally::default(); CLASSES.len()];
        let mut pushed = PushCheck::default();
        let mut extra_sum = 0.0f64;
        let displacer_at = Vec3::new(PUSHED_CENTRE[0], 0.0, PUSHED_CENTRE[1]);
        let mut skipped = 0u32;
        let (w, h) = (render.width as usize, render.height as usize);
        for y in 0..h {
            for x in 0..w {
                let index = y * w + x;
                let d = depth[index];
                if !depth_convention.nearer(d, depth_convention.far()) {
                    gbuffer_tallies[GB_SKY].add(&GBUFFER_CLASSES[GB_SKY], x, y, gbuffer[index]);
                    skipped += 1;
                    continue;
                }
                let ndc = Vec4::new(
                    (x as f32 + 0.5) / render.width as f32 * 2.0 - 1.0,
                    1.0 - (y as f32 + 0.5) / render.height as f32 * 2.0,
                    d,
                    1.0,
                );
                let world = unproject * ndc;
                let world = world.truncate() / world.w;
                let distance = |cube: &Cube| {
                    let outside = ((world - cube.centre).abs() - Vec3::splat(0.5)).max(Vec3::ZERO);
                    outside.length()
                };
                let near = cubes.iter().find(|cube| distance(cube) < SHELL);
                let (class, expected) = match near {
                    Some(cube) if distance(cube) <= ON_BOX && world.y > SHELL => {
                        (cube.class, cube.expected)
                    }
                    // The pushed patch's ground pixels include blade bases that the push
                    // moves: neither terrain nor reliably foliage.
                    None if (world.x - PUSHED_CENTRE[0]).abs() < FOLIAGE_HALF
                        && (world.z - PUSHED_CENTRE[1]).abs() < FOLIAGE_HALF
                        && world.y.abs() < GROUND =>
                    {
                        skipped += 1;
                        continue;
                    }
                    None if world.y.abs() < GROUND => (CAMERA, Vec3::ZERO),
                    None if world.y > GROUND
                        && (world.x - FOLIAGE_CENTRE[0]).abs() < FOLIAGE_HALF
                        && (world.z - FOLIAGE_CENTRE[1]).abs() < FOLIAGE_HALF =>
                    {
                        (FOLIAGE, Vec3::ZERO)
                    }
                    None if world.y > GROUND
                        && (world.x - PUSHED_CENTRE[0]).abs() < FOLIAGE_HALF
                        && (world.z - PUSHED_CENTRE[1]).abs() < FOLIAGE_HALF =>
                    {
                        (PUSHED, Vec3::ZERO)
                    }
                    _ => {
                        skipped += 1;
                        continue;
                    }
                };
                let under_foliage = [FOLIAGE_CENTRE, PUSHED_CENTRE].iter().any(|centre| {
                    (world.x - centre[0]).abs() < FOLIAGE_HALF
                        && (world.z - centre[1]).abs() < FOLIAGE_HALF
                });
                match gbuffer_class(class) {
                    Some(GB_TERRAIN) if under_foliage => {}
                    Some(class) => {
                        gbuffer_tallies[class].add(&GBUFFER_CLASSES[class], x, y, gbuffer[index]);
                    }
                    None => {}
                }
                let Some(twin) = motion_pixels(
                    camera_b.view_proj,
                    world,
                    camera_a.view_proj,
                    world - expected,
                    render,
                ) else {
                    skipped += 1;
                    continue;
                };
                let measured = motion[index];
                if class == PUSHED {
                    // Extra motion beyond the camera's, and the direction back to the
                    // displacer on screen (both render pixels, y down).
                    let extra = [measured[0] - twin[0], measured[1] - twin[1]];
                    let here = clip_to_pixels(camera_b.view_proj * world.extend(1.0), render);
                    let there =
                        clip_to_pixels(camera_b.view_proj * displacer_at.extend(1.0), render);
                    pushed.pixels += 1;
                    let length = extra[0].hypot(extra[1]);
                    if let (Some(here), Some(there)) = (here, there)
                        && length > PUSH_MOVED_PX
                    {
                        pushed.moved += 1;
                        extra_sum += f64::from(length);
                        let towards = [there[0] - here[0], there[1] - here[1]];
                        if extra[0] * towards[0] + extra[1] * towards[1] > 0.0 {
                            pushed.inward += 1;
                        }
                    }
                    continue;
                }
                let error = (measured[0] - twin[0]).hypot(measured[1] - twin[1]);
                let tally = &mut tallies[class];
                tally.pixels += 1;
                tally.max_error = tally.max_error.max(error);
                tally.sum_error += f64::from(error);
                tally.sum_motion += f64::from(twin[0].hypot(twin[1]));
            }
        }
        let checks = CLASSES
            .iter()
            .zip(tallies)
            .map(|(name, tally)| {
                let count = f64::from(tally.pixels.max(1));
                MotionCheck {
                    name,
                    pixels: tally.pixels,
                    max_error: tally.max_error,
                    mean_error: (tally.sum_error / count) as f32,
                    mean_motion: (tally.sum_motion / count) as f32,
                }
            })
            .collect();
        pushed.mean_extra = (extra_sum / f64::from(pushed.moved.max(1))) as f32;

        // Frame C: the camera at the horizon, for the sky's G-buffer (its far-plane pixels).
        let camera_c = Camera::perspective(
            Vec3::new(0.0, 1.5, 9.0),
            Vec3::new(0.0, 1.5, 0.0),
            Vec3::Y,
            0.8,
            aspect,
            1.0,
            60.0,
        );
        let scene_c = Scene::new(camera_c, &units_b)
            .with_bones(&bones_b)
            .with_environment(EnvironmentFrame::new(FOLIAGE_TIME, &push));
        if !(0..3).any(|_| self.render_internal(&scene_c, None, &[]).unwrap_or(false)) {
            return Err(probe_error("the sky frame was skipped"));
        }
        let sky = self.read_targets()?;
        for (index, (&d, &texels)) in sky.depth.iter().zip(&sky.gbuffer).enumerate() {
            if !depth_convention.nearer(d, depth_convention.far()) {
                gbuffer_tallies[GB_SKY].add(&GBUFFER_CLASSES[GB_SKY], index % w, index / w, texels);
            }
        }
        let gbuffer = GBUFFER_CLASSES
            .iter()
            .zip(gbuffer_tallies)
            .map(|(expect, tally)| GBufferCheck {
                name: expect.name,
                pixels: tally.pixels,
                mismatches: tally.mismatches,
                first_mismatch: tally.first.map(|(x, y, texels)| {
                    format!(
                        "({x}, {y}) gb0={:?} gb1={:#010x} gb2={:?}",
                        texels.gb0, texels.gb1, texels.gb2
                    )
                }),
            })
            .collect();
        Ok(MotionProbeReport {
            render: [render.width, render.height],
            jitter,
            checks,
            pushed,
            gbuffer,
            skipped,
        })
    }

    /// Frame B's motion (render pixels), depth and G-buffer, row-major at the render
    /// resolution.
    #[allow(clippy::too_many_lines)] // Transitions, five copies, transitions back.
    fn read_targets(&self) -> RenderResult<Readback> {
        let plan = &self.plans[0];
        let temporal = plan
            .resources
            .temporal
            .ok_or_else(|| probe_error("no motion target"))?;
        let color = vk::ImageAspectFlags::COLOR;
        let depth_format = self.frame_spec.depth_format;
        let depth_aspect = self.frame_spec.depth_aspect;
        let depth_bytes = depth_texel_bytes(depth_format);
        // (resource, name, aspect copied, aspect of the image, bytes per texel); every colour
        // target is 4 bytes a texel (`R16G16_SFLOAT`, `A2B10G10R10`, `R8G8B8A8`).
        let targets = [
            (temporal.motion, "motion", color, color, 4),
            (temporal.gbuffer[0], "gb0", color, color, 4),
            (temporal.gbuffer[1], "gb1", color, color, 4),
            (temporal.gbuffer[2], "gb2", color, color, 4),
            (
                plan.resources.depth,
                "depth",
                vk::ImageAspectFlags::DEPTH,
                depth_aspect,
                depth_bytes,
            ),
        ];
        let extent = self.render_extent;
        let texels = u64::from(extent.width) * u64::from(extent.height);
        let device = &self.device.device;
        let all = vk::PipelineStageFlags2::ALL_COMMANDS;
        let any = vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE;
        let copy_state = (
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            vk::PipelineStageFlags2::COPY,
            vk::AccessFlags2::TRANSFER_READ,
        );
        let mut images = Vec::with_capacity(targets.len());
        let mut buffers = Vec::with_capacity(targets.len());
        let mut to_copy = Vec::with_capacity(targets.len());
        let mut back = Vec::with_capacity(targets.len());
        for (resource, name, _, aspect, bytes) in targets {
            let (image, layout) = final_state(self, plan, resource, name)?;
            images.push(image);
            to_copy.push(layout_barrier(
                image,
                aspect,
                (layout, all, any),
                copy_state,
            ));
            back.push(layout_barrier(
                image,
                aspect,
                copy_state,
                (layout, all, any),
            ));
            buffers.push(GpuBuffer::new(
                device.clone(),
                self.device.allocator(),
                "motion-probe-readback",
                texels * bytes,
                vk::BufferUsageFlags::TRANSFER_DST,
                MemoryLocation::GpuToCpu,
            )?);
        }
        let pool = CommandPool::new(device.clone(), self.device.graphics_family)?;
        let cmd = pool.allocate_one()?;
        // The copies' writes made available to the host reads after the queue wait.
        let to_host = [vk::MemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::HOST)
            .dst_access_mask(vk::AccessFlags2::HOST_READ)];
        let region = |aspect: vk::ImageAspectFlags| {
            [vk::BufferImageCopy::default()
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(aspect)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width: extent.width,
                    height: extent.height,
                    depth: 1,
                })]
        };
        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: `cmd` was just allocated from `pool` and is not recording.
        unsafe { device.begin_command_buffer(cmd, &begin) }?;
        // SAFETY: the device is idle after frame B (`render_internal` returned and nothing
        // else was submitted), and every image is live in the layout the frame left it in.
        unsafe {
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&to_copy),
            );
        }
        for ((image, buffer), (_, _, aspect, _, _)) in images.iter().zip(&buffers).zip(targets) {
            // SAFETY: the image is in TRANSFER_SRC_OPTIMAL (the barrier above, with
            // `TRANSFER_SRC` usage from `RenderConfig::readback`) and the buffer holds the
            // render extent's texels of this target.
            unsafe {
                device.cmd_copy_image_to_buffer(
                    cmd,
                    *image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    buffer.handle(),
                    &region(aspect),
                );
            }
        }
        // SAFETY: returns every image to the layout the next frame's barriers expect.
        unsafe {
            device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default()
                    .memory_barriers(&to_host)
                    .image_memory_barriers(&back),
            );
        }
        // SAFETY: recording into `cmd` began above.
        unsafe { device.end_command_buffer(cmd) }?;
        // SAFETY: the device must be idle before the copies read the frame's images.
        unsafe { device.device_wait_idle() }?;
        let commands = [cmd];
        let submit = vk::SubmitInfo::default().command_buffers(&commands);
        // SAFETY: `cmd` is executable on the graphics queue; the renderer submits only from
        // this thread.
        unsafe { device.queue_submit(self.device.graphics_queue, &[submit], vk::Fence::null()) }?;
        // SAFETY: waits for the submission above; the pool and buffers outlive it.
        unsafe { device.queue_wait_idle(self.device.graphics_queue) }?;
        let count = usize::try_from(texels).expect("render extent fits usize");
        let [
            motion_buffer,
            gb0_buffer,
            gb1_buffer,
            gb2_buffer,
            depth_buffer,
        ] = &mut buffers[..]
        else {
            unreachable!("five targets");
        };
        let host = || probe_error("readback buffer is not host visible");
        let motion = motion_buffer
            .mapped_slice_mut::<[u16; 2]>(count)
            .ok_or_else(host)?
            .iter()
            .map(|texel| [f32_from_f16(texel[0]), f32_from_f16(texel[1])])
            .collect();
        let gb0 = gb0_buffer
            .mapped_slice_mut::<[u8; 4]>(count)
            .ok_or_else(host)?;
        let gb1 = gb1_buffer.mapped_slice_mut::<u32>(count).ok_or_else(host)?;
        let gb2 = gb2_buffer
            .mapped_slice_mut::<[u8; 4]>(count)
            .ok_or_else(host)?;
        let gbuffer = gb0
            .iter()
            .zip(gb1.iter())
            .zip(gb2.iter())
            .map(|((&gb0, &gb1), &gb2)| GBufferTexels { gb0, gb1, gb2 })
            .collect();
        let width = usize::try_from(depth_bytes).expect("small");
        let depth = depth_buffer
            .mapped_slice_mut::<u8>(count * width)
            .ok_or_else(host)?
            .chunks_exact(width)
            .map(|texel| decode_depth(depth_format, texel))
            .collect();
        Ok(Readback {
            motion,
            depth,
            gbuffer,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_cube_matches_the_built_in_winding_and_its_vat_parses() {
        let (positions, normals, indices) = cube_mesh();
        assert_eq!(
            (positions.len(), normals.len(), indices.len()),
            (24, 24, 36)
        );
        // The built-in cube's +X face starts at (0.5, -0.5, -0.5) and turns towards +Z.
        assert_eq!(positions[0], [0.5, -0.5, -0.5]);
        assert_eq!(positions[1], [0.5, -0.5, 0.5]);
        let mesh = MeshData::new(positions.clone(), normals.clone(), indices).expect("mesh");
        let (texture, descriptor) = vat_assets(&mesh, &positions, &normals).expect("vat");
        descriptor.check_texture(&texture).expect("texture matches");
        descriptor.check_mesh(&mesh).expect("mesh matches");
        assert_eq!(f16_from_f32(0.5 + VAT_MOVE.z), f16_from_f32(0.625));
        assert_eq!(f32_from_f16(f16_from_f32(0.5625)), 0.5625);
    }

    #[test]
    fn depth_decodes_every_format() {
        assert_eq!(
            decode_depth(vk::Format::D32_SFLOAT, &0.25f32.to_le_bytes()),
            0.25
        );
        assert_eq!(
            decode_depth(vk::Format::D24_UNORM_S8_UINT, &0xffff_ffffu32.to_le_bytes()),
            1.0
        );
        assert_eq!(decode_depth(vk::Format::D16_UNORM, &[0, 0]), 0.0);
        assert_eq!(depth_texel_bytes(vk::Format::D16_UNORM), 2);
        assert_eq!(depth_texel_bytes(vk::Format::D32_SFLOAT_S8_UINT), 4);
    }

    /// The probe's expectations are what the twin encodes for each class (ADR-0023
    /// amendment 1 item 9), and a wrong flag, model, rim slot, F0, metallic or base colour
    /// fails, as does the lavapipe fault's lost blue and alpha (item 16).
    #[test]
    fn gbuffer_expectations_match_the_twin() {
        use crate::gbuffer::{SurfaceParams, encode};
        let surface = |model, flags, f0: f32| SurfaceParams {
            base_color: UNIT_COLOUR,
            metallic: 0.0,
            normal: [0.0, 1.0, 0.0],
            roughness: 0.6,
            model,
            model_data: [f0, 0.0, 0.0],
            ao: 1.0,
            flags,
            rim_slot: 0,
        };
        for expect in &GBUFFER_CLASSES[..GB_SKY] {
            let f0 = if expect.f0 { 0.04 } else { 0.3 };
            let good = encode(&surface(expect.model, expect.flags, f0));
            assert!(gbuffer_matches(expect, good), "{}", expect.name);
            let flagged = encode(&surface(expect.model, expect.flags ^ FLAG_UNIT, f0));
            assert!(!gbuffer_matches(expect, flagged), "{}", expect.name);
            let mut rim = good;
            rim.gb2[3] |= 1;
            assert!(!gbuffer_matches(expect, rim), "{}", expect.name);
            let mut metallic = good;
            metallic.gb0[3] = 1;
            assert!(!gbuffer_matches(expect, metallic), "{}", expect.name);
            // The fault: every attachment's blue and alpha dropped.
            let mut dropped = good;
            dropped.gb0[2] = 0;
            dropped.gb2[2] = 0;
            dropped.gb2[3] = 0;
            assert!(!gbuffer_matches(expect, dropped), "{}", expect.name);
        }
        let unit = &GBUFFER_CLASSES[0];
        let mut rounded = encode(&surface(unit.model, UNIT_FLAGS, 0.04));
        rounded.gb0[0] += 1;
        assert!(gbuffer_matches(unit, rounded), "one step of rounding");
        let mut blue = encode(&surface(unit.model, UNIT_FLAGS, 0.04));
        blue.gb0[2] = 0;
        assert!(!gbuffer_matches(unit, blue), "gb0's blue alone");
        assert!(!gbuffer_matches(
            unit,
            encode(&surface(unit.model, UNIT_FLAGS, 0.05))
        ));
        assert!(!gbuffer_matches(
            unit,
            encode(&surface(ShadingModel::Foliage, UNIT_FLAGS, 0.04))
        ));
        let sky = &GBUFFER_CLASSES[GB_SKY];
        assert!(gbuffer_matches(sky, GBufferTexels::default()));
        let normal = GBufferTexels {
            gb1: 1,
            ..GBufferTexels::default()
        };
        assert!(!gbuffer_matches(sky, normal));
        let coloured = GBufferTexels {
            gb0: [0, 0, 1, 0],
            ..GBufferTexels::default()
        };
        assert!(!gbuffer_matches(sky, coloured));
        for class in [CAMERA, 1, 2, 3, FOLIAGE, 5, 6, 7, PUSHED] {
            assert!(gbuffer_class(class).is_some(), "{class}");
        }
    }

    #[test]
    fn the_pan_is_no_camera_cut() {
        assert!(CAMERA_PAN.length() < 8.0);
    }
}
