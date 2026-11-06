//! Shape-aware light occlusion tests.
//!
//! Vanilla's 1.21.11 light engines merge the two states' outline face shapes
//! before deciding whether light may cross a block face. These tests pin the
//! per-state masks (slabs, stairs, fences/panes/walls, snow layers, transparent
//! blocks) and exercise both the whole-chunk and incremental engines.

use folia_core::chunk::{BlockState, Chunk, ChunkSection, PalettedContainer};
use folia_core::light::{LightScene, SectionLight, face_occludes, light_block};
use folia_core::math::Direction;
use folia_core::registry::block_state_id;

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: vec![],
    }
}

fn state_with(name: &str, properties: &[(&str, &str)]) -> BlockState {
    BlockState {
        name: name.into(),
        properties: properties
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect(),
    }
}

fn id(name: &str) -> i32 {
    block_state_id(&state(name))
}

fn id_with(name: &str, properties: &[(&str, &str)]) -> i32 {
    block_state_id(&state_with(name, properties))
}

fn slab(kind: &str) -> i32 {
    id_with(
        "minecraft:oak_slab",
        &[("type", kind), ("waterlogged", "false")],
    )
}

fn stairs(half: &str, shape: &str) -> i32 {
    id_with(
        "minecraft:oak_stairs",
        &[
            ("facing", "north"),
            ("half", half),
            ("shape", shape),
            ("waterlogged", "false"),
        ],
    )
}

fn faces(state_id: i32) -> Vec<Direction> {
    Direction::ALL
        .into_iter()
        .filter(|&direction| face_occludes(state_id, direction))
        .collect()
}

fn air_chunk(x: i32, z: i32) -> Chunk {
    let sections = (0..=3)
        .map(|y| ChunkSection {
            y,
            block_states: PalettedContainer {
                palette: vec![state("minecraft:air")],
                indices: vec![0; 4096],
            },
            biomes: Some(PalettedContainer {
                palette: vec!["minecraft:plains".into()],
                indices: vec![0; 64],
            }),
            light: SectionLight::default(),
        })
        .collect();
    Chunk { x, z, sections }
}

fn set(chunk: &mut Chunk, x: i32, y: i32, z: i32, name: &str) {
    assert!(chunk.set_block_state(x as usize, y, z as usize, state(name)));
}

fn set_state(chunk: &mut Chunk, x: i32, y: i32, z: i32, value: BlockState) {
    assert!(chunk.set_block_state(x as usize, y, z as usize, value));
}

fn level(chunk: &Chunk, x: usize, y: i32, z: usize) -> u8 {
    chunk.sky_light(x, y, z).unwrap_or(0)
}

fn block_level(chunk: &Chunk, x: usize, y: i32, z: usize) -> u8 {
    chunk.block_light(x, y, z).unwrap_or(0)
}

