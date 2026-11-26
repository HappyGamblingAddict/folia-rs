use folia_core::chunk::{
    BlockState, Chunk, ChunkSection, PalettedContainer, decode_chunk, encode_chunk,
};
use folia_core::light::{DataLayer, LightScene, SectionLight};

const MIN_SECTION: i8 = -4;
const MAX_SECTION: i8 = 19;

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: Vec::new(),
    }
}

fn air() -> BlockState {
    state("minecraft:air")
}

fn stone() -> BlockState {
    state("minecraft:stone")
}

fn torch() -> BlockState {
    state("minecraft:torch")
}

fn air_chunk(x: i32, z: i32) -> Chunk {
    let air = air();
    let mut sections = Vec::with_capacity((MAX_SECTION - MIN_SECTION + 1) as usize);
    for y in MIN_SECTION..=MAX_SECTION {
        sections.push(ChunkSection {
            y,
            block_states: PalettedContainer {
                palette: vec![air.clone()],
                indices: vec![0; 4096],
            },
            biomes: Some(PalettedContainer {
                palette: vec!["minecraft:plains".into()],
                indices: vec![0; 64],
            }),
            light: SectionLight::default(),
        });
    }
    Chunk {
        status: folia_core::chunk::ChunkStatus::Full,
        light_present: false,
        x,
        z,
        sections,
    }
}

#[test]
fn no_light_writes_keep_every_section_lightless() {
    let mut chunk = air_chunk(0, 0);
    assert!(!chunk.has_light());

    for i in 0..2_000usize {
        let y = -64 + (i % 384) as i32;
        let next = if i % 2 == 0 { stone() } else { air() };
        chunk.set_block_state(i % 16, y, (i / 16) % 16, next);
    }

    assert!(!chunk.has_light());
    for section in &chunk.sections {
        assert!(
            section.light.is_empty(),
            "section {} gained light during worldgen-style writes",
            section.y
        );
    }
    assert_eq!(chunk.block_state(0, -64, 0), Some(&stone()));
}

#[test]
fn writes_into_a_new_section_create_default_light() {
    let mut chunk = Chunk {
        status: folia_core::chunk::ChunkStatus::Full,
        light_present: false,
        x: 0,
        z: 0,
        sections: Vec::new(),
    };
    assert!(!chunk.has_light());

    assert!(chunk.set_block_state(3, 70, 5, stone()));
    let section = chunk.section(4).expect("section 4 created on demand");
    assert!(section.light.is_empty(), "new section has no light layers");
    assert_eq!(chunk.block_state(3, 70, 5), Some(&stone()));
    assert!(!chunk.has_light());
}

#[test]
fn installed_light_is_invalidated_by_a_write() {
    let mut chunk = air_chunk(0, 0);
    assert!(chunk.set_section_light(
        4,
        SectionLight {
            sky: Some(DataLayer::filled(15)),
            block: Some(DataLayer::filled(7)),
        }
    ));
    assert!(chunk.has_light());

    assert!(chunk.set_block_state(8, 70, 8, stone()));
    assert!(
        !chunk.has_light(),
        "installed light must not escape invalidation"
    );
    for section in &chunk.sections {
        assert!(section.light.is_empty());
    }
}

#[test]
fn refresh_repairs_the_cache_after_a_raw_section_mutation() {
    let mut chunk = air_chunk(0, 0);
    let section = chunk
        .sections
        .iter_mut()
        .find(|section| section.y == 4)
        .expect("section 4");
    section.light = SectionLight {
        sky: Some(DataLayer::filled(15)),
        block: None,
    };
    assert!(!chunk.has_light(), "raw mutation bypasses the cached flag");

    chunk.refresh_light_present();
    assert!(chunk.has_light(), "refresh observes the installed layer");
    assert!(chunk.set_block_state(8, 70, 8, stone()));
    assert!(!chunk.has_light());
    assert!(chunk.section(4).unwrap().light.is_empty());
}

