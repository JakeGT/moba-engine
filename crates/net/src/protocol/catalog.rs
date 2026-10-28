//! Runtime catalogue as carried on the wire: limits and the `Wire*` types.

/// Maximum items in a handshake catalogue.
pub const MAX_WIRE_ITEMS: usize = 16;
/// Fixed item-name bytes on the wire.
pub const WIRE_ITEM_NAME: usize = 16;
/// Maximum heroes in a handshake catalogue.
pub const MAX_WIRE_HEROES: usize = 8;
/// Fixed hero-name bytes on the wire.
pub const WIRE_HERO_NAME: usize = 16;
/// Maximum abilities in a handshake catalogue.
pub const MAX_WIRE_ABILITIES: usize = 24;
/// Fixed ability-name bytes on the wire.
pub const WIRE_ABILITY_NAME: usize = 16;
/// Maximum lanes in a handshake map.
pub const MAX_WIRE_LANES: usize = 4;
/// Maximum points per lane on the wire.
pub const MAX_WIRE_LANE_POINTS: usize = 16;
/// Fixed lane-name bytes on the wire.
pub const WIRE_LANE_NAME: usize = 16;
/// Maximum vision-wall segments in a handshake map.
pub const MAX_WIRE_WALLS: usize = 32;
/// Maximum structures in a handshake map.
pub const MAX_WIRE_STRUCTURES: usize = 32;
/// Maximum modifier definitions in a handshake catalogue.
pub const MAX_WIRE_MODIFIERS: usize = 16;
/// Fixed modifier-name bytes on the wire.
pub const WIRE_MODIFIER_NAME: usize = 16;
/// Maximum top-level ability effects on the wire.
pub const MAX_WIRE_EFFECTS: usize = 3;
/// Maximum area/on-hit effects per list on the wire.
pub const MAX_WIRE_AREA_EFFECTS: usize = 3;
/// Fixed manifest-field bytes on the wire.
pub const WIRE_MANIFEST_FIELD: usize = 32;
/// Fixed visual-asset-name bytes on the wire (`WireHero::model`, `WireMap::visuals`).
pub const WIRE_VISUAL_NAME: usize = 16;

const _: () = {
    assert!(MAX_WIRE_ITEMS == moba_game::MAX_CONTENT_ITEMS);
    assert!(MAX_WIRE_HEROES == moba_game::MAX_CONTENT_HEROES);
    assert!(MAX_WIRE_ABILITIES == moba_game::MAX_CONTENT_ABILITIES);
    assert!(MAX_WIRE_LANES == moba_game::MAX_CONTENT_LANES);
    assert!(MAX_WIRE_LANE_POINTS == moba_game::MAX_LANE_POINTS);
    assert!(MAX_WIRE_WALLS == moba_game::MAX_CONTENT_WALLS);
    assert!(MAX_WIRE_STRUCTURES == moba_game::MAX_CONTENT_STRUCTURES);
    assert!(WIRE_MANIFEST_FIELD == moba_game::MAX_MANIFEST_FIELD);
    assert!(WIRE_LANE_NAME == moba_game::MAX_LANE_NAME);
    assert!(MAX_WIRE_MODIFIERS == moba_game::MAX_CONTENT_MODIFIERS);
    assert!(WIRE_MODIFIER_NAME == moba_game::MAX_ITEM_NAME);
    assert!(MAX_WIRE_EFFECTS == moba_game::MAX_ABILITY_EFFECTS);
    assert!(MAX_WIRE_AREA_EFFECTS == moba_game::MAX_AREA_EFFECTS);
    assert!(WIRE_ITEM_NAME == moba_game::MAX_ITEM_NAME);
    assert!(WIRE_HERO_NAME == moba_game::MAX_HERO_NAME);
    assert!(WIRE_ABILITY_NAME == moba_game::MAX_ABILITY_NAME);
    assert!(WIRE_VISUAL_NAME == moba_game::MAX_VISUAL_NAME);
};

/// One runtime item definition carried in the handshake.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WireItem {
    /// Name bytes, padded with zeroes.
    pub name: [u8; WIRE_ITEM_NAME],
    /// Used bytes in `name`.
    pub name_len: u8,
    /// Gold cost.
    pub cost: u32,
    /// Health bonus.
    pub health: i32,
    /// Mana bonus.
    pub mana: i32,
    /// Attack damage bonus.
    pub attack_damage: i32,
    /// Armour bonus.
    pub armor: i32,
    /// Magic resistance bonus.
    pub magic_resist: i32,
    /// Movement speed bonus in raw fixed-point bits.
    pub move_speed_bits: i64,
    /// Bonus on-hit damage.
    pub on_hit_damage: u16,
    /// Damage reflected to attackers.
    pub on_damaged_damage: u16,
    /// Self-heal applied by the active (0 = no active).
    pub active_heal: i32,
    /// Active cooldown in ticks (0 = no active).
    pub active_cooldown_ticks: u16,
}

