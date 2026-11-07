//! End content tests: dragon-arena spikes, platform, gateways, and chorus.
//!
//! Spike values are pinned for seed 12345. The fixture chunks are fully air so
//! every asserted block was written by the feature under test.

use folia_core::chunk::{BlockState, Chunk, ChunkSection, PalettedContainer};
use folia_core::light::SectionLight;
use folia_core::worldgen::end::{
    DIMENSION_HEIGHT, DRAGON_ARENA, EndGateway, EndGenerator, MIN_Y, PLATFORM_ORIGIN,
    RETURN_GATEWAY_EXIT, SPIKE_COUNT, place_chorus_plant, place_configured_feature,
    place_end_gateway, place_end_platform, place_spike,
};
use folia_core::worldgen::generator::{Dimension, WorldGenerator, generate_dimension};
use folia_random::LegacyRandomSource;

const SEED: i64 = 12345;

fn block(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: vec![],
    }
}

fn air_chunk(chunk_x: i32, chunk_z: i32) -> Chunk {
    let mut sections = Vec::new();
    for index in 0..(DIMENSION_HEIGHT / 16) {
        sections.push(ChunkSection {
            y: index as i8,
            block_states: PalettedContainer {
                palette: vec![block("minecraft:air")],
                indices: vec![0; 4096],
            },
            biomes: Some(PalettedContainer {
                palette: vec!["minecraft:the_end".into()],
                indices: vec![0; 64],
            }),
            light: SectionLight::default(),
        });
    }
    Chunk {
        x: chunk_x,
        z: chunk_z,
        sections,
    }
}

fn flat_chorus_chunk(chunk_x: i32, chunk_z: i32) -> Chunk {
    let mut chunk = air_chunk(chunk_x, chunk_z);
    for x in 0..16 {
        for z in 0..16 {
            chunk.set_block_state(x, 63, z, block("minecraft:end_stone"));
        }
    }
    chunk
}

fn name_at(chunk: &Chunk, x: i32, y: i32, z: i32) -> &str {
    let local_x = (i64::from(x) - i64::from(chunk.x) * 16) as usize;
    let local_z = (i64::from(z) - i64::from(chunk.z) * 16) as usize;
    &chunk.block_state(local_x, y, local_z).unwrap().name
}

fn in_chunk(chunk: &Chunk, x: i32, z: i32) -> bool {
    (chunk.x * 16..chunk.x * 16 + 16).contains(&x) && (chunk.z * 16..chunk.z * 16 + 16).contains(&z)
}

#[test]
fn end_spike_list_is_pinned_for_seed_12345() {
    let generator = EndGenerator::new(SEED).unwrap();
    let spikes = generator.spikes();
    assert_eq!(spikes.len(), SPIKE_COUNT);
    assert_eq!(
        spikes[0],
        folia_core::worldgen::end::EndSpike {
            center_x: 42,
            center_z: 0,
            radius: 4,
            height: 97,
            guarded: false,
        }
    );
    assert_eq!(
        spikes[6],
        folia_core::worldgen::end::EndSpike {
            center_x: -34,
            center_z: -25,
            radius: 2,
            height: 79,
            guarded: true,
        }
    );
    assert_eq!(spikes.iter().filter(|spike| spike.guarded).count(), 2);
    assert_eq!(
        spikes,
        EndGenerator::new(SEED).unwrap().spikes(),
        "spike generation must be deterministic"
    );
}

#[test]
fn dragon_arena_bounds_contain_the_spike_ring() {
    assert!(DRAGON_ARENA.contains(0, 0));
    assert!(DRAGON_ARENA.contains(99, 0));
    assert!(!DRAGON_ARENA.contains(101, 0));
    assert!(DRAGON_ARENA.contains_y(0));
    assert!(DRAGON_ARENA.contains_y(127));
    assert!(!DRAGON_ARENA.contains_y(128));
    assert_eq!(DRAGON_ARENA.spike_radius, 42);
    assert_eq!(DRAGON_ARENA.exit_podium_radius, 4);

    let generator = EndGenerator::new(SEED).unwrap();
    for spike in generator.spikes() {
        assert!(
            DRAGON_ARENA.contains(spike.center_x, spike.center_z),
            "spike {spike:?} left the dragon arena"
        );
        assert!((MIN_Y..DIMENSION_HEIGHT).contains(&spike.height));
    }
}

