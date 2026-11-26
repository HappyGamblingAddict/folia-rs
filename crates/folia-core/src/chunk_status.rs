//! Named, ordered generation statuses for a chunk.
//!
//! Vanilla upgrades a column one [`ChunkStatus`] at a time. Each status is a
//! checkpoint: once a stage has run, resuming from that status must not repeat
//! the earlier stages. This module only models the status value itself; the
//! driver that runs stages is [`crate::chunk_status_pipeline::ChunkStatusPipeline`].

use std::fmt;
use std::str::FromStr;

/// The generation stages of a chunk, in vanilla order.
///
/// The declaration order is the canonical ordering: deriving `PartialOrd`/`Ord`
/// on a fieldless enum uses the discriminant, so comparisons and sorting agree
/// with [`Self::ordinal`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChunkStatus {
    #[default]
    Empty,
    StructureStarts,
    StructureReferences,
    Biomes,
    Noise,
    Surface,
    Carvers,
    Features,
    InitializeLight,
    Light,
    Spawn,
    Full,
}

impl ChunkStatus {
    /// All statuses in upgrade order. Useful for iterating a whole pipeline.
    pub const ALL: [Self; 12] = [
        Self::Empty,
        Self::StructureStarts,
        Self::StructureReferences,
        Self::Biomes,
        Self::Noise,
        Self::Surface,
        Self::Carvers,
        Self::Features,
        Self::InitializeLight,
        Self::Light,
        Self::Spawn,
        Self::Full,
    ];

    pub const fn count() -> usize {
        Self::ALL.len()
    }

    /// Zero-based position of this status in the upgrade chain.
    pub const fn ordinal(self) -> usize {
        self as usize
    }

    /// Status at a zero-based position, or `None` when out of range.
    pub const fn from_ordinal(ordinal: usize) -> Option<Self> {
        if ordinal < Self::count() {
            // Every discriminant in 0..count has a variant.
            Some(Self::ALL[ordinal])
        } else {
            None
        }
    }

    /// Vanilla resource name without the `minecraft:` namespace.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::StructureStarts => "structure_starts",
            Self::StructureReferences => "structure_references",
            Self::Biomes => "biomes",
            Self::Noise => "noise",
            Self::Surface => "surface",
            Self::Carvers => "carvers",
            Self::Features => "features",
            Self::InitializeLight => "initialize_light",
            Self::Light => "light",
            Self::Spawn => "spawn",
            Self::Full => "full",
        }
    }

    /// Alias for [`Self::name`] matching the usual `as_str` convention.
    pub const fn as_str(self) -> &'static str {
        self.name()
    }

    /// The value written to an Anvil chunk's `Status` field.
    pub const fn to_nbt(self) -> &'static str {
        match self {
            Self::Empty => "minecraft:empty",
            Self::StructureStarts => "minecraft:structure_starts",
            Self::StructureReferences => "minecraft:structure_references",
            Self::Biomes => "minecraft:biomes",
            Self::Noise => "minecraft:noise",
            Self::Surface => "minecraft:surface",
            Self::Carvers => "minecraft:carvers",
            Self::Features => "minecraft:features",
            Self::InitializeLight => "minecraft:initialize_light",
            Self::Light => "minecraft:light",
            Self::Spawn => "minecraft:spawn",
            Self::Full => "minecraft:full",
        }
    }

    /// Parse either a namespaced (`minecraft:noise`) or bare (`noise`) name.
    pub fn parse(value: &str) -> Option<Self> {
        let bare = value.strip_prefix("minecraft:").unwrap_or(value);
        Self::ALL
            .iter()
            .copied()
            .find(|status| status.name() == bare)
    }

    /// Next status in the chain, or `None` for [`Self::Full`].
    pub const fn next(self) -> Option<Self> {
        Self::from_ordinal(self.ordinal() + 1)
    }

    /// Previous status in the chain, or `None` for [`Self::Empty`].
    pub const fn previous(self) -> Option<Self> {
        if self.ordinal() == 0 {
            None
        } else {
            Some(Self::ALL[self.ordinal() - 1])
        }
    }

    pub const fn is_before(self, other: Self) -> bool {
        self.ordinal() < other.ordinal()
    }

    pub const fn is_after(self, other: Self) -> bool {
        self.ordinal() > other.ordinal()
    }

    pub const fn at_or_before(self, other: Self) -> bool {
        self.ordinal() <= other.ordinal()
    }

    pub const fn at_or_after(self, other: Self) -> bool {
        self.ordinal() >= other.ordinal()
    }

    /// The later of two statuses.
    pub const fn max(self, other: Self) -> Self {
        if self.ordinal() >= other.ordinal() {
            self
        } else {
            other
        }
    }

    /// Inclusive chain of statuses from `from` through `to`.
    ///
    /// An empty vector means `to` precedes `from` (a downgrade is not an
    /// upgrade and performs no stages).
    pub fn steps_between(from: Self, to: Self) -> Vec<Self> {
        if from.is_after(to) {
            Vec::new()
        } else {
            (from.ordinal()..=to.ordinal())
                .filter_map(Self::from_ordinal)
                .collect()
        }
    }
}