#[test]
fn face_masks_follow_the_generated_outline_shapes() {
    assert!(faces(id("minecraft:air")).is_empty());
    assert_eq!(faces(id("minecraft:stone")), Direction::ALL.to_vec());

    // A bottom or top slab covers both horizontal faces, but its side face is
    // only half a block tall, so light may still enter sideways.
    let bottom_slab = slab("bottom");
    assert_eq!(
        faces(bottom_slab),
        vec![Direction::Down, Direction::Up],
        "bottom slab"
    );
    let top_slab = slab("top");
    assert_eq!(
        faces(top_slab),
        vec![Direction::Down, Direction::Up],
        "top slab"
    );

    // Stairs are two boxes. A straight bottom stair covers DOWN and the
    // full-height back face (NORTH for this state), but not the open step.
    assert_eq!(
        faces(stairs("bottom", "straight")),
        vec![Direction::Down, Direction::North],
        "straight bottom stair"
    );
    assert_eq!(
        faces(stairs("top", "straight")),
        vec![Direction::Up, Direction::North],
        "straight top stair"
    );

    // Inner corners gain a third full face from the union of their boxes;
    // outer corners keep only the slab's DOWN face.
    assert_eq!(
        faces(stairs("bottom", "inner_left")),
        vec![Direction::Down, Direction::North, Direction::West],
        "inner corner stair"
    );
    assert_eq!(
        faces(stairs("bottom", "outer_left")),
        vec![Direction::Down],
        "outer corner stair"
    );

    // Fences, panes, and walls are thin crosses even when fully connected:
    // they attenuate at most one level and never cover a whole face.
    let connected_fence = id_with(
        "minecraft:oak_fence",
        &[
            ("east", "true"),
            ("north", "true"),
            ("south", "true"),
            ("waterlogged", "false"),
            ("west", "true"),
        ],
    );
    let connected_pane = id_with(
        "minecraft:glass_pane",
        &[
            ("east", "true"),
            ("north", "true"),
            ("south", "true"),
            ("waterlogged", "false"),
            ("west", "true"),
        ],
    );
    let connected_wall = id_with(
        "minecraft:cobblestone_wall",
        &[
            ("east", "tall"),
            ("north", "tall"),
            ("south", "tall"),
            ("up", "true"),
            ("waterlogged", "false"),
            ("west", "tall"),
        ],
    );
    assert!(faces(connected_fence).is_empty(), "connected fence");
    assert!(faces(connected_pane).is_empty(), "connected pane");
    assert!(faces(connected_wall).is_empty(), "connected wall");

    // A snow layer slices at its own top: one layer covers the projected
    // horizontal square, while eight layers are a full cube and also cover
    // the four sides.
    let snow_one = id_with("minecraft:snow", &[("layers", "1")]);
    let snow_eight = id_with("minecraft:snow", &[("layers", "8")]);
    assert_eq!(
        faces(snow_one),
        vec![Direction::Down, Direction::Up],
        "snow layer 1"
    );
    assert_eq!(faces(snow_eight), Direction::ALL.to_vec(), "snow layer 8");
    assert_eq!(light_block(snow_one), 0);
    assert_eq!(light_block(snow_eight), 15);

    // Farmland and paths are 15/16 tall: full top/bottom faces, open sides.
    assert_eq!(
        faces(id("minecraft:farmland")),
        vec![Direction::Down, Direction::Up],
        "farmland"
    );
    assert_eq!(
        faces(id("minecraft:dirt_path")),
        vec![Direction::Down, Direction::Up],
        "dirt path"
    );

    // Soul sand is an opaque full-cube-ish block; honey and the transparent
    // full cubes keep their attenuation without shape-occluding (vanilla
    // `noOcclusion()`).
    assert_eq!(
        faces(id("minecraft:soul_sand")),
        Direction::ALL.to_vec(),
        "soul sand"
    );
    for name in [
        "minecraft:honey_block",
        "minecraft:glass",
        "minecraft:ice",
        "minecraft:oak_leaves",
    ] {
        assert!(faces(id(name)).is_empty(), "{name} should stay transparent");
    }
    assert_eq!(light_block(id("minecraft:glass")), 0);
    for name in [
        "minecraft:water",
        "minecraft:oak_leaves",
        "minecraft:ice",
        "minecraft:honey_block",
    ] {
        assert_eq!(light_block(id(name)), 1, "{name} attenuation");
    }
    // Water has no outline at all (fluids are passable), so it never
    // shape-occludes even though it attenuates by one.
    assert!(faces(id("minecraft:water")).is_empty());
}

#[test]
fn opaque_emitters_still_light_their_neighbours() {
    // Glowstone's outline is a full cube. Vanilla skips its face shape because
    // `useShapeForLightOcclusion` is false; only `light_block` (15) matters, so
    // the emitter itself must still fill the adjacent air cells.
    let mut chunk = air_chunk(0, 0);
    set(&mut chunk, 8, 40, 8, "minecraft:glowstone");
    chunk.recompute_light();

    assert_eq!(block_level(&chunk, 8, 40, 8), 15, "glowstone source");
    assert_eq!(block_level(&chunk, 7, 40, 8), 14, "west neighbour");
    assert_eq!(block_level(&chunk, 9, 40, 8), 14, "east neighbour");
    assert_eq!(block_level(&chunk, 8, 39, 8), 14, "neighbour below");
    assert_eq!(block_level(&chunk, 8, 41, 8), 14, "neighbour above");

    // Campfires have a partial outline whose projected top/bottom faces are
    // full, but `CampfireBlock` leaves `useShapeForLightOcclusion` false. The
    // emission must escape vertically as well as sideways.
    let mut campfire = air_chunk(0, 0);
    set(&mut campfire, 8, 40, 8, "minecraft:campfire");
    campfire.recompute_light();
    assert_eq!(block_level(&campfire, 8, 40, 8), 15, "campfire source");
    assert_eq!(block_level(&campfire, 8, 41, 8), 14, "campfire above");
    assert_eq!(block_level(&campfire, 8, 39, 8), 14, "campfire below");
    assert_eq!(block_level(&campfire, 7, 40, 8), 14, "campfire west");
}

