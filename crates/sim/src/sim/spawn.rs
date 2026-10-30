//! Structure, hero and minion-wave spawning, plus hero respawns.

use moba_core::{Fix, FixVec2};
use moba_ecs::Entity;
use moba_game::{DamageType, EMPTY_ITEM, HeroId, Team};

use crate::components::{
    AttackDamageType, AttackStats, AttackTarget, BaseMoveSpeed, Cooldowns, Dead, Health,
    HeroIdentity, HeroSlot, ItemCooldowns, LaneWalker, LastHitBy, Mana, Modifiers, MoveSpeed,
    MoveTarget, Position, Progression, Recall, Regen, Resistances, UnitKind, VisionRange,
};

use super::{MAX_UNITS, ONE_TICK, SimEvent, Simulation};

impl Simulation {
    pub(super) fn spawn_structures(&mut self) {
        for structure in self.content.map.structures.clone() {
            let entity = self.world.spawn();
            let (stats, kind) = match structure.kind {
                moba_game::StructureKind::Tower => (&self.content.rules.tower, UnitKind::Tower),
                moba_game::StructureKind::Nexus => (&self.content.rules.nexus, UnitKind::Nexus),
            };
            let _ = self.world.insert(entity, Position(structure.position));
            let _ = self.world.insert(entity, structure.team);
            let _ = self.world.insert(entity, kind);
            let _ = self.world.insert(
                entity,
                Health {
                    current: stats.max_health,
                    max: stats.max_health,
                },
            );
            let _ = self.world.insert(
                entity,
                AttackStats::new(
                    stats.attack_damage,
                    stats.attack_range,
                    stats.attack_interval_ticks,
                ),
            );
            let _ = self.world.insert(
                entity,
                Resistances {
                    armor: stats.armor,
                    magic_resist: stats.magic_resist,
                },
            );
            let _ = self.world.insert(entity, AttackTarget::default());
            let _ = self.world.insert(entity, VisionRange(stats.vision_range));
            let _ = self
                .world
                .insert(entity, AttackDamageType(DamageType::Physical));
            let _ = self.world.insert(entity, Modifiers::default());
            let _ = self.world.insert(entity, MoveTarget::default());
        }
    }

    pub(super) fn spawn_hero(&mut self, slot: usize) -> Entity {
        let team_size = usize::from(self.content.rules.team_size);
        let team = if slot < team_size {
            Team::Blue
        } else {
            Team::Red
        };
        let pick = usize::from(self.config.hero_picks[slot]) % self.content.heroes.len();
        let hero_id = HeroId(u8::try_from(pick).expect("hero index fits u8"));
        let def = &self.content.heroes[pick];
        let entity = self.world.spawn();
        let _ = self.world.insert(
            entity,
            Position(
                self.content
                    .map
                    .hero_spawn(team, u8::try_from(slot % team_size).expect("slot fits u8")),
            ),
        );
        let _ = self.world.insert(entity, team);
        let _ = self.world.insert(entity, UnitKind::Hero);
        let _ = self.world.insert(
            entity,
            Health {
                current: def.max_health,
                max: def.max_health,
            },
        );
        let _ = self.world.insert(
            entity,
            Mana {
                current: def.max_mana,
                max: def.max_mana,
            },
        );
        let _ = self.world.insert(
            entity,
            AttackStats::new(
                def.attack_damage,
                def.attack_range,
                def.attack_interval_ticks,
            ),
        );
        let _ = self.world.insert(
            entity,
            Resistances {
                armor: def.armor,
                magic_resist: def.magic_resist,
            },
        );
        let base_speed = def.move_speed_per_sec * ONE_TICK;
        let _ = self.world.insert(entity, MoveSpeed(base_speed));
        let _ = self.world.insert(entity, BaseMoveSpeed(base_speed));
        let _ = self.world.insert(entity, MoveTarget::default());
        let _ = self.world.insert(entity, AttackTarget::default());
        let _ = self
            .world
            .insert(entity, HeroSlot(u8::try_from(slot).expect("slot fits u8")));
        let _ = self.world.insert(entity, HeroIdentity(hero_id));
        let _ = self.world.insert(entity, Cooldowns::default());
        let _ = self.world.insert(entity, ItemCooldowns::default());
        let _ = self.world.insert(
            entity,
            Progression {
                level: 1,
                xp: 0,
                gold: self.content.rules.starting_gold,
                items: [EMPTY_ITEM; 6],
                item_count: 0,
            },
        );
        let _ = self.world.insert(
            entity,
            Regen {
                health: def.health_regen_per_tick,
                mana: def.mana_regen_per_tick,
            },
        );
        let _ = self.world.insert(entity, VisionRange(def.vision_range));
        let _ = self
            .world
            .insert(entity, AttackDamageType(DamageType::Physical));
        let _ = self.world.insert(entity, Modifiers::default());
        let _ = self.world.insert(entity, Recall::default());
        entity
    }

