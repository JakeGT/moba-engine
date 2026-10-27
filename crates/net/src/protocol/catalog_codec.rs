//! Catalogue byte encoding and the `ContentUpdate` payload.

use super::WireError;
use super::bytes::{Reader, put_u32};
use super::catalog::{
    MAX_WIRE_AREA_EFFECTS, MAX_WIRE_EFFECTS, WIRE_ABILITY_NAME, WIRE_HERO_NAME, WIRE_ITEM_NAME,
    WIRE_LANE_NAME, WIRE_MANIFEST_FIELD, WIRE_MODIFIER_NAME, WIRE_VISUAL_NAME, WireCatalog,
    WireMinionStats, WireRewards, WireRules, WireStructureStats, WireUnitRewards,
};

#[allow(clippy::too_many_lines)] // Wire field order mirrors `decode_catalog`.
/// Encodes a full catalogue update payload.
pub fn encode_content_update(catalog: &WireCatalog, content_hash: u64, out: &mut Vec<u8>) {
    out.extend_from_slice(&content_hash.to_le_bytes());
    encode_catalog(catalog, out);
}

/// Decodes and validates a full catalogue update payload.
pub fn decode_content_update(data: &[u8]) -> Result<(WireCatalog, u64), WireError> {
    let mut reader = Reader::new(data);
    let content_hash = reader.u64()?;
    let catalog = decode_catalog(&mut reader)?;
    if reader.remaining() != 0 {
        return Err(WireError::BadValue);
    }
    if catalog.content_hash()? != content_hash {
        return Err(WireError::BadValue);
    }
    Ok((catalog, content_hash))
}