#[test]
fn first_light_population_is_correct() {
    let mut chunk = air_chunk(0, 0);
    assert!(chunk.set_block_state(8, 70, 8, torch()));
    assert!(
        !chunk.has_light(),
        "edits before the first relight carry no light"
    );

    chunk.recompute_light();
    assert!(chunk.has_light());
    assert_eq!(chunk.block_light(8, 70, 8), Some(14), "torch source");
    assert_eq!(chunk.block_light(7, 70, 8), Some(13), "one block away");
    assert_eq!(chunk.sky_light(8, 71, 8), Some(15), "open sky");
}

#[test]
fn write_after_light_population_invalidates_all_sections() {
    let mut chunk = air_chunk(0, 0);
    assert!(chunk.set_block_state(8, 70, 8, torch()));
    chunk.recompute_light();
    assert!(chunk.has_light());
    let lit_sections = chunk
        .sections
        .iter()
        .filter(|section| !section.light.is_empty())
        .count();
    assert!(lit_sections > 0);

    assert!(chunk.set_block_state(4, 60, 4, stone()));
    assert!(!chunk.has_light());
    assert_eq!(chunk.block_light(8, 70, 8), None);
    assert_eq!(chunk.sky_light(8, 71, 8), None);
    for section in &chunk.sections {
        assert!(section.light.is_empty());
    }
}

#[test]
fn light_survives_nbt_round_trip_then_invalidates_after_an_edit() {
    let mut chunk = air_chunk(2, -3);
    assert!(chunk.set_block_state(8, 70, 8, torch()));
    chunk.recompute_light();

    let expected: Vec<(i8, SectionLight)> = chunk
        .sections
        .iter()
        .filter(|section| !section.light.is_empty())
        .map(|section| (section.y, section.light.clone()))
        .collect();
    assert!(!expected.is_empty());

    let tag = encode_chunk(&chunk).expect("encode lit chunk");
    let decoded = decode_chunk(&tag).expect("decode lit chunk");
    assert!(decoded.has_light());
    for (y, light) in &expected {
        assert_eq!(
            decoded
                .section(*y)
                .expect("section survives round trip")
                .light,
            *light,
            "section {y} light survives the round trip"
        );
    }
    assert_eq!(decoded.block_light(8, 70, 8), chunk.block_light(8, 70, 8));
    assert_eq!(decoded.sky_light(8, 71, 8), chunk.sky_light(8, 71, 8));

    let mut edited = decoded;
    assert!(edited.set_block_state(1, 70, 1, stone()));
    assert!(!edited.has_light());
}

#[test]
fn light_scene_edit_keeps_restored_light_coherent() {
    let mut center = air_chunk(0, 0);
    let mut neighbours = [air_chunk(1, 0)];
    assert!(center.set_block_state(8, 70, 8, torch()));
    center.recompute_light();
    assert!(center.has_light());

    let before_block = center.block_light(8, 70, 8);
    {
        let mut scene = LightScene::new(&mut center, &mut neighbours);
        scene.clear_dirty();
        assert!(scene.set_block_state(4, 70, 4, stone()));
        assert!(
            !scene.dirty().is_empty(),
            "the edit marks light sections dirty"
        );
        assert_eq!(
            scene.block_level(8, 70, 8),
            before_block.expect("torch light was computed"),
            "the scene restores pre-edit light before updating the delta"
        );
    }
    assert!(
        center.has_light(),
        "the chunk keeps light after the scene edit"
    );
    assert_eq!(center.block_state(4, 70, 4), Some(&stone()));
}

#[test]
fn writes_land_identically_with_and_without_light() {
    let mut cold = air_chunk(0, 0);
    let mut warm = air_chunk(0, 0);
    warm.recompute_light();
    assert!(warm.has_light());

    for i in 0..256usize {
        let x = i % 16;
        let z = (i / 16) % 16;
        let y = -60 + (i % 8) as i32;
        let next = if i % 3 == 0 { stone() } else { torch() };
        assert!(cold.set_block_state(x, y, z, next.clone()));
        assert!(warm.set_block_state(x, y, z, next));
    }

    for section_y in MIN_SECTION..=MAX_SECTION {
        for local_y in 0..16 {
            for z in 0..16 {
                for x in 0..16 {
                    let y = i32::from(section_y) * 16 + local_y;
                    assert_eq!(
                        cold.block_state(x, y, z),
                        warm.block_state(x, y, z),
                        "block at ({x}, {y}, {z}) is identical with and without light"
                    );
                }
            }
        }
    }
}
