//! [`WireCatalog`] -> [`GameContent`] reconstruction and validation.

use moba_core::{Fix, FixVec2};
use moba_game::{
    AbilitySpec, AreaEffect, AreaEffectList, ContentManifest, DamageType, Effect, EffectList,
    GameContent, HeroSpec, ItemSpec, LaneSpec, MapSpec, ModifierDef, StructureKind, StructureSpec,
    Targeting, Team, WallSpec,
};

use super::WireError;
use super::catalog::{
    MAX_WIRE_ABILITIES, MAX_WIRE_AREA_EFFECTS, MAX_WIRE_EFFECTS, MAX_WIRE_HEROES, MAX_WIRE_ITEMS,
    MAX_WIRE_LANE_POINTS, MAX_WIRE_LANES, MAX_WIRE_MODIFIERS, MAX_WIRE_STRUCTURES, MAX_WIRE_WALLS,
    WIRE_ABILITY_NAME, WIRE_HERO_NAME, WIRE_ITEM_NAME, WIRE_LANE_NAME, WIRE_MANIFEST_FIELD,
    WIRE_MODIFIER_NAME, WireCatalog, WireEffect, WireMinionStats, WireRewards, WireStructureStats,
    WireUnitRewards,
};

fn decode_area_effects(wire: &WireEffect) -> Result<AreaEffectList, WireError> {
    let count = usize::from(wire.area_count);
    if count > MAX_WIRE_AREA_EFFECTS {
        return Err(WireError::BadValue);
    }
    let mut effects = AreaEffectList::EMPTY;
    for (slot, encoded) in wire.area[..count].iter().enumerate() {
        effects.effects[slot] = match encoded.kind {
            0 => AreaEffect::None,
            1 => AreaEffect::Damage {
                amount: encoded.amount,
                damage_type: damage_type_from_code(encoded.damage_type)?,
            },
            2 => AreaEffect::ApplyModifier {
                modifier: encoded.modifier,
                duration_ticks: encoded.duration_ticks,
                factor: Fix::from_bits(encoded.factor_bits),
                amount: encoded.amount,
                damage_type: damage_type_from_code(encoded.damage_type)?,
            },
            _ => return Err(WireError::BadValue),
        };
    }
    Ok(effects)
}

fn minion_spec(wire: WireMinionStats) -> moba_game::MinionStatsSpec {
    moba_game::MinionStatsSpec {
        max_health: wire.max_health,
        attack_damage: wire.attack_damage,
        attack_range: Fix::from_bits(wire.attack_range_bits),
        attack_interval_ticks: wire.attack_interval_ticks,
        move_speed_per_sec: Fix::from_bits(wire.move_speed_bits),
        armor: wire.armor,
        magic_resist: wire.magic_resist,
        vision_range: Fix::from_bits(wire.vision_range_bits),
    }
}

fn structure_spec(wire: WireStructureStats) -> moba_game::StructureStatsSpec {
    moba_game::StructureStatsSpec {
        max_health: wire.max_health,
        attack_damage: wire.attack_damage,
        attack_range: Fix::from_bits(wire.attack_range_bits),
        attack_interval_ticks: wire.attack_interval_ticks,
        armor: wire.armor,
        magic_resist: wire.magic_resist,
        vision_range: Fix::from_bits(wire.vision_range_bits),
    }
}

const fn unit_rewards_spec(wire: WireUnitRewards) -> moba_game::UnitRewardsSpec {
    moba_game::UnitRewardsSpec {
        xp: wire.xp,
        gold: wire.gold,
    }
}

const fn rewards_spec(wire: WireRewards) -> moba_game::RewardsSpec {
    moba_game::RewardsSpec {
        melee_minion: unit_rewards_spec(wire.melee_minion),
        ranged_minion: unit_rewards_spec(wire.ranged_minion),
        tower: unit_rewards_spec(wire.tower),
        nexus: unit_rewards_spec(wire.nexus),
        hero: unit_rewards_spec(wire.hero),
    }
}

