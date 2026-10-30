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
    use moba_assets::VatQuality;
    use moba_core::math::Vec3;
    use moba_net::protocol::{ENTITY_FLAG_DEAD, MatchPhase, NetEntity, NetProgress, SnapshotState};
    use moba_render::MeshId;

    use super::{Displacers, displacement_at};
    use crate::app::NetworkView;
    use crate::app::animation::{AnimLod, AnimationDriver};
    use crate::app::bone_lod::BoneLod;
    use crate::app::map_visuals::MapLibrary;
    use crate::app::minions::{MinionDriver, MinionLibrary};
    use crate::app::models::{ModelBindings, ModelLibrary};
    use crate::app::scene_build::{AnimationFrame, animated_rig, rebuild_network_units};
    use crate::app::skinning::BonePool;
    use crate::app::vfx::{Anchors, UnitAnchor};

    fn anchor(id: u32, x: f32, z: f32, height: f32) -> UnitAnchor {
        UnitAnchor {
            id,
            position: Vec3::new(x, 0.0, z),
            height,
            model: None,
        }
    }

    fn hero(net_id: u16, team: u8, x: f32, y: f32, dead: bool) -> NetEntity {
        let quantize = |value: f32| (value * 16.0) as u16;
        NetEntity {
            net_id,
            kind: 0,
            team,
            hero_slot: (net_id % 5) as u8,
            hero_pick: 0,
            flags: if dead { ENTITY_FLAG_DEAD } else { 0 },
            status_bits: 0,
            max_health: 600,
            max_mana: 0,
            x: quantize(x),
            y: quantize(y),
            health: if dead { 0 } else { 600 },
            mana: 0,
            cooldowns: [0; 3],
            progress: NetProgress::default(),
        }
    }

    /// One client frame at snapshot `tick` (pinned, as in a capture): the snapshot arrives,
    /// the animation driver sees its death edges, and `rebuild_network_units` rebuilds the
    /// drawn units and the anchors the displacers are built from.
    struct Frame {
        network: NetworkView,
        models: ModelLibrary,
        map: MapLibrary,
        minion_library: MinionLibrary,
        driver: AnimationDriver,
        minions: MinionDriver,
        units: Vec<moba_render::UnitInstance>,
        bones: BonePool,
        anchors: Anchors,
        displacers: Displacers,
    }

    impl Frame {
        fn step(&mut self, tick: u32, entities: Vec<NetEntity>) -> Vec<(u32, Vec3)> {
            let mut state = SnapshotState::default();
            state.tick = tick;
            state.entities = entities;
            let models = &self.models;
            self.driver.on_snapshot(&state, |pick| {
                (pick == 0)
                    .then(|| animated_rig(models, models.hero(0)))
                    .flatten()
            });
            self.network.interpolator.push(state);
            self.network.last_tick = tick;
            self.network.capture_tick = Some(tick);
            rebuild_network_units(
                &self.network,
                &self.models,
                &self.map,
                &mut self.units,
                &mut self.bones,
                AnimationFrame {
                    driver: &mut self.driver,
                    view: None,
                    minions: &mut self.minions,
                    minion_library: &self.minion_library,
                    bone_lod: BoneLod::for_tier(moba_render::QualityTier::Low),
                    far_attachments: false,
                },
                &mut self.anchors,
                false,
            );
            self.displacers.build(&self.anchors, Vec3::ZERO);
            self.anchors
                .units()
                .iter()
                .map(|anchor| (anchor.id, anchor.position))
                .collect()
        }
    }

    /// ADR-0017 §2, end to end: the displacers are built from the anchors that
    /// `rebuild_network_units` makes of the team's snapshot, as in the app. The server
    /// withholds an enemy the team cannot see (in brush or the fog of war), so where it
    /// stands no blade bends, while the same unit bends the grass while it is in the
    /// snapshot; a dead hero bends it while its death clip plays and not once the clip and
    /// linger are over (it is no longer drawn).
    #[test]
    fn hidden_and_long_dead_units_produce_no_foliage_displacement() {
        let content =
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content"));
        let mut models = ModelLibrary::new(content.clone(), ModelBindings::default());
        models.bind_heroes([Some("sentinel-skinned")], |_, _| Ok(MeshId::CUBE));
        assert!(
            models.graph(0).is_some(),
            "the sample's animation set binds"
        );
        let mut network = NetworkView::test_stub();
        network.phase = MatchPhase::Playing;
        let mut frame = Frame {
            network,
            models,
            map: MapLibrary::new(content.clone()),
            minion_library: MinionLibrary::new(content, false, VatQuality::FULL),
            driver: AnimationDriver::new(AnimLod::parse("off"), 30),
            minions: MinionDriver::new(30),
            units: Vec::new(),
            bones: BonePool::new(4_096),
            anchors: Anchors::new(),
            displacers: Displacers::new(),
        };
        let (ally, enemy, fallen) = (1, 2, 3);
        let bends =
            |frame: &Frame, at: Vec3| displacement_at(frame.displacers.as_slice(), at.x, at.z);
        let find = |anchors: &[(u32, Vec3)], id: u16| {
            anchors
                .iter()
                .find(|(other, _)| *other == u32::from(id))
                .map(|(_, position)| *position)
        };

        // Tick 100: everyone in the snapshot and alive.
        let seen = frame.step(
            100,
            vec![
                hero(ally, 0, 100.0, 100.0, false),
                hero(enemy, 1, 112.0, 104.0, false),
                hero(fallen, 1, 90.0, 94.0, false),
            ],
        );
        let enemy_at = find(&seen, enemy).expect("a visible enemy is anchored");
        let fallen_at = find(&seen, fallen).expect("anchored while alive");
        assert!(
            bends(&frame, enemy_at) > 0.9,
            "a visible enemy bends the grass"
        );
        assert!(bends(&frame, fallen_at) > 0.9);

        // Tick 101: the enemy steps into brush, so the snapshot withholds it; the third
        // hero dies (its death clip starts).
        let entities = || {
            vec![
                hero(ally, 0, 100.0, 100.0, false),
                hero(fallen, 1, 90.0, 94.0, true),
            ]
        };
        let seen = frame.step(101, entities());
        assert_eq!(
            find(&seen, enemy),
            None,
            "a withheld enemy is never anchored"
        );
        assert_eq!(
            bends(&frame, enemy_at),
            0.0,
            "no blade bends where it stands"
        );
        assert_eq!(bends(&frame, enemy_at + Vec3::new(0.5, 0.0, 0.0)), 0.0);
        assert!(
            find(&seen, fallen).is_some() && bends(&frame, fallen_at) > 0.9,
            "a dying hero is drawn and bends the grass while its death clip plays"
        );

        // A minute later: still dead, the clip and linger long over, so it is not drawn.
        let seen = frame.step(101 + 60 * 30, entities());
        assert_eq!(find(&seen, fallen), None, "a long-dead hero is not drawn");
        assert_eq!(bends(&frame, fallen_at), 0.0, "and bends nothing");
        assert!(find(&seen, ally).is_some_and(|at| bends(&frame, at) > 0.9));
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
