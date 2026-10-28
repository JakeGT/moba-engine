//! Bounded binary glTF (`.glb`) mesh loader (ADR-0007), with optional skinning (WP-4.1).
//!
//! Input is untrusted: a model file comes from a client-local asset directory that a player
//! (or a downloaded mod) controls. The loader therefore:
//!
//! - caps the file at [`MAX_GLB_BYTES`] and the JSON chunk at [`MAX_JSON_BYTES`] before
//!   parsing anything;
//! - parses the GLB container itself with checked arithmetic, then hands only the JSON chunk
//!   to `gltf-json` (the `gltf` crate with default features off: no file/URI import, no
//!   images, no base64);
//! - reads accessors itself through bounds-checked slices instead of the `gltf` reader, so
//!   no slice index or subtraction can panic;
//! - checks vertex/index budgets against [`MeshData::MAX_VERTICES`] and
//!   [`MeshData::MAX_INDICES`] *before* reserving memory;
//! - bounds the node walk (visits and depth), so cycles and huge graphs fail instead of
//!   looping or exhausting the stack.
//!
//! Supported: triangle-list primitives with `POSITION` (`f32` VEC3), optional `NORMAL`
//! (`f32` VEC3) and optional `u8`/`u16`/`u32` indices in the embedded BIN chunk; node
//! transforms of the default scene are baked into the vertices. Missing normals are replaced
//! by flat face normals, as the glTF specification requires. Sparse accessors, external
//! buffers and any `extensionsRequired` entry are rejected; materials, textures and morph
//! targets are ignored.
//!
//! **Skinning.** If the mesh nodes of the default scene use a skin, they must all use the
//! *same* one (a file mixing skinned and static meshes, or two skins, is rejected). Skinned
//! vertices stay in bind space (the skinned node's own transform is ignored, as glTF
//! requires), and the loader also reads:
//!
//! - `JOINTS_0` (`u8`/`u16` VEC4) and `WEIGHTS_0` (`f32` or normalised `u8`/`u16` VEC4) on
//!   every primitive. Weights are re-normalised and quantised to `UNORM8`, and a lane with
//!   zero weight has its joint index cleared. Further influence sets (`JOINTS_1`, ...) are
//!   ignored;
//! - the skin's joints (at most [`MAX_JOINTS`]): parent links, rest transforms, the static
//!   transform above each skeleton root, and `inverseBindMatrices` (identity if absent);
//! - the first animation that drives any joint: translation/rotation/scale channels with
//!   `f32` keys and `LINEAR`/`STEP` interpolation (`CUBICSPLINE` keeps only its values and
//!   plays linearly), capped at [`MAX_TRACK_KEYS`] keys per track and [`MAX_CLIP_KEYS`] per
//!   clip. Channels on non-joint nodes and morph weights are ignored.

use std::io::Read;
use std::path::Path;

use gltf::json::accessor::{ComponentType, GenericComponentType, Type};
use gltf::json::animation::{Interpolation as GltfInterpolation, Property};
use gltf::json::mesh::{Mode, Semantic};
use gltf::json::validation::Checked;
use moba_core::math::{Mat3, Mat4, Quat, Vec3};

use crate::mesh::{MeshData, MeshError};
use crate::skin::{
    AnimationClip, Interpolation, JointTransform, MAX_CLIP_KEYS, MAX_JOINTS, MAX_TRACK_KEYS,
    MeshSkin, Skeleton, SkinError, Track, TrackPath, quantize_weights,
};

/// Largest accepted `.glb` file (16 MiB).
pub const MAX_GLB_BYTES: usize = 16 << 20;
/// Largest accepted JSON chunk (256 KiB). Static meshes need a few KiB. The cap also bounds
/// the parsed document: a hostile array of empty `{}` objects expands to roughly 250 bytes
/// per 3 input bytes, so the worst case is tens of MiB, transiently, at load time.
pub const MAX_JSON_BYTES: usize = 256 << 10;
/// Nodes the scene walk may visit (shared subtrees count once per visit).
const MAX_NODE_VISITS: usize = 1_024;
/// Deepest node hierarchy accepted.
const MAX_NODE_DEPTH: usize = 32;
/// Primitives (including skipped non-triangle ones) the walk may inspect.
const MAX_PRIMITIVES: usize = 4_096;

const GLB_MAGIC: &[u8; 4] = b"glTF";
const GLB_HEADER: usize = 12;
const CHUNK_HEADER: usize = 8;
const CHUNK_JSON: u32 = 0x4E4F_534A;
const CHUNK_BIN: u32 = 0x004E_4942;

/// Why a `.glb` file was rejected.
#[derive(Debug)]
#[non_exhaustive]
pub enum GlbError {
    /// The file or its JSON chunk exceeds the size caps.
    TooLarge,
    /// Missing `glTF` magic, unsupported version or inconsistent total length.
    BadHeader,
    /// Missing or truncated JSON chunk.
    BadChunk,
    /// The JSON chunk is not a glTF document.
    Json(gltf::json::Error),
    /// A buffer is not the embedded BIN chunk (URIs are never followed).
    ExternalBuffer,
    /// An accessor references data the file does not contain, or has the wrong type.
    BadAccessor,
    /// Dangling node/mesh/scene index, cycle, or graph too large/deep.
    BadNodeGraph,
    /// A feature this loader deliberately does not support.
    Unsupported(&'static str),
    /// The assembled mesh violates a [`MeshData`] invariant (including the caps).
    Mesh(MeshError),
    /// Reading the file failed.
    Io(std::io::ErrorKind),
}

impl core::fmt::Display for GlbError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooLarge => write!(
                f,
                "glb exceeds size caps ({MAX_GLB_BYTES} bytes, {MAX_JSON_BYTES} JSON bytes)"
            ),
            Self::BadHeader => f.write_str("not a complete version-2 binary glTF file"),
            Self::BadChunk => f.write_str("missing or truncated glb JSON chunk"),
            Self::Json(error) => write!(f, "glb JSON: {error}"),
            Self::ExternalBuffer => f.write_str("buffers must be embedded in the glb BIN chunk"),
            Self::BadAccessor => f.write_str("invalid or out-of-bounds accessor"),
            Self::BadNodeGraph => f.write_str("invalid, cyclic or oversized node graph"),
            Self::Unsupported(what) => write!(f, "unsupported glTF feature: {what}"),
            Self::Mesh(error) => write!(f, "glb mesh: {error}"),
            Self::Io(kind) => write!(f, "glb read failed: {kind}"),
        }
    }
}

impl std::error::Error for GlbError {}

impl From<MeshError> for GlbError {
    fn from(error: MeshError) -> Self {
        Self::Mesh(error)
    }
}

impl From<SkinError> for GlbError {
    fn from(error: SkinError) -> Self {
        Self::Mesh(MeshError::Skin(error))
    }
}

