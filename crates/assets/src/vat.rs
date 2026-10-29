//! Vertex-animation textures (VAT) for crowds of simple units (R4b, ADR-0012 amendment).
//!
//! A skinned model's clips are baked offline ([`bake_vat`], driven by `moba-tools vat-bake`)
//! into one `RGBA16F` texture of skinned vertex positions and normals plus a small TOML
//! sidecar, the [`VatDescriptor`]. At run time the renderer plays a frame of it in the vertex
//! shader from two numbers per instance ([`VatPose`]), so a minion needs no animator, no bone
//! range and no skinning matrices. [`VatPlayer`] is the per-unit state (idle ↔ run, attack,
//! death) that picks those numbers; it is fixed-size and never allocates.
//!
//! **Texture layout.** Texel `i = frame × vertex_count + vertex` (row-major, wrapping at the
//! texture width) holds the vertex's skinned model-space position (`w = 1`); texel
//! `frame_count × vertex_count + i` holds its normal (`w = 0`). The height is
//! `ceil(2 × frame_count × vertex_count / width)` ([`vat_extent`]).
//!
//! **Descriptor** (`<asset root>/vat/<name>.toml`, written by the baker):
//!
//! ```toml
//! model = "sentinel-skinned"   # models/<model>.glb: the static mesh that is drawn
//! vertex_count = 1186
//! frame_count = 88
//! width = 4096
//! height = 54
//! fps = 20.0
//! mesh_hash = "0x1234abcd5678ef90"   # FNV-1a of the model's positions and indices
//!
//! [[clips]]
//! name = "idle"
//! first = 0
//! frames = 48
//! loop = true
//!
//! [playback]          # optional
//! run_speed = 4.0     # m/s at which the run clip plays at its authored rate
//! attack_release = 0.35
//! crossfade = 0.15
//! ```
//!
//! Like animation sets, VAT assets are untrusted, client-local and cosmetic: never hashed,
//! signed, replicated or replayed. [`VatDescriptor::load_file`] reads at most
//! [`MAX_VAT_BYTES`], rejects unknown keys and range-checks every value;
//! [`VatDescriptor::check_texture`] and [`VatDescriptor::check_mesh`] reject a texture or a
//! model that does not match it.

use std::io::Read;
use std::path::{Path, PathBuf};

use moba_core::math::{Mat3, Vec3};
use serde::Deserialize;

use crate::map_visuals::valid_mesh_name;
use crate::mesh::MeshData;
use crate::pose::{ClipWrap, Pose};
use crate::skin::{JointMatrix, MAX_JOINTS, valid_animation_name};
use crate::texture::{
    MAX_KTX2_BYTES, MAX_TEXTURE_DIMENSION, TextureData, TextureFormat, f16_from_f32,
};

/// Directory under the asset root that holds VAT assets.
pub const VAT_DIR: &str = "vat";
/// Largest descriptor file accepted, in bytes.
pub const MAX_VAT_BYTES: u64 = 8 * 1024;
/// Most clips one VAT asset holds.
pub const MAX_VAT_CLIPS: usize = 16;
/// Most frames one VAT asset holds (all clips together).
pub const MAX_VAT_FRAMES: u32 = 4_096;
/// Default bake rate, frames per second.
pub const DEFAULT_VAT_FPS: f32 = 20.0;
/// Seconds a dead unit stays drawn after its death clip ends.
pub const VAT_DEATH_LINGER: f32 = 0.6;
/// Most texels one VAT texture holds: 8 bytes each (`RGBA16F`), so the KTX2 file (data plus a
/// header allowance of 4 KiB) stays within [`MAX_KTX2_BYTES`] and loads.
pub const MAX_VAT_TEXELS: u64 = (MAX_KTX2_BYTES - 4_096) / 8;

const FPS_RANGE: (f32, f32) = (1.0, 120.0);
const RUN_SPEED_RANGE: (f32, f32) = (0.1, 100.0);
const RELEASE_RANGE: (f32, f32) = (0.0, 10.0);
const CROSSFADE_RANGE: (f32, f32) = (0.0, 2.0);
/// Below this ground speed (m/s) a unit plays its idle clip.
const MOVING_SPEED: f32 = 0.3;
/// The run clip's playback rate follows speed / `run_speed`, within these bounds.
const RUN_RATE_RANGE: (f32, f32) = (0.6, 1.6);

/// Why a VAT asset was rejected or could not be baked.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum VatError {
    /// Reading the file failed.
    Io(String),
    /// The path is not a regular file.
    NotAFile,
    /// The file exceeds [`MAX_VAT_BYTES`].
    TooLarge,
    /// The TOML did not parse or has unknown keys.
    Toml(String),
    /// A value is out of range or inconsistent.
    Invalid(&'static str),
    /// The texture or mesh does not match the descriptor.
    Mismatch(&'static str),
    /// A clip named for baking does not exist.
    UnknownClip(String),
}

impl core::fmt::Display for VatError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "cannot read VAT descriptor: {error}"),
            Self::NotAFile => f.write_str("VAT descriptor is not a regular file"),
            Self::TooLarge => write!(f, "VAT descriptor exceeds {MAX_VAT_BYTES} bytes"),
            Self::Toml(error) => write!(f, "VAT descriptor: {error}"),
            Self::Invalid(what) => write!(f, "VAT descriptor: {what}"),
            Self::Mismatch(what) => write!(f, "VAT asset mismatch: {what}"),
            Self::UnknownClip(name) => write!(f, "no clip named '{name}'"),
        }
    }
}

impl std::error::Error for VatError {}

/// One baked clip: a run of frames in the texture.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct VatClip {
    /// Clip name (1–32 of `[A-Za-z0-9_-]`).
    pub name: String,
    /// First frame of the clip.
    pub first: u32,
    /// Frame count (≥ 1).
    pub frames: u32,
    /// Whether the clip loops (frame `frames` is frame 0 again).
    pub looping: bool,
}

