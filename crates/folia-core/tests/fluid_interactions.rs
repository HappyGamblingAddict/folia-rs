//! Focused end-to-end tests for the vanilla fluid interaction rules:
//! lava/water quenching, lava fire, bubble columns, plant destruction, and
//! deterministic interaction ordering. These drive the real `tick_world` loop
//! over persisted chunks.

use folia_core::anvil::Compression;
use folia_core::block_update;
use folia_core::chunk::{BlockState, Chunk};
use folia_core::chunk_packet::flat_chunk;
use folia_core::fluid::{self, BubbleColumnPlacement, FluidKind};
use folia_core::math::{BlockPos, Direction};
use folia_core::registry::default_block_state;
use folia_core::tick::{self, TickPriority};
use folia_core::world::WorldStorage;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-fluid-interactions-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn solid(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: Vec::new(),
    }
}

fn water(level: u8) -> BlockState {
    BlockState {
        name: "minecraft:water".into(),
        properties: vec![("level".into(), level.to_string())],
    }
}

fn lava(level: u8) -> BlockState {
    BlockState {
        name: "minecraft:lava".into(),
        properties: vec![("level".into(), level.to_string())],
    }
}

fn named(name: &str) -> BlockState {
    default_block_state(name).unwrap_or_else(|| solid(name))
}

/// A flat stone floor at y=-61 over the flat fixture's dirt/bedrock column.
fn floor_chunk() -> Chunk {
    let mut chunk = flat_chunk(0, 0);
    for x in 0..16 {
        for z in 0..16 {
            chunk.set_block_state(x, -61, z, solid("minecraft:stone"));
        }
    }
    chunk
}

fn storage_with_chunk(name: &str, chunk: Chunk) -> WorldStorage {
    let root = temp_dir(name);
    let mut storage = WorldStorage::new(root.join("region"));
    storage.save_chunk(&chunk, Compression::Zlib).unwrap();
    storage.flush().unwrap();
    storage
}

fn floor_storage(name: &str) -> WorldStorage {
    storage_with_chunk(name, floor_chunk())
}

fn place(storage: &mut WorldStorage, x: i32, y: i32, z: i32, state: BlockState) {
    block_update::set_block(storage, x, y, z, state)
        .unwrap()
        .expect("placement changed the block");
}

fn block(storage: &WorldStorage, x: i32, y: i32, z: i32) -> BlockState {
    storage
        .block_state_at(x, y, z)
        .expect("block is in a cached chunk")
}

fn block_name(storage: &WorldStorage, x: i32, y: i32, z: i32) -> String {
    block(storage, x, y, z).name
}

fn fluid_level(storage: &WorldStorage, x: i32, y: i32, z: i32) -> Option<u8> {
    let state = block(storage, x, y, z);
    if state.name != "minecraft:water" && state.name != "minecraft:lava" {
        return None;
    }
    fluid::property(&state, "level").and_then(|value| value.parse::<u8>().ok())
}

fn drag(storage: &WorldStorage, x: i32, y: i32, z: i32) -> Option<bool> {
    fluid::bubble_column_drag(&block(storage, x, y, z))
}

fn run_ticks(storage: &mut WorldStorage, from: u64, to: u64) {
    for tick_number in from..=to {
        tick::tick_world(storage, tick_number).unwrap();
    }
}

fn fire_positions(storage: &WorldStorage, center: BlockPos, radius: i32) -> Vec<BlockPos> {
    let mut positions = Vec::new();
    for dx in -radius..=radius {
        for dy in -1..=3 {
            for dz in -radius..=radius {
                let pos = center.offset(dx, dy, dz);
                if block_name(storage, pos.x(), pos.y(), pos.z()) == "minecraft:fire" {
                    positions.push(pos);
                }
            }
        }
    }
    positions
}

fn has_flammable_neighbour(storage: &WorldStorage, pos: BlockPos) -> bool {
    Direction::ALL.iter().any(|direction| {
        let neighbor = pos.relative(*direction);
        let state = block(storage, neighbor.x(), neighbor.y(), neighbor.z());
        fluid::ignited_by_lava(&state)
    })
}

#[test]
fn lava_source_next_to_water_becomes_obsidian() {
    let mut storage = floor_storage("lava-source-side");

    place(&mut storage, 8, -60, 8, lava(0));
    place(&mut storage, 9, -60, 8, water(0));

    assert_eq!(
        block_name(&storage, 8, -60, 8),
        "minecraft:obsidian",
        "LiquidBlock.shouldSpreadLiquid converts a lava source touching water"
    );
    assert_eq!(
        block_name(&storage, 9, -60, 8),
        "minecraft:water",
        "the water source is not consumed"
    );
}