fn decode_effect(wire: &WireEffect) -> Result<Effect, WireError> {
    Ok(match wire.kind {
        0 => Effect::None,
        1 => Effect::Damage {
            amount: wire.amount,
            damage_type: damage_type_from_code(wire.damage_type)?,
        },
        2 => Effect::Heal {
            amount: wire.amount,
        },
        3 => {
            let area = decode_area_effects(wire)?;
            let Some(AreaEffect::ApplyModifier {
                modifier,
                duration_ticks,
                factor,
                amount,
                damage_type,
            }) = area.effects.first().copied()
            else {
                return Err(WireError::BadValue);
            };
            Effect::ApplyModifier {
                modifier,
                duration_ticks,
                factor,
                amount,
                damage_type,
            }
        }
        4 => Effect::Displace {
            range: Fix::from_bits(wire.range_bits),
        },
        5 => Effect::Projectile {
            speed_per_sec: Fix::from_bits(wire.speed_bits),
            range: Fix::from_bits(wire.range_bits),
            radius: Fix::from_bits(wire.radius_bits),
            arc: Fix::from_bits(wire.arc_bits),
            on_hit: decode_area_effects(wire)?,
        },
        6 => Effect::Area {
            radius: Fix::from_bits(wire.radius_bits),
            effects: decode_area_effects(wire)?,
        },
        7 => Effect::Zone {
            radius: Fix::from_bits(wire.radius_bits),
            duration_ticks: wire.duration_ticks,
            period_ticks: wire.period_ticks,
            effects: decode_area_effects(wire)?,
        },
        _ => return Err(WireError::BadValue),
    })
}

