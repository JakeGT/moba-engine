//! Per-frame joint matrices for GPU-skinned heroes (WP-4.1).
//!
//! Heroes whose model has an animation set are posed by the animation runtime
//! (`app::animation`, ADR-0012) and copied in with [`BonePool::push`]. The rest use the
//! WP-4.1 pose, which comes only from presentation state the client already has:
//! interpolated position, the facing and gait derived from it, and the render tick. The
//! locomotion clip (`run`, else `walk`, else the first) plays at full weight while a hero
//! moves (`gait = 1`) and the skeleton's rest pose is the idle pose (`gait = 0`). There are no
//! new wire fields.
//!
//! [`BonePool`] is allocated once with the renderer's bone budget and cleared every frame.
//! A hero that does not fit is not allocated for: it is counted and drawn with its static
//! mesh that frame.

use moba_assets::Skeleton;
use moba_render::{BoneRange, JointMatrix};

/// Seconds of phase offset per unit of `phase_offset`, so heroes do not walk in lockstep.
const PHASE_SECONDS: f32 = 0.137;

/// Preallocated joint matrices for one frame.
#[derive(Debug)]
pub(crate) struct BonePool {
    bones: Box<[JointMatrix]>,
    used: usize,
    truncated: u32,
    /// Truncation is logged once per process; [`Self::truncated`] has the per-frame count.
    truncation_logged: bool,
}

impl BonePool {
    /// A pool for `capacity` joint matrices (normally [`moba_render::DEFAULT_MAX_BONES`]).
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            bones: vec![[[0.0; 4]; 3]; capacity].into_boxed_slice(),
            used: 0,
            truncated: 0,
            truncation_logged: false,
        }
    }

    /// Starts a frame: forgets every pose and the truncation count. Never allocates.
    pub(crate) fn clear(&mut self) {
        self.used = 0;
        self.truncated = 0;
    }

    /// Joint matrices written this frame, for [`moba_render::Scene::with_bones`].
    pub(crate) fn bones(&self) -> &[JointMatrix] {
        &self.bones[..self.used]
    }

    /// Skinned heroes that did not fit this frame and fell back to their static mesh.
    #[cfg(test)]
    pub(crate) fn truncated(&self) -> u32 {
        self.truncated
    }

    /// Copies already-posed joint matrices (an [`moba_assets::Animator`]'s) into the next
    /// free joints and returns their range, or `None` (counted) when the pool is full.
    /// Never allocates.
    pub(crate) fn push(&mut self, matrices: &[JointMatrix]) -> Option<BoneRange> {
        let end = self.used + matrices.len();
        if matrices.is_empty() {
            return None;
        }
        let Some(slots) = self.bones.get_mut(self.used..end) else {
            self.note_truncation();
            return None;
        };
        slots.copy_from_slice(matrices);
        let range = BoneRange::new(
            u32::try_from(self.used).ok()?,
            u32::try_from(matrices.len()).ok()?,
        );
        self.used = end;
        Some(range)
    }

    fn note_truncation(&mut self) {
        self.truncated = self.truncated.saturating_add(1);
        if !self.truncation_logged {
            self.truncation_logged = true;
            eprintln!(
                "[render] skinned hero over the {}-joint budget; drawing it static",
                self.bones.len()
            );
        }
    }

    /// Poses `rig` into the next free joints and returns their range.
    ///
    /// `time` is in seconds (the render tick / 30), `phase_offset` desynchronises units and
    /// `gait` (0 idle .. 1 walking) blends from the rest pose to the clip. Returns `None` and
    /// counts the hero when the pool is full. Never allocates.
    pub(crate) fn pose(
        &mut self,
        rig: &Skeleton,
        time: f32,
        phase_offset: f32,
        gait: f32,
    ) -> Option<BoneRange> {
        let joints = rig.joint_count();
        let end = self.used + joints;
        let Some(slots) = self.bones.get_mut(self.used..end) else {
            self.note_truncation();
            return None;
        };
        let time = time + phase_offset * PHASE_SECONDS;
        if !rig.sample(time, gait, slots) {
            return None;
        }
        // `end <= bones.len() == max_bones`, a few thousand at most.
        let range = BoneRange::new(u32::try_from(self.used).ok()?, u32::try_from(joints).ok()?);
        self.used = end;
        Some(range)
    }
}

#[cfg(test)]
mod tests {
    use super::BonePool;

    const SKINNED_SAMPLE: &[u8] = include_bytes!("../../../../content/models/sentinel-skinned.glb");

    #[test]
    fn poses_fill_the_pool_then_count_truncation() {
        let mesh = moba_assets::load_glb(SKINNED_SAMPLE).expect("sample");
        let rig = mesh.skin().expect("skin").skeleton();
        let joints = rig.joint_count();
        let mut pool = BonePool::new(joints * 2);
        let first = pool.pose(rig, 0.5, 0.0, 1.0).expect("fits");
        let second = pool.pose(rig, 0.5, 3.0, 0.0).expect("fits");
        assert_eq!((first.first, first.count), (0, 10));
        assert_eq!((second.first, second.count), (10, 10));
        assert!(pool.pose(rig, 0.5, 0.0, 1.0).is_none());
        assert_eq!(pool.truncated(), 1);
        assert_eq!(pool.bones().len(), joints * 2);
        // Idle (gait 0) is the rest pose, which for this rig is the bind pose (identity).
        assert!((pool.bones()[joints][0][0] - 1.0).abs() < 1e-6);
        pool.clear();
        assert_eq!(pool.bones().len(), 0);
        assert_eq!(pool.truncated(), 0);
    }
}
