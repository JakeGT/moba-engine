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
}

impl Default for GameContent {
    fn default() -> Self {
        Self {
            items: ITEMS.iter().map(ItemSpec::from).collect(),
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
}

impl core::fmt::Display for ContentError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoItems => f.write_str("content must define at least one item"),
            Self::TooManyItems => write!(f, "content has more than {MAX_CONTENT_ITEMS} items"),
            Self::BadName => f.write_str("item names must be 1..=16 ASCII bytes"),
            Self::DuplicateName => f.write_str("item names must be unique"),
            Self::FreeItem => f.write_str("item costs must be greater than zero"),
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
