//! Block-entity persistence tests: typed state, chunk NBT compatibility, and
//! world-storage lifecycle.

use folia_core::anvil::{Compression, RegionFile};
use folia_core::block_entity::{
    BLOCK_ENTITIES_KEY, BlockEntity, BlockEntityMap, BlockEntityPos, BlockEntityTicker,
    CHEST_SLOTS, ChestBlockEntity, FURNACE_SLOTS, FurnaceBlockEntity, LEGACY_BLOCK_ENTITIES_KEY,
    SignBlockEntity, SignText, SimpleBlockEntityTicker, SpawnerBlockEntity, UnknownBlockEntity,
    decode_block_entities, store_block_entities,
};
use folia_core::chunk::{
    BlockState, Chunk, decode_chunk_with_block_entities, encode_chunk,
    encode_chunk_with_block_entities,
};
use folia_core::crafting::FurnaceKind;
use folia_core::item_components::{ComponentValue, ItemStack};
use folia_core::loot::RecipeCatalog;
use folia_core::nbt::Tag;
use folia_core::world::{Dimension, WorldStorage};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: vec![],
    }
}

fn position() -> BlockEntityPos {
    BlockEntityPos::new(3, 64, -7)
}

fn chunk_at(x: i32, z: i32) -> Chunk {
    let mut chunk = Chunk {
        x,
        z,
        sections: Vec::new(),
    };
    for local_x in 0..3 {
        assert!(chunk.set_block_state(local_x, 64, 0, state("minecraft:stone")));
    }
    chunk
}

fn entity_map(entity: BlockEntity) -> BlockEntityMap {
    let mut entities = BlockEntityMap::new();
    entities.insert(entity.pos(), entity);
    entities
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "folia-rs-block-entity-{}-{}-{label}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn chest_round_trips_through_chunk_nbt_with_item_components() {
    let mut chest = ChestBlockEntity::new(position());
    let mut sword = ItemStack::new("minecraft:diamond_sword", 1);
    sword.set_component(ComponentValue::CustomName(Tag::String("Excalibur".into())));
    sword.set_component(ComponentValue::Damage(7));
    sword.set_component(ComponentValue::MaxDamage(1561));
    sword.set_component(ComponentValue::Enchantments(BTreeMap::from([(
        "minecraft:sharpness".into(),
        5,
    )])));
    assert!(chest.set_slot(0, sword.clone()));
    assert!(chest.set_slot(26, ItemStack::new("minecraft:apple", 3)));
    assert!(!chest.set_slot(CHEST_SLOTS, ItemStack::new("minecraft:stone", 1)));
    assert_eq!(chest.items.len(), CHEST_SLOTS);
    chest.set_loot_table(
        Some("minecraft:chests/simple_dungeon".into()),
        0x1122_3344_5566_7788,
    );

    let chunk = chunk_at(0, 0);
    let root = encode_chunk_with_block_entities(&chunk, &entity_map(BlockEntity::Chest(chest)))
        .expect("encode chunk");
    let list = root
        .get(BLOCK_ENTITIES_KEY)
        .and_then(Tag::as_list)
        .expect("block_entities list");
    assert_eq!(list.len(), 1);

    let (decoded_chunk, entities) = decode_chunk_with_block_entities(&root).expect("decode chunk");
    assert_eq!(decoded_chunk.x, chunk.x);
    assert_eq!(decoded_chunk.z, chunk.z);
    let Some(BlockEntity::Chest(chest)) = entities.get(&position()) else {
        panic!("chest did not round-trip");
    };
    assert_eq!(chest.slot(0), Some(&sword));
    assert_eq!(
        chest
            .slot(26)
            .map(|stack| (stack.item.as_str(), stack.count)),
        Some(("minecraft:apple", 3))
    );
    assert_eq!(
        chest.loot_table.as_deref(),
        Some("minecraft:chests/simple_dungeon")
    );
    assert_eq!(chest.loot_table_seed, 0x1122_3344_5566_7788);
    assert_eq!(
        chest
            .slot(0)
            .and_then(|stack| stack.get_component("minecraft:enchantments")),
        Some(&ComponentValue::Enchantments(BTreeMap::from([(
            "minecraft:sharpness".into(),
            5,
        )])))
    );
}