#[test]
fn blocks_with_use_shape_disabled_do_not_occlude() {
    // A closed trapdoor is a full horizontal projection, but vanilla does not
    // use its shape for light occlusion, so direct sky still reaches the cells
    // above and below it.
    let mut chunk = air_chunk(0, 0);
    set(&mut chunk, 8, 40, 8, "minecraft:oak_trapdoor");
    chunk.recompute_light();
    assert_eq!(level(&chunk, 8, 41, 8), 15, "above the trapdoor");
    assert_eq!(level(&chunk, 8, 40, 8), 15, "trapdoor cell");
    assert_eq!(level(&chunk, 8, 39, 8), 15, "below the trapdoor");
}

#[test]
fn slab_blocks_sky_from_above_but_not_from_the_side() {
    let mut chunk = air_chunk(0, 0);
    set_state(
        &mut chunk,
        8,
        40,
        8,
        state_with(
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        ),
    );
    chunk.recompute_light();

    assert_eq!(level(&chunk, 8, 41, 8), 15, "above the slab is direct sky");
    assert_eq!(
        level(&chunk, 7, 40, 8),
        15,
        "an open column beside the slab is still direct sky"
    );
    assert_eq!(
        level(&chunk, 8, 40, 8),
        14,
        "the slab cell only receives light through its open side"
    );
    assert_eq!(
        level(&chunk, 8, 39, 8),
        14,
        "the cell below the slab is lit from the side, not through the top face"
    );

    // A top slab covers its DOWN face too, so it blocks vertical propagation
    // in both directions while still passing side light.
    let mut top = air_chunk(0, 0);
    set_state(
        &mut top,
        8,
        40,
        8,
        state_with(
            "minecraft:oak_slab",
            &[("type", "top"), ("waterlogged", "false")],
        ),
    );
    top.recompute_light();
    assert_eq!(level(&top, 8, 40, 8), 14);
    assert_eq!(level(&top, 8, 39, 8), 14);
}

#[test]
fn snow_layer_eight_is_opaque_while_layer_one_is_only_a_gap() {
    let mut snow_one = air_chunk(0, 0);
    set_state(
        &mut snow_one,
        8,
        40,
        8,
        state_with("minecraft:snow", &[("layers", "1")]),
    );
    snow_one.recompute_light();
    assert_eq!(level(&snow_one, 8, 40, 8), 14);
    assert_eq!(level(&snow_one, 8, 39, 8), 14);

    let mut snow_eight = air_chunk(0, 0);
    set_state(
        &mut snow_eight,
        8,
        40,
        8,
        state_with("minecraft:snow", &[("layers", "8")]),
    );
    snow_eight.recompute_light();
    assert_eq!(
        level(&snow_eight, 8, 40, 8),
        0,
        "a full snow layer is opaque"
    );
    assert_eq!(
        level(&snow_eight, 8, 39, 8),
        14,
        "the cell below still receives the surrounding sky light"
    );
}

#[test]
fn transparent_full_cubes_keep_their_attenuation() {
    let mut glass = air_chunk(0, 0);
    set(&mut glass, 8, 40, 8, "minecraft:glass");
    glass.recompute_light();
    assert_eq!(
        level(&glass, 8, 40, 8),
        15,
        "glass does not attenuate sky light"
    );
    assert_eq!(level(&glass, 8, 39, 8), 15);

    for name in ["minecraft:water", "minecraft:oak_leaves", "minecraft:ice"] {
        let mut chunk = air_chunk(0, 0);
        set(&mut chunk, 8, 40, 8, name);
        chunk.recompute_light();
        assert_eq!(level(&chunk, 8, 40, 8), 14, "{name} attenuates one level");
        assert_eq!(level(&chunk, 8, 39, 8), 14, "{name} below");
    }
}

