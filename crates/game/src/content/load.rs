//! TOML loading: converts the serde schema into [`GameContent`] and validates it.

use std::path::Path;

use moba_core::{Fix, FixVec2};

use crate::{
    AreaEffect, AreaEffectList, DamageType, Effect, EffectList, StructureKind, Targeting, Team,
};

use super::schema::{ContentFile, MapEntry, MinionStatsEntry, StructureStatsEntry};
use super::{
    AbilitySpec, ContentLoadError, ContentManifest, GameContent, HeroSpec, ItemSpec, LaneSpec,
    MapSpec, MapTheme, MinionStatsSpec, ModifierDef, ModifierKind, PackAttestation, RewardsSpec,
    RulesSpec, StructureSpec, StructureStatsSpec, UnitRewardsSpec, WallSpec,
};

fn invalid_field(field: &'static str, value: impl Into<String>) -> ContentLoadError {
    ContentLoadError::InvalidField {
        field,
        value: value.into(),
    }
}

fn fix_param(value: f32, field: &'static str) -> Result<Fix, ContentLoadError> {
    if value.is_finite() {
        Ok(Fix::from_f32(value))
    } else {
        Err(invalid_field(field, value.to_string()))
    }
}

pub(super) fn parse_modifier_kind(value: &str) -> Result<ModifierKind, ContentLoadError> {
    match value {
        "shield" => Ok(ModifierKind::Shield),
        "slow" => Ok(ModifierKind::Slow),
        "haste" => Ok(ModifierKind::Haste),
        "root" => Ok(ModifierKind::Root),
        "silence" => Ok(ModifierKind::Silence),
        "burn" => Ok(ModifierKind::Burn),
        "stun" => Ok(ModifierKind::Stun),
        other => Err(invalid_field("modifiers.kind", other)),
    }
}

fn parse_hex_array<const N: usize>(value: &str) -> Result<[u8; N], ()> {
    let bytes = value.as_bytes();
    if bytes.len() != N * 2 || !bytes.iter().all(u8::is_ascii_hexdigit) {
        return Err(());
    }
    let mut output = [0u8; N];
    for (index, byte) in output.iter_mut().enumerate() {
        let text = std::str::from_utf8(&bytes[index * 2..index * 2 + 2]).map_err(|_| ())?;
        *byte = u8::from_str_radix(text, 16).map_err(|_| ())?;
    }
    Ok(output)
}

fn parse_damage_type(value: &str) -> Result<DamageType, ContentLoadError> {
    match value {
        "physical" => Ok(DamageType::Physical),
        "magic" => Ok(DamageType::Magic),
        other => Err(invalid_field("abilities.damage_type", other)),
    }
}

fn parse_team(value: &str) -> Result<Team, ContentLoadError> {
    match value {
        "blue" => Ok(Team::Blue),
        "red" => Ok(Team::Red),
        other => Err(invalid_field("map.structures.team", other)),
    }
}

fn parse_structure_kind(value: &str) -> Result<StructureKind, ContentLoadError> {
    match value {
        "nexus" => Ok(StructureKind::Nexus),
        "tower" => Ok(StructureKind::Tower),
        other => Err(invalid_field("map.structures.kind", other)),
    }
}