#[test]
fn furnace_state_survives_save_and_load() {
    let mut furnace = FurnaceBlockEntity::new(position(), FurnaceKind::Furnace);
    assert!(furnace.set_slot(0, ItemStack::new("minecraft:raw_iron", 2)));
    assert!(furnace.set_slot(1, ItemStack::new("minecraft:coal", 1)));
    assert_eq!(furnace.items.len(), FURNACE_SLOTS);

    let mut entity = BlockEntity::Furnace(furnace);
    let mut ticker = SimpleBlockEntityTicker::with_recipes(RecipeCatalog::global());
    for _ in 0..201 {
        ticker.tick(&mut entity);
    }
    let BlockEntity::Furnace(furnace) = &entity else {
        panic!("ticker changed the entity type");
    };
    assert_eq!(
        furnace.slot(2).map(|stack| stack.item.as_str()),
        Some("minecraft:iron_ingot")
    );
    assert!(furnace.lit_time_remaining > 0);
    assert!(furnace.cooking_total_time > 0);
    assert!(
        furnace
            .recipes_used
            .contains_key("minecraft:iron_ingot_from_smelting_raw_iron")
    );
    let before_lit = furnace.lit_time_remaining;
    let before_cook = furnace.cooking_time_spent;
    let before_total = furnace.cooking_total_time;
    let before_recipes = furnace.recipes_used.clone();

    let chunk = chunk_at(0, 0);
    let root =
        encode_chunk_with_block_entities(&chunk, &entity_map(entity)).expect("encode furnace");
    let (_, entities) = decode_chunk_with_block_entities(&root).expect("decode furnace");
    let Some(BlockEntity::Furnace(decoded)) = entities.get(&position()) else {
        panic!("furnace did not round-trip");
    };
    assert_eq!(decoded.kind, FurnaceKind::Furnace);
    assert_eq!(decoded.slot(0).map(|stack| stack.count), Some(1));
    assert!(decoded.slot(1).is_some_and(ItemStack::is_empty));
    assert_eq!(
        decoded.slot(2).map(|stack| stack.item.as_str()),
        Some("minecraft:iron_ingot")
    );
    assert_eq!(decoded.lit_time_remaining, before_lit);
    assert_eq!(decoded.cooking_time_spent, before_cook);
    assert_eq!(decoded.cooking_total_time, before_total);
    assert_eq!(decoded.cook_speed_multiplier, 1.0);
    assert_eq!(decoded.recipes_used, before_recipes);
    assert_eq!(
        decoded.experience, 0.0,
        "vanilla derives experience from RecipesUsed instead of persisting it"
    );
}

#[test]
fn sign_text_round_trips_front_back_color_and_styles() {
    let mut sign = SignBlockEntity::new(position());
    sign.front_text = SignText::plain(["Hello", "world", "", ""]);
    sign.front_text.color = "red".into();
    sign.front_text.has_glowing_text = true;
    sign.front_text.messages[0] = Tag::Compound(vec![
        ("text".into(), Tag::String("Styled".into())),
        ("color".into(), Tag::String("gold".into())),
    ]);
    sign.back_text = SignText::plain(["Back", "side", "", ""]);
    sign.is_waxed = true;

    let chunk = chunk_at(0, 0);
    let root =
        encode_chunk_with_block_entities(&chunk, &entity_map(BlockEntity::Sign(Box::new(sign))))
            .expect("encode sign");
    let (_, entities) = decode_chunk_with_block_entities(&root).expect("decode sign");
    let Some(BlockEntity::Sign(decoded)) = entities.get(&position()) else {
        panic!("sign did not round-trip");
    };
    assert_eq!(decoded.front_text.line(0), Some("Styled"));
    assert_eq!(decoded.front_text.line(1), Some("world"));
    assert_eq!(decoded.front_text.color, "red");
    assert!(decoded.front_text.has_glowing_text);
    assert_eq!(decoded.back_text.line(0), Some("Back"));
    assert_eq!(decoded.back_text.line(1), Some("side"));
    assert!(decoded.is_waxed);
    assert_eq!(
        decoded.front_text.messages[0],
        Tag::Compound(vec![
            ("text".into(), Tag::String("Styled".into())),
            ("color".into(), Tag::String("gold".into())),
        ]),
        "styled component NBT is preserved"
    );
}