/// Playback parameters of a VAT asset (the `[playback]` table).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct VatPlaybackParams {
    /// Ground speed (m/s) at which the run clip plays at its authored rate.
    pub run_speed: f32,
    /// Seconds into the attack clip of the hit frame (lined up with the attack event).
    pub attack_release: f32,
    /// Crossfade time between clips, seconds.
    pub crossfade: f32,
}

impl Default for VatPlaybackParams {
    fn default() -> Self {
        Self {
            run_speed: 4.0,
            attack_release: 0.35,
            crossfade: 0.15,
        }
    }
}

impl VatPlaybackParams {
    /// These parameters if every one is in the range a descriptor accepts (`run_speed`
    /// 0.1..=100 m/s, `attack_release` 0..=10 s, `crossfade` 0..=2 s).
    ///
    /// # Errors
    /// [`VatError::Invalid`] naming the first value out of range or not finite.
    pub fn validated(self) -> Result<Self, VatError> {
        Ok(Self {
            run_speed: check_range(
                self.run_speed,
                RUN_SPEED_RANGE,
                "run_speed must be 0.1..=100",
            )?,
            attack_release: check_range(
                self.attack_release,
                RELEASE_RANGE,
                "attack_release must be 0..=10 seconds",
            )?,
            crossfade: check_range(
                self.crossfade,
                CROSSFADE_RANGE,
                "crossfade must be 0..=2 seconds",
            )?,
        })
    }
}

