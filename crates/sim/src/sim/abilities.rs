//! Ability casting, area effects and persistent zones.

use moba_core::{Fix, FixVec2};
use moba_ecs::Entity;
use moba_game::{AreaEffect, AreaEffectList, Effect, ModifierKind, Targeting, Team};

use crate::components::{Cooldowns, Health, HeroIdentity, Mana, Modifiers, Position};

use super::{
    MAX_PROJECTILES, MAX_ZONES, ONE_TICK, Projectile, RecallCancel, SimEvent, Simulation, Zone,
};

impl Simulation {
    fn gather_enemies(&mut self, team: Team, target: FixVec2, radius: Fix) {
        self.damage_hits.clear();
        self.grid.for_each_in_circle(target, radius, |candidate| {
            if candidate.team == team {
                return;
            }
            if self
                .world
                .get::<Health>(candidate.entity)
                .is_some_and(|health| health.current > 0)
            {
                self.damage_hits.push(candidate.entity);
            }
        });
    }

    #[allow(clippy::too_many_lines)] // One validated targeting/effect pass.
    pub(super) fn cast_ability(&mut self, caster: Entity, slot: u8, mut target: FixVec2) {
        let Some(identity) = self.world.get::<HeroIdentity>(caster).copied() else {
            return;
        };
        let Some(def) = self.content.heroes.get(usize::from(identity.0.0)) else {
            return;
        };
        let ability_index = usize::from(slot);
        let Some(ability) = def
            .abilities
            .get(ability_index)
            .and_then(|&id| self.content.abilities.get(usize::from(id)))
        else {
            return;
        };
        let cooldown_ticks = ability.cooldown_ticks;
        let mana_cost = ability.mana_cost;
        let targeting = ability.targeting;
        let effects = ability.effects;
        let Some(cooldowns) = self.world.get::<Cooldowns>(caster).copied() else {
            return;
        };
        if cooldowns.0[ability_index] > 0 {
            return;
        }
        let Some(mana) = self.world.get::<Mana>(caster).copied() else {
            return;
        };
        if mana.current < mana_cost {
            return;
        }
        let Some(position) = self.world.get::<Position>(caster).map(|p| p.0) else {
            return;
        };
        let Some(team) = self.world.get::<Team>(caster).copied() else {
            return;
        };
        if self
            .world
            .get::<Modifiers>(caster)
            .is_some_and(|m| m.has(ModifierKind::Silence) || m.has(ModifierKind::Stun))
        {
            return;
        }
        if self
            .world
            .get::<Modifiers>(caster)
            .is_some_and(|m| m.has(ModifierKind::Root) || m.has(ModifierKind::Stun))
            && effects
                .iter()
                .any(|effect| matches!(effect, Effect::Displace { .. }))
        {
            return;
        }
        match targeting {
            Targeting::SelfCast => target = position,
            Targeting::Point { range } => {
                if range == Fix::ZERO {
                    target = position;
                } else if position.distance(target) > range {
                    return;
                }
            }
            Targeting::Direction { range } => {
                let distance = position.distance(target);
                if distance == Fix::ZERO || distance > range {
                    return;
                }
            }
        }
        // The cast passed validation: it interrupts a recall channel.
        self.cancel_recall(caster, RecallCancel::Cast);
        self.push_event(SimEvent::AbilityCast {
            caster,
            team,
            ability: slot,
            target,
        });
        for effect in effects.iter() {
            match effect {
                Effect::None => {}
                Effect::Damage {
                    amount,
                    damage_type,
                } => {
                    self.gather_enemies(team, target, Fix::ONE);
                    let mut hits = core::mem::take(&mut self.damage_hits);
                    for target_entity in hits.drain(..) {
                        self.damage(target_entity, *amount, *damage_type, caster);
                    }
                    self.damage_hits = hits;
                }
                Effect::Heal { amount } => {
                    if let Some(health) = self.world.get_mut::<Health>(caster) {
                        health.current = (health.current + amount).min(health.max);
                    }
                }
                Effect::ApplyModifier {
                    modifier,
                    duration_ticks,
                    factor,
                    amount,
                    damage_type,
                } => {
                    self.apply_modifier(
                        caster,
                        *modifier,
                        *duration_ticks,
                        *factor,
                        *amount,
                        caster,
                        *damage_type,
                    );
                }
                Effect::Displace { range } => {
                    let delta = target - position;
                    let distance = delta.length().min(*range);
                    if distance > Fix::ZERO {
                        let destination =
                            self.clamp_position(position + delta.normalize_or_zero() * distance);
                        if let Some(current) = self.world.get_mut::<Position>(caster) {
                            current.0 = destination;
                        }
                    }
                }
                Effect::Projectile {
                    speed_per_sec,
                    range,
                    radius,
                    arc,
                    on_hit,
                } => {
                    let direction = (target - position).normalize_or_zero();
                    if direction == FixVec2::ZERO || self.projectiles.len() >= MAX_PROJECTILES {
                        continue;
                    }
                    self.projectiles.push(Projectile {
                        owner: caster,
                        team,
                        position,
                        direction,
                        speed_per_tick: *speed_per_sec * ONE_TICK,
                        remaining: *range,
                        radius: *radius,
                        arc: *arc,
                        on_hit: *on_hit,
                    });
                    self.push_event(SimEvent::ProjectileSpawned {
                        owner: caster,
                        team,
                        position,
                        direction,
                        speed_per_sec: *speed_per_sec,
                        radius: *radius,
                        arc: *arc,
                        range: *range,
                    });
                }
                Effect::Area { radius, effects } => {
                    self.apply_area_effects(caster, team, target, *radius, *effects);
                }
                Effect::Zone {
                    radius,
                    duration_ticks,
                    period_ticks,
                    effects,
                } => {
                    if self.zones.len() < MAX_ZONES {
                        self.zones.push(Zone {
                            owner: caster,
                            team,
                            position: target,
                            radius: *radius,
                            remaining: *duration_ticks,
                            period_ticks: *period_ticks,
                            cooldown: 0,
                            effects: *effects,
                        });
                        self.push_event(SimEvent::ZoneSpawned {
                            owner: caster,
                            team,
                            position: target,
                            radius: *radius,
                            duration_ticks: *duration_ticks,
                        });
                    }
                }
            }
        }

        if let Some(mana) = self.world.get_mut::<Mana>(caster) {
            mana.current = (mana.current - mana_cost).max(0);
        }
        if let Some(cooldowns) = self.world.get_mut::<Cooldowns>(caster) {
            cooldowns.0[ability_index] = cooldown_ticks;
        }
    }