#[allow(clippy::too_many_lines)] // Wire field order mirrors the catalogue schema.
pub(super) fn encode_catalog(catalog: &WireCatalog, out: &mut Vec<u8>) {
    out.extend_from_slice(&catalog.manifest_name);
    out.push(catalog.manifest_name_len);
    out.extend_from_slice(&catalog.manifest_version);
    out.push(catalog.manifest_version_len);
    out.extend_from_slice(&catalog.manifest_author);
    out.push(catalog.manifest_author_len);
    out.push(catalog.item_count);
    for item in &catalog.items {
        out.extend_from_slice(&item.name);
        out.push(item.name_len);
        put_u32(out, item.cost);
        put_u32(out, item.health as u32);
        put_u32(out, item.mana as u32);
        put_u32(out, item.attack_damage as u32);
        put_u32(out, item.armor as u32);
        put_u32(out, item.magic_resist as u32);
        out.extend_from_slice(&item.move_speed_bits.to_le_bytes());
        out.extend_from_slice(&item.on_hit_damage.to_le_bytes());
        out.extend_from_slice(&item.on_damaged_damage.to_le_bytes());
        put_u32(out, item.active_heal as u32);
        out.extend_from_slice(&item.active_cooldown_ticks.to_le_bytes());
    }
    out.push(catalog.hero_count);
    for hero in &catalog.heroes {
        out.extend_from_slice(&hero.name);
        out.push(hero.name_len);
        for value in [
            hero.max_health,
            hero.max_mana,
            hero.health_regen_per_tick,
            hero.mana_regen_per_tick,
            hero.attack_damage,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&hero.attack_range_bits.to_le_bytes());
        out.extend_from_slice(&hero.attack_interval_ticks.to_le_bytes());
        out.extend_from_slice(&hero.move_speed_bits.to_le_bytes());
        out.extend_from_slice(&hero.armor.to_le_bytes());
        out.extend_from_slice(&hero.magic_resist.to_le_bytes());
        out.extend_from_slice(&hero.vision_bits.to_le_bytes());
        out.extend_from_slice(&hero.abilities);
        for value in [
            hero.health_per_level,
            hero.mana_per_level,
            hero.attack_damage_per_level,
            hero.armor_per_level,
            hero.magic_resist_per_level,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&hero.model);
        out.push(hero.model_len);
    }
    out.push(catalog.ability_count);
    for ability in &catalog.abilities {
        out.extend_from_slice(&ability.name);
        out.push(ability.name_len);
        out.extend_from_slice(&ability.cooldown_ticks.to_le_bytes());
        out.extend_from_slice(&ability.mana_cost.to_le_bytes());
        out.extend_from_slice(&ability.cast_range_bits.to_le_bytes());
        out.extend_from_slice(&ability.radius_bits.to_le_bytes());
        out.push(ability.targeting_kind);
        out.extend_from_slice(&ability.targeting_range_bits.to_le_bytes());
        out.push(ability.effect_count);
        for effect in ability
            .effects
            .iter()
            .take(usize::from(ability.effect_count))
        {
            out.push(effect.kind);
            out.extend_from_slice(&effect.amount.to_le_bytes());
            out.push(effect.damage_type);
            out.extend_from_slice(&effect.speed_bits.to_le_bytes());
            out.extend_from_slice(&effect.range_bits.to_le_bytes());
            out.extend_from_slice(&effect.radius_bits.to_le_bytes());
            out.extend_from_slice(&effect.arc_bits.to_le_bytes());
            out.extend_from_slice(&effect.duration_ticks.to_le_bytes());
            out.extend_from_slice(&effect.period_ticks.to_le_bytes());
            out.push(effect.area_count);
            for area in effect.area.iter().take(usize::from(effect.area_count)) {
                out.push(area.kind);
                out.extend_from_slice(&area.modifier.to_le_bytes());
                out.extend_from_slice(&area.duration_ticks.to_le_bytes());
                out.extend_from_slice(&area.factor_bits.to_le_bytes());
                out.extend_from_slice(&area.amount.to_le_bytes());
                out.push(area.damage_type);
            }
        }
    }
    out.push(catalog.modifier_count);
    for modifier in &catalog.modifiers {
        out.extend_from_slice(&modifier.name);
        out.push(modifier.name_len);
        out.push(modifier.kind);
        put_u32(out, modifier.bonus_armor as u32);
        put_u32(out, modifier.bonus_attack_damage as u32);
        put_u32(out, modifier.on_expire_damage as u32);
        put_u32(out, modifier.on_damaged_damage as u32);
        put_u32(out, modifier.on_tick_damage as u32);
        out.extend_from_slice(&modifier.status_bits.to_le_bytes());
    }
    for minion in [catalog.rules.melee_minion, catalog.rules.ranged_minion] {
        out.extend_from_slice(&minion.max_health.to_le_bytes());
        out.extend_from_slice(&minion.attack_damage.to_le_bytes());
        out.extend_from_slice(&minion.attack_range_bits.to_le_bytes());
        out.extend_from_slice(&minion.attack_interval_ticks.to_le_bytes());
        out.extend_from_slice(&minion.move_speed_bits.to_le_bytes());
        out.extend_from_slice(&minion.armor.to_le_bytes());
        out.extend_from_slice(&minion.magic_resist.to_le_bytes());
        out.extend_from_slice(&minion.vision_range_bits.to_le_bytes());
    }
    for structure in [catalog.rules.tower, catalog.rules.nexus] {
        out.extend_from_slice(&structure.max_health.to_le_bytes());
        out.extend_from_slice(&structure.attack_damage.to_le_bytes());
        out.extend_from_slice(&structure.attack_range_bits.to_le_bytes());
        out.extend_from_slice(&structure.attack_interval_ticks.to_le_bytes());
        out.extend_from_slice(&structure.armor.to_le_bytes());
        out.extend_from_slice(&structure.magic_resist.to_le_bytes());
        out.extend_from_slice(&structure.vision_range_bits.to_le_bytes());
    }
    out.push(catalog.rules.level_cap);
    for value in [catalog.rules.xp_base, catalog.rules.xp_per_level] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for value in [
        catalog.rules.melee_vision_bits,
        catalog.rules.ranged_vision_bits,
        catalog.rules.tower_vision_bits,
        catalog.rules.nexus_vision_bits,
    ] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.push(catalog.rules.team_size);
    out.push(u8::from(catalog.rules.bot_fill));
    for value in [
        catalog.rules.hero_respawn_ticks,
        catalog.rules.respawn_per_level,
        catalog.rules.wave_interval_ticks,
        catalog.rules.first_wave_ticks,
        catalog.rules.starting_gold,
        catalog.rules.gold_per_interval,
        catalog.rules.gold_tick_interval,
    ] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for value in [catalog.rules.xp_radius_bits, catalog.rules.shop_radius_bits] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&catalog.rules.shared_xp_percent.to_le_bytes());
    for reward in [
        catalog.rules.rewards.melee_minion,
        catalog.rules.rewards.ranged_minion,
        catalog.rules.rewards.tower,
        catalog.rules.rewards.nexus,
        catalog.rules.rewards.hero,
    ] {
        out.extend_from_slice(&reward.xp.to_le_bytes());
        out.extend_from_slice(&reward.gold.to_le_bytes());
    }
    out.push(u8::from(catalog.rules.structure_heroes_first));
    out.push(u8::from(catalog.rules.nexus_win));
    out.extend_from_slice(&catalog.rules.score_limit.to_le_bytes());
    let map = &catalog.map;
    out.extend_from_slice(&map.name);
    out.push(map.name_len);
    out.push(map.theme);
    out.extend_from_slice(&map.visuals);
    out.push(map.visuals_len);
    out.extend_from_slice(&map.extent.to_le_bytes());
    out.push(map.lane_count);
    for lane in &map.lanes {
        out.extend_from_slice(&lane.name);
        out.push(lane.name_len);
        out.push(lane.point_count);
        for point in &lane.points {
            out.extend_from_slice(&point[0].to_le_bytes());
            out.extend_from_slice(&point[1].to_le_bytes());
        }
    }
    for point in [map.blue_spawn, map.red_spawn] {
        out.extend_from_slice(&point[0].to_le_bytes());
        out.extend_from_slice(&point[1].to_le_bytes());
    }
    out.push(map.structure_count);
    for structure in &map.structures {
        out.push(structure.team);
        out.push(structure.kind);
        out.extend_from_slice(&structure.position[0].to_le_bytes());
        out.extend_from_slice(&structure.position[1].to_le_bytes());
        out.push(structure.lane);
    }
    out.push(map.wall_count);
    for wall in &map.walls {
        for point in [wall.a, wall.b] {
            out.extend_from_slice(&point[0].to_le_bytes());
            out.extend_from_slice(&point[1].to_le_bytes());
        }
    }
}