#[test]
fn water_above_lava_source_becomes_obsidian() {
    let mut storage = floor_storage("lava-source-above");

    place(&mut storage, 8, -60, 8, lava(0));
    place(&mut storage, 8, -59, 8, water(0));

    assert_eq!(
        block_name(&storage, 8, -60, 8),
        "minecraft:obsidian",
        "the reversed DOWN face checks the block above the lava source"
    );
}

#[test]
fn flowing_lava_next_to_water_becomes_cobblestone() {
    let mut storage = floor_storage("lava-flowing-side");

    place(&mut storage, 8, -60, 8, lava(0));
    run_ticks(&mut storage, 1, 30);
    assert_eq!(
        fluid_level(&storage, 9, -60, 8),
        Some(2),
        "the source has spread one lava level east"
    );

    // (9, -60, 7) is orthogonal to the flowing lava at (9, -60, 8) but diagonal
    // to the source at (8, -60, 8), so only the flowing block may react.
    place(&mut storage, 9, -60, 7, water(0));

    assert_eq!(
        block_name(&storage, 9, -60, 8),
        "minecraft:cobblestone",
        "flowing lava quenches to cobblestone"
    );
    assert_eq!(
        block_name(&storage, 8, -60, 8),
        "minecraft:lava",
        "the diagonal water did not touch the source"
    );
}

#[test]
fn water_above_flowing_lava_becomes_cobblestone() {
    let mut storage = floor_storage("lava-flowing-above");

    place(&mut storage, 8, -60, 8, lava(0));
    run_ticks(&mut storage, 1, 30);
    assert_eq!(fluid_level(&storage, 9, -60, 8), Some(2));

    place(&mut storage, 9, -59, 8, water(0));

    assert_eq!(
        block_name(&storage, 9, -60, 8),
        "minecraft:cobblestone",
        "water above a flowing lava block produces cobblestone"
    );
}

#[test]
fn lava_source_above_water_flows_down_and_becomes_stone() {
    let mut storage = floor_storage("lava-source-over-water");

    place(&mut storage, 8, -61, 8, water(0));
    place(&mut storage, 8, -60, 8, lava(0));

    assert_eq!(
        block_name(&storage, 8, -60, 8),
        "minecraft:lava",
        "water below is not one of LiquidBlock's five contact faces"
    );

    run_ticks(&mut storage, 1, 30);

    assert_eq!(
        block_name(&storage, 8, -61, 8),
        "minecraft:stone",
        "LavaFluid.spreadTo turns lava flowing down into water into stone"
    );
    assert_eq!(
        block_name(&storage, 8, -60, 8),
        "minecraft:lava",
        "the lava source above the new stone survives"
    );
}

#[test]
fn only_liquid_block_faces_quench_lava() {
    let mut storage = floor_storage("lava-faces");

    place(&mut storage, 8, -60, 8, lava(0));
    place(&mut storage, 9, -59, 8, water(0));
    place(&mut storage, 7, -59, 8, water(0));

    assert_eq!(
        block_name(&storage, 8, -60, 8),
        "minecraft:lava",
        "diagonal water is not an orthogonal contact face"
    );
}

#[test]
fn water_flowing_into_lava_source_converts_it() {
    let mut storage = floor_storage("water-into-lava-source");

    place(&mut storage, 8, -60, 8, lava(0));
    place(&mut storage, 6, -60, 8, water(0));

    run_ticks(&mut storage, 1, 5);

    assert_eq!(
        fluid_level(&storage, 7, -60, 8),
        Some(1),
        "the water source spread one block east"
    );
    assert_eq!(
        block_name(&storage, 8, -60, 8),
        "minecraft:obsidian",
        "flowing water reaching a lava source triggers the liquid-block conversion"
    );
}

#[test]
fn water_flowing_into_flowing_lava_converts_it() {
    let mut storage = floor_storage("water-into-flowing-lava");

    place(&mut storage, 10, -60, 8, lava(0));
    run_ticks(&mut storage, 1, 30);
    assert_eq!(
        fluid_level(&storage, 9, -60, 8),
        Some(2),
        "the lava source spread west into a flowing block"
    );

    place(&mut storage, 6, -60, 8, water(0));
    run_ticks(&mut storage, 31, 40);

    assert_eq!(
        block_name(&storage, 9, -60, 8),
        "minecraft:cobblestone",
        "water reaching flowing lava produces cobblestone"
    );
    assert_eq!(
        block_name(&storage, 10, -60, 8),
        "minecraft:lava",
        "the source two blocks from the water is untouched"
    );
}

