//! Dynamic registry tables captured from the 1.21.11 configuration stream.
//!
//! `scripts/dump_registries.ps1` reads `assets/join_config.bin`, splits every
//! `RegistryData` packet into a compact `id<TAB>name` table under
//! `assets/registries/`, and this module embeds those tables. IDs are the
//! protocol IDs the client receives during configuration, so gameplay code can
//! use them without re-parsing the captured packet stream.

use std::collections::HashMap;
use std::sync::OnceLock;

/// A dynamic registry that must be synchronized during configuration.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DynamicRegistry {
    BannerPattern,
    CatVariant,
    ChatType,
    ChickenVariant,
    CowVariant,
    DamageType,
    Dialog,
    DimensionType,
    Enchantment,
    FrogVariant,
    Instrument,
    JukeboxSong,
    PaintingVariant,
    PigVariant,
    TestEnvironment,
    TestInstance,
    Timeline,
    TrimMaterial,
    TrimPattern,
    WolfSoundVariant,
    WolfVariant,
    WorldgenBiome,
    ZombieNautilusVariant,
}

impl DynamicRegistry {
    pub const COUNT: usize = 23;

    /// Every registry, in the same order as [`ASSETS`].
    pub const ALL: [DynamicRegistry; Self::COUNT] = [
        Self::BannerPattern,
        Self::CatVariant,
        Self::ChatType,
        Self::ChickenVariant,
        Self::CowVariant,
        Self::DamageType,
        Self::Dialog,
        Self::DimensionType,
        Self::Enchantment,
        Self::FrogVariant,
        Self::Instrument,
        Self::JukeboxSong,
        Self::PaintingVariant,
        Self::PigVariant,
        Self::TestEnvironment,
        Self::TestInstance,
        Self::Timeline,
        Self::TrimMaterial,
        Self::TrimPattern,
        Self::WolfSoundVariant,
        Self::WolfVariant,
        Self::WorldgenBiome,
        Self::ZombieNautilusVariant,
    ];

    /// Namespaced registry key, e.g. `minecraft:dimension_type`.
    pub fn name(self) -> &'static str {
        match self {
            Self::BannerPattern => "minecraft:banner_pattern",
            Self::CatVariant => "minecraft:cat_variant",
            Self::ChatType => "minecraft:chat_type",
            Self::ChickenVariant => "minecraft:chicken_variant",
            Self::CowVariant => "minecraft:cow_variant",
            Self::DamageType => "minecraft:damage_type",
            Self::Dialog => "minecraft:dialog",
            Self::DimensionType => "minecraft:dimension_type",
            Self::Enchantment => "minecraft:enchantment",
            Self::FrogVariant => "minecraft:frog_variant",
            Self::Instrument => "minecraft:instrument",
            Self::JukeboxSong => "minecraft:jukebox_song",
            Self::PaintingVariant => "minecraft:painting_variant",
            Self::PigVariant => "minecraft:pig_variant",
            Self::TestEnvironment => "minecraft:test_environment",
            Self::TestInstance => "minecraft:test_instance",
            Self::Timeline => "minecraft:timeline",
            Self::TrimMaterial => "minecraft:trim_material",
            Self::TrimPattern => "minecraft:trim_pattern",
            Self::WolfSoundVariant => "minecraft:wolf_sound_variant",
            Self::WolfVariant => "minecraft:wolf_variant",
            Self::WorldgenBiome => "minecraft:worldgen/biome",
            Self::ZombieNautilusVariant => "minecraft:zombie_nautilus_variant",
        }
    }

    /// Short asset file stem (`worldgen_biome.tsv` for `worldgen/biome`).
    pub fn asset_name(self) -> &'static str {
        match self {
            Self::BannerPattern => "banner_pattern",
            Self::CatVariant => "cat_variant",
            Self::ChatType => "chat_type",
            Self::ChickenVariant => "chicken_variant",
            Self::CowVariant => "cow_variant",
            Self::DamageType => "damage_type",
            Self::Dialog => "dialog",
            Self::DimensionType => "dimension_type",
            Self::Enchantment => "enchantment",
            Self::FrogVariant => "frog_variant",
            Self::Instrument => "instrument",
            Self::JukeboxSong => "jukebox_song",
            Self::PaintingVariant => "painting_variant",
            Self::PigVariant => "pig_variant",
            Self::TestEnvironment => "test_environment",
            Self::TestInstance => "test_instance",
            Self::Timeline => "timeline",
            Self::TrimMaterial => "trim_material",
            Self::TrimPattern => "trim_pattern",
            Self::WolfSoundVariant => "wolf_sound_variant",
            Self::WolfVariant => "wolf_variant",
            Self::WorldgenBiome => "worldgen_biome",
            Self::ZombieNautilusVariant => "zombie_nautilus_variant",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

const ASSETS: [&str; DynamicRegistry::COUNT] = [
    include_str!("../assets/registries/banner_pattern.tsv"),
    include_str!("../assets/registries/cat_variant.tsv"),
    include_str!("../assets/registries/chat_type.tsv"),
    include_str!("../assets/registries/chicken_variant.tsv"),
    include_str!("../assets/registries/cow_variant.tsv"),
    include_str!("../assets/registries/damage_type.tsv"),
    include_str!("../assets/registries/dialog.tsv"),
    include_str!("../assets/registries/dimension_type.tsv"),
    include_str!("../assets/registries/enchantment.tsv"),
    include_str!("../assets/registries/frog_variant.tsv"),
    include_str!("../assets/registries/instrument.tsv"),
    include_str!("../assets/registries/jukebox_song.tsv"),
    include_str!("../assets/registries/painting_variant.tsv"),
    include_str!("../assets/registries/pig_variant.tsv"),
    include_str!("../assets/registries/test_environment.tsv"),
    include_str!("../assets/registries/test_instance.tsv"),
    include_str!("../assets/registries/timeline.tsv"),
    include_str!("../assets/registries/trim_material.tsv"),
    include_str!("../assets/registries/trim_pattern.tsv"),
    include_str!("../assets/registries/wolf_sound_variant.tsv"),
    include_str!("../assets/registries/wolf_variant.tsv"),
    include_str!("../assets/registries/worldgen_biome.tsv"),
    include_str!("../assets/registries/zombie_nautilus_variant.tsv"),
];

/// One registry's protocol IDs in wire order.
#[derive(Debug)]
pub struct RegistryTable {
    names: Vec<&'static str>,
    by_name: HashMap<&'static str, i32>,
}

impl RegistryTable {
    fn parse(asset: &'static str) -> Self {
        let mut names: Vec<(&'static str, i32)> = Vec::new();
        for line in asset.lines() {
            let Some((id, name)) = line.split_once('\t') else {
                continue;
            };
            let Ok(id) = id.parse::<i32>() else {
                continue;
            };
            names.push((name, id));
        }
        names.sort_unstable_by_key(|(_, id)| *id);

        let mut by_name = HashMap::with_capacity(names.len());
        let mut ordered = Vec::with_capacity(names.len());
        for (name, id) in names {
            by_name.insert(name, id);
            ordered.push(name);
        }
        Self {
            names: ordered,
            by_name,
        }
    }

    /// Entries in protocol-id order.
    pub fn names(&self) -> &[&'static str] {
        &self.names
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether the registry has no entries.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Protocol ID for a namespaced or bare name.
    pub fn id(&self, name: &str) -> Option<i32> {
        let name = normalize(name);
        self.by_name.get(name.as_ref()).copied()
    }

    /// Name for a protocol ID.
    pub fn name(&self, id: i32) -> Option<&'static str> {
        usize::try_from(id)
            .ok()
            .and_then(|index| self.names.get(index))
            .copied()
    }
}

static TABLES: OnceLock<[RegistryTable; DynamicRegistry::COUNT]> = OnceLock::new();

fn tables() -> &'static [RegistryTable; DynamicRegistry::COUNT] {
    TABLES.get_or_init(|| std::array::from_fn(|index| RegistryTable::parse(ASSETS[index])))
}

fn normalize(name: &str) -> std::borrow::Cow<'_, str> {
    if name.contains(':') {
        std::borrow::Cow::Borrowed(name)
    } else {
        std::borrow::Cow::Owned(format!("minecraft:{name}"))
    }
}

