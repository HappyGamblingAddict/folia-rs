use folia_core::chunk::BlockState;
use folia_core::math::{BlockPos, Vec3};
use folia_core::nbt::Tag;
use folia_core::worldgen::feature::FeatureLevel;
use folia_core::worldgen::structure::template::{
    Mirror, Rotation, StructurePlaceSettings, StructureTemplate, transform, transform_vec3,
};
use std::collections::HashMap;
use std::io;

// Byte-for-byte gzip copies of shipped 1.21.11 templates. They are embedded so
// the test does not depend on the out-of-tree decompiled reference directory.
// Sources:
//   data/minecraft/structure/empty.nbt
//   data/minecraft/structure/igloo/middle.nbt
//   data/minecraft/structure/village/desert/camel_spawn.nbt
//   data/minecraft/structure/nether_fossils/fossil_8.nbt
const EMPTY_NBT: &str = "\
1f8b08000000000000ff3dcbb10e82401084e12107ca416c7c1c1b1b6b4bfb950cc946b823b75bf9f48a8a93fcd5e4eb\
8088daf4c90060adda8a68995c5d695817b1bb4f797858f77dc3926d43ff0534e6e2fc89fd2213ddf9212deaabccc461\
d6c4a1c8e827d1f226fd455c6e2ca63901c7335e677d5d0996000000";

const IGLOO_MIDDLE_NBT: &str = "\
1f8b08000000000000ff8d92c16ac330104447729cdacea125877c4c2fbdf45c72eab56cecb523a248465a28e4eb6b43\
44db40622de820789a9915d3003556d15cb800f0efd4a8d88911c311f3d4581fac6f4fb1992ecf358ad1c704a75105ca\
28247c7df1975109ca60f403466778e1aab5944765e451197996bc90b8052f9d914767e4c9f1bafd437567f725e6d66b\
669e46b22cc273557485d5079d19bbb371dc06eae5358a77fc750866aa131a34fbe0470e73d52a6cbe279d60fd307087\
b2271bb9c2baa7d6b801a5f3418e48922fbf9296ba8ec3946af34e429f1ca2f10ed8bee1076063782fe4020000";

const CAMEL_SPAWN_NBT: &str = "\
1f8b08000000000000ff8d52c14ec3300c759b6e4b3b21d03e870b17ce6827ae93d77ac5234daac4d2045f4fb2518db5\
93a8255f9efc9e9ff35201945004fe260500a9b3d845ecbc044d5658984295f00a94dd8b869c1b78ecd852edf120cf35\
7664a288de1b577f6e5d188486ca4a50bd0bcb117c5b252ccffcf32a7d614c84142c82a0d02f633c939cc38c99ff74f2\
193af90c1d3543474d7486675e1c1d5b81151a6e2d351ad607b6687697c9876b00c85e4361630c7f63a1ae97af88f7ce\
993b780af1e98a1eb90d78d2b014f42dc98470c7398caecbe2ccaa4743229442cce3f2b764ead66abcafda7ad7934f1f\
2b5ee53cc76f86c2ce42d5b893dd59e7e50306fac4655cba7e45c177f2219160f3023f47165efcc7020000";

const FOSSIL_8_NBT: &str = "\
1f8b08000000000000ff8d92c10ac2301044274dabb62282f8355ebc78164f5e259615823629490eead7db8855290be9\
c042c2be1992652ba044eef5932480be8aae44891999a083268fa81293f3cdd6575f75976509d95adf1b327c2451f8a0\
027d1c43468c60b2118c4c3051f9801103468c60befa63322627c580f93b979362c0cc87cb49316066c8e5a4988c9961\
64a6adba510814574556a80eceb6e4e22acd90abbbf6100f74c7bd6a08eb461baa9dba84cdd91a3abdb70cbceb9e70f5\
ddc5afabb4eb9e37dfa9a08ee4bcb606586df10264a41102fa020000";

fn hex(source: &str) -> Vec<u8> {
    assert!(
        source.len().is_multiple_of(2),
        "hex input has an odd length"
    );
    (0..source.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&source[index..index + 2], 16).expect("valid hex"))
        .collect()
}

fn load_template(source: &str) -> StructureTemplate {
    StructureTemplate::from_gzip(&hex(source)).expect("embedded template parses")
}