#[test]
fn torch_under_a_slab_still_lights_sideways() {
    let mut chunk = air_chunk(0, 0);
    set_state(
        &mut chunk,
        8,
        41,
        8,
        state_with(
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        ),
    );
    set(&mut chunk, 8, 40, 8, "minecraft:torch");
    chunk.recompute_light();

    assert_eq!(block_level(&chunk, 8, 40, 8), 14, "torch source");
    assert_eq!(block_level(&chunk, 7, 40, 8), 13, "west of the torch");
    assert_eq!(block_level(&chunk, 9, 40, 8), 13, "east of the torch");
    assert_eq!(block_level(&chunk, 8, 39, 8), 13, "below the torch");
    assert_eq!(
        block_level(&chunk, 7, 41, 8),
        12,
        "light escapes around the slab's open side"
    );
}

#[test]
fn incremental_slab_edit_clears_and_restores_light() {
    let mut chunk = air_chunk(0, 0);
    let mut scene = LightScene::new(&mut chunk, &mut []);
    scene.recompute_all();
    scene.clear_dirty();

    assert_eq!(scene.sky_level(8, 40, 8), 15);
    assert!(scene.set_block_state(
        8,
        40,
        8,
        state_with(
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "false"),]
        )
    ));
    assert_eq!(scene.sky_level(8, 41, 8), 15);
    assert_eq!(scene.sky_level(8, 40, 8), 14);
    assert_eq!(scene.sky_level(8, 39, 8), 14);
    assert!(!scene.dirty().is_empty());

    assert!(scene.set_block_state(8, 40, 8, state("minecraft:air")));
    assert_eq!(scene.sky_level(8, 40, 8), 15);
    assert_eq!(scene.sky_level(8, 39, 8), 15);

    // Torch light through the slab's side still decreases correctly when the
    // slab closes the vertical path, then comes back when it is removed.
    assert!(scene.set_block_state(8, 40, 8, state("minecraft:torch")));
    assert_eq!(scene.block_level(8, 41, 8), 13);
    assert!(scene.set_block_state(
        8,
        41,
        8,
        state_with(
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "false"),]
        )
    ));
    let shaded = scene.block_level(8, 41, 8);
    assert!(
        shaded < 13,
        "the slab must clear the direct torch light above it, got {shaded}"
    );
    assert!(shaded > 0, "side light still reaches the slab cell");

    assert!(scene.set_block_state(8, 41, 8, state("minecraft:air")));
    assert_eq!(scene.block_level(8, 41, 8), 13);

    assert!(scene.set_block_state(8, 40, 8, state("minecraft:air")));
    assert_eq!(scene.block_level(8, 41, 8), 0, "torch decrease clears it");
}

fn shape_fixture() -> Chunk {
    let mut chunk = air_chunk(0, 0);
    for x in 0..16 {
        for z in 0..16 {
            if (x + z) % 4 == 0 {
                set(&mut chunk, x, 30, z, "minecraft:stone");
            }
        }
    }
    set_state(
        &mut chunk,
        3,
        31,
        3,
        state_with(
            "minecraft:oak_stairs",
            &[
                ("facing", "north"),
                ("half", "bottom"),
                ("shape", "outer_left"),
                ("waterlogged", "false"),
            ],
        ),
    );
    set(&mut chunk, 4, 31, 4, "minecraft:glass");
    set(&mut chunk, 5, 31, 5, "minecraft:water");
    set_state(
        &mut chunk,
        6,
        31,
        6,
        state_with(
            "minecraft:oak_slab",
            &[("type", "top"), ("waterlogged", "false")],
        ),
    );
    set_state(
        &mut chunk,
        7,
        31,
        7,
        state_with("minecraft:snow", &[("layers", "3")]),
    );
    set(&mut chunk, 8, 30, 8, "minecraft:torch");
    chunk
}

