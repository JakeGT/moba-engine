//! Recall: a channel that teleports a hero to its team's spawn (see [`Simulation`]'s
//! "Recall" section for the rules).

use moba_ecs::Entity;
use moba_game::{ModifierKind, Team};

use crate::components::{AttackTarget, Dead, HeroSlot, Modifiers, MoveTarget, Position, Recall};

use super::{RecallCancel, RecallEnd, SimEvent, Simulation};

impl Simulation {
    /// Ticks left in the recall channel of `slot`'s hero: `Some(0)` when it is not
    /// recalling, `None` for a slot without a hero.
    #[must_use]
    pub fn recall_remaining(&self, slot: usize) -> Option<u32> {
        self.hero_entity(slot)
            .and_then(|hero| self.world.get::<Recall>(hero))
            .map(|recall| recall.remaining)
    }

    /// Starts `hero`'s recall channel ([`super::PlayerCommand::Recall`]).
    pub(super) fn start_recall(&mut self, hero: Entity) {
        let duration = self.content.rules.recall_ticks;
        if duration == 0 || self.world.has::<Dead>(hero) {
            return;
        }
        let Some(recall) = self.world.get::<Recall>(hero).copied() else {
            return;
        };
        // Recalling again while channelling has no effect: it neither restarts the channel
        // nor cancels it.
        if recall.active() {
            return;
        }
        if self
            .world
            .get::<Modifiers>(hero)
            .is_some_and(|modifiers| modifiers.has(ModifierKind::Stun))
        {
            return;
        }
        let (Some(position), Some(team)) = (
            self.world.get::<Position>(hero).map(|position| position.0),
            self.world.get::<Team>(hero).copied(),
        ) else {
            return;
        };
        if let Some(recall) = self.world.get_mut::<Recall>(hero) {
            recall.remaining = duration;
        }
        // The hero stands still while it channels.
        if let Some(target) = self.world.get_mut::<MoveTarget>(hero) {
            target.0 = None;
        }
        if let Some(target) = self.world.get_mut::<AttackTarget>(hero) {
            target.0 = None;
        }
        self.push_event(SimEvent::RecallStarted {
            hero,
            team,
            position,
            duration_ticks: duration,
        });
    }

    /// Cancels `entity`'s recall channel, if it is channelling one.
    pub(super) fn cancel_recall(&mut self, entity: Entity, cause: RecallCancel) {
        let Some(recall) = self.world.get_mut::<Recall>(entity) else {
            return;
        };
        if !recall.active() {
            return;
        }
        recall.remaining = 0;
        let (Some(position), Some(team)) = (
            self.world
                .get::<Position>(entity)
                .map(|position| position.0),
            self.world.get::<Team>(entity).copied(),
        ) else {
            return;
        };
        self.push_event(SimEvent::RecallEnded {
            hero: entity,
            team,
            position,
            end: RecallEnd::Cancelled(cause),
        });
    }

    /// Advances every channel by one tick and teleports the heroes whose channel ran out.
    /// Runs after deaths, so damage or a death this tick has already cancelled it.
    pub(super) fn tick_recalls(&mut self) {
        for hero in self.hero_entities {
            let Some(recall) = self.world.get_mut::<Recall>(hero) else {
                continue;
            };
            if !recall.active() {
                continue;
            }
            recall.remaining -= 1;
            if recall.remaining > 0 {
                continue;
            }
            let (Some(position), Some(team), Some(slot)) = (
                self.world.get::<Position>(hero).map(|position| position.0),
                self.world.get::<Team>(hero).copied(),
                self.world.get::<HeroSlot>(hero).map(|slot| slot.0),
            ) else {
                continue;
            };
            let destination = self
                .content
                .map
                .hero_spawn(team, slot % self.content.rules.team_size.max(1));
            if let Some(current) = self.world.get_mut::<Position>(hero) {
                current.0 = destination;
            }
            if let Some(target) = self.world.get_mut::<MoveTarget>(hero) {
                target.0 = None;
            }
            if let Some(target) = self.world.get_mut::<AttackTarget>(hero) {
                target.0 = None;
            }
            self.push_event(SimEvent::RecallEnded {
                hero,
                team,
                position,
                end: RecallEnd::Completed { destination },
            });
        }
    }
}