fn root(entries: Vec<(&str, Tag)>) -> Tag {
    Tag::Compound(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

fn ints(values: &[i32]) -> Tag {
    Tag::List(values.iter().map(|value| Tag::Int(*value)).collect())
}

fn palette_entry(name: &str) -> Tag {
    Tag::Compound(vec![("Name".to_owned(), Tag::String(name.to_owned()))])
}

fn palette(name: &str) -> Tag {
    Tag::List(vec![palette_entry(name)])
}

fn block(pos: &[i32], state: i32) -> Tag {
    Tag::Compound(vec![
        ("pos".to_owned(), ints(pos)),
        ("state".to_owned(), Tag::Int(state)),
    ])
}

fn valid_root() -> Tag {
    root(vec![
        ("size", ints(&[3, 3, 3])),
        ("palette", palette("minecraft:stone")),
        ("blocks", Tag::List(vec![block(&[0, 0, 0], 0)])),
        ("entities", Tag::List(Vec::new())),
    ])
}

fn assert_invalid(tag: &Tag) {
    let error = StructureTemplate::from_nbt(tag).expect_err("malformed template must be rejected");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{error}");
}

#[derive(Default)]
struct MockLevel {
    min_y: i32,
    height: i32,
    blocks: HashMap<BlockPos, BlockState>,
}

impl MockLevel {
    fn new(min_y: i32, height: i32) -> Self {
        Self {
            min_y,
            height,
            blocks: HashMap::new(),
        }
    }
}

impl FeatureLevel for MockLevel {
    fn min_y(&self) -> i32 {
        self.min_y
    }

    fn height(&self) -> i32 {
        self.height
    }

    fn get_block(&self, pos: BlockPos) -> Option<&BlockState> {
        self.blocks.get(&pos)
    }

    fn set_block(&mut self, pos: BlockPos, state: BlockState) -> bool {
        if pos.y() < self.min_y || pos.y() >= self.min_y + self.height {
            return false;
        }
        self.blocks.insert(pos, state);
        true
    }
}

fn find_block(
    template: &StructureTemplate,
    settings: &StructurePlaceSettings,
    pos: BlockPos,
) -> BlockState {
    template
        .placement_blocks(settings)
        .into_iter()
        .find(|block| block.pos == pos)
        .unwrap_or_else(|| panic!("no block at {pos:?}"))
        .state
}

#[test]
fn loads_the_shipped_empty_template() {
    let template = load_template(EMPTY_NBT);
    assert_eq!(template.data_version(), 4671);
    assert_eq!(template.size(), [1, 1, 1]);
    assert_eq!(template.palette_count(), 1);
    assert_eq!(template.block_count(), 1);
    assert_eq!(template.entity_count(), 0);
    assert_eq!(
        template.palette(0),
        Some(
            [BlockState {
                name: "minecraft:air".to_owned(),
                properties: Vec::new(),
            }]
            .as_slice()
        )
    );

    let placed = template.placement_blocks(&StructurePlaceSettings::new());
    assert_eq!(placed.len(), 1);
    assert_eq!(placed[0].pos, BlockPos::new(0, 0, 0));
    assert_eq!(placed[0].state.name, "minecraft:air");
    assert!(placed[0].nbt.is_none());

    let ignored = template.placement_blocks(&StructurePlaceSettings::new().with_ignore_air(true));
    assert!(ignored.is_empty(), "ignore-air removes the only entry");
}

#[test]
fn loads_the_igloo_middle_template() {
    let template = load_template(IGLOO_MIDDLE_NBT);
    assert_eq!(template.data_version(), 4671);
    assert_eq!(template.size(), [3, 3, 3]);
    assert_eq!(template.palette_count(), 1);
    assert_eq!(template.block_count(), 15);
    assert_eq!(template.entity_count(), 0);

    let palette = template.palette(0).expect("one palette");
    assert_eq!(palette.len(), 2);
    assert_eq!(palette[0].name, "minecraft:stone_bricks");
    assert!(palette[0].properties.is_empty());
    assert_eq!(palette[1].name, "minecraft:ladder");
    assert_eq!(
        palette[1].properties,
        vec![
            ("facing".to_owned(), "north".to_owned()),
            ("waterlogged".to_owned(), "false".to_owned()),
        ]
    );

    let placed = template.placement_blocks(&StructurePlaceSettings::new());
    assert_eq!(placed.len(), 15);
    assert_eq!(
        placed
            .iter()
            .filter(|block| block.state.name == "minecraft:stone_bricks")
            .count(),
        12
    );
    assert_eq!(
        placed
            .iter()
            .filter(|block| block.state.name == "minecraft:ladder")
            .count(),
        3
    );

    // Java's buildInfoList sorts non-NBT blocks by y, x, z.
    assert_eq!(placed[0].pos, BlockPos::new(0, 0, 1));
    assert_eq!(placed[1].pos, BlockPos::new(1, 0, 0));
    assert_eq!(placed[2].pos, BlockPos::new(1, 0, 1));
    assert_eq!(placed[4].pos, BlockPos::new(2, 0, 1));
}

#[test]
fn loads_the_camel_spawn_template_with_block_entity_and_entity_nbt() {
    let template = load_template(CAMEL_SPAWN_NBT);
    assert_eq!(template.size(), [1, 4, 2]);
    assert_eq!(template.palette_count(), 1);
    assert_eq!(template.block_count(), 8);
    assert_eq!(template.entity_count(), 1);

    let palette = template.palette(0).expect("one palette");
    assert_eq!(palette.len(), 2);
    assert_eq!(palette[0].name, "minecraft:air");
    assert_eq!(palette[1].name, "minecraft:jigsaw");
    assert_eq!(
        palette[1].properties,
        vec![("orientation".to_owned(), "down_north".to_owned())]
    );

    let block_entities: Vec<_> = template
        .blocks()
        .iter()
        .filter(|block| block.nbt.is_some())
        .collect();
    assert_eq!(block_entities.len(), 1);
    assert_eq!(block_entities[0].pos, BlockPos::new(0, 0, 0));
    assert_eq!(block_entities[0].state, 1);
    let nbt = block_entities[0].nbt.as_ref().expect("block entity nbt");
    assert_eq!(
        nbt.get("id").and_then(Tag::as_str),
        Some("minecraft:jigsaw")
    );
    assert_eq!(
        nbt.get("name").and_then(Tag::as_str),
        Some("minecraft:empty")
    );

    let entity = &template.entities()[0];
    assert_eq!(entity.pos, Vec3::new(0.0, 0.0, 0.0));
    assert_eq!(entity.block_pos, BlockPos::new(0, 0, 1));
    let nbt = entity.nbt.as_ref().expect("entity nbt");
    assert_eq!(nbt.get("id").and_then(Tag::as_str), Some("minecraft:camel"));
}

#[test]
fn loads_the_fossil_template_with_axis_properties_and_air_entries() {
    let template = load_template(FOSSIL_8_NBT);
    assert_eq!(template.size(), [3, 5, 1]);
    assert_eq!(template.palette_count(), 1);
    let palette = template.palette(0).expect("one palette");
    assert_eq!(palette.len(), 3);
    assert_eq!(palette[0].name, "minecraft:bone_block");
    assert_eq!(
        palette[0].properties,
        vec![("axis".to_owned(), "y".to_owned())]
    );
    assert_eq!(
        palette[1].properties,
        vec![("axis".to_owned(), "x".to_owned())]
    );
    assert_eq!(palette[2].name, "minecraft:air");
    assert_eq!(template.block_count(), 15);

    let placed = template.placement_blocks(&StructurePlaceSettings::new());
    assert_eq!(
        placed
            .iter()
            .filter(|block| block.state.name == "minecraft:bone_block")
            .count(),
        6
    );
    assert_eq!(
        placed
            .iter()
            .filter(|block| block.state.name == "minecraft:air")
            .count(),
        9
    );
    assert_eq!(
        template
            .placement_blocks(&StructurePlaceSettings::new().with_ignore_air(true))
            .len(),
        6
    );
}

#[test]
fn rotations_transform_positions_and_swap_horizontal_extents() {
    assert_eq!(
        Rotation::ALL.map(Rotation::name),
        ["none", "clockwise_90", "180", "counterclockwise_90"]
    );
    assert_eq!(
        Rotation::ALL.map(|rotation| Rotation::from_name(rotation.name())),
        [
            Some(Rotation::None),
            Some(Rotation::Clockwise90),
            Some(Rotation::Clockwise180),
            Some(Rotation::CounterClockwise90)
        ]
    );

    let template = load_template(FOSSIL_8_NBT);
    let cases = [
        (Rotation::None, BlockPos::new(2, 0, 0)),
        (Rotation::Clockwise90, BlockPos::new(0, 0, 2)),
        (Rotation::Clockwise180, BlockPos::new(-2, 0, 0)),
        (Rotation::CounterClockwise90, BlockPos::new(0, 0, -2)),
    ];
    for (rotation, expected) in cases {
        let settings = StructurePlaceSettings::new().with_rotation(rotation);
        let block = find_block(&template, &settings, expected);
        assert_eq!(block.name, "minecraft:bone_block", "{rotation:?}");
        assert_eq!(
            block.properties,
            vec![("axis".to_owned(), "y".to_owned())],
            "{rotation:?}"
        );
        assert_eq!(
            template.rotated_size(rotation),
            if rotation.swaps_horizontal_axes() {
                [1, 5, 3]
            } else {
                [3, 5, 1]
            }
        );
    }

    // RotatedPillarBlock swaps x/z axes on quarter turns and keeps them on 180.
    for (rotation, expected_axis) in [
        (Rotation::None, "x"),
        (Rotation::Clockwise90, "z"),
        (Rotation::Clockwise180, "x"),
        (Rotation::CounterClockwise90, "z"),
    ] {
        let settings = StructurePlaceSettings::new().with_rotation(rotation);
        let block = find_block(&template, &settings, BlockPos::new(0, 4, 0));
        assert_eq!(block.name, "minecraft:bone_block");
        assert_eq!(
            block.properties,
            vec![("axis".to_owned(), expected_axis.to_owned())],
            "{rotation:?}"
        );
    }
}

#[test]
fn mirrors_are_applied_before_rotation() {
    assert_eq!(
        Mirror::ALL.map(Mirror::name),
        ["none", "left_right", "front_back"]
    );
    assert_eq!(
        Mirror::ALL.map(|mirror| Mirror::from_name(mirror.name())),
        [
            Some(Mirror::None),
            Some(Mirror::LeftRight),
            Some(Mirror::FrontBack)
        ]
    );

    let template = load_template(IGLOO_MIDDLE_NBT);
    let cases = [
        (Mirror::None, Rotation::None, BlockPos::new(1, 0, 2)),
        (Mirror::LeftRight, Rotation::None, BlockPos::new(1, 0, -2)),
        (Mirror::FrontBack, Rotation::None, BlockPos::new(-1, 0, 2)),
        (
            Mirror::LeftRight,
            Rotation::Clockwise90,
            BlockPos::new(2, 0, 1),
        ),
        (
            Mirror::FrontBack,
            Rotation::Clockwise90,
            BlockPos::new(-2, 0, -1),
        ),
        (
            Mirror::LeftRight,
            Rotation::CounterClockwise90,
            BlockPos::new(-2, 0, -1),
        ),
        (
            Mirror::FrontBack,
            Rotation::CounterClockwise90,
            BlockPos::new(2, 0, 1),
        ),
        (
            Mirror::LeftRight,
            Rotation::Clockwise180,
            BlockPos::new(-1, 0, 2),
        ),
        (
            Mirror::FrontBack,
            Rotation::Clockwise180,
            BlockPos::new(1, 0, -2),
        ),
    ];

    for (mirror, rotation, expected) in cases {
        let settings = StructurePlaceSettings::new()
            .with_mirror(mirror)
            .with_rotation(rotation);
        let block = find_block(&template, &settings, expected);
        assert_eq!(
            block.name, "minecraft:stone_bricks",
            "{mirror:?} {rotation:?}"
        );
    }
}

#[test]
fn directional_state_properties_follow_mirror_then_rotation() {
    let template = load_template(IGLOO_MIDDLE_NBT);

    for (mirror, rotation, expected_facing) in [
        (Mirror::None, Rotation::None, "north"),
        (Mirror::None, Rotation::Clockwise90, "east"),
        (Mirror::LeftRight, Rotation::None, "south"),
        (Mirror::FrontBack, Rotation::None, "north"),
        (Mirror::LeftRight, Rotation::Clockwise90, "west"),
        (Mirror::FrontBack, Rotation::Clockwise90, "east"),
    ] {
        let settings = StructurePlaceSettings::new()
            .with_mirror(mirror)
            .with_rotation(rotation);
        let ladders: Vec<_> = template
            .placement_blocks(&settings)
            .into_iter()
            .filter(|block| block.state.name == "minecraft:ladder")
            .collect();
        assert_eq!(ladders.len(), 3);
        for ladder in ladders {
            assert_eq!(
                ladder.state.properties,
                vec![
                    ("facing".to_owned(), expected_facing.to_owned()),
                    ("waterlogged".to_owned(), "false".to_owned()),
                ],
                "{mirror:?} {rotation:?}"
            );
        }
    }

    // Jigsaw orientation is a front/top pair, not a plain facing property.
    let camel = load_template(CAMEL_SPAWN_NBT);
    for (mirror, rotation, expected_orientation) in [
        (Mirror::None, Rotation::None, "down_north"),
        (Mirror::None, Rotation::Clockwise90, "down_east"),
        (Mirror::LeftRight, Rotation::None, "down_south"),
        (Mirror::FrontBack, Rotation::None, "down_north"),
    ] {
        let settings = StructurePlaceSettings::new()
            .with_mirror(mirror)
            .with_rotation(rotation);
        let jigsaw = camel
            .placement_blocks(&settings)
            .into_iter()
            .find(|block| block.state.name == "minecraft:jigsaw")
            .expect("jigsaw block");
        assert_eq!(
            jigsaw.state.properties,
            vec![("orientation".to_owned(), expected_orientation.to_owned())],
            "{mirror:?} {rotation:?}"
        );
    }
}

#[test]
fn rotation_pivot_and_origin_are_applied_in_java_order() {
    let template = load_template(IGLOO_MIDDLE_NBT);
    let source = BlockPos::new(1, 0, 2);

    let without_pivot = StructurePlaceSettings::new().with_rotation(Rotation::Clockwise90);
    assert_eq!(
        find_block(&template, &without_pivot, BlockPos::new(-2, 0, 1)).name,
        "minecraft:stone_bricks"
    );

    let with_pivot = StructurePlaceSettings::new()
        .with_rotation(Rotation::Clockwise90)
        .with_rotation_pivot(BlockPos::new(1, 0, 1));
    assert_eq!(
        transform(
            source,
            with_pivot.mirror(),
            with_pivot.rotation(),
            with_pivot.rotation_pivot()
        ),
        BlockPos::new(0, 0, 1)
    );
    assert_eq!(
        find_block(&template, &with_pivot, BlockPos::new(0, 0, 1)).name,
        "minecraft:stone_bricks"
    );

    let mut level = MockLevel::new(0, 384);
    let placement = template.place_into(&mut level, BlockPos::new(10, 64, -5), &with_pivot);
    assert_eq!(placement.written, 15);
    assert_eq!(placement.rejected, 0);
    assert_eq!(
        level
            .get_block(BlockPos::new(10, 64, -4))
            .expect("pivot-transformed block at origin + (0,0,1)")
            .name,
        "minecraft:stone_bricks"
    );
}

#[test]
fn place_into_writes_blocks_and_returns_block_entities_and_entities() {
    let template = load_template(CAMEL_SPAWN_NBT);
    let mut level = MockLevel::new(0, 384);
    let origin = BlockPos::new(5, 10, 5);
    let placement = template.place_into(&mut level, origin, &StructurePlaceSettings::new());

    assert_eq!(placement.written, 8);
    assert_eq!(placement.rejected, 0);
    assert_eq!(level.blocks.len(), 8);
    assert_eq!(
        level
            .get_block(BlockPos::new(5, 10, 5))
            .expect("jigsaw block")
            .name,
        "minecraft:jigsaw"
    );
    assert_eq!(
        level
            .get_block(BlockPos::new(5, 10, 6))
            .expect("air block")
            .name,
        "minecraft:air"
    );

    assert_eq!(placement.block_entities.len(), 1);
    let block_entity = &placement.block_entities[0];
    assert_eq!(block_entity.pos, BlockPos::new(5, 10, 5));
    assert_eq!(
        block_entity
            .nbt
            .as_ref()
            .and_then(|nbt| nbt.get("id"))
            .and_then(Tag::as_str),
        Some("minecraft:jigsaw")
    );

    assert_eq!(placement.entities.len(), 1);
    assert_eq!(placement.entities[0].pos, Vec3::new(5.0, 10.0, 5.0));
    assert_eq!(placement.entities[0].block_pos, BlockPos::new(5, 10, 6));
    assert_eq!(
        placement.entities[0]
            .nbt
            .as_ref()
            .and_then(|nbt| nbt.get("id"))
            .and_then(Tag::as_str),
        Some("minecraft:camel")
    );

    let mut only_blocks = MockLevel::new(0, 384);
    let settings = StructurePlaceSettings::new()
        .with_ignore_air(true)
        .with_ignore_entities(true);
    let placement = template.place_into(&mut only_blocks, origin, &settings);
    assert_eq!(placement.written, 1);
    assert_eq!(placement.rejected, 0);
    assert_eq!(only_blocks.blocks.len(), 1);
    assert!(placement.block_entities.len() == 1);
    assert!(placement.entities.is_empty());
    assert!(
        only_blocks.get_block(BlockPos::new(5, 10, 6)).is_none(),
        "ignore-air skipped the template's air entries"
    );
}

#[test]
fn out_of_bounds_writes_are_rejected_not_dropped_silently() {
    let template = load_template(FOSSIL_8_NBT);
    let mut level = MockLevel::new(10, 2); // accepts y = 10 and 11 only
    let placement = template.place_into(
        &mut level,
        BlockPos::new(0, 10, 0),
        &StructurePlaceSettings::new(),
    );

    // Three blocks per layer, five layers: y 0/1 (in bounds) and 2/3/4 rejected.
    assert_eq!(placement.written, 6);
    assert_eq!(placement.rejected, 9);
    assert_eq!(level.blocks.len(), 6);
    assert!(level.get_block(BlockPos::new(2, 10, 0)).is_some());
    assert!(level.get_block(BlockPos::new(2, 12, 0)).is_none());
}

#[test]
fn transform_helpers_match_java_vectors() {
    let pivot = BlockPos::new(1, 0, 1);
    assert_eq!(
        transform(BlockPos::new(1, 0, 2), Mirror::None, Rotation::None, pivot),
        BlockPos::new(1, 0, 2),
        "NONE/NONE ignores the pivot"
    );
    assert_eq!(
        transform(
            BlockPos::new(1, 0, 2),
            Mirror::LeftRight,
            Rotation::None,
            pivot
        ),
        BlockPos::new(1, 0, -2),
        "mirror-only does not use the pivot"
    );
    assert_eq!(
        transform(
            BlockPos::new(1, 0, 2),
            Mirror::None,
            Rotation::Clockwise90,
            pivot
        ),
        BlockPos::new(0, 0, 1)
    );

    let vec = Vec3::new(0.0, 4.0, 1.0);
    assert_eq!(
        transform_vec3(vec, Mirror::None, Rotation::None, pivot),
        vec
    );
    assert_eq!(
        transform_vec3(vec, Mirror::LeftRight, Rotation::None, pivot),
        Vec3::new(0.0, 4.0, 0.0)
    );
    assert_eq!(
        transform_vec3(vec, Mirror::None, Rotation::Clockwise90, pivot),
        Vec3::new(2.0, 4.0, 0.0)
    );
}

#[test]
fn malformed_templates_are_rejected() {
    let mut missing_size = valid_root();
    if let Tag::Compound(entries) = &mut missing_size {
        entries.retain(|(key, _)| key != "size");
    }
    assert_invalid(&missing_size);

    let wrong_size = root(vec![
        ("size", ints(&[3, 3])),
        ("palette", palette("minecraft:stone")),
        ("blocks", Tag::List(Vec::new())),
    ]);
    assert_invalid(&wrong_size);

    let zero_size = root(vec![
        ("size", ints(&[3, 0, 1])),
        ("palette", palette("minecraft:stone")),
        ("blocks", Tag::List(Vec::new())),
    ]);
    assert_invalid(&zero_size);

    let missing_palette = root(vec![
        ("size", ints(&[3, 3, 3])),
        ("blocks", Tag::List(Vec::new())),
    ]);
    assert_invalid(&missing_palette);

    let missing_name = root(vec![
        ("size", ints(&[3, 3, 3])),
        ("palette", Tag::List(vec![Tag::Compound(Vec::new())])),
        ("blocks", Tag::List(Vec::new())),
    ]);
    assert_invalid(&missing_name);

    let out_of_range_state = root(vec![
        ("size", ints(&[3, 3, 3])),
        ("palette", palette("minecraft:stone")),
        ("blocks", Tag::List(vec![block(&[0, 0, 0], 5)])),
    ]);
    assert_invalid(&out_of_range_state);

    let negative_state = root(vec![
        ("size", ints(&[3, 3, 3])),
        ("palette", palette("minecraft:stone")),
        ("blocks", Tag::List(vec![block(&[0, 0, 0], -1)])),
    ]);
    assert_invalid(&negative_state);

    let outside_size = root(vec![
        ("size", ints(&[3, 3, 3])),
        ("palette", palette("minecraft:stone")),
        ("blocks", Tag::List(vec![block(&[3, 0, 0], 0)])),
    ]);
    assert_invalid(&outside_size);

    let bad_block_pos = root(vec![
        ("size", ints(&[3, 3, 3])),
        ("palette", palette("minecraft:stone")),
        (
            "blocks",
            Tag::List(vec![Tag::Compound(vec![(
                "pos".to_owned(),
                Tag::String("nope".to_owned()),
            )])]),
        ),
    ]);
    assert_invalid(&bad_block_pos);

    let bad_block_nbt = root(vec![
        ("size", ints(&[3, 3, 3])),
        ("palette", palette("minecraft:stone")),
        (
            "blocks",
            Tag::List(vec![Tag::Compound(vec![
                ("pos".to_owned(), ints(&[0, 0, 0])),
                ("state".to_owned(), Tag::Int(0)),
                ("nbt".to_owned(), Tag::String("nope".to_owned())),
            ])]),
        ),
    ]);
    assert_invalid(&bad_block_nbt);

    let bad_entity_pos = root(vec![
        ("size", ints(&[3, 3, 3])),
        ("palette", palette("minecraft:stone")),
        ("blocks", Tag::List(Vec::new())),
        (
            "entities",
            Tag::List(vec![Tag::Compound(vec![("pos".to_owned(), ints(&[0, 0]))])]),
        ),
    ]);
    assert_invalid(&bad_entity_pos);

    let corrupt_gzip = [0x1f, 0x8b, 0x00];
    assert!(StructureTemplate::from_gzip(&corrupt_gzip).is_err());

    // Sanity check: the builder used for these cases is otherwise valid.
    StructureTemplate::from_nbt(&valid_root()).expect("valid synthetic root");
}

#[test]
fn multiple_palettes_are_parsed_and_validated() {
    let two = Tag::List(vec![
        Tag::List(vec![
            palette_entry("minecraft:stone"),
            palette_entry("minecraft:dirt"),
        ]),
        Tag::List(vec![
            palette_entry("minecraft:stone"),
            palette_entry("minecraft:dirt"),
        ]),
    ]);
    let template = StructureTemplate::from_nbt(&root(vec![
        ("size", ints(&[1, 1, 1])),
        ("palettes", two),
        ("blocks", Tag::List(vec![block(&[0, 0, 0], 1)])),
    ]))
    .expect("two complete palettes");
    assert_eq!(template.palette_count(), 2);
    assert_eq!(
        template.palette(0).expect("first palette")[1].name,
        "minecraft:dirt"
    );
    assert_eq!(
        template.palette(1).expect("second palette")[1].name,
        "minecraft:dirt"
    );
    assert_eq!(
        template.placement_blocks(&StructurePlaceSettings::new())[0]
            .state
            .name,
        "minecraft:dirt",
        "placement uses the first palette until random palette selection lands"
    );

    let truncated = Tag::List(vec![
        Tag::List(vec![
            palette_entry("minecraft:stone"),
            palette_entry("minecraft:dirt"),
        ]),
        Tag::List(vec![palette_entry("minecraft:stone")]),
    ]);
    assert_invalid(&root(vec![
        ("size", ints(&[1, 1, 1])),
        ("palettes", truncated),
        ("blocks", Tag::List(vec![block(&[0, 0, 0], 1)])),
    ]));
}