/// Reads and parses a `.glb` file, refusing files larger than [`MAX_GLB_BYTES`] without
/// reading them.
///
/// # Errors
/// Returns [`GlbError::Io`] for file errors, otherwise as [`load_glb`].
pub fn load_glb_file(path: &Path) -> Result<MeshData, GlbError> {
    let file = std::fs::File::open(path).map_err(|error| GlbError::Io(error.kind()))?;
    let length = file
        .metadata()
        .map_err(|error| GlbError::Io(error.kind()))?
        .len();
    if length > MAX_GLB_BYTES as u64 {
        return Err(GlbError::TooLarge);
    }
    let mut bytes = Vec::with_capacity(length as usize);
    // `take` keeps the cap even if the file grows between `metadata` and `read`.
    file.take(MAX_GLB_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| GlbError::Io(error.kind()))?;
    load_glb(&bytes)
}

/// Parses a binary glTF file into one merged, validated mesh, skinned if it uses a skin.
///
/// Never panics. Allocation is bounded by the caps: the parsed JSON document (see
/// [`MAX_JSON_BYTES`]) plus at most [`MeshData::MAX_VERTICES`] vertices,
/// [`MeshData::MAX_INDICES`] indices, [`MAX_JOINTS`] joints and [`MAX_CLIP_KEYS`] keys.
///
/// # Errors
/// Returns a [`GlbError`] describing the first problem found.
pub fn load_glb(bytes: &[u8]) -> Result<MeshData, GlbError> {
    if bytes.len() > MAX_GLB_BYTES {
        return Err(GlbError::TooLarge);
    }
    let (json, bin) = split_chunks(bytes)?;
    let root: gltf::json::Root =
        gltf::json::deserialize::from_slice(json).map_err(GlbError::Json)?;
    if !root.extensions_required.is_empty() {
        return Err(GlbError::Unsupported("extensionsRequired"));
    }
    let mut builder = Builder::default();
    let scene = root
        .scene
        .map(|index| index.value())
        .or_else(|| (!root.scenes.is_empty()).then_some(0));
    if let Some(scene) = scene {
        let scene = root.scenes.get(scene).ok_or(GlbError::BadNodeGraph)?;
        let mut stack: Vec<(usize, Mat4, usize)> = Vec::new();
        let mut pushed = 0usize;
        for node in scene.nodes.iter().rev() {
            push_node(&mut stack, &mut pushed, node.value(), Mat4::IDENTITY, 0)?;
        }
        while let Some((index, parent, depth)) = stack.pop() {
            let node = root.nodes.get(index).ok_or(GlbError::BadNodeGraph)?;
            let world = parent * node_matrix(node);
            if let Some(mesh) = node.mesh {
                let skin = node.skin.map(|skin| skin.value());
                builder.set_skin(&root, skin)?;
                // A skinned mesh is in bind space; its node transform is ignored (glTF 2.0)
                // because the joints place it.
                let transform = if skin.is_some() {
                    Mat4::IDENTITY
                } else {
                    world
                };
                builder.append_mesh(&root, bin, mesh.value(), transform)?;
            }
            if let Some(children) = &node.children {
                for child in children.iter().rev() {
                    push_node(&mut stack, &mut pushed, child.value(), world, depth + 1)?;
                }
            }
        }
    } else {
        // No scene: the file is a mesh library; take every mesh untransformed.
        if root.meshes.len() > MAX_NODE_VISITS {
            return Err(GlbError::BadNodeGraph);
        }
        for mesh in 0..root.meshes.len() {
            builder.append_mesh(&root, bin, mesh, Mat4::IDENTITY)?;
        }
    }
    let skin = builder.skin;
    let mesh = MeshData::new(builder.positions, builder.normals, builder.indices)?;
    let SkinMode::Skinned(skin) = skin else {
        return Ok(mesh);
    };
    let skeleton = read_skeleton(&root, bin, skin)?;
    let skin = MeshSkin::new(builder.joints, builder.weights, skeleton)?;
    Ok(mesh.with_skin(skin)?)
}

/// Node index → parent node index for every node, rejecting dangling or shared children.
fn parent_map(root: &gltf::json::Root) -> Result<Vec<Option<usize>>, GlbError> {
    let mut parents = vec![None; root.nodes.len()];
    for (index, node) in root.nodes.iter().enumerate() {
        for child in node.children.iter().flatten() {
            let slot = parents
                .get_mut(child.value())
                .ok_or(GlbError::BadNodeGraph)?;
            if slot.is_some() || child.value() == index {
                return Err(GlbError::BadNodeGraph);
            }
            *slot = Some(index);
        }
    }
    Ok(parents)
}

/// A node's local transform as translation/rotation/scale (decomposing a `matrix`).
fn node_trs(node: &gltf::json::Node) -> Result<JointTransform, GlbError> {
    if let Some(matrix) = node.matrix {
        let matrix = Mat4::from_cols_array(&matrix);
        if !matrix.is_finite() || matrix.determinant().abs() < 1e-12 {
            return Err(SkinError::NonFinite.into());
        }
        let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
        return Ok(JointTransform {
            translation,
            rotation,
            scale,
        });
    }
    Ok(JointTransform {
        translation: Vec3::from_array(node.translation.unwrap_or([0.0; 3])),
        rotation: node
            .rotation
            .map_or(Quat::IDENTITY, |rotation| Quat::from_array(rotation.0)),
        scale: Vec3::from_array(node.scale.unwrap_or([1.0; 3])),
    })
}

/// Reads skin `index`: hierarchy, rest pose, inverse bind matrices and one clip.
fn read_skeleton(
    root: &gltf::json::Root,
    bin: Option<&[u8]>,
    index: usize,
) -> Result<Skeleton, GlbError> {
    let skin = root.skins.get(index).ok_or(GlbError::BadNodeGraph)?;
    if skin.joints.is_empty() || skin.joints.len() > MAX_JOINTS {
        return Err(SkinError::JointCount.into());
    }
    let joints: Vec<usize> = skin.joints.iter().map(gltf::json::Index::value).collect();
    for (position, joint) in joints.iter().enumerate() {
        if *joint >= root.nodes.len() || joints[..position].contains(joint) {
            return Err(GlbError::BadNodeGraph);
        }
    }
    let joint_of = |node: usize| joints.iter().position(|&joint| joint == node);
    let node_parents = parent_map(root)?;
    let count = joints.len();
    let mut parents = Vec::with_capacity(count);
    let mut rest = Vec::with_capacity(count);
    let mut base = Vec::with_capacity(count);
    for &node in &joints {
        rest.push(node_trs(&root.nodes[node])?);
        let parent = node_parents[node];
        match parent.map(|parent| (parent, joint_of(parent))) {
            // `count <= MAX_JOINTS`, so every joint index fits `u8`.
            Some((_, Some(joint))) => {
                parents.push(Some(
                    u8::try_from(joint).map_err(|_| SkinError::JointCount)?,
                ));
                base.push(Mat4::IDENTITY);
            }
            Some((parent, None)) => {
                parents.push(None);
                base.push(static_global(root, &node_parents, parent, &joint_of)?);
            }
            None => {
                parents.push(None);
                base.push(Mat4::IDENTITY);
            }
        }
    }
    let inverse_bind = match skin.inverse_bind_matrices {
        Some(accessor_index) => {
            let data = accessor(
                root,
                bin,
                accessor_index.value(),
                Type::Mat4,
                &[ComponentType::F32],
            )?;
            if data.count != count {
                return Err(GlbError::BadAccessor);
            }
            (0..count)
                .map(|element| {
                    data.f32s::<16>(element)
                        .map(|values| Mat4::from_cols_array(&values))
                        .ok_or(GlbError::BadAccessor)
                })
                .collect::<Result<Vec<_>, _>>()?
        }
        None => vec![Mat4::IDENTITY; count],
    };
    let clip = read_clip(root, bin, &joint_of)?;
    Ok(Skeleton::new(parents, rest, base, inverse_bind, clip)?)
}

