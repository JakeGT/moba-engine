//! Foliage displacers (R3b, ADR-0017 §2): which units bend the grass this frame.
//!
//! Built **only** from this frame's [`Anchors`], i.e. the units the client draws: in a match
//! those come from the server's snapshot, which never contains a unit the team cannot see,
//! so a hidden enemy bends nothing and brush can never give it away. Structures do not bend
//! foliage (none grows near them). The nearest units to the camera focus are kept, up to
//! [`MAX_FOLIAGE_DISPLACERS`]; the list is preallocated, so building it never allocates.

use moba_core::math::Vec3;
use moba_render::{FoliageDisplacer, MAX_FOLIAGE_DISPLACERS};

use super::vfx::Anchors;

/// Units taller than this are structures (towers 9 m, nexus 15 m): no displacement.
const MAX_DISPLACER_HEIGHT: f32 = 4.0;

/// This frame's displacers.
#[derive(Debug)]
pub(crate) struct Displacers {
    list: Vec<FoliageDisplacer>,
    /// Squared distance to the focus per entry (kept sorted with `list`).
    distances: Vec<f32>,
}

impl Displacers {
    pub(crate) fn new() -> Self {
        Self {
            list: Vec::with_capacity(MAX_FOLIAGE_DISPLACERS),
            distances: Vec::with_capacity(MAX_FOLIAGE_DISPLACERS),
        }
    }

    /// Rebuilds the list from `anchors`, nearest to `focus` (on the ground) first.
    pub(crate) fn build(&mut self, anchors: &Anchors, focus: Vec3) {
        self.list.clear();
        self.distances.clear();
        for anchor in anchors.units() {
            if anchor.height > MAX_DISPLACER_HEIGHT || !anchor.position.is_finite() {
                continue;
            }
            let dx = anchor.position.x - focus.x;
            let dz = anchor.position.z - focus.z;
            let distance = dx * dx + dz * dz;
            // Insertion into the sorted, capped list: no allocation.
            let at = self.distances.partition_point(|&other| other <= distance);
            if at >= MAX_FOLIAGE_DISPLACERS {
                continue;
            }
            if self.list.len() == MAX_FOLIAGE_DISPLACERS {
                self.list.pop();
                self.distances.pop();
            }
            let radius = (anchor.height * 0.45).clamp(0.7, 1.6);
            self.list.insert(
                at,
                FoliageDisplacer::new([anchor.position.x, anchor.position.z], radius, 1.0),
            );
            self.distances.insert(at, distance);
        }
    }

    pub(crate) fn as_slice(&self) -> &[FoliageDisplacer] {
        &self.list
    }
}

/// How strongly the displacers bend a blade rooted at `x, z`, `0..=1`: the CPU twin of
/// `foliage.vert`'s flattening (used by the tests).
#[cfg(test)]
pub(crate) fn displacement_at(displacers: &[FoliageDisplacer], x: f32, z: f32) -> f32 {
    displacers.iter().fold(0.0f32, |most, displacer| {
        let d = ((x - displacer.position[0]).powi(2) + (z - displacer.position[1]).powi(2)).sqrt();
        if d < displacer.radius {
            let f = 1.0 - d / displacer.radius;
            most.max(f * f * displacer.strength)
        } else {
            most
        }
    })
}

#[cfg(test)]
mod tests {
    use moba_core::math::Vec3;

    use super::{Displacers, displacement_at};
    use crate::app::vfx::{Anchors, UnitAnchor};

    fn anchor(id: u32, x: f32, z: f32, height: f32) -> UnitAnchor {
        UnitAnchor {
            id,
            position: Vec3::new(x, 0.0, z),
            height,
            model: None,
        }
    }

    /// ADR-0017 §2: displacement comes only from drawn units. The snapshot the server sends a
    /// team withholds an enemy it cannot see (in brush or the fog of war); the anchors are
    /// rebuilt from that snapshot, so the hidden enemy bends no blade where it stands, while
    /// the same unit, once visible and drawn, does.
    #[test]
    fn hidden_units_produce_no_foliage_displacement() {
        let ally = anchor(1, -20.0, 0.0, 3.6);
        let hidden_enemy = anchor(2, 10.0, 5.0, 3.6);
        let tower = anchor(3, 30.0, 0.0, 9.0);
        // What the client draws: the ally and the tower; the hidden enemy is not in the
        // snapshot, so it is never anchored.
        let mut anchors = Anchors::new();
        anchors.push(ally);
        anchors.push(tower);
        let mut displacers = Displacers::new();
        displacers.build(&anchors, Vec3::ZERO);
        assert_eq!(
            displacers.as_slice().len(),
            1,
            "structures never bend foliage"
        );
        assert_eq!(displacement_at(displacers.as_slice(), 10.0, 5.0), 0.0);
        assert_eq!(displacement_at(displacers.as_slice(), 10.5, 5.0), 0.0);
        assert!(displacement_at(displacers.as_slice(), -20.0, 0.0) > 0.9);
        // Revealed (in the snapshot and drawn): now it bends the grass.
        anchors.clear();
        anchors.push(ally);
        anchors.push(hidden_enemy);
        displacers.build(&anchors, Vec3::ZERO);
        assert!(displacement_at(displacers.as_slice(), 10.0, 5.0) > 0.9);
    }

    #[test]
    fn the_nearest_units_are_kept_without_allocating() {
        let mut anchors = Anchors::new();
        for index in 0..100u32 {
            anchors.push(anchor(index, index as f32, 0.0, 2.0));
        }
        let mut displacers = Displacers::new();
        displacers.build(&anchors, Vec3::new(50.0, 0.0, 0.0));
        let list = displacers.as_slice();
        assert_eq!(list.len(), moba_render::MAX_FOLIAGE_DISPLACERS);
        assert_eq!(list[0].position, [50.0, 0.0]);
        assert!(
            list.iter()
                .all(|displacer| (displacer.position[0] - 50.0).abs() <= 16.0)
        );
        let ((), allocations) = crate::app::allocation_probe::probe(|| {
            for _ in 0..10 {
                displacers.build(&anchors, Vec3::new(20.0, 0.0, 0.0));
            }
        });
        assert_eq!(allocations, 0);
    }
}