impl Default for WireItem {
    fn default() -> Self {
        Self {
            name: [0; WIRE_ITEM_NAME],
            name_len: 0,
            cost: 0,
            health: 0,
            mana: 0,
            attack_damage: 0,
            armor: 0,
            magic_resist: 0,
            move_speed_bits: 0,
            on_hit_damage: 0,
            on_damaged_damage: 0,
            active_heal: 0,
            active_cooldown_ticks: 0,
        }
    }
}

impl WireItem {
    /// Decodes the name as UTF-8, falling back to an empty string.
    #[must_use]
    pub fn name_str(&self) -> &str {
        let end = usize::from(self.name_len).min(WIRE_ITEM_NAME);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

/// One runtime hero definition carried in the handshake.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WireHero {
    /// Name bytes, padded with zeroes.
    pub name: [u8; WIRE_HERO_NAME],
    /// Used bytes in `name`.
    pub name_len: u8,
    /// Maximum health.
    pub max_health: i32,
    /// Maximum mana.
    pub max_mana: i32,
    /// Health regenerated per tick.
    pub health_regen_per_tick: i32,
    /// Mana regenerated per tick.
    pub mana_regen_per_tick: i32,
    /// Basic attack damage.
    pub attack_damage: i32,
    /// Basic attack range in raw fixed-point bits.
    pub attack_range_bits: i64,
    /// Basic attack interval in ticks.
    pub attack_interval_ticks: u32,
    /// Movement speed in raw fixed-point bits.
    pub move_speed_bits: i64,
    /// Armour.
    pub armor: i32,
    /// Magic resistance.
    pub magic_resist: i32,
    /// Vision radius in raw fixed-point bits.
    pub vision_bits: i64,
    /// Ability ids for Q/W/E.
    pub abilities: [u8; 3],
    /// Health gained per level.
    pub health_per_level: i32,
    /// Mana gained per level.
    pub mana_per_level: i32,
    /// Attack damage gained per level.
    pub attack_damage_per_level: i32,
    /// Armour gained per level.
    pub armor_per_level: i32,
    /// Magic resistance gained per level.
    pub magic_resist_per_level: i32,
    /// Model name bytes, padded with zeroes (a name, never a path; protocol v40).
    pub model: [u8; WIRE_VISUAL_NAME],
    /// Used bytes in `model`; 0 means no model (procedural rig).
    pub model_len: u8,
}

impl Default for WireHero {
    fn default() -> Self {
        Self {
            name: [0; WIRE_HERO_NAME],
            name_len: 0,
            max_health: 0,
            max_mana: 0,
            health_regen_per_tick: 0,
            mana_regen_per_tick: 0,
            attack_damage: 0,
            attack_range_bits: 0,
            attack_interval_ticks: 0,
            move_speed_bits: 0,
            armor: 0,
            magic_resist: 0,
            vision_bits: 0,
            abilities: [0; 3],
            health_per_level: 0,
            mana_per_level: 0,
            attack_damage_per_level: 0,
            armor_per_level: 0,
            magic_resist_per_level: 0,
            model: [0; WIRE_VISUAL_NAME],
            model_len: 0,
        }
    }
}

impl WireHero {
    /// Decodes the name as UTF-8, falling back to an empty string.
    #[must_use]
    pub fn name_str(&self) -> &str {
        let end = usize::from(self.name_len).min(WIRE_HERO_NAME);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }

    /// The model name, or `None` for the procedural rig (or undecodable bytes).
    #[must_use]
    pub fn model_str(&self) -> Option<&str> {
        let end = usize::from(self.model_len).min(WIRE_VISUAL_NAME);
        core::str::from_utf8(&self.model[..end])
            .ok()
            .filter(|name| !name.is_empty())
    }
}

/// One area/on-hit effect carried in the handshake.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WireAreaEffect {
    /// 0 none, 1 damage, 2 apply-modifier.
    pub kind: u8,
    /// Modifier definition id.
    pub modifier: u16,
    /// Duration in ticks.
    pub duration_ticks: u32,
    /// Movement factor in raw fixed-point bits.
    pub factor_bits: i64,
    /// Shield/burn amount.
    pub amount: i32,
    /// Damage type discriminant.
    pub damage_type: u8,
}