/// World transform of a non-joint node above a skeleton root. Its ancestors must not be
/// joints of the skin (that would make it animated), and the walk is bounded.
fn static_global(
    root: &gltf::json::Root,
    node_parents: &[Option<usize>],
    node: usize,
    joint_of: &impl Fn(usize) -> Option<usize>,
) -> Result<Mat4, GlbError> {
    let mut world = Mat4::IDENTITY;
    let mut current = Some(node);
    for _ in 0..=MAX_NODE_DEPTH {
        let Some(index) = current else {
            return Ok(world);
        };
        if joint_of(index).is_some() {
            return Err(GlbError::Unsupported("skin joint below a non-joint node"));
        }
        let ancestor = root.nodes.get(index).ok_or(GlbError::BadNodeGraph)?;
        world = node_matrix(ancestor) * world;
        current = node_parents.get(index).copied().flatten();
    }
    Err(GlbError::BadNodeGraph)
}

/// The first animation that drives any joint, as a clip.
fn read_clip(
    root: &gltf::json::Root,
    bin: Option<&[u8]>,
    joint_of: &impl Fn(usize) -> Option<usize>,
) -> Result<Option<AnimationClip>, GlbError> {
    let mut inspected = 0usize;
    let mut keys = 0usize;
    for animation in &root.animations {
        let mut tracks = Vec::new();
        for channel in &animation.channels {
            inspected += 1;
            if inspected > MAX_PRIMITIVES {
                return Err(GlbError::BadNodeGraph);
            }
            let Some(joint) = joint_of(channel.target.node.value()) else {
                continue; // Not a joint of this skin.
            };
            let (path, kind) = match channel.target.path {
                Checked::Valid(Property::Translation) => (TrackPath::Translation, Type::Vec3),
                Checked::Valid(Property::Rotation) => (TrackPath::Rotation, Type::Vec4),
                Checked::Valid(Property::Scale) => (TrackPath::Scale, Type::Vec3),
                Checked::Valid(Property::MorphTargetWeights) => continue,
                Checked::Invalid => return Err(GlbError::BadAccessor),
            };
            let sampler = animation
                .samplers
                .get(channel.sampler.value())
                .ok_or(GlbError::BadNodeGraph)?;
            let (interpolation, stride) = match sampler.interpolation {
                Checked::Valid(GltfInterpolation::Linear) => (Interpolation::Linear, 1),
                Checked::Valid(GltfInterpolation::Step) => (Interpolation::Step, 1),
                // In-tangent, value, out-tangent per key: keep the values, play linearly.
                Checked::Valid(GltfInterpolation::CubicSpline) => (Interpolation::Linear, 3),
                Checked::Invalid => return Err(GlbError::BadAccessor),
            };
            let input = accessor(
                root,
                bin,
                sampler.input.value(),
                Type::Scalar,
                &[ComponentType::F32],
            )?;
            let output = accessor(
                root,
                bin,
                sampler.output.value(),
                kind,
                &[ComponentType::F32],
            )?;
            // Caps before reserving: per track, then for the whole clip.
            if input.count > MAX_TRACK_KEYS {
                return Err(SkinError::BadTrack.into());
            }
            keys = keys.saturating_add(input.count);
            if keys > MAX_CLIP_KEYS {
                return Err(SkinError::ClipTooLarge.into());
            }
            if input.count.checked_mul(stride) != Some(output.count) {
                return Err(GlbError::BadAccessor);
            }
            let mut times = Vec::with_capacity(input.count);
            let mut values = Vec::with_capacity(input.count);
            for key in 0..input.count {
                let [time] = input.f32s::<1>(key).ok_or(GlbError::BadAccessor)?;
                // The value is the middle element of a cubic-spline triple.
                let element = key * stride + stride / 2;
                let value = if kind == Type::Vec4 {
                    output.f32s::<4>(element)
                } else {
                    output.f32s::<3>(element).map(|[x, y, z]| [x, y, z, 0.0])
                }
                .ok_or(GlbError::BadAccessor)?;
                times.push(time);
                values.push(value);
            }
            // `joint < MAX_JOINTS`, so it fits `u8`.
            let joint = u8::try_from(joint).map_err(|_| SkinError::JointCount)?;
            tracks.push(Track::new(joint, path, interpolation, times, values)?);
        }
        if !tracks.is_empty() {
            return Ok(Some(AnimationClip::new(tracks)?));
        }
    }
    Ok(None)
}

fn push_node(
    stack: &mut Vec<(usize, Mat4, usize)>,
    pushed: &mut usize,
    index: usize,
    parent: Mat4,
    depth: usize,
) -> Result<(), GlbError> {
    *pushed += 1;
    if *pushed > MAX_NODE_VISITS || depth > MAX_NODE_DEPTH {
        return Err(GlbError::BadNodeGraph);
    }
    stack.push((index, parent, depth));
    Ok(())
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    Some(u32::from_le_bytes(bytes.get(offset..end)?.try_into().ok()?))
}

/// Splits a GLB container into its JSON chunk and optional BIN chunk.
fn split_chunks(bytes: &[u8]) -> Result<(&[u8], Option<&[u8]>), GlbError> {
    if bytes.get(..4) != Some(GLB_MAGIC.as_slice()) {
        return Err(GlbError::BadHeader);
    }
    let version = read_u32(bytes, 4).ok_or(GlbError::BadHeader)?;
    let total = read_u32(bytes, 8).ok_or(GlbError::BadHeader)? as usize;
    if version != 2 || total < GLB_HEADER + CHUNK_HEADER || total > bytes.len() {
        return Err(GlbError::BadHeader);
    }
    let body = bytes.get(GLB_HEADER..total).ok_or(GlbError::BadHeader)?;
    let json_len = read_u32(body, 0).ok_or(GlbError::BadChunk)? as usize;
    if read_u32(body, 4) != Some(CHUNK_JSON) {
        return Err(GlbError::BadChunk);
    }
    if json_len > MAX_JSON_BYTES {
        return Err(GlbError::TooLarge);
    }
    let json_end = CHUNK_HEADER
        .checked_add(json_len)
        .ok_or(GlbError::BadChunk)?;
    let json = body.get(CHUNK_HEADER..json_end).ok_or(GlbError::BadChunk)?;
    let rest = body.get(json_end..).unwrap_or_default();
    // Only the chunk directly after JSON may be BIN; unknown chunk types are ignored.
    let bin = match (read_u32(rest, 0), read_u32(rest, 4)) {
        (Some(length), Some(CHUNK_BIN)) => {
            let end = CHUNK_HEADER
                .checked_add(length as usize)
                .ok_or(GlbError::BadChunk)?;
            Some(rest.get(CHUNK_HEADER..end).ok_or(GlbError::BadChunk)?)
        }
        _ => None,
    };
    Ok((json, bin))
}