#[test]
fn lava_ignites_only_flammable_neighbours() {
    let lava_pos = BlockPos::new(8, -60, 8);

    let mut stone_world = floor_storage("lava-fire-negative");
    place(&mut stone_world, 8, -60, 8, lava(0));
    for tick_number in 1..=300 {
        tick::random_tick_at(&mut stone_world, tick_number, lava_pos).unwrap();
    }
    assert!(
        fire_positions(&stone_world, lava_pos, 3).is_empty(),
        "a ring of stone and air must never ignite"
    );

    let mut plank_world = floor_storage("lava-fire-positive");
    place(&mut plank_world, 8, -60, 8, lava(0));
    place(&mut plank_world, 9, -60, 8, named("minecraft:oak_planks"));
    place(&mut plank_world, 8, -60, 7, named("minecraft:oak_planks"));
    for tick_number in 1..=300 {
        tick::random_tick_at(&mut plank_world, tick_number, lava_pos).unwrap();
    }

    let fires = fire_positions(&plank_world, lava_pos, 3);
    assert!(
        !fires.is_empty(),
        "lava random ticks must eventually place fire beside oak planks"
    );
    for pos in &fires {
        assert_eq!(
            pos.y(),
            -59,
            "LavaFluid random tick only places fire one block above or above a floor"
        );
        assert!(
            has_flammable_neighbour(&plank_world, *pos),
            "fire at {pos:?} has no flammable neighbour"
        );
        let age = fluid::fire_age(&block(&plank_world, pos.x(), pos.y(), pos.z()));
        assert_eq!(age, Some(0), "a newly placed fire starts at age 0");
    }
}

#[test]
fn fire_ages_on_scheduled_ticks() {
    let mut storage = floor_storage("fire-ages");
    let fire_pos = BlockPos::new(8, -60, 8);
    place(&mut storage, 8, -60, 8, named("minecraft:fire"));
    let current_tick = storage.tick_state().current_tick;
    storage.tick_state_mut().scheduled.schedule_after(
        current_tick,
        fire_pos,
        "minecraft:fire",
        1,
        TickPriority::Normal,
    );

    let mut aged = false;
    let table = fluid::interaction_table();
    for tick_number in 1..=40 {
        tick::tick_world_with_table(&mut storage, tick_number, &table).unwrap();
        let state = block(&storage, 8, -60, 8);
        if state.name == "minecraft:fire" && fluid::fire_age(&state).unwrap_or(0) > 0 {
            aged = true;
            break;
        }
    }
    assert!(aged, "FireBlock.tick must age fire over scheduled ticks");
}

#[test]
fn water_reaching_fire_extinguishes_it() {
    let mut storage = floor_storage("fire-water");
    place(&mut storage, 8, -60, 8, named("minecraft:fire"));
    place(&mut storage, 10, -60, 8, water(0));

    run_ticks(&mut storage, 1, 5);
    assert_eq!(
        fluid_level(&storage, 9, -60, 8),
        Some(1),
        "the water source reaches the block next to the fire"
    );
    assert_eq!(
        block_name(&storage, 8, -60, 8),
        "minecraft:fire",
        "water one block away has not touched the fire yet"
    );

    run_ticks(&mut storage, 6, 20);
    assert_eq!(
        block_name(&storage, 8, -60, 8),
        "minecraft:water",
        "water flowing into the fire block replaces it"
    );
    assert_ne!(
        fluid_level(&storage, 8, -60, 8),
        Some(0),
        "the extinguish block is flowing water, not a new source"
    );
}

#[test]
fn soul_sand_creates_an_upward_bubble_column() {
    let mut storage = floor_storage("bubble-soul");
    place(&mut storage, 8, -61, 8, named("minecraft:soul_sand"));
    place(&mut storage, 8, -60, 8, water(0));
    place(&mut storage, 8, -59, 8, water(0));

    run_ticks(&mut storage, 1, 10);

    assert_eq!(block_name(&storage, 8, -60, 8), "minecraft:bubble_column");
    assert_eq!(block_name(&storage, 8, -59, 8), "minecraft:bubble_column");
    assert_eq!(drag(&storage, 8, -60, 8), Some(false));
    assert_eq!(drag(&storage, 8, -59, 8), Some(false));
    assert_eq!(
        fluid::bubble_column_velocity(false, BubbleColumnPlacement::AboveSurface, 0.0),
        0.1,
        "soul sand columns launch entities upward at the surface"
    );
    assert_eq!(
        fluid::fluid_state(&block(&storage, 8, -60, 8)).map(|fluid| fluid.kind),
        Some(FluidKind::Water),
        "bubble columns still expose a water source fluid state"
    );
}

