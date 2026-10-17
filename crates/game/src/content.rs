//! Runtime content definitions shared by the simulation and server.
//!
//! The default catalogue is derived from the compile-time [`ITEMS`] table, so
//! existing games and replays keep working. Servers can load a different item catalogue and
//! publish it to clients in the handshake; clients render and shop against the received
//! catalogue. The catalogue is deliberately small and fixed-capacity on the wire.

use std::path::Path;

use moba_core::{Fix, FixVec2};
use serde::Deserialize;

use crate::{
    ABILITIES, AbilityDef, AbilityKind, DamageType, ITEMS, ItemDef, MAP, MapDef, StructureKind,
    Team,
};

/// Maximum length of a manifest field.
pub const MAX_MANIFEST_FIELD: usize = 32;

/// Human-readable identity of a content pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentManifest {
    /// Pack name.
    pub name: String,
    /// Pack version string.
    pub version: String,
    /// Author/maintainer.
    pub author: String,
}

impl Default for ContentManifest {
    fn default() -> Self {
        Self {
            name: String::from("Core"),
            version: String::from(env!("CARGO_PKG_VERSION")),
            author: String::from("MOBA Engine"),
        }
    }
}

/// Maximum number of items in a content pack (matches the wire catalogue).
pub const MAX_CONTENT_ITEMS: usize = 16;
/// Maximum item display-name length in bytes.
pub const MAX_ITEM_NAME: usize = 16;
/// Maximum heroes in a content pack.
pub const MAX_CONTENT_HEROES: usize = 8;
/// Maximum hero display-name length in bytes.
pub const MAX_HERO_NAME: usize = 16;
/// Maximum abilities in a content pack.
pub const MAX_CONTENT_ABILITIES: usize = 24;
/// Maximum ability display-name length in bytes.
pub const MAX_ABILITY_NAME: usize = 16;
/// Maximum lanes in a content pack.
pub const MAX_CONTENT_LANES: usize = 4;
/// Maximum waypoints per lane.
pub const MAX_LANE_POINTS: usize = 16;
/// Maximum structures in a content pack.
pub const MAX_CONTENT_STRUCTURES: usize = 32;
/// Maximum vision-blocking wall segments.
pub const MAX_CONTENT_WALLS: usize = 32;

/// An owned, runtime item definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemSpec {
    /// Display name (1..=16 ASCII bytes).
    pub name: String,
    /// Cost in gold.
    pub cost: u32,
    /// Bonus maximum/current health.
    pub health: i32,
    /// Bonus maximum/current mana.
    pub mana: i32,
    /// Bonus attack damage.
    pub attack_damage: i32,
    /// Bonus armour.
    pub armor: i32,
    /// Bonus magic resistance.
    pub magic_resist: i32,
    /// Bonus movement speed in world units per second.
    pub move_speed_per_sec: Fix,
}

/// An owned lane path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneSpec {
    /// Display name (1..=16 ASCII bytes).
    pub name: String,
    /// Waypoints from blue base to red base.
    pub path: Vec<FixVec2>,
}

impl LaneSpec {
    /// Number of waypoints.
    #[must_use]
    pub fn len(&self) -> usize {
        self.path.len()
    }

    /// Returns `true` when the lane has no waypoints.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.path.is_empty()
    }

    /// Waypoint at `index` measured from `team`'s own base.
    #[must_use]
    pub fn point(&self, team: Team, index: usize) -> FixVec2 {
        match team {
            Team::Blue => self.path[index.min(self.path.len() - 1)],
            Team::Red => self.path[self.path.len() - 1 - index.min(self.path.len() - 1)],
        }
    }
}

/// An owned placed structure.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct StructureSpec {
    /// Owning team.
    pub team: Team,
    /// World position.
    pub position: FixVec2,
    /// Structure type.
    pub kind: StructureKind,
    /// Optional lane index.
    pub lane: Option<u8>,
}

