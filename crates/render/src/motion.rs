//! The motion history (ADR-0021 §2c): what each keyed instance looked like on the previous
//! presented frame, so the prepass can write per-object motion vectors.
//!
//! The client gives every unit part a [`UnitInstance::motion_key`](crate::UnitInstance)
//! (`net_id << 16 | part`; 0 for map instances, which have no history) and sets
//! `motion_reset` on a unit's first drawn frame and across teleports. The renderer records the
//! transform, joint matrices and VAT pose it **drew** for each key, and reads them back the
//! next frame: the drawn path is what the reprojection needs, whatever the frame rate or the
//! tick phase. No new client state is involved.
//!
//! - **Fixed capacity:** two open-addressing tables of [`MOTION_HISTORY_CAPACITY`] entries and
//!   two joint-matrix stores, all allocated at creation. A frame swaps them; entries are
//!   invalidated by a generation stamp, so nothing is cleared or allocated per frame.
//! - **Dropped keys:** an entry not seen this frame is gone next frame (it is not copied).
//! - **Missing or reset:** a key without an entry, with `motion_reset`, or whose drawn
//!   position moved more than [`TELEPORT_GUARD_METRES`] (a teleport the client missed) uses
//!   its current state as its previous one: camera-only motion.

use moba_assets::{JointMatrix, VatPose};
use moba_core::math::Mat4;

/// Slots per table (ADR-0021 §2c).
pub(crate) const MOTION_HISTORY_CAPACITY: usize = 4096;
/// Keys recorded per frame: the table is never filled past 3/4, so a lookup (hit or miss)
/// probes a short run. Keys beyond it get no history (camera-only motion).
pub(crate) const MOTION_HISTORY_KEYS: usize = MOTION_HISTORY_CAPACITY / 4 * 3;
/// A keyed instance whose drawn position moved further than this in one frame is treated as
/// reset: no interpolated move can do that (ADR-0021 §2c).
pub(crate) const TELEPORT_GUARD_METRES: f32 = 20.0;

/// One instance as drawn on a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DrawnState {
    pub(crate) transform: Mat4,
    pub(crate) vat: VatPose,
    /// The joint matrices' range in that frame's joint store (empty when unskinned or when
    /// the store was full).
    pub(crate) first_joint: u32,
    pub(crate) joint_count: u32,
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    key: u32,
    stamp: u32,
    state: DrawnState,
}

const EMPTY: Entry = Entry {
    key: 0,
    stamp: 0,
    state: DrawnState {
        transform: Mat4::IDENTITY,
        vat: VatPose::single(moba_assets::VatSample::new(0, 0, 0.0)),
        first_joint: 0,
        joint_count: 0,
    },
};

/// One frame's table and joint store.
struct Frame {
    entries: Box<[Entry]>,
    joints: Vec<JointMatrix>,
    /// The generation the entries of this frame carry (0 is never a live generation).
    stamp: u32,
    len: usize,
}

impl Frame {
    fn new(max_joints: usize) -> Self {
        Self {
            entries: vec![EMPTY; MOTION_HISTORY_CAPACITY].into_boxed_slice(),
            joints: Vec::with_capacity(max_joints),
            stamp: 0,
            len: 0,
        }
    }

    /// The slot holding `key`, or the empty slot where it would go (`None` when full).
    fn slot(&self, key: u32) -> Option<usize> {
        let mask = MOTION_HISTORY_CAPACITY - 1;
        let mut index = hash(key) & mask;
        for _ in 0..MOTION_HISTORY_CAPACITY {
            let entry = &self.entries[index];
            if entry.stamp != self.stamp || entry.key == key {
                return Some(index);
            }
            index = (index + 1) & mask;
        }
        None
    }

    fn get(&self, key: u32) -> Option<&DrawnState> {
        if self.stamp == 0 {
            return None;
        }
        let index = self.slot(key)?;
        let entry = &self.entries[index];
        (entry.stamp == self.stamp && entry.key == key).then_some(&entry.state)
    }
}