fn node_matrix(node: &gltf::json::Node) -> Mat4 {
    if let Some(matrix) = node.matrix {
        return Mat4::from_cols_array(&matrix);
    }
    let translation = Vec3::from_array(node.translation.unwrap_or([0.0; 3]));
    let scale = Vec3::from_array(node.scale.unwrap_or([1.0; 3]));
    let rotation = node
        .rotation
        .map_or(Quat::IDENTITY, |rotation| Quat::from_array(rotation.0));
    // An unnormalised or degenerate quaternion is a malformed file, not a reason to panic;
    // treat it as no rotation.
    let rotation = if rotation.is_finite() && rotation.length_squared() > 1e-12 {
        rotation.normalize()
    } else {
        Quat::IDENTITY
    };
    Mat4::from_scale_rotation_translation(scale, rotation, translation)
}

const fn component_size(component: ComponentType) -> usize {
    match component {
        ComponentType::I8 | ComponentType::U8 => 1,
        ComponentType::I16 | ComponentType::U16 => 2,
        ComponentType::U32 | ComponentType::F32 => 4,
    }
}

/// A bounds-checked view of one accessor's elements inside the BIN chunk.
struct AccessorData<'a> {
    bytes: &'a [u8],
    stride: usize,
    count: usize,
    component: ComponentType,
}

impl AccessorData<'_> {
    fn f32_vec3(&self, element: usize) -> Option<[f32; 3]> {
        self.f32s::<3>(element)
    }

    /// `N` consecutive `f32` lanes of one element.
    fn f32s<const N: usize>(&self, element: usize) -> Option<[f32; N]> {
        let base = element.checked_mul(self.stride)?;
        let mut out = [0.0; N];
        for (lane, value) in out.iter_mut().enumerate() {
            *value = f32::from_bits(read_u32(self.bytes, base.checked_add(lane * 4)?)?);
        }
        Some(out)
    }

    /// Lane `lane` of an unsigned 8/16-bit element, widened.
    fn small_uint(&self, element: usize, lane: usize) -> Option<u32> {
        let size = component_size(self.component);
        let offset = element
            .checked_mul(self.stride)?
            .checked_add(lane.checked_mul(size)?)?;
        match self.component {
            ComponentType::U8 => self.bytes.get(offset).copied().map(u32::from),
            ComponentType::U16 => {
                let end = offset.checked_add(2)?;
                let pair: [u8; 2] = self.bytes.get(offset..end)?.try_into().ok()?;
                Some(u32::from(u16::from_le_bytes(pair)))
            }
            _ => None,
        }
    }

    /// A `u8`/`u16` VEC4 (`JOINTS_0`).
    fn uint_vec4(&self, element: usize) -> Option<[u32; 4]> {
        Some([
            self.small_uint(element, 0)?,
            self.small_uint(element, 1)?,
            self.small_uint(element, 2)?,
            self.small_uint(element, 3)?,
        ])
    }

    /// An `f32` or normalised `u8`/`u16` VEC4 (`WEIGHTS_0`) as `f32`.
    fn unorm_vec4(&self, element: usize) -> Option<[f32; 4]> {
        let scale = match self.component {
            ComponentType::F32 => return self.f32s::<4>(element),
            ComponentType::U8 => 255.0,
            ComponentType::U16 => 65_535.0,
            _ => return None,
        };
        Some(self.uint_vec4(element)?.map(|value| value as f32 / scale))
    }

    fn index(&self, element: usize) -> Option<u32> {
        let base = element.checked_mul(self.stride)?;
        match self.component {
            ComponentType::U8 => self.bytes.get(base).copied().map(u32::from),
            ComponentType::U16 => {
                let end = base.checked_add(2)?;
                let pair: [u8; 2] = self.bytes.get(base..end)?.try_into().ok()?;
                Some(u32::from(u16::from_le_bytes(pair)))
            }
            ComponentType::U32 => read_u32(self.bytes, base),
            _ => None,
        }
    }
}

fn accessor<'a>(
    root: &gltf::json::Root,
    bin: Option<&'a [u8]>,
    index: usize,
    kind: Type,
    allowed: &[ComponentType],
) -> Result<AccessorData<'a>, GlbError> {
    let accessor = root.accessors.get(index).ok_or(GlbError::BadAccessor)?;
    if accessor.sparse.is_some() {
        return Err(GlbError::Unsupported("sparse accessor"));
    }
    let Checked::Valid(GenericComponentType(component)) = accessor.component_type else {
        return Err(GlbError::BadAccessor);
    };
    if accessor.type_ != Checked::Valid(kind) || !allowed.contains(&component) {
        return Err(GlbError::BadAccessor);
    }
    let view_index = accessor.buffer_view.ok_or(GlbError::BadAccessor)?.value();
    let view = root
        .buffer_views
        .get(view_index)
        .ok_or(GlbError::BadAccessor)?;
    let buffer_index = view.buffer.value();
    let buffer = root
        .buffers
        .get(buffer_index)
        .ok_or(GlbError::BadAccessor)?;
    // Only buffer 0 without a URI refers to the GLB BIN chunk.
    if buffer_index != 0 || buffer.uri.is_some() {
        return Err(GlbError::ExternalBuffer);
    }
    let bin = bin.ok_or(GlbError::ExternalBuffer)?;
    let size = |value: u64| usize::try_from(value).map_err(|_| GlbError::BadAccessor);
    let element = component_size(component) * kind.multiplicity();
    let stride = match view.byte_stride {
        Some(stride) if stride.0 < element || stride.0 > 252 => {
            return Err(GlbError::BadAccessor);
        }
        Some(stride) => stride.0,
        None => element,
    };
    let count = size(accessor.count.0)?;
    if count == 0 {
        return Err(GlbError::BadAccessor);
    }
    let view_start = size(view.byte_offset.map_or(0, |offset| offset.0))?;
    let view_length = size(view.byte_length.0)?;
    let view_end = view_start
        .checked_add(view_length)
        .ok_or(GlbError::BadAccessor)?;
    let view_bytes = bin.get(view_start..view_end).ok_or(GlbError::BadAccessor)?;
    let accessor_start = size(accessor.byte_offset.map_or(0, |offset| offset.0))?;
    let accessor_end = (count - 1)
        .checked_mul(stride)
        .and_then(|span| span.checked_add(element))
        .and_then(|span| span.checked_add(accessor_start))
        .ok_or(GlbError::BadAccessor)?;
    let bytes = view_bytes
        .get(accessor_start..accessor_end)
        .ok_or(GlbError::BadAccessor)?;
    Ok(AccessorData {
        bytes,
        stride,
        count,
        component,
    })
}