/// One top-level ability effect carried in the handshake.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WireEffect {
    /// 0 none, 1 damage, 2 heal, 3 modifier, 4 displace, 5 projectile, 6 area, 7 zone.
    pub kind: u8,
    /// Damage/heal amount.
    pub amount: i32,
    /// Damage type discriminant.
    pub damage_type: u8,
    /// Projectile speed in raw fixed-point bits.
    pub speed_bits: i64,
    /// Range/radius/displace in raw fixed-point bits.
    pub range_bits: i64,
    /// Radius in raw fixed-point bits.
    pub radius_bits: i64,
    /// Visual projectile arc height in raw fixed-point bits.
    pub arc_bits: i64,
    /// Duration in ticks.
    pub duration_ticks: u32,
    /// Period in ticks.
    pub period_ticks: u32,
    /// Number of valid `area` entries.
    pub area_count: u8,
    /// Area/on-hit effects.
    pub area: [WireAreaEffect; MAX_WIRE_AREA_EFFECTS],
}

/// One runtime ability definition carried in the handshake.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WireAbility {
    /// Name bytes, padded with zeroes.
    pub name: [u8; WIRE_ABILITY_NAME],
    /// Used bytes in `name`.
    pub name_len: u8,
    /// Cooldown in ticks.
    pub cooldown_ticks: u32,
    /// Mana cost.
    pub mana_cost: i32,
    /// Summary maximum cast distance in raw fixed-point bits.
    pub cast_range_bits: i64,
    /// Summary display radius in raw fixed-point bits.
    pub radius_bits: i64,
    /// 0 self, 1 point, 2 direction.
    pub targeting_kind: u8,
    /// Targeting range in raw fixed-point bits.
    pub targeting_range_bits: i64,
    /// Number of valid `effects` entries.
    pub effect_count: u8,
    /// Ordered effects.
    pub effects: [WireEffect; MAX_WIRE_EFFECTS],
}

impl Default for WireAbility {
    fn default() -> Self {
        Self {
            name: [0; WIRE_ABILITY_NAME],
            name_len: 0,
            cooldown_ticks: 0,
            mana_cost: 0,
            cast_range_bits: 0,
            radius_bits: 0,
            targeting_kind: 0,
            targeting_range_bits: 0,
            effect_count: 0,
            effects: [WireEffect::default(); MAX_WIRE_EFFECTS],
        }
    }
}

impl WireAbility {
    /// Decodes the name as UTF-8, falling back to an empty string.
    #[must_use]
    pub fn name_str(&self) -> &str {
        let end = usize::from(self.name_len).min(WIRE_ABILITY_NAME);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

/// One lane path carried in the handshake map./// One lane path carried in the handshake map.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WireLane {
    /// Name bytes, padded with zeroes.
    pub name: [u8; WIRE_LANE_NAME],
    /// Used bytes in `name`.
    pub name_len: u8,
    /// Number of valid points.
    pub point_count: u8,
    /// World-space points.
    pub points: [[i16; 2]; MAX_WIRE_LANE_POINTS],
}

impl Default for WireLane {
    fn default() -> Self {
        Self {
            name: [0; WIRE_LANE_NAME],
            name_len: 0,
            point_count: 0,
            points: [[0; 2]; MAX_WIRE_LANE_POINTS],
        }
    }
}

impl WireLane {
    /// Decodes the name as UTF-8, falling back to an empty string.
    #[must_use]
    pub fn name_str(&self) -> &str {
        let end = usize::from(self.name_len).min(WIRE_LANE_NAME);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

/// One vision-blocking wall segment.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WireWall {
    /// First endpoint.
    pub a: [i16; 2],
    /// Second endpoint.
    pub b: [i16; 2],
}

/// Minimap geometry carried in the handshake.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WireMap {
    /// Display name bytes, padded with zeroes.
    pub name: [u8; WIRE_MANIFEST_FIELD],
    /// Used name bytes.
    pub name_len: u8,
    /// Visual theme id.
    pub theme: u8,
    /// Explicit visuals-name bytes, padded with zeroes (protocol v40).
    pub visuals: [u8; WIRE_VISUAL_NAME],
    /// Used bytes in `visuals`; 0 means the theme's default visuals.
    pub visuals_len: u8,
    /// Square world extent in world units.
    pub extent: u16,
    /// Number of valid lanes.
    pub lane_count: u8,
    /// Lane paths.
    pub lanes: [WireLane; MAX_WIRE_LANES],
    /// Blue spawn.
    pub blue_spawn: [i16; 2],
    /// Red spawn.
    pub red_spawn: [i16; 2],
    /// Number of valid structures.
    pub structure_count: u8,
    /// Placed structures.
    pub structures: [WireStructure; MAX_WIRE_STRUCTURES],
    /// Number of valid walls.
    pub wall_count: u8,
    /// Vision-blocking walls.
    pub walls: [WireWall; MAX_WIRE_WALLS],
}

/// One placed structure carried in the handshake map.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WireStructure {
    /// Team index (blue 0, red 1).
    pub team: u8,
    /// Structure kind (tower 0, nexus 1).
    pub kind: u8,
    /// World position.
    pub position: [i16; 2],
    /// Lane index, or `0xFF` for none.
    pub lane: u8,
}