impl fmt::Display for ChunkStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for ChunkStatus {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bare = value.strip_prefix("minecraft:").unwrap_or(value);
        Self::ALL
            .iter()
            .copied()
            .find(|status| status.name() == bare)
            .ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declares_all_twelve_statuses_in_vanilla_order() {
        assert_eq!(ChunkStatus::count(), 12);
        for (index, status) in ChunkStatus::ALL.iter().copied().enumerate() {
            assert_eq!(status.ordinal(), index);
            assert_eq!(ChunkStatus::from_ordinal(index), Some(status));
        }
        assert_eq!(ChunkStatus::from_ordinal(12), None);
        assert_eq!(ChunkStatus::default(), ChunkStatus::Empty);
    }

    #[test]
    fn ordering_follows_declaration_order() {
        assert!(ChunkStatus::Empty.is_before(ChunkStatus::Full));
        assert!(ChunkStatus::Noise.is_after(ChunkStatus::Biomes));
        assert!(ChunkStatus::Features.at_or_before(ChunkStatus::Features));
        assert!(ChunkStatus::Full.at_or_after(ChunkStatus::Spawn));
        assert!(!ChunkStatus::Full.is_before(ChunkStatus::Full));
        assert_eq!(
            ChunkStatus::Light.max(ChunkStatus::Surface),
            ChunkStatus::Light
        );
        let mut names = vec![ChunkStatus::Full, ChunkStatus::Noise, ChunkStatus::Empty];
        names.sort_unstable();
        assert_eq!(
            names,
            vec![ChunkStatus::Empty, ChunkStatus::Noise, ChunkStatus::Full]
        );
    }

    #[test]
    fn display_and_parse_round_trip_every_status() {
        for status in ChunkStatus::ALL {
            assert_eq!(status.to_string(), status.name());
            assert_eq!(status.as_str(), status.name());
            assert_eq!(ChunkStatus::parse(status.name()), Some(status));
            assert_eq!(ChunkStatus::parse(status.to_nbt()), Some(status));
            assert_eq!(status.to_string().parse::<ChunkStatus>(), Ok(status));
        }
        assert_eq!(
            ChunkStatus::parse("minecraft:structure_references"),
            Some(ChunkStatus::StructureReferences)
        );
        assert_eq!(ChunkStatus::parse("minecraft:unknown"), None);
        assert!("not_a_status".parse::<ChunkStatus>().is_err());
    }

    #[test]
    fn nbt_names_use_the_minecraft_namespace() {
        assert_eq!(ChunkStatus::Empty.to_nbt(), "minecraft:empty");
        assert_eq!(
            ChunkStatus::InitializeLight.to_nbt(),
            "minecraft:initialize_light"
        );
        assert_eq!(ChunkStatus::Full.to_nbt(), "minecraft:full");
    }

    #[test]
    fn next_and_previous_walk_the_chain() {
        assert_eq!(ChunkStatus::Empty.previous(), None);
        assert_eq!(
            ChunkStatus::Empty.next(),
            Some(ChunkStatus::StructureStarts)
        );
        assert_eq!(ChunkStatus::Full.next(), None);
        assert_eq!(ChunkStatus::Full.previous(), Some(ChunkStatus::Spawn));
        let mut status = ChunkStatus::Empty;
        let mut walked = 1;
        while let Some(next) = status.next() {
            status = next;
            walked += 1;
        }
        assert_eq!(status, ChunkStatus::Full);
        assert_eq!(walked, ChunkStatus::count());
    }

    #[test]
    fn steps_between_is_inclusive_and_rejects_downgrades() {
        assert_eq!(
            ChunkStatus::steps_between(ChunkStatus::Noise, ChunkStatus::Surface),
            vec![ChunkStatus::Noise, ChunkStatus::Surface]
        );
        assert_eq!(
            ChunkStatus::steps_between(ChunkStatus::Full, ChunkStatus::Empty),
            Vec::new()
        );
        assert_eq!(
            ChunkStatus::steps_between(ChunkStatus::Full, ChunkStatus::Full),
            vec![ChunkStatus::Full]
        );
    }
}