/// One primitive's skinning attributes and the skin's joint count.
struct SkinStreams<'a> {
    joints: AccessorData<'a>,
    weights: AccessorData<'a>,
    joint_count: usize,
}

impl SkinStreams<'_> {
    /// Joints and quantised weights of one vertex; zero-weight lanes get joint 0.
    fn vertex(&self, element: usize) -> Result<([u8; 4], [u8; 4]), GlbError> {
        let raw = self
            .weights
            .unorm_vec4(element)
            .ok_or(GlbError::BadAccessor)?;
        let weights = quantize_weights(raw).ok_or(GlbError::BadAccessor)?;
        let indices = self
            .joints
            .uint_vec4(element)
            .ok_or(GlbError::BadAccessor)?;
        let mut joints = [0u8; 4];
        for lane in 0..4 {
            if weights[lane] == 0 {
                continue;
            }
            joints[lane] = u8::try_from(indices[lane])
                .ok()
                .filter(|&joint| usize::from(joint) < self.joint_count)
                .ok_or(SkinError::JointOutOfRange)?;
        }
        Ok((joints, weights))
    }
}

/// Whether the mesh nodes seen so far are skinned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SkinMode {
    /// No mesh node yet.
    #[default]
    Unset,
    /// Static meshes, transforms baked in.
    Static,
    /// Every mesh node uses this skin.
    Skinned(usize),
}

impl SkinMode {
    fn of(skin: Option<usize>) -> Self {
        skin.map_or(Self::Static, Self::Skinned)
    }
}

/// Accumulates primitives into one mesh while enforcing the caps before allocating.
#[derive(Default)]
struct Builder {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    indices: Vec<u32>,
    joints: Vec<[u8; 4]>,
    weights: Vec<[u8; 4]>,
    primitives: usize,
    skin: SkinMode,
    joint_count: usize,
}

impl Builder {
    /// Records the first mesh node's skin and rejects files that mix skins.
    fn set_skin(&mut self, root: &gltf::json::Root, skin: Option<usize>) -> Result<(), GlbError> {
        match self.skin {
            SkinMode::Unset => {
                if let Some(index) = skin {
                    let joints = root
                        .skins
                        .get(index)
                        .ok_or(GlbError::BadNodeGraph)?
                        .joints
                        .len();
                    if joints == 0 || joints > MAX_JOINTS {
                        return Err(SkinError::JointCount.into());
                    }
                    self.joint_count = joints;
                }
                self.skin = SkinMode::of(skin);
                Ok(())
            }
            current if current == SkinMode::of(skin) => Ok(()),
            _ => Err(GlbError::Unsupported(
                "mesh nodes must all use the same skin (or none)",
            )),
        }
    }

    fn skinned(&self) -> bool {
        matches!(self.skin, SkinMode::Skinned(_))
    }

    fn reserve(&mut self, vertices: usize, indices: usize) -> Result<(), GlbError> {
        let total_vertices = self.positions.len().saturating_add(vertices);
        let total_indices = self.indices.len().saturating_add(indices);
        if total_vertices > MeshData::MAX_VERTICES {
            return Err(MeshError::TooManyVertices.into());
        }
        if total_indices > MeshData::MAX_INDICES {
            return Err(MeshError::TooManyIndices.into());
        }
        self.positions.reserve(vertices);
        self.normals.reserve(vertices);
        self.indices.reserve(indices);
        if self.skinned() {
            self.joints.reserve(vertices);
            self.weights.reserve(vertices);
        }
        Ok(())
    }

    fn push_skin(
        &mut self,
        skin: Option<&SkinStreams<'_>>,
        element: usize,
    ) -> Result<(), GlbError> {
        if let Some(skin) = skin {
            let (joints, weights) = skin.vertex(element)?;
            self.joints.push(joints);
            self.weights.push(weights);
        }
        Ok(())
    }

