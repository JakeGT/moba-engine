//! Semantic validation: [`GameContent::validate`] and [`RulesSpec::invalid_field`].
//!
//! Range checks for every content table live here so new schema fields (and map-coordinate
//! checks) have one place to land. Validation never affects the content hash.

use moba_core::{Fix, FixVec2};

use crate::{AreaEffect, AreaEffectList, DamageType, Effect};

use super::{
    ContentError, GameContent, LaneSpec, MAX_ABILITY_NAME, MAX_CONTENT_ABILITIES,
    MAX_CONTENT_HEROES, MAX_CONTENT_ITEMS, MAX_CONTENT_LANES, MAX_CONTENT_MODIFIERS,
    MAX_CONTENT_STRUCTURES, MAX_CONTENT_WALLS, MAX_COOLDOWN_TICKS, MAX_DURATION_TICKS,
    MAX_HASTE_FACTOR, MAX_HERO_NAME, MAX_ITEM_NAME, MAX_ITEM_STAT, MAX_LANE_NAME, MAX_LANE_POINTS,
    MAX_MANIFEST_FIELD, MAX_MAP_COORD, MAX_RANGE, MAX_RESIST, MAX_SPEED_PER_SEC, MAX_STAT, MapSpec,
    MinionStatsSpec, ModifierDef, ModifierKind, RulesSpec, StructureSpec, StructureStatsSpec,
    WallSpec,
};

fn area_effect_valid(effect: &AreaEffect, modifiers: &[ModifierDef]) -> bool {
    match effect {
        AreaEffect::None => true,
        AreaEffect::Damage { amount, .. } => (0..=MAX_STAT).contains(amount),
        AreaEffect::ApplyModifier {
            modifier,
            duration_ticks,
            factor,
            amount,
            ..
        } => {
            let Some(definition) = modifiers.get(usize::from(*modifier)) else {
                return false;
            };
            if !(1..=MAX_DURATION_TICKS).contains(duration_ticks) {
                return false;
            }
            match definition.kind {
                ModifierKind::Shield => (0..=MAX_STAT).contains(amount),
                ModifierKind::Slow => *factor > Fix::ZERO && *factor < Fix::ONE,
                ModifierKind::Haste => {
                    (Fix::ONE..=Fix::from_i32(MAX_HASTE_FACTOR)).contains(factor)
                }
                ModifierKind::Burn => (1..=MAX_STAT).contains(amount),
                ModifierKind::Root | ModifierKind::Silence | ModifierKind::Stun => true,
            }
        }
    }
}

fn area_effects_valid(effects: &AreaEffectList, modifiers: &[ModifierDef]) -> bool {
    !effects.is_empty()
        && effects
            .iter()
            .all(|effect| area_effect_valid(effect, modifiers))
}

fn effect_valid(effect: &Effect, modifiers: &[ModifierDef]) -> bool {
    let in_range = |value: Fix| value > Fix::ZERO && value <= Fix::from_i32(MAX_RANGE);
    match effect {
        Effect::None => false,
        Effect::Damage { amount, .. } | Effect::Heal { amount } => (0..=MAX_STAT).contains(amount),
        Effect::ApplyModifier {
            modifier,
            duration_ticks,
            factor,
            amount,
            ..
        } => area_effect_valid(
            &AreaEffect::ApplyModifier {
                modifier: *modifier,
                duration_ticks: *duration_ticks,
                factor: *factor,
                amount: *amount,
                damage_type: DamageType::Physical,
            },
            modifiers,
        ),
        Effect::Displace { range } => in_range(*range),
        Effect::Projectile {
            speed_per_sec,
            range,
            radius,
            arc,
            on_hit,
        } => {
            in_range(*speed_per_sec)
                && in_range(*range)
                && in_range(*radius)
                && *arc >= Fix::ZERO
                && *arc <= Fix::from_i32(MAX_RANGE)
                && area_effects_valid(on_hit, modifiers)
        }
        Effect::Area { radius, effects } => {
            in_range(*radius) && area_effects_valid(effects, modifiers)
        }
        Effect::Zone {
            radius,
            duration_ticks,
            period_ticks,
            effects,
        } => {
            in_range(*radius)
                && (1..=MAX_DURATION_TICKS).contains(duration_ticks)
                && (1..=MAX_DURATION_TICKS).contains(period_ticks)
                && area_effects_valid(effects, modifiers)
        }
    }
}