    fn apply_area_effects(
        &mut self,
        caster: Entity,
        team: Team,
        center: FixVec2,
        radius: Fix,
        effects: AreaEffectList,
    ) {
        self.gather_enemies(team, center, radius);
        let mut hits = core::mem::take(&mut self.damage_hits);
        for target in hits.drain(..) {
            self.apply_area_effects_to(caster, target, effects);
        }
        self.damage_hits = hits;
    }

    pub(super) fn apply_area_effects_to(
        &mut self,
        caster: Entity,
        target: Entity,
        effects: AreaEffectList,
    ) {
        for effect in effects.iter() {
            match effect {
                AreaEffect::None => {}
                AreaEffect::Damage {
                    amount,
                    damage_type,
                } => self.damage(target, *amount, *damage_type, caster),
                AreaEffect::ApplyModifier {
                    modifier,
                    duration_ticks,
                    factor,
                    amount,
                    damage_type,
                } => self.apply_modifier(
                    target,
                    *modifier,
                    *duration_ticks,
                    *factor,
                    *amount,
                    caster,
                    *damage_type,
                ),
            }
        }
    }

    pub(super) fn tick_zones(&mut self) {
        let mut index = 0;
        while index < self.zones.len() {
            let mut zone = self.zones[index];
            if zone.cooldown == 0 {
                self.apply_area_effects(
                    zone.owner,
                    zone.team,
                    zone.position,
                    zone.radius,
                    zone.effects,
                );
                zone.cooldown = zone.period_ticks;
            }
            zone.cooldown = zone.cooldown.saturating_sub(1);
            zone.remaining = zone.remaining.saturating_sub(1);
            if zone.remaining == 0 {
                self.zones.swap_remove(index);
            } else {
                self.zones[index] = zone;
                index += 1;
            }
        }
    }
}