/// Parses and validates a content pack from TOML.
///
/// # Errors
/// Returns [`ContentLoadError`] for syntax or validation failures.
#[allow(clippy::too_many_lines)] // One declarative conversion for every content table.
pub fn load_toml(text: &str) -> Result<GameContent, ContentLoadError> {
    let file: ContentFile = toml::from_str(text).map_err(ContentLoadError::Toml)?;
    let items = file
        .items
        .into_iter()
        .map(|item| {
            Ok::<ItemSpec, ContentLoadError>(ItemSpec {
                name: item.name,
                cost: item.cost,
                health: item.health,
                mana: item.mana,
                attack_damage: item.attack_damage,
                armor: item.armor,
                magic_resist: item.magic_resist,
                move_speed_per_sec: fix_param(item.move_speed, "items.move_speed")?,
                on_hit_damage: item.on_hit_damage,
                on_damaged_damage: item.on_damaged_damage,
                active_heal: item.active_heal,
                active_cooldown_ticks: item.active_cooldown_ticks,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let items = if items.is_empty() {
        GameContent::default().items
    } else {
        items
    };
    let heroes = match file.heroes {
        Some(heroes) => heroes
            .into_iter()
            .map(|hero| {
                Ok::<HeroSpec, ContentLoadError>(HeroSpec {
                    name: hero.name,
                    max_health: hero.max_health,
                    max_mana: hero.max_mana,
                    health_regen_per_tick: hero.health_regen_per_tick,
                    mana_regen_per_tick: hero.mana_regen_per_tick,
                    attack_damage: hero.attack_damage,
                    attack_range: fix_param(hero.attack_range, "heroes.attack_range")?,
                    attack_interval_ticks: hero.attack_interval_ticks,
                    move_speed_per_sec: fix_param(hero.move_speed, "heroes.move_speed")?,
                    armor: hero.armor,
                    magic_resist: hero.magic_resist,
                    vision_range: fix_param(hero.vision_range, "heroes.vision_range")?,
                    abilities: hero.abilities,
                    health_per_level: hero.health_per_level.unwrap_or(crate::LEVEL_GROWTH.health),
                    mana_per_level: hero.mana_per_level.unwrap_or(crate::LEVEL_GROWTH.mana),
                    attack_damage_per_level: hero
                        .attack_damage_per_level
                        .unwrap_or(crate::LEVEL_GROWTH.attack_damage),
                    armor_per_level: hero.armor_per_level.unwrap_or(crate::LEVEL_GROWTH.armor),
                    magic_resist_per_level: hero
                        .magic_resist_per_level
                        .unwrap_or(crate::LEVEL_GROWTH.magic_resist),
                    model: hero.model,
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        None => GameContent::default().heroes,
    };
    let modifiers = match file.modifiers {
        Some(modifiers) => modifiers
            .into_iter()
            .map(|modifier| {
                Ok(ModifierDef {
                    name: modifier.name,
                    kind: parse_modifier_kind(&modifier.kind)?,
                    bonus_armor: modifier.bonus_armor,
                    bonus_attack_damage: modifier.bonus_attack_damage,
                    on_expire_damage: modifier.on_expire_damage,
                    on_damaged_damage: modifier.on_damaged_damage,
                    on_tick_damage: modifier.on_tick_damage,
                    status_bits: modifier.status_bits,
                })
            })
            .collect::<Result<Vec<_>, ContentLoadError>>()?,
        None => GameContent::default().modifiers,
    };
    for required in [
        ModifierKind::Shield,
        ModifierKind::Slow,
        ModifierKind::Haste,
        ModifierKind::Root,
        ModifierKind::Silence,
        ModifierKind::Burn,
    ] {
        if !modifiers.iter().any(|modifier| modifier.kind == required) {
            return Err(invalid_field(
                "modifiers.kind",
                "missing required modifier kind",
            ));
        }
    }
    if let Some(abilities) = &file.abilities {
        for ability in abilities {
            if let Some(kind) = ability.on_hit.as_deref() {
                let kind = parse_modifier_kind(kind)?;
                if !modifiers.iter().any(|modifier| modifier.kind == kind) {
                    return Err(invalid_field(
                        "abilities.on_hit",
                        ability.on_hit.clone().unwrap_or_default(),
                    ));
                }
            }
        }
    }
    let abilities = match file.abilities {
        Some(abilities) => abilities
            .into_iter()
            .map(|ability| {
                let damage_type = parse_damage_type(&ability.damage_type)?;
                let cast_range = fix_param(ability.cast_range, "abilities.cast_range")?;
                let radius = fix_param(ability.radius, "abilities.radius")?;
                let factor = fix_param(ability.factor, "abilities.factor")?;
                let modifier_id = |kind: ModifierKind| {
                    modifiers
                        .iter()
                        .position(|definition| definition.kind == kind)
                        .and_then(|index| u16::try_from(index).ok())
                        .expect("modifier kinds validated before ability construction")
                };
                let apply = |kind: ModifierKind, duration_ticks: u32, amount: i32| {
                    AreaEffect::ApplyModifier {
                        modifier: modifier_id(kind),
                        duration_ticks,
                        factor,
                        amount,
                        damage_type,
                    }
                };
                let (targeting, effects) = match ability.kind.as_str() {
                    "skillshot" => {
                        let mut on_hit = AreaEffectList::one(AreaEffect::Damage {
                            amount: ability.damage,
                            damage_type,
                        });
                        if let Some(status) = ability.on_hit.as_deref() {
                            let kind = parse_modifier_kind(status)?;
                            let duration = if ability.on_hit_duration_ticks == 0 {
                                ability.duration_ticks
                            } else {
                                ability.on_hit_duration_ticks
                            };
                            on_hit.effects[1] = AreaEffect::ApplyModifier {
                                modifier: modifier_id(kind),
                                duration_ticks: duration,
                                factor: Fix::ONE,
                                amount: 0,
                                damage_type,
                            };
                        }
                        (
                            Targeting::Direction { range: cast_range },
                            EffectList::one(Effect::Projectile {
                                speed_per_sec: fix_param(ability.speed, "abilities.speed")?,
                                range: fix_param(ability.range, "abilities.range")?,
                                radius,
                                arc: fix_param(ability.arc, "abilities.arc")?,
                                on_hit,
                            }),
                        )
                    }
                    "dash" => (
                        Targeting::Direction { range: cast_range },
                        EffectList::one(Effect::Displace {
                            range: fix_param(ability.range, "abilities.range")?,
                        }),
                    ),
                    "heal" => (
                        Targeting::SelfCast,
                        EffectList::one(Effect::Heal {
                            amount: ability.amount,
                        }),
                    ),
                    "haste" => (
                        Targeting::SelfCast,
                        EffectList::one(Effect::ApplyModifier {
                            modifier: modifier_id(ModifierKind::Haste),
                            duration_ticks: ability.duration_ticks,
                            factor,
                            amount: 0,
                            damage_type,
                        }),
                    ),
                    "burn" => (
                        Targeting::Point { range: cast_range },
                        EffectList::one(Effect::Area {
                            radius,
                            effects: AreaEffectList::one(apply(
                                ModifierKind::Burn,
                                ability.duration_ticks,
                                ability.damage,
                            )),
                        }),
                    ),
                    "silence" => (
                        Targeting::Point { range: cast_range },
                        EffectList::one(Effect::Area {
                            radius,
                            effects: AreaEffectList::one(apply(
                                ModifierKind::Silence,
                                ability.duration_ticks,
                                0,
                            )),
                        }),
                    ),
                    "root" => (
                        Targeting::Point { range: cast_range },
                        EffectList::one(Effect::Area {
                            radius,
                            effects: AreaEffectList::one(apply(
                                ModifierKind::Root,
                                ability.duration_ticks,
                                0,
                            )),
                        }),
                    ),
                    "slow" => (
                        Targeting::Point { range: cast_range },
                        EffectList::one(Effect::Area {
                            radius,
                            effects: AreaEffectList::one(apply(
                                ModifierKind::Slow,
                                ability.duration_ticks,
                                0,
                            )),
                        }),
                    ),
                    "shield" => (
                        Targeting::SelfCast,
                        EffectList::one(Effect::ApplyModifier {
                            modifier: modifier_id(ModifierKind::Shield),
                            duration_ticks: ability.duration_ticks,
                            factor: Fix::ONE,
                            amount: ability.amount,
                            damage_type,
                        }),
                    ),
                    "burn_zone" => (
                        Targeting::Point { range: cast_range },
                        EffectList::one(Effect::Zone {
                            radius,
                            duration_ticks: ability.duration_ticks,
                            period_ticks: if ability.period_ticks == 0 {
                                30
                            } else {
                                ability.period_ticks
                            },
                            effects: AreaEffectList::one(apply(
                                ModifierKind::Burn,
                                ability.duration_ticks,
                                ability.damage,
                            )),
                        }),
                    ),
                    "burst" => (
                        Targeting::Point { range: cast_range },
                        EffectList::one(Effect::Area {
                            radius,
                            effects: AreaEffectList::one(AreaEffect::Damage {
                                amount: ability.damage,
                                damage_type,
                            }),
                        }),
                    ),
                    other => return Err(invalid_field("abilities.kind", other)),
                };
                Ok::<AbilitySpec, ContentLoadError>(AbilitySpec {
                    name: ability.name,
                    cooldown_ticks: ability.cooldown_ticks,
                    mana_cost: ability.mana_cost,
                    targeting,
                    effects,
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        None => GameContent::default().abilities,
    };
    // An explicit `map.visuals` must name a manifest entry even when it equals the theme
    // default (which is then stored as `None`).
    let mut explicit_visuals: Option<String> = None;
    let map = match file.map {
        Some(map) => {
            let MapEntry {
                name,
                theme,
                blue_spawn,
                red_spawn,
                lanes,
                structures,
                walls,
                extent,
                visuals,
            } = map;
            let theme =
                MapTheme::from_name(&theme).ok_or_else(|| invalid_field("map.theme", &theme))?;
            let structures = structures
                .into_iter()
                .map(|structure| {
                    Ok::<StructureSpec, ContentLoadError>(StructureSpec {
                        team: parse_team(&structure.team)?,
                        position: FixVec2::from_ints(structure.position[0], structure.position[1]),
                        kind: parse_structure_kind(&structure.kind)?,
                        lane: structure.lane,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            explicit_visuals.clone_from(&visuals);
            MapSpec {
                name,
                theme,
                // Canonical form: naming the theme's own default is the same as omitting it,
                // so it hashes (and crosses the wire) identically.
                visuals: visuals.filter(|name| name != theme.asset_name()),
                extent: Fix::from_i32(extent),
                blue_spawn: FixVec2::from_ints(blue_spawn[0], blue_spawn[1]),
                red_spawn: FixVec2::from_ints(red_spawn[0], red_spawn[1]),
                lanes: lanes
                    .into_iter()
                    .map(|lane| LaneSpec {
                        name: lane.name,
                        path: lane
                            .path
                            .into_iter()
                            .map(|point| FixVec2::from_ints(point[0], point[1]))
                            .collect(),
                    })
                    .collect(),
                structures,
                walls: walls
                    .into_iter()
                    .map(|wall| WallSpec {
                        a: FixVec2::from_ints(wall.a[0], wall.a[1]),
                        b: FixVec2::from_ints(wall.b[0], wall.b[1]),
                    })
                    .collect(),
            }
        }
        None => GameContent::default().map,
    };
    let rules = match file.rules {
        Some(rules) => {
            let minion = |stats: MinionStatsEntry,
                          field: &'static str|
             -> Result<MinionStatsSpec, ContentLoadError> {
                Ok(MinionStatsSpec {
                    max_health: stats.max_health,
                    attack_damage: stats.attack_damage,
                    attack_range: fix_param(stats.attack_range, field)?,
                    attack_interval_ticks: stats.attack_interval_ticks,
                    move_speed_per_sec: fix_param(stats.move_speed_per_sec, field)?,
                    armor: stats.armor,
                    magic_resist: stats.magic_resist,
                    vision_range: fix_param(stats.vision_range, field)?,
                })
            };
            let structure = |stats: StructureStatsEntry,
                             field: &'static str|
             -> Result<StructureStatsSpec, ContentLoadError> {
                Ok(StructureStatsSpec {
                    max_health: stats.max_health,
                    attack_damage: stats.attack_damage,
                    attack_range: fix_param(stats.attack_range, field)?,
                    attack_interval_ticks: stats.attack_interval_ticks,
                    armor: stats.armor,
                    magic_resist: stats.magic_resist,
                    vision_range: fix_param(stats.vision_range, field)?,
                })
            };
            let melee_minion = minion(rules.melee_minion.into(), "rules.melee_minion")?;
            let ranged_minion = minion(rules.ranged_minion.into(), "rules.ranged_minion")?;
            let tower = structure(rules.tower.into(), "rules.tower")?;
            let nexus = structure(rules.nexus.into(), "rules.nexus")?;
            RulesSpec {
                melee_vision: melee_minion.vision_range,
                ranged_vision: ranged_minion.vision_range,
                tower_vision: tower.vision_range,
                nexus_vision: nexus.vision_range,
                melee_minion,
                ranged_minion,
                tower,
                nexus,
                level_cap: rules.level_cap,
                xp_base: rules.xp_base,
                xp_per_level: rules.xp_per_level,
                team_size: rules.team_size,
                bot_fill: rules.bot_fill,
                hero_respawn_ticks: rules.hero_respawn_ticks,
                respawn_per_level: rules.respawn_per_level,
                wave_interval_ticks: rules.wave_interval_ticks,
                first_wave_ticks: rules.first_wave_ticks,
                starting_gold: rules.starting_gold,
                gold_per_interval: rules.gold_per_interval,
                gold_tick_interval: rules.gold_tick_interval,
                xp_radius: fix_param(rules.xp_radius, "rules.xp_radius")?,
                shop_radius: fix_param(rules.shop_radius, "rules.shop_radius")?,
                shared_xp_percent: rules.shared_xp_percent,
                rewards: RewardsSpec {
                    melee_minion: UnitRewardsSpec {
                        xp: rules.rewards.melee_minion.xp,
                        gold: rules.rewards.melee_minion.gold,
                    },
                    ranged_minion: UnitRewardsSpec {
                        xp: rules.rewards.ranged_minion.xp,
                        gold: rules.rewards.ranged_minion.gold,
                    },
                    tower: UnitRewardsSpec {
                        xp: rules.rewards.tower.xp,
                        gold: rules.rewards.tower.gold,
                    },
                    nexus: UnitRewardsSpec {
                        xp: rules.rewards.nexus.xp,
                        gold: rules.rewards.nexus.gold,
                    },
                    hero: UnitRewardsSpec {
                        xp: rules.rewards.hero.xp,
                        gold: rules.rewards.hero.gold,
                    },
                },
                structure_heroes_first: rules.structure_heroes_first,
                nexus_win: rules.nexus_win,
                score_limit: rules.score_limit,
                recall_ticks: rules.recall_ticks,
            }
        }
        None => RulesSpec::default(),
    };
    let attestation = match (file.publisher.as_deref(), file.signature.as_deref()) {
        (None, None) => None,
        (Some(publisher), Some(signature)) => Some(PackAttestation {
            publisher: parse_hex_array(publisher)
                .map_err(|()| invalid_field("publisher", publisher))?,
            signature: parse_hex_array(signature)
                .map_err(|()| invalid_field("signature", signature))?,
        }),
        _ => {
            return Err(invalid_field(
                "attestation",
                "publisher and signature are both required",
            ));
        }
    };
    let defaults = ContentManifest::default();
    let manifest = ContentManifest {
        name: file.name.unwrap_or(defaults.name),
        version: file.version.unwrap_or(defaults.version),
        author: file.author.unwrap_or(defaults.author),
    };
    let assets = file.assets.unwrap_or_default();
    assets.validate().map_err(ContentLoadError::Assets)?;
    let content = GameContent {
        manifest,
        items,
        heroes,
        abilities,
        modifiers,
        rules,
        map,
        attestation,
        assets,
    };
    content.validate().map_err(ContentLoadError::Invalid)?;
    check_asset_references(&content, explicit_visuals.as_deref())?;
    Ok(content)
}

/// Every visual name a pack declares must have an entry in its own `[assets]` manifest.
///
/// This is pack-local: a client reconstructing content from the wire has no manifest and
/// resolves the names against its own asset root instead. The theme-default `map.visuals`
/// (none given) needs no entry, because clients fall back to the built-in theme visuals.
fn check_asset_references(
    content: &GameContent,
    explicit_visuals: Option<&str>,
) -> Result<(), ContentLoadError> {
    for hero in &content.heroes {
        if let Some(model) = hero.model.as_deref()
            && content.assets.unit(model).is_none()
        {
            return Err(ContentLoadError::UnknownAsset {
                field: "heroes.model",
                name: model.to_owned(),
            });
        }
    }
    if let Some(visuals) = explicit_visuals
        && content.assets.map(visuals).is_none()
    {
        return Err(ContentLoadError::UnknownAsset {
            field: "map.visuals",
            name: visuals.to_owned(),
        });
    }
    Ok(())
}

/// Reads, parses and validates a content pack.
///
/// # Errors
/// Returns [`ContentLoadError`] for I/O, syntax or validation failures.
pub fn load_file(path: &Path) -> Result<GameContent, ContentLoadError> {
    let text = std::fs::read_to_string(path).map_err(ContentLoadError::Io)?;
    load_toml(&text)
}