#[test]
fn end_spike_generates_an_obsidian_pillar_and_crystal_support() {
    let generator = EndGenerator::new(SEED).unwrap();
    let spike = generator.spikes()[0];
    let chunk_x = spike.chunk_x();
    let chunk_z = spike.chunk_z();
    let chunk = generator.generate_chunk(chunk_x, chunk_z).unwrap();

    for y in MIN_Y..spike.height {
        assert_eq!(
            name_at(&chunk, spike.center_x, y, spike.center_z),
            "minecraft:obsidian",
            "missing spike obsidian at y={y}"
        );
    }
    let (crystal_x, crystal_y, crystal_z) = spike.crystal_position();
    assert_eq!(
        name_at(&chunk, crystal_x, crystal_y - 1, crystal_z),
        "minecraft:bedrock",
        "missing crystal bedrock support"
    );
    assert_eq!(
        name_at(&chunk, crystal_x, crystal_y, crystal_z),
        "minecraft:fire",
        "crystal cell must contain fire"
    );
    assert_eq!(
        name_at(&chunk, spike.center_x, spike.height + 10, spike.center_z),
        "minecraft:air",
        "spike must clear the space above y=65"
    );
}

#[test]
fn guarded_spike_generates_iron_bars() {
    let generator = EndGenerator::new(SEED).unwrap();
    let spike = generator
        .spikes()
        .iter()
        .copied()
        .find(|spike| spike.guarded)
        .expect("seed 12345 has guarded spikes");
    let chunk = generator
        .generate_chunk(spike.chunk_x(), spike.chunk_z())
        .unwrap();
    let mut bars = 0;
    for x in (spike.center_x - 2)..=(spike.center_x + 2) {
        if !in_chunk(&chunk, x, spike.center_z) {
            continue;
        }
        for z in (spike.center_z - 2)..=(spike.center_z + 2) {
            for y in spike.height..=(spike.height + 3) {
                if name_at(&chunk, x, y, z) == "minecraft:iron_bars" {
                    bars += 1;
                }
            }
        }
    }
    assert!(bars > 0, "guarded spike placed no iron bars");
}

#[test]
fn end_platform_constructs_the_full_5x5_obsidian_base() {
    let (origin_x, origin_y, origin_z) = PLATFORM_ORIGIN;
    let chunk_x = origin_x.div_euclid(16);
    let chunk_z = origin_z.div_euclid(16);
    let mut chunk = air_chunk(chunk_x, chunk_z);
    assert!(place_end_platform(
        &mut chunk,
        folia_math::BlockPos::new(origin_x, origin_y, origin_z)
    ));
    for x in (origin_x - 2)..=(origin_x + 2) {
        for z in origin_z..=(origin_z + 2) {
            assert_eq!(
                name_at(&chunk, x, origin_y - 1, z),
                "minecraft:obsidian",
                "missing platform obsidian at ({x},{},{z})",
                origin_y - 1
            );
            for y in origin_y..origin_y + 3 {
                assert_eq!(
                    name_at(&chunk, x, y, z),
                    "minecraft:air",
                    "platform headroom blocked at ({x},{y},{z})"
                );
            }
        }
    }

    // The platform spans z=-2..2 and also writes the southern half into the
    // neighbouring chunk.
    let mut north = air_chunk(chunk_x, chunk_z - 1);
    assert!(place_end_platform(
        &mut north,
        folia_math::BlockPos::new(origin_x, origin_y, origin_z)
    ));
    for x in (origin_x - 2)..=(origin_x + 2) {
        for z in (origin_z - 2)..origin_z {
            assert_eq!(
                name_at(&north, x, origin_y - 1, z),
                "minecraft:obsidian",
                "platform did not cross the chunk border at ({x},{},{z})",
                origin_y - 1
            );
        }
    }
}

#[test]
fn end_gateway_block_construction_returns_its_exit_data() {
    let mut chunk = air_chunk(0, 0);
    let gateway = EndGateway::returning((8, 64, 8));
    assert_eq!(gateway.exit, Some(RETURN_GATEWAY_EXIT));
    assert!(gateway.exact);

    let block_entity = place_end_gateway(&mut chunk, &gateway).expect("gateway centre is in chunk");
    assert_eq!(block_entity.position, (8, 64, 8));
    assert_eq!(block_entity.exit, Some(RETURN_GATEWAY_EXIT));
    assert!(block_entity.exact);
    assert_eq!(name_at(&chunk, 8, 64, 8), "minecraft:end_gateway");
    assert_eq!(name_at(&chunk, 8, 63, 8), "minecraft:bedrock");
    assert_eq!(name_at(&chunk, 8, 65, 8), "minecraft:bedrock");
    assert_eq!(name_at(&chunk, 8, 63, 7), "minecraft:bedrock");
    assert_eq!(name_at(&chunk, 7, 63, 7), "minecraft:air");
    assert_eq!(name_at(&chunk, 7, 64, 8), "minecraft:air");

    let delayed = EndGateway::delayed((8, 64, 8));
    assert_eq!(delayed.exit, None);
    assert!(!delayed.exact);
    let block_entity = place_end_gateway(&mut chunk, &delayed).unwrap();
    assert_eq!(block_entity.exit, None);
    assert!(!block_entity.exact);
}