/// A vision-blocking wall segment (does not block movement).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WallSpec {
    /// First endpoint.
    pub a: FixVec2,
    /// Second endpoint.
    pub b: FixVec2,
}

/// An owned runtime map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapSpec {
    /// Lanes in stable order.
    pub lanes: Vec<LaneSpec>,
    /// Structures in stable order.
    pub structures: Vec<StructureSpec>,
    /// Vision-blocking wall segments.
    pub walls: Vec<WallSpec>,
    /// Blue spawn.
    pub blue_spawn: FixVec2,
    /// Red spawn.
    pub red_spawn: FixVec2,
}

impl From<&MapDef> for MapSpec {
    fn from(map: &MapDef) -> Self {
        Self {
            lanes: map
                .lanes
                .iter()
                .map(|lane| LaneSpec {
                    name: lane.name.to_owned(),
                    path: lane.path.to_vec(),
                })
                .collect(),
            structures: map
                .structures
                .iter()
                .map(|structure| StructureSpec {
                    team: structure.team,
                    position: structure.position,
                    kind: structure.kind,
                    lane: structure.lane,
                })
                .collect(),
            walls: Vec::new(),
            blue_spawn: map.blue_spawn,
            red_spawn: map.red_spawn,
        }
    }
}

impl MapSpec {
    /// Team spawn point.
    #[must_use]
    pub const fn spawn(&self, team: Team) -> FixVec2 {
        match team {
            Team::Blue => self.blue_spawn,
            Team::Red => self.red_spawn,
        }
    }

    /// A hero's spawn position for a slot in `0..5`.
    #[must_use]
    pub fn hero_spawn(&self, team: Team, slot: u8) -> FixVec2 {
        let base = self.spawn(team);
        let offsets = [(0, 0), (4, -7), (4, 7), (-4, -12), (-4, 12)];
        let (dx, dy) = offsets[slot as usize % offsets.len()];
        let dx = if team == Team::Blue { dx } else { -dx };
        FixVec2::new(base.x + Fix::from_i32(dx), base.y + Fix::from_i32(dy))
    }

    /// Nexus position for a team.
    #[must_use]
    pub fn nexus_position(&self, team: Team) -> FixVec2 {
        self.structures
            .iter()
            .find(|structure| structure.team == team && structure.kind == StructureKind::Nexus)
            .map_or(self.spawn(team), |structure| structure.position)
    }
}

/// An owned, runtime ability definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbilitySpec {
    /// Display name.
    pub name: String,
    /// Cooldown in simulation ticks.
    pub cooldown_ticks: u32,
    /// Mana cost.
    pub mana_cost: i32,
    /// Maximum cast distance.
    pub cast_range: Fix,
    /// Effect/behaviour.
    pub kind: AbilityKind,
}

impl From<&AbilityDef> for AbilitySpec {
    fn from(ability: &AbilityDef) -> Self {
        Self {
            name: ability.name.to_owned(),
            cooldown_ticks: ability.cooldown_ticks,
            mana_cost: ability.mana_cost,
            cast_range: ability.cast_range,
            kind: ability.kind,
        }
    }
}

/// An owned, runtime hero stat definition.
///
/// Ability behaviour still comes from the engine ability library; `abilities` are stable
/// ability ids in Q/W/E order so packs can recombine/tune existing kits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeroSpec {
    /// Display name.
    pub name: String,
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
    /// Basic attack range.
    pub attack_range: Fix,
    /// Basic attack interval in ticks.
    pub attack_interval_ticks: u32,
    /// Movement speed in world units per second.
    pub move_speed_per_sec: Fix,
    /// Armour.
    pub armor: i32,
    /// Magic resistance.
    pub magic_resist: i32,
    /// Vision radius.
    pub vision_range: Fix,
    /// Ability ids for Q/W/E.
    pub abilities: [u8; 3],
}