impl WireCatalog {
    /// Reconstructs the runtime content represented by this catalogue.
    ///
    /// # Errors
    /// Returns [`WireError::BadValue`] for invalid enum/UTF-8/count fields.
    #[allow(clippy::too_many_lines)] // One exhaustive reconstruction.
    pub fn to_content(&self) -> Result<GameContent, WireError> {
        macro_rules! count {
            ($value:expr, $max:expr) => {{
                let count = usize::from($value);
                if count > $max {
                    return Err(WireError::BadValue);
                }
                count
            }};
        }
        let items = (0..count!(self.item_count, MAX_WIRE_ITEMS))
            .map(|index| {
                let item = &self.items[index];
                Ok(ItemSpec {
                    name: read_wire_name(&item.name, item.name_len, WIRE_ITEM_NAME)?,
                    cost: item.cost,
                    health: item.health,
                    mana: item.mana,
                    attack_damage: item.attack_damage,
                    armor: item.armor,
                    magic_resist: item.magic_resist,
                    move_speed_per_sec: Fix::from_bits(item.move_speed_bits),
                    on_hit_damage: item.on_hit_damage,
                    on_damaged_damage: item.on_damaged_damage,
                    active_heal: item.active_heal,
                    active_cooldown_ticks: item.active_cooldown_ticks,
                })
            })
            .collect::<Result<Vec<_>, WireError>>()?;
        let heroes = (0..count!(self.hero_count, MAX_WIRE_HEROES))
            .map(|index| {
                let hero = &self.heroes[index];
                Ok(HeroSpec {
                    name: read_wire_name(&hero.name, hero.name_len, WIRE_HERO_NAME)?,
                    max_health: hero.max_health,
                    max_mana: hero.max_mana,
                    health_regen_per_tick: hero.health_regen_per_tick,
                    mana_regen_per_tick: hero.mana_regen_per_tick,
                    attack_damage: hero.attack_damage,
                    attack_range: Fix::from_bits(hero.attack_range_bits),
                    attack_interval_ticks: hero.attack_interval_ticks,
                    move_speed_per_sec: Fix::from_bits(hero.move_speed_bits),
                    armor: hero.armor,
                    magic_resist: hero.magic_resist,
                    vision_range: Fix::from_bits(hero.vision_bits),
                    abilities: hero.abilities,
                    health_per_level: hero.health_per_level,
                    mana_per_level: hero.mana_per_level,
                    attack_damage_per_level: hero.attack_damage_per_level,
                    armor_per_level: hero.armor_per_level,
                    magic_resist_per_level: hero.magic_resist_per_level,
                })
            })
            .collect::<Result<Vec<_>, WireError>>()?;
        let abilities = (0..count!(self.ability_count, MAX_WIRE_ABILITIES))
            .map(|index| {
                let ability = &self.abilities[index];
                let targeting = match ability.targeting_kind {
                    0 => Targeting::SelfCast,
                    1 => Targeting::Point {
                        range: Fix::from_bits(ability.targeting_range_bits),
                    },
                    2 => Targeting::Direction {
                        range: Fix::from_bits(ability.targeting_range_bits),
                    },
                    _ => return Err(WireError::BadValue),
                };
                let effect_count = count!(ability.effect_count, MAX_WIRE_EFFECTS);
                let mut effects = EffectList::EMPTY;
                for (slot, wire) in ability.effects[..effect_count].iter().enumerate() {
                    effects.effects[slot] = decode_effect(wire)?;
                }
                Ok(AbilitySpec {
                    name: read_wire_name(&ability.name, ability.name_len, WIRE_ABILITY_NAME)?,
                    cooldown_ticks: ability.cooldown_ticks,
                    mana_cost: ability.mana_cost,
                    targeting,
                    effects,
                })
            })
            .collect::<Result<Vec<_>, WireError>>()?;
        let modifiers = (0..count!(self.modifier_count, MAX_WIRE_MODIFIERS))
            .map(|index| {
                let modifier = &self.modifiers[index];
                Ok(ModifierDef {
                    name: read_wire_name(&modifier.name, modifier.name_len, WIRE_MODIFIER_NAME)?,
                    bonus_armor: modifier.bonus_armor,
                    bonus_attack_damage: modifier.bonus_attack_damage,
                    on_expire_damage: modifier.on_expire_damage,
                    on_damaged_damage: modifier.on_damaged_damage,
                    on_tick_damage: modifier.on_tick_damage,
                    status_bits: modifier.status_bits,
                    kind: match modifier.kind {
                        0 => moba_game::ModifierKind::Shield,
                        1 => moba_game::ModifierKind::Slow,
                        2 => moba_game::ModifierKind::Haste,
                        3 => moba_game::ModifierKind::Root,
                        4 => moba_game::ModifierKind::Silence,
                        5 => moba_game::ModifierKind::Burn,
                        6 => moba_game::ModifierKind::Stun,
                        _ => return Err(WireError::BadValue),
                    },
                })
            })
            .collect::<Result<Vec<_>, WireError>>()?;
        let map = &self.map;
        let lanes = (0..count!(map.lane_count, MAX_WIRE_LANES))
            .map(|index| {
                let lane = &map.lanes[index];
                let point_count = count!(lane.point_count, MAX_WIRE_LANE_POINTS);
                Ok(LaneSpec {
                    name: read_wire_name(&lane.name, lane.name_len, WIRE_LANE_NAME)?,
                    path: lane.points[..point_count]
                        .iter()
                        .map(|point| FixVec2::from_ints(i32::from(point[0]), i32::from(point[1])))
                        .collect(),
                })
            })
            .collect::<Result<Vec<_>, WireError>>()?;
        let structures = (0..count!(map.structure_count, MAX_WIRE_STRUCTURES))
            .map(|index| {
                let structure = &map.structures[index];
                Ok(StructureSpec {
                    team: match structure.team {
                        0 => Team::Blue,
                        1 => Team::Red,
                        _ => return Err(WireError::BadValue),
                    },
                    position: FixVec2::from_ints(
                        i32::from(structure.position[0]),
                        i32::from(structure.position[1]),
                    ),
                    kind: match structure.kind {
                        0 => StructureKind::Tower,
                        1 => StructureKind::Nexus,
                        _ => return Err(WireError::BadValue),
                    },
                    lane: (structure.lane != 0xFF).then_some(structure.lane),
                })
            })
            .collect::<Result<Vec<_>, WireError>>()?;
        let walls = (0..count!(map.wall_count, MAX_WIRE_WALLS))
            .map(|index| {
                let wall = &map.walls[index];
                Ok(WallSpec {
                    a: FixVec2::from_ints(i32::from(wall.a[0]), i32::from(wall.a[1])),
                    b: FixVec2::from_ints(i32::from(wall.b[0]), i32::from(wall.b[1])),
                })
            })
            .collect::<Result<Vec<_>, WireError>>()?;
        Ok(GameContent {
            manifest: ContentManifest {
                name: read_wire_name(
                    &self.manifest_name,
                    self.manifest_name_len,
                    WIRE_MANIFEST_FIELD,
                )?,
                version: read_wire_name(
                    &self.manifest_version,
                    self.manifest_version_len,
                    WIRE_MANIFEST_FIELD,
                )?,
                author: read_wire_name(
                    &self.manifest_author,
                    self.manifest_author_len,
                    WIRE_MANIFEST_FIELD,
                )?,
            },
            items,
            heroes,
            abilities,
            modifiers,
            rules: moba_game::RulesSpec {
                melee_minion: minion_spec(self.rules.melee_minion),
                ranged_minion: minion_spec(self.rules.ranged_minion),
                tower: structure_spec(self.rules.tower),
                nexus: structure_spec(self.rules.nexus),
                level_cap: self.rules.level_cap,
                xp_base: self.rules.xp_base,
                xp_per_level: self.rules.xp_per_level,
                melee_vision: moba_core::Fix::from_bits(self.rules.melee_vision_bits),
                ranged_vision: moba_core::Fix::from_bits(self.rules.ranged_vision_bits),
                tower_vision: moba_core::Fix::from_bits(self.rules.tower_vision_bits),
                nexus_vision: moba_core::Fix::from_bits(self.rules.nexus_vision_bits),
                team_size: self.rules.team_size,
                bot_fill: self.rules.bot_fill,
                hero_respawn_ticks: self.rules.hero_respawn_ticks,
                respawn_per_level: self.rules.respawn_per_level,
                wave_interval_ticks: self.rules.wave_interval_ticks,
                first_wave_ticks: self.rules.first_wave_ticks,
                starting_gold: self.rules.starting_gold,
                gold_per_interval: self.rules.gold_per_interval,
                gold_tick_interval: self.rules.gold_tick_interval,
                xp_radius: moba_core::Fix::from_bits(self.rules.xp_radius_bits),
                shop_radius: moba_core::Fix::from_bits(self.rules.shop_radius_bits),
                shared_xp_percent: self.rules.shared_xp_percent,
                rewards: rewards_spec(self.rules.rewards),
                structure_heroes_first: self.rules.structure_heroes_first,
                nexus_win: self.rules.nexus_win,
                score_limit: self.rules.score_limit,
            },
            attestation: None,
            map: MapSpec {
                name: map.name_str().to_string(),
                theme: moba_game::MapTheme::from_id(map.theme).ok_or(WireError::BadValue)?,
                extent: Fix::from_i32(i32::from(map.extent)),
                lanes,
                structures,
                walls,
                blue_spawn: FixVec2::from_ints(
                    i32::from(map.blue_spawn[0]),
                    i32::from(map.blue_spawn[1]),
                ),
                red_spawn: FixVec2::from_ints(
                    i32::from(map.red_spawn[0]),
                    i32::from(map.red_spawn[1]),
                ),
            },
        })
    }

    /// Hashes the reconstructed catalogue with [`GameContent::hash`].
    ///
    /// # Errors
    /// Returns [`WireError::BadValue`] for an invalid catalogue.
    pub fn content_hash(&self) -> Result<u64, WireError> {
        Ok(self.to_content()?.hash())
    }
}

fn read_wire_name(bytes: &[u8], used: u8, capacity: usize) -> Result<String, WireError> {
    let used = usize::from(used);
    if used > capacity || used > bytes.len() {
        return Err(WireError::BadValue);
    }
    String::from_utf8(bytes[..used].to_vec()).map_err(|_| WireError::BadValue)
}

const fn damage_type_from_code(code: u8) -> Result<DamageType, WireError> {
    match code {
        0 => Ok(DamageType::Physical),
        1 => Ok(DamageType::Magic),
        _ => Err(WireError::BadValue),
    }
}