    fn spawn_minion(
        &mut self,
        team: Team,
        lane: usize,
        kind: UnitKind,
        position: FixVec2,
        waypoint: u16,
    ) -> Option<Entity> {
        if self.world.entity_count() >= MAX_UNITS {
            return None;
        }
        let def = if kind == UnitKind::MeleeMinion {
            &self.content.rules.melee_minion
        } else {
            &self.content.rules.ranged_minion
        };
        let entity = self.world.spawn();
        let _ = self.world.insert(entity, Position(position));
        let _ = self.world.insert(entity, team);
        let _ = self.world.insert(entity, kind);
        let _ = self.world.insert(
            entity,
            Health {
                current: def.max_health,
                max: def.max_health,
            },
        );
        let _ = self.world.insert(
            entity,
            AttackStats::new(
                def.attack_damage,
                def.attack_range,
                def.attack_interval_ticks,
            ),
        );
        let _ = self.world.insert(
            entity,
            Resistances {
                armor: def.armor,
                magic_resist: def.magic_resist,
            },
        );
        let base_speed = def.move_speed_per_sec * ONE_TICK;
        let _ = self.world.insert(entity, MoveSpeed(base_speed));
        let _ = self.world.insert(entity, BaseMoveSpeed(base_speed));
        let _ = self.world.insert(entity, MoveTarget::default());
        let _ = self.world.insert(entity, AttackTarget::default());
        let _ = self.world.insert(
            entity,
            LaneWalker {
                lane: u8::try_from(lane).expect("lane fits u8"),
                waypoint,
            },
        );
        let _ = self.world.insert(entity, VisionRange(def.vision_range));
        let _ = self
            .world
            .insert(entity, AttackDamageType(DamageType::Physical));
        let _ = self.world.insert(entity, Modifiers::default());
        Some(entity)
    }

    pub(super) fn spawn_wave(&mut self) {
        self.wave = self.wave.saturating_add(1);
        // Alternate spawn order so neither team systematically receives the first tick of
        // wave movement/entity ordering.
        let order = if self.wave.is_multiple_of(2) {
            [Team::Red, Team::Blue]
        } else {
            [Team::Blue, Team::Red]
        };
        for team in order {
            let lanes: Vec<(FixVec2, FixVec2)> = self
                .content
                .map
                .lanes
                .iter()
                .map(|lane| (lane.point(team, 0), lane.point(team, 1)))
                .collect();
            for (lane_index, (start, next)) in lanes.into_iter().enumerate() {
                let direction = (next - start).normalize_or_zero();
                // The map is mirrored in x (as are hero spawns), so red's formation offset
                // is the x-mirror of blue's. `perpendicular()` alone is a 90° rotation, which
                // made the two formations point reflections of each other: in the top lane
                // blue's ranged minion spawned ahead of the lane start and red's behind it
                // (#290). Negating it for red restores the mirror.
                let perpendicular = match team {
                    Team::Blue => direction.perpendicular(),
                    Team::Red => -direction.perpendicular(),
                };
                for index in 0..3i32 {
                    let offset = Fix::from_i32(index - 1);
                    let position = self.clamp_position(
                        start
                            + perpendicular * offset * Fix::from_i32(2)
                            + direction * Fix::from_i32(-1),
                    );
                    self.spawn_minion(team, lane_index, UnitKind::MeleeMinion, position, 1);
                }
                let position = self.clamp_position(start + perpendicular * Fix::from_i32(6));
                self.spawn_minion(team, lane_index, UnitKind::RangedMinion, position, 1);
            }
        }
        self.push_event(SimEvent::WaveSpawned { wave: self.wave });
    }

    pub(super) fn respawn_hero(&mut self, entity: Entity) {
        let Some(team) = self.world.get::<Team>(entity).copied() else {
            return;
        };
        let Some(slot) = self.world.get::<HeroSlot>(entity).map(|slot| slot.0) else {
            return;
        };
        let _ = self.world.remove::<Dead>(entity);
        let _ = self.world.remove::<LastHitBy>(entity);
        if let Some(modifiers) = self.world.get_mut::<Modifiers>(entity) {
            modifiers.clear();
        }
        self.recompute_move_speed(entity);
        if let Some(position) = self.world.get_mut::<Position>(entity) {
            position.0 = self
                .content
                .map
                .hero_spawn(team, slot % self.content.rules.team_size);
        }
        if let Some(health) = self.world.get_mut::<Health>(entity) {
            health.current = health.max;
        }
        if let Some(mana) = self.world.get_mut::<Mana>(entity) {
            mana.current = mana.max;
        }
        if let Some(cooldowns) = self.world.get_mut::<Cooldowns>(entity) {
            cooldowns.0 = [0; 3];
        }
        if let Some(stats) = self.world.get_mut::<AttackStats>(entity) {
            stats.cooldown = 0;
        }
        if let Some(recall) = self.world.get_mut::<Recall>(entity) {
            recall.remaining = 0;
        }
        if let Some(target) = self.world.get_mut::<MoveTarget>(entity) {
            target.0 = None;
        }
        if let Some(target) = self.world.get_mut::<AttackTarget>(entity) {
            target.0 = None;
        }
        self.push_event(SimEvent::HeroRespawned { entity });
    }
}
