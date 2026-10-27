//! [`GameContent`] -> [`WireCatalog`] conversion.

use moba_core::FixVec2;
use moba_game::{
    AbilitySpec, AreaEffect, AreaEffectList, ContentManifest, DamageType, Effect, GameContent,
    HeroSpec, ItemSpec, LaneSpec, MapSpec, ModifierDef, StructureKind, StructureSpec, Targeting,
    WallSpec,
};

use super::catalog::{
    MAX_WIRE_ABILITIES, MAX_WIRE_AREA_EFFECTS, MAX_WIRE_EFFECTS, MAX_WIRE_HEROES, MAX_WIRE_ITEMS,
    MAX_WIRE_LANE_POINTS, MAX_WIRE_LANES, MAX_WIRE_MODIFIERS, MAX_WIRE_STRUCTURES, MAX_WIRE_WALLS,
    WireAreaEffect, WireCatalog, WireEffect, WireMinionStats, WireRewards, WireRules,
    WireStructureStats, WireUnitRewards,
};

fn encode_area_effects(effects: &AreaEffectList) -> (u8, [WireAreaEffect; MAX_WIRE_AREA_EFFECTS]) {
    let mut area = [WireAreaEffect::default(); MAX_WIRE_AREA_EFFECTS];
    let mut count = 0usize;
    for effect in effects.iter() {
        if count >= MAX_WIRE_AREA_EFFECTS {
            break;
        }
        area[count] = match effect {
            AreaEffect::None => WireAreaEffect::default(),
            AreaEffect::Damage {
                amount,
                damage_type,
            } => WireAreaEffect {
                kind: 1,
                amount: *amount,
                damage_type: damage_type_code(*damage_type),
                ..WireAreaEffect::default()
            },
            AreaEffect::ApplyModifier {
                modifier,
                duration_ticks,
                factor,
                amount,
                damage_type,
            } => WireAreaEffect {
                kind: 2,
                modifier: *modifier,
                duration_ticks: *duration_ticks,
                factor_bits: factor.to_bits(),
                amount: *amount,
                damage_type: damage_type_code(*damage_type),
            },
        };
        count += 1;
    }
    (u8::try_from(count).unwrap_or(u8::MAX), area)
}

