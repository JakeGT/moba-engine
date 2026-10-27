//! Deterministic state hashing used by replays and desync checks.

use moba_core::StableHasher;
use moba_ecs::{Entity, World};
use moba_game::{AreaEffect, AreaEffectList, Team};

use crate::components::{
    AttackDamageType, AttackStats, AttackTarget, BaseMoveSpeed, Cooldowns, Dead, Health,
    HeroIdentity, HeroSlot, ItemCooldowns, LaneWalker, LastHitBy, Mana, Modifiers, MoveSpeed,
    MoveTarget, Position, Progression, Regen, Resistances, UnitKind, VisionRange,
};

use super::{MatchOutcome, Simulation};

const HASH_POSITION: u32 = 1;
const HASH_HEALTH: u32 = 2;
const HASH_MANA: u32 = 3;
const HASH_ATTACK_STATS: u32 = 4;
const HASH_ATTACK_TARGET: u32 = 5;
const HASH_MOVE_TARGET: u32 = 6;
const HASH_COOLDOWNS: u32 = 7;
const HASH_LANE_WALKER: u32 = 8;
const HASH_DEAD: u32 = 9;
const HASH_HERO_IDENTITY: u32 = 10;
const HASH_HERO_SLOT: u32 = 11;
const HASH_PROGRESSION: u32 = 12;
const HASH_MODIFIERS: u32 = 13;
const HASH_TEAM: u32 = 19;
const HASH_KIND: u32 = 20;
const HASH_REGEN: u32 = 21;
const HASH_RESISTANCES: u32 = 22;
const HASH_VISION: u32 = 23;
const HASH_MOVE_SPEED: u32 = 24;
const HASH_BASE_MOVE_SPEED: u32 = 25;
const HASH_LAST_HIT_BY: u32 = 26;
const HASH_ATTACK_DAMAGE_TYPE: u32 = 27;
const HASH_PROJECTILE: u32 = 28;
const HASH_ZONE: u32 = 29;
const HASH_ITEM_COOLDOWNS: u32 = 30;

impl Simulation {
    /// Deterministic hash of the complete authoritative state.
    ///
    /// Used by replay/reconciliation tests. Iteration follows ECS archetype order, which is
    /// deterministic for the same command sequence.
    #[must_use]
    #[allow(clippy::too_many_lines)] // Deterministic field coverage is intentionally flat.
    pub fn state_hash(&self) -> u64 {
        self.hash_state(self.config.seed)
    }

    /// Hash that ignores the match seed, for cross-seed balance/outcome comparisons.
    #[must_use]
    pub fn state_hash_normalized(&self) -> u64 {
        self.hash_state(0)
    }

