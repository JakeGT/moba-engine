//! Skeletons, per-vertex skin weights and one animation clip for GPU skinning (WP-4.1).
//!
//! [`MeshSkin`] is the optional skinning half of a [`MeshData`](crate::MeshData): one
//! `JOINTS_0`/`WEIGHTS_0` pair per vertex plus the [`Skeleton`] those joints index. The
//! skeleton carries its rest pose, inverse bind matrices and at most one [`AnimationClip`].
//!
//! Like `MeshData`, every type here is built through a validating constructor and has private
//! fields, so the renderer and the client can rely on these invariants without re-checking:
//!
//! - 1..=[`MAX_JOINTS`] joints; every parent index names another joint and the hierarchy is
//!   acyclic (an evaluation order with parents first exists and is stored);
//! - rest transforms, base and inverse bind matrices are finite, rotations are normalised;
//! - every per-vertex joint index is below the joint count and the four `u8` weights are
//!   either all zero (the vertex stays in bind space) or sum to exactly 255;
//! - clip tracks target existing joints, have 1..=[`MAX_TRACK_KEYS`] keys with finite,
//!   non-decreasing times and finite values, and the clip has at most [`MAX_CLIP_KEYS`] keys
//!   in total and lasts at most [`MAX_CLIP_SECONDS`].
//!
//! [`Skeleton::sample`] is the per-frame entry point: it writes one [`JointMatrix`] per joint
//! into a caller-provided slice using only stack scratch, so it never allocates.

use moba_core::math::{Mat4, Quat, Vec3, Vec4};

/// Most joints a skeleton may have. Matches the renderer's per-instance bone budget.
pub const MAX_JOINTS: usize = 64;
/// Most keyframes one animation track may have.
pub const MAX_TRACK_KEYS: usize = 4_096;
/// Most keyframes a clip may have across all of its tracks.
pub const MAX_CLIP_KEYS: usize = 65_536;
/// Longest accepted clip, in seconds.
pub const MAX_CLIP_SECONDS: f32 = 600.0;

/// One joint's skinning matrix: the first three rows of a row-major affine 4×4 matrix.
///
/// This is the GPU layout (three `vec4` rows per joint, std430-friendly; no `mat3` padding
/// rules). Row `r` holds `(m[0][r], m[1][r], m[2][r], m[3][r])` for a column-major `m`.
pub type JointMatrix = [[f32; 4]; 3];

/// Why a skin, skeleton or clip was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkinError {
    /// No joints, or more than [`MAX_JOINTS`].
    JointCount,
    /// Per-joint arrays have different lengths, or per-vertex streams do not match the mesh.
    LengthMismatch,
    /// A parent index is out of range, or the hierarchy has a cycle.
    BadHierarchy,
    /// A per-vertex joint index is not below the joint count.
    JointOutOfRange,
    /// Weights neither all zero nor summing to 255.
    BadWeights,
    /// A transform, time or key value is NaN or infinite, or a rotation is degenerate.
    NonFinite,
    /// A track is empty, too long, targets a missing joint or has decreasing times.
    BadTrack,
    /// The clip has too many keys in total or lasts too long.
    ClipTooLarge,
}

impl core::fmt::Display for SkinError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let text = match self {
            Self::JointCount => "skin has no joints or more than 64",
            Self::LengthMismatch => "skin arrays have inconsistent lengths",
            Self::BadHierarchy => "joint hierarchy is out of range or cyclic",
            Self::JointOutOfRange => "vertex joint index out of range",
            Self::BadWeights => "vertex weights must be all zero or sum to 255",
            Self::NonFinite => "non-finite or degenerate skin data",
            Self::BadTrack => "invalid animation track",
            Self::ClipTooLarge => "animation clip exceeds its key or duration caps",
        };
        f.write_str(text)
    }
}

impl std::error::Error for SkinError {}

/// A joint's local transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointTransform {
    /// Translation relative to the parent joint.
    pub translation: Vec3,
    /// Rotation relative to the parent joint (unit quaternion).
    pub rotation: Quat,
    /// Non-uniform scale.
    pub scale: Vec3,
}

impl JointTransform {
    /// The identity transform.
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    fn is_finite(&self) -> bool {
        self.translation.is_finite() && self.rotation.is_finite() && self.scale.is_finite()
    }

    fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

/// Which part of a joint transform a track animates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TrackPath {
    /// Translation (`xyz` of each key).
    Translation,
    /// Rotation (`xyzw` quaternion of each key).
    Rotation,
    /// Scale (`xyz` of each key).
    Scale,
}

/// Keyframe interpolation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Interpolation {
    /// Hold each key until the next.
    Step,
    /// Linear for translation/scale, spherical-linear for rotation.
    Linear,
}

/// One animated property of one joint.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    joint: u8,
    path: TrackPath,
    interpolation: Interpolation,
    times: Vec<f32>,
    values: Vec<[f32; 4]>,
}

impl Track {
    /// Builds a track. `values` are `xyz_` for translation/scale and `xyzw` for rotation.
    ///
    /// # Errors
    /// [`SkinError::BadTrack`] for an empty, overlong or mismatched track or decreasing
    /// times, [`SkinError::NonFinite`] for non-finite data or a zero rotation key.
    pub fn new(
        joint: u8,
        path: TrackPath,
        interpolation: Interpolation,
        times: Vec<f32>,
        mut values: Vec<[f32; 4]>,
    ) -> Result<Self, SkinError> {
        if times.is_empty() || times.len() > MAX_TRACK_KEYS || times.len() != values.len() {
            return Err(SkinError::BadTrack);
        }
        if times.iter().any(|time| !time.is_finite() || *time < 0.0)
            || values.iter().flatten().any(|value| !value.is_finite())
        {
            return Err(SkinError::NonFinite);
        }
        if times.windows(2).any(|pair| pair[1] < pair[0]) {
            return Err(SkinError::BadTrack);
        }
        if path == TrackPath::Rotation {
            for value in &mut values {
                let rotation = Quat::from_array(*value);
                if rotation.length_squared() < 1e-12 {
                    return Err(SkinError::NonFinite);
                }
                *value = rotation.normalize().to_array();
            }
        }
        Ok(Self {
            joint,
            path,
            interpolation,
            times,
            values,
        })
    }

    /// The joint this track animates.
    #[must_use]
    pub fn joint(&self) -> usize {
        usize::from(self.joint)
    }

    /// Time of the last key.
    #[must_use]
    pub fn end(&self) -> f32 {
        self.times.last().copied().unwrap_or(0.0)
    }

    /// Number of keys.
    #[must_use]
    pub fn len(&self) -> usize {
        self.times.len()
    }

    /// `false`: tracks always have at least one key.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }

    /// Samples the track at `time`, clamping outside the key range. Never allocates.
    fn sample(&self, time: f32) -> Vec4 {
        let next = self.times.partition_point(|&key| key <= time);
        let last = self.values.len() - 1;
        if next == 0 {
            return Vec4::from_array(self.values[0]);
        }
        if next > last {
            return Vec4::from_array(self.values[last]);
        }
        let previous = next - 1;
        let from = Vec4::from_array(self.values[previous]);
        if self.interpolation == Interpolation::Step {
            return from;
        }
        let to = Vec4::from_array(self.values[next]);
        let span = self.times[next] - self.times[previous];
        let fraction = if span > 0.0 {
            ((time - self.times[previous]) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        match self.path {
            TrackPath::Rotation => Vec4::from(
                Quat::from_vec4(from)
                    .slerp(Quat::from_vec4(to), fraction)
                    .normalize(),
            ),
            TrackPath::Translation | TrackPath::Scale => from.lerp(to, fraction),
        }
    }
}

/// One looping animation clip.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationClip {
    duration: f32,
    tracks: Vec<Track>,
}

impl AnimationClip {
    /// Builds a clip from its tracks; the duration is the latest key time.
    ///
    /// # Errors
    /// [`SkinError::ClipTooLarge`] past [`MAX_CLIP_KEYS`] or [`MAX_CLIP_SECONDS`];
    /// [`SkinError::BadTrack`] for a clip with no tracks.
    pub fn new(tracks: Vec<Track>) -> Result<Self, SkinError> {
        if tracks.is_empty() {
            return Err(SkinError::BadTrack);
        }
        let keys = tracks.iter().map(Track::len).sum::<usize>();
        let duration = tracks.iter().map(Track::end).fold(0.0f32, f32::max);
        if keys > MAX_CLIP_KEYS || duration > MAX_CLIP_SECONDS {
            return Err(SkinError::ClipTooLarge);
        }
        Ok(Self { duration, tracks })
    }