impl From<&GameContent> for WireCatalog {
    #[allow(clippy::too_many_lines)] // One exhaustive destructured conversion.
    fn from(content: &GameContent) -> Self {
        let GameContent {
            manifest,
            items: content_items,
            heroes: content_heroes,
            abilities: content_abilities,
            modifiers: content_modifiers,
            rules: content_rules,
            map: content_map,
            attestation: _,
        } = content;
        let ContentManifest {
            name,
            version,
            author,
        } = manifest;
        let mut catalog = Self::default();
        catalog.manifest_name_len = put_wire_name(&mut catalog.manifest_name, name);
        catalog.manifest_version_len = put_wire_name(&mut catalog.manifest_version, version);
        catalog.manifest_author_len = put_wire_name(&mut catalog.manifest_author, author);

        catalog.item_count = u8::try_from(content_items.len().min(MAX_WIRE_ITEMS))
            .unwrap_or(u8::try_from(MAX_WIRE_ITEMS).unwrap_or(u8::MAX));
        for (wire, item) in catalog.items.iter_mut().zip(content_items) {
            let ItemSpec {
                name,
                cost,
                health,
                mana,
                attack_damage,
                armor,
                magic_resist,
                move_speed_per_sec,
                on_hit_damage,
                on_damaged_damage,
                active_heal,
                active_cooldown_ticks,
            } = item;
            wire.name_len = put_wire_name(&mut wire.name, name);
            wire.cost = *cost;
            wire.health = *health;
            wire.mana = *mana;
            wire.attack_damage = *attack_damage;
            wire.armor = *armor;
            wire.magic_resist = *magic_resist;
            wire.move_speed_bits = move_speed_per_sec.to_bits();
            wire.on_hit_damage = *on_hit_damage;
            wire.on_damaged_damage = *on_damaged_damage;
            wire.active_heal = *active_heal;
            wire.active_cooldown_ticks = *active_cooldown_ticks;
        }

        catalog.hero_count = u8::try_from(content_heroes.len().min(MAX_WIRE_HEROES))
            .unwrap_or(u8::try_from(MAX_WIRE_HEROES).unwrap_or(u8::MAX));
        for (wire, hero) in catalog.heroes.iter_mut().zip(content_heroes) {
            let HeroSpec {
                name,
                max_health,
                max_mana,
                health_regen_per_tick,
                mana_regen_per_tick,
                attack_damage,
                attack_range,
                attack_interval_ticks,
                move_speed_per_sec,
                armor,
                magic_resist,
                vision_range,
                abilities,
                health_per_level,
                mana_per_level,
                attack_damage_per_level,
                armor_per_level,
                magic_resist_per_level,
            } = hero;
            wire.name_len = put_wire_name(&mut wire.name, name);
            wire.max_health = *max_health;
            wire.max_mana = *max_mana;
            wire.health_regen_per_tick = *health_regen_per_tick;
            wire.mana_regen_per_tick = *mana_regen_per_tick;
            wire.attack_damage = *attack_damage;
            wire.attack_range_bits = attack_range.to_bits();
            wire.attack_interval_ticks = *attack_interval_ticks;
            wire.move_speed_bits = move_speed_per_sec.to_bits();
            wire.armor = *armor;
            wire.magic_resist = *magic_resist;
            wire.vision_bits = vision_range.to_bits();
            wire.abilities = *abilities;
            wire.health_per_level = *health_per_level;
            wire.mana_per_level = *mana_per_level;
            wire.attack_damage_per_level = *attack_damage_per_level;
            wire.armor_per_level = *armor_per_level;
            wire.magic_resist_per_level = *magic_resist_per_level;
        }

        catalog.ability_count = u8::try_from(content_abilities.len().min(MAX_WIRE_ABILITIES))
            .unwrap_or(u8::try_from(MAX_WIRE_ABILITIES).unwrap_or(u8::MAX));
        for (wire, ability) in catalog.abilities.iter_mut().zip(content_abilities) {
            let AbilitySpec {
                name,
                cooldown_ticks,
                mana_cost,
                targeting,
                effects,
            } = ability;
            wire.name_len = put_wire_name(&mut wire.name, name);
            wire.cooldown_ticks = *cooldown_ticks;
            wire.mana_cost = *mana_cost;
            wire.cast_range_bits = targeting.range().to_bits();
            wire.radius_bits = ability.display_radius().to_bits();
            match targeting {
                Targeting::SelfCast => wire.targeting_kind = 0,
                Targeting::Point { range } => {
                    wire.targeting_kind = 1;
                    wire.targeting_range_bits = range.to_bits();
                }
                Targeting::Direction { range } => {
                    wire.targeting_kind = 2;
                    wire.targeting_range_bits = range.to_bits();
                }
            }
            let mut count = 0usize;
            for effect in effects.iter() {
                if count >= MAX_WIRE_EFFECTS {
                    break;
                }
                let mut encoded = WireEffect::default();
                match effect {
                    Effect::None => continue,
                    Effect::Damage {
                        amount,
                        damage_type,
                    } => {
                        encoded.kind = 1;
                        encoded.amount = *amount;
                        encoded.damage_type = damage_type_code(*damage_type);
                    }
                    Effect::Heal { amount } => {
                        encoded.kind = 2;
                        encoded.amount = *amount;
                    }
                    Effect::ApplyModifier {
                        modifier,
                        duration_ticks,
                        factor,
                        amount,
                        damage_type,
                    } => {
                        encoded.kind = 3;
                        encoded.amount = *amount;
                        encoded.damage_type = damage_type_code(*damage_type);
                        encoded.range_bits = factor.to_bits();
                        encoded.duration_ticks = *duration_ticks;
                        encoded.area[0] = WireAreaEffect {
                            kind: 2,
                            modifier: *modifier,
                            duration_ticks: *duration_ticks,
                            factor_bits: factor.to_bits(),
                            amount: *amount,
                            damage_type: damage_type_code(*damage_type),
                        };
                        encoded.area_count = 1;
                    }
                    Effect::Displace { range } => {
                        encoded.kind = 4;
                        encoded.range_bits = range.to_bits();
                    }
                    Effect::Projectile {
                        speed_per_sec,
                        range,
                        radius,
                        arc,
                        on_hit,
                    } => {
                        encoded.kind = 5;
                        encoded.speed_bits = speed_per_sec.to_bits();
                        encoded.range_bits = range.to_bits();
                        encoded.radius_bits = radius.to_bits();
                        encoded.arc_bits = arc.to_bits();
                        let (area_count, area) = encode_area_effects(on_hit);
                        encoded.area_count = area_count;
                        encoded.area = area;
                    }
                    Effect::Area { radius, effects } => {
                        encoded.kind = 6;
                        encoded.radius_bits = radius.to_bits();
                        let (area_count, area) = encode_area_effects(effects);
                        encoded.area_count = area_count;
                        encoded.area = area;
                    }
                    Effect::Zone {
                        radius,
                        duration_ticks,
                        period_ticks,
                        effects,
                    } => {
                        encoded.kind = 7;
                        encoded.radius_bits = radius.to_bits();
                        encoded.duration_ticks = *duration_ticks;
                        encoded.period_ticks = *period_ticks;
                        let (area_count, area) = encode_area_effects(effects);
                        encoded.area_count = area_count;
                        encoded.area = area;
                    }
                }
                wire.effects[count] = encoded;
                count += 1;
            }
            wire.effect_count = u8::try_from(count).unwrap_or(u8::MAX);
        }

        catalog.modifier_count = u8::try_from(content_modifiers.len().min(MAX_WIRE_MODIFIERS))
            .unwrap_or(u8::try_from(MAX_WIRE_MODIFIERS).unwrap_or(u8::MAX));
        for (wire, modifier) in catalog.modifiers.iter_mut().zip(content_modifiers) {
            let ModifierDef {
                name,
                kind,
                bonus_armor,
                bonus_attack_damage,
                on_expire_damage,
                on_damaged_damage,
                on_tick_damage,
                status_bits,
            } = modifier;
            wire.name_len = put_wire_name(&mut wire.name, name);
            wire.kind = match kind {
                moba_game::ModifierKind::Shield => 0,
                moba_game::ModifierKind::Slow => 1,
                moba_game::ModifierKind::Haste => 2,
                moba_game::ModifierKind::Root => 3,
                moba_game::ModifierKind::Silence => 4,
                moba_game::ModifierKind::Burn => 5,
                moba_game::ModifierKind::Stun => 6,
            };
            wire.bonus_armor = *bonus_armor;
            wire.bonus_attack_damage = *bonus_attack_damage;
            wire.on_expire_damage = *on_expire_damage;
            wire.on_damaged_damage = *on_damaged_damage;
            wire.on_tick_damage = *on_tick_damage;
            wire.status_bits = *status_bits;
        }

        let MapSpec {
            name: content_name,
            theme: content_theme,
            extent,
            lanes: content_lanes,
            structures: content_structures,
            walls: content_walls,
            blue_spawn,
            red_spawn,
        } = content_map;
        let minion = |stats: moba_game::MinionStatsSpec| WireMinionStats {
            max_health: stats.max_health,
            attack_damage: stats.attack_damage,
            attack_range_bits: stats.attack_range.to_bits(),
            attack_interval_ticks: stats.attack_interval_ticks,
            move_speed_bits: stats.move_speed_per_sec.to_bits(),
            armor: stats.armor,
            magic_resist: stats.magic_resist,
            vision_range_bits: stats.vision_range.to_bits(),
        };
        let structure = |stats: moba_game::StructureStatsSpec| WireStructureStats {
            max_health: stats.max_health,
            attack_damage: stats.attack_damage,
            attack_range_bits: stats.attack_range.to_bits(),
            attack_interval_ticks: stats.attack_interval_ticks,
            armor: stats.armor,
            magic_resist: stats.magic_resist,
            vision_range_bits: stats.vision_range.to_bits(),
        };
        // Structural guard: this destructure has no `..`, so adding a `RulesSpec` field fails
        // to compile here until it is carried in `WireRules` (and the parity test is extended).
        let moba_game::RulesSpec {
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
            rewards: content_rewards,
            structure_heroes_first,
            nexus_win,
            score_limit,
        } = content_rules;
        // The nested destructures guard reward kinds and per-unit scalars as well: a new
        // `RewardsSpec` kind or `UnitRewardsSpec` field fails to compile here until it is wired.
        let moba_game::RewardsSpec {
            melee_minion: melee_reward,
            ranged_minion: ranged_reward,
            tower: tower_reward,
            nexus: nexus_reward,
            hero: hero_reward,
        } = content_rewards;
        let unit = |reward: &moba_game::UnitRewardsSpec| {
            let moba_game::UnitRewardsSpec { xp, gold } = reward;
            WireUnitRewards {
                xp: *xp,
                gold: *gold,
            }
        };
        let rewards = WireRewards {
            melee_minion: unit(melee_reward),
            ranged_minion: unit(ranged_reward),
            tower: unit(tower_reward),
            nexus: unit(nexus_reward),
            hero: unit(hero_reward),
        };
        catalog.rules = WireRules {
            melee_minion: minion(*melee_minion),
            ranged_minion: minion(*ranged_minion),
            tower: structure(*tower),
            nexus: structure(*nexus),
            level_cap: *level_cap,
            xp_base: *xp_base,
            xp_per_level: *xp_per_level,
            melee_vision_bits: melee_vision.to_bits(),
            ranged_vision_bits: ranged_vision.to_bits(),
            tower_vision_bits: tower_vision.to_bits(),
            nexus_vision_bits: nexus_vision.to_bits(),
            team_size: *team_size,
            bot_fill: *bot_fill,
            hero_respawn_ticks: *hero_respawn_ticks,
            respawn_per_level: *respawn_per_level,
            wave_interval_ticks: *wave_interval_ticks,
            first_wave_ticks: *first_wave_ticks,
            starting_gold: *starting_gold,
            gold_per_interval: *gold_per_interval,
            gold_tick_interval: *gold_tick_interval,
            xp_radius_bits: xp_radius.to_bits(),
            shop_radius_bits: shop_radius.to_bits(),
            shared_xp_percent: *shared_xp_percent,
            rewards,
            structure_heroes_first: *structure_heroes_first,
            nexus_win: *nexus_win,
            score_limit: *score_limit,
        };
        // Map geometry, counts and names below are exact for content that passed
        // `GameContent::validate` (#296): it rejects every coordinate `MapSpec::wire_point` /
        // `MapSpec::wire_extent` cannot carry, over-capacity lane/point/structure/wall counts,
        // and over-long or non-ASCII map/lane names. The fallbacks only fire for unvalidated
        // content, whose reconstructed hash then differs, so the client rejects it.
        let point = |point: &FixVec2| MapSpec::wire_point(*point).unwrap_or_default();
        let map = &mut catalog.map;
        map.name_len = put_wire_name(&mut map.name, content_name);
        map.theme = content_theme.id();
        map.lane_count = u8::try_from(content_lanes.len().min(MAX_WIRE_LANES))
            .unwrap_or(u8::try_from(MAX_WIRE_LANES).unwrap_or(u8::MAX));
        for (wire, lane) in map.lanes.iter_mut().zip(content_lanes) {
            let LaneSpec { name, path } = lane;
            wire.name_len = put_wire_name(&mut wire.name, name);
            wire.point_count = u8::try_from(path.len().min(MAX_WIRE_LANE_POINTS))
                .unwrap_or(u8::try_from(MAX_WIRE_LANE_POINTS).unwrap_or(u8::MAX));
            for (target, waypoint) in wire.points.iter_mut().zip(path) {
                *target = point(waypoint);
            }
        }
        map.extent = MapSpec::wire_extent(*extent).unwrap_or(256);
        map.blue_spawn = point(blue_spawn);
        map.red_spawn = point(red_spawn);
        map.structure_count = u8::try_from(content_structures.len().min(MAX_WIRE_STRUCTURES))
            .unwrap_or(u8::try_from(MAX_WIRE_STRUCTURES).unwrap_or(u8::MAX));
        for (wire, structure) in map.structures.iter_mut().zip(content_structures) {
            let StructureSpec {
                team,
                position,
                kind,
                lane,
            } = structure;
            wire.team = team.index() as u8;
            wire.kind = match kind {
                StructureKind::Tower => 0,
                StructureKind::Nexus => 1,
            };
            wire.position = point(position);
            wire.lane = lane.unwrap_or(0xFF);
        }
        map.wall_count = u8::try_from(content_walls.len().min(MAX_WIRE_WALLS))
            .unwrap_or(u8::try_from(MAX_WIRE_WALLS).unwrap_or(u8::MAX));
        for (wire, wall) in map.walls.iter_mut().zip(content_walls) {
            let WallSpec { a, b } = wall;
            wire.a = point(a);
            wire.b = point(b);
        }
        catalog
    }
}

fn put_wire_name<const N: usize>(target: &mut [u8; N], name: &str) -> u8 {
    let bytes = name.as_bytes();
    let length = bytes.len().min(N);
    target[..length].copy_from_slice(&bytes[..length]);
    u8::try_from(length).unwrap_or(u8::MAX)
}

const fn damage_type_code(damage_type: DamageType) -> u8 {
    match damage_type {
        DamageType::Physical => 0,
        DamageType::Magic => 1,
    }
}