/// Validates map limits, names and geometry.
///
/// Structural guard: every map struct is destructured without `..`, so adding map geometry
/// fails to compile here until it is validated. Every coordinate must first pass
/// [`MapSpec::wire_point`] / [`MapSpec::wire_extent`] — the same conversions
/// `From<&GameContent> for WireCatalog` uses — so validated geometry crosses the wire exactly
/// and the client reconstructs the server's content hash (#296).
fn validate_map(map: &MapSpec) -> Result<(), ContentError> {
    let MapSpec {
        name,
        // A closed enum; every variant has a wire id.
        theme: _,
        extent,
        lanes,
        structures,
        walls,
        blue_spawn,
        red_spawn,
    } = map;
    let point = |field: &'static str, point: FixVec2| {
        MapSpec::wire_point(point)
            .map(|_| ())
            .ok_or(ContentError::BadMapCoordinate(field))
    };
    MapSpec::wire_extent(*extent).ok_or(ContentError::BadMapCoordinate("map.extent"))?;
    point("map.blue_spawn", *blue_spawn)?;
    point("map.red_spawn", *red_spawn)?;
    for lane in lanes {
        let LaneSpec { name: _, path } = lane;
        for waypoint in path {
            point("map.lanes.path", *waypoint)?;
        }
    }
    for structure in structures {
        let StructureSpec {
            team: _,
            position,
            kind: _,
            lane: _,
        } = structure;
        point("map.structures.position", *position)?;
    }
    for wall in walls {
        let WallSpec { a, b } = wall;
        point("map.walls.a", *a)?;
        point("map.walls.b", *b)?;
    }

    if *extent <= Fix::ZERO || *extent > Fix::from_i32(MAX_MAP_COORD) {
        return Err(ContentError::BadMap);
    }
    let extent = *extent;
    let in_world = |point: FixVec2| {
        point.x >= Fix::ZERO && point.x <= extent && point.y >= Fix::ZERO && point.y <= extent
    };
    if !in_world(*blue_spawn)
        || !in_world(*red_spawn)
        || lanes
            .iter()
            .any(|lane| lane.path.iter().any(|point| !in_world(*point)))
        || structures
            .iter()
            .any(|structure| !in_world(structure.position))
    {
        return Err(ContentError::BadMapData);
    }
    if walls.len() > MAX_CONTENT_WALLS
        || walls
            .iter()
            .any(|wall| wall.a == wall.b || !in_world(wall.a) || !in_world(wall.b))
    {
        return Err(ContentError::BadWalls);
    }
    if structures.iter().any(|structure| {
        structure
            .lane
            .is_some_and(|lane| usize::from(lane) >= lanes.len())
    }) {
        return Err(ContentError::BadMapData);
    }
    // Counts the wire catalogue would otherwise silently truncate. An empty lane path is
    // rejected too: lane walkers index the first and last waypoint.
    if lanes.len() > MAX_CONTENT_LANES
        || structures.len() > MAX_CONTENT_STRUCTURES
        || lanes
            .iter()
            .any(|lane| lane.path.is_empty() || lane.path.len() > MAX_LANE_POINTS)
    {
        return Err(ContentError::BadMap);
    }
    // Names the wire catalogue would otherwise truncate (possibly mid-UTF-8 sequence).
    if name.len() > MAX_MANIFEST_FIELD
        || !name.is_ascii()
        || lanes.iter().any(|lane| {
            lane.name.is_empty() || lane.name.len() > MAX_LANE_NAME || !lane.name.is_ascii()
        })
    {
        return Err(ContentError::BadMapData);
    }
    Ok(())
}