/// Returns the table for `registry`.
pub fn table(registry: DynamicRegistry) -> &'static RegistryTable {
    &tables()[registry.index()]
}

/// Protocol ID for `name` in `registry`, accepting a bare or namespaced name.
pub fn id(registry: DynamicRegistry, name: &str) -> Option<i32> {
    table(registry).id(name)
}

/// Namespaced name for `id` in `registry`.
pub fn name(registry: DynamicRegistry, id: i32) -> Option<&'static str> {
    table(registry).name(id)
}

/// Number of entries in `registry`.
pub fn len(registry: DynamicRegistry) -> usize {
    table(registry).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_table_has_dense_ids_from_zero() {
        for registry in DynamicRegistry::ALL {
            let table = table(registry);
            for (index, name) in table.names().iter().enumerate() {
                let id = i32::try_from(index).expect("registry index fits in i32");
                assert_eq!(table.name(id), Some(*name), "{} id {id}", registry.name());
                assert_eq!(table.id(name), Some(id), "{} name {name}", registry.name());
            }
        }
    }

    #[test]
    fn spot_checks_match_the_captured_configuration_stream() {
        assert_eq!(id(DynamicRegistry::WorldgenBiome, "plains"), Some(40));
        assert_eq!(id(DynamicRegistry::DimensionType, "the_nether"), Some(3));
        assert_eq!(id(DynamicRegistry::DamageType, "arrow"), Some(0));
        assert_eq!(id(DynamicRegistry::Enchantment, "sharpness"), Some(33));
        assert_eq!(id(DynamicRegistry::Instrument, "ponder_goat_horn"), Some(4));
        assert_eq!(id(DynamicRegistry::JukeboxSong, "13"), Some(1));
        assert_eq!(id(DynamicRegistry::PaintingVariant, "alban"), Some(0));
        assert_eq!(id(DynamicRegistry::TrimMaterial, "quartz"), Some(8));
        assert_eq!(id(DynamicRegistry::WolfVariant, "pale"), Some(3));
        assert_eq!(id(DynamicRegistry::BannerPattern, "base"), Some(0));
        assert_eq!(id(DynamicRegistry::ChatType, "chat"), Some(0));

        assert_eq!(
            name(DynamicRegistry::WorldgenBiome, 40),
            Some("minecraft:plains")
        );
        assert_eq!(
            name(DynamicRegistry::DimensionType, 0),
            Some("minecraft:overworld")
        );
        assert_eq!(name(DynamicRegistry::DamageType, 999), None);
        assert_eq!(id(DynamicRegistry::DamageType, "no_such_damage"), None);
    }
}
