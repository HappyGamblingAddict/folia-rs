//! Focused tests for structured chunk entities in the Anvil `Entities` list.

use folia_core::anvil::{Compression, RegionFile};
use folia_core::block_entity::{BlockEntity, BlockEntityMap, BlockEntityPos, UnknownBlockEntity};
use folia_core::chunk::{encode_chunk, encode_chunk_with_entities};
use folia_core::chunk_entity::{ChunkEntities, ChunkEntity, ChunkEntity as Entity};
use folia_core::chunk_packet::flat_chunk;
use folia_core::nbt::Tag;
use folia_core::world::{Dimension, RegionPos, WorldStorage};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "folia-rs-chunk-entities-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn uuid(seed: u8) -> [u8; 16] {
    let mut value = [0u8; 16];
    value[0] = seed;
    value[15] = seed;
    value
}

fn entity(seed: u8, x: f64, z: f64) -> ChunkEntity {
    let mut entity = ChunkEntity::new("minecraft:item", uuid(seed), [x, 64.0, z]);
    entity.velocity = [0.1, -0.2, 0.3];
    entity.rotation = [30.0, -15.0];
    entity.set_payload_field("OnGround", Tag::Byte(1));
    entity.set_payload_field("Age", Tag::Short(42));
    entity.set_payload_field(
        "Item",
        folia_core::snbt::parse(r#"{id: "minecraft:stone", Count: 2b}"#).unwrap(),
    );
    entity
}

fn write_raw_chunk(region_dir: &std::path::Path, position: RegionPos, x: u8, z: u8, root: &Tag) {
    let mut region = RegionFile::open_or_create(region_dir.join(position.file_name())).unwrap();
    region.write_chunk(x, z, root, Compression::Zlib).unwrap();
    region.save().unwrap();
}

fn stored_root(region_dir: &std::path::Path, position: RegionPos, x: u8, z: u8) -> Tag {
    let region = RegionFile::open(region_dir.join(position.file_name())).unwrap();
    region.read_chunk(x, z).unwrap().unwrap()
}

#[test]
fn encode_decode_round_trip_keeps_both_entity_lists() {
    let chunk = flat_chunk(3, 4);
    let mut block_entities = BlockEntityMap::new();
    block_entities.insert(
        BlockEntityPos::new(48, 64, 64),
        BlockEntity::Unknown(UnknownBlockEntity {
            pos: BlockEntityPos::new(48, 64, 64),
            id: "mod:custom_block_entity".into(),
            data: folia_core::snbt::parse(
                r#"{id: "mod:custom_block_entity", x: 48, y: 64, z: 64, CustomName: "loot"}"#,
            )
            .unwrap(),
        }),
    );
    let entities = vec![entity(7, 48.5, 64.25)];

    let root = encode_chunk_with_entities(&chunk, &block_entities, &entities).unwrap();
    let (decoded, decoded_block_entities, decoded_entities) =
        folia_core::chunk::decode_chunk_with_entities(&root).unwrap();

    assert_eq!((decoded.x, decoded.z), (3, 4));
    assert_eq!(decoded_block_entities, block_entities);
    assert_eq!(decoded_entities, entities);
    assert_eq!(
        root.get("Entities")
            .and_then(Tag::as_list)
            .map(<[Tag]>::len),
        Some(1)
    );
}

#[test]
fn server_owned_entities_survive_a_save_reload() {
    let root = temp_dir("save-reload");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let chunk = flat_chunk(0, 0);

    {
        let mut storage = WorldStorage::new(&region_dir);
        storage.save_chunk(&chunk, Compression::Zlib).unwrap();
        let placed = entity(11, 8.5, 4.25);
        storage.insert_entity(placed.clone()).unwrap();
        storage
            .save_chunk_with_entities(&chunk, Compression::Zlib)
            .unwrap();
        assert!(storage.is_chunk_dirty(0, 0));
        storage.flush().unwrap();
    }

    let mut reopened = WorldStorage::new(&region_dir);
    let decoded = reopened.load_chunk(0, 0).unwrap().expect("chunk survives");
    assert_eq!((decoded.x, decoded.z), (0, 0));
    let entities = reopened
        .entities(0, 0)
        .unwrap()
        .expect("entity list survives");
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].id, "minecraft:item");
    assert_eq!(entities[0].uuid, uuid(11));
    assert_eq!(entities[0].position, [8.5, 64.0, 4.25]);
    assert_eq!(entities[0].velocity, [0.1, -0.2, 0.3]);
    assert_eq!(entities[0].rotation, [30.0, -15.0]);
    assert_eq!(entities[0].payload_field("Age"), Some(&Tag::Short(42)));
    assert_eq!(
        entities[0].payload_field("Item"),
        Some(&folia_core::snbt::parse(r#"{id: "minecraft:stone", Count: 2b}"#).unwrap())
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn entity_edits_keep_block_entities_and_unknown_top_level_fields() {
    let root = temp_dir("preserve");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let position = RegionPos::from_chunk(0, 0);
    let chunk = flat_chunk(0, 0);

    {
        let mut stored = encode_chunk(&chunk).unwrap();
        if let Tag::Compound(fields) = &mut stored {
            fields.push(("Status".into(), Tag::String("full".into())));
            fields.push((
                "BlockEntities".into(),
                Tag::List(vec![
                    folia_core::snbt::parse(r#"{id: "minecraft:furnace", x: 1, y: 64, z: 1}"#)
                        .unwrap(),
                ]),
            ));
            fields.push((
                "Entities".into(),
                Tag::List(vec![entity(3, 4.5, 8.5).to_nbt().unwrap()]),
            ));
            fields.push((
                "structures".into(),
                folia_core::snbt::parse("{Starts: {}, References: {}}").unwrap(),
            ));
            fields.push(("mod:extra".into(), Tag::String("keep me".into())));
        }
        write_raw_chunk(&region_dir, position, 0, 0, &stored);
    }

    {
        let mut storage = WorldStorage::new(&region_dir);
        storage.load_chunk(0, 0).unwrap().expect("stored chunk");
        storage.insert_entity(entity(9, 2.5, 3.5)).unwrap();
        storage
            .update_chunk(&chunk, Compression::Zlib, &[(0, 64, 0)])
            .unwrap();
        storage.flush().unwrap();
    }

    let mut reopened = WorldStorage::new(&region_dir);
    let raw = stored_root(&region_dir, position, 0, 0);
    assert_eq!(
        raw.get("mod:extra"),
        Some(&Tag::String("keep me".into())),
        "unknown top-level fields survive an entity edit"
    );
    assert!(raw.get("structures").is_some());
    let block_entities = raw
        .get("block_entities")
        .and_then(Tag::as_list)
        .expect("block entities survive");
    assert_eq!(block_entities.len(), 1, "untouched block entity survives");
    let entities = raw
        .get("Entities")
        .and_then(Tag::as_list)
        .expect("Entities is written");
    assert_eq!(
        entities.len(),
        2,
        "loaded and inserted entities both survive"
    );
    assert_eq!(
        raw.get("LegacyBlockEntities")
            .or_else(|| raw.get("BlockEntities")),
        None,
        "the legacy block-entity key is removed on rewrite"
    );

    let list = reopened
        .entities(0, 0)
        .unwrap()
        .expect("decoded entity list");
    let ids: Vec<[u8; 16]> = list.iter().map(|entry| entry.uuid).collect();
    assert!(ids.contains(&uuid(3)));
    assert!(ids.contains(&uuid(9)));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn edits_across_multiple_dirty_chunks_are_flushed_together() {
    let root = temp_dir("multi-dirty");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let coords = [(0, 0), (1, 0), (0, 1), (31, 31)];

    {
        let mut storage = WorldStorage::new(&region_dir);
        for &(chunk_x, chunk_z) in &coords {
            storage
                .save_chunk(&flat_chunk(chunk_x, chunk_z), Compression::Zlib)
                .unwrap();
        }
        storage.flush().unwrap();
        let baseline = storage.chunk_write_count();

        for (index, &(chunk_x, chunk_z)) in coords.iter().enumerate() {
            storage
                .insert_entity(entity(
                    index as u8 + 1,
                    (chunk_x * 16 + 1) as f64,
                    (chunk_z * 16 + 2) as f64,
                ))
                .unwrap();
            storage
                .save_chunk_with_entities(&flat_chunk(chunk_x, chunk_z), Compression::Zlib)
                .unwrap();
        }
        assert_eq!(storage.dirty_chunk_count(), coords.len());
        storage.flush().unwrap();
        assert_eq!(storage.dirty_chunk_count(), 0);
        assert_eq!(storage.chunk_write_count(), baseline + coords.len() as u64);
    }

    let mut reopened = WorldStorage::new(&region_dir);
    for (index, &(chunk_x, chunk_z)) in coords.iter().enumerate() {
        reopened
            .load_chunk(chunk_x, chunk_z)
            .unwrap()
            .expect("chunk survives");
        let list = reopened
            .entities(chunk_x, chunk_z)
            .unwrap()
            .unwrap_or_else(|| panic!("chunk ({chunk_x},{chunk_z}) has entities"));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].uuid, uuid(index as u8 + 1));
        assert_eq!(list[0].chunk(), (chunk_x, chunk_z));
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn generated_chunk_replaces_stored_entities() {
    let root = temp_dir("replace");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let chunk = flat_chunk(0, 0);

    {
        let mut storage = WorldStorage::new(&region_dir);
        storage.save_chunk(&chunk, Compression::Zlib).unwrap();
        storage.insert_entity(entity(5, 3.5, 7.5)).unwrap();
        storage
            .save_chunk_with_entities(&chunk, Compression::Zlib)
            .unwrap();
        storage.flush().unwrap();

        storage.save_chunk(&chunk, Compression::Zlib).unwrap();
        storage.flush().unwrap();
    }

    let mut reopened = WorldStorage::new(&region_dir);
    reopened.load_chunk(0, 0).unwrap().unwrap();
    assert!(
        reopened.entities(0, 0).unwrap().unwrap().is_empty(),
        "regenerated terrain does not inherit stale entities"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_entity_list_fails_loudly() {
    let root = temp_dir("malformed");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let position = RegionPos::from_chunk(0, 0);

    {
        let mut stored = encode_chunk(&flat_chunk(0, 0)).unwrap();
        if let Tag::Compound(fields) = &mut stored {
            fields.push(("Entities".into(), Tag::Int(3)));
        }
        write_raw_chunk(&region_dir, position, 0, 0, &stored);
    }

    let mut storage = WorldStorage::new(&region_dir);
    assert!(
        storage.load_chunk(0, 0).is_err(),
        "a malformed Entities list is an error, not a silently empty list"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_lowercase_entity_list_is_read_but_rewritten_as_modern() {
    let root = temp_dir("legacy");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let position = RegionPos::from_chunk(0, 0);

    {
        let mut stored = encode_chunk(&flat_chunk(0, 0)).unwrap();
        if let Tag::Compound(fields) = &mut stored {
            fields.push((
                "entities".into(),
                Tag::List(vec![entity(13, 1.5, 2.5).to_nbt().unwrap()]),
            ));
        }
        write_raw_chunk(&region_dir, position, 0, 0, &stored);
    }

    {
        let mut storage = WorldStorage::new(&region_dir);
        storage.load_chunk(0, 0).unwrap().unwrap();
        assert_eq!(storage.entities(0, 0).unwrap().unwrap().len(), 1);
        storage
            .update_chunk(&flat_chunk(0, 0), Compression::Zlib, &[])
            .unwrap();
        storage.flush().unwrap();
    }

    let raw = stored_root(&region_dir, position, 0, 0);
    assert!(raw.get("entities").is_none(), "legacy key is removed");
    assert_eq!(
        raw.get("Entities").and_then(Tag::as_list).map(<[Tag]>::len),
        Some(1)
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn remove_and_replace_entity_by_uuid() {
    let root = temp_dir("remove");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let mut storage = WorldStorage::new(&region_dir);
    storage
        .save_chunk(&flat_chunk(0, 0), Compression::Zlib)
        .unwrap();
    storage.insert_entity(entity(1, 1.5, 1.5)).unwrap();
    storage.insert_entity(entity(2, 2.5, 2.5)).unwrap();

    let removed = storage.remove_entity(0, 0, uuid(1)).unwrap().unwrap();
    assert_eq!(removed.uuid, uuid(1));
    let list: &ChunkEntities = storage.entities(0, 0).unwrap().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].uuid, uuid(2));

    let replacement = Entity::new("minecraft:armor_stand", uuid(2), [2.5, 65.0, 2.5]);
    storage.insert_entity(replacement.clone()).unwrap();
    let list = storage.entities(0, 0).unwrap().unwrap();
    assert_eq!(list.len(), 1, "matching UUID replaces instead of appending");
    assert_eq!(list[0].position, [2.5, 65.0, 2.5]);

    storage.clear_entities(0, 0);
    assert_eq!(storage.entities(0, 0).unwrap().unwrap().len(), 0);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn clearing_entities_is_not_undone_by_a_later_chunk_update() {
    let root = temp_dir("clear");
    let region_dir = Dimension::Overworld.region_dir(&root);
    let chunk = flat_chunk(0, 0);

    {
        let mut storage = WorldStorage::new(&region_dir);
        storage.save_chunk(&chunk, Compression::Zlib).unwrap();
        storage.insert_entity(entity(4, 1.5, 2.5)).unwrap();
        storage
            .save_chunk_with_entities(&chunk, Compression::Zlib)
            .unwrap();
        storage.flush().unwrap();
    }

    {
        let mut storage = WorldStorage::new(&region_dir);
        storage.load_chunk(0, 0).unwrap().unwrap();
        storage.clear_entities(0, 0);
        storage
            .update_chunk(&chunk, Compression::Zlib, &[])
            .unwrap();
        storage.flush().unwrap();
    }

    let mut reopened = WorldStorage::new(&region_dir);
    reopened.load_chunk(0, 0).unwrap().unwrap();
    assert!(
        reopened.entities(0, 0).unwrap().unwrap().is_empty(),
        "a cleared chunk does not resurrect its saved entities on update"
    );

    fs::remove_dir_all(root).unwrap();
}