impl From<&crate::HeroDef> for HeroSpec {
    fn from(hero: &crate::HeroDef) -> Self {
        Self {
            name: hero.name.to_owned(),
            max_health: hero.max_health,
            max_mana: hero.max_mana,
            health_regen_per_tick: hero.health_regen_per_tick,
            mana_regen_per_tick: hero.mana_regen_per_tick,
            attack_damage: hero.attack_damage,
            attack_range: hero.attack_range,
            attack_interval_ticks: hero.attack_interval_ticks,
            move_speed_per_sec: hero.move_speed_per_sec,
            armor: hero.armor,
            magic_resist: hero.magic_resist,
            vision_range: hero.vision_range,
            abilities: hero.abilities,
        }
    }
}

impl From<&ItemDef> for ItemSpec {
    fn from(item: &ItemDef) -> Self {
        Self {
            name: item.name.to_owned(),
            cost: item.cost,
            health: item.health,
            mana: item.mana,
            attack_damage: item.attack_damage,
            armor: item.armor,
            magic_resist: item.magic_resist,
            move_speed_per_sec: item.move_speed_per_sec,
        }
    }
}

/// A validated content pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameContent {
    /// Pack identity/version metadata.
    pub manifest: ContentManifest,
    /// Item catalogue in stable id order.
    pub items: Vec<ItemSpec>,
    /// Hero catalogue in stable id order.
    pub heroes: Vec<HeroSpec>,
    /// Ability catalogue in stable id order.
    pub abilities: Vec<AbilitySpec>,
    /// Map geometry.
    pub map: MapSpec,
}

impl Default for GameContent {
    fn default() -> Self {
        Self {
            manifest: ContentManifest::default(),
            items: ITEMS.iter().map(ItemSpec::from).collect(),
            heroes: crate::HEROES.iter().map(HeroSpec::from).collect(),
            abilities: ABILITIES.iter().map(AbilitySpec::from).collect(),
            map: MapSpec::from(&MAP),
        }
    }
}

/// Content validation errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentError {
    /// No items were supplied.
    NoItems,
    /// Too many items for the wire catalogue.
    TooManyItems,
    /// An item name is empty, too long or non-ASCII.
    BadName,
    /// Two items share a name.
    DuplicateName,
    /// An item costs zero gold.
    FreeItem,
    /// No heroes were supplied.
    NoHeroes,
    /// Too many heroes for the wire catalogue.
    TooManyHeroes,
    /// A hero name is empty, too long or non-ASCII.
    BadHeroName,
    /// Two heroes share a name.
    DuplicateHeroName,
    /// A hero ability id is outside the ability library.
    BadAbility,
    /// A hero has non-positive health or an invalid attack interval.
    BadHeroStats,
    /// No abilities were supplied.
    NoAbilities,
    /// Too many abilities for the wire catalogue.
    TooManyAbilities,
    /// An ability name is empty, too long or non-ASCII.
    BadAbilityName,
    /// Two abilities share a name.
    DuplicateAbilityName,
    /// Ability cooldown mana or cast range is invalid.
    BadAbilityStats,
    /// Ability kind parameters are invalid.
    BadAbilityKind,
    /// Map has no lanes or too many lanes/points/structures.
    BadMap,
    /// Map lane names or structure layout are invalid.
    BadMapData,
    /// Wall count/endpoints are invalid.
    BadWalls,
    /// Manifest fields are empty, too long or non-ASCII.
    BadManifest,
}