impl RulesSpec {
    /// Returns the first invalid rules field, if any.
    #[allow(clippy::too_many_lines)] // One ordered validation pass.
    pub fn invalid_field(&self) -> Option<&'static str> {
        let range = Fix::from_i32(MAX_RANGE);
        let speed = Fix::from_i32(MAX_SPEED_PER_SEC);
        let stats = |stats: &MinionStatsSpec, field: &'static str| {
            if !(1..=MAX_STAT).contains(&stats.max_health)
                || !(0..=MAX_STAT).contains(&stats.attack_damage)
                || !(Fix::ZERO..=range).contains(&stats.attack_range)
                || !(1..=MAX_COOLDOWN_TICKS).contains(&stats.attack_interval_ticks)
                || !(Fix::from_bits(1)..=speed).contains(&stats.move_speed_per_sec)
                || stats.armor.unsigned_abs() > MAX_RESIST as u32
                || stats.magic_resist.unsigned_abs() > MAX_RESIST as u32
                || !(Fix::from_bits(1)..=range).contains(&stats.vision_range)
            {
                Some(field)
            } else {
                None
            }
        };
        let structure = |stats: &StructureStatsSpec, field: &'static str| {
            if !(1..=MAX_STAT).contains(&stats.max_health)
                || !(0..=MAX_STAT).contains(&stats.attack_damage)
                || !(Fix::ZERO..=range).contains(&stats.attack_range)
                || !(1..=MAX_COOLDOWN_TICKS).contains(&stats.attack_interval_ticks)
                || stats.armor.unsigned_abs() > MAX_RESIST as u32
                || stats.magic_resist.unsigned_abs() > MAX_RESIST as u32
                || !(Fix::from_bits(1)..=range).contains(&stats.vision_range)
            {
                Some(field)
            } else {
                None
            }
        };
        if !(1..=5).contains(&self.team_size) {
            return Some("team_size");
        }
        if !(1..=99).contains(&self.level_cap) {
            return Some("level_cap");
        }
        if self.xp_base == 0 {
            return Some("xp_base");
        }
        if self.xp_per_level == 0 {
            return Some("xp_per_level");
        }
        if self.hero_respawn_ticks == 0 {
            return Some("hero_respawn_ticks");
        }
        if self.wave_interval_ticks == 0 {
            return Some("wave_interval_ticks");
        }
        if self.first_wave_ticks == 0 {
            return Some("first_wave_ticks");
        }
        if self.respawn_per_level > 1_000 {
            return Some("respawn_per_level");
        }
        if self.starting_gold > 1_000_000 {
            return Some("starting_gold");
        }
        if self.gold_per_interval > 1_000 {
            return Some("gold_per_interval");
        }
        if self.gold_tick_interval == 0 || self.gold_tick_interval > 10_000 {
            return Some("gold_tick_interval");
        }
        if !(Fix::ZERO..=range).contains(&self.xp_radius) || self.xp_radius == Fix::ZERO {
            return Some("xp_radius");
        }
        if !(Fix::ZERO..=range).contains(&self.shop_radius) || self.shop_radius == Fix::ZERO {
            return Some("shop_radius");
        }
        if !(100..=300).contains(&self.shared_xp_percent) {
            return Some("shared_xp_percent");
        }
        for reward in [
            self.rewards.melee_minion,
            self.rewards.ranged_minion,
            self.rewards.tower,
            self.rewards.nexus,
            self.rewards.hero,
        ] {
            if reward.xp > 100_000 || reward.gold > 100_000 {
                return Some("rewards");
            }
        }
        if self.score_limit > 1_000 {
            return Some("score_limit");
        }
        if self.melee_vision != self.melee_minion.vision_range {
            return Some("melee_vision");
        }
        if self.ranged_vision != self.ranged_minion.vision_range {
            return Some("ranged_vision");
        }
        if self.tower_vision != self.tower.vision_range {
            return Some("tower_vision");
        }
        if self.nexus_vision != self.nexus.vision_range {
            return Some("nexus_vision");
        }
        stats(&self.melee_minion, "melee_minion")
            .or_else(|| stats(&self.ranged_minion, "ranged_minion"))
            .or_else(|| structure(&self.tower, "tower"))
            .or_else(|| structure(&self.nexus, "nexus"))
    }
}

