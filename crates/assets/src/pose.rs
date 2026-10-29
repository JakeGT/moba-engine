//! Local poses, bone masks, clip sampling and blending (ADR-0012).
//!
//! A [`Pose`] is one local [`JointTransform`] per joint in a fixed-size inline array, so
//! poses live on the stack or inside preallocated scratch and never allocate. The runtime
//! builds a frame's pose from these steps:
//!
//! - [`Skeleton::sample_clip`] writes a clip at a time (looped or clamped) over the rest pose;
//!   keys interpolate linearly (translation/scale) or by slerp (rotation), or step;
//! - [`Pose::blend`] crossfades towards another pose (lerp, rotation by shortest-path nlerp),
//!   optionally limited to a [`BoneMask`] such as the upper body;
//! - [`Pose::add`] layers an additive pose (its difference from a reference pose, for example
//!   a hit reaction relative to its first frame);
//! - [`BlendSpace1d`] blends clips along one parameter (idle ↔ run by speed), and
//!   [`BlendSpace2d`] over a plane (strafe × forward velocity, R4b);
//! - [`Skeleton::model_transforms`] and [`Skeleton::skinning_matrices`] turn a pose into
//!   model-space joint transforms and the GPU's [`JointMatrix`] rows.

use moba_core::math::{Mat4, Quat, Vec2, Vec3};

use crate::skin::{JointMatrix, JointTransform, MAX_JOINTS, Skeleton, TrackPath};

/// Most points in a [`BlendSpace1d`].
pub const MAX_BLEND_POINTS: usize = 8;

/// One local transform per joint (at most [`MAX_JOINTS`]), stored inline.
#[derive(Clone, Debug, PartialEq)]
pub struct Pose {
    joints: [JointTransform; MAX_JOINTS],
    count: usize,
}

impl Default for Pose {
    fn default() -> Self {
        Self {
            joints: [JointTransform::IDENTITY; MAX_JOINTS],
            count: 0,
        }
    }
}