impl core::fmt::Display for ContentError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoItems => f.write_str("content must define at least one item"),
            Self::TooManyItems => write!(f, "content has more than {MAX_CONTENT_ITEMS} items"),
            Self::BadName => f.write_str("item names must be 1..=16 ASCII bytes"),
            Self::DuplicateName => f.write_str("item names must be unique"),
            Self::FreeItem => f.write_str("item costs must be greater than zero"),
            Self::NoHeroes => f.write_str("content must define at least one hero"),
            Self::TooManyHeroes => write!(f, "content has more than {MAX_CONTENT_HEROES} heroes"),
            Self::BadHeroName => f.write_str("hero names must be 1..=16 ASCII bytes"),
            Self::DuplicateHeroName => f.write_str("hero names must be unique"),
            Self::BadAbility => f.write_str("hero ability ids must exist"),
            Self::BadHeroStats => f.write_str("hero stats must be positive"),
            Self::NoAbilities => f.write_str("content must define at least one ability"),
            Self::TooManyAbilities => {
                write!(f, "content has more than {MAX_CONTENT_ABILITIES} abilities")
            }
            Self::BadAbilityName => f.write_str("ability names must be 1..=16 ASCII bytes"),
            Self::DuplicateAbilityName => f.write_str("ability names must be unique"),
            Self::BadAbilityStats => {
                f.write_str("ability cooldown must be positive and mana/cast range non-negative")
            }
            Self::BadAbilityKind => f.write_str("ability kind parameters must be positive"),
            Self::BadMap => f.write_str("map limits are invalid"),
            Self::BadMapData => f.write_str("map lanes/structures are invalid"),
            Self::BadWalls => f.write_str("map walls are invalid"),
            Self::BadManifest => f.write_str("manifest fields must be 1..=32 ASCII bytes"),
        }
    }
}