impl GameContent {
    /// Validates names, counts and costs.
    ///
    /// # Errors
    /// Returns the first [`ContentError`] found.
    #[allow(clippy::too_many_lines)] // Flat validation table is intentionally exhaustive.
    pub fn validate(&self) -> Result<(), ContentError> {
        for field in [
            &self.manifest.name,
            &self.manifest.version,
            &self.manifest.author,
        ] {
            if field.is_empty() || field.len() > MAX_MANIFEST_FIELD || !field.is_ascii() {
                return Err(ContentError::BadManifest);
            }
        }
        if self.items.is_empty() {
            return Err(ContentError::NoItems);
        }
        if self.items.len() > MAX_CONTENT_ITEMS {
            return Err(ContentError::TooManyItems);
        }
        for (index, item) in self.items.iter().enumerate() {
            if item.name.is_empty() || item.name.len() > MAX_ITEM_NAME || !item.name.is_ascii() {
                return Err(ContentError::BadName);
            }
            if item.cost == 0 || item.cost > MAX_STAT as u32 {
                return Err(ContentError::FreeItem);
            }
            for value in [
                item.health,
                item.mana,
                item.attack_damage,
                item.armor,
                item.magic_resist,
            ] {
                if value.unsigned_abs() > MAX_ITEM_STAT as u32 {
                    return Err(ContentError::BadItemStats);
                }
            }
            if item.move_speed_per_sec < Fix::ZERO
                || item.move_speed_per_sec > Fix::from_i32(MAX_SPEED_PER_SEC)
                || !(0..=MAX_ITEM_STAT).contains(&item.active_heal)
                || u32::from(item.active_cooldown_ticks) > MAX_COOLDOWN_TICKS
                || item.on_hit_damage > 1_000
                || item.on_damaged_damage > 1_000
                || (item.active_heal > 0) != (item.active_cooldown_ticks > 0)
            {
                return Err(ContentError::BadItemStats);
            }
            if self.items[..index]
                .iter()
                .any(|other| other.name == item.name)
            {
                return Err(ContentError::DuplicateName);
            }
        }
        if self.abilities.is_empty() {
            return Err(ContentError::NoAbilities);
        }
        if self.abilities.len() > MAX_CONTENT_ABILITIES {
            return Err(ContentError::TooManyAbilities);
        }
        for (index, ability) in self.abilities.iter().enumerate() {
            if ability.name.is_empty()
                || ability.name.len() > MAX_ABILITY_NAME
                || !ability.name.is_ascii()
            {
                return Err(ContentError::BadAbilityName);
            }
            if self.abilities[..index]
                .iter()
                .any(|other| other.name == ability.name)
            {
                return Err(ContentError::DuplicateAbilityName);
            }
            if ability.cooldown_ticks == 0
                || ability.cooldown_ticks > MAX_COOLDOWN_TICKS
                || ability.mana_cost < 0
                || ability.mana_cost > MAX_STAT
                || ability.targeting.range() < Fix::ZERO
                || ability.targeting.range() > Fix::from_i32(MAX_RANGE)
                || ability.effects.is_empty()
                || !ability
                    .effects
                    .iter()
                    .all(|effect| effect_valid(effect, &self.modifiers))
            {
                return Err(ContentError::BadAbilityStats);
            }
        }
        if self.modifiers.is_empty() || self.modifiers.len() > MAX_CONTENT_MODIFIERS {
            return Err(ContentError::BadModifiers);
        }
        for (index, modifier) in self.modifiers.iter().enumerate() {
            if modifier.bonus_armor.unsigned_abs() > MAX_RESIST as u32
                || modifier.bonus_attack_damage.unsigned_abs() > MAX_STAT as u32
                || modifier.on_expire_damage.unsigned_abs() > MAX_STAT as u32
                || modifier.on_damaged_damage.unsigned_abs() > MAX_STAT as u32
                || modifier.on_tick_damage.unsigned_abs() > MAX_STAT as u32
            {
                return Err(ContentError::BadModifiers);
            }
            if modifier.name.is_empty()
                || modifier.name.len() > MAX_ITEM_NAME
                || !modifier.name.is_ascii()
                || self.modifiers[..index]
                    .iter()
                    .any(|other| other.name == modifier.name || other.kind == modifier.kind)
            {
                return Err(ContentError::BadModifiers);
            }
        }
        if let Some(field) = self.rules.invalid_field() {
            return Err(ContentError::BadRules(field));
        }
        validate_map(&self.map)?;
        if self.heroes.is_empty() {
            return Err(ContentError::NoHeroes);
        }
        if self.heroes.len() > MAX_CONTENT_HEROES {
            return Err(ContentError::TooManyHeroes);
        }
        for (index, hero) in self.heroes.iter().enumerate() {
            if hero.name.is_empty() || hero.name.len() > MAX_HERO_NAME || !hero.name.is_ascii() {
                return Err(ContentError::BadHeroName);
            }
            if self.heroes[..index]
                .iter()
                .any(|other| other.name == hero.name)
            {
                return Err(ContentError::DuplicateHeroName);
            }
            if hero
                .abilities
                .iter()
                .any(|&id| usize::from(id) >= self.abilities.len())
            {
                return Err(ContentError::BadAbility);
            }
            if hero.health_per_level < 0
                || hero.mana_per_level < 0
                || hero.attack_damage_per_level < 0
                || hero.armor_per_level < 0
                || hero.magic_resist_per_level < 0
            {
                return Err(ContentError::BadHeroStats);
            }
            if hero.max_health <= 0
                || hero.max_health > MAX_STAT
                || hero.max_mana < 0
                || hero.max_mana > MAX_STAT
                || hero.health_regen_per_tick.unsigned_abs() > MAX_ITEM_STAT as u32
                || hero.mana_regen_per_tick.unsigned_abs() > MAX_ITEM_STAT as u32
                || hero.attack_damage.unsigned_abs() > MAX_ITEM_STAT as u32
                || hero.armor.unsigned_abs() > MAX_RESIST as u32
                || hero.magic_resist.unsigned_abs() > MAX_RESIST as u32
                || hero.attack_interval_ticks == 0
                || hero.attack_interval_ticks > MAX_COOLDOWN_TICKS
                || hero.attack_range < Fix::ZERO
                || hero.attack_range > Fix::from_i32(MAX_RANGE)
                || hero.move_speed_per_sec <= Fix::ZERO
                || hero.move_speed_per_sec > Fix::from_i32(MAX_SPEED_PER_SEC)
                || hero.vision_range <= Fix::ZERO
                || hero.vision_range > Fix::from_i32(MAX_RANGE)
            {
                return Err(ContentError::BadHeroStats);
            }
            for growth in [
                hero.health_per_level,
                hero.mana_per_level,
                hero.attack_damage_per_level,
                hero.armor_per_level,
                hero.magic_resist_per_level,
            ] {
                if growth.unsigned_abs() > MAX_ITEM_STAT as u32 {
                    return Err(ContentError::BadHeroStats);
                }
            }
        }
        Ok(())
    }
}