#[allow(clippy::too_many_lines)] // Wire field order mirrors `encode_catalog`.
pub(super) fn decode_catalog(reader: &mut Reader<'_>) -> Result<WireCatalog, WireError> {
    let mut catalog = WireCatalog::default();
    catalog
        .manifest_name
        .copy_from_slice(reader.bytes(WIRE_MANIFEST_FIELD)?);
    catalog.manifest_name_len = reader.u8()?;
    catalog
        .manifest_version
        .copy_from_slice(reader.bytes(WIRE_MANIFEST_FIELD)?);
    catalog.manifest_version_len = reader.u8()?;
    catalog
        .manifest_author
        .copy_from_slice(reader.bytes(WIRE_MANIFEST_FIELD)?);
    catalog.manifest_author_len = reader.u8()?;
    catalog.item_count = reader.u8()?;
    for item in &mut catalog.items {
        item.name.copy_from_slice(reader.bytes(WIRE_ITEM_NAME)?);
        item.name_len = reader.u8()?;
        item.cost = reader.u32()?;
        item.health = reader.i32()?;
        item.mana = reader.i32()?;
        item.attack_damage = reader.i32()?;
        item.armor = reader.i32()?;
        item.magic_resist = reader.i32()?;
        item.move_speed_bits = reader.i64()?;
        item.on_hit_damage = reader.u16()?;
        item.on_damaged_damage = reader.u16()?;
        item.active_heal = reader.i32()?;
        item.active_cooldown_ticks = reader.u16()?;
    }
    catalog.hero_count = reader.u8()?;
    for hero in &mut catalog.heroes {
        hero.name.copy_from_slice(reader.bytes(WIRE_HERO_NAME)?);
        hero.name_len = reader.u8()?;
        hero.max_health = reader.i32()?;
        hero.max_mana = reader.i32()?;
        hero.health_regen_per_tick = reader.i32()?;
        hero.mana_regen_per_tick = reader.i32()?;
        hero.attack_damage = reader.i32()?;
        hero.attack_range_bits = reader.i64()?;
        hero.attack_interval_ticks = reader.u32()?;
        hero.move_speed_bits = reader.i64()?;
        hero.armor = reader.i32()?;
        hero.magic_resist = reader.i32()?;
        hero.vision_bits = reader.i64()?;
        hero.abilities.copy_from_slice(reader.bytes(3)?);
        hero.health_per_level = reader.i32()?;
        hero.mana_per_level = reader.i32()?;
        hero.attack_damage_per_level = reader.i32()?;
        hero.armor_per_level = reader.i32()?;
        hero.magic_resist_per_level = reader.i32()?;
        hero.model.copy_from_slice(reader.bytes(WIRE_VISUAL_NAME)?);
        hero.model_len = reader.u8()?;
    }
    catalog.ability_count = reader.u8()?;
    for ability in &mut catalog.abilities {
        ability
            .name
            .copy_from_slice(reader.bytes(WIRE_ABILITY_NAME)?);
        ability.name_len = reader.u8()?;
        ability.cooldown_ticks = reader.u32()?;
        ability.mana_cost = reader.i32()?;
        ability.cast_range_bits = reader.i64()?;
        ability.radius_bits = reader.i64()?;
        ability.targeting_kind = reader.u8()?;
        ability.targeting_range_bits = reader.i64()?;
        ability.effect_count = reader.u8()?;
        if usize::from(ability.effect_count) > MAX_WIRE_EFFECTS {
            return Err(WireError::BadValue);
        }
        for index in 0..usize::from(ability.effect_count) {
            let effect = &mut ability.effects[index];
            effect.kind = reader.u8()?;
            effect.amount = reader.i32()?;
            effect.damage_type = reader.u8()?;
            effect.speed_bits = reader.i64()?;
            effect.range_bits = reader.i64()?;
            effect.radius_bits = reader.i64()?;
            effect.arc_bits = reader.i64()?;
            effect.duration_ticks = reader.u32()?;
            effect.period_ticks = reader.u32()?;
            effect.area_count = reader.u8()?;
            if usize::from(effect.area_count) > MAX_WIRE_AREA_EFFECTS {
                return Err(WireError::BadValue);
            }
            for area_index in 0..usize::from(effect.area_count) {
                let area = &mut effect.area[area_index];
                area.kind = reader.u8()?;
                area.modifier = reader.u16()?;
                area.duration_ticks = reader.u32()?;
                area.factor_bits = reader.i64()?;
                area.amount = reader.i32()?;
                area.damage_type = reader.u8()?;
            }
        }
    }
    catalog.modifier_count = reader.u8()?;
    for modifier in &mut catalog.modifiers {
        modifier
            .name
            .copy_from_slice(reader.bytes(WIRE_MODIFIER_NAME)?);
        modifier.name_len = reader.u8()?;
        modifier.kind = reader.u8()?;
        modifier.bonus_armor = reader.i32()?;
        modifier.bonus_attack_damage = reader.i32()?;
        modifier.on_expire_damage = reader.i32()?;
        modifier.on_damaged_damage = reader.i32()?;
        modifier.on_tick_damage = reader.i32()?;
        modifier.status_bits = reader.u16()?;
    }
    let mut stats = [WireMinionStats::default(); 2];
    for minion in &mut stats {
        minion.max_health = reader.i32()?;
        minion.attack_damage = reader.i32()?;
        minion.attack_range_bits = reader.i64()?;
        minion.attack_interval_ticks = reader.u32()?;
        minion.move_speed_bits = reader.i64()?;
        minion.armor = reader.i32()?;
        minion.magic_resist = reader.i32()?;
        minion.vision_range_bits = reader.i64()?;
    }
    let mut structures = [WireStructureStats::default(); 2];
    for structure in &mut structures {
        structure.max_health = reader.i32()?;
        structure.attack_damage = reader.i32()?;
        structure.attack_range_bits = reader.i64()?;
        structure.attack_interval_ticks = reader.u32()?;
        structure.armor = reader.i32()?;
        structure.magic_resist = reader.i32()?;
        structure.vision_range_bits = reader.i64()?;
    }
    catalog.rules = WireRules {
        melee_minion: stats[0],
        ranged_minion: stats[1],
        tower: structures[0],
        nexus: structures[1],
        level_cap: reader.u8()?,
        xp_base: reader.u32()?,
        xp_per_level: reader.u32()?,
        melee_vision_bits: reader.i64()?,
        ranged_vision_bits: reader.i64()?,
        tower_vision_bits: reader.i64()?,
        nexus_vision_bits: reader.i64()?,
        team_size: reader.u8()?,
        bot_fill: reader.u8()? != 0,
        hero_respawn_ticks: reader.u32()?,
        respawn_per_level: reader.u32()?,
        wave_interval_ticks: reader.u32()?,
        first_wave_ticks: reader.u32()?,
        starting_gold: reader.u32()?,
        gold_per_interval: reader.u32()?,
        gold_tick_interval: reader.u32()?,
        xp_radius_bits: reader.i64()?,
        shop_radius_bits: reader.i64()?,
        shared_xp_percent: reader.u32()?,
        rewards: WireRewards {
            melee_minion: WireUnitRewards {
                xp: reader.u32()?,
                gold: reader.u32()?,
            },
            ranged_minion: WireUnitRewards {
                xp: reader.u32()?,
                gold: reader.u32()?,
            },
            tower: WireUnitRewards {
                xp: reader.u32()?,
                gold: reader.u32()?,
            },
            nexus: WireUnitRewards {
                xp: reader.u32()?,
                gold: reader.u32()?,
            },
            hero: WireUnitRewards {
                xp: reader.u32()?,
                gold: reader.u32()?,
            },
        },
        structure_heroes_first: reader.u8()? != 0,
        nexus_win: reader.u8()? != 0,
        score_limit: reader.u16()?,
    };
    let map = &mut catalog.map;
    map.name.copy_from_slice(reader.bytes(WIRE_MANIFEST_FIELD)?);
    map.name_len = reader.u8()?;
    map.theme = reader.u8()?;
    map.visuals.copy_from_slice(reader.bytes(WIRE_VISUAL_NAME)?);
    map.visuals_len = reader.u8()?;
    map.extent = reader.u16()?;
    map.lane_count = reader.u8()?;
    for lane in &mut map.lanes {
        lane.name.copy_from_slice(reader.bytes(WIRE_LANE_NAME)?);
        lane.name_len = reader.u8()?;
        lane.point_count = reader.u8()?;
        for point in &mut lane.points {
            point[0] = i16::from_le_bytes(
                reader
                    .bytes(2)?
                    .try_into()
                    .map_err(|_| WireError::Truncated)?,
            );
            point[1] = i16::from_le_bytes(
                reader
                    .bytes(2)?
                    .try_into()
                    .map_err(|_| WireError::Truncated)?,
            );
        }
    }
    for point in [&mut map.blue_spawn, &mut map.red_spawn] {
        point[0] = i16::from_le_bytes(
            reader
                .bytes(2)?
                .try_into()
                .map_err(|_| WireError::Truncated)?,
        );
        point[1] = i16::from_le_bytes(
            reader
                .bytes(2)?
                .try_into()
                .map_err(|_| WireError::Truncated)?,
        );
    }
    map.structure_count = reader.u8()?;
    for structure in &mut map.structures {
        structure.team = reader.u8()?;
        structure.kind = reader.u8()?;
        structure.position[0] = i16::from_le_bytes(
            reader
                .bytes(2)?
                .try_into()
                .map_err(|_| WireError::Truncated)?,
        );
        structure.position[1] = i16::from_le_bytes(
            reader
                .bytes(2)?
                .try_into()
                .map_err(|_| WireError::Truncated)?,
        );
        structure.lane = reader.u8()?;
    }
    map.wall_count = reader.u8()?;
    for wall in &mut map.walls {
        for point in [&mut wall.a, &mut wall.b] {
            point[0] = i16::from_le_bytes(
                reader
                    .bytes(2)?
                    .try_into()
                    .map_err(|_| WireError::Truncated)?,
            );
            point[1] = i16::from_le_bytes(
                reader
                    .bytes(2)?
                    .try_into()
                    .map_err(|_| WireError::Truncated)?,
            );
        }
    }
    Ok(catalog)
}