impl std::error::Error for ContentError {}

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
            if item.cost == 0 {
                return Err(ContentError::FreeItem);
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
                || ability.mana_cost < 0
                || ability.cast_range < Fix::ZERO
            {
                return Err(ContentError::BadAbilityStats);
            }
            let kind_ok = match ability.kind {
                AbilityKind::Skillshot {
                    speed_per_sec,
                    range,
                    radius,
                    damage,
                    ..
                } => {
                    speed_per_sec > Fix::ZERO
                        && range > Fix::ZERO
                        && radius > Fix::ZERO
                        && damage >= 0
                }
                AbilityKind::Dash { range } => range > Fix::ZERO,
                AbilityKind::Shield {
                    amount,
                    duration_ticks,
                } => amount > 0 && duration_ticks > 0,
                AbilityKind::Heal { amount, radius } => amount >= 0 && radius >= Fix::ZERO,
                AbilityKind::Burst { radius, damage, .. } => radius > Fix::ZERO && damage >= 0,
            };
            if !kind_ok {
                return Err(ContentError::BadAbilityKind);
            }
        }
        if self.map.lanes.is_empty()
            || self.map.lanes.len() > MAX_CONTENT_LANES
            || self.map.structures.len() > MAX_CONTENT_STRUCTURES
        {
            return Err(ContentError::BadMap);
        }
        for (index, lane) in self.map.lanes.iter().enumerate() {
            if lane.name.is_empty()
                || lane.name.len() > MAX_HERO_NAME
                || !lane.name.is_ascii()
                || lane.path.len() < 2
                || lane.path.len() > MAX_LANE_POINTS
            {
                return Err(ContentError::BadMapData);
            }
            if self.map.lanes[..index]
                .iter()
                .any(|other| other.name == lane.name)
            {
                return Err(ContentError::BadMapData);
            }
        }
        for team in Team::ALL {
            if !self
                .map
                .structures
                .iter()
                .any(|structure| structure.team == team && structure.kind == StructureKind::Nexus)
            {
                return Err(ContentError::BadMapData);
            }
        }
        if self.map.walls.len() > MAX_CONTENT_WALLS
            || self.map.walls.iter().any(|wall| wall.a == wall.b)
        {
            return Err(ContentError::BadWalls);
        }
        if self.map.structures.iter().any(|structure| {
            structure
                .lane
                .is_some_and(|lane| usize::from(lane) >= self.map.lanes.len())
        }) {
            return Err(ContentError::BadMapData);
        }
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
            if hero.max_health <= 0
                || hero.max_mana < 0
                || hero.attack_interval_ticks == 0
                || hero.attack_range < Fix::ZERO
                || hero.move_speed_per_sec <= Fix::ZERO
                || hero.vision_range <= Fix::ZERO
            {
                return Err(ContentError::BadHeroStats);
            }
        }
        Ok(())
    }

    /// Stable FNV-1a hash of the catalogue, used for diagnostics and mismatch checks.
    #[must_use]
    #[allow(clippy::too_many_lines)] // Canonical field feed mirrors the content schema.
    pub fn hash(&self) -> u64 {
        let mut hash = 0xCBF2_9CE4_8422_2325u64;
        let mut feed = |bytes: &[u8]| {
            for byte in bytes {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
            }
        };
        for field in [
            &self.manifest.name,
            &self.manifest.version,
            &self.manifest.author,
        ] {
            feed(field.as_bytes());
            feed(&[0]);
        }
        for item in &self.items {
            feed(item.name.as_bytes());
            feed(&[0]);
            for value in [
                item.cost,
                item.health as u32,
                item.mana as u32,
                item.attack_damage as u32,
                item.armor as u32,
                item.magic_resist as u32,
            ] {
                feed(&value.to_le_bytes());
            }
            feed(&item.move_speed_per_sec.to_bits().to_le_bytes());
        }
        for ability in &self.abilities {
            feed(ability.name.as_bytes());
            feed(&[0]);
            feed(&ability.cooldown_ticks.to_le_bytes());
            feed(&(ability.mana_cost as u32).to_le_bytes());
            feed(&ability.cast_range.to_bits().to_le_bytes());
            match ability.kind {
                AbilityKind::Skillshot {
                    speed_per_sec,
                    range,
                    radius,
                    damage,
                    damage_type,
                } => {
                    feed(&[0]);
                    feed(&speed_per_sec.to_bits().to_le_bytes());
                    feed(&range.to_bits().to_le_bytes());
                    feed(&radius.to_bits().to_le_bytes());
                    feed(&(damage as u32).to_le_bytes());
                    feed(&[damage_type as u8]);
                }
                AbilityKind::Dash { range } => {
                    feed(&[1]);
                    feed(&range.to_bits().to_le_bytes());
                }
                AbilityKind::Heal { amount, radius } => {
                    feed(&[2]);
                    feed(&(amount as u32).to_le_bytes());
                    feed(&radius.to_bits().to_le_bytes());
                }
                AbilityKind::Shield {
                    amount,
                    duration_ticks,
                } => {
                    feed(&[4]);
                    feed(&(amount as u32).to_le_bytes());
                    feed(&duration_ticks.to_le_bytes());
                }
                AbilityKind::Burst {
                    radius,
                    damage,
                    damage_type,
                } => {
                    feed(&[3]);
                    feed(&radius.to_bits().to_le_bytes());
                    feed(&(damage as u32).to_le_bytes());
                    feed(&[damage_type as u8]);
                }
            }
        }
        for hero in &self.heroes {
            feed(hero.name.as_bytes());
            feed(&[0]);
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
        }
        feed(&self.map.blue_spawn.x.to_bits().to_le_bytes());
        feed(&self.map.blue_spawn.y.to_bits().to_le_bytes());
        feed(&self.map.red_spawn.x.to_bits().to_le_bytes());
        feed(&self.map.red_spawn.y.to_bits().to_le_bytes());
        for lane in &self.map.lanes {
            feed(lane.name.as_bytes());
            feed(&[0]);
            for point in &lane.path {
                feed(&point.x.to_bits().to_le_bytes());
                feed(&point.y.to_bits().to_le_bytes());
            }
        }
        for structure in &self.map.structures {
            feed(&[structure.team.index() as u8, structure.kind as u8]);
            feed(&structure.position.x.to_bits().to_le_bytes());
            feed(&structure.position.y.to_bits().to_le_bytes());
            feed(&[structure.lane.unwrap_or(0xFF)]);
        }
        for wall in &self.map.walls {
            for point in [wall.a, wall.b] {
                feed(&point.x.to_bits().to_le_bytes());
                feed(&point.y.to_bits().to_le_bytes());
            }
        }
        hash
    }
}