    /// Length in seconds (the latest key time; `0` for a single-pose clip).
    #[must_use]
    pub fn duration(&self) -> f32 {
        self.duration
    }

    /// The clip's tracks.
    #[must_use]
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// Maps any time (including negative or non-finite) into `[0, duration)`.
    #[must_use]
    pub fn wrap(&self, time: f32) -> f32 {
        if !time.is_finite() || self.duration <= 0.0 {
            return 0.0;
        }
        time.rem_euclid(self.duration)
    }
}

/// A joint hierarchy with its rest pose, inverse bind matrices and optional clip.
#[derive(Clone, Debug, PartialEq)]
pub struct Skeleton {
    parents: Vec<Option<u8>>,
    order: Vec<u8>,
    rest: Vec<JointTransform>,
    base: Vec<Mat4>,
    inverse_bind: Vec<Mat4>,
    clip: Option<AnimationClip>,
}

impl Skeleton {
    /// Builds a skeleton.
    ///
    /// - `parents[j]` is the parent joint of joint `j`, or `None` for a root;
    /// - `rest[j]` is the joint's local transform when no clip drives it;
    /// - `base[j]` is the static transform above a root joint (the non-joint ancestors in
    ///   the source file); it is ignored for joints with a parent;
    /// - `inverse_bind[j]` maps mesh (bind) space into joint `j`'s space.
    ///
    /// # Errors
    /// The first violated invariant from the module docs.
    pub fn new(
        parents: Vec<Option<u8>>,
        mut rest: Vec<JointTransform>,
        base: Vec<Mat4>,
        inverse_bind: Vec<Mat4>,
        clip: Option<AnimationClip>,
    ) -> Result<Self, SkinError> {
        let count = parents.len();
        if count == 0 || count > MAX_JOINTS {
            return Err(SkinError::JointCount);
        }
        if rest.len() != count || base.len() != count || inverse_bind.len() != count {
            return Err(SkinError::LengthMismatch);
        }
        if parents.iter().enumerate().any(|(joint, parent)| {
            parent.is_some_and(|p| usize::from(p) >= count || usize::from(p) == joint)
        }) {
            return Err(SkinError::BadHierarchy);
        }
        if rest.iter().any(|joint| !joint.is_finite())
            || base.iter().chain(&inverse_bind).any(|m| !m.is_finite())
        {
            return Err(SkinError::NonFinite);
        }
        for joint in &mut rest {
            if joint.rotation.length_squared() < 1e-12 {
                return Err(SkinError::NonFinite);
            }
            joint.rotation = joint.rotation.normalize();
        }
        let order = evaluation_order(&parents).ok_or(SkinError::BadHierarchy)?;
        if let Some(clip) = &clip
            && clip.tracks.iter().any(|track| track.joint() >= count)
        {
            return Err(SkinError::BadTrack);
        }
        Ok(Self {
            parents,
            order,
            rest,
            base,
            inverse_bind,
            clip,
        })
    }

    /// Number of joints (1..=[`MAX_JOINTS`]).
    #[must_use]
    pub fn joint_count(&self) -> usize {
        self.parents.len()
    }

    /// The clip, if the file had one that drives this skeleton.
    #[must_use]
    pub fn clip(&self) -> Option<&AnimationClip> {
        self.clip.as_ref()
    }

    /// Parent of each joint.
    #[must_use]
    pub fn parents(&self) -> &[Option<u8>] {
        &self.parents
    }