const _: () = assert!(MOTION_HISTORY_CAPACITY.is_power_of_two());

/// Fibonacci hashing of a motion key (`net_id << 16 | part` keeps the part in the low bits).
fn hash(key: u32) -> usize {
    (key.wrapping_mul(0x9E37_79B9) >> 20) as usize
}

/// The renderer's motion history: last frame's drawn states by key, and this frame's being
/// recorded.
pub(crate) struct MotionHistory {
    frames: [Frame; 2],
    /// Index of the frame being recorded; the other one is the previous frame.
    current: usize,
    generation: u32,
}

impl MotionHistory {
    /// A history whose joint stores hold up to `max_joints` matrices per frame.
    pub(crate) fn new(max_joints: usize) -> Self {
        Self {
            frames: [Frame::new(max_joints), Frame::new(max_joints)],
            current: 0,
            generation: 0,
        }
    }

    /// Starts recording a new frame: the frame recorded last becomes the previous one. With
    /// `reset` (`Scene::history_reset`, a camera cut, a resize) the previous frame is
    /// forgotten too, so every key gets camera-only motion.
    pub(crate) fn begin_frame(&mut self, reset: bool) {
        self.current ^= 1;
        self.generation = self.generation.wrapping_add(1).max(1);
        let frame = &mut self.frames[self.current];
        frame.stamp = self.generation;
        frame.len = 0;
        frame.joints.clear();
        if reset {
            self.frames[self.current ^ 1].stamp = 0;
        }
    }

    /// The previous drawn state of `key` and its joint matrices, when it has a usable one: not
    /// key 0, not `reset`, and not further than [`TELEPORT_GUARD_METRES`] from `current`.
    pub(crate) fn previous(
        &self,
        key: u32,
        reset: bool,
        current: &Mat4,
    ) -> Option<(DrawnState, &[JointMatrix])> {
        if key == 0 || reset {
            return None;
        }
        let previous = &self.frames[self.current ^ 1];
        let state = *previous.get(key)?;
        let moved = (current.w_axis.truncate() - state.transform.w_axis.truncate()).length();
        if !moved.is_finite() || moved > TELEPORT_GUARD_METRES {
            return None;
        }
        let first = state.first_joint as usize;
        let joints = previous
            .joints
            .get(first..first + state.joint_count as usize)
            .unwrap_or(&[]);
        Some((state, joints))
    }

    /// Records what `key` drew this frame (`joints`: its joint matrices, empty when
    /// unskinned). Key 0, a new key past [`MOTION_HISTORY_KEYS`] and a full joint store are
    /// skipped: the key then has no history (or no joint history) next frame.
    pub(crate) fn record(
        &mut self,
        key: u32,
        transform: Mat4,
        vat: VatPose,
        joints: &[JointMatrix],
    ) {
        if key == 0 {
            return;
        }
        let frame = &mut self.frames[self.current];
        let Some(index) = frame.slot(key) else {
            return;
        };
        let new = frame.entries[index].stamp != frame.stamp;
        if new && frame.len >= MOTION_HISTORY_KEYS {
            return;
        }
        let (first_joint, joint_count) =
            if frame.joints.len() + joints.len() <= frame.joints.capacity() {
                let first = frame.joints.len();
                frame.joints.extend_from_slice(joints);
                (
                    u32::try_from(first).unwrap_or(0),
                    u32::try_from(joints.len()).unwrap_or(0),
                )
            } else {
                (0, 0)
            };
        let entry = &mut frame.entries[index];
        if new {
            frame.len += 1;
        }
        *entry = Entry {
            key,
            stamp: frame.stamp,
            state: DrawnState {
                transform,
                vat,
                first_joint,
                joint_count,
            },
        };
    }

