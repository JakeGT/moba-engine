//! Fog-of-war observation: per-team visible unit lists and wall line-of-sight.

use moba_core::{Fix, FixVec2};
use moba_ecs::Entity;
use moba_game::{EMPTY_ITEM, ModifierKind, Team};

use crate::components::{
    Cooldowns, Health, HeroIdentity, HeroSlot, Mana, Modifiers, Position, Progression, UnitKind,
    VisionRange,
};

use super::{MAX_UNITS, ObservedCandidate, ObservedUnit, Simulation, VisionSource};

impl Simulation {
    /// Builds the list of units visible to `viewer` for a snapshot.
    ///
    /// Teammates are always visible (including dead heroes so the HUD can show respawn
    /// timers). Enemy structures are treated as permanently known map features; enemy
    /// heroes and minions are visible only while inside the vision radius of a living
    /// teammate. The output vector is cleared first and must be owned by the caller so the
    /// hot path allocates nothing after warm-up.
    pub fn observed_units(&mut self, viewer: Team, out: &mut Vec<ObservedUnit>) {
        self.observe(Some(viewer), out);
    }

    /// Whether `entity` is currently visible to `viewer` under the fog rules.
    pub fn is_visible_to(&mut self, viewer: Team, entity: Entity) -> bool {
        let mut observed = core::mem::take(&mut self.bot_observed[viewer.index()]);
        self.observe(Some(viewer), &mut observed);
        let visible = observed.iter().any(|unit| unit.entity == entity);
        self.bot_observed[viewer.index()] = observed;
        visible
    }

    /// Builds the complete unit list for a spectator (no fog filtering).
    pub fn observed_all(&mut self, out: &mut Vec<ObservedUnit>) {
        self.observe(None, out);
    }

    #[allow(clippy::too_many_lines)] // One fog pass keeps the visibility rules together.
    fn observe(&mut self, viewer: Option<Team>, out: &mut Vec<ObservedUnit>) {
        out.clear();
        let mut sources = [(FixVec2::ZERO, Fix::ZERO); MAX_UNITS];
        let mut source_count = 0usize;
        let mut max_vision = Fix::ZERO;
        if let Some(viewer) = viewer {
            for (_entity, (team, health, position, vision)) in
                self.world
                    .query::<(&Team, &Health, &Position, &VisionRange)>()
            {
                if *team != viewer || health.current <= 0 {
                    continue;
                }
                max_vision = max_vision.max(vision.0);
                if source_count < MAX_UNITS {
                    sources[source_count] = (position.0, vision.0);
                    source_count += 1;
                }
            }
        }

        let mut vision_grid = self.vision_grid.take().expect("vision grid");
        vision_grid.clear();
        for &(position, range) in &sources[..source_count] {
            vision_grid.insert(position, VisionSource { position, range });
        }
        self.observed_candidates.clear();
        for (entity, (team, kind, position, health)) in
            self.world.query::<(&Team, &UnitKind, &Position, &Health)>()
        {
            self.observed_candidates.push(ObservedCandidate {
                entity,
                team: *team,
                kind: *kind,
                position: position.0,
                health: health.current,
                max_health: health.max,
            });
        }
        let candidates = core::mem::take(&mut self.observed_candidates);
        for candidate in &candidates {
            let entity = candidate.entity;
            let team = candidate.team;
            let kind = candidate.kind;
            let position = candidate.position;
            let dead = candidate.health <= 0;
            let allied = viewer.is_some_and(|viewer| team == viewer);
            let visible = viewer.is_none()
                || allied
                || kind.is_structure()
                || (!dead
                    && vision_grid.any_in_circle(position, max_vision, |source| {
                        source.position.distance_sq(position) <= source.range * source.range
                            && self.content.map.walls.iter().all(|wall| {
                                !segments_intersect(source.position, position, wall.a, wall.b)
                            })
                    }));
            if !visible {
                continue;
            }
            let mana = self
                .world
                .get::<Mana>(entity)
                .map_or((0, 0), |mana| (mana.current, mana.max));
            let hero_slot = self.world.get::<HeroSlot>(entity).map(|slot| slot.0);
            let hero_pick = self
                .world
                .get::<HeroIdentity>(entity)
                .map_or(0xFF, |identity| identity.0.0);
            let progression = self.world.get::<Progression>(entity).copied();
            let modifiers = self.world.get::<Modifiers>(entity);
            let cooldowns = self
                .world
                .get::<Cooldowns>(entity)
                .map_or([0; 3], |cooldowns| {
                    [
                        u32::from(u16::try_from(cooldowns.0[0]).unwrap_or(u16::MAX)),
                        u32::from(u16::try_from(cooldowns.0[1]).unwrap_or(u16::MAX)),
                        u32::from(u16::try_from(cooldowns.0[2]).unwrap_or(u16::MAX)),
                    ]
                });
            out.push(ObservedUnit {
                entity,
                kind,
                team,
                position,
                health: candidate.health,
                max_health: candidate.max_health,
                mana: mana.0,
                max_mana: mana.1,
                dead: dead && hero_slot.is_some(),
                hero_slot,
                hero_pick,
                cooldowns,
                level: progression.map_or(0, |progress| progress.level),
                xp: progression.map_or(0, |progress| progress.xp),
                gold: progression.map_or(0, |progress| progress.gold),
                items: progression.map_or([EMPTY_ITEM; 6], |progress| progress.items),
                shielded: modifiers.is_some_and(|m| m.shield_total() > 0),
                slowed: modifiers.is_some_and(|m| m.has(ModifierKind::Slow)),
                rooted: modifiers.is_some_and(|m| m.has(ModifierKind::Root)),
                silenced: modifiers.is_some_and(|m| m.has(ModifierKind::Silence)),
                burning: modifiers.is_some_and(|m| m.has(ModifierKind::Burn)),
                hasted: modifiers.is_some_and(|m| m.has(ModifierKind::Haste)),
                status_bits: modifiers.map_or(0, |modifiers| {
                    modifiers
                        .entries()
                        .iter()
                        .filter_map(|modifier| {
                            self.content.modifiers.get(usize::from(modifier.def_id))
                        })
                        .fold(0, |bits, definition| bits | definition.status_bits)
                }),
            });
        }
        self.observed_candidates = candidates;
        self.vision_grid = Some(vision_grid);
    }
}

pub(super) fn cross(a: FixVec2, b: FixVec2) -> Fix {
    a.x * b.y - a.y * b.x
}

pub(super) fn segments_intersect(a: FixVec2, b: FixVec2, c: FixVec2, d: FixVec2) -> bool {
    let ab = b - a;
    let cd = d - c;
    let ac = c - a;
    let ad = d - a;
    let ca = a - c;
    let cb = b - c;
    let d1 = cross(ab, ac);
    let d2 = cross(ab, ad);
    let d3 = cross(cd, ca);
    let d4 = cross(cd, cb);
    ((d1 > Fix::ZERO) != (d2 > Fix::ZERO)) && ((d3 > Fix::ZERO) != (d4 > Fix::ZERO))
}