/// Sanitises a blend weight: non-finite becomes 0, the rest is clamped to `0..=1`.
fn weight01(weight: f32) -> f32 {
    if weight.is_finite() {
        weight.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

impl Pose {
    /// The skeleton's rest pose.
    #[must_use]
    pub fn rest(skeleton: &Skeleton) -> Self {
        let mut pose = Self::default();
        pose.set_rest(skeleton);
        pose
    }

    /// Overwrites this pose with the skeleton's rest pose. Never allocates.
    pub fn set_rest(&mut self, skeleton: &Skeleton) {
        let rest = skeleton.rest();
        self.count = rest.len();
        self.joints[..rest.len()].copy_from_slice(rest);
    }

    /// Number of joints in the pose.
    #[must_use]
    pub fn joint_count(&self) -> usize {
        self.count
    }

    /// The local transforms.
    #[must_use]
    pub fn joints(&self) -> &[JointTransform] {
        &self.joints[..self.count]
    }

    /// The local transforms, for procedural edits.
    pub fn joints_mut(&mut self) -> &mut [JointTransform] {
        &mut self.joints[..self.count]
    }

    /// Blends towards `other` by `weight` (0 keeps this pose, 1 copies `other`) on the
    /// joints in `mask`. Translation and scale lerp; rotation uses shortest-path normalised
    /// lerp. Non-finite weights count as 0. Never allocates.
    pub fn blend(&mut self, other: &Self, weight: f32, mask: BoneMask) {
        let weight = weight01(weight);
        if weight <= 0.0 {
            return;
        }
        let count = self.count.min(other.count);
        for (joint, (target, source)) in self.joints[..count]
            .iter_mut()
            .zip(&other.joints[..count])
            .enumerate()
        {
            if !mask.contains(joint) {
                continue;
            }
            if weight >= 1.0 {
                *target = *source;
                continue;
            }
            target.translation = target.translation.lerp(source.translation, weight);
            target.scale = target.scale.lerp(source.scale, weight);
            // `Quat::lerp` flips to the shortest path and renormalises (nlerp).
            target.rotation = target.rotation.lerp(source.rotation, weight);
        }
    }

    /// Adds `weight` of the difference between `additive` and `reference` on the joints in
    /// `mask`: rotations compose (`rotation * slerp(identity, reference⁻¹ * additive)`),
    /// translations add and scales multiply. Never allocates.
    pub fn add(&mut self, additive: &Self, reference: &Self, weight: f32, mask: BoneMask) {
        let weight = weight01(weight);
        if weight <= 0.0 {
            return;
        }
        let count = self.count.min(additive.count).min(reference.count);
        for joint in 0..count {
            if !mask.contains(joint) {
                continue;
            }
            let (add, base) = (&additive.joints[joint], &reference.joints[joint]);
            let target = &mut self.joints[joint];
            let delta = (base.rotation.inverse() * add.rotation).normalize();
            target.rotation = (target.rotation * Quat::IDENTITY.lerp(delta, weight)).normalize();
            target.translation += (add.translation - base.translation) * weight;
            let ratio = |add: f32, base: f32| {
                if base.abs() > 1e-6 { add / base } else { 1.0 }
            };
            let scale = Vec3::new(
                ratio(add.scale.x, base.scale.x),
                ratio(add.scale.y, base.scale.y),
                ratio(add.scale.z, base.scale.z),
            );
            target.scale *= Vec3::ONE.lerp(scale, weight);
        }
    }
}

/// A set of joints (one bit per joint; [`MAX_JOINTS`] is 64).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BoneMask(u64);

impl BoneMask {
    /// Every joint.
    pub const ALL: Self = Self(u64::MAX);
    /// No joint.
    pub const NONE: Self = Self(0);

    /// The mask with these bits (bit `j` is joint `j`).
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// The raw bits.
    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// `root` and every joint below it.
    #[must_use]
    pub fn subtree(skeleton: &Skeleton, root: usize) -> Self {
        let mut bits = 0u64;
        if root >= skeleton.joint_count() {
            return Self(0);
        }
        // Parents come first, so one pass sees each parent's membership before its children.
        for &joint in skeleton.evaluation_order() {
            let joint = usize::from(joint);
            let inside = joint == root
                || skeleton.parents()[joint].is_some_and(|parent| bits & (1 << parent) != 0);
            if inside {
                bits |= 1 << joint;
            }
        }
        Self(bits)
    }

    /// Whether joint `joint` is in the mask.
    #[must_use]
    pub const fn contains(self, joint: usize) -> bool {
        joint < 64 && self.0 & (1 << joint) != 0
    }

    /// Joints in either mask.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Joints not in this mask.
    #[must_use]
    pub const fn complement(self) -> Self {
        Self(!self.0)
    }

    /// Whether the mask is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// How a clip time outside `[0, duration]` is mapped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClipWrap {
    /// Loop (`time mod duration`).
    Loop,
    /// Clamp to the first or last key (one-shot clips).
    Clamp,
}

/// The three affine rows of `matrix`, the GPU joint layout.
#[must_use]
pub fn joint_matrix(matrix: Mat4) -> JointMatrix {
    [
        matrix.row(0).to_array(),
        matrix.row(1).to_array(),
        matrix.row(2).to_array(),
    ]
}

impl Skeleton {
    /// Writes clip `clip` at `time` seconds over the rest pose into `out`. Joints the clip
    /// does not animate keep their rest transform.
    ///
    /// Returns `false` (and writes the rest pose) for a missing clip. Never allocates.
    pub fn sample_clip(&self, clip: usize, time: f32, wrap: ClipWrap, out: &mut Pose) -> bool {
        out.set_rest(self);
        let Some(clip) = self.clips().get(clip) else {
            return false;
        };
        let time = match wrap {
            ClipWrap::Loop => clip.wrap(time),
            ClipWrap::Clamp if time.is_finite() => time.clamp(0.0, clip.duration()),
            ClipWrap::Clamp => 0.0,
        };
        for track in clip.tracks() {
            let value = track.sample(time);
            let local = &mut out.joints[track.joint()];
            match track.path() {
                TrackPath::Translation => local.translation = value.truncate(),
                TrackPath::Scale => local.scale = value.truncate(),
                TrackPath::Rotation => local.rotation = Quat::from_vec4(value).normalize(),
            }
        }
        true
    }

    /// Writes each joint's model-space transform for `pose` into `out[..joint_count]`.
    ///
    /// Returns `false` (and writes nothing) if `out` or the pose is too short. Never
    /// allocates.
    pub fn model_transforms(&self, pose: &Pose, out: &mut [Mat4]) -> bool {
        let count = self.joint_count();
        if out.len() < count || pose.count < count {
            return false;
        }
        for &joint in self.evaluation_order() {
            let joint = usize::from(joint);
            let local = pose.joints[joint].matrix();
            out[joint] = match self.parents()[joint] {
                Some(parent) => out[usize::from(parent)] * local,
                None => self.base(joint) * local,
            };
        }
        true
    }

    /// Writes one skinning matrix (`model_transform * inverse_bind`) per joint into
    /// `out[..joint_count]` from model-space transforms such as
    /// [`Skeleton::model_transforms`] wrote.
    ///
    /// Returns `false` (and writes nothing) if either slice is too short. Never allocates.
    pub fn skinning_matrices(&self, model: &[Mat4], out: &mut [JointMatrix]) -> bool {
        let count = self.joint_count();
        if out.len() < count || model.len() < count {
            return false;
        }
        for (joint, target) in out[..count].iter_mut().enumerate() {
            *target = joint_matrix(model[joint] * self.inverse_bind(joint));
        }
        true
    }
}

/// Why a blend space was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BlendSpaceError {
    /// No points, or more than [`MAX_BLEND_POINTS`].
    PointCount,
    /// Parameters are not finite and strictly increasing.
    Unordered,
    /// A clip index does not fit `u8`.
    ClipIndex,
    /// Two points of a [`BlendSpace2d`] coincide, or a coordinate is not finite.
    Coincident,
}

impl core::fmt::Display for BlendSpaceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::PointCount => "blend spaces need 1 to 8 points",
            Self::Unordered => "blend space parameters must be finite and strictly increasing",
            Self::ClipIndex => "blend space clip index out of range",
            Self::Coincident => "2D blend space points must be finite and distinct",
        })
    }
}