/// A validated VAT descriptor (see the module docs).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct VatDescriptor {
    /// The model whose static mesh is drawn (`models/<model>.glb`).
    pub model: String,
    /// Vertices per frame (the mesh's vertex count).
    pub vertex_count: u32,
    /// Frames of all clips together.
    pub frame_count: u32,
    /// Texture width in texels.
    pub width: u32,
    /// Texture height in texels.
    pub height: u32,
    /// Frames per second of every clip.
    pub fps: f32,
    /// [`mesh_hash`] of the model the texture was baked from.
    pub mesh_hash: u64,
    /// The clips, in texture order.
    pub clips: Vec<VatClip>,
    /// Playback parameters.
    pub playback: VatPlaybackParams,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDescriptor {
    model: String,
    vertex_count: u32,
    frame_count: u32,
    width: u32,
    height: u32,
    #[serde(default)]
    fps: Option<f32>,
    mesh_hash: String,
    clips: Vec<RawClip>,
    #[serde(default)]
    playback: Option<RawPlayback>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawClip {
    name: String,
    first: u32,
    frames: u32,
    #[serde(default, rename = "loop")]
    looping: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlayback {
    #[serde(default)]
    run_speed: Option<f32>,
    #[serde(default)]
    attack_release: Option<f32>,
    #[serde(default)]
    crossfade: Option<f32>,
}

fn check_range(value: f32, (min, max): (f32, f32), what: &'static str) -> Result<f32, VatError> {
    if value.is_finite() && (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(VatError::Invalid(what))
    }
}

/// The texture extent for `frame_count` frames of `vertex_count` vertices: the width is the
/// texel count capped at [`MAX_TEXTURE_DIMENSION`], the height what the rest needs. `None`
/// if it needs more than [`MAX_VAT_TEXELS`] (the KTX2 size limit) or a 4096² texture.
#[must_use]
pub fn vat_extent(vertex_count: u32, frame_count: u32) -> Option<(u32, u32)> {
    let texels = u64::from(vertex_count)
        .checked_mul(u64::from(frame_count))?
        .checked_mul(2)?;
    if texels == 0 {
        return None;
    }
    let width = texels.min(u64::from(MAX_TEXTURE_DIMENSION));
    let height = texels.div_ceil(width);
    (height <= u64::from(MAX_TEXTURE_DIMENSION) && width * height <= MAX_VAT_TEXELS)
        .then(|| (u32::try_from(width).ok(), u32::try_from(height).ok()))
        .and_then(|(width, height)| Some((width?, height?)))
}

/// FNV-1a (64-bit) of a mesh's vertex positions (IEEE bits) and indices: identifies the mesh
/// a VAT was baked from.
#[must_use]
pub fn mesh_hash(mesh: &MeshData) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut feed = |bytes: [u8; 4]| {
        for byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    };
    feed(
        u32::try_from(mesh.vertex_count())
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    for position in mesh.positions() {
        for value in position {
            feed(value.to_bits().to_le_bytes());
        }
    }
    for index in mesh.indices() {
        feed(index.to_le_bytes());
    }
    hash
}

/// `<root>/vat/<name>.toml` and `<root>/vat/<name>.ktx2`, or `None` for an invalid name.
#[must_use]
pub fn vat_paths(root: &Path, name: &str) -> Option<(PathBuf, PathBuf)> {
    valid_mesh_name(name).then(|| {
        let dir = root.join(VAT_DIR);
        (
            dir.join(format!("{name}.toml")),
            dir.join(format!("{name}.ktx2")),
        )
    })
}

impl VatDescriptor {
    /// Parses and validates a descriptor.
    ///
    /// # Errors
    /// [`VatError::TooLarge`], a TOML error, or the first invalid value.
    pub fn parse(text: &str) -> Result<Self, VatError> {
        if text.len() as u64 > MAX_VAT_BYTES {
            return Err(VatError::TooLarge);
        }
        let raw: RawDescriptor =
            toml::from_str(text).map_err(|error| VatError::Toml(error.to_string()))?;
        if !valid_mesh_name(&raw.model) {
            return Err(VatError::Invalid("model must be 1-32 of [A-Za-z0-9_-]"));
        }
        if raw.vertex_count == 0 || raw.frame_count == 0 || raw.frame_count > MAX_VAT_FRAMES {
            return Err(VatError::Invalid(
                "vertex_count must be positive and frame_count 1..=4096",
            ));
        }
        if vat_extent(raw.vertex_count, raw.frame_count) != Some((raw.width, raw.height)) {
            return Err(VatError::Invalid(
                "width/height do not match vertex_count and frame_count",
            ));
        }
        let hash = raw
            .mesh_hash
            .strip_prefix("0x")
            .and_then(|hex| u64::from_str_radix(hex, 16).ok())
            .ok_or(VatError::Invalid("mesh_hash must be a 0x-prefixed hex u64"))?;
        if raw.clips.is_empty() || raw.clips.len() > MAX_VAT_CLIPS {
            return Err(VatError::Invalid("1 to 16 clips"));
        }
        let mut clips: Vec<VatClip> = Vec::with_capacity(raw.clips.len());
        for clip in raw.clips {
            if !valid_animation_name(&clip.name) {
                return Err(VatError::Invalid(
                    "clip names must be 1-32 of [A-Za-z0-9_-]",
                ));
            }
            if clips
                .iter()
                .any(|other| other.name.eq_ignore_ascii_case(&clip.name))
            {
                return Err(VatError::Invalid("clip names must be unique"));
            }
            let end = clip.first.checked_add(clip.frames);
            if clip.frames == 0 || end.is_none_or(|end| end > raw.frame_count) {
                return Err(VatError::Invalid(
                    "a clip's frames must lie inside frame_count",
                ));
            }
            clips.push(VatClip {
                name: clip.name,
                first: clip.first,
                frames: clip.frames,
                looping: clip.looping,
            });
        }
        let defaults = VatPlaybackParams::default();
        let playback = match raw.playback {
            None => defaults,
            Some(raw) => VatPlaybackParams {
                run_speed: raw.run_speed.unwrap_or(defaults.run_speed),
                attack_release: raw.attack_release.unwrap_or(defaults.attack_release),
                crossfade: raw.crossfade.unwrap_or(defaults.crossfade),
            }
            .validated()?,
        };
        Ok(Self {
            model: raw.model,
            vertex_count: raw.vertex_count,
            frame_count: raw.frame_count,
            width: raw.width,
            height: raw.height,
            fps: check_range(
                raw.fps.unwrap_or(DEFAULT_VAT_FPS),
                FPS_RANGE,
                "fps must be 1..=120",
            )?,
            mesh_hash: hash,
            clips,
            playback,
        })
    }

    /// Reads and parses a descriptor file, reading at most [`MAX_VAT_BYTES`] + 1 bytes.
    ///
    /// # Errors
    /// An I/O, size, TOML or range error.
    pub fn load_file(path: &Path) -> Result<Self, VatError> {
        let file = std::fs::File::open(path).map_err(|error| VatError::Io(error.to_string()))?;
        let metadata = file
            .metadata()
            .map_err(|error| VatError::Io(error.to_string()))?;
        if !metadata.is_file() {
            return Err(VatError::NotAFile);
        }
        if metadata.len() > MAX_VAT_BYTES {
            return Err(VatError::TooLarge);
        }
        let mut text = String::new();
        file.take(MAX_VAT_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|error| VatError::Io(error.to_string()))?;
        Self::parse(&text)
    }

    /// The descriptor as TOML (what the baker writes; [`Self::parse`] reads it back).
    #[must_use]
    pub fn to_toml(&self) -> String {
        use core::fmt::Write as _;
        let mut text = String::from(
            "# Vertex-animation texture descriptor (ADR-0012 R4b), written by\n\
             # `moba-tools vat-bake`. Client-local and cosmetic: never hashed or sent.\n\
             # Format: docs/content-authoring.md#vertex-animation-textures\n\n",
        );
        let _ = writeln!(text, "model = \"{}\"", self.model);
        let _ = writeln!(text, "vertex_count = {}", self.vertex_count);
        let _ = writeln!(text, "frame_count = {}", self.frame_count);
        let _ = writeln!(text, "width = {}", self.width);
        let _ = writeln!(text, "height = {}", self.height);
        let _ = writeln!(text, "fps = {:?}", self.fps);
        let _ = writeln!(text, "mesh_hash = \"0x{:016x}\"", self.mesh_hash);
        for clip in &self.clips {
            let _ = write!(
                text,
                "\n[[clips]]\nname = \"{}\"\nfirst = {}\nframes = {}\nloop = {}\n",
                clip.name, clip.first, clip.frames, clip.looping
            );
        }
        let _ = write!(
            text,
            "\n[playback]\nrun_speed = {:?}\nattack_release = {:?}\ncrossfade = {:?}\n",
            self.playback.run_speed, self.playback.attack_release, self.playback.crossfade
        );
        text
    }

    /// The clip called `name` (ASCII case-insensitive).
    #[must_use]
    pub fn clip(&self, name: &str) -> Option<usize> {
        self.clips
            .iter()
            .position(|clip| clip.name.eq_ignore_ascii_case(name))
    }

    /// Checks that `texture` is this asset's texture: `RGBA16F`, one level, 2D, the
    /// descriptor's extent.
    ///
    /// # Errors
    /// [`VatError::Mismatch`].
    pub fn check_texture(&self, texture: &TextureData) -> Result<(), VatError> {
        if texture.format() != TextureFormat::Rgba16Float {
            return Err(VatError::Mismatch("the texture must be rgba16f"));
        }
        if texture.is_cube() || texture.level_count() != 1 {
            return Err(VatError::Mismatch("the texture must be 2D with one level"));
        }
        if (texture.width(), texture.height()) != (self.width, self.height) {
            return Err(VatError::Mismatch(
                "texture size differs from the descriptor",
            ));
        }
        Ok(())
    }

    /// Checks that `mesh` is the mesh the texture was baked from (vertex count and
    /// [`mesh_hash`]).
    ///
    /// # Errors
    /// [`VatError::Mismatch`].
    pub fn check_mesh(&self, mesh: &MeshData) -> Result<(), VatError> {
        if u32::try_from(mesh.vertex_count()).ok() != Some(self.vertex_count) {
            return Err(VatError::Mismatch(
                "vertex count differs from the descriptor",
            ));
        }
        if mesh_hash(mesh) != self.mesh_hash {
            return Err(VatError::Mismatch(
                "the model changed since the bake (re-run vat-bake)",
            ));
        }
        Ok(())
    }
}

/// One clip to bake: its name in the source skeleton and whether it loops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct VatBakeClip<'a> {
    /// Clip name in the skeleton.
    pub name: &'a str,
    /// Whether the clip loops.
    pub looping: bool,
}

impl<'a> VatBakeClip<'a> {
    /// The clip `name`, looping or not.
    #[must_use]
    pub const fn new(name: &'a str, looping: bool) -> Self {
        Self { name, looping }
    }
}

/// Frames a clip of `duration` seconds takes at `fps`: a looping clip covers `[0, duration)`
/// (its next frame is frame 0 again), a one-shot `[0, duration]`.
fn clip_frames(duration: f32, fps: f32, looping: bool) -> u32 {
    let span = (duration.max(0.0) * fps).round();
    // Bounded by MAX_CLIP_SECONDS × 120 fps, far below u32::MAX.
    let frames = span as u32;
    if looping { frames.max(1) } else { frames + 1 }
}

/// Bakes `clips` of a skinned `mesh` at `fps` into a VAT texture and its descriptor (named
/// after `model`, the mesh's model name). Every vertex is skinned on the CPU with the same
/// formula as the GPU skinning shader. `playback` is checked and copied into the descriptor,
/// so a bake always produces a descriptor [`VatDescriptor::parse`] accepts.
///
/// # Errors
/// [`VatError::Invalid`] for a mesh without a skin, no clips, too many clips or frames, an
/// fps or playback value out of range or a texture that would not fit (over
/// [`MAX_VAT_TEXELS`]); [`VatError::UnknownClip`] for a clip the skeleton does not have.
#[allow(clippy::too_many_lines)] // One pass: lay out the clips, sample, skin, encode.
pub fn bake_vat(
    mesh: &MeshData,
    model: &str,
    clips: &[VatBakeClip<'_>],
    fps: f32,
    playback: VatPlaybackParams,
) -> Result<(TextureData, VatDescriptor), VatError> {
    if !valid_mesh_name(model) {
        return Err(VatError::Invalid("model must be 1-32 of [A-Za-z0-9_-]"));
    }
    let skin = mesh
        .skin()
        .ok_or(VatError::Invalid("the model has no skin"))?;
    let skeleton = skin.skeleton();
    let fps = check_range(fps, FPS_RANGE, "fps must be 1..=120")?;
    let playback = playback.validated()?;
    if clips.is_empty() || clips.len() > MAX_VAT_CLIPS {
        return Err(VatError::Invalid("1 to 16 clips"));
    }
    let mut layout = Vec::with_capacity(clips.len());
    let mut frame_count = 0u32;
    for clip in clips {
        let index = skeleton
            .clip_index(clip.name)
            .ok_or_else(|| VatError::UnknownClip(clip.name.to_owned()))?;
        if layout
            .iter()
            .any(|(other, _): &(VatClip, usize)| other.name.eq_ignore_ascii_case(clip.name))
        {
            return Err(VatError::Invalid("clip names must be unique"));
        }
        let duration = skeleton.clips()[index].duration();
        let frames = clip_frames(duration, fps, clip.looping);
        layout.push((
            VatClip {
                name: clip.name.to_owned(),
                first: frame_count,
                frames,
                looping: clip.looping,
            },
            index,
        ));
        frame_count = frame_count
            .checked_add(frames)
            .filter(|&count| count <= MAX_VAT_FRAMES)
            .ok_or(VatError::Invalid("at most 4096 frames"))?;
    }
    let vertex_count =
        u32::try_from(mesh.vertex_count()).map_err(|_| VatError::Invalid("too many vertices"))?;
    let (width, height) = vat_extent(vertex_count, frame_count).ok_or(VatError::Invalid(
        "the frames do not fit in a 4096² texture",
    ))?;
    let texel_count = (width as usize) * (height as usize);
    let mut data = vec![0u8; texel_count * 8];
    let normals_base = (frame_count as usize) * (vertex_count as usize);
    let mut pose = Pose::default();
    let mut model_space = [moba_core::math::Mat4::IDENTITY; MAX_JOINTS];
    let mut matrices: [JointMatrix; MAX_JOINTS] = [[[0.0; 4]; 3]; MAX_JOINTS];
    let joints = skeleton.joint_count();
    let mut write = |texel: usize, value: [f32; 4]| {
        let offset = texel * 8;
        for (lane, component) in value.iter().enumerate() {
            let bits = f16_from_f32(*component).to_le_bytes();
            data[offset + lane * 2..offset + lane * 2 + 2].copy_from_slice(&bits);
        }
    };
    for (clip, index) in &layout {
        let wrap = if clip.looping {
            ClipWrap::Loop
        } else {
            ClipWrap::Clamp
        };
        for frame in 0..clip.frames {
            let time = frame as f32 / fps;
            skeleton.sample_clip(*index, time, wrap, &mut pose);
            skeleton.model_transforms(&pose, &mut model_space[..joints]);
            skeleton.skinning_matrices(&model_space[..joints], &mut matrices[..joints]);
            let row = (clip.first + frame) as usize * vertex_count as usize;
            for (vertex, ((position, normal), (lanes, weights))) in mesh
                .positions()
                .iter()
                .zip(mesh.normals())
                .zip(skin.joints().iter().zip(skin.weights()))
                .enumerate()
            {
                let (skinned, normal) =
                    skin_vertex(&matrices[..joints], *position, *normal, *lanes, *weights);
                write(row + vertex, [skinned.x, skinned.y, skinned.z, 1.0]);
                write(
                    normals_base + row + vertex,
                    [normal.x, normal.y, normal.z, 0.0],
                );
            }
        }
    }
    let texture = TextureData::new(TextureFormat::Rgba16Float, width, height, &[data])
        .map_err(|_| VatError::Invalid("texture"))?;
    let descriptor = VatDescriptor {
        model: model.to_owned(),
        vertex_count,
        frame_count,
        width,
        height,
        fps,
        mesh_hash: mesh_hash(mesh),
        clips: layout.into_iter().map(|(clip, _)| clip).collect(),
        playback,
    };
    Ok((texture, descriptor))
}

/// Skins one vertex like `skinned.vert`: the weight-blended joint matrix (normalised by the
/// weight sum; identity for an unweighted vertex) applied to the position, and its
/// inverse-transpose to the normal.
fn skin_vertex(
    matrices: &[JointMatrix],
    position: [f32; 3],
    normal: [f32; 3],
    lanes: [u8; 4],
    weights: [u8; 4],
) -> (Vec3, Vec3) {
    let total: f32 = weights.iter().map(|&weight| f32::from(weight)).sum();
    let mut rows = [[0.0f32; 4]; 3];
    if total > 0.0 {
        for (&joint, &weight) in lanes.iter().zip(&weights) {
            if weight == 0 {
                continue;
            }
            let Some(matrix) = matrices.get(usize::from(joint)) else {
                continue;
            };
            let share = f32::from(weight) / total;
            for (row, source) in rows.iter_mut().zip(matrix) {
                for (value, component) in row.iter_mut().zip(source) {
                    *value += share * component;
                }
            }
        }
    } else {
        rows = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ];
    }
    let p = Vec3::from_array(position);
    let apply = |row: [f32; 4]| row[0] * p.x + row[1] * p.y + row[2] * p.z + row[3];
    let skinned = Vec3::new(apply(rows[0]), apply(rows[1]), apply(rows[2]));
    // Column-major 3x3 from the row-major rows.
    let linear = Mat3::from_cols(
        Vec3::new(rows[0][0], rows[1][0], rows[2][0]),
        Vec3::new(rows[0][1], rows[1][1], rows[2][1]),
        Vec3::new(rows[0][2], rows[1][2], rows[2][2]),
    );
    let normal_matrix = if linear.determinant().abs() > 1e-12 {
        linear.inverse().transpose()
    } else {
        linear
    };
    let normal = (normal_matrix * Vec3::from_array(normal)).normalize_or_zero();
    (skinned, normal)
}

/// One frame sample of a VAT: two neighbouring frames and the blend between them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct VatSample {
    /// Frame index (into the whole texture).
    pub frame: u32,
    /// The frame after it (the same frame at a one-shot clip's end).
    pub next: u32,
    /// Weight of `next`, `0..1`.
    pub t: f32,
}

