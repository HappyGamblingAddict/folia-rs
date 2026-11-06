//! Guards the dynamic registry tables captured from the 1.21.11 configuration
//! stream. Counts are the exact output of `scripts/dump_registries.ps1`.

use folia_core::registries::{self, DynamicRegistry};

#[test]
fn every_dynamic_registry_has_the_expected_entry_count() {
    let expected = [
        (DynamicRegistry::BannerPattern, 43),
        (DynamicRegistry::CatVariant, 11),
        (DynamicRegistry::ChatType, 8),
        (DynamicRegistry::ChickenVariant, 3),
        (DynamicRegistry::CowVariant, 3),
        (DynamicRegistry::DamageType, 50),
        (DynamicRegistry::Dialog, 3),
        (DynamicRegistry::DimensionType, 4),
        (DynamicRegistry::Enchantment, 43),
        (DynamicRegistry::FrogVariant, 3),
        (DynamicRegistry::Instrument, 8),
        (DynamicRegistry::JukeboxSong, 21),
        (DynamicRegistry::PaintingVariant, 51),
        (DynamicRegistry::PigVariant, 3),
        (DynamicRegistry::TestEnvironment, 1),
        (DynamicRegistry::TestInstance, 1),
        (DynamicRegistry::Timeline, 4),
        (DynamicRegistry::TrimMaterial, 11),
        (DynamicRegistry::TrimPattern, 18),
        (DynamicRegistry::WolfSoundVariant, 7),
        (DynamicRegistry::WolfVariant, 9),
        (DynamicRegistry::WorldgenBiome, 65),
        (DynamicRegistry::ZombieNautilusVariant, 2),
    ];

    assert_eq!(expected.len(), DynamicRegistry::COUNT);
    for (registry, count) in expected {
        assert_eq!(
            registries::len(registry),
            count,
            "{} entry count changed",
            registry.name()
        );
    }
}

#[test]
fn ids_are_dense_and_round_trip_to_names() {
    for registry in DynamicRegistry::ALL {
        let table = registries::table(registry);
        assert_eq!(table.len(), table.names().len());
        for (index, name) in table.names().iter().enumerate() {
            let id = i32::try_from(index).expect("registry size fits in i32");
            assert_eq!(table.name(id), Some(*name), "{} id {id}", registry.name());
            assert_eq!(table.id(name), Some(id), "{} name {name}", registry.name());
        }
    }
}

#[test]
fn namespaced_and_bare_names_resolve_the_same_entry() {
    assert_eq!(
        registries::id(DynamicRegistry::WorldgenBiome, "plains"),
        registries::id(DynamicRegistry::WorldgenBiome, "minecraft:plains")
    );
    assert_eq!(registries::name(DynamicRegistry::DamageType, 999), None);
    assert_eq!(
        registries::id(DynamicRegistry::DamageType, "no_such_damage"),
        None
    );
}
