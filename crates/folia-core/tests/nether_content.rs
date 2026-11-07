//! Nether content tests: carvers, feature-pass steps, and structure selection.
//!
//! The carver fixture starts from a completely solid netherrack column so an
//! air pocket can only come from `NetherGenerator::carve_chunk`. Structure
//! selection is pinned to the shipped `nether_complexes` set for seed 12345.

use folia_core::chunk::{BlockState, Chunk, ChunkSection, PalettedContainer};
use folia_core::light::SectionLight;
use folia_core::worldgen::generator::{Dimension, WorldGenerator, generate_dimension};
use folia_core::worldgen::nether::{HEIGHT, MIN_Y, NetherGenerator, SEA_LEVEL};
use folia_core::worldgen::structure::StructureError;

const SEED: i64 = 12345;

fn block(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: vec![],
    }
}

fn flat_nether_chunk(chunk_x: i32, chunk_z: i32) -> Chunk {
    let mut sections = Vec::new();
    for index in 0..16 {
        sections.push(ChunkSection {
            y: index,
            block_states: PalettedContainer {
                palette: vec![block("minecraft:air")],
                indices: vec![0; 4096],
            },
            biomes: Some(PalettedContainer {
                palette: vec!["minecraft:nether_wastes".into()],
                indices: vec![0; 64],
            }),
            light: SectionLight::default(),
        });
    }
    let mut chunk = Chunk {
        x: chunk_x,
        z: chunk_z,
        sections,
    };
    for x in 0..16 {
        for z in 0..16 {
            chunk.set_block_state(x as usize, MIN_Y, z as usize, block("minecraft:bedrock"));
            chunk.set_block_state(
                x as usize,
                MIN_Y + HEIGHT - 1,
                z as usize,
                block("minecraft:bedrock"),
            );
            for y in 1..HEIGHT - 1 {
                chunk.set_block_state(x as usize, y, z as usize, block("minecraft:netherrack"));
            }
        }
    }
    chunk
}

fn carved_fixture(chunk_x: i32, chunk_z: i32) -> Chunk {
    let generator = NetherGenerator::new(SEED).unwrap();
    let mut chunk = flat_nether_chunk(chunk_x, chunk_z);
    generator.carve_chunk(&mut chunk);
    chunk
}

#[test]
fn fixed_seed_nether_carver_produces_air_pockets_below_the_roof() {
    let chunk = carved_fixture(0, -3);
    let mut air_above_sea = 0;
    for x in 0..16 {
        for z in 0..16 {
            for y in SEA_LEVEL..=120 {
                if chunk.block_state(x, y, z).unwrap().name == "minecraft:air" {
                    air_above_sea += 1;
                }
            }
        }
    }
    assert!(
        air_above_sea > 100,
        "expected substantial carved cave air, found {air_above_sea}"
    );
}

#[test]
fn nether_carver_confines_lava_below_the_sea_and_protects_the_roof() {
    let chunk = carved_fixture(0, 0);
    let mut lava = 0;
    for x in 0..16 {
        for z in 0..16 {
            for y in 1..SEA_LEVEL {
                let state = chunk.block_state(x, y, z).unwrap();
                assert_ne!(
                    state.name, "minecraft:air",
                    "carver left air below the lava sea at ({x},{y},{z})"
                );
                if state.name == "minecraft:lava" {
                    lava += 1;
                }
            }
            for y in 121..HEIGHT - 1 {
                assert_eq!(
                    chunk.block_state(x, y, z).unwrap().name,
                    "minecraft:netherrack",
                    "carver reached the protected roof layers at ({x},{y},{z})"
                );
            }
            assert_eq!(
                chunk.block_state(x, 0, z).unwrap().name,
                "minecraft:bedrock",
                "carver removed floor bedrock at ({x},0,{z})"
            );
            assert_eq!(
                chunk.block_state(x, HEIGHT - 1, z).unwrap().name,
                "minecraft:bedrock",
                "carver removed roof bedrock at ({x},{},{z})",
                HEIGHT - 1
            );
        }
    }
    assert!(lava > 0, "carved cave blocks below y=32 must become lava");
}

#[test]
fn nether_carver_is_deterministic() {
    assert_eq!(
        carved_fixture(0, -3),
        carved_fixture(0, -3),
        "Nether carver replay must be deterministic"
    );
}

#[test]
fn nether_complex_structure_selection_is_pinned_for_seed_12345() {
    let generator = NetherGenerator::new(SEED).unwrap();

    let fortress = generator
        .structure_at_chunk(6, -32)
        .expect("structure selection evaluates")
        .expect("pinned fortress chunk");
    assert_eq!(fortress.set, "minecraft:nether_complexes");
    assert_eq!(fortress.structure, "minecraft:fortress");
    assert_eq!((fortress.chunk.x, fortress.chunk.z), (6, -32));

    let bastion = generator
        .structure_at_chunk(12, 20)
        .expect("structure selection evaluates")
        .expect("pinned bastion chunk");
    assert_eq!(bastion.set, "minecraft:nether_complexes");
    assert_eq!(bastion.structure, "minecraft:bastion_remnant");
    assert_eq!((bastion.chunk.x, bastion.chunk.z), (12, 20));

    assert_eq!(
        generator.structure_at_chunk(6, -32).unwrap(),
        Some(fortress),
        "selection must replay deterministically"
    );
}

#[test]
fn nether_structure_starts_port_fortress_and_reject_missing_bastion_templates() {
    let generator = NetherGenerator::new(SEED).unwrap();

    let fortress = generator.structure_at_chunk(6, -32).unwrap().unwrap();
    let start = generator
        .structure_start(&fortress)
        .expect("fortress procedural pieces are ported");
    let fortress_start = start.fortress().expect("fortress start data");
    assert!(
        fortress_start.pieces().len() > 1,
        "fortress should expand into multiple pieces"
    );

    let bastion = generator.structure_at_chunk(12, 20).unwrap().unwrap();
    let error = generator
        .structure_start(&bastion)
        .expect_err("bastion starts require the unshipped template pool");
    assert!(matches!(
        error,
        StructureError::UnknownTemplate { ref structure, .. }
            if structure == "minecraft:bastion/starts"
    ));
    assert!(
        error.to_string().contains("template"),
        "the failure must name the missing template asset: {error}"
    );
}

#[test]
fn generated_nether_chunks_keep_no_air_below_the_lava_sea() {
    let generator = NetherGenerator::new(SEED).unwrap();
    let chunk = generator.generate_chunk(0, -3).unwrap();
    for x in 0..16 {
        for z in 0..16 {
            for y in 0..SEA_LEVEL {
                assert_ne!(
                    chunk.block_state(x, y, z).unwrap().name,
                    "minecraft:air",
                    "generated carver left air below the sea at ({x},{y},{z})"
                );
            }
        }
    }
}

#[test]
fn overworld_dimension_entry_point_is_unchanged() {
    let direct = WorldGenerator::overworld(SEED)
        .unwrap()
        .generate_chunk(0, -1)
        .unwrap();
    let through_entry = generate_dimension(Dimension::Overworld, 0, -1, SEED).unwrap();
    assert_eq!(
        direct, through_entry,
        "Nether content must not change the Overworld parity path"
    );
}
