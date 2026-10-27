//! Deterministic built-in bot AI and its fog-aware target selection.

use moba_core::{Fix, FixVec2};
use moba_ecs::{Entity, Without};
use moba_game::Team;

use crate::components::{
    AttackStats, Cooldowns, Dead, Health, HeroIdentity, HeroSlot, ItemCooldowns, Mana, Position,
    Progression, UnitKind, VisionRange,
};

use super::vision::segments_intersect;
use super::{
    BotPolicy, HeroView, MAX_PLAYERS, MAX_UNITS, PlayerCommand, Simulation, TargetRef, TickInputs,
};

impl Simulation {
    /// Installs a custom bot policy (or restores the default with `None`).
    pub fn set_bot_policy(&mut self, policy: Option<Box<dyn BotPolicy>>) {
        self.bot_policy = policy;
    }

    /// Deterministic bot AI. Returns one command per bot-controlled slot.
    #[must_use]
    pub fn bot_inputs(&mut self) -> TickInputs {
        if let Some(mut policy) = self.bot_policy.take() {
            let inputs = policy.inputs(self);
            self.bot_policy = Some(policy);
            inputs
        } else {
            self.default_bot_inputs()
        }
    }

    #[must_use]
    #[allow(clippy::too_many_lines)] // Explicit priority ladder is easier to audit.
    pub(super) fn default_bot_inputs(&mut self) -> TickInputs {
        self.refresh_spatial();
        let visibility = self.bot_visibility_masks();
        self.hero_views.clear();
        for (_entity, (slot, team, position, health, mana, cooldowns, identity, stats)) in
            self.world.query_filtered::<(
                &HeroSlot,
                &Team,
                &Position,
                &Health,
                &Mana,
                &Cooldowns,
                &HeroIdentity,
                &AttackStats,
            ), Without<Dead>>()
        {
            self.hero_views.push(HeroView {
                slot: slot.0,
                team: *team,
                position: position.0,
                health: health.current,
                mana: mana.current,
                cooldowns: *cooldowns,
                attack_range_sq: stats.range_sq,
                max_health: self.content.heroes[usize::from(identity.0.0)].max_health,
                ability_mana: {
                    let mut mana = [0; 3];
                    for (slot, id) in self.content.heroes[usize::from(identity.0.0)]
                        .abilities
                        .iter()
                        .enumerate()
                    {
                        mana[slot] = self
                            .content
                            .abilities
                            .get(usize::from(*id))
                            .map_or(0, |ability| ability.mana_cost);
                    }
                    mana
                },
                ability_range: {
                    let mut range = [Fix::ZERO; 3];
                    for (slot, id) in self.content.heroes[usize::from(identity.0.0)]
                        .abilities
                        .iter()
                        .enumerate()
                    {
                        range[slot] = self
                            .content
                            .abilities
                            .get(usize::from(*id))
                            .map_or(Fix::ZERO, moba_game::AbilitySpec::cast_range);
                    }
                    range
                },
            });
        }

        let mut inputs = TickInputs::new();
        let view_count = self.hero_views.len();
        for index in 0..view_count {
            let view = self.hero_views[index];
            let slot = usize::from(view.slot);
            let (cast_roll, jitter_x, jitter_y, buy_roll) = {
                let rng = &mut self.bot_rng[slot % MAX_PLAYERS];
                (
                    rng.gen_range_u32(100),
                    i32::try_from(rng.gen_range_u32(5)).unwrap_or(0) - 2,
                    i32::try_from(rng.gen_range_u32(5)).unwrap_or(0) - 2,
                    rng.gen_range_u32(100),
                )
            };
            if self.config.bot_mask & (1 << slot) == 0 {
                continue;
            }
            if let Some(hero) = self.hero_entities.get(slot).copied() {
                if let Some(progress) = self.world.get::<Progression>(hero).copied()
                    && let Some(cooldowns) = self.world.get::<ItemCooldowns>(hero).copied()
                    && view.health * 2 <= view.max_health
                {
                    let mut used = false;
                    for item_slot in 0..usize::from(progress.item_count) {
                        let item_id = progress.items[item_slot];
                        if cooldowns.0[item_slot] == 0
                            && self
                                .content
                                .items
                                .get(usize::from(item_id))
                                .is_some_and(|item| {
                                    item.active_heal > 0 && item.active_cooldown_ticks > 0
                                })
                        {
                            inputs.set(
                                slot,
                                PlayerCommand::UseItem(u8::try_from(item_slot).unwrap_or(0)),
                            );
                            used = true;
                            break;
                        }
                    }
                    if used {
                        continue;
                    }
                }
                let near_shop = self.world.get::<Position>(hero).is_some_and(|position| {
                    position
                        .0
                        .distance_sq(self.content.map.nexus_position(view.team))
                        <= self.content.rules.shop_radius * self.content.rules.shop_radius
                });
                if near_shop
                    && let Some(progress) = self.world.get::<Progression>(hero)
                    && usize::from(progress.item_count) < moba_game::ITEM_SLOTS
                {
                    let affordable: Vec<u8> = self
                        .content
                        .items
                        .iter()
                        .enumerate()
                        .filter(|(_, item)| item.cost <= progress.gold)
                        .map(|(id, _)| u8::try_from(id).unwrap_or(0))
                        .collect();
                    if !affordable.is_empty() {
                        let pick = usize::try_from(buy_roll).unwrap_or(0) % affordable.len();
                        inputs.set(slot, PlayerCommand::BuyItem(affordable[pick]));
                        continue;
                    }
                }
            }
            let enemy_nexus = self.content.map.nexus_position(view.team.enemy());
            let hero_target = self.nearest_enemy(
                view.position,
                view.team,
                Fix::from_i32(13),
                &visibility,
                UnitKind::is_hero,
            );
            let minion_target = self.nearest_enemy(
                view.position,
                view.team,
                Fix::from_i32(10),
                &visibility,
                UnitKind::is_minion,
            );
            let structure_target = self.nearest_enemy(
                view.position,
                view.team,
                Fix::from_i32(200),
                &visibility,
                UnitKind::is_structure,
            );
            let target = hero_target.or(minion_target).or(structure_target);

            let command = if let Some(target) = target {
                let distance_sq = view.position.distance_sq(target.position);
                let defensive = i64::from(view.health) * 100 <= i64::from(view.max_health) * 45;
                if defensive && view.cooldowns.0[1] == 0 && cast_roll < 80 {
                    if view.mana >= view.ability_mana[1] {
                        Some(PlayerCommand::CastAbility {
                            slot: 1,
                            target: view.position,
                        })
                    } else {
                        None
                    }
                } else if view.cooldowns.0[0] == 0
                    && view.mana >= view.ability_mana[0]
                    && distance_sq <= view.ability_range[0] * view.ability_range[0]
                    && cast_roll < 85
                {
                    Some(PlayerCommand::CastAbility {
                        slot: 0,
                        target: target.position
                            + FixVec2::new(Fix::from_i32(jitter_x), Fix::from_i32(jitter_y)),
                    })
                } else if distance_sq > view.attack_range_sq
                    && distance_sq <= Fix::from_i32(12) * Fix::from_i32(12)
                    && view.cooldowns.0[2] == 0
                    && view.mana >= view.ability_mana[2]
                    && distance_sq <= view.ability_range[2] * view.ability_range[2]
                    && cast_roll < 75
                {
                    Some(PlayerCommand::CastAbility {
                        slot: 2,
                        target: target.position
                            + FixVec2::new(Fix::from_i32(jitter_x), Fix::from_i32(jitter_y)),
                    })
                } else if distance_sq <= view.attack_range_sq {
                    Some(PlayerCommand::Attack(target.entity))
                } else {
                    Some(PlayerCommand::MoveTo(target.position))
                }
            } else {
                Some(PlayerCommand::MoveTo(enemy_nexus))
            };
            inputs.set(slot, command.unwrap_or(PlayerCommand::Stop));
        }
        inputs
    }