impl std::error::Error for BlendSpaceError {}

/// Clips placed along one parameter (for locomotion: idle at speed 0, run at run speed).
///
/// Sampling finds the two points around the parameter (clamping at the ends), samples both
/// clips looped at the same time and blends them by the parameter's position between them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlendSpace1d {
    params: [f32; MAX_BLEND_POINTS],
    clips: [u8; MAX_BLEND_POINTS],
    len: usize,
}

impl BlendSpace1d {
    /// Builds a blend space from `(parameter, clip index)` points.
    ///
    /// # Errors
    /// A [`BlendSpaceError`] for an empty, overlong or unordered point list.
    pub fn new(points: &[(f32, usize)]) -> Result<Self, BlendSpaceError> {
        if points.is_empty() || points.len() > MAX_BLEND_POINTS {
            return Err(BlendSpaceError::PointCount);
        }
        let mut space = Self {
            params: [0.0; MAX_BLEND_POINTS],
            clips: [0; MAX_BLEND_POINTS],
            len: points.len(),
        };
        for (index, &(param, clip)) in points.iter().enumerate() {
            if !param.is_finite() || (index > 0 && param <= space.params[index - 1]) {
                return Err(BlendSpaceError::Unordered);
            }
            space.params[index] = param;
            space.clips[index] = u8::try_from(clip).map_err(|_| BlendSpaceError::ClipIndex)?;
        }
        Ok(space)
    }

    /// The two clips and the weight of the second for `param`.
    #[must_use]
    pub fn weights(&self, param: f32) -> (usize, usize, f32) {
        let first = usize::from(self.clips[0]);
        let param = if param.is_finite() {
            param
        } else {
            self.params[0]
        };
        if self.len == 1 || param <= self.params[0] {
            return (first, first, 0.0);
        }
        for index in 1..self.len {
            if param < self.params[index] {
                let (low, high) = (self.params[index - 1], self.params[index]);
                return (
                    usize::from(self.clips[index - 1]),
                    usize::from(self.clips[index]),
                    (param - low) / (high - low),
                );
            }
        }
        let last = usize::from(self.clips[self.len - 1]);
        (last, last, 0.0)
    }

    /// Samples the blend at `param` and `time` seconds into `out`, using `scratch` for the
    /// second clip. Never allocates.
    pub fn sample(
        &self,
        skeleton: &Skeleton,
        param: f32,
        time: f32,
        out: &mut Pose,
        scratch: &mut Pose,
    ) {
        let (low, high, weight) = self.weights(param);
        skeleton.sample_clip(low, time, ClipWrap::Loop, out);
        if weight > 0.0 && high != low {
            skeleton.sample_clip(high, time, ClipWrap::Loop, scratch);
            out.blend(scratch, weight, BoneMask::ALL);
        }
    }
}