    /// Writes one skinning matrix per joint into `out[..joint_count]`.
    ///
    /// The pose blends from the rest pose (`blend = 0`) to the clip sampled at `time`
    /// seconds, looped (`blend = 1`); without a clip it is the rest pose. Each matrix is
    /// `global(joint) * inverse_bind(joint)`, so the bind pose maps every vertex to itself.
    ///
    /// Returns `false` (and writes nothing) if `out` is shorter than the joint count. Uses
    /// fixed stack scratch only: it never allocates.
    pub fn sample(&self, time: f32, blend: f32, out: &mut [JointMatrix]) -> bool {
        let count = self.joint_count();
        if out.len() < count {
            return false;
        }
        let blend = if blend.is_finite() {
            blend.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let mut locals = [JointTransform::IDENTITY; MAX_JOINTS];
        locals[..count].copy_from_slice(&self.rest);
        if let Some(clip) = self.clip.as_ref().filter(|_| blend > 0.0) {
            let time = clip.wrap(time);
            for track in &clip.tracks {
                let joint = track.joint();
                let rest = &self.rest[joint];
                let local = &mut locals[joint];
                let value = track.sample(time);
                match track.path {
                    TrackPath::Translation => {
                        local.translation = rest.translation.lerp(value.truncate(), blend);
                    }
                    TrackPath::Scale => local.scale = rest.scale.lerp(value.truncate(), blend),
                    TrackPath::Rotation => {
                        local.rotation = rest
                            .rotation
                            .slerp(Quat::from_vec4(value), blend)
                            .normalize();
                    }
                }
            }
        }
        let mut globals = [Mat4::IDENTITY; MAX_JOINTS];
        for &joint in &self.order {
            let joint = usize::from(joint);
            let local = locals[joint].matrix();
            globals[joint] = match self.parents[joint] {
                Some(parent) => globals[usize::from(parent)] * local,
                None => self.base[joint] * local,
            };
        }
        for (joint, target) in out[..count].iter_mut().enumerate() {
            let skin = globals[joint] * self.inverse_bind[joint];
            *target = [
                skin.row(0).to_array(),
                skin.row(1).to_array(),
                skin.row(2).to_array(),
            ];
        }
        true
    }
}

/// Parents-first order of the joints, or `None` if the hierarchy has a cycle.
fn evaluation_order(parents: &[Option<u8>]) -> Option<Vec<u8>> {
    let count = parents.len();
    let mut placed = [false; MAX_JOINTS];
    let mut order = Vec::with_capacity(count);
    // Each pass places at least one joint or proves a cycle: at most `count` passes.
    for _ in 0..count {
        for joint in 0..count {
            if placed[joint] {
                continue;
            }
            let ready = parents[joint].is_none_or(|parent| placed[usize::from(parent)]);
            if ready {
                placed[joint] = true;
                order.push(u8::try_from(joint).ok()?);
            }
        }
        if order.len() == count {
            return Some(order);
        }
    }
    None
}

/// Per-vertex skinning streams and the skeleton they index.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshSkin {
    joints: Vec<[u8; 4]>,
    weights: Vec<[u8; 4]>,
    skeleton: Skeleton,
}

impl MeshSkin {
    /// Builds a skin. `weights` are `UNORM8` and must be all zero or sum to 255 per vertex.
    ///
    /// The vertex count is checked against the mesh in
    /// [`MeshData::with_skin`](crate::MeshData::with_skin).
    ///
    /// # Errors
    /// [`SkinError::LengthMismatch`], [`SkinError::JointOutOfRange`] or
    /// [`SkinError::BadWeights`].
    pub fn new(
        joints: Vec<[u8; 4]>,
        weights: Vec<[u8; 4]>,
        skeleton: Skeleton,
    ) -> Result<Self, SkinError> {
        if joints.len() != weights.len() {
            return Err(SkinError::LengthMismatch);
        }
        let count = skeleton.joint_count();
        if joints
            .iter()
            .flatten()
            .any(|&joint| usize::from(joint) >= count)
        {
            return Err(SkinError::JointOutOfRange);
        }
        if weights.iter().any(|lanes| {
            let sum: u32 = lanes.iter().map(|&lane| u32::from(lane)).sum();
            sum != 0 && sum != 255
        }) {
            return Err(SkinError::BadWeights);
        }
        Ok(Self {
            joints,
            weights,
            skeleton,
        })
    }

    /// Four joint indices per vertex (`R8G8B8A8_UINT`).
    #[must_use]
    pub fn joints(&self) -> &[[u8; 4]] {
        &self.joints
    }

    /// Four weights per vertex (`R8G8B8A8_UNORM`).
    #[must_use]
    pub fn weights(&self) -> &[[u8; 4]] {
        &self.weights
    }

