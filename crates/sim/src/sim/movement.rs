//! Minion/structure target selection and unit movement.

use moba_core::Fix;
use moba_ecs::{Entity, With, Without};
use moba_game::Team;

use crate::components::{
    AttackStats, AttackTarget, Dead, LaneWalker, MoveSpeed, MoveTarget, Position, UnitKind,
};

use super::{Simulation, clamp_to_extent};

const MINION_AGGRO: Fix = Fix::from_i32(6);
const MINION_AGGRO_SQ: Fix = Fix::from_bits(
    MINION_AGGRO
        .to_bits()
        .saturating_mul(MINION_AGGRO.to_bits()),
);
const WAYPOINT_REACH: Fix = Fix::from_i32(3);

impl Simulation {
    pub(super) fn update_minion_targets(&mut self) {
        let grid = &self.grid;
        let map = &self.content.map;
        for (_, (position, team, stats, attack_target, move_target, walker)) in
            self.world.query_filtered::<(
                &Position,
                &Team,
                &AttackStats,
                &mut AttackTarget,
                &mut MoveTarget,
                &mut LaneWalker,
            ), (With<LaneWalker>, Without<Dead>)>()
        {
            let position = position.0;
            let team = *team;
            let mut best_unit: Option<(Fix, Entity)> = None;
            let mut best_structure: Option<(Fix, Entity)> = None;
            let search_radius = stats.range.max(MINION_AGGRO);
            grid.for_each_in_circle(position, search_radius, |candidate| {
                if candidate.team == team {
                    return;
                }
                let distance = candidate.position.distance_sq(position);
                if candidate.kind.is_structure() {
                    if distance <= stats.range_sq
                        && best_structure.is_none_or(|(best, entity)| {
                            distance < best || (distance == best && candidate.entity < entity)
                        })
                    {
                        best_structure = Some((distance, candidate.entity));
                    }
                } else if distance <= MINION_AGGRO_SQ
                    && best_unit.is_none_or(|(best, entity)| {
                        distance < best || (distance == best && candidate.entity < entity)
                    })
                {
                    best_unit = Some((distance, candidate.entity));
                }
            });

            if let Some((_, target)) = best_unit {
                attack_target.0 = Some(target);
                move_target.0 = None;
                continue;
            }
            if let Some((_, target)) = best_structure {
                attack_target.0 = Some(target);
                move_target.0 = None;
                continue;
            }

            attack_target.0 = None;
            let lane = &map.lanes[usize::from(walker.lane).min(map.lanes.len() - 1)];
            let mut waypoint = usize::from(walker.waypoint).min(lane.len() - 1);
            while waypoint < lane.len() - 1
                && position.distance(lane.point(team, waypoint)) <= WAYPOINT_REACH
            {
                waypoint += 1;
            }
            walker.waypoint = u16::try_from(waypoint).expect("waypoint fits u16");
            let destination = if waypoint >= lane.len() - 1 {
                map.nexus_position(team.enemy())
            } else {
                lane.point(team, waypoint)
            };
            move_target.0 = Some(destination);
        }
    }

    pub(super) fn update_structure_targets(&mut self) {
        let grid = &self.grid;
        for (_, (position, team, stats, kind, attack_target)) in
            self.world
                .query::<(&Position, &Team, &AttackStats, &UnitKind, &mut AttackTarget)>()
        {
            if !kind.is_structure() {
                continue;
            }
            let position = position.0;
            let team = *team;
            let mut best_minion: Option<(Fix, Entity)> = None;
            let mut best_hero: Option<(Fix, Entity)> = None;
            grid.for_each_in_circle(position, stats.range, |candidate| {
                if candidate.team == team {
                    return;
                }
                let distance = candidate.position.distance_sq(position);
                if distance > stats.range_sq {
                    return;
                }
                if candidate.kind.is_minion() {
                    if best_minion.is_none_or(|(best, entity)| {
                        distance < best || (distance == best && candidate.entity < entity)
                    }) {
                        best_minion = Some((distance, candidate.entity));
                    }
                } else if candidate.kind.is_hero()
                    && best_hero.is_none_or(|(best, entity)| {
                        distance < best || (distance == best && candidate.entity < entity)
                    })
                {
                    best_hero = Some((distance, candidate.entity));
                }
            });
            attack_target.0 = if self.content.rules.structure_heroes_first {
                best_hero.or(best_minion)
            } else {
                best_minion.or(best_hero)
            }
            .map(|(_, entity)| entity);
        }
    }

    pub(super) fn move_units(&mut self) {
        let extent = self.content.map.extent;
        for (_, (position, target, speed)) in self
            .world
            .query_filtered::<(&mut Position, &MoveTarget, &MoveSpeed), Without<Dead>>()
        {
            let Some(destination) = target.0 else {
                continue;
            };
            let next = position.0.move_towards(destination, speed.0);
            position.0 = clamp_to_extent(next, extent);
        }
    }
}