    #[allow(clippy::too_many_lines)] // Deterministic field coverage is intentionally flat.
    fn hash_state(&self, seed: u64) -> u64 {
        let mut hasher = StableHasher::new();
        hasher.write_u64(seed);
        // `bot_mask` is a server-side input source, not simulation state. Recording it
        // only at the end of a replay made mid-match disconnects diverge; generated inputs
        // are already resolved and recorded per tick.
        hasher.write_u32(u32::from(self.config.spawn_waves));
        for pick in self.config.hero_picks {
            hasher.write_u32(u32::from(pick));
        }
        hasher.write_u32(self.tick.get());
        hasher.write_u32(self.wave);
        hasher.write_u32(self.next_wave_tick);
        hasher.write_u32(self.blue_score);
        hasher.write_u32(self.red_score);
        match self.outcome {
            Some(MatchOutcome::Winner(team)) => hasher.write_u32(1 + team.index() as u32),
            Some(MatchOutcome::Draw) => hasher.write_u32(3),
            None => hasher.write_u32(0),
        }

        for projectile in &self.projectiles {
            hasher.write_u32(HASH_PROJECTILE);
            hasher.write_u64(projectile.owner.to_bits());
            hasher.write_u32(projectile.team.index() as u32);
            hasher.write_u64(projectile.position.x.to_bits() as u64);
            hasher.write_u64(projectile.position.y.to_bits() as u64);
            hasher.write_u64(projectile.direction.x.to_bits() as u64);
            hasher.write_u64(projectile.direction.y.to_bits() as u64);
            hasher.write_u64(projectile.speed_per_tick.to_bits() as u64);
            hasher.write_u64(projectile.remaining.to_bits() as u64);
            hasher.write_u64(projectile.radius.to_bits() as u64);
            hasher.write_u64(projectile.arc.to_bits() as u64);
            hash_runtime_area_effects(&mut hasher, &projectile.on_hit);
        }
        for zone in &self.zones {
            hasher.write_u32(HASH_ZONE);
            hasher.write_u64(zone.owner.to_bits());
            hasher.write_u32(zone.team.index() as u32);
            hasher.write_u64(zone.position.x.to_bits() as u64);
            hasher.write_u64(zone.position.y.to_bits() as u64);
            hasher.write_u64(zone.radius.to_bits() as u64);
            hasher.write_u32(zone.remaining);
            hasher.write_u32(zone.period_ticks);
            hasher.write_u32(zone.cooldown);
            hash_runtime_area_effects(&mut hasher, &zone.effects);
        }

        for entity in self.world.entities() {
            hasher.write_u64(entity.to_bits());
            hash_team(&self.world, entity, &mut hasher);
            hash_kind(&self.world, entity, &mut hasher);
            if let Some(position) = self.world.get::<Position>(entity) {
                hasher.write_u32(HASH_POSITION);
                hasher.write_u64(position.0.x.to_bits() as u64);
                hasher.write_u64(position.0.y.to_bits() as u64);
            }
            if let Some(health) = self.world.get::<Health>(entity) {
                hasher.write_u32(HASH_HEALTH);
                hasher.write_u32(health.current as u32);
                hasher.write_u32(health.max as u32);
            }
            if let Some(mana) = self.world.get::<Mana>(entity) {
                hasher.write_u32(HASH_MANA);
                hasher.write_u32(mana.current as u32);
                hasher.write_u32(mana.max as u32);
            }
            if let Some(stats) = self.world.get::<AttackStats>(entity) {
                hasher.write_u32(HASH_ATTACK_STATS);
                hasher.write_u32(stats.damage as u32);
                hasher.write_u64(stats.range.to_bits() as u64);
                hasher.write_u64(stats.range_sq.to_bits() as u64);
                hasher.write_u32(stats.interval);
                hasher.write_u32(stats.cooldown);
            }
            if let Some(target) = self.world.get::<AttackTarget>(entity) {
                hasher.write_u32(HASH_ATTACK_TARGET);
                hasher.write_u64(target.0.map_or(u64::MAX, Entity::to_bits));
            }
            if let Some(target) = self.world.get::<MoveTarget>(entity) {
                hasher.write_u32(HASH_MOVE_TARGET);
                match target.0 {
                    Some(position) => {
                        hasher.write_u32(1);
                        hasher.write_u64(position.x.to_bits() as u64);
                        hasher.write_u64(position.y.to_bits() as u64);
                    }
                    None => hasher.write_u32(0),
                }
            }
            if let Some(cooldowns) = self.world.get::<Cooldowns>(entity) {
                hasher.write_u32(HASH_COOLDOWNS);
                for cooldown in cooldowns.0 {
                    hasher.write_u32(cooldown);
                }
            }
            if let Some(cooldowns) = self.world.get::<ItemCooldowns>(entity) {
                hasher.write_u32(HASH_ITEM_COOLDOWNS);
                for cooldown in cooldowns.0 {
                    hasher.write_u32(u32::from(cooldown));
                }
            }
            if let Some(walker) = self.world.get::<LaneWalker>(entity) {
                hasher.write_u32(HASH_LANE_WALKER);
                hasher.write_u32(u32::from(walker.lane));
                hasher.write_u32(u32::from(walker.waypoint));
            }
            if let Some(dead) = self.world.get::<Dead>(entity) {
                hasher.write_u32(HASH_DEAD);
                hasher.write_u32(dead.respawn_in);
            }
            if let Some(identity) = self.world.get::<HeroIdentity>(entity) {
                hasher.write_u32(HASH_HERO_IDENTITY);
                hasher.write_u32(u32::from(identity.0.0));
            }
            if let Some(slot) = self.world.get::<HeroSlot>(entity) {
                hasher.write_u32(HASH_HERO_SLOT);
                hasher.write_u32(u32::from(slot.0));
            }
            if let Some(progress) = self.world.get::<Progression>(entity) {
                hasher.write_u32(HASH_PROGRESSION);
                hasher.write_u32(u32::from(progress.level));
                hasher.write_u32(progress.xp);
                hasher.write_u32(progress.gold);
                hasher.write_u32(u32::from(progress.item_count));
                for item in progress.items {
                    hasher.write_u32(u32::from(item));
                }
            }
            if let Some(modifiers) = self.world.get::<Modifiers>(entity) {
                hasher.write_u32(HASH_MODIFIERS);
                hasher.write_u32(u32::from(modifiers.len));
                for modifier in modifiers.entries() {
                    hasher.write_u32(u32::from(modifier.def_id));
                    hasher.write_u32(modifier.kind as u32);
                    hasher.write_u32(modifier.remaining);
                    hasher.write_u64(modifier.factor.to_bits() as u64);
                    hasher.write_u32(modifier.amount as u32);
                    hasher.write_u32(modifier.damage_type as u32);
                    hasher.write_u64(modifier.source.to_bits());
                }
            }
            if let Some(regen) = self.world.get::<Regen>(entity) {
                hasher.write_u32(HASH_REGEN);
                hasher.write_u32(regen.health as u32);
                hasher.write_u32(regen.mana as u32);
            }
            if let Some(resistances) = self.world.get::<Resistances>(entity) {
                hasher.write_u32(HASH_RESISTANCES);
                hasher.write_u32(resistances.armor as u32);
                hasher.write_u32(resistances.magic_resist as u32);
            }
            if let Some(vision) = self.world.get::<VisionRange>(entity) {
                hasher.write_u32(HASH_VISION);
                hasher.write_u64(vision.0.to_bits() as u64);
            }
            if let Some(speed) = self.world.get::<MoveSpeed>(entity) {
                hasher.write_u32(HASH_MOVE_SPEED);
                hasher.write_u64(speed.0.to_bits() as u64);
            }
            if let Some(speed) = self.world.get::<BaseMoveSpeed>(entity) {
                hasher.write_u32(HASH_BASE_MOVE_SPEED);
                hasher.write_u64(speed.0.to_bits() as u64);
            }
            if let Some(last_hit) = self.world.get::<LastHitBy>(entity) {
                hasher.write_u32(HASH_LAST_HIT_BY);
                hasher.write_u32(last_hit.team.index() as u32);
                hasher.write_u64(last_hit.entity.to_bits());
            }
            if let Some(damage_type) = self.world.get::<AttackDamageType>(entity) {
                hasher.write_u32(HASH_ATTACK_DAMAGE_TYPE);
                hasher.write_u32(damage_type.0 as u32);
            }
        }
        hasher.finish64()
    }
}