/// Errors loading a content pack from TOML.
#[derive(Debug)]
pub enum ContentLoadError {
    /// File could not be read.
    Io(std::io::Error),
    /// TOML could not be parsed.
    Toml(toml::de::Error),
    /// Content failed semantic validation.
    Invalid(ContentError),
}

impl core::fmt::Display for ContentLoadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "cannot read content: {error}"),
            Self::Toml(error) => write!(f, "invalid TOML: {error}"),
            Self::Invalid(error) => write!(f, "invalid content: {error}"),
        }
    }
}

impl std::error::Error for ContentLoadError {}

#[derive(Deserialize)]
struct ContentFile {
    name: Option<String>,
    version: Option<String>,
    author: Option<String>,
    #[serde(default)]
    items: Vec<ItemEntry>,
    heroes: Option<Vec<HeroEntry>>,
    abilities: Option<Vec<AbilityEntry>>,
    map: Option<MapEntry>,
}

#[derive(Deserialize)]
struct MapEntry {
    blue_spawn: [i32; 2],
    red_spawn: [i32; 2],
    lanes: Vec<LaneEntry>,
    #[serde(default)]
    structures: Vec<StructureEntry>,
    #[serde(default)]
    walls: Vec<WallEntry>,
}

#[derive(Deserialize)]
struct WallEntry {
    a: [i32; 2],
    b: [i32; 2],
}

#[derive(Deserialize)]
struct LaneEntry {
    name: String,
    path: Vec<[i32; 2]>,
}

#[derive(Deserialize)]
struct StructureEntry {
    team: String,
    kind: String,
    position: [i32; 2],
    #[serde(default)]
    lane: Option<u8>,
}

#[derive(Deserialize)]
struct ItemEntry {
    name: String,
    cost: u32,
    #[serde(default)]
    health: i32,
    #[serde(default)]
    mana: i32,
    #[serde(default)]
    attack_damage: i32,
    #[serde(default)]
    armor: i32,
    #[serde(default)]
    magic_resist: i32,
    #[serde(default)]
    move_speed: f32,
}

#[derive(Deserialize)]
struct AbilityEntry {
    name: String,
    cooldown_ticks: u32,
    #[serde(default)]
    mana_cost: i32,
    #[serde(default)]
    cast_range: f32,
    kind: String,
    #[serde(default)]
    speed: f32,
    #[serde(default)]
    range: f32,
    #[serde(default)]
    radius: f32,
    #[serde(default)]
    damage: i32,
    #[serde(default)]
    amount: i32,
    #[serde(default)]
    duration_ticks: u32,
    #[serde(default = "default_damage_type")]
    damage_type: String,
}

fn default_damage_type() -> String {
    String::from("physical")
}

#[derive(Deserialize)]
struct HeroEntry {
    name: String,
    max_health: i32,
    max_mana: i32,
    #[serde(default)]
    health_regen_per_tick: i32,
    #[serde(default)]
    mana_regen_per_tick: i32,
    attack_damage: i32,
    attack_range: f32,
    attack_interval_ticks: u32,
    move_speed: f32,
    #[serde(default)]
    armor: i32,
    #[serde(default)]
    magic_resist: i32,
    vision_range: f32,
    abilities: [u8; 3],
}

