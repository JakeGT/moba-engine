//! Canonical catalogue feed, diagnostic FNV hash and publisher signing message.
//!
//! The byte order fed by `feed_catalogue` is a compatibility contract: replays record the
//! content hash and attestations sign a SHA-256 digest of the same feed. Any change here
//! changes every pack hash; attestation fields are deliberately excluded.

use sha2::{Digest, Sha256};

use crate::{AreaEffect, AreaEffectList, Effect, EffectList, Targeting};

use super::{GameContent, RewardsSpec, RulesSpec, UnitRewardsSpec};

fn hash_area_effects(feed: &mut dyn FnMut(&[u8]), effects: &AreaEffectList) {
    feed(&[u8::try_from(effects.iter().count()).unwrap_or(u8::MAX)]);
    for effect in effects.iter() {
        match effect {
            AreaEffect::None => {}
            AreaEffect::Damage {
                amount,
                damage_type,
            } => {
                feed(&[0]);
                feed(&amount.to_le_bytes());
                feed(&[*damage_type as u8]);
            }
            AreaEffect::ApplyModifier {
                modifier,
                duration_ticks,
                factor,
                amount,
                damage_type,
            } => {
                feed(&[1]);
                feed(&modifier.to_le_bytes());
                feed(&duration_ticks.to_le_bytes());
                feed(&factor.to_bits().to_le_bytes());
                feed(&amount.to_le_bytes());
                feed(&[*damage_type as u8]);
            }
        }
    }
}

fn hash_effects(feed: &mut dyn FnMut(&[u8]), effects: &EffectList) {
    feed(&[u8::try_from(effects.iter().count()).unwrap_or(u8::MAX)]);
    for effect in effects.iter() {
        match effect {
            Effect::None => {}
            Effect::Damage {
                amount,
                damage_type,
            } => {
                feed(&[0]);
                feed(&amount.to_le_bytes());
                feed(&[*damage_type as u8]);
            }
            Effect::Heal { amount } => {
                feed(&[1]);
                feed(&amount.to_le_bytes());
            }
            Effect::ApplyModifier {
                modifier,
                duration_ticks,
                factor,
                amount,
                damage_type,
            } => {
                feed(&[2]);
                feed(&modifier.to_le_bytes());
                feed(&duration_ticks.to_le_bytes());
                feed(&factor.to_bits().to_le_bytes());
                feed(&amount.to_le_bytes());
                feed(&[*damage_type as u8]);
            }
            Effect::Displace { range } => {
                feed(&[3]);
                feed(&range.to_bits().to_le_bytes());
            }
            Effect::Projectile {
                speed_per_sec,
                range,
                radius,
                arc,
                on_hit,
            } => {
                feed(&[4]);
                feed(&speed_per_sec.to_bits().to_le_bytes());
                feed(&range.to_bits().to_le_bytes());
                feed(&radius.to_bits().to_le_bytes());
                feed(&arc.to_bits().to_le_bytes());
                hash_area_effects(&mut *feed, on_hit);
            }
            Effect::Area { radius, effects } => {
                feed(&[5]);
                feed(&radius.to_bits().to_le_bytes());
                hash_area_effects(&mut *feed, effects);
            }
            Effect::Zone {
                radius,
                duration_ticks,
                period_ticks,
                effects,
            } => {
                feed(&[6]);
                feed(&radius.to_bits().to_le_bytes());
                feed(&duration_ticks.to_le_bytes());
                feed(&period_ticks.to_le_bytes());
                hash_area_effects(&mut *feed, effects);
            }
        }
    }
}

impl GameContent {
    /// Stable bytes signed by a publisher attestation.
    ///
    /// Version 2 signs a SHA-256 digest of the canonical catalogue feed rather than the
    /// diagnostic 64-bit FNV hash.
    #[must_use]
    pub fn signing_message(&self) -> Vec<u8> {
        let mut canonical = Vec::new();
        self.feed_catalogue(&mut |bytes| canonical.extend_from_slice(bytes));
        let digest = Sha256::digest(&canonical);
        let mut message = Vec::with_capacity(128);
        message.extend_from_slice(b"moba-pack-v2\n");
        for field in [
            &self.manifest.name,
            &self.manifest.version,
            &self.manifest.author,
        ] {
            message.extend_from_slice(field.as_bytes());
            message.push(b'\n');
        }
        message.extend_from_slice(&digest);
        message
    }