impl VatSample {
    /// A sample at `frame` blending `t` towards `next`.
    #[must_use]
    pub const fn new(frame: u32, next: u32, t: f32) -> Self {
        Self { frame, next, t }
    }
}

/// What a VAT instance shows this frame: `from` crossfaded towards `to` by `blend`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct VatPose {
    /// The clip being faded out (or the only one, with `blend = 0`).
    pub from: VatSample,
    /// The clip being faded in.
    pub to: VatSample,
    /// Weight of `to`, `0..=1`.
    pub blend: f32,
}

impl VatPose {
    /// A pose of one sample, no crossfade.
    #[must_use]
    pub const fn single(sample: VatSample) -> Self {
        Self {
            from: sample,
            to: sample,
            blend: 0.0,
        }
    }

    /// A crossfade from `from` to `to` with weight `blend` on `to`.
    #[must_use]
    pub const fn crossfade(from: VatSample, to: VatSample, blend: f32) -> Self {
        Self { from, to, blend }
    }
}

/// How much work the VAT vertex shader does, by quality tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct VatQuality {
    /// Interpolate between neighbouring frames (otherwise the nearest frame).
    pub interpolate: bool,
    /// Crossfade between clips (otherwise switch at the midpoint of the fade).
    pub crossfade: bool,
}