/// Clips placed on a plane (R4b): for locomotion, `x` is the strafe and `y` the forward
/// velocity in m/s in the unit's facing frame (idle at the origin, run forward at `(0, run)`).
///
/// Weights come from gradient-band interpolation (Johansen 2009): point `i`'s weight is the
/// minimum over the other points `j` of `1 - ((p - p_i) · (p_j - p_i)) / |p_j - p_i|²`,
/// clamped to `0..=1`, then all are normalised. It needs no triangulation, is exact at every
/// sample point and continuous between them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlendSpace2d {
    points: [Vec2; MAX_BLEND_POINTS],
    clips: [u8; MAX_BLEND_POINTS],
    len: usize,
}

impl BlendSpace2d {
    /// Builds a blend space from `([x, y], clip index)` points.
    ///
    /// # Errors
    /// [`BlendSpaceError::PointCount`], [`BlendSpaceError::Coincident`] for non-finite or
    /// repeated points, [`BlendSpaceError::ClipIndex`].
    pub fn new(points: &[([f32; 2], usize)]) -> Result<Self, BlendSpaceError> {
        if points.is_empty() || points.len() > MAX_BLEND_POINTS {
            return Err(BlendSpaceError::PointCount);
        }
        let mut space = Self {
            points: [Vec2::ZERO; MAX_BLEND_POINTS],
            clips: [0; MAX_BLEND_POINTS],
            len: points.len(),
        };
        for (index, &(point, clip)) in points.iter().enumerate() {
            let point = Vec2::from_array(point);
            if !point.is_finite()
                || space.points[..index]
                    .iter()
                    .any(|other| other.distance_squared(point) < 1e-6)
            {
                return Err(BlendSpaceError::Coincident);
            }
            space.points[index] = point;
            space.clips[index] = u8::try_from(clip).map_err(|_| BlendSpaceError::ClipIndex)?;
        }
        Ok(space)
    }

    /// Number of points.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the space has no points (never true for a built space).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The clip of point `index`.
    #[must_use]
    pub fn clip(&self, index: usize) -> Option<usize> {
        (index < self.len).then(|| usize::from(self.clips[index]))
    }

    /// Normalised weights of every point at `param` (unused entries are 0).
    #[must_use]
    pub fn weights(&self, param: [f32; 2]) -> [f32; MAX_BLEND_POINTS] {
        let mut weights = [0.0; MAX_BLEND_POINTS];
        let param = Vec2::from_array(param);
        let param = if param.is_finite() { param } else { Vec2::ZERO };
        if self.len == 1 {
            weights[0] = 1.0;
            return weights;
        }
        let points = &self.points[..self.len];
        let mut total = 0.0;
        for (i, &point) in points.iter().enumerate() {
            let mut weight = 1.0f32;
            for (j, &other) in points.iter().enumerate() {
                if i == j {
                    continue;
                }
                let edge = other - point;
                let band = 1.0 - (param - point).dot(edge) / edge.length_squared();
                weight = weight.min(band);
            }
            let weight = weight.clamp(0.0, 1.0);
            weights[i] = weight;
            total += weight;
        }
        if total > 1e-6 {
            for weight in &mut weights[..self.len] {
                *weight /= total;
            }
        } else {
            // Unreachable for distinct points (the nearest point always has weight > 0);
            // fall back to the nearest one.
            let nearest = (0..self.len)
                .min_by(|&a, &b| {
                    points[a]
                        .distance_squared(param)
                        .total_cmp(&points[b].distance_squared(param))
                })
                .unwrap_or(0);
            weights[nearest] = 1.0;
        }
        weights
    }

