//! Simulation components.
//!
//! All values are plain old data: no heap indirection, no floats, deterministic layout.
//! Change tracking for network snapshots comes from the ECS change ticks, not from these
//! structs.

use moba_core::{Fix, FixVec2};
use moba_ecs::Entity;
use moba_game::{DamageType, HeroId, ModifierKind, Team};

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

/// Maximum simultaneous modifiers on one unit.
pub const MAX_MODIFIERS: usize = 8;

/// One active modifier instance.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Modifier {
    /// Content definition id.
    pub def_id: u16,
    /// Definition/behaviour kind.
    pub kind: ModifierKind,
    /// Remaining duration in ticks.
    pub remaining: u32,
    /// Movement multiplier (slow/haste), otherwise one.
    pub factor: Fix,
    /// Shield absorption or per-tick burn damage.
    pub amount: i32,
    /// Burn mitigation type.
    pub damage_type: moba_game::DamageType,
    /// Entity that applied the modifier (kill/credit source).
    pub source: Entity,
}

impl Default for Modifier {
    fn default() -> Self {
        Self {
            def_id: 0,
            kind: ModifierKind::Shield,
            remaining: 0,
            factor: Fix::ONE,
            amount: 0,
            damage_type: moba_game::DamageType::Physical,
            source: Entity::from_bits(0),
        }
    }
}

/// Always-present fixed-capacity modifier list.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Modifiers {
    pub(crate) entries: [Modifier; MAX_MODIFIERS],
    pub(crate) len: u8,
}

impl Default for Modifiers {
    fn default() -> Self {
        Self {
            entries: [Modifier::default(); MAX_MODIFIERS],
            len: 0,
        }
    }
}

impl Modifiers {
    /// Active modifiers in application order.
    #[must_use]
    pub fn entries(&self) -> &[Modifier] {
        &self.entries[..usize::from(self.len)]
    }

    /// Whether any active modifier has `kind`.
    #[must_use]
    pub fn has(&self, kind: ModifierKind) -> bool {
        self.entries().iter().any(|modifier| modifier.kind == kind)
    }

    /// Product of all movement factors (slow/haste), or one.
    #[must_use]
    pub fn move_factor(&self) -> Fix {
        self.entries()
            .iter()
            .filter(|modifier| matches!(modifier.kind, ModifierKind::Slow | ModifierKind::Haste))
            .fold(Fix::ONE, |factor, modifier| factor * modifier.factor)
    }

    /// Total remaining shield absorption.
    #[must_use]
    pub fn shield_total(&self) -> i32 {
        self.entries()
            .iter()
            .filter(|modifier| modifier.kind == ModifierKind::Shield)
            .map(|modifier| modifier.amount.max(0))
            .sum()
    }

    /// Adds a modifier when capacity remains.
    pub fn push(&mut self, modifier: Modifier) -> bool {
        if usize::from(self.len) >= MAX_MODIFIERS {
            return false;
        }
        self.entries[usize::from(self.len)] = modifier;
        self.len += 1;
        true
    }

    /// Removes every modifier.
    pub fn clear(&mut self) {
        self.len = 0;
    }
}

/// Mana.
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

/// Movement speed in world units per tick (effective, after modifiers).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MoveSpeed(pub Fix);

/// Base movement speed from hero stats and items, before temporary modifiers.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BaseMoveSpeed(pub Fix);

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

/// The unit and team that landed the most recent damage (kill credit).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LastHitBy {
    /// Team of the last attacker.
    pub team: Team,
    /// Last attacking entity.
    pub entity: Entity,
}

/// Hero level, experience, gold and inventory.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Progression {
    /// Current level (`1..=LEVEL_CAP`).
    pub level: u8,
    /// Experience towards the next level.
    pub xp: u32,
    /// Spendable gold.
    pub gold: u32,
    /// Purchased item ids, `moba_game::EMPTY_ITEM` for empty slots.
    pub items: [u8; moba_game::ITEM_SLOTS],
    /// Number of filled inventory slots.
    pub item_count: u8,
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