impl VatQuality {
    /// Everything on (`High`/`Ultra`): two frames per clip, two clips, 8 fetches per vertex.
    pub const FULL: Self = Self {
        interpolate: true,
        crossfade: true,
    };
    /// Frames interpolated, no crossfade (`Medium`): 4 fetches.
    pub const INTERPOLATED: Self = Self {
        interpolate: true,
        crossfade: false,
    };
    /// Nearest frame, no crossfade (`Low`): 2 fetches.
    pub const NEAREST: Self = Self {
        interpolate: false,
        crossfade: false,
    };

    fn apply(self, pose: VatPose) -> VatPose {
        let nearest = |sample: VatSample| {
            if self.interpolate {
                sample
            } else {
                let frame = if sample.t >= 0.5 {
                    sample.next
                } else {
                    sample.frame
                };
                VatSample::new(frame, frame, 0.0)
            }
        };
        let (from, to) = (nearest(pose.from), nearest(pose.to));
        if self.crossfade {
            VatPose::crossfade(from, to, pose.blend)
        } else {
            VatPose::single(if pose.blend >= 0.5 { to } else { from })
        }
    }
}

/// A [`VatDescriptor`]'s clips resolved for [`VatPlayer`]: `idle`, `run` (or `walk`),
/// `attack` and `death` by name. Only `idle` (else the first clip) is required.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VatAnimation {
    clips: [(u32, u32, bool); MAX_VAT_CLIPS],
    idle: u8,
    run: Option<u8>,
    attack: Option<u8>,
    death: Option<u8>,
    fps: f32,
    playback: VatPlaybackParams,
    frame_count: u32,
}

