//! Per-tick timers, regeneration and status-effect (modifier) bookkeeping.

use moba_core::Fix;
use moba_ecs::{Entity, Without};
use moba_game::{DamageType, ModifierKind};

use crate::components::{
    AttackStats, BaseMoveSpeed, Cooldowns, Dead, Health, ItemCooldowns, MAX_MODIFIERS, Mana,
    Modifier, Modifiers, MoveSpeed, Regen,
};

use super::{RecallCancel, Simulation};

impl Simulation {
    pub(super) fn recompute_move_speed(&mut self, entity: Entity) {
        let base = self
            .world
            .get::<BaseMoveSpeed>(entity)
            .map(|base| base.0)
            .or_else(|| self.world.get::<MoveSpeed>(entity).map(|speed| speed.0));
        let Some(base) = base else {
            return;
        };
        let factor = self
            .world
            .get::<Modifiers>(entity)
            .map_or(Fix::ONE, Modifiers::move_factor);
        if let Some(speed) = self.world.get_mut::<MoveSpeed>(entity) {
            speed.0 = base * factor;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_modifier(
        &mut self,
        entity: Entity,
        def_id: u16,
        remaining: u32,
        factor: Fix,
        amount: i32,
        source: Entity,
        damage_type: DamageType,
    ) {
        let Some(kind) = self
            .content
            .modifiers
            .get(usize::from(def_id))
            .map(|definition| definition.kind)
        else {
            return;
        };
        let Some(modifiers) = self.world.get_mut::<Modifiers>(entity) else {
            return;
        };
        let mut existing = None;
        for index in 0..usize::from(modifiers.len) {
            if modifiers.entries[index].kind == kind {
                existing = Some(index);
                break;
            }
        }
        if let Some(index) = existing {
            let modifier = &mut modifiers.entries[index];
            modifier.def_id = def_id;
            modifier.remaining = modifier.remaining.max(remaining);
            match kind {
                ModifierKind::Shield => {
                    modifier.amount = modifier.amount.saturating_add(amount);
                }
                ModifierKind::Slow => {
                    modifier.factor = modifier.factor.min(factor);
                }
                ModifierKind::Haste => {
                    modifier.factor = modifier.factor.max(factor);
                }
                ModifierKind::Burn => {
                    modifier.amount = amount;
                    modifier.damage_type = damage_type;
                    modifier.source = source;
                }
                ModifierKind::Root | ModifierKind::Silence | ModifierKind::Stun => {}
            }
        } else if usize::from(modifiers.len) < MAX_MODIFIERS {
            let index = usize::from(modifiers.len);
            modifiers.entries[index] = Modifier {
                def_id,
                kind,
                remaining,
                factor,
                amount,
                damage_type,
                source,
            };
            modifiers.len += 1;
        } else {
            return;
        }
        if matches!(kind, ModifierKind::Slow | ModifierKind::Haste) {
            self.recompute_move_speed(entity);
        }
        if kind == ModifierKind::Stun {
            // Hard crowd control interrupts a recall.
            self.cancel_recall(entity, RecallCancel::Stun);
        }
    }

    #[allow(clippy::too_many_lines)] // One ordered status/timer pass.
    pub(super) fn tick_timers(&mut self) {
        for (_, cooldowns) in self.world.query::<&mut Cooldowns>() {
            for cooldown in &mut cooldowns.0 {
                *cooldown = cooldown.saturating_sub(1);
            }
        }
        for (_, stats) in self.world.query::<&mut AttackStats>() {
            stats.cooldown = stats.cooldown.saturating_sub(1);
        }
        for (_, cooldowns) in self.world.query::<&mut ItemCooldowns>() {
            for cooldown in &mut cooldowns.0 {
                *cooldown = cooldown.saturating_sub(1);
            }
        }
        for (_, (health, mana, regen)) in self
            .world
            .query_filtered::<(&mut Health, &mut Mana, &Regen), Without<Dead>>()
        {
            // A hero that reached zero this tick is already dead; regeneration must not
            // revive it before the death pass runs.
            if health.current > 0 {
                health.current = (health.current + regen.health).min(health.max);
                mana.current = (mana.current + regen.mana).min(mana.max);
            }
        }

        self.modifier_entities.clear();
        for (entity, _) in self.world.query::<&Modifiers>() {
            self.modifier_entities.push(entity);
        }
        let mut burn_hits = core::mem::take(&mut self.burn_hits);
        burn_hits.clear();
        for index in 0..self.modifier_entities.len() {
            let entity = self.modifier_entities[index];
            let Some(modifiers) = self.world.get_mut::<Modifiers>(entity) else {
                continue;
            };
            let mut position = 0;
            let mut recompute = false;
            while position < usize::from(modifiers.len) {
                let modifier = &mut modifiers.entries[position];
                modifier.remaining = modifier.remaining.saturating_sub(1);
                if modifier.kind == ModifierKind::Burn {
                    burn_hits.push((
                        entity,
                        modifier.amount,
                        modifier.damage_type,
                        modifier.source,
                    ));
                } else if let Some(definition) =
                    self.content.modifiers.get(usize::from(modifier.def_id))
                    && definition.on_tick_damage > 0
                {
                    burn_hits.push((
                        entity,
                        definition.on_tick_damage,
                        DamageType::Magic,
                        modifier.source,
                    ));
                }
                let expired = modifier.remaining == 0
                    || (modifier.kind == ModifierKind::Shield && modifier.amount <= 0);
                if expired {
                    if let Some(definition) =
                        self.content.modifiers.get(usize::from(modifier.def_id))
                        && definition.on_expire_damage > 0
                    {
                        burn_hits.push((
                            entity,
                            definition.on_expire_damage,
                            DamageType::Magic,
                            modifier.source,
                        ));
                    }
                    recompute |= matches!(modifier.kind, ModifierKind::Slow | ModifierKind::Haste);
                    modifiers
                        .entries
                        .swap(position, usize::from(modifiers.len) - 1);
                    modifiers.len -= 1;
                } else {
                    position += 1;
                }
            }
            if recompute {
                self.recompute_move_speed(entity);
            }
        }
        for (target, damage, damage_type, source) in burn_hits.drain(..) {
            self.damage(target, damage, damage_type, source);
        }
        self.burn_hits = burn_hits;
        self.respawns.clear();
        for (entity, dead) in self.world.query::<&mut Dead>() {
            dead.respawn_in = dead.respawn_in.saturating_sub(1);
            if dead.respawn_in == 0 {
                self.respawns.push(entity);
            }
        }
        let mut respawns = core::mem::take(&mut self.respawns);
        for entity in respawns.drain(..) {
            self.respawn_hero(entity);
        }
        self.respawns = respawns;
    }
}