    /// The two heaviest points' clips and the share of the second between them, for
    /// reports: `(first clip, second clip, weight of second)`.
    #[must_use]
    pub fn dominant(&self, param: [f32; 2]) -> (usize, usize, f32) {
        let weights = self.weights(param);
        let mut order: [usize; MAX_BLEND_POINTS] = core::array::from_fn(|index| index);
        let order = &mut order[..self.len];
        order.sort_by(|&a, &b| weights[b].total_cmp(&weights[a]).then(a.cmp(&b)));
        let first = order[0];
        let second = order.get(1).copied().unwrap_or(first);
        let pair = weights[first] + weights[second];
        let share = if pair > 0.0 && second != first {
            weights[second] / pair
        } else {
            0.0
        };
        (
            usize::from(self.clips[first]),
            usize::from(self.clips[second]),
            share,
        )
    }

    /// Samples the blend at `param` and `time` seconds (all clips looped at the same time)
    /// into `out`, using `scratch` for the other clips. Never allocates.
    pub fn sample(
        &self,
        skeleton: &Skeleton,
        param: [f32; 2],
        time: f32,
        out: &mut Pose,
        scratch: &mut Pose,
    ) {
        let weights = self.weights(param);
        let mut accumulated = 0.0f32;
        for (index, &weight) in weights[..self.len].iter().enumerate() {
            if weight <= 1e-4 {
                continue;
            }
            let clip = usize::from(self.clips[index]);
            if accumulated == 0.0 {
                skeleton.sample_clip(clip, time, ClipWrap::Loop, out);
            } else {
                skeleton.sample_clip(clip, time, ClipWrap::Loop, scratch);
                out.blend(scratch, weight / (accumulated + weight), BoneMask::ALL);
            }
            accumulated += weight;
        }
        if accumulated == 0.0 {
            out.set_rest(skeleton);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skin::{AnimationClip, Interpolation, Track};

    /// Root → spine → arm, plus a leg under the root; clip 0 turns the spine, clip 1 lifts
    /// the root, clip 2 steps the arm.
    fn rig() -> Skeleton {
        let quarter = Quat::from_rotation_z(core::f32::consts::FRAC_PI_2);
        let spin = Track::new(
            1,
            TrackPath::Rotation,
            Interpolation::Linear,
            vec![0.0, 1.0],
            vec![Quat::IDENTITY.to_array(), quarter.to_array()],
        )
        .expect("track");
        let lift = Track::new(
            0,
            TrackPath::Translation,
            Interpolation::Linear,
            vec![0.0, 2.0],
            vec![[0.0, 0.0, 0.0, 0.0], [0.0, 2.0, 0.0, 0.0]],
        )
        .expect("track");
        let step = Track::new(
            2,
            TrackPath::Scale,
            Interpolation::Step,
            vec![0.0, 0.5],
            vec![[1.0, 1.0, 1.0, 0.0], [2.0, 2.0, 2.0, 0.0]],
        )
        .expect("track");
        let clips = vec![
            AnimationClip::new(vec![spin])
                .expect("clip")
                .with_name("spin")
                .expect("name"),
            AnimationClip::new(vec![lift])
                .expect("clip")
                .with_name("lift")
                .expect("name"),
            AnimationClip::new(vec![step]).expect("clip"),
        ];
        let at = |y: f32| JointTransform {
            translation: Vec3::Y * y,
            ..JointTransform::IDENTITY
        };
        Skeleton::new(
            vec![None, Some(0), Some(1), Some(0)],
            vec![at(0.0), at(1.0), at(1.0), at(-1.0)],
            vec![Mat4::IDENTITY; 4],
            vec![
                Mat4::IDENTITY,
                Mat4::from_translation(-Vec3::Y),
                Mat4::from_translation(-Vec3::Y * 2.0),
                Mat4::from_translation(Vec3::Y),
            ],
            None,
        )
        .and_then(|skeleton| skeleton.with_clips(clips))
        .and_then(|skeleton| {
            skeleton.with_joint_names(vec![
                Some("root".into()),
                Some("spine".into()),
                Some("arm".into()),
                None,
            ])
        })
        .expect("rig")
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn clips_sample_by_name_with_loop_clamp_and_step() {
        let rig = rig();
        assert_eq!(rig.clip_index("LIFT"), Some(1));
        assert_eq!(rig.clip_index("missing"), None);
        assert_eq!(rig.joint_index("Arm"), Some(2));
        assert_eq!(rig.joint_name(3), None);
        let mut pose = Pose::default();
        assert!(rig.sample_clip(1, 1.0, ClipWrap::Loop, &mut pose));
        assert!(close(pose.joints()[0].translation.y, 1.0));
        // Looping wraps 3 s to 1 s; clamping holds the last key.
        assert!(rig.sample_clip(1, 3.0, ClipWrap::Loop, &mut pose));
        assert!(close(pose.joints()[0].translation.y, 1.0));
        assert!(rig.sample_clip(1, 3.0, ClipWrap::Clamp, &mut pose));
        assert!(close(pose.joints()[0].translation.y, 2.0));
        // Untouched joints keep the rest pose.
        assert!(close(pose.joints()[1].translation.y, 1.0));
        // Step interpolation holds the first key until the second.
        assert!(rig.sample_clip(2, 0.49, ClipWrap::Clamp, &mut pose));
        assert!(close(pose.joints()[2].scale.x, 1.0));
        assert!(rig.sample_clip(2, 0.5, ClipWrap::Clamp, &mut pose));
        assert!(close(pose.joints()[2].scale.x, 2.0));
        // Slerp halfway is an eighth turn.
        assert!(rig.sample_clip(0, 0.5, ClipWrap::Clamp, &mut pose));
        let eighth = Quat::from_rotation_z(core::f32::consts::FRAC_PI_4);
        assert!(pose.joints()[1].rotation.dot(eighth).abs() > 0.999_99);
        assert!(!rig.sample_clip(9, 0.0, ClipWrap::Loop, &mut pose));
        assert_eq!(
            pose,
            Pose::rest(&rig),
            "a missing clip leaves the rest pose"
        );
        for time in [f32::NAN, f32::INFINITY, -1e30] {
            for wrap in [ClipWrap::Loop, ClipWrap::Clamp] {
                assert!(rig.sample_clip(0, time, wrap, &mut pose));
                assert!(pose.joints().iter().all(|joint| joint.rotation.is_finite()));
            }
        }
    }

    #[test]
    fn crossfade_masks_and_additive_layers() {
        let rig = rig();
        let rest = Pose::rest(&rig);
        let mut lifted = Pose::default();
        rig.sample_clip(1, 2.0, ClipWrap::Clamp, &mut lifted);
        let mut turned = Pose::default();
        rig.sample_clip(0, 1.0, ClipWrap::Clamp, &mut turned);

        let mut pose = rest.clone();
        pose.blend(&lifted, 0.25, BoneMask::ALL);
        assert!(close(pose.joints()[0].translation.y, 0.5));
        pose.blend(&lifted, f32::NAN, BoneMask::ALL);
        assert!(
            close(pose.joints()[0].translation.y, 0.5),
            "NaN weight is 0"
        );
        pose.blend(&lifted, 1.0, BoneMask::ALL);
        assert_eq!(pose, lifted);

        // An upper-body mask (spine subtree) leaves the root and the leg alone.
        let upper = BoneMask::subtree(&rig, 1);
        assert_eq!(upper.bits(), 0b0110);
        assert_eq!(upper.complement().bits() & 0b1111, 0b1001);
        assert!(BoneMask::subtree(&rig, 9).is_empty());
        let mut masked = lifted.clone();
        masked.blend(&turned, 1.0, upper);
        assert!(
            close(masked.joints()[0].translation.y, 2.0),
            "root keeps lift"
        );
        assert_eq!(masked.joints()[1].rotation, turned.joints()[1].rotation);

        // Shortest path: blending to the negated quaternion is a no-op rotation.
        let mut flipped = rest.clone();
        flipped.joints_mut()[1].rotation = -turned.joints()[1].rotation;
        let mut half = turned.clone();
        half.blend(&flipped, 0.5, BoneMask::ALL);
        assert!(
            half.joints()[1]
                .rotation
                .dot(turned.joints()[1].rotation)
                .abs()
                > 0.999_99
        );

        // Additive: the difference to the reference is layered onto another pose.
        let mut layered = lifted.clone();
        layered.add(&turned, &rest, 1.0, BoneMask::ALL);
        assert!(close(layered.joints()[0].translation.y, 2.0));
        assert!(
            layered.joints()[1]
                .rotation
                .dot(turned.joints()[1].rotation)
                .abs()
                > 0.999_99
        );
        let mut none = lifted.clone();
        none.add(&turned, &rest, 0.0, BoneMask::ALL);
        assert_eq!(none, lifted);
    }

    #[test]
    fn model_and_skinning_matrices_follow_the_hierarchy() {
        let rig = rig();
        let mut pose = Pose::rest(&rig);
        let mut model = [Mat4::IDENTITY; MAX_JOINTS];
        let mut skin = [[[0.0; 4]; 3]; MAX_JOINTS];
        assert!(rig.model_transforms(&pose, &mut model));
        assert!(model[2].w_axis.truncate().distance(Vec3::Y * 2.0) < 1e-6);
        assert!(rig.skinning_matrices(&model, &mut skin));
        // Bind pose: identity skinning.
        assert_eq!(skin[2], joint_matrix(Mat4::IDENTITY));
        // Lifting the root moves every child.
        pose.joints_mut()[0].translation = Vec3::Y;
        assert!(rig.model_transforms(&pose, &mut model));
        assert!(model[2].w_axis.truncate().distance(Vec3::Y * 3.0) < 1e-6);
        assert!(!rig.model_transforms(&pose, &mut model[..2]));
        assert!(!rig.skinning_matrices(&model[..2], &mut skin));
        assert!(!rig.model_transforms(&Pose::default(), &mut model));
    }

    #[test]
    fn blend_space_weights_and_samples() {
        let rig = rig();
        assert_eq!(BlendSpace1d::new(&[]), Err(BlendSpaceError::PointCount));
        assert_eq!(
            BlendSpace1d::new(&[(1.0, 0), (1.0, 1)]),
            Err(BlendSpaceError::Unordered)
        );
        assert_eq!(
            BlendSpace1d::new(&[(f32::NAN, 0)]),
            Err(BlendSpaceError::Unordered)
        );
        assert_eq!(
            BlendSpace1d::new(&[(0.0, 300)]),
            Err(BlendSpaceError::ClipIndex)
        );
        let space = BlendSpace1d::new(&[(0.0, 0), (4.0, 1)]).expect("space");
        assert_eq!(space.weights(-1.0), (0, 0, 0.0));
        assert_eq!(space.weights(1.0), (0, 1, 0.25));
        assert_eq!(space.weights(9.0), (1, 1, 0.0));
        assert_eq!(space.weights(f32::NAN), (0, 0, 0.0));
        let (mut out, mut scratch) = (Pose::default(), Pose::default());
        space.sample(&rig, 2.0, 1.0, &mut out, &mut scratch);
        // Half lift (clip 1 at 1 s = 1.0) and half spin (clip 0 at 1 s wraps to 0 = identity).
        assert!(close(out.joints()[0].translation.y, 0.5));
    }

    #[test]
    fn blend_space_2d_is_exact_at_points_and_normalised_between() {
        let space = BlendSpace2d::new(&[
            ([0.0, 0.0], 0),
            ([0.0, 4.0], 1),
            ([4.0, 0.0], 2),
            ([-4.0, 0.0], 3),
        ])
        .expect("space");
        for (index, point) in [[0.0, 0.0], [0.0, 4.0], [4.0, 0.0], [-4.0, 0.0]]
            .into_iter()
            .enumerate()
        {
            let weights = space.weights(point);
            assert!((weights[index] - 1.0).abs() < 1e-5, "{index}: {weights:?}");
        }
        let between = space.weights([1.0, 2.0]);
        let total: f32 = between.iter().sum();
        assert!((total - 1.0).abs() < 1e-5);
        assert!(
            between[1] > between[3] && between[2] > between[3],
            "{between:?}"
        );
        let (first, second, share) = space.dominant([0.0, 3.0]);
        assert_eq!((first, second), (1, 0));
        assert!(share > 0.0 && share < 0.5);
        assert_eq!(
            BlendSpace2d::new(&[([0.0, 0.0], 0), ([0.0, 0.0], 1)]),
            Err(BlendSpaceError::Coincident)
        );
        assert_eq!(BlendSpace2d::new(&[]), Err(BlendSpaceError::PointCount));
        // Sampling exactly at the forward point is that clip.
        let skeleton = rig();
        let mut out = Pose::default();
        let mut scratch = Pose::default();
        space.sample(&skeleton, [0.0, 4.0], 0.5, &mut out, &mut scratch);
        let mut expected = Pose::default();
        skeleton.sample_clip(1, 0.5, ClipWrap::Loop, &mut expected);
        assert_eq!(out, expected);
    }
}