fn hash_team(world: &World, entity: Entity, hasher: &mut StableHasher) {
    if let Some(team) = world.get::<Team>(entity) {
        hasher.write_u32(HASH_TEAM);
        hasher.write_u32(team.index() as u32);
    }
}

fn hash_runtime_area_effects(hasher: &mut StableHasher, effects: &AreaEffectList) {
    hasher.write_u32(u32::try_from(effects.iter().count()).unwrap_or(u32::MAX));
    for effect in effects.iter() {
        match effect {
            AreaEffect::None => {}
            AreaEffect::Damage {
                amount,
                damage_type,
            } => {
                hasher.write_u32(0);
                hasher.write_u32(*amount as u32);
                hasher.write_u32(*damage_type as u32);
            }
            AreaEffect::ApplyModifier {
                modifier,
                duration_ticks,
                factor,
                amount,
                damage_type,
            } => {
                hasher.write_u32(1);
                hasher.write_u32(u32::from(*modifier));
                hasher.write_u32(*duration_ticks);
                hasher.write_u64(factor.to_bits() as u64);
                hasher.write_u32(*amount as u32);
                hasher.write_u32(*damage_type as u32);
            }
        }
    }
}

fn hash_kind(world: &World, entity: Entity, hasher: &mut StableHasher) {
    if let Some(kind) = world.get::<UnitKind>(entity) {
        hasher.write_u32(HASH_KIND);
        hasher.write_u32(*kind as u32);
    }
}