    pub(super) fn bot_visibility_masks(&mut self) -> [u64; 16] {
        let mut sources = core::mem::take(&mut self.bot_sources);
        sources.clear();
        sources.resize(MAX_UNITS * 2, (FixVec2::ZERO, Fix::ZERO));
        let mut source_count = [0usize; 2];
        for (_entity, (team, health, position, vision)) in
            self.world
                .query::<(&Team, &Health, &Position, &VisionRange)>()
        {
            if health.current <= 0 {
                continue;
            }
            let index = team.index();
            // Store both teams in one fixed array, offset by team, so both fog masks are
            // built from a single world query and candidate pass.
            if source_count[index] < MAX_UNITS {
                let slot = index * MAX_UNITS + source_count[index];
                sources[slot] = (position.0, vision.0);
                source_count[index] += 1;
            }
        }
        let mut team_masks = [0u64; 16];
        for (entity, (team, kind, position, health)) in
            self.world.query::<(&Team, &UnitKind, &Position, &Health)>()
        {
            let slot = entity.index() as usize;
            if slot >= MAX_UNITS {
                continue;
            }
            let index = team.index();
            let allied = |viewer: usize| viewer == index;
            let visible_from =
                |viewer: usize| {
                    let start = viewer * MAX_UNITS;
                    let count = source_count[viewer];
                    sources[start..start + count]
                        .iter()
                        .any(|&(source, range)| {
                            source.distance_sq(position.0) <= range * range
                                && self.content.map.walls.iter().all(|wall| {
                                    !segments_intersect(source, position.0, wall.a, wall.b)
                                })
                        })
                };
            let dead = health.current <= 0;
            for viewer in 0..2 {
                if allied(viewer) || kind.is_structure() || (!dead && visible_from(viewer)) {
                    team_masks[viewer * 8 + slot / 64] |= 1 << (slot % 64);
                }
            }
        }
        self.bot_sources = sources;
        team_masks
    }

    pub(super) fn visible_to(masks: &[u64; 16], team: Team, entity: Entity) -> bool {
        let slot = entity.index() as usize;
        slot < MAX_UNITS && masks[team.index() * 8 + slot / 64] & (1 << (slot % 64)) != 0
    }

    fn nearest_enemy(
        &self,
        position: FixVec2,
        team: Team,
        radius: Fix,
        masks: &[u64; 16],
        filter: impl Fn(UnitKind) -> bool,
    ) -> Option<TargetRef> {
        let mut best: Option<(Fix, TargetRef)> = None;
        self.grid.for_each_in_circle(position, radius, |candidate| {
            if candidate.team == team
                || !filter(candidate.kind)
                || !Self::visible_to(masks, team, candidate.entity)
            {
                return;
            }
            let distance = candidate.position.distance_sq(position);
            if best.as_ref().is_none_or(|(best_distance, best_ref)| {
                distance < *best_distance
                    || (distance == *best_distance && candidate.entity < best_ref.entity)
            }) {
                best = Some((distance, candidate));
            }
        });
        best.map(|(_, target)| target)
    }
}