impl VatAnimation {
    /// Resolves the conventional clip names of `descriptor`.
    #[must_use]
    pub fn bind(descriptor: &VatDescriptor) -> Self {
        let mut clips = [(0, 1, false); MAX_VAT_CLIPS];
        for (slot, clip) in clips.iter_mut().zip(&descriptor.clips) {
            *slot = (clip.first, clip.frames, clip.looping);
        }
        let find = |name: &str| {
            descriptor
                .clip(name)
                .and_then(|index| u8::try_from(index).ok())
        };
        Self {
            clips,
            idle: find("idle").unwrap_or(0),
            run: find("run").or_else(|| find("walk")),
            attack: find("attack"),
            death: find("death"),
            fps: descriptor.fps,
            playback: descriptor.playback,
            frame_count: descriptor.frame_count,
        }
    }

    /// Frames in the texture.
    #[must_use]
    pub fn frame_count(&self) -> u32 {
        self.frame_count
    }

    /// Seconds clip `clip` takes at rate 1.
    fn duration(&self, clip: u8) -> f32 {
        let (_, frames, looping) = self.clips[usize::from(clip)];
        let frames = if looping { frames } else { frames - 1 };
        frames as f32 / self.fps
    }

    /// The sample of `clip` at `time` seconds into it.
    fn sample(&self, clip: u8, time: f32) -> VatSample {
        let (first, frames, looping) = self.clips[usize::from(clip)];
        let position = (time.max(0.0) * self.fps).max(0.0);
        if looping {
            let wrapped = position % frames as f32;
            // `wrapped < frames`, a u32.
            let frame = (wrapped as u32).min(frames - 1);
            let next = (frame + 1) % frames;
            VatSample::new(first + frame, first + next, wrapped - frame as f32)
        } else {
            let last = frames - 1;
            if position >= last as f32 {
                return VatSample::new(first + last, first + last, 0.0);
            }
            let frame = position as u32;
            VatSample::new(first + frame, first + frame + 1, position - frame as f32)
        }
    }

    /// Seconds a death stays drawn: the death clip plus [`VAT_DEATH_LINGER`] (just the linger
    /// without a death clip).
    #[must_use]
    pub fn death_seconds(&self) -> f32 {
        self.death.map_or(0.0, |death| self.duration(death)) + VAT_DEATH_LINGER
    }
}

/// One action (attack or death) of a [`VatPlayer`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct VatAction {
    clip: u8,
    /// Clock time at clip time 0.
    start: f32,
    /// Clip seconds per clock second.
    rate: f32,
}

/// Per-unit VAT playback state: locomotion (idle ↔ run by speed), one attack at a time lined
/// up with its hit, and death. 40 bytes, no heap.
///
/// Like [`Animator`](crate::Animator) it runs on an absolute clock and keeps no history
/// beyond its trigger stamps: [`VatPlayer::pose`] is a pure function of the render time, the
/// speed and the stamps, so a pinned capture tick gives the same pose whatever the frame
/// pacing. Idle and run crossfade by speed (a ramp from 0.3 to 1.5 m/s), not by time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VatPlayer {
    phase: f32,
    attack: Option<VatAction>,
    death: Option<VatAction>,
}

/// The idle → run crossfade by speed: `(speed - MOVING_SPEED) / RUN_BLEND_SPAN`.
const RUN_BLEND_SPAN: f32 = 1.2;

impl VatPlayer {
    /// A player idling; `phase` (seconds) desynchronises units.
    #[must_use]
    pub fn new(_animation: &VatAnimation, phase: f32) -> Self {
        Self {
            phase: if phase.is_finite() { phase } else { 0.0 },
            attack: None,
            death: None,
        }
    }

    /// An attack whose hit lands at clock time `hit`; `interval` (seconds, 0 if unknown)
    /// speeds the clip up when attacks come faster than it. A repeat of the playing attack
    /// within 0.1 s is ignored; a later one replaces it.
    pub fn attack(&mut self, animation: &VatAnimation, hit: f32, interval: f32) {
        let Some(clip) = animation.attack else {
            return;
        };
        if self.death.is_some() || !hit.is_finite() {
            return;
        }
        let duration = animation.duration(clip);
        let rate = if interval > 0.0 && interval < duration {
            duration / interval
        } else {
            1.0
        };
        let release = animation.playback.attack_release.min(duration);
        let start = hit - release / rate;
        if self
            .attack
            .is_some_and(|playing| (playing.start - start).abs() < 0.1)
        {
            return;
        }
        self.attack = Some(VatAction { clip, start, rate });
    }

    /// The unit died at clock time `at`: the death clip plays and holds its last frame.
    pub fn die(&mut self, animation: &VatAnimation, at: f32) {
        if self.death.is_none() && at.is_finite() {
            self.death = Some(VatAction {
                clip: animation.death.unwrap_or(animation.idle),
                start: at,
                rate: 1.0,
            });
            self.attack = None;
        }
    }

    /// Whether the unit has died.
    #[must_use]
    pub fn is_dead(&self) -> bool {
        self.death.is_some()
    }

    /// Whether to draw the unit at `now`: alive, or dead for less than
    /// [`VatAnimation::death_seconds`].
    #[must_use]
    pub fn visible(&self, animation: &VatAnimation, now: f32) -> bool {
        self.death
            .is_none_or(|death| now < death.start + animation.death_seconds())
    }