#[test]
fn spawner_fields_and_spawn_data_round_trip() {
    let mut spawner = SpawnerBlockEntity::new(position());
    spawner.set_entity_id("minecraft:pig");
    spawner.delay = 42;
    spawner.min_spawn_delay = 100;
    spawner.max_spawn_delay = 300;
    spawner.spawn_count = 3;
    spawner.max_nearby_entities = 8;
    spawner.required_player_range = 12;
    spawner.spawn_range = 5;
    if let Some(Tag::Compound(fields)) = &mut spawner.spawn_data {
        let entity = fields
            .iter_mut()
            .find(|(key, _)| key == "entity")
            .map(|(_, value)| value)
            .expect("entity compound");
        let Tag::Compound(entity_fields) = entity else {
            panic!("entity tag is a compound");
        };
        entity_fields.push(("CustomName".into(), Tag::String("Piggy".into())));
    }

    assert_eq!(spawner.entity_id.as_deref(), Some("minecraft:pig"));
    spawner.tick();
    assert_eq!(spawner.delay, 41);

    let chunk = chunk_at(0, 0);
    let root = encode_chunk_with_block_entities(&chunk, &entity_map(BlockEntity::Spawner(spawner)))
        .expect("encode spawner");
    let (_, entities) = decode_chunk_with_block_entities(&root).expect("decode spawner");
    let Some(BlockEntity::Spawner(decoded)) = entities.get(&position()) else {
        panic!("spawner did not round-trip");
    };
    assert_eq!(decoded.entity_id.as_deref(), Some("minecraft:pig"));
    assert_eq!(decoded.delay, 41);
    assert_eq!(decoded.min_spawn_delay, 100);
    assert_eq!(decoded.max_spawn_delay, 300);
    assert_eq!(decoded.spawn_count, 3);
    assert_eq!(decoded.max_nearby_entities, 8);
    assert_eq!(decoded.required_player_range, 12);
    assert_eq!(decoded.spawn_range, 5);
    assert_eq!(
        decoded
            .spawn_data
            .as_ref()
            .and_then(|data| data.get("entity"))
            .and_then(|entity| entity.get("CustomName"))
            .and_then(Tag::as_str),
        Some("Piggy")
    );
    assert!(decoded.spawn_potentials.as_list().is_some());
}

#[test]
fn unknown_block_entity_is_byte_preserved() {
    let original = Tag::Compound(vec![
        ("id".into(), Tag::String("mod:magic_block".into())),
        ("x".into(), Tag::Int(position().x)),
        ("y".into(), Tag::Int(position().y)),
        ("z".into(), Tag::Int(position().z)),
        (
            "ModData".into(),
            Tag::Compound(vec![
                ("level".into(), Tag::Int(9)),
                (
                    "tags".into(),
                    Tag::List(vec![
                        Tag::String("alpha".into()),
                        Tag::String("beta".into()),
                    ]),
                ),
            ]),
        ),
        ("KeepPacked".into(), Tag::Byte(1)),
    ]);
    let mut entities = BlockEntityMap::new();
    entities.insert(
        position(),
        BlockEntity::Unknown(UnknownBlockEntity {
            pos: position(),
            id: "mod:magic_block".into(),
            data: original.clone(),
        }),
    );

    let mut root = Tag::Compound(vec![
        ("xPos".into(), Tag::Int(0)),
        ("zPos".into(), Tag::Int(0)),
        ("sections".into(), Tag::List(Vec::new())),
    ]);
    store_block_entities(&mut root, &entities).unwrap();
    let decoded = decode_block_entities(&root).unwrap();
    let Some(BlockEntity::Unknown(unknown)) = decoded.get(&position()) else {
        panic!("unknown entity was dropped");
    };
    assert_eq!(unknown.data, original);

    let mut second = Tag::Compound(vec![
        ("xPos".into(), Tag::Int(0)),
        ("zPos".into(), Tag::Int(0)),
        ("sections".into(), Tag::List(Vec::new())),
    ]);
    store_block_entities(&mut second, &decoded).unwrap();
    assert_eq!(
        root.get(BLOCK_ENTITIES_KEY),
        second.get(BLOCK_ENTITIES_KEY),
        "an unmodified unknown entity is byte-identical after a save"
    );
}