    /// Canonical, unambiguous catalogue feed used by the diagnostic hash and signatures.
    #[allow(clippy::too_many_lines)] // Canonical field feed mirrors the content schema.
    fn feed_catalogue(&self, feed: &mut dyn FnMut(&[u8])) {
        fn len(feed: &mut dyn FnMut(&[u8]), value: usize) {
            feed(&u32::try_from(value).unwrap_or(u32::MAX).to_le_bytes());
        }
        fn string(feed: &mut dyn FnMut(&[u8]), value: &str) {
            len(feed, value.len());
            feed(value.as_bytes());
        }
        len(feed, 3);
        for field in [
            &self.manifest.name,
            &self.manifest.version,
            &self.manifest.author,
        ] {
            string(feed, field);
        }
        len(feed, self.items.len());
        for item in &self.items {
            string(feed, &item.name);
            for value in [
                item.cost,
                item.health as u32,
                item.mana as u32,
                item.attack_damage as u32,
                item.armor as u32,
                item.magic_resist as u32,
                u32::from(item.on_hit_damage),
                u32::from(item.on_damaged_damage),
                item.active_heal as u32,
                u32::from(item.active_cooldown_ticks),
            ] {
                feed(&value.to_le_bytes());
            }
            feed(&item.move_speed_per_sec.to_bits().to_le_bytes());
        }
        len(feed, self.abilities.len());
        for ability in &self.abilities {
            string(feed, &ability.name);
            feed(&ability.cooldown_ticks.to_le_bytes());
            feed(&(ability.mana_cost as u32).to_le_bytes());
            match ability.targeting {
                Targeting::SelfCast => feed(&[0]),
                Targeting::Point { range } => {
                    feed(&[1]);
                    feed(&range.to_bits().to_le_bytes());
                }
                Targeting::Direction { range } => {
                    feed(&[2]);
                    feed(&range.to_bits().to_le_bytes());
                }
            }
            hash_effects(&mut *feed, &ability.effects);
        }
        len(feed, self.heroes.len());
        for hero in &self.heroes {
            string(feed, &hero.name);
            for value in [
                hero.max_health as u32,
                hero.max_mana as u32,
                hero.health_regen_per_tick as u32,
                hero.mana_regen_per_tick as u32,
                hero.attack_damage as u32,
                hero.attack_interval_ticks,
                hero.armor as u32,
                hero.magic_resist as u32,
            ] {
                feed(&value.to_le_bytes());
            }
            for value in [
                hero.attack_range,
                hero.move_speed_per_sec,
                hero.vision_range,
            ] {
                feed(&value.to_bits().to_le_bytes());
            }
            feed(&hero.abilities);
            for value in [
                hero.health_per_level as u32,
                hero.mana_per_level as u32,
                hero.attack_damage_per_level as u32,
                hero.armor_per_level as u32,
                hero.magic_resist_per_level as u32,
            ] {
                feed(&value.to_le_bytes());
            }
        }
        // Structural guard: this destructure has no `..`, so adding a `RulesSpec` field fails to
        // compile here until it is hashed (and carried on the wire; see the parity test in
        // `moba-net`). The bindings are used below so an added field can't be silently ignored.
        let RulesSpec {
            melee_minion,
            ranged_minion,
            tower,
            nexus,
            level_cap,
            xp_base,
            xp_per_level,
            melee_vision,
            ranged_vision,
            tower_vision,
            nexus_vision,
            team_size,
            bot_fill,
            hero_respawn_ticks,
            respawn_per_level,
            wave_interval_ticks,
            first_wave_ticks,
            starting_gold,
            gold_per_interval,
            gold_tick_interval,
            xp_radius,
            shop_radius,
            shared_xp_percent,
            rewards,
            structure_heroes_first,
            nexus_win,
            score_limit,
        } = &self.rules;
        feed(&[*team_size, u8::from(*bot_fill), *level_cap]);
        for value in [xp_base, xp_per_level] {
            feed(&value.to_le_bytes());
        }
        for value in [melee_vision, ranged_vision, tower_vision, nexus_vision] {
            feed(&value.to_bits().to_le_bytes());
        }
        for minion in [melee_minion, ranged_minion] {
            feed(&minion.max_health.to_le_bytes());
            feed(&minion.attack_damage.to_le_bytes());
            feed(&minion.attack_range.to_bits().to_le_bytes());
            feed(&minion.attack_interval_ticks.to_le_bytes());
            feed(&minion.move_speed_per_sec.to_bits().to_le_bytes());
            feed(&minion.armor.to_le_bytes());
            feed(&minion.magic_resist.to_le_bytes());
            feed(&minion.vision_range.to_bits().to_le_bytes());
        }
        for structure in [tower, nexus] {
            feed(&structure.max_health.to_le_bytes());
            feed(&structure.attack_damage.to_le_bytes());
            feed(&structure.attack_range.to_bits().to_le_bytes());
            feed(&structure.attack_interval_ticks.to_le_bytes());
            feed(&structure.armor.to_le_bytes());
            feed(&structure.magic_resist.to_le_bytes());
            feed(&structure.vision_range.to_bits().to_le_bytes());
        }
        for value in [
            hero_respawn_ticks,
            respawn_per_level,
            wave_interval_ticks,
            first_wave_ticks,
            starting_gold,
            gold_per_interval,
            gold_tick_interval,
        ] {
            feed(&value.to_le_bytes());
        }
        for value in [xp_radius, shop_radius] {
            feed(&value.to_bits().to_le_bytes());
        }
        feed(&shared_xp_percent.to_le_bytes());
        // Exhaustive nested destructures too: a new reward kind or a new per-unit scalar fails
        // to compile here until it is hashed.
        let RewardsSpec {
            melee_minion: melee_reward,
            ranged_minion: ranged_reward,
            tower: tower_reward,
            nexus: nexus_reward,
            hero: hero_reward,
        } = rewards;
        for reward in [
            melee_reward,
            ranged_reward,
            tower_reward,
            nexus_reward,
            hero_reward,
        ] {
            let UnitRewardsSpec { xp, gold } = reward;
            feed(&xp.to_le_bytes());
            feed(&gold.to_le_bytes());
        }
        feed(&[u8::from(*structure_heroes_first)]);
        feed(&[u8::from(*nexus_win)]);
        feed(&score_limit.to_le_bytes());
        string(feed, &self.map.name);
        feed(&[self.map.theme.id()]);
        feed(&self.map.extent.to_bits().to_le_bytes());
        feed(&self.map.blue_spawn.x.to_bits().to_le_bytes());
        feed(&self.map.blue_spawn.y.to_bits().to_le_bytes());
        feed(&self.map.red_spawn.x.to_bits().to_le_bytes());
        feed(&self.map.red_spawn.y.to_bits().to_le_bytes());
        len(feed, self.modifiers.len());
        for modifier in &self.modifiers {
            string(feed, &modifier.name);
            feed(&[modifier.kind as u8]);
            feed(&modifier.bonus_armor.to_le_bytes());
            feed(&modifier.bonus_attack_damage.to_le_bytes());
            feed(&modifier.on_expire_damage.to_le_bytes());
            feed(&modifier.on_damaged_damage.to_le_bytes());
            feed(&modifier.on_tick_damage.to_le_bytes());
            feed(&modifier.status_bits.to_le_bytes());
        }
        len(feed, self.map.lanes.len());
        for lane in &self.map.lanes {
            string(feed, &lane.name);
            len(feed, lane.path.len());
            for point in &lane.path {
                feed(&point.x.to_bits().to_le_bytes());
                feed(&point.y.to_bits().to_le_bytes());
            }
        }
        len(feed, self.map.structures.len());
        for structure in &self.map.structures {
            feed(&[structure.team.index() as u8, structure.kind as u8]);
            feed(&structure.position.x.to_bits().to_le_bytes());
            feed(&structure.position.y.to_bits().to_le_bytes());
            feed(&[structure.lane.unwrap_or(0xFF)]);
        }
        len(feed, self.map.walls.len());
        for wall in &self.map.walls {
            for point in [wall.a, wall.b] {
                feed(&point.x.to_bits().to_le_bytes());
                feed(&point.y.to_bits().to_le_bytes());
            }
        }
    }

    /// Stable FNV-1a hash of the catalogue, used for diagnostics and mismatch checks.
    #[must_use]
    pub fn hash(&self) -> u64 {
        let mut hash = 0xCBF2_9CE4_8422_2325u64;
        let mut feed = |bytes: &[u8]| {
            for byte in bytes {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
            }
        };
        self.feed_catalogue(&mut feed);
        hash
    }
}
