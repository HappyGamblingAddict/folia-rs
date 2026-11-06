//! Focused tests for the natural-spawning foundation.

use folia_core::entity_ai::MemoryWorld;
use folia_core::math::{BlockPos, Vec3};
use folia_core::random::LegacyRandomSource;
use folia_core::spawn::{
    ChunkPos, ChunkSpawnResult, MobCategory, NaturalSpawner, SpawnFailure, SpawnSettings,
    SpawnState, biome_spawn_count, biome_spawns, entity_type, entity_type_by_id,
    is_spawnable_position, spawn_category_for_position, spawn_for_chunk,
};

fn flat_world() -> MemoryWorld {
    let mut world = MemoryWorld::flat(63, 320, 63, 32);
    world.set_default_biome("minecraft:plains");
    world.set_default_sky_light(0);
    world.set_default_block_light(0);
    world
}

fn far_player() -> Vec3 {
    Vec3::new(40.5, 64.0, 0.5)
}

fn spawn_until_result(seed: i64) -> ChunkSpawnResult {
    let world = flat_world();
    let mut state = SpawnState::new(289);
    let mut random = LegacyRandomSource::new(seed);
    let settings = SpawnSettings::default();
    let players = [far_player()];
    for _ in 0..200 {
        let result = spawn_for_chunk(
            &mut state,
            &world,
            ChunkPos::new(0, 0),
            MobCategory::Monster,
            &settings,
            &players,
            &mut random,
        );
        if !result.spawned.is_empty() {
            return result;
        }
    }
    panic!("expected a monster pack within 200 attempts");
}

#[test]
fn category_caps_enforce_the_global_limit() {
    let world = flat_world();
    let players = [far_player()];
    let settings = SpawnSettings::default();
    let mut random = LegacyRandomSource::new(11);

    let mut state = SpawnState::new(289);
    state.set_count(MobCategory::Monster, 70);
    let capped = spawn_for_chunk(
        &mut state,
        &world,
        ChunkPos::new(0, 0),
        MobCategory::Monster,
        &settings,
        &players,
        &mut random,
    );
    assert!(capped.spawned.is_empty());
    assert_eq!(capped.failure, Some(SpawnFailure::Capacity));

    state.set_count(MobCategory::Monster, 69);
    let mut spawned = false;
    for _ in 0..200 {
        let result = spawn_for_chunk(
            &mut state,
            &world,
            ChunkPos::new(0, 0),
            MobCategory::Monster,
            &settings,
            &players,
            &mut random,
        );
        if !result.spawned.is_empty() {
            spawned = true;
            break;
        }
    }
    assert!(spawned, "one slot below the cap must allow a spawn");
    assert!(state.count(MobCategory::Monster) > 69);
}

#[test]
fn light_and_height_reject_unspawnable_positions() {
    let mut world = flat_world();
    let zombie = *entity_type("minecraft:zombie").expect("zombie type");
    let cow = *entity_type("minecraft:cow").expect("cow type");
    let mut random = LegacyRandomSource::new(5);

    assert!(is_spawnable_position(
        &world,
        BlockPos::new(0, 64, 0),
        &zombie,
        &mut random
    ));
    assert!(!is_spawnable_position(
        &world,
        BlockPos::new(0, 65, 0),
        &zombie,
        &mut random
    ));

    world.set_default_sky_light(15);
    assert!(!is_spawnable_position(
        &world,
        BlockPos::new(0, 64, 0),
        &zombie,
        &mut random
    ));

    world.set_default_sky_light(0);
    assert!(!is_spawnable_position(
        &world,
        BlockPos::new(0, 64, 0),
        &cow,
        &mut random
    ));
    world.set_default_sky_light(9);
    assert!(is_spawnable_position(
        &world,
        BlockPos::new(0, 64, 0),
        &cow,
        &mut random
    ));
}

#[test]
fn distance_gate_rejects_spawns_next_to_the_player() {
    let world = flat_world();
    let mut state = SpawnState::new(289);
    let settings = SpawnSettings::default();
    let players = [Vec3::new(0.5, 64.0, 0.5)];
    let mut random = LegacyRandomSource::new(3);

    let result = spawn_category_for_position(
        &mut state,
        &world,
        BlockPos::new(0, 64, 0),
        MobCategory::Monster,
        &settings,
        &players,
        &mut random,
    );

    assert!(result.spawned.is_empty());
    assert_eq!(result.failure, Some(SpawnFailure::Distance));
}

#[test]
fn fixed_seed_produces_identical_packs() {
    let first = spawn_until_result(2026);
    let second = spawn_until_result(2026);

    assert_eq!(first, second);
    assert!(!first.spawned.is_empty());
    assert!(first.spawned.len() <= 4);

    let min_x = first.spawned.iter().map(|mob| mob.pos.x()).min().unwrap();
    let max_x = first.spawned.iter().map(|mob| mob.pos.x()).max().unwrap();
    let min_y = first.spawned.iter().map(|mob| mob.pos.y()).min().unwrap();
    let max_y = first.spawned.iter().map(|mob| mob.pos.y()).max().unwrap();
    let min_z = first.spawned.iter().map(|mob| mob.pos.z()).min().unwrap();
    let max_z = first.spawned.iter().map(|mob| mob.pos.z()).max().unwrap();
    assert!(max_x - min_x <= 2);
    assert!(max_y - min_y <= 2);
    assert!(max_z - min_z <= 2);
}

#[test]
fn real_entity_ids_and_biome_lists_are_used() {
    assert_eq!(biome_spawn_count(), 65);
    let plains = biome_spawns("plains").expect("plains biome");
    assert!(
        plains
            .spawners(MobCategory::Monster)
            .iter()
            .any(|spawner| { spawner.entity_type == "minecraft:zombie" })
    );
    assert!(
        plains
            .spawners(MobCategory::Creature)
            .iter()
            .any(|spawner| { spawner.entity_type == "minecraft:sheep" })
    );

    let zombie = entity_type("minecraft:zombie").expect("zombie");
    assert_eq!(zombie.id(), 150);
    assert_eq!(
        entity_type_by_id(150).map(|entity| entity.name),
        Some("minecraft:zombie")
    );
    assert_eq!(zombie.category, MobCategory::Monster);
}

#[test]
fn spawn_cycle_rotates_categories_deterministically() {
    let world = flat_world();
    let players = [far_player()];
    let settings = SpawnSettings {
        max_spawns_per_chunk: 4,
        ..Default::default()
    };

    let mut first = NaturalSpawner::new(289, settings.clone());
    let mut second = NaturalSpawner::new(289, settings);
    let mut first_random = LegacyRandomSource::new(77);
    let mut second_random = LegacyRandomSource::new(77);

    let mut spawned_any = false;
    for _ in 0..20 {
        let first_results =
            first.run_cycle(&world, ChunkPos::new(0, 0), &players, &mut first_random);
        let second_results =
            second.run_cycle(&world, ChunkPos::new(0, 0), &players, &mut second_random);
        assert_eq!(first_results.len(), MobCategory::SPAWNING.len());
        assert_eq!(first_results, second_results);
        spawned_any |= first_results
            .iter()
            .any(|result| !result.spawned.is_empty());
    }

    assert!(
        spawned_any,
        "at least one category should spawn in the plains test world"
    );
}