impl WireMap {
    /// Display name as UTF-8.
    #[must_use]
    pub fn name_str(&self) -> &str {
        let end = usize::from(self.name_len).min(self.name.len());
        core::str::from_utf8(&self.name[..end]).unwrap_or("Map")
    }

    /// The explicit visuals name, or `None` for the theme default (or undecodable bytes).
    #[must_use]
    pub fn visuals_str(&self) -> Option<&str> {
        let end = usize::from(self.visuals_len).min(WIRE_VISUAL_NAME);
        core::str::from_utf8(&self.visuals[..end])
            .ok()
            .filter(|name| !name.is_empty())
    }
}

impl Default for WireMap {
    fn default() -> Self {
        Self {
            name: [0; WIRE_MANIFEST_FIELD],
            name_len: 0,
            theme: 0,
            visuals: [0; WIRE_VISUAL_NAME],
            visuals_len: 0,
            extent: 256,
            lane_count: 0,
            lanes: [WireLane::default(); MAX_WIRE_LANES],
            blue_spawn: [0; 2],
            red_spawn: [0; 2],
            structure_count: 0,
            structures: [WireStructure::default(); MAX_WIRE_STRUCTURES],
            wall_count: 0,
            walls: [WireWall::default(); MAX_WIRE_WALLS],
        }
    }
}

/// One modifier definition carried in the handshake.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WireModifier {
    /// Name bytes, padded with zeroes.
    pub name: [u8; WIRE_MODIFIER_NAME],
    /// Used bytes in `name`.
    pub name_len: u8,
    /// Behaviour kind.
    pub kind: u8,
    /// Flat armour bonus.
    pub bonus_armor: i32,
    /// Flat attack-damage bonus.
    pub bonus_attack_damage: i32,
    /// Damage dealt on expiry.
    pub on_expire_damage: i32,
    /// Damage reflected while active.
    pub on_damaged_damage: i32,
    /// Damage dealt each tick.
    pub on_tick_damage: i32,
    /// Replicated status bits.
    pub status_bits: u16,
}

impl WireModifier {
    /// Decodes the name as UTF-8, falling back to an empty string.
    #[must_use]
    pub fn name_str(&self) -> &str {
        let end = usize::from(self.name_len).min(WIRE_MODIFIER_NAME);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

/// Runtime minion stats carried in the handshake.
#[allow(missing_docs)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WireMinionStats {
    pub max_health: i32,
    pub attack_damage: i32,
    pub attack_range_bits: i64,
    pub attack_interval_ticks: u32,
    pub move_speed_bits: i64,
    pub armor: i32,
    pub magic_resist: i32,
    pub vision_range_bits: i64,
}

/// Runtime structure stats carried in the handshake.
#[allow(missing_docs)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WireStructureStats {
    pub max_health: i32,
    pub attack_damage: i32,
    pub attack_range_bits: i64,
    pub attack_interval_ticks: u32,
    pub armor: i32,
    pub magic_resist: i32,
    pub vision_range_bits: i64,
}

/// XP and gold granted to the killer of one unit kind.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WireUnitRewards {
    /// XP granted.
    pub xp: u32,
    /// Gold granted.
    pub gold: u32,
}

/// Kill rewards per unit kind carried in the handshake.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WireRewards {
    /// Melee minion reward.
    pub melee_minion: WireUnitRewards,
    /// Ranged minion reward.
    pub ranged_minion: WireUnitRewards,
    /// Tower reward.
    pub tower: WireUnitRewards,
    /// Nexus reward.
    pub nexus: WireUnitRewards,
    /// Hero kill reward.
    pub hero: WireUnitRewards,
}

