//! The match map: lanes, structures and spawn points.
//!
//! One map ships with the vertical slice: a 256×256 symmetric three-lane map. Coordinates
//! are fixed-point world units. The simulation never reads a texture or navmesh at this
//! milestone; minions follow the lane waypoint lists and heroes steer in a straight line.

use moba_core::{Fix, FixVec2};

use crate::team::Team;

/// One lane's path from the blue base to the red base.
#[derive(Copy, Clone, Debug)]
pub struct LaneDef {
    /// Stable lane index (0 = top, 1 = mid, 2 = bottom).
    pub id: u8,
    /// Display name.
    pub name: &'static str,
    /// Waypoints ordered from blue spawn to red spawn.
    pub path: &'static [FixVec2],
}

impl LaneDef {
    /// Number of waypoints.
    #[inline]
    #[must_use]
    pub const fn len(&self) -> usize {
        self.path.len()
    }

    /// Returns `true` when the lane has no waypoints (never true for shipped maps).
    #[inline]
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.path.is_empty()
    }

    /// Waypoint at `index` measured from `team`'s own base.
    ///
    /// Blue walks the list forwards, red walks it backwards. This keeps one source of
    /// truth for lane geometry.
    #[inline]
    #[must_use]
    pub fn point(&self, team: Team, index: usize) -> FixVec2 {
        match team {
            Team::Blue => self.path[index.min(self.path.len() - 1)],
            Team::Red => self.path[self.path.len() - 1 - index.min(self.path.len() - 1)],
        }
    }
}

/// What a structure is.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StructureKind {
    /// An attackable lane/base tower.
    Tower,
    /// The team's nexus; destroying it ends the match.
    Nexus,
}

/// A placed structure.
#[derive(Copy, Clone, Debug)]
pub struct StructureDef {
    /// Owning team.
    pub team: Team,
    /// World position.
    pub position: FixVec2,
    /// Structure type.
    pub kind: StructureKind,
    /// Lane index for towers, `None` for nexus structures.
    pub lane: Option<u8>,
}

/// Static map description.
#[derive(Copy, Clone, Debug)]
pub struct MapDef {
    /// All lanes.
    pub lanes: &'static [LaneDef],
    /// All structures.
    pub structures: &'static [StructureDef],
    /// Blue team spawn point.
    pub blue_spawn: FixVec2,
    /// Red team spawn point.
    pub red_spawn: FixVec2,
}

impl MapDef {
    /// Team spawn point.
    #[inline]
    #[must_use]
    pub const fn spawn(self, team: Team) -> FixVec2 {
        match team {
            Team::Blue => self.blue_spawn,
            Team::Red => self.red_spawn,
        }
    }

    /// A hero's spawn position for a slot in `0..5`.
    #[must_use]
    pub fn hero_spawn(self, team: Team, slot: u8) -> FixVec2 {
        let base = self.spawn(team);
        let offsets = [(0, 0), (4, -7), (4, 7), (-4, -12), (-4, 12)];
        let (dx, dy) = offsets[slot as usize % offsets.len()];
        let dx = if team == Team::Blue { dx } else { -dx };
        FixVec2::new(base.x + Fix::from_i32(dx), base.y + Fix::from_i32(dy))
    }

    /// The nexus position for a team.
    #[must_use]
    pub fn nexus_position(self, team: Team) -> FixVec2 {
        self.structures
            .iter()
            .find(|structure| structure.team == team && structure.kind == StructureKind::Nexus)
            .map_or(self.spawn(team), |structure| structure.position)
    }
}

const fn p(x: i32, y: i32) -> FixVec2 {
    FixVec2::from_ints(x, y)
}

const TOP_PATH: [FixVec2; 9] = [
    p(52, 128),
    p(44, 72),
    p(60, 44),
    p(92, 28),
    p(128, 24),
    p(164, 28),
    p(196, 44),
    p(212, 72),
    p(204, 128),
];

const MID_PATH: [FixVec2; 3] = [p(52, 128), p(128, 128), p(204, 128)];

const BOT_PATH: [FixVec2; 9] = [
    p(52, 128),
    p(44, 184),
    p(60, 212),
    p(92, 228),
    p(128, 232),
    p(164, 228),
    p(196, 212),
    p(212, 184),
    p(204, 128),
];

