//! Death resolution, bounties, experience and levelling.

use moba_core::FixVec2;
use moba_ecs::Entity;
use moba_game::{LEVEL_GROWTH, Team};

use crate::components::{
    AttackStats, AttackTarget, Dead, Health, HeroIdentity, HeroSlot, LastHitBy, Mana, MoveTarget,
    Position, Progression, Resistances, UnitKind,
};

use super::{MAX_PLAYERS, RecallCancel, SimEvent, Simulation};

impl Simulation {
    pub(super) fn resolve_deaths(&mut self) {
        let mut deaths = core::mem::take(&mut self.deaths);
        deaths.clear();
        for (entity, (health, team, kind, position)) in
            self.world.query::<(&Health, &Team, &UnitKind, &Position)>()
        {
            if health.current <= 0 {
                deaths.push((entity, *team, *kind, position.0));
            }
        }

        for (entity, team, kind, position) in deaths.iter().copied() {
            if kind.is_hero() {
                if self.world.has::<Dead>(entity) {
                    continue;
                }
                self.cancel_recall(entity, RecallCancel::Death);
                let level = self
                    .world
                    .get::<Progression>(entity)
                    .map_or(1, |progress| progress.level);
                let respawn_in = self.content.rules.hero_respawn_ticks.saturating_add(
                    self.content
                        .rules
                        .respawn_per_level
                        .saturating_mul(u32::from(level.saturating_sub(1))),
                );
                let _ = self.world.insert(entity, Dead { respawn_in });
                if let Some(target) = self.world.get_mut::<MoveTarget>(entity) {
                    target.0 = None;
                }
                if let Some(target) = self.world.get_mut::<AttackTarget>(entity) {
                    target.0 = None;
                }
                let credit = self.world.get::<LastHitBy>(entity).copied();
                let hero_slot = self.world.get::<HeroSlot>(entity).map(|slot| slot.0);
                let killer_slot = credit.and_then(|credit| {
                    self.world.get::<HeroSlot>(credit.entity).map(|slot| slot.0)
                });
                if let Some(credit) = credit {
                    match credit.team {
                        Team::Blue => self.blue_score = self.blue_score.saturating_add(1),
                        Team::Red => self.red_score = self.red_score.saturating_add(1),
                    }
                }
                self.award_bounty(team, kind, position, credit.map(|credit| credit.entity));
                self.push_event(SimEvent::UnitDied {
                    entity,
                    kind,
                    team,
                    position,
                    killer: credit.map(|credit| credit.entity),
                    hero_slot,
                    killer_slot,
                });
                continue;
            }

            let credit = self.world.get::<LastHitBy>(entity).copied();
            let hero_slot = self.world.get::<HeroSlot>(entity).map(|slot| slot.0);
            let killer_slot = credit
                .and_then(|credit| self.world.get::<HeroSlot>(credit.entity).map(|slot| slot.0));
            self.award_bounty(team, kind, position, credit.map(|credit| credit.entity));
            self.world.despawn(entity);
            self.push_event(SimEvent::UnitDied {
                entity,
                kind,
                team,
                position,
                killer: credit.map(|credit| credit.entity),
                hero_slot,
                killer_slot,
            });
            if kind.is_structure() {
                self.push_event(SimEvent::StructureDestroyed {
                    team,
                    kind,
                    position,
                });
                if kind == UnitKind::Nexus && self.content.rules.nexus_win {
                    self.queue_win(team.enemy());
                }
            }
        }
        self.deaths = deaths;
        // Score-limit wins are checked once, after every death this tick, so two teams
        // reaching the limit in the same tick draw instead of the first processed winning.
        let limit = u32::from(self.content.rules.score_limit);
        if limit > 0 {
            if self.blue_score >= limit {
                self.queue_win(Team::Blue);
            }
            if self.red_score >= limit {
                self.queue_win(Team::Red);
            }
        }
    }