/// Tunable match rules carried in the handshake.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WireRules {
    /// Melee minion stats.
    pub melee_minion: WireMinionStats,
    /// Ranged minion stats.
    pub ranged_minion: WireMinionStats,
    /// Tower stats.
    pub tower: WireStructureStats,
    /// Nexus stats.
    pub nexus: WireStructureStats,
    /// Maximum hero level.
    pub level_cap: u8,
    /// Base XP.
    pub xp_base: u32,
    /// XP per level.
    pub xp_per_level: u32,
    /// Melee minion vision bits.
    pub melee_vision_bits: i64,
    /// Ranged minion vision bits.
    pub ranged_vision_bits: i64,
    /// Tower vision bits.
    pub tower_vision_bits: i64,
    /// Nexus vision bits.
    pub nexus_vision_bits: i64,
    /// Heroes per team.
    pub team_size: u8,
    /// Fill empty player slots with bots.
    pub bot_fill: bool,
    /// Base hero respawn ticks.
    pub hero_respawn_ticks: u32,
    /// Extra respawn ticks per level.
    pub respawn_per_level: u32,
    /// Ticks between waves.
    pub wave_interval_ticks: u32,
    /// First-wave delay ticks.
    pub first_wave_ticks: u32,
    /// Starting gold per hero.
    pub starting_gold: u32,
    /// Passive gold granted per interval.
    pub gold_per_interval: u32,
    /// Ticks between passive gold grants.
    pub gold_tick_interval: u32,
    /// Shared-XP radius bits.
    pub xp_radius_bits: i64,
    /// Shop interaction radius bits.
    pub shop_radius_bits: i64,
    /// Shared minion XP percentage (100 = full).
    pub shared_xp_percent: u32,
    /// Kill rewards per unit kind.
    pub rewards: WireRewards,
    /// Whether structures prioritize heroes over minions.
    pub structure_heroes_first: bool,
    /// Whether destroying a nexus ends the match.
    pub nexus_win: bool,
    /// Score limit that ends the match (0 disables).
    pub score_limit: u16,
}

/// Complete runtime content catalogue.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WireCatalog {
    /// Manifest name bytes.
    pub manifest_name: [u8; WIRE_MANIFEST_FIELD],
    /// Used manifest name bytes.
    pub manifest_name_len: u8,
    /// Manifest version bytes.
    pub manifest_version: [u8; WIRE_MANIFEST_FIELD],
    /// Used manifest version bytes.
    pub manifest_version_len: u8,
    /// Manifest author bytes.
    pub manifest_author: [u8; WIRE_MANIFEST_FIELD],
    /// Used manifest author bytes.
    pub manifest_author_len: u8,
    /// Number of valid items.
    pub item_count: u8,
    /// Item catalogue.
    pub items: [WireItem; MAX_WIRE_ITEMS],
    /// Number of valid heroes.
    pub hero_count: u8,
    /// Hero catalogue.
    pub heroes: [WireHero; MAX_WIRE_HEROES],
    /// Number of valid abilities.
    pub ability_count: u8,
    /// Ability catalogue.
    pub abilities: [WireAbility; MAX_WIRE_ABILITIES],
    /// Number of valid modifiers.
    pub modifier_count: u8,
    /// Modifier definitions.
    pub modifiers: [WireModifier; MAX_WIRE_MODIFIERS],
    /// Tunable match rules.
    pub rules: WireRules,
    /// Map geometry.
    pub map: WireMap,
}

impl Default for WireCatalog {
    fn default() -> Self {
        Self {
            manifest_name: [0; WIRE_MANIFEST_FIELD],
            manifest_name_len: 0,
            manifest_version: [0; WIRE_MANIFEST_FIELD],
            manifest_version_len: 0,
            manifest_author: [0; WIRE_MANIFEST_FIELD],
            manifest_author_len: 0,
            item_count: 0,
            items: [WireItem::default(); MAX_WIRE_ITEMS],
            hero_count: 0,
            heroes: [WireHero::default(); MAX_WIRE_HEROES],
            ability_count: 0,
            abilities: [WireAbility::default(); MAX_WIRE_ABILITIES],
            modifier_count: 0,
            modifiers: [WireModifier::default(); MAX_WIRE_MODIFIERS],
            rules: WireRules::default(),
            map: WireMap::default(),
        }
    }
}