    /// The skeleton the joints index.
    #[must_use]
    pub fn skeleton(&self) -> &Skeleton {
        &self.skeleton
    }
}

/// Quantises four non-negative weights to `UNORM8` lanes that sum to exactly 255, or to all
/// zeros when they sum to (almost) nothing. Returns `None` for negative or non-finite input.
#[must_use]
pub fn quantize_weights(weights: [f32; 4]) -> Option<[u8; 4]> {
    if weights
        .iter()
        .any(|weight| !weight.is_finite() || *weight < 0.0)
    {
        return None;
    }
    let sum: f32 = weights.iter().sum();
    if sum <= 1e-6 {
        return Some([0; 4]);
    }
    let mut lanes = [0u8; 4];
    let mut total = 0i32;
    let mut largest = 0;
    for (lane, weight) in weights.iter().enumerate() {
        // In [0, 255] because each weight is in [0, sum].
        let quantized = (weight / sum * 255.0).round().clamp(0.0, 255.0) as u8;
        lanes[lane] = quantized;
        total += i32::from(quantized);
        if weight > &weights[largest] {
            largest = lane;
        }
    }
    // Rounding leaves the sum within ±2 of 255; settle the remainder on the largest lane.
    let fixed = i32::from(lanes[largest]) + (255 - total);
    lanes[largest] = u8::try_from(fixed.clamp(0, 255)).ok()?;
    let settled: i32 = lanes.iter().map(|&lane| i32::from(lane)).sum();
    (settled == 255).then_some(lanes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_bone(clip: Option<AnimationClip>) -> Skeleton {
        Skeleton::new(
            vec![None, Some(0)],
            vec![
                JointTransform::IDENTITY,
                JointTransform {
                    translation: Vec3::Y,
                    ..JointTransform::IDENTITY
                },
            ],
            vec![Mat4::IDENTITY; 2],
            vec![Mat4::IDENTITY, Mat4::from_translation(-Vec3::Y)],
            clip,
        )
        .expect("valid skeleton")
    }

    fn apply(matrix: &JointMatrix, point: Vec3) -> Vec3 {
        let p = point.extend(1.0);
        Vec3::new(
            Vec4::from_array(matrix[0]).dot(p),
            Vec4::from_array(matrix[1]).dot(p),
            Vec4::from_array(matrix[2]).dot(p),
        )
    }

    #[test]
    fn rest_pose_is_the_bind_pose() {
        let skeleton = two_bone(None);
        let mut out = [[[0.0; 4]; 3]; 2];
        assert!(skeleton.sample(0.3, 1.0, &mut out));
        let point = Vec3::new(0.2, 1.5, -0.4);
        for matrix in &out {
            assert!(apply(matrix, point).distance(point) < 1e-6);
        }
        assert!(!skeleton.sample(0.0, 1.0, &mut out[..1]), "short output");
    }

    #[test]
    fn clip_rotates_the_child_and_loops() {
        let quarter = Quat::from_rotation_z(core::f32::consts::FRAC_PI_2);
        let track = Track::new(
            1,
            TrackPath::Rotation,
            Interpolation::Linear,
            vec![0.0, 1.0],
            vec![Quat::IDENTITY.to_array(), quarter.to_array()],
        )
        .expect("track");
        let skeleton = two_bone(Some(AnimationClip::new(vec![track]).expect("clip")));
        assert!((skeleton.clip().expect("clip").duration() - 1.0).abs() < 1e-6);
        let mut out = [[[0.0; 4]; 3]; 2];
        // Just before the loop point the child is (almost) a quarter turn about its origin
        // at y = 1: a point one metre above the joint swings to -x.
        assert!(skeleton.sample(0.999_9, 1.0, &mut out));
        let swung = apply(&out[1], Vec3::new(0.0, 2.0, 0.0));
        assert!(
            swung.distance(Vec3::new(-1.0, 1.0, 0.0)) < 1e-3,
            "{swung:?}"
        );
        // Halfway, blend 0 is the rest pose regardless of time.
        assert!(skeleton.sample(0.5, 0.0, &mut out));
        let rest = apply(&out[1], Vec3::new(0.0, 2.0, 0.0));
        assert!(rest.distance(Vec3::new(0.0, 2.0, 0.0)) < 1e-6);
        // Time wraps (1.5 s == 0.5 s) and hostile times do not panic.
        let mut wrapped = [[[0.0; 4]; 3]; 2];
        assert!(skeleton.sample(1.5, 1.0, &mut wrapped));
        assert!(skeleton.sample(0.5, 1.0, &mut out));
        assert_eq!(wrapped, out);
        for time in [f32::NAN, f32::INFINITY, -3.25, 1e30] {
            assert!(skeleton.sample(time, 1.0, &mut out));
            assert!(
                out.iter()
                    .flatten()
                    .flatten()
                    .all(|value| value.is_finite())
            );
        }
    }

    #[test]
    fn rejects_bad_skeletons_tracks_and_skins() {
        let one = |parents: Vec<Option<u8>>| {
            let count = parents.len();
            Skeleton::new(
                parents,
                vec![JointTransform::IDENTITY; count],
                vec![Mat4::IDENTITY; count],
                vec![Mat4::IDENTITY; count],
                None,
            )
        };
        assert_eq!(one(Vec::new()), Err(SkinError::JointCount));
        assert_eq!(one(vec![None; MAX_JOINTS + 1]), Err(SkinError::JointCount));
        assert_eq!(one(vec![Some(0)]), Err(SkinError::BadHierarchy));
        assert_eq!(one(vec![Some(5)]), Err(SkinError::BadHierarchy));
        assert_eq!(one(vec![Some(1), Some(0)]), Err(SkinError::BadHierarchy));
        // Children listed before parents are fine.
        assert!(one(vec![Some(2), Some(2), None]).is_ok());
        let mut nan = JointTransform::IDENTITY;
        nan.translation.x = f32::NAN;
        assert_eq!(
            Skeleton::new(
                vec![None],
                vec![nan],
                vec![Mat4::IDENTITY],
                vec![Mat4::IDENTITY],
                None
            ),
            Err(SkinError::NonFinite)
        );
        let linear = Interpolation::Linear;
        let path = TrackPath::Translation;
        assert_eq!(
            Track::new(0, path, linear, Vec::new(), Vec::new()),
            Err(SkinError::BadTrack)
        );
        assert_eq!(
            Track::new(0, path, linear, vec![1.0, 0.5], vec![[0.0; 4]; 2]),
            Err(SkinError::BadTrack)
        );
        assert_eq!(
            Track::new(0, path, linear, vec![f32::NAN], vec![[0.0; 4]]),
            Err(SkinError::NonFinite)
        );
        assert_eq!(
            Track::new(0, TrackPath::Rotation, linear, vec![0.0], vec![[0.0; 4]]),
            Err(SkinError::NonFinite)
        );
        let late = Track::new(
            0,
            path,
            linear,
            vec![MAX_CLIP_SECONDS + 1.0],
            vec![[0.0; 4]],
        )
        .expect("track");
        assert_eq!(AnimationClip::new(vec![late]), Err(SkinError::ClipTooLarge));
        let stray = Track::new(9, path, linear, vec![0.0], vec![[0.0; 4]]).expect("track");
        assert_eq!(
            Skeleton::new(
                vec![None],
                vec![JointTransform::IDENTITY],
                vec![Mat4::IDENTITY],
                vec![Mat4::IDENTITY],
                Some(AnimationClip::new(vec![stray]).expect("clip")),
            ),
            Err(SkinError::BadTrack)
        );
        let skeleton = two_bone(None);
        assert_eq!(
            MeshSkin::new(vec![[0; 4]], Vec::new(), skeleton.clone()),
            Err(SkinError::LengthMismatch)
        );
        assert_eq!(
            MeshSkin::new(vec![[2, 0, 0, 0]], vec![[255, 0, 0, 0]], skeleton.clone()),
            Err(SkinError::JointOutOfRange)
        );
        assert_eq!(
            MeshSkin::new(vec![[0; 4]], vec![[200, 0, 0, 0]], skeleton.clone()),
            Err(SkinError::BadWeights)
        );
        assert!(
            MeshSkin::new(
                vec![[0, 1, 0, 0]; 2],
                vec![[0; 4], [128, 127, 0, 0]],
                skeleton
            )
            .is_ok()
        );
    }

    #[test]
    fn weights_quantise_to_exactly_255() {
        assert_eq!(quantize_weights([1.0, 0.0, 0.0, 0.0]), Some([255, 0, 0, 0]));
        assert_eq!(quantize_weights([0.0; 4]), Some([0; 4]));
        assert_eq!(quantize_weights([-1.0, 0.0, 0.0, 0.0]), None);
        assert_eq!(quantize_weights([f32::NAN, 0.0, 0.0, 0.0]), None);
        for weights in [
            [1.0, 1.0, 1.0, 0.0],
            [0.25; 4],
            [0.333, 0.333, 0.334, 0.0],
            [0.7, 0.1, 0.1, 0.1],
            [3.0, 2.0, 1.0, 0.5],
        ] {
            let lanes = quantize_weights(weights).expect("valid");
            let sum: u32 = lanes.iter().map(|&lane| u32::from(lane)).sum();
            assert_eq!(sum, 255, "{weights:?} -> {lanes:?}");
        }
    }
}