    fn award_bounty(
        &mut self,
        dead_team: Team,
        kind: UnitKind,
        position: FixVec2,
        last_hit: Option<Entity>,
    ) {
        let enemy = dead_team.enemy();
        let reward = self.unit_rewards(kind);
        let mut beneficiaries = [Entity::PLACEHOLDER; MAX_PLAYERS];
        let mut count = 0usize;
        for hero in self.hero_entities {
            if self.world.get::<Team>(hero).copied() != Some(enemy) || self.world.has::<Dead>(hero)
            {
                continue;
            }
            let Some(hero_position) = self.world.get::<Position>(hero).map(|p| p.0) else {
                continue;
            };
            if hero_position.distance_sq(position)
                <= self.content.rules.xp_radius * self.content.rules.xp_radius
            {
                beneficiaries[count] = hero;
                count += 1;
            }
        }
        if count > 0 {
            // League-style shared minion experience: 130% split among nearby allies.
            let shared = (reward.0 * self.content.rules.shared_xp_percent / 100)
                / u32::try_from(count).unwrap_or(1);
            let share = shared.max(1);
            for hero in &beneficiaries[..count] {
                self.grant_xp(*hero, share);
            }
        }
        let Some(killer) = last_hit else {
            return;
        };
        if self.world.get::<Team>(killer).copied() != Some(enemy)
            || self.world.get::<HeroSlot>(killer).is_none()
        {
            return;
        }
        if let Some(progress) = self.world.get_mut::<Progression>(killer) {
            progress.gold = progress.gold.saturating_add(reward.1);
        }
    }

    pub(super) fn grant_xp(&mut self, hero: Entity, amount: u32) {
        let Some(mut progress) = self.world.get::<Progression>(hero).copied() else {
            return;
        };
        let cap = self.content.rules.level_cap;
        let xp_base = self.content.rules.xp_base;
        let xp_per_level = self.content.rules.xp_per_level;
        let xp_needed =
            |level: u8| xp_base.saturating_add(xp_per_level.saturating_mul(u32::from(level)));
        if progress.level >= cap {
            progress.xp = 0;
            let _ = self.world.insert(hero, progress);
            return;
        }
        progress.xp = progress.xp.saturating_add(amount);
        while progress.level < cap {
            let needed = xp_needed(progress.level);
            if progress.xp < needed {
                break;
            }
            progress.xp -= needed;
            progress.level += 1;
            self.level_up(hero);
        }
        if progress.level >= cap {
            progress.xp = 0;
        }
        let _ = self.world.insert(hero, progress);
    }

    fn level_up(&mut self, hero: Entity) {
        let growth = self
            .world
            .get::<HeroIdentity>(hero)
            .and_then(|identity| self.content.heroes.get(usize::from(identity.0.0)))
            .map_or(LEVEL_GROWTH, |hero| moba_game::LevelGrowth {
                health: hero.health_per_level,
                mana: hero.mana_per_level,
                attack_damage: hero.attack_damage_per_level,
                armor: hero.armor_per_level,
                magic_resist: hero.magic_resist_per_level,
            });
        if let Some(health) = self.world.get_mut::<Health>(hero) {
            health.max = health.max.saturating_add(growth.health);
            health.current = (health.current + growth.health).min(health.max);
        }
        if let Some(mana) = self.world.get_mut::<Mana>(hero) {
            mana.max = mana.max.saturating_add(growth.mana);
            mana.current = (mana.current + growth.mana).min(mana.max);
        }
        if let Some(stats) = self.world.get_mut::<AttackStats>(hero) {
            stats.damage = stats.damage.saturating_add(growth.attack_damage);
        }
        if let Some(resistances) = self.world.get_mut::<Resistances>(hero) {
            resistances.armor = resistances.armor.saturating_add(growth.armor);
            resistances.magic_resist = resistances.magic_resist.saturating_add(growth.magic_resist);
        }
    }

    fn unit_rewards(&self, kind: UnitKind) -> (u32, u32) {
        let reward = match kind {
            UnitKind::MeleeMinion => self.content.rules.rewards.melee_minion,
            UnitKind::RangedMinion => self.content.rules.rewards.ranged_minion,
            UnitKind::Tower => self.content.rules.rewards.tower,
            UnitKind::Nexus => self.content.rules.rewards.nexus,
            UnitKind::Hero => self.content.rules.rewards.hero,
        };
        (reward.xp, reward.gold)
    }
}