#[test]
fn chorus_plant_grows_deterministically_from_end_stone() {
    let mut first = flat_chorus_chunk(0, 0);
    assert!(place_chorus_plant(
        &mut first,
        (8, 64, 8),
        &mut LegacyRandomSource::new(7)
    ));
    let mut second = flat_chorus_chunk(0, 0);
    assert!(place_chorus_plant(
        &mut second,
        (8, 64, 8),
        &mut LegacyRandomSource::new(7)
    ));
    assert_eq!(first, second, "chorus growth must replay deterministically");

    let mut plants = 0;
    let mut flowers = 0;
    for x in 0..16 {
        for z in 0..16 {
            for y in 64..DIMENSION_HEIGHT {
                match first.block_state(x, y, z).unwrap().name.as_str() {
                    "minecraft:chorus_plant" => {
                        plants += 1;
                        let state = first.block_state(x, y, z).unwrap();
                        assert!(
                            state
                                .properties
                                .iter()
                                .any(|(key, value)| key != "waterlogged" && value == "true"),
                            "chorus plant at ({x},{y},{z}) has no connections"
                        );
                    }
                    "minecraft:chorus_flower" => flowers += 1,
                    _ => {}
                }
            }
        }
    }
    assert!(plants > 4, "chorus plant only placed {plants} blocks");
    assert!(flowers >= 1, "chorus growth must terminate in a flower");

    let mut rejected = flat_chorus_chunk(0, 0);
    assert!(
        !place_chorus_plant(&mut rejected, (8, 65, 8), &mut LegacyRandomSource::new(7)),
        "chorus requires end stone support"
    );
}

#[test]
fn end_feature_dispatch_covers_chorus_and_fails_loudly_otherwise() {
    let mut chunk = flat_chorus_chunk(0, 0);
    let placed = place_configured_feature(
        &mut chunk,
        "minecraft:chorus_plant",
        (8, 64, 8),
        &mut LegacyRandomSource::new(7),
    )
    .expect("chorus dispatch is implemented");
    assert!(placed);

    let mut gateway = air_chunk(0, 0);
    let placed = place_configured_feature(
        &mut gateway,
        "minecraft:end_gateway_return",
        (8, 64, 8),
        &mut LegacyRandomSource::new(7),
    )
    .expect("gateway dispatch is implemented");
    assert!(placed);
    assert_eq!(name_at(&gateway, 8, 64, 8), "minecraft:end_gateway");

    let error = place_configured_feature(
        &mut gateway,
        "minecraft:end_island",
        (8, 64, 8),
        &mut LegacyRandomSource::new(7),
    )
    .expect_err("end_island has no Rust implementation");
    assert!(error.to_string().contains("end_island"));
}

#[test]
fn end_content_keeps_the_overworld_dimension_path_unchanged() {
    let direct = WorldGenerator::overworld(SEED)
        .unwrap()
        .generate_chunk(0, -1)
        .unwrap();
    let through_entry = generate_dimension(Dimension::Overworld, 0, -1, SEED).unwrap();
    assert_eq!(
        direct, through_entry,
        "End content must not change the Overworld parity path"
    );

    let direct_end = EndGenerator::new(SEED)
        .unwrap()
        .generate_chunk(0, 0)
        .unwrap();
    let through_end = generate_dimension(Dimension::End, 0, 0, SEED).unwrap();
    assert_eq!(direct_end, through_end);
}

#[test]
fn place_spike_writes_only_inside_the_target_chunk() {
    let spike = folia_core::worldgen::end::EndSpike {
        center_x: 0,
        center_z: 0,
        radius: 2,
        height: 80,
        guarded: false,
    };
    let mut chunk = air_chunk(0, 0);
    place_spike(&mut chunk, spike);
    assert_eq!(name_at(&chunk, 0, 0, 0), "minecraft:obsidian");
    assert_eq!(name_at(&chunk, 0, 79, 0), "minecraft:obsidian");
    assert_eq!(name_at(&chunk, 0, 80, 0), "minecraft:bedrock");
    assert_eq!(name_at(&chunk, 0, 81, 0), "minecraft:fire");
}
