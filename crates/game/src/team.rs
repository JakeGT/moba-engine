//! The two teams.

/// A team in a match.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Team {
    /// Blue team (spawns on the low-X side).
    Blue = 0,
    /// Red team (spawns on the high-X side).
    Red = 1,
}

impl Team {
    /// Both teams, in stable order.
    pub const ALL: [Self; 2] = [Self::Blue, Self::Red];

    /// The opposing team.
    #[inline]
    #[must_use]
    pub const fn enemy(self) -> Self {
        match self {
            Self::Blue => Self::Red,
            Self::Red => Self::Blue,
        }
    }

    /// Dense index for arrays.
    #[inline]
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teams_are_symmetric() {
        assert_eq!(Team::Blue.enemy(), Team::Red);
        assert_eq!(Team::Red.enemy(), Team::Blue);
        assert_eq!(Team::Blue.index(), 0);
        assert_eq!(Team::Red.index(), 1);
    }
}