fn apply_shape_edit_to_chunk(chunk: &mut Chunk, x: i32, y: i32, z: i32, next: BlockState) {
    set_state(chunk, x, y, z, next);
}

fn apply_shape_edits_to_scene(scene: &mut LightScene<'_>) {
    let edits = [
        (4, 30, 4, state("minecraft:air")),
        (
            5,
            31,
            5,
            state_with(
                "minecraft:oak_slab",
                &[("type", "bottom"), ("waterlogged", "false")],
            ),
        ),
        (
            6,
            31,
            6,
            state_with(
                "minecraft:oak_stairs",
                &[
                    ("facing", "north"),
                    ("half", "bottom"),
                    ("shape", "outer_right"),
                    ("waterlogged", "false"),
                ],
            ),
        ),
        (8, 30, 8, state("minecraft:air")),
        (9, 30, 9, state("minecraft:torch")),
        (10, 31, 10, state_with("minecraft:snow", &[("layers", "8")])),
    ];
    for (x, y, z, next) in edits {
        assert!(scene.set_block_state(x, y, z, next));
    }
}

fn apply_shape_edits_to_chunk(chunk: &mut Chunk) {
    apply_shape_edit_to_chunk(chunk, 4, 30, 4, state("minecraft:air"));
    apply_shape_edit_to_chunk(
        chunk,
        5,
        31,
        5,
        state_with(
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        ),
    );
    apply_shape_edit_to_chunk(
        chunk,
        6,
        31,
        6,
        state_with(
            "minecraft:oak_stairs",
            &[
                ("facing", "north"),
                ("half", "bottom"),
                ("shape", "outer_right"),
                ("waterlogged", "false"),
            ],
        ),
    );
    apply_shape_edit_to_chunk(chunk, 8, 30, 8, state("minecraft:air"));
    apply_shape_edit_to_chunk(chunk, 9, 30, 9, state("minecraft:torch"));
    apply_shape_edit_to_chunk(
        chunk,
        10,
        31,
        10,
        state_with("minecraft:snow", &[("layers", "8")]),
    );
}

#[test]
fn incremental_shape_edits_match_a_bulk_recompute() {
    let mut center = shape_fixture();
    let incremental = {
        let mut scene = LightScene::new(&mut center, &mut []);
        scene.recompute_all();
        apply_shape_edits_to_scene(&mut scene);
        center.clone()
    };

    let mut bulk_center = shape_fixture();
    apply_shape_edits_to_chunk(&mut bulk_center);
    bulk_center.recompute_light();

    // The bulk path stops materializing layers at the highest sky source, while
    // the incremental scene materializes every section it owns. Compare the
    // range both paths persist; the structures all live below y=48.
    for y in 0..48 {
        for z in 0..16 {
            for x in 0..16 {
                assert_eq!(
                    incremental.sky_light(x, y, z),
                    bulk_center.sky_light(x, y, z),
                    "sky light differs at ({x}, {y}, {z})"
                );
                assert_eq!(
                    incremental.block_light(x, y, z),
                    bulk_center.block_light(x, y, z),
                    "block light differs at ({x}, {y}, {z})"
                );
            }
        }
    }
}

#[test]
fn slab_occlusion_still_crosses_the_chunk_border() {
    let mut center = air_chunk(0, 0);
    set_state(
        &mut center,
        15,
        40,
        8,
        state_with(
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        ),
    );
    let mut east = air_chunk(1, 0);
    center.recompute_light_with_neighbours(std::slice::from_ref(&east));

    assert_eq!(level(&center, 15, 41, 8), 15);
    assert_eq!(level(&center, 15, 40, 8), 14, "side-lit border slab");
    assert_eq!(level(&center, 14, 40, 8), 15, "the inner column is open");

    // The incremental path must use the same shape test across the border.
    let mut scene = LightScene::new(&mut center, std::slice::from_mut(&mut east));
    scene.clear_dirty();
    assert!(scene.set_block_state(15, 40, 8, state("minecraft:air")));
    assert_eq!(scene.sky_level(15, 40, 8), 15);
    assert_eq!(scene.sky_level(16, 40, 8), 15);
}