#[test]
fn magma_creates_a_downward_bubble_column() {
    let mut storage = floor_storage("bubble-magma");
    place(&mut storage, 8, -61, 8, named("minecraft:magma_block"));
    place(&mut storage, 8, -60, 8, water(0));
    place(&mut storage, 8, -59, 8, water(0));

    run_ticks(&mut storage, 1, 10);

    assert_eq!(block_name(&storage, 8, -60, 8), "minecraft:bubble_column");
    assert_eq!(block_name(&storage, 8, -59, 8), "minecraft:bubble_column");
    assert_eq!(drag(&storage, 8, -60, 8), Some(true));
    assert_eq!(drag(&storage, 8, -59, 8), Some(true));
    assert_eq!(
        fluid::bubble_column_velocity(true, BubbleColumnPlacement::AboveSurface, 0.0),
        -0.03,
        "magma columns pull entities down at the surface"
    );
    assert_eq!(
        fluid::bubble_column_velocity(true, BubbleColumnPlacement::Inside, 0.0),
        -0.03
    );
}

#[test]
fn flowing_water_does_not_create_a_bubble_column() {
    let mut storage = floor_storage("bubble-flowing");
    place(&mut storage, 8, -61, 8, named("minecraft:soul_sand"));
    place(&mut storage, 7, -60, 8, water(0));

    run_ticks(&mut storage, 1, 20);

    assert_eq!(
        block_name(&storage, 8, -60, 8),
        "minecraft:water",
        "only a water source over soul sand starts a column"
    );
    assert_eq!(fluid_level(&storage, 8, -60, 8), Some(1));
    assert_eq!(drag(&storage, 8, -60, 8), None);
}

#[test]
fn flowing_water_destroys_replaceable_plants() {
    let mut storage = floor_storage("fluid-destroys-plant");
    place(&mut storage, 9, -61, 8, named("minecraft:dirt"));
    place(&mut storage, 9, -60, 8, named("minecraft:short_grass"));
    place(&mut storage, 8, -60, 8, water(0));
    place(&mut storage, 7, -60, 8, named("minecraft:stone"));

    run_ticks(&mut storage, 1, 10);

    assert_eq!(
        block_name(&storage, 9, -60, 8),
        "minecraft:water",
        "flowing water replaces the short grass block"
    );
    assert_eq!(fluid_level(&storage, 9, -60, 8), Some(1));
    assert_eq!(
        block_name(&storage, 7, -60, 8),
        "minecraft:stone",
        "a full solid block is not destroyed by fluid"
    );
}

#[test]
fn interaction_sequence_is_deterministic_across_two_worlds() {
    let mut first = floor_storage("interactions-a");
    let mut second = floor_storage("interactions-b");

    for storage in [&mut first, &mut second] {
        storage.set_tick_seed(424_242);
        // Lava source + water -> obsidian.
        place(storage, 8, -60, 8, lava(0));
        place(storage, 7, -60, 8, water(0));
        // Flowing water destroys a plant.
        place(storage, 5, -61, 8, named("minecraft:dirt"));
        place(storage, 5, -60, 8, named("minecraft:short_grass"));
        // Soul-sand bubble column.
        place(storage, 3, -61, 8, named("minecraft:soul_sand"));
        place(storage, 3, -60, 8, water(0));
        place(storage, 3, -59, 8, water(0));
        // A fire with an explicit scheduled tick, so aging is deterministic.
        let fire_pos = BlockPos::new(12, -60, 12);
        place(storage, 12, -60, 12, named("minecraft:fire"));
        let current_tick = storage.tick_state().current_tick;
        storage.tick_state_mut().scheduled.schedule_after(
            current_tick,
            fire_pos,
            "minecraft:fire",
            1,
            TickPriority::Normal,
        );
    }

    let table = fluid::interaction_table();
    for tick_number in 1..=40 {
        let first_report = tick::tick_world_with_table(&mut first, tick_number, &table).unwrap();
        let second_report = tick::tick_world_with_table(&mut second, tick_number, &table).unwrap();
        assert_eq!(first_report, second_report, "tick {tick_number}");
        assert_eq!(
            first.tick_state().last_neighbor_updates,
            second.tick_state().last_neighbor_updates,
            "neighbour queue order at tick {tick_number}"
        );
    }

    assert_eq!(
        first.load_chunk(0, 0).unwrap().unwrap(),
        second.load_chunk(0, 0).unwrap().unwrap(),
        "identical interaction input produces identical chunks"
    );
}
