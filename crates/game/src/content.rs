//! Runtime content definitions shared by the simulation and server.
//!
//! The default catalogue is derived from the compile-time [`ITEMS`] table, so
//! existing games and replays keep working. Servers can load a different item catalogue and
//! publish it to clients in the handshake; clients render and shop against the received
//! catalogue. The catalogue is deliberately small and fixed-capacity on the wire.

use moba_core::Fix;

use crate::{ITEMS, ItemDef};

/// Maximum number of items in a content pack (matches the wire catalogue).
pub const MAX_CONTENT_ITEMS: usize = 16;
/// Maximum item display-name length in bytes.
pub const MAX_ITEM_NAME: usize = 16;
/// Maximum heroes in a content pack.
pub const MAX_CONTENT_HEROES: usize = 8;
/// Maximum hero display-name length in bytes.
pub const MAX_HERO_NAME: usize = 16;

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

impl HeroSpec {
    /// Returns one static ability definition for a slot, if the id is valid.
    #[must_use]
    pub fn ability(&self, slot: usize) -> Option<&'static crate::AbilityDef> {
        self.abilities
            .get(slot)
            .and_then(|&id| crate::ABILITIES.get(usize::from(id)))
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
    /// Item catalogue in stable id order.
    pub items: Vec<ItemSpec>,
    /// Hero catalogue in stable id order.
    pub heroes: Vec<HeroSpec>,
}

impl Default for GameContent {
    fn default() -> Self {
        Self {
            items: ITEMS.iter().map(ItemSpec::from).collect(),
            heroes: crate::HEROES.iter().map(HeroSpec::from).collect(),
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
        }
    }
}

impl std::error::Error for ContentError {}

impl GameContent {
    /// Validates names, counts and costs.
    ///
    /// # Errors
    /// Returns the first [`ContentError`] found.
    pub fn validate(&self) -> Result<(), ContentError> {
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
                .any(|&id| usize::from(id) >= crate::ABILITIES.len())
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
    pub fn hash(&self) -> u64 {
        let mut hash = 0xCBF2_9CE4_8422_2325u64;
        let mut feed = |bytes: &[u8]| {
            for byte in bytes {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
            }
        };
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
        hash
    }
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
