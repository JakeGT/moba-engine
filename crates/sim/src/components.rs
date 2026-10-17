//! Simulation components.
//!
//! All values are plain old data: no heap indirection, no floats, deterministic layout.
//! Change tracking for network snapshots comes from the ECS change ticks, not from these
//! structs.

use moba_core::{Fix, FixVec2};
use moba_ecs::Entity;
use moba_game::{DamageType, HeroId};

/// What kind of unit an entity is.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum UnitKind {
    /// A player-controlled hero.
    Hero,
    /// Melee lane minion.
    MeleeMinion,
    /// Ranged lane minion.
    RangedMinion,
    /// Attackable lane/base tower.
    Tower,
    /// The team nexus.
    Nexus,
}

impl UnitKind {
    /// Returns `true` for lane minions.
    #[inline]
    #[must_use]
    pub const fn is_minion(self) -> bool {
        matches!(self, Self::MeleeMinion | Self::RangedMinion)
    }

    /// Returns `true` for towers and nexuses.
    #[inline]
    #[must_use]
    pub const fn is_structure(self) -> bool {
        matches!(self, Self::Tower | Self::Nexus)
    }

    /// Returns `true` for heroes.
    #[inline]
    #[must_use]
    pub const fn is_hero(self) -> bool {
        matches!(self, Self::Hero)
    }
}

/// World position on the ground plane.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Position(pub FixVec2);

/// Hit points.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Health {
    /// Current hit points (`0` while dead).
    pub current: i32,
    /// Maximum hit points.
    pub max: i32,
}

/// Mana.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Mana {
    /// Current mana.
    pub current: i32,
    /// Maximum mana.
    pub max: i32,
}

/// Basic attack stats.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AttackStats {
    /// Damage per hit.
    pub damage: i32,
    /// Attack range in world units.
    pub range: Fix,
    /// Squared range, cached to keep target checks multiplication-free.
    pub range_sq: Fix,
    /// Ticks between attacks.
    pub interval: u32,
    /// Ticks until the next attack is allowed.
    pub cooldown: u32,
}

impl AttackStats {
    /// Creates attack stats and caches the squared range.
    #[must_use]
    pub fn new(damage: i32, range: Fix, interval: u32) -> Self {
        Self {
            damage,
            range,
            range_sq: range * range,
            interval,
            cooldown: 0,
        }
    }
}

/// Armour and magic resistance.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Resistances {
    /// Physical mitigation.
    pub armor: i32,
    /// Magic mitigation.
    pub magic_resist: i32,
}

/// Movement speed in world units per tick.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MoveSpeed(pub Fix);

/// Where a mobile unit is walking.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct MoveTarget(pub Option<FixVec2>);

/// The unit this entity will basic-attack when in range.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct AttackTarget(pub Option<Entity>);

/// Which lane a minion walks and its next waypoint.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LaneWalker {
    /// Lane index.
    pub lane: u8,
    /// Next waypoint index from the team's own base.
    pub waypoint: u16,
}

/// Player slot (`0..10`) controlling a hero.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct HeroSlot(pub u8);

/// Hero identity, for respawn and stats.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct HeroIdentity(pub HeroId);

/// Remaining cooldown in ticks for Q/W/E.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Cooldowns(pub [u32; 3]);

/// Marks a dead hero waiting to respawn.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Dead {
    /// Ticks until respawn.
    pub respawn_in: u32,
}

/// Vision radius. Used by interaction range checks now and fog of war in M5.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct VisionRange(pub Fix);

/// Per-tick regeneration.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Regen {
    /// Health per tick.
    pub health: i32,
    /// Mana per tick.
    pub mana: i32,
}

/// Damage type used by an entity's attacks (basic attacks are physical).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AttackDamageType(pub DamageType);