    /// `JOINTS_0`/`WEIGHTS_0` of a primitive when the mesh is skinned.
    fn skin_streams<'a>(
        &self,
        root: &gltf::json::Root,
        bin: Option<&'a [u8]>,
        primitive: &gltf::json::mesh::Primitive,
        vertex_count: usize,
    ) -> Result<Option<SkinStreams<'a>>, GlbError> {
        if !self.skinned() {
            return Ok(None);
        }
        let attributes = &primitive.attributes;
        let (Some(joints), Some(weights)) = (
            attributes.get(&Checked::Valid(Semantic::Joints(0))),
            attributes.get(&Checked::Valid(Semantic::Weights(0))),
        ) else {
            return Err(GlbError::Unsupported(
                "skinned primitive without JOINTS_0/WEIGHTS_0",
            ));
        };
        let joints = accessor(
            root,
            bin,
            joints.value(),
            Type::Vec4,
            &[ComponentType::U8, ComponentType::U16],
        )?;
        let weights = accessor(
            root,
            bin,
            weights.value(),
            Type::Vec4,
            &[ComponentType::F32, ComponentType::U8, ComponentType::U16],
        )?;
        if joints.count != vertex_count || weights.count != vertex_count {
            return Err(GlbError::BadAccessor);
        }
        Ok(Some(SkinStreams {
            joints,
            weights,
            joint_count: self.joint_count,
        }))
    }

    fn append_mesh(
        &mut self,
        root: &gltf::json::Root,
        bin: Option<&[u8]>,
        mesh: usize,
        world: Mat4,
    ) -> Result<(), GlbError> {
        let mesh = root.meshes.get(mesh).ok_or(GlbError::BadNodeGraph)?;
        let linear = Mat3::from_mat4(world);
        // A collapsed transform (zero scale on an axis) draws nothing; skip it rather than
        // producing non-finite normals.
        if !linear.is_finite() || linear.determinant().abs() < 1e-12 {
            return Ok(());
        }
        let normal_matrix = linear.inverse().transpose();
        for primitive in &mesh.primitives {
            self.primitives += 1;
            if self.primitives > MAX_PRIMITIVES {
                return Err(GlbError::BadNodeGraph);
            }
            if primitive.mode != Checked::Valid(Mode::Triangles) {
                continue; // Points, lines, strips and fans are not drawn.
            }
            let Some(position_index) = primitive
                .attributes
                .get(&Checked::Valid(Semantic::Positions))
            else {
                continue;
            };
            let positions = accessor(
                root,
                bin,
                position_index.value(),
                Type::Vec3,
                &[ComponentType::F32],
            )?;
            let normals = primitive
                .attributes
                .get(&Checked::Valid(Semantic::Normals))
                .map(|index| accessor(root, bin, index.value(), Type::Vec3, &[ComponentType::F32]))
                .transpose()?;
            if normals
                .as_ref()
                .is_some_and(|normals| normals.count != positions.count)
            {
                return Err(GlbError::BadAccessor);
            }
            let indices = primitive
                .indices
                .map(|index| {
                    accessor(
                        root,
                        bin,
                        index.value(),
                        Type::Scalar,
                        &[ComponentType::U8, ComponentType::U16, ComponentType::U32],
                    )
                })
                .transpose()?;
            let index_count = indices.as_ref().map_or(positions.count, |data| data.count);
            if !index_count.is_multiple_of(3) {
                return Err(MeshError::NotTriangles.into());
            }
            let skin = self.skin_streams(root, bin, primitive, positions.count)?;
            match normals {
                Some(normals) => self.append_indexed(
                    &positions,
                    &normals,
                    indices.as_ref(),
                    index_count,
                    world,
                    normal_matrix,
                    skin.as_ref(),
                )?,
                None => self.append_flat(
                    &positions,
                    indices.as_ref(),
                    index_count,
                    world,
                    skin.as_ref(),
                )?,
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // Accessor views plus the node transform.
    fn append_indexed(
        &mut self,
        positions: &AccessorData<'_>,
        normals: &AccessorData<'_>,
        indices: Option<&AccessorData<'_>>,
        index_count: usize,
        world: Mat4,
        normal_matrix: Mat3,
        skin: Option<&SkinStreams<'_>>,
    ) -> Result<(), GlbError> {
        self.reserve(positions.count, index_count)?;
        let base = u32::try_from(self.positions.len()).map_err(|_| MeshError::TooManyVertices)?;
        for element in 0..positions.count {
            let position = positions.f32_vec3(element).ok_or(GlbError::BadAccessor)?;
            let normal = normals.f32_vec3(element).ok_or(GlbError::BadAccessor)?;
            let position = world.transform_point3(Vec3::from_array(position));
            let normal = (normal_matrix * Vec3::from_array(normal)).normalize_or(Vec3::Y);
            self.positions.push(position.to_array());
            self.normals.push(normal.to_array());
            self.push_skin(skin, element)?;
        }
        let vertex_count = positions.count as u32; // <= MAX_VERTICES, checked by `reserve`.
        for element in 0..index_count {
            let index = match indices {
                Some(data) => data.index(element).ok_or(GlbError::BadAccessor)?,
                None => element as u32,
            };
            if index >= vertex_count {
                return Err(MeshError::IndexOutOfRange.into());
            }
            self.indices.push(base + index);
        }
        Ok(())
    }

    /// Emits one vertex per corner with the face normal (glTF: missing normals are flat).
    fn append_flat(
        &mut self,
        positions: &AccessorData<'_>,
        indices: Option<&AccessorData<'_>>,
        index_count: usize,
        world: Mat4,
        skin: Option<&SkinStreams<'_>>,
    ) -> Result<(), GlbError> {
        self.reserve(index_count, index_count)?;
        let base = u32::try_from(self.positions.len()).map_err(|_| MeshError::TooManyVertices)?;
        let mut corners = [Vec3::ZERO; 3];
        let mut sources = [0usize; 3];
        for triangle in 0..index_count / 3 {
            for (corner, slot) in corners.iter_mut().enumerate() {
                let element = triangle * 3 + corner;
                let index = match indices {
                    Some(data) => data.index(element).ok_or(GlbError::BadAccessor)? as usize,
                    None => element,
                };
                if index >= positions.count {
                    return Err(MeshError::IndexOutOfRange.into());
                }
                let position = positions.f32_vec3(index).ok_or(GlbError::BadAccessor)?;
                *slot = world.transform_point3(Vec3::from_array(position));
                sources[corner] = index;
            }
            let normal = (corners[1] - corners[0])
                .cross(corners[2] - corners[0])
                .normalize_or(Vec3::Y);
            for (corner, source) in corners.iter().zip(sources) {
                self.positions.push(corner.to_array());
                self.normals.push(normal.to_array());
                self.push_skin(skin, source)?;
            }
        }
        for element in 0..index_count {
            self.indices.push(base + element as u32);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a GLB container around `json` and an optional BIN chunk.
    fn glb(json: &str, bin: Option<&[u8]>) -> Vec<u8> {
        let mut json = json.as_bytes().to_vec();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let mut out = Vec::new();
        out.extend_from_slice(GLB_MAGIC);
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(&CHUNK_JSON.to_le_bytes());
        out.extend_from_slice(&json);
        if let Some(bin) = bin {
            let mut bin = bin.to_vec();
            while !bin.len().is_multiple_of(4) {
                bin.push(0);
            }
            out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
            out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
            out.extend_from_slice(&bin);
        }
        let total = out.len() as u32;
        out[8..12].copy_from_slice(&total.to_le_bytes());
        out
    }

    fn floats(values: &[f32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    /// One triangle: positions at 0, normals at 36, u16 indices at 72.
    fn triangle_bin() -> Vec<u8> {
        let mut bin = floats(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        bin.extend(floats(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]));
        bin.extend([0u16, 1, 2].iter().flat_map(|index| index.to_le_bytes()));
        bin
    }

    fn triangle_json(extra_node: &str, accessor0: &str) -> String {
        format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],
            "nodes":[{{"mesh":0{extra_node}}}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1}},"indices":2}}]}}],
            "buffers":[{{"byteLength":80}}],
            "bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},
                           {{"buffer":0,"byteOffset":36,"byteLength":36}},
                           {{"buffer":0,"byteOffset":72,"byteLength":6}}],
            "accessors":[{accessor0},
                         {{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"}},
                         {{"bufferView":2,"componentType":5123,"count":3,"type":"SCALAR"}}]}}"#
        )
    }

    const POSITION_ACCESSOR: &str =
        r#"{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}"#;

    #[test]
    fn loads_an_indexed_triangle_with_a_node_transform() {
        let json = triangle_json(r#","translation":[0,2,0]"#, POSITION_ACCESSOR);
        let mesh = load_glb(&glb(&json, Some(&triangle_bin()))).expect("valid triangle");
        assert_eq!(mesh.vertex_count(), 3);
        assert_eq!(mesh.indices(), &[0, 1, 2]);
        assert_eq!(mesh.positions()[1], [1.0, 2.0, 0.0]);
        assert_eq!(mesh.normals()[0], [0.0, 0.0, 1.0]);
    }

    #[test]
    fn missing_normals_become_flat_face_normals() {
        let json = r#"{"asset":{"version":"2.0"},
            "meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],
            "buffers":[{"byteLength":36}],
            "bufferViews":[{"buffer":0,"byteLength":36}],
            "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}]}"#;
        let bin = floats(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        let mesh = load_glb(&glb(json, Some(&bin))).expect("scene-less mesh library");
        assert_eq!(mesh.triangle_count(), 1);
        assert_eq!(mesh.normals(), &[[0.0, 0.0, 1.0]; 3]);
    }

    #[test]
    fn rejects_malformed_containers() {
        let valid = glb(&triangle_json("", POSITION_ACCESSOR), Some(&triangle_bin()));
        assert!(matches!(load_glb(&[]), Err(GlbError::BadHeader)));
        assert!(matches!(load_glb(b"glTF"), Err(GlbError::BadHeader)));
        let mut wrong_magic = valid.clone();
        wrong_magic[0] = b'x';
        assert!(matches!(load_glb(&wrong_magic), Err(GlbError::BadHeader)));
        let mut version_one = valid.clone();
        version_one[4] = 1;
        assert!(matches!(load_glb(&version_one), Err(GlbError::BadHeader)));
        let mut long_total = valid.clone();
        long_total[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(load_glb(&long_total), Err(GlbError::BadHeader)));
        let mut short_total = valid.clone();
        short_total[8..12].copy_from_slice(&4u32.to_le_bytes());
        assert!(matches!(load_glb(&short_total), Err(GlbError::BadHeader)));
        let mut json_overflow = valid.clone();
        json_overflow[12..16].copy_from_slice(&(MAX_JSON_BYTES as u32 - 4).to_le_bytes());
        assert!(matches!(load_glb(&json_overflow), Err(GlbError::BadChunk)));
        let mut json_huge = valid.clone();
        json_huge[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(load_glb(&json_huge), Err(GlbError::TooLarge)));
        let mut wrong_chunk = valid;
        wrong_chunk[16] = b'X';
        assert!(matches!(load_glb(&wrong_chunk), Err(GlbError::BadChunk)));
        let oversized = vec![0u8; MAX_GLB_BYTES + 1];
        assert!(matches!(load_glb(&oversized), Err(GlbError::TooLarge)));
        assert!(matches!(
            load_glb(&glb("not json", None)),
            Err(GlbError::Json(_))
        ));
        assert!(matches!(
            load_glb(&glb("[1,2,3]", None)),
            Err(GlbError::Json(_))
        ));
    }

    #[test]
    fn rejects_bad_buffers_and_accessors() {
        let bin = triangle_bin();
        // No BIN chunk at all.
        let no_bin = glb(&triangle_json("", POSITION_ACCESSOR), None);
        assert!(matches!(load_glb(&no_bin), Err(GlbError::ExternalBuffer)));
        // External URI buffers are never followed.
        let uri = triangle_json("", POSITION_ACCESSOR).replace(
            r#""byteLength":80"#,
            r#""byteLength":80,"uri":"/etc/passwd""#,
        );
        assert!(matches!(
            load_glb(&glb(&uri, Some(&bin))),
            Err(GlbError::ExternalBuffer)
        ));
        let cases = [
            // Count runs past the view.
            r#"{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3"}"#,
            // Huge count must not allocate.
            r#"{"bufferView":0,"componentType":5126,"count":4000000000,"type":"VEC3"}"#,
            // Offset past the view.
            r#"{"bufferView":0,"byteOffset":40,"componentType":5126,"count":1,"type":"VEC3"}"#,
            // Wrong component type and wrong element type.
            r#"{"bufferView":0,"componentType":5123,"count":3,"type":"VEC3"}"#,
            r#"{"bufferView":0,"componentType":5126,"count":9,"type":"SCALAR"}"#,
            // Missing view, zero count, unknown component type.
            r#"{"componentType":5126,"count":3,"type":"VEC3"}"#,
            r#"{"bufferView":0,"componentType":5126,"count":0,"type":"VEC3"}"#,
            r#"{"bufferView":0,"componentType":1,"count":3,"type":"VEC3"}"#,
            // Dangling view index.
            r#"{"bufferView":9,"componentType":5126,"count":3,"type":"VEC3"}"#,
        ];
        for case in cases {
            let json = triangle_json("", case);
            let result = load_glb(&glb(&json, Some(&bin)));
            assert!(
                matches!(result, Err(GlbError::BadAccessor | GlbError::Mesh(_))),
                "{case}: {result:?}"
            );
        }
        let sparse = r#"{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3",
            "sparse":{"count":1,"indices":{"bufferView":2,"componentType":5123},
                      "values":{"bufferView":1}}}"#;
        assert!(matches!(
            load_glb(&glb(&triangle_json("", sparse), Some(&bin))),
            Err(GlbError::Unsupported(_))
        ));
        let stride = triangle_json("", POSITION_ACCESSOR).replace(
            r#""byteOffset":0,"byteLength":36"#,
            r#""byteOffset":0,"byteLength":36,"byteStride":4"#,
        );
        assert!(matches!(
            load_glb(&glb(&stride, Some(&bin))),
            Err(GlbError::BadAccessor)
        ));
        let mut bad_index = bin.clone();
        bad_index[76] = 3;
        assert!(matches!(
            load_glb(&glb(
                &triangle_json("", POSITION_ACCESSOR),
                Some(&bad_index)
            )),
            Err(GlbError::Mesh(MeshError::IndexOutOfRange))
        ));
        let mut nan = bin;
        nan[0..4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(matches!(
            load_glb(&glb(&triangle_json("", POSITION_ACCESSOR), Some(&nan))),
            Err(GlbError::Mesh(MeshError::NonFinite))
        ));
    }

    #[test]
    fn rejects_bad_node_graphs_and_required_extensions() {
        let bin = triangle_bin();
        let cycle = triangle_json(r#","children":[0]"#, POSITION_ACCESSOR);
        assert!(matches!(
            load_glb(&glb(&cycle, Some(&bin))),
            Err(GlbError::BadNodeGraph)
        ));
        let dangling = triangle_json(r#","children":[7]"#, POSITION_ACCESSOR);
        assert!(matches!(
            load_glb(&glb(&dangling, Some(&bin))),
            Err(GlbError::BadNodeGraph)
        ));
        let bad_scene =
            triangle_json("", POSITION_ACCESSOR).replace(r#""scene":0"#, r#""scene":5"#);
        assert!(matches!(
            load_glb(&glb(&bad_scene, Some(&bin))),
            Err(GlbError::BadNodeGraph)
        ));
        let required = triangle_json("", POSITION_ACCESSOR).replace(
            r#""scene":0"#,
            r#""scene":0,"extensionsRequired":["KHR_draco_mesh_compression"]"#,
        );
        assert!(matches!(
            load_glb(&glb(&required, Some(&bin))),
            Err(GlbError::Unsupported(_))
        ));
        // A zero-scale node draws nothing, which leaves an empty mesh.
        let collapsed = triangle_json(r#","scale":[0,1,1]"#, POSITION_ACCESSOR);
        assert!(matches!(
            load_glb(&glb(&collapsed, Some(&bin))),
            Err(GlbError::Mesh(MeshError::Empty))
        ));
    }

    #[test]
    fn shipped_sample_model_loads() {
        let bytes = include_bytes!("../../../content/models/sentinel.glb");
        let mesh = load_glb(bytes).expect("shipped sample model");
        assert!(mesh.triangle_count() > 100, "{}", mesh.triangle_count());
        let (min, max) = mesh.bounds();
        assert!(min[1].abs() < 1e-4 && max[1] > 3.0, "{min:?} {max:?}");
    }

    /// Deterministic stand-in for the cargo-fuzz target: every truncation and a sweep of
    /// single-byte corruptions of the shipped models must return, not panic.
    #[test]
    fn truncations_and_byte_flips_never_panic() {
        // (model, byte-flip step): the skinned sweep is sparser to keep the test quick.
        let models: [(&[u8], usize); 2] = [
            (include_bytes!("../../../content/models/sentinel.glb"), 11),
            (SKINNED, 29),
        ];
        let mut pose = [[[0.0; 4]; 3]; MAX_JOINTS];
        for (bytes, step) in models {
            for length in 0..bytes.len().min(4_096) {
                let _ = load_glb(&bytes[..length]);
            }
            let mut corrupt = bytes.to_vec();
            for position in (0..corrupt.len()).step_by(step) {
                for value in [0x00, 0xFF, b'9', b'"'] {
                    let original = corrupt[position];
                    corrupt[position] = value;
                    // Whatever loads must also pose without panicking.
                    if let Ok(mesh) = load_glb(&corrupt)
                        && let Some(skin) = mesh.skin()
                    {
                        assert!(skin.skeleton().sample(0.37, 1.0, &mut pose));
                    }
                    corrupt[position] = original;
                }
            }
        }
    }

    const SKINNED: &[u8] = include_bytes!("../../../content/models/sentinel-skinned.glb");

    /// The skinned sample's JSON (as text) and BIN chunk, for targeted mutations.
    fn skinned_parts() -> (String, Vec<u8>) {
        let (json, bin) = split_chunks(SKINNED).expect("sample container");
        (
            String::from_utf8(json.to_vec()).expect("utf-8 JSON"),
            bin.expect("BIN chunk").to_vec(),
        )
    }

    #[test]
    fn shipped_skinned_model_loads_with_skin_and_clip() {
        let mesh = load_glb(SKINNED).expect("skinned sample");
        let skin = mesh.skin().expect("skin");
        assert_eq!(skin.joints().len(), mesh.vertex_count());
        assert_eq!(skin.skeleton().joint_count(), 7);
        assert_eq!(skin.skeleton().parents()[1], Some(0));
        assert_eq!(skin.skeleton().clip().expect("clip").tracks().len(), 6);
        // Bind-space vertices: the same geometry as the static sample.
        let (min, max) = mesh.bounds();
        assert!(min[1].abs() < 1e-4 && max[1] > 3.7, "{min:?} {max:?}");
    }

    #[test]
    fn skinned_variants_and_rejections() {
        let (json, bin) = skinned_parts();
        let load = |json: &str| load_glb(&glb(json, Some(&bin)));
        let skin_error = |result: Result<MeshData, GlbError>, expected: SkinError| matches!(result, Err(GlbError::Mesh(MeshError::Skin(error))) if error == expected);
        // Fewer joints than the vertices reference.
        let short = json.replace(r#""joints":[1,2,3,4,5,6,7]"#, r#""joints":[1,2,3]"#);
        assert!(skin_error(load(&short), SkinError::JointOutOfRange));
        // One inverse bind matrix per joint.
        let ibm_count = json.replace(r#""count":7,"type":"MAT4""#, r#""count":6,"type":"MAT4""#);
        assert!(matches!(load(&ibm_count), Err(GlbError::BadAccessor)));
        // Duplicate joint nodes.
        let duplicate = json.replace(r#""joints":[1,2,3,4,5,6,7]"#, r#""joints":[1,1,3,4,5,6,7]"#);
        assert!(matches!(load(&duplicate), Err(GlbError::BadNodeGraph)));
        // More than 64 joints.
        let many = vec!["1"; MAX_JOINTS + 1].join(",");
        let many = json.replace(
            r#""joints":[1,2,3,4,5,6,7]"#,
            &format!(r#""joints":[{many}]"#),
        );
        assert!(skin_error(load(&many), SkinError::JointCount));
        // Missing weights on a skinned primitive.
        let no_weights = json.replace(r#","WEIGHTS_0":3"#, "");
        assert!(matches!(load(&no_weights), Err(GlbError::Unsupported(_))));
        // A static mesh node next to the skinned one.
        let mixed = json
            .replace(r#""nodes":[0,1]"#, r#""nodes":[0,1,8]"#)
            .replace(
                r#"{"name":"arm_r","translation":[0.8,0.8000001,0]}]"#,
                r#"{"name":"arm_r","translation":[0.8,0.8000001,0]},{"mesh":0}]"#,
            );
        assert!(matches!(load(&mixed), Err(GlbError::Unsupported(_))));
        // Inverse bind matrices must be MAT4.
        let bad_ibm = json.replace(r#""inverseBindMatrices":4"#, r#""inverseBindMatrices":5"#);
        assert!(matches!(load(&bad_ibm), Err(GlbError::BadAccessor)));
        // CUBICSPLINE needs three outputs per key.
        let cubic = json.replacen(
            r#""interpolation":"LINEAR""#,
            r#""interpolation":"CUBICSPLINE""#,
            1,
        );
        assert!(matches!(load(&cubic), Err(GlbError::BadAccessor)));
        // STEP is accepted; channels on non-joint nodes are ignored (no clip at all here).
        let step = json.replace(r#""interpolation":"LINEAR""#, r#""interpolation":"STEP""#);
        assert!(load(&step).is_ok());
        let off_skeleton = json.replace(r#""target":{"node":"#, r#""target":{"node":0,"x":"#);
        let mesh = load(&off_skeleton).expect("channels on non-joint nodes are ignored");
        assert!(mesh.skin().expect("skin").skeleton().clip().is_none());
        // A joint parented under a non-joint node that is itself below a joint.
        let detached = json
            .replace(r#""children":[2,4,5]"#, r#""children":[2,4,8]"#)
            .replace(
                r#"{"name":"arm_r","translation":[0.8,0.8000001,0]}]"#,
                r#"{"name":"arm_r","translation":[0.8,0.8000001,0]},{"children":[5]}]"#,
            );
        assert!(matches!(load(&detached), Err(GlbError::Unsupported(_))));
        // A node listed as a child twice.
        let shared = json.replace(r#""children":[3,6,7]"#, r#""children":[3,6,7,4]"#);
        assert!(matches!(load(&shared), Err(GlbError::BadNodeGraph)));
    }

    #[test]
    fn non_joint_ancestors_become_the_skeleton_base() {
        let (json, bin) = skinned_parts();
        // Put the hips under a non-joint "armature" node lifted by 2 m; the rest pose then
        // lifts every joint, which the inverse bind matrices do not undo.
        let lifted = json
            .replace(r#""nodes":[0,1]"#, r#""nodes":[0,8]"#)
            .replace(
                r#"{"name":"arm_r","translation":[0.8,0.8000001,0]}]"#,
                r#"{"name":"arm_r","translation":[0.8,0.8000001,0]},{"translation":[0,2,0],"children":[1]}]"#,
            );
        let mesh = load_glb(&glb(&lifted, Some(&bin))).expect("armature parent");
        let skeleton = mesh.skin().expect("skin").skeleton();
        let mut pose = [[[0.0; 4]; 3]; 7];
        assert!(skeleton.sample(0.0, 0.0, &mut pose));
        for joint in pose {
            assert!((joint[1][3] - 2.0).abs() < 1e-5, "{joint:?}");
        }
    }
}