#[test]
fn legacy_block_entities_are_read_and_rewritten_under_the_modern_key() {
    let mut root = encode_chunk(&chunk_at(0, 0)).unwrap();
    let Tag::Compound(fields) = &mut root else {
        unreachable!();
    };
    fields.push((
        LEGACY_BLOCK_ENTITIES_KEY.into(),
        Tag::List(vec![Tag::Compound(vec![
            ("id".into(), Tag::String("mod:legacy".into())),
            ("x".into(), Tag::Int(1)),
            ("y".into(), Tag::Int(2)),
            ("z".into(), Tag::Int(3)),
            ("LegacyField".into(), Tag::String("kept".into())),
        ])]),
    ));

    let decoded = decode_block_entities(&root).unwrap();
    assert!(decoded.contains_key(&BlockEntityPos::new(1, 2, 3)));
    store_block_entities(&mut root, &decoded).unwrap();
    assert!(root.get(LEGACY_BLOCK_ENTITIES_KEY).is_none());
    assert_eq!(
        root.get(BLOCK_ENTITIES_KEY)
            .and_then(Tag::as_list)
            .map(<[Tag]>::len),
        Some(1)
    );
}

#[test]
fn malformed_block_entity_nbt_fails_explicitly() {
    let missing_id = Tag::Compound(vec![(
        BLOCK_ENTITIES_KEY.into(),
        Tag::List(vec![Tag::Compound(vec![
            ("x".into(), Tag::Int(1)),
            ("y".into(), Tag::Int(2)),
            ("z".into(), Tag::Int(3)),
        ])]),
    )]);
    assert!(decode_block_entities(&missing_id).is_err());

    let bad_items = Tag::Compound(vec![(
        BLOCK_ENTITIES_KEY.into(),
        Tag::List(vec![Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:chest".into())),
            ("x".into(), Tag::Int(1)),
            ("y".into(), Tag::Int(2)),
            ("z".into(), Tag::Int(3)),
            ("Items".into(), Tag::String("not a list".into())),
        ])]),
    )]);
    assert!(decode_block_entities(&bad_items).is_err());

    let bad_sign = Tag::Compound(vec![(
        BLOCK_ENTITIES_KEY.into(),
        Tag::List(vec![Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:sign".into())),
            ("x".into(), Tag::Int(1)),
            ("y".into(), Tag::Int(2)),
            ("z".into(), Tag::Int(3)),
            (
                "front_text".into(),
                Tag::Compound(vec![(
                    "messages".into(),
                    Tag::List(vec![Tag::String("only one line".into())]),
                )]),
            ),
        ])]),
    )]);
    assert!(decode_block_entities(&bad_sign).is_err());
}

