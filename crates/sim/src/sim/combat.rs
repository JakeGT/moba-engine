//! Basic attacks, projectiles, damage application and damage modifiers.

use moba_core::{Fix, FixVec2};
use moba_ecs::Entity;
use moba_game::{DamageType, ModifierKind, Team, mitigate_typed};

use crate::components::{
    AttackStats, AttackTarget, Dead, Health, HeroSlot, LastHitBy, Modifiers, Position, Progression,
    Resistances,
};

use super::vision::cross;
use super::{AttackerSnapshot, RecallCancel, SimEvent, Simulation};

impl Simulation {
    pub(super) fn modifier_armor(&self, entity: Entity) -> i32 {
        self.world.get::<Modifiers>(entity).map_or(0, |modifiers| {
            modifiers
                .entries()
                .iter()
                .filter_map(|modifier| self.content.modifiers.get(usize::from(modifier.def_id)))
                .map(|definition| definition.bonus_armor)
                .sum()
        })
    }

    pub(super) fn modifier_attack_damage(&self, entity: Entity) -> i32 {
        self.world.get::<Modifiers>(entity).map_or(0, |modifiers| {
            modifiers
                .entries()
                .iter()
                .filter_map(|modifier| self.content.modifiers.get(usize::from(modifier.def_id)))
                .map(|definition| definition.bonus_attack_damage)
                .sum()
        })
    }

    pub(super) fn modifier_on_damaged(&self, target: Entity) -> i32 {
        self.world.get::<Modifiers>(target).map_or(0, |modifiers| {
            modifiers
                .entries()
                .iter()
                .filter_map(|modifier| self.content.modifiers.get(usize::from(modifier.def_id)))
                .map(|definition| definition.on_damaged_damage)
                .sum()
        })
    }

    pub(super) fn on_damaged_bonus(&self, target: Entity) -> i32 {
        if self.world.get::<HeroSlot>(target).is_none() {
            return 0;
        }
        self.world.get::<Progression>(target).map_or(0, |progress| {
            progress.items[..usize::from(progress.item_count)]
                .iter()
                .filter_map(|item| self.content.items.get(usize::from(*item)))
                .map(|item| i32::from(item.on_damaged_damage))
                .sum()
        })
    }

    pub(super) fn on_hit_bonus(&self, attacker: Entity) -> i32 {
        if self.world.get::<HeroSlot>(attacker).is_none() {
            return 0;
        }
        self.world
            .get::<Progression>(attacker)
            .map_or(0, |progress| {
                progress.items[..usize::from(progress.item_count)]
                    .iter()
                    .filter_map(|item| self.content.items.get(usize::from(*item)))
                    .map(|item| i32::from(item.on_hit_damage))
                    .sum()
            })
    }

    pub(super) fn resolve_attacks(&mut self) {
        self.attackers.clear();
        for (attacker, (stats, target)) in self.world.query::<(&AttackStats, &AttackTarget)>() {
            if stats.cooldown > 0 {
                continue;
            }
            let Some(target) = target.0 else {
                continue;
            };
            self.attackers.push(AttackerSnapshot {
                attacker,
                target,
                damage: stats.damage,
                range_sq: stats.range_sq,
                interval: stats.interval,
            });
        }
        let attackers = core::mem::take(&mut self.attackers);
        let alternate = !self.blue_first();
        for index in 0..attackers.len() {
            let snapshot = if alternate {
                attackers[attackers.len() - 1 - index]
            } else {
                attackers[index]
            };
            if self.world.has::<Dead>(snapshot.attacker) {
                continue;
            }
            let Some(attacker_position) = self
                .world
                .get::<Position>(snapshot.attacker)
                .map(|position| position.0)
            else {
                continue;
            };
            let Some(target_position) = self
                .world
                .get::<Position>(snapshot.target)
                .map(|position| position.0)
            else {
                continue;
            };
            let Some(target_health) = self.world.get::<Health>(snapshot.target) else {
                continue;
            };
            if target_health.current <= 0 {
                continue;
            }
            let Some(attacker_team) = self.world.get::<Team>(snapshot.attacker).copied() else {
                continue;
            };
            let target_team = self.world.get::<Team>(snapshot.target).copied();
            if Some(attacker_team) == target_team {
                continue;
            }
            if attacker_position.distance_sq(target_position) > snapshot.range_sq {
                continue;
            }
            let bonus = self.on_hit_bonus(snapshot.attacker);
            let modifier_damage = self.modifier_attack_damage(snapshot.attacker);
            self.damage(
                snapshot.target,
                snapshot
                    .damage
                    .saturating_add(bonus)
                    .saturating_add(modifier_damage),
                DamageType::Physical,
                snapshot.attacker,
            );
            let thorns = self
                .on_damaged_bonus(snapshot.target)
                .saturating_add(self.modifier_on_damaged(snapshot.target));
            if thorns > 0 && self.world.has::<Health>(snapshot.attacker) {
                self.damage(
                    snapshot.attacker,
                    thorns,
                    DamageType::Physical,
                    snapshot.target,
                );
            }
            if let Some(stats) = self.world.get_mut::<AttackStats>(snapshot.attacker) {
                stats.cooldown = snapshot.interval;
            }
            // Presentation only (animation hit frames): events are outputs, never state.
            self.push_event(SimEvent::BasicAttack {
                attacker: snapshot.attacker,
                team: attacker_team,
                target: snapshot.target,
                interval_ticks: snapshot.interval,
            });
        }
        self.attackers = attackers;
    }