    /// Keys recorded this frame.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.frames[self.current].len
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moba_core::math::Vec3;

    fn at(x: f32) -> Mat4 {
        Mat4::from_translation(Vec3::new(x, 0.0, 0.0))
    }

    fn pose() -> VatPose {
        VatPose::single(moba_assets::VatSample::new(3, 4, 0.5))
    }

    #[test]
    fn a_key_reads_back_what_it_drew_last_frame_only() {
        let mut history = MotionHistory::new(16);
        history.begin_frame(false);
        history.record(7, at(1.0), pose(), &[]);
        assert!(
            history.previous(7, false, &at(1.0)).is_none(),
            "nothing before"
        );
        history.begin_frame(false);
        let (state, joints) = history.previous(7, false, &at(1.5)).expect("history");
        assert_eq!(state.transform, at(1.0));
        assert_eq!(state.vat, pose());
        assert_eq!(joints, &[] as &[JointMatrix]);
        // Not drawn this frame: gone the frame after.
        history.begin_frame(false);
        assert!(history.previous(7, false, &at(1.5)).is_none());
    }

    #[test]
    fn resets_key_zero_and_teleports_have_no_history() {
        let mut history = MotionHistory::new(16);
        history.begin_frame(false);
        history.record(0, at(1.0), pose(), &[]);
        history.record(9, at(1.0), pose(), &[]);
        assert_eq!(history.len(), 1, "key 0 is never recorded");
        history.begin_frame(false);
        assert!(
            history.previous(9, true, &at(1.0)).is_none(),
            "motion_reset"
        );
        assert!(
            history.previous(9, false, &at(21.5)).is_none(),
            "teleport guard"
        );
        assert!(history.previous(9, false, &at(20.5)).is_some());
        history.record(9, at(1.0), pose(), &[]);
        history.begin_frame(true);
        assert!(
            history.previous(9, false, &at(1.0)).is_none(),
            "history reset"
        );
    }

    #[test]
    fn joints_are_kept_per_key_up_to_the_store() {
        let joint = |value: f32| -> JointMatrix { [[value; 4]; 3] };
        let mut history = MotionHistory::new(3);
        history.begin_frame(false);
        history.record(1, at(0.0), pose(), &[joint(1.0), joint(2.0)]);
        // Does not fit: recorded without joints.
        history.record(2, at(0.0), pose(), &[joint(3.0), joint(4.0)]);
        history.begin_frame(false);
        let (_, joints) = history.previous(1, false, &at(0.0)).unwrap();
        assert_eq!(joints, [joint(1.0), joint(2.0)]);
        let (state, joints) = history.previous(2, false, &at(0.0)).unwrap();
        assert_eq!((state.joint_count, joints.len()), (0, 0));
    }

    #[test]
    fn the_table_holds_three_quarters_of_its_slots_and_no_more() {
        let mut history = MotionHistory::new(0);
        history.begin_frame(false);
        for key in 1..=(MOTION_HISTORY_CAPACITY as u32 + 10) {
            history.record(key << 16, at(0.0), pose(), &[]);
        }
        assert_eq!(history.len(), MOTION_HISTORY_KEYS);
        history.begin_frame(false);
        let found = (1..=MOTION_HISTORY_CAPACITY as u32)
            .filter(|key| history.previous(key << 16, false, &at(0.0)).is_some())
            .count();
        assert_eq!(found, MOTION_HISTORY_KEYS);
        // The first keys are the ones kept, and a later key is a miss.
        assert!(history.previous(1 << 16, false, &at(0.0)).is_some());
        assert!(
            history
                .previous((MOTION_HISTORY_CAPACITY as u32) << 16, false, &at(0.0))
                .is_none()
        );
        // Re-recording a key in the same frame replaces it.
        history.record(5, at(1.0), pose(), &[]);
        history.record(5, at(2.0), pose(), &[]);
        assert_eq!(history.len(), 1);
    }
}