#[test]
fn world_storage_loads_saves_and_removes_block_entities_with_their_block() {
    let directory = TempDir::new("world");
    let region_dir = Dimension::Overworld.region_dir(directory.path());
    let chest_pos = BlockEntityPos::new(0, 64, 0);
    let furnace_pos = BlockEntityPos::new(1, 64, 0);

    {
        let mut storage = WorldStorage::new(&region_dir);
        let chunk = chunk_at(0, 0);
        storage.save_chunk(&chunk, Compression::Zlib).unwrap();

        let mut chest = ChestBlockEntity::new(chest_pos);
        chest.set_loot_table(
            Some("minecraft:chests/simple_dungeon".into()),
            -9_000_000_000,
        );
        let mut crown = ItemStack::new("minecraft:golden_apple", 2);
        crown.set_component(ComponentValue::CustomName(Tag::String("Crown".into())));
        chest.set_slot(3, crown);

        let mut furnace = FurnaceBlockEntity::new(furnace_pos, FurnaceKind::BlastFurnace);
        furnace.set_slot(0, ItemStack::new("minecraft:raw_gold", 1));
        furnace.lit_time_remaining = 77;
        furnace.lit_total_time = 100;

        storage
            .insert_block_entity(BlockEntity::Chest(chest))
            .unwrap();
        storage
            .insert_block_entity(BlockEntity::Furnace(furnace))
            .unwrap();
        storage
            .save_chunk_with_block_entities(&chunk, Compression::Zlib)
            .unwrap();
        storage.flush().unwrap();

        assert_eq!(
            storage
                .block_entities(0, 0)
                .unwrap()
                .map(BlockEntityMap::len),
            Some(2)
        );
    }

    {
        let mut storage = WorldStorage::new(&region_dir);
        let chest = storage
            .block_entity(chest_pos)
            .unwrap()
            .expect("chest was stored");
        let BlockEntity::Chest(chest) = chest else {
            panic!("chest type changed");
        };
        assert_eq!(chest.loot_table_seed, -9_000_000_000);
        assert_eq!(
            chest
                .slot(3)
                .and_then(|stack| stack.get_component("minecraft:custom_name")),
            Some(&ComponentValue::CustomName(Tag::String("Crown".into())))
        );
        let furnace = storage
            .block_entity(furnace_pos)
            .unwrap()
            .expect("furnace was stored");
        let BlockEntity::Furnace(furnace) = furnace else {
            panic!("furnace type changed");
        };
        assert_eq!(furnace.kind, FurnaceKind::BlastFurnace);
        assert_eq!(furnace.lit_time_remaining, 77);

        let mut chunk = storage.load_chunk(0, 0).unwrap().unwrap();
        assert!(chunk.set_block_state(0, 64, 0, state("minecraft:air")));
        storage
            .update_chunk(&chunk, Compression::Zlib, &[(0, 64, 0)])
            .unwrap();

        let mut chunk = storage.load_chunk(0, 0).unwrap().unwrap();
        assert!(chunk.set_block_state(2, 64, 2, state("minecraft:dirt")));
        storage
            .update_chunk(&chunk, Compression::Zlib, &[(2, 64, 2)])
            .unwrap();
        storage.flush().unwrap();
    }

    {
        let mut storage = WorldStorage::new(&region_dir);
        let chunk = storage.load_chunk(0, 0).unwrap().unwrap();
        assert_eq!(chunk.block_state(0, 64, 0).unwrap().name, "minecraft:air");
        assert_eq!(chunk.block_state(2, 64, 2).unwrap().name, "minecraft:dirt");
        assert!(
            storage.block_entity(chest_pos).unwrap().is_none(),
            "replacing a block with air removes its block entity"
        );
        assert!(
            storage.block_entity(furnace_pos).unwrap().is_some(),
            "editing a different position keeps the block entity"
        );
        assert_eq!(
            storage
                .remove_block_entity(furnace_pos)
                .unwrap()
                .map(|entity| entity.id().to_owned()),
            Some("minecraft:blast_furnace".into())
        );
        assert!(storage.block_entity(furnace_pos).unwrap().is_none());
    }
}

#[test]
fn a_malformed_stored_chunk_fails_the_load() {
    let directory = TempDir::new("malformed-world");
    let region_dir = Dimension::Overworld.region_dir(directory.path());
    let region_path = region_dir.join("r.0.0.mca");
    let mut root = encode_chunk(&chunk_at(0, 0)).unwrap();
    if let Tag::Compound(fields) = &mut root {
        fields.push((
            BLOCK_ENTITIES_KEY.into(),
            Tag::List(vec![Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:chest".into())),
                ("x".into(), Tag::Int(0)),
                ("z".into(), Tag::Int(0)),
            ])]),
        ));
    }
    let mut region = RegionFile::create(&region_path).unwrap();
    region.write_chunk(0, 0, &root, Compression::Zlib).unwrap();
    region.save().unwrap();

    let mut storage = WorldStorage::new(&region_dir);
    assert!(
        storage.load_chunk(0, 0).is_err(),
        "malformed block-entity NBT must not be silently discarded"
    );
}