    pub(super) fn update_projectiles(&mut self) {
        let mut index = 0;
        while index < self.projectiles.len() {
            let mut projectile = self.projectiles[index];
            let start = projectile.position;
            projectile.position += projectile.direction * projectile.speed_per_tick;
            let end = projectile.position;
            let sweep = (end - start).length() > projectile.radius * Fix::from_i32(2);
            let midpoint = (start + end) / Fix::from_i32(2);
            let radius = if sweep {
                projectile.radius + (end - start).length() / Fix::from_i32(2)
            } else {
                projectile.radius
            };
            let mut hit: Option<(Fix, Entity)> = None;
            self.grid.for_each_in_circle(midpoint, radius, |candidate| {
                if candidate.team == projectile.team {
                    return;
                }
                let distance = if sweep {
                    segment_distance_sq(candidate.position, start, end)
                } else {
                    candidate.position.distance_sq(end)
                };
                if distance <= projectile.radius * projectile.radius
                    && hit.is_none_or(|(best, entity)| {
                        distance < best || (distance == best && candidate.entity < entity)
                    })
                {
                    hit = Some((distance, candidate.entity));
                }
            });
            if let Some((_, target)) = hit {
                self.push_event(SimEvent::ProjectileHit {
                    owner: projectile.owner,
                    team: projectile.team,
                    target,
                    position: projectile.position,
                });
                self.apply_area_effects_to(projectile.owner, target, projectile.on_hit);
                self.projectiles.swap_remove(index);
                continue;
            }
            projectile.remaining -= projectile.speed_per_tick;
            if projectile.remaining <= Fix::ZERO {
                self.projectiles.swap_remove(index);
            } else {
                self.projectiles[index] = projectile;
                index += 1;
            }
        }
    }

    pub(super) fn damage(
        &mut self,
        target: Entity,
        amount: i32,
        damage_type: DamageType,
        source: Entity,
    ) {
        let Some(source_team) = self.world.get::<Team>(source).copied() else {
            return;
        };
        let Some(health) = self.world.get::<Health>(target).copied() else {
            return;
        };
        if health.current <= 0 {
            return;
        }
        let resistances = self
            .world
            .get::<Resistances>(target)
            .copied()
            .unwrap_or(Resistances {
                armor: 0,
                magic_resist: 0,
            });
        let bonus_armor = self.modifier_armor(target);
        let dealt = mitigate_typed(
            amount,
            damage_type,
            resistances.armor.saturating_add(bonus_armor),
            resistances.magic_resist,
        );
        if dealt > 0 {
            // Any damage that reaches the unit (before shields) interrupts a recall.
            self.cancel_recall(target, RecallCancel::Damage);
        }
        let mut damage = dealt;
        if let Some(modifiers) = self.world.get_mut::<Modifiers>(target) {
            let mut index = 0;
            while index < usize::from(modifiers.len) {
                let modifier = &mut modifiers.entries[index];
                if modifier.kind == ModifierKind::Shield && modifier.amount > 0 {
                    let absorbed = modifier.amount.min(damage);
                    modifier.amount -= absorbed;
                    damage -= absorbed;
                }
                if modifier.kind == ModifierKind::Shield && modifier.amount <= 0 {
                    modifiers
                        .entries
                        .swap(index, usize::from(modifiers.len) - 1);
                    modifiers.len -= 1;
                } else {
                    index += 1;
                }
            }
        }
        let remaining = if let Some(health) = self.world.get_mut::<Health>(target) {
            health.current = (health.current - damage).max(0);
            health.current
        } else {
            return;
        };
        if remaining <= 0 {
            let target_team = self.world.get::<Team>(target).copied();
            if target_team.is_some_and(|team| team != source_team) {
                let _ = self.world.insert(
                    target,
                    LastHitBy {
                        team: source_team,
                        entity: source,
                    },
                );
            }
        }
    }
}

pub(super) fn segment_distance_sq(point: FixVec2, start: FixVec2, end: FixVec2) -> Fix {
    let segment = end - start;
    let length_sq = segment.length_sq();
    if length_sq == Fix::ZERO {
        return point.distance_sq(start);
    }
    let offset = point - start;
    let area = cross(segment, offset);
    area * area / length_sq
}