    /// The pose at clock time `now` for a unit moving at `speed` m/s, reduced to `quality`.
    /// Never allocates.
    #[must_use]
    pub fn pose(
        &self,
        animation: &VatAnimation,
        now: f32,
        speed: f32,
        quality: VatQuality,
    ) -> VatPose {
        let now = if now.is_finite() { now } else { 0.0 };
        let fade = animation.playback.crossfade;
        let ramp = |elapsed: f32| {
            if fade > 0.0 {
                (elapsed / fade).clamp(0.0, 1.0)
            } else if elapsed >= 0.0 {
                1.0
            } else {
                0.0
            }
        };
        if let Some(death) = self.death {
            let elapsed = now - death.start;
            let sample = animation.sample(death.clip, elapsed.max(0.0) * death.rate);
            let base = self.locomotion_sample(animation, animation.idle, now, 0.0);
            return quality.apply(VatPose::crossfade(base, sample, ramp(elapsed)));
        }
        let speed = if speed.is_finite() {
            speed.max(0.0)
        } else {
            0.0
        };
        let idle = self.locomotion_sample(animation, animation.idle, now, speed);
        let mut pose = match animation.run {
            Some(run) => {
                let weight = ((speed - MOVING_SPEED) / RUN_BLEND_SPAN).clamp(0.0, 1.0);
                if weight <= 0.0 {
                    VatPose::single(idle)
                } else {
                    let running = self.locomotion_sample(animation, run, now, speed);
                    VatPose::crossfade(idle, running, weight)
                }
            }
            None => VatPose::single(idle),
        };
        if let Some(attack) = self.attack {
            let duration = animation.duration(attack.clip) / attack.rate;
            let elapsed = now - attack.start;
            if elapsed >= 0.0 && elapsed < duration + fade {
                let fade_out = if fade > 0.0 {
                    (1.0 - (elapsed - duration) / fade).clamp(0.0, 1.0)
                } else if elapsed < duration {
                    1.0
                } else {
                    0.0
                };
                let sample = animation.sample(attack.clip, elapsed * attack.rate);
                let dominant = if pose.blend >= 0.5 {
                    pose.to
                } else {
                    pose.from
                };
                pose = VatPose::crossfade(dominant, sample, ramp(elapsed).min(fade_out));
            }
        }
        quality.apply(pose)
    }