/// Parses and validates a content pack from TOML.
///
/// # Errors
/// Returns [`ContentLoadError`] for syntax or validation failures.
#[allow(clippy::too_many_lines)] // One declarative conversion for every content table.
pub fn load_toml(text: &str) -> Result<GameContent, ContentLoadError> {
    let file: ContentFile = toml::from_str(text).map_err(ContentLoadError::Toml)?;
    let defaults = GameContent::default();
    let items = file
        .items
        .into_iter()
        .map(|item| ItemSpec {
            name: item.name,
            cost: item.cost,
            health: item.health,
            mana: item.mana,
            attack_damage: item.attack_damage,
            armor: item.armor,
            magic_resist: item.magic_resist,
            move_speed_per_sec: Fix::from_f32(item.move_speed),
        })
        .collect::<Vec<_>>();
    let items = if items.is_empty() {
        defaults.items
    } else {
        items
    };
    let heroes = file.heroes.map_or_else(
        || GameContent::default().heroes,
        |heroes| {
            heroes
                .into_iter()
                .map(|hero| HeroSpec {
                    name: hero.name,
                    max_health: hero.max_health,
                    max_mana: hero.max_mana,
                    health_regen_per_tick: hero.health_regen_per_tick,
                    mana_regen_per_tick: hero.mana_regen_per_tick,
                    attack_damage: hero.attack_damage,
                    attack_range: Fix::from_f32(hero.attack_range),
                    attack_interval_ticks: hero.attack_interval_ticks,
                    move_speed_per_sec: Fix::from_f32(hero.move_speed),
                    armor: hero.armor,
                    magic_resist: hero.magic_resist,
                    vision_range: Fix::from_f32(hero.vision_range),
                    abilities: hero.abilities,
                })
                .collect()
        },
    );
    let abilities = file.abilities.map_or_else(
        || GameContent::default().abilities,
        |abilities| {
            abilities
                .into_iter()
                .map(|ability| {
                    let damage_type = match ability.damage_type.as_str() {
                        "magic" => DamageType::Magic,

                        _ => DamageType::Physical,
                    };
                    let kind = match ability.kind.as_str() {
                        "dash" => AbilityKind::Dash {
                            range: Fix::from_f32(ability.range),
                        },
                        "heal" => AbilityKind::Heal {
                            amount: ability.amount,
                            radius: Fix::from_f32(ability.radius),
                        },
                        "shield" => AbilityKind::Shield {
                            amount: ability.amount,
                            duration_ticks: ability.duration_ticks,
                        },
                        "burst" => AbilityKind::Burst {
                            radius: Fix::from_f32(ability.radius),
                            damage: ability.damage,
                            damage_type,
                        },
                        _ => AbilityKind::Skillshot {
                            speed_per_sec: Fix::from_f32(ability.speed),
                            range: Fix::from_f32(ability.range),
                            radius: Fix::from_f32(ability.radius),
                            damage: ability.damage,
                            damage_type,
                        },
                    };
                    AbilitySpec {
                        name: ability.name,
                        cooldown_ticks: ability.cooldown_ticks,
                        mana_cost: ability.mana_cost,
                        cast_range: Fix::from_f32(ability.cast_range),
                        kind,
                    }
                })
                .collect()
        },
    );
    let map = file.map.map_or_else(
        || GameContent::default().map,
        |map| MapSpec {
            blue_spawn: FixVec2::from_ints(map.blue_spawn[0], map.blue_spawn[1]),
            red_spawn: FixVec2::from_ints(map.red_spawn[0], map.red_spawn[1]),
            lanes: map
                .lanes
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
            structures: map
                .structures
                .into_iter()
                .map(|structure| StructureSpec {
                    team: if structure.team == "red" {
                        Team::Red
                    } else {
                        Team::Blue
                    },
                    position: FixVec2::from_ints(structure.position[0], structure.position[1]),
                    kind: if structure.kind == "nexus" {
                        StructureKind::Nexus
                    } else {
                        StructureKind::Tower
                    },
                    lane: structure.lane,
                })
                .collect(),
            walls: map
                .walls
                .into_iter()
                .map(|wall| WallSpec {
                    a: FixVec2::from_ints(wall.a[0], wall.a[1]),
                    b: FixVec2::from_ints(wall.b[0], wall.b[1]),
                })
                .collect(),
        },
    );
    let defaults = GameContent::default();
    let manifest = ContentManifest {
        name: file.name.unwrap_or(defaults.manifest.name),
        version: file.version.unwrap_or(defaults.manifest.version),
        author: file.author.unwrap_or(defaults.manifest.author),
    };
    let content = GameContent {
        manifest,
        items,
        heroes,
        abilities,
        map,
    };
    content.validate().map_err(ContentLoadError::Invalid)?;
    Ok(content)
}

