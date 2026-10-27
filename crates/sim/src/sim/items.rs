//! Shop purchases and active item use.

use moba_ecs::Entity;
use moba_game::Team;

use crate::components::{
    AttackStats, BaseMoveSpeed, Dead, Health, ItemCooldowns, Mana, MoveSpeed, Position,
    Progression, Resistances,
};

use super::{ONE_TICK, Simulation};

impl Simulation {
    /// Purchases an item for `slot`'s hero. Used by the network server for shop inputs,
    /// which are applied immediately rather than queued as movement orders.
    ///
    /// Returns whether the purchase succeeded (near own nexus, enough gold, free slot).
    pub fn purchase(&mut self, slot: usize, item_id: u8) -> bool {
        let Some(&hero) = self.hero_entities.get(slot) else {
            return false;
        };
        if self.world.has::<Dead>(hero) {
            return false;
        }
        self.buy_item(hero, item_id)
    }

    pub(super) fn use_item(&mut self, hero: Entity, slot: u8) {
        if self.world.has::<Dead>(hero) {
            return;
        }
        let Some(progress) = self.world.get::<Progression>(hero).copied() else {
            return;
        };
        if slot >= progress.item_count {
            return;
        }
        let item_id = progress.items[usize::from(slot)];
        let Some(item) = self.content.items.get(usize::from(item_id)) else {
            return;
        };
        if item.active_heal <= 0 || item.active_cooldown_ticks == 0 {
            return;
        }
        let Some(mut cooldowns) = self.world.get::<ItemCooldowns>(hero).copied() else {
            return;
        };
        if cooldowns.0[usize::from(slot)] != 0 {
            return;
        }
        if let Some(health) = self.world.get_mut::<Health>(hero) {
            health.current = (health.current + item.active_heal).min(health.max);
        }
        cooldowns.0[usize::from(slot)] = item.active_cooldown_ticks;
        let _ = self.world.insert(hero, cooldowns);
    }

    #[allow(clippy::too_many_lines)] // Flat stat application is clearer than a trait tower.
    pub(super) fn buy_item(&mut self, hero: Entity, item_id: u8) -> bool {
        let Some(item) = self.content.items.get(usize::from(item_id)) else {
            return false;
        };
        let cost = item.cost;
        let item_health = item.health;
        let item_mana = item.mana;
        let item_attack_damage = item.attack_damage;
        let item_armor = item.armor;
        let item_magic_resist = item.magic_resist;
        let item_move_speed = item.move_speed_per_sec;
        let Some(team) = self.world.get::<Team>(hero).copied() else {
            return false;
        };
        let Some(position) = self.world.get::<Position>(hero).map(|p| p.0) else {
            return false;
        };
        if position.distance_sq(self.content.map.nexus_position(team))
            > self.content.rules.shop_radius * self.content.rules.shop_radius
        {
            return false;
        }
        let Some(mut progress) = self.world.get::<Progression>(hero).copied() else {
            return false;
        };
        if progress.gold < cost || usize::from(progress.item_count) >= moba_game::ITEM_SLOTS {
            return false;
        }
        progress.gold -= cost;
        progress.items[usize::from(progress.item_count)] = item_id;
        progress.item_count += 1;
        let _ = self.world.insert(hero, progress);
        if let Some(health) = self.world.get_mut::<Health>(hero) {
            health.max = health.max.saturating_add(item_health);
            health.current = health.current.saturating_add(item_health);
        }
        if let Some(mana) = self.world.get_mut::<Mana>(hero) {
            mana.max = mana.max.saturating_add(item_mana);
            mana.current = mana.current.saturating_add(item_mana);
        }
        if let Some(stats) = self.world.get_mut::<AttackStats>(hero) {
            stats.damage = stats.damage.saturating_add(item_attack_damage);
        }
        if let Some(resistances) = self.world.get_mut::<Resistances>(hero) {
            resistances.armor = resistances.armor.saturating_add(item_armor);
            resistances.magic_resist = resistances.magic_resist.saturating_add(item_magic_resist);
        }
        let bonus = item_move_speed * ONE_TICK;
        if let Some(base) = self.world.get_mut::<BaseMoveSpeed>(hero) {
            base.0 += bonus;
        } else if let Some(speed) = self.world.get_mut::<MoveSpeed>(hero) {
            speed.0 += bonus;
        }
        self.recompute_move_speed(hero);
        true
    }
}
