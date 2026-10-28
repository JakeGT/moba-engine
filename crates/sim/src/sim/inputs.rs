//! Player command resolution for one tick.

use moba_game::{ModifierKind, Team};

use crate::components::{AttackTarget, Dead, Modifiers, MoveTarget};

use super::{MAX_PLAYERS, PlayerCommand, Simulation, TickInputs};

impl Simulation {
    /// Deterministic scheduling phase: half the seeds resolve blue first, half red first.
    pub(super) fn blue_first(&self) -> bool {
        (self.tick.get().wrapping_add((self.config.seed & 1) as u32)).is_multiple_of(2)
    }

    pub(super) fn apply_inputs(&mut self, inputs: &TickInputs) {
        // Alternate command resolution order so neither team systematically resolves casts,
        // purchases or attack orders first.
        let order: [usize; MAX_PLAYERS] = if self.blue_first() {
            core::array::from_fn(|index| index)
        } else {
            core::array::from_fn(|index| MAX_PLAYERS - 1 - index)
        };
        for slot in order {
            let entity = self.hero_entities[slot];
            if self.world.has::<Dead>(entity) {
                continue;
            }
            if let Some(item) = inputs.purchases[slot] {
                self.buy_item(entity, item);
            }
            let Some(command) = inputs.players[slot] else {
                continue;
            };
            match command {
                PlayerCommand::Stop => {
                    if let Some(target) = self.world.get_mut::<MoveTarget>(entity) {
                        target.0 = None;
                    }
                    if let Some(target) = self.world.get_mut::<AttackTarget>(entity) {
                        target.0 = None;
                    }
                }
                PlayerCommand::MoveTo(position) => {
                    if self
                        .world
                        .get::<Modifiers>(entity)
                        .is_none_or(|m| !m.has(ModifierKind::Root) && !m.has(ModifierKind::Stun))
                    {
                        let destination = self.clamp_position(position);
                        if let Some(target) = self.world.get_mut::<MoveTarget>(entity) {
                            target.0 = Some(destination);
                        }
                        if let Some(target) = self.world.get_mut::<AttackTarget>(entity) {
                            target.0 = None;
                        }
                    }
                }
                PlayerCommand::Attack(target) => {
                    if let Some(order) = self.world.get_mut::<AttackTarget>(entity) {
                        order.0 = Some(target);
                    }
                    if let Some(order) = self.world.get_mut::<MoveTarget>(entity) {
                        order.0 = None;
                    }
                }
                PlayerCommand::CastAbility {
                    slot: ability,
                    target,
                } => {
                    self.cast_ability(entity, ability, target);
                }
                PlayerCommand::BuyItem(item) => {
                    self.buy_item(entity, item);
                }
                PlayerCommand::UseItem(slot) => {
                    self.use_item(entity, slot);
                }
                PlayerCommand::Surrender => {
                    if let Some(team) = self.world.get::<Team>(entity).copied() {
                        self.queue_win(team.enemy());
                    }
                }
            }
        }
    }
}