/// All lanes in stable order.
pub const LANES: [LaneDef; 3] = [
    LaneDef {
        id: 0,
        name: "Top",
        path: &TOP_PATH,
    },
    LaneDef {
        id: 1,
        name: "Mid",
        path: &MID_PATH,
    },
    LaneDef {
        id: 2,
        name: "Bottom",
        path: &BOT_PATH,
    },
];

/// All structures in stable order.
pub const STRUCTURES: [StructureDef; 16] = [
    // Blue lane towers.
    StructureDef {
        team: Team::Blue,
        position: p(44, 84),
        kind: StructureKind::Tower,
        lane: Some(0),
    },
    StructureDef {
        team: Team::Blue,
        position: p(92, 32),
        kind: StructureKind::Tower,
        lane: Some(0),
    },
    StructureDef {
        team: Team::Blue,
        position: p(84, 128),
        kind: StructureKind::Tower,
        lane: Some(1),
    },
    StructureDef {
        team: Team::Blue,
        position: p(124, 128),
        kind: StructureKind::Tower,
        lane: Some(1),
    },
    StructureDef {
        team: Team::Blue,
        position: p(44, 172),
        kind: StructureKind::Tower,
        lane: Some(2),
    },
    StructureDef {
        team: Team::Blue,
        position: p(92, 224),
        kind: StructureKind::Tower,
        lane: Some(2),
    },
    // Red lane towers (mirrored).
    StructureDef {
        team: Team::Red,
        position: p(212, 84),
        kind: StructureKind::Tower,
        lane: Some(0),
    },
    StructureDef {
        team: Team::Red,
        position: p(164, 32),
        kind: StructureKind::Tower,
        lane: Some(0),
    },
    StructureDef {
        team: Team::Red,
        position: p(172, 128),
        kind: StructureKind::Tower,
        lane: Some(1),
    },
    StructureDef {
        team: Team::Red,
        position: p(132, 128),
        kind: StructureKind::Tower,
        lane: Some(1),
    },
    StructureDef {
        team: Team::Red,
        position: p(212, 172),
        kind: StructureKind::Tower,
        lane: Some(2),
    },
    StructureDef {
        team: Team::Red,
        position: p(164, 224),
        kind: StructureKind::Tower,
        lane: Some(2),
    },
    // Nexus towers.
    StructureDef {
        team: Team::Blue,
        position: p(44, 128),
        kind: StructureKind::Tower,
        lane: None,
    },
    StructureDef {
        team: Team::Red,
        position: p(212, 128),
        kind: StructureKind::Tower,
        lane: None,
    },
    // Nexuses.
    StructureDef {
        team: Team::Blue,
        position: p(28, 128),
        kind: StructureKind::Nexus,
        lane: None,
    },
    StructureDef {
        team: Team::Red,
        position: p(228, 128),
        kind: StructureKind::Nexus,
        lane: None,
    },
];

/// The map used by the vertical slice.
pub const MAP: MapDef = MapDef {
    lanes: &LANES,
    structures: &STRUCTURES,
    blue_spawn: p(52, 128),
    red_spawn: p(204, 128),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_is_symmetric_and_complete() {
        assert_eq!(LANES.len(), 3);
        assert_eq!(STRUCTURES.len(), 16);
        for lane in &LANES {
            assert_eq!(lane.path[0], MAP.blue_spawn);
            assert_eq!(lane.path[lane.path.len() - 1], MAP.red_spawn);
            assert_eq!(lane.point(Team::Blue, 0), MAP.blue_spawn);
            assert_eq!(lane.point(Team::Red, 0), MAP.red_spawn);
            assert_eq!(lane.point(Team::Blue, lane.len() - 1), MAP.red_spawn);
            assert_eq!(lane.point(Team::Red, lane.len() - 1), MAP.blue_spawn);
        }

        for team in Team::ALL {
            let towers = STRUCTURES
                .iter()
                .filter(|s| s.team == team && s.kind == StructureKind::Tower)
                .count();
            let nexuses = STRUCTURES
                .iter()
                .filter(|s| s.team == team && s.kind == StructureKind::Nexus)
                .count();
            assert_eq!(towers, 7);
            assert_eq!(nexuses, 1);
        }

        assert_ne!(
            MAP.nexus_position(Team::Blue),
            MAP.nexus_position(Team::Red)
        );
        for slot in 0..5 {
            let blue = MAP.hero_spawn(Team::Blue, slot);
            let red = MAP.hero_spawn(Team::Red, slot);
            assert!(blue.x < Fix::from_i32(80));
            assert!(red.x > Fix::from_i32(176));
        }
    }
}
