//! Validation ([`ContentError`]) and loading ([`ContentLoadError`]) errors.

use super::{MAX_CONTENT_ABILITIES, MAX_CONTENT_HEROES, MAX_CONTENT_ITEMS};

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
    /// An item has an out-of-range stat.
    BadItemStats,
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
    /// Modifier definitions are invalid.
    BadModifiers,
    /// Runtime rules field is invalid; the name identifies the field.
    BadRules(&'static str),
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
            Self::BadItemStats => f.write_str("item stats are out of range"),
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
            Self::BadModifiers => f.write_str("modifier names/kinds are invalid or duplicated"),
            Self::BadRules(field) => write!(f, "invalid rules field: {field}"),
            Self::BadMap => f.write_str("map limits are invalid"),
            Self::BadMapData => f.write_str("map lanes/structures are invalid"),
            Self::BadWalls => f.write_str("map walls are invalid"),
            Self::BadManifest => f.write_str("manifest fields must be 1..=32 ASCII bytes"),
        }
    }
}

impl std::error::Error for ContentError {}

/// Errors loading a content pack from TOML.
#[derive(Debug)]
pub enum ContentLoadError {
    /// File could not be read.
    Io(std::io::Error),
    /// TOML could not be parsed.
    Toml(toml::de::Error),
    /// Content failed semantic validation.
    Invalid(ContentError),
    /// A field has an unsupported value.
    InvalidField {
        /// Dotted field name.
        field: &'static str,
        /// Rejected value.
        value: String,
    },
}

impl core::fmt::Display for ContentLoadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "cannot read content: {error}"),
            Self::Toml(error) => write!(f, "invalid TOML: {error}"),
            Self::Invalid(error) => write!(f, "invalid content: {error}"),
            Self::InvalidField { field, value } => {
                write!(f, "invalid value {value:?} for {field}")
            }
        }
    }
}

impl std::error::Error for ContentLoadError {}