    fn locomotion_sample(
        &self,
        animation: &VatAnimation,
        clip: u8,
        now: f32,
        speed: f32,
    ) -> VatSample {
        let rate = if Some(clip) == animation.run {
            (speed / animation.playback.run_speed).clamp(RUN_RATE_RANGE.0, RUN_RATE_RANGE.1)
        } else {
            1.0
        };
        animation.sample(clip, (now + self.phase) * rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skin::{
        AnimationClip, Interpolation, JointTransform, MeshSkin, Skeleton, Track, TrackPath,
    };
    use moba_core::math::{Mat4, Quat};

    /// A two-joint rig: a root and a child 1 m up; `swing` turns the child 90° about Z over
    /// one second, `hold` keeps the rest pose.
    fn mesh() -> MeshData {
        let rest = vec![
            JointTransform::IDENTITY,
            JointTransform {
                translation: Vec3::Y,
                ..JointTransform::IDENTITY
            },
        ];
        let quarter = Quat::from_rotation_z(core::f32::consts::FRAC_PI_2);
        let swing = Track::new(
            1,
            TrackPath::Rotation,
            Interpolation::Linear,
            vec![0.0, 1.0],
            vec![Quat::IDENTITY.to_array(), quarter.to_array()],
        )
        .expect("track");
        let idle = Track::new(
            0,
            TrackPath::Translation,
            Interpolation::Linear,
            vec![0.0, 0.5],
            vec![[0.0; 4], [0.0; 4]],
        )
        .expect("track");
        let skeleton = Skeleton::new(
            vec![None, Some(0)],
            rest,
            vec![Mat4::IDENTITY; 2],
            vec![Mat4::IDENTITY, Mat4::from_translation(-Vec3::Y)],
            None,
        )
        .expect("skeleton")
        .with_clips(vec![
            AnimationClip::new(vec![idle])
                .expect("clip")
                .with_name("idle")
                .expect("name"),
            AnimationClip::new(vec![swing])
                .expect("clip")
                .with_name("death")
                .expect("name"),
        ])
        .expect("clips");
        // A vertex on the root and one 1 m along the child's +X.
        let positions = vec![[0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
        let normals = vec![[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
        let skin = MeshSkin::new(
            vec![[0, 0, 0, 0], [1, 0, 0, 0], [1, 0, 0, 0]],
            vec![[255, 0, 0, 0], [255, 0, 0, 0], [255, 0, 0, 0]],
            skeleton,
        )
        .expect("skin");
        MeshData::new(positions, normals, vec![0, 1, 2])
            .expect("mesh")
            .with_skin(skin)
            .expect("skinned")
    }

    fn texel(texture: &TextureData, index: usize) -> [f32; 4] {
        let data = texture.data();
        core::array::from_fn(|lane| {
            let offset = index * 8 + lane * 2;
            crate::texture::f32_from_f16(u16::from_le_bytes([data[offset], data[offset + 1]]))
        })
    }

    fn bake() -> (TextureData, VatDescriptor) {
        bake_vat(
            &mesh(),
            "rig",
            &[
                VatBakeClip {
                    name: "idle",
                    looping: true,
                },
                VatBakeClip {
                    name: "death",
                    looping: false,
                },
            ],
            10.0,
            VatPlaybackParams::default(),
        )
        .expect("bake")
    }

    #[test]
    fn bakes_skinned_positions_and_normals() {
        let (texture, descriptor) = bake();
        // idle: 0.5 s looped at 10 fps = 5 frames; death: 1 s one-shot = 11 frames.
        assert_eq!(descriptor.clips[0].frames, 5);
        assert_eq!(descriptor.clips[1].first, 5);
        assert_eq!(descriptor.clips[1].frames, 11);
        assert_eq!(descriptor.frame_count, 16);
        assert_eq!((descriptor.width, descriptor.height), (96, 1));
        descriptor.check_texture(&texture).expect("texture matches");
        descriptor.check_mesh(&mesh()).expect("mesh matches");
        // Death's last frame: the child turned 90° about Z, so (1, 1, 0) → (0, 2, 0).
        let last = (5 + 10) * 3 + 1;
        let position = texel(&texture, last);
        assert!(
            (position[0] - 0.0).abs() < 1e-2 && (position[1] - 2.0).abs() < 1e-2,
            "{position:?}"
        );
        assert_eq!(position[3], 1.0);
        let normal = texel(&texture, 16 * 3 + last);
        assert!((normal[1] - 1.0).abs() < 1e-2, "{normal:?}");
        assert_eq!(normal[3], 0.0);
        // The first idle frame is the rest pose.
        assert_eq!(texel(&texture, 1)[..3], [1.0, 1.0, 0.0]);
    }

    #[test]
    fn descriptors_round_trip_and_reject_bad_input() {
        let (_, descriptor) = bake();
        let text = descriptor.to_toml();
        assert_eq!(VatDescriptor::parse(&text).expect("parses"), descriptor);
        let bad = [
            text.replace("width = 96", "width = 95"),
            text.replace("model = \"rig\"", "model = \"../x\""),
            text.replace("frames = 11", "frames = 12"),
            text.replace("fps = 10.0", "fps = 0.0"),
            text.replace("mesh_hash = \"0x", "mesh_hash = \""),
            format!("{text}\nextra = 1\n"),
            text.replace("crossfade = 0.15", "crossfade = 9.0"),
        ];
        for text in bad {
            assert!(VatDescriptor::parse(&text).is_err(), "{text}");
        }
        let mut other = mesh();
        other = MeshData::new(
            other
                .positions()
                .iter()
                .map(|p| [p[0] + 0.5, p[1], p[2]])
                .collect(),
            other.normals().to_vec(),
            other.indices().to_vec(),
        )
        .expect("mesh");
        assert!(
            descriptor.check_mesh(&other).is_err(),
            "re-exported model detected"
        );
        assert!(
            bake_vat(
                &mesh(),
                "rig",
                &[VatBakeClip {
                    name: "nope",
                    looping: true
                }],
                10.0,
                VatPlaybackParams::default()
            )
            .is_err()
        );
        assert_eq!(vat_extent(4096, 4096), None);
        // 4096² RGBA16F would be 128 MiB, over the KTX2 limit: capped at MAX_VAT_TEXELS.
        assert_eq!(vat_extent(2048, 2048), None);
        let (width, height) = vat_extent(1024, 4000).expect("fits");
        assert!(u64::from(width) * u64::from(height) * 8 + 4_096 <= MAX_KTX2_BYTES);
        assert!(u64::from(width) * u64::from(height) <= MAX_VAT_TEXELS);
        // Playback values a descriptor would reject are refused at bake time.
        let clip = [VatBakeClip::new("idle", true)];
        for playback in [
            VatPlaybackParams {
                crossfade: 5.0,
                ..VatPlaybackParams::default()
            },
            VatPlaybackParams {
                run_speed: -1.0,
                ..VatPlaybackParams::default()
            },
            VatPlaybackParams {
                attack_release: f32::NAN,
                ..VatPlaybackParams::default()
            },
        ] {
            assert!(matches!(
                bake_vat(&mesh(), "rig", &clip, 10.0, playback),
                Err(VatError::Invalid(_))
            ));
        }
        assert_eq!(vat_extent(1000, 10), Some((4096, 5)));
    }

    #[test]
    fn the_player_walks_attacks_and_dies() {
        let (_, descriptor) = bake();
        let animation = VatAnimation::bind(&descriptor);
        let mut player = VatPlayer::new(&animation, 0.0);
        // Idle: frame 0..5 looping at 10 fps.
        let pose = player.pose(&animation, 0.25, 0.0, VatQuality::FULL);
        assert_eq!((pose.from.frame, pose.from.next), (2, 3));
        assert!((pose.from.t - 0.5).abs() < 1e-4);
        assert_eq!(pose.blend, 0.0);
        let looped = player.pose(&animation, 0.45, 0.0, VatQuality::FULL);
        assert_eq!((looped.from.frame, looped.from.next), (4, 0), "idle loops");
        // Nearest-frame quality rounds and never blends.
        let nearest = player.pose(&animation, 0.26, 0.0, VatQuality::NEAREST);
        assert_eq!(
            (nearest.from.frame, nearest.from.next, nearest.from.t),
            (3, 3, 0.0)
        );
        // Death holds its last frame and stays visible for the clip plus the linger.
        player.die(&animation, 1.0);
        assert!(player.is_dead());
        let dying = player.pose(&animation, 1.05, 0.0, VatQuality::FULL);
        assert!(
            dying.blend > 0.0 && dying.blend < 1.0,
            "crossfades into death"
        );
        let dead = player.pose(&animation, 3.0, 0.0, VatQuality::FULL);
        assert_eq!((dead.to.frame, dead.to.next, dead.blend), (15, 15, 1.0));
        assert!(player.visible(&animation, 2.5));
        assert!(!player.visible(&animation, 1.0 + 1.0 + VAT_DEATH_LINGER + 0.01));
        // No attack clip in this asset: attacks are ignored.
        let mut fresh = VatPlayer::new(&animation, 0.0);
        fresh.attack(&animation, 0.5, 1.0);
        assert_eq!(
            fresh.pose(&animation, 0.5, 0.0, VatQuality::FULL).blend,
            0.0
        );
    }
}