/// Reads, parses and validates a content pack.
///
/// # Errors
/// Returns [`ContentLoadError`] for I/O, syntax or validation failures.
pub fn load_file(path: &Path) -> Result<GameContent, ContentLoadError> {
    let text = std::fs::read_to_string(path).map_err(ContentLoadError::Io)?;
    load_toml(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_content_is_valid_and_stable() {
        let content = GameContent::default();
        assert_eq!(content.items.len(), ITEMS.len());
        assert_eq!(content.validate(), Ok(()));
        assert_eq!(content.hash(), GameContent::default().hash());
    }

    #[test]
    fn map_only_pack_overlays_defaults() {
        let text = r#"
[map]
blue_spawn = [32, 128]
red_spawn = [224, 128]

[[map.lanes]]
name = "Mid"
path = [[32, 128], [128, 128], [224, 128]]

[[map.structures]]
team = "blue"
kind = "nexus"
position = [20, 128]

[[map.structures]]
team = "red"
kind = "nexus"
position = [236, 128]

[[map.walls]]
a = [80, 96]
b = [80, 160]
"#;
        let content = load_toml(text).expect("map pack");
        assert_eq!(content.items.len(), ITEMS.len());
        assert_eq!(content.map.lanes.len(), 1);
        assert_eq!(content.map.walls.len(), 1);
        assert_eq!(content.map.nexus_position(Team::Blue).x.to_i64(), 20);
    }

    #[test]
    fn example_pack_parses() {
        let content = load_toml(include_str!("../../../content/example-pack.toml"))
            .expect("example pack is valid");
        assert_eq!(content.items.len(), 8);
        assert_eq!(content.heroes.len(), 1);
        assert_eq!(content.heroes[0].name, "Vanguard");
        assert_eq!(content.manifest.name, "Example Pack");
        assert_eq!(content.manifest.version, "1.0.0");
    }

    #[test]
    fn ability_pack_parses_and_heroes_reference_it() {
        let text = r#"
[[items]]
name = "Stick"
cost = 1

[[abilities]]
name = "Zap"
cooldown_ticks = 60
mana_cost = 10
cast_range = 5.0
kind = "burst"
radius = 3.0
damage = 50
damage_type = "magic"

[[heroes]]
name = "Spark"
max_health = 500
max_mana = 300
attack_damage = 40
attack_range = 8.0
attack_interval_ticks = 30
move_speed = 6.0
vision_range = 14.0
abilities = [0, 0, 0]
"#;
        let content = load_toml(text).expect("ability pack");
        assert_eq!(content.abilities.len(), 1);
        assert_eq!(content.abilities[0].name, "Zap");
        assert!(matches!(
            content.abilities[0].kind,
            AbilityKind::Burst { damage: 50, .. }
        ));
    }

    #[test]
    fn manifest_validation_rejects_empty_fields() {
        let mut content = GameContent::default();
        content.manifest.author.clear();
        assert_eq!(content.validate(), Err(ContentError::BadManifest));
    }

    #[test]
    fn hero_validation_rejects_bad_ability_ids() {
        let mut content = GameContent::default();
        content.heroes[0].abilities = [255, 0, 0];
        assert_eq!(content.validate(), Err(ContentError::BadAbility));
        let mut content = GameContent::default();
        content.heroes[0].max_health = 0;
        assert_eq!(content.validate(), Err(ContentError::BadHeroStats));
    }

    #[test]
    fn validation_rejects_bad_names_and_duplicates() {
        let mut content = GameContent::default();
        content.items[0].name.clear();
        assert_eq!(content.validate(), Err(ContentError::BadName));
        let mut content = GameContent::default();
        content.items[1].name = content.items[0].name.clone();
        assert_eq!(content.validate(), Err(ContentError::DuplicateName));
        let mut content = GameContent::default();
        content.items[0].cost = 0;
        assert_eq!(content.validate(), Err(ContentError::FreeItem));
    }
}
