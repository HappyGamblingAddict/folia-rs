//! Differential-style checks for the generated per-state block shapes and the
//! player/entity physics that consume them.
//!
//! Expected boxes are the oracle values from
//! `assets/block_shapes.tsv.gz` (Folia 1.21.11 `getCollisionShape`/`getShape`).

use folia_core::block_shapes::{self, Aabb};
use folia_core::chunk::BlockState;
use folia_core::entity::{BlockCollider, Entity};
use folia_core::player_physics::{MovementInput, PhysicsWorld, PlayerPhysics, STEP_HEIGHT};
use folia_core::registry;

fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
    BlockState {
        name: name.into(),
        properties: properties
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect(),
    }
}

fn id(name: &str, properties: &[(&str, &str)]) -> i32 {
    registry::block_state_id(&state(name, properties))
}

fn collision(name: &str, properties: &[(&str, &str)]) -> Vec<Aabb> {
    block_shapes::collision_boxes(id(name, properties)).to_vec()
}

fn outline(name: &str, properties: &[(&str, &str)]) -> Vec<Aabb> {
    block_shapes::outline_boxes(id(name, properties)).to_vec()
}

fn assert_boxes(actual: &[Aabb], expected: &[Aabb]) {
    assert_eq!(actual.len(), expected.len(), "actual={actual:?}");
    for (actual, expected) in actual.iter().zip(expected) {
        assert!(
            (actual.min_x - expected.min_x).abs() < 1.0e-9
                && (actual.min_y - expected.min_y).abs() < 1.0e-9
                && (actual.min_z - expected.min_z).abs() < 1.0e-9
                && (actual.max_x - expected.max_x).abs() < 1.0e-9
                && (actual.max_y - expected.max_y).abs() < 1.0e-9
                && (actual.max_z - expected.max_z).abs() < 1.0e-9,
            "actual={actual:?} expected={expected:?}"
        );
    }
}

#[test]
fn table_covers_every_registry_state() {
    assert_eq!(block_shapes::load_error(), None);
    assert_eq!(block_shapes::state_count(), registry::block_state_count());
    assert_eq!(block_shapes::state_count(), 29_671);
}

#[test]
fn stone_is_a_full_cube() {
    assert_boxes(&collision("minecraft:stone", &[]), &[Aabb::FULL]);
    assert_boxes(&outline("minecraft:stone", &[]), &[Aabb::FULL]);
    assert!(block_shapes::has_collision(id("minecraft:stone", &[])));
}

#[test]
fn slabs_use_bottom_top_and_double_shapes() {
    let bottom = id(
        "minecraft:oak_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    );
    assert_boxes(
        block_shapes::collision_boxes(bottom),
        &[Aabb::new(0.0, 0.0, 0.0, 1.0, 0.5, 1.0)],
    );

    let top = id(
        "minecraft:oak_slab",
        &[("type", "top"), ("waterlogged", "false")],
    );
    assert_boxes(
        block_shapes::collision_boxes(top),
        &[Aabb::new(0.0, 0.5, 0.0, 1.0, 1.0, 1.0)],
    );

    let double = id(
        "minecraft:oak_slab",
        &[("type", "double"), ("waterlogged", "false")],
    );
    assert_boxes(block_shapes::collision_boxes(double), &[Aabb::FULL]);
    assert_boxes(block_shapes::outline_boxes(double), &[Aabb::FULL]);
}

#[test]
fn stairs_use_half_and_corner_shapes() {
    let straight_bottom = id(
        "minecraft:oak_stairs",
        &[
            ("facing", "north"),
            ("half", "bottom"),
            ("shape", "straight"),
            ("waterlogged", "false"),
        ],
    );
    assert_boxes(
        block_shapes::collision_boxes(straight_bottom),
        &[
            Aabb::new(0.0, 0.0, 0.0, 1.0, 0.5, 1.0),
            Aabb::new(0.0, 0.5, 0.0, 1.0, 1.0, 0.5),
        ],
    );

    let straight_top = id(
        "minecraft:oak_stairs",
        &[
            ("facing", "north"),
            ("half", "top"),
            ("shape", "straight"),
            ("waterlogged", "false"),
        ],
    );
    assert_boxes(
        block_shapes::collision_boxes(straight_top),
        &[
            Aabb::new(0.0, 0.0, 0.0, 1.0, 1.0, 0.5),
            Aabb::new(0.0, 0.5, 0.5, 1.0, 1.0, 1.0),
        ],
    );

    let outer_left = id(
        "minecraft:oak_stairs",
        &[
            ("facing", "north"),
            ("half", "bottom"),
            ("shape", "outer_left"),
            ("waterlogged", "false"),
        ],
    );
    assert_boxes(
        block_shapes::collision_boxes(outer_left),
        &[
            Aabb::new(0.0, 0.0, 0.0, 1.0, 0.5, 1.0),
            Aabb::new(0.0, 0.5, 0.0, 0.5, 1.0, 0.5),
        ],
    );

    let outer_right = id(
        "minecraft:oak_stairs",
        &[
            ("facing", "north"),
            ("half", "bottom"),
            ("shape", "outer_right"),
            ("waterlogged", "false"),
        ],
    );
    assert_boxes(
        block_shapes::collision_boxes(outer_right),
        &[
            Aabb::new(0.0, 0.0, 0.0, 1.0, 0.5, 1.0),
            Aabb::new(0.5, 0.5, 0.0, 1.0, 1.0, 0.5),
        ],
    );

    let inner_left = id(
        "minecraft:oak_stairs",
        &[
            ("facing", "north"),
            ("half", "bottom"),
            ("shape", "inner_left"),
            ("waterlogged", "false"),
        ],
    );
    assert_boxes(
        block_shapes::collision_boxes(inner_left),
        &[
            Aabb::new(0.0, 0.0, 0.0, 1.0, 0.5, 1.0),
            Aabb::new(0.0, 0.5, 0.0, 0.5, 1.0, 1.0),
            Aabb::new(0.5, 0.5, 0.0, 1.0, 1.0, 0.5),
        ],
    );
}

#[test]
fn fence_has_a_post_and_connecting_arms() {
    let north = id(
        "minecraft:oak_fence",
        &[
            ("east", "false"),
            ("north", "true"),
            ("south", "false"),
            ("waterlogged", "false"),
            ("west", "false"),
        ],
    );
    assert_boxes(
        block_shapes::collision_boxes(north),
        &[Aabb::new(0.375, 0.0, 0.0, 0.625, 1.5, 0.625)],
    );
    assert_boxes(
        block_shapes::outline_boxes(north),
        &[Aabb::new(0.375, 0.0, 0.0, 0.625, 1.0, 0.625)],
    );

    let crossing = id(
        "minecraft:oak_fence",
        &[
            ("east", "true"),
            ("north", "true"),
            ("south", "false"),
            ("waterlogged", "false"),
            ("west", "true"),
        ],
    );
    assert_boxes(
        block_shapes::collision_boxes(crossing),
        &[
            Aabb::new(0.0, 0.0, 0.375, 1.0, 1.5, 0.625),
            Aabb::new(0.375, 0.0, 0.0, 0.625, 1.5, 0.375),
        ],
    );
}

#[test]
fn pane_is_a_thin_post_with_connecting_arms() {
    let east = id(
        "minecraft:glass_pane",
        &[
            ("east", "true"),
            ("north", "false"),
            ("south", "false"),
            ("waterlogged", "false"),
            ("west", "false"),
        ],
    );
    assert_boxes(
        block_shapes::collision_boxes(east),
        &[Aabb::new(0.4375, 0.0, 0.4375, 1.0, 1.0, 0.5625)],
    );
    assert_boxes(
        block_shapes::outline_boxes(east),
        &[Aabb::new(0.4375, 0.0, 0.4375, 1.0, 1.0, 0.5625)],
    );

    let post = id(
        "minecraft:iron_bars",
        &[
            ("east", "false"),
            ("north", "false"),
            ("south", "false"),
            ("waterlogged", "false"),
            ("west", "false"),
        ],
    );
    assert_boxes(
        block_shapes::collision_boxes(post),
        &[Aabb::new(0.4375, 0.0, 0.4375, 0.5625, 1.0, 0.5625)],
    );
}

#[test]
fn wall_post_and_low_arms_use_the_taller_collision_shape() {
    let wall = id(
        "minecraft:cobblestone_wall",
        &[
            ("east", "none"),
            ("north", "low"),
            ("south", "none"),
            ("up", "true"),
            ("waterlogged", "false"),
            ("west", "none"),
        ],
    );
    assert_boxes(
        block_shapes::collision_boxes(wall),
        &[
            Aabb::new(0.25, 0.0, 0.25, 0.75, 1.5, 0.75),
            Aabb::new(0.3125, 0.0, 0.0, 0.6875, 1.5, 0.25),
        ],
    );
    assert_boxes(
        block_shapes::outline_boxes(wall),
        &[
            Aabb::new(0.25, 0.0, 0.25, 0.75, 1.0, 0.75),
            Aabb::new(0.3125, 0.0, 0.0, 0.6875, 0.875, 0.25),
        ],
    );
}

#[test]
fn fence_gate_is_empty_while_open() {
    let closed = id(
        "minecraft:oak_fence_gate",
        &[
            ("facing", "north"),
            ("in_wall", "false"),
            ("open", "false"),
            ("powered", "false"),
        ],
    );
    assert_boxes(
        block_shapes::collision_boxes(closed),
        &[Aabb::new(0.0, 0.0, 0.375, 1.0, 1.5, 0.625)],
    );
    assert_boxes(
        block_shapes::outline_boxes(closed),
        &[Aabb::new(0.0, 0.0, 0.375, 1.0, 1.0, 0.625)],
    );

    let open = id(
        "minecraft:oak_fence_gate",
        &[
            ("facing", "north"),
            ("in_wall", "false"),
            ("open", "true"),
            ("powered", "false"),
        ],
    );
    assert!(block_shapes::collision_boxes(open).is_empty());
    assert!(!block_shapes::outline_boxes(open).is_empty());
}

#[test]
fn snow_layers_use_height_and_the_first_layer_has_no_collision() {
    let one = id("minecraft:snow", &[("layers", "1")]);
    assert!(block_shapes::collision_boxes(one).is_empty());
    assert_boxes(
        block_shapes::outline_boxes(one),
        &[Aabb::new(0.0, 0.0, 0.0, 1.0, 0.125, 1.0)],
    );
    assert!(!block_shapes::has_collision(one));

    let eight = id("minecraft:snow", &[("layers", "8")]);
    assert_boxes(
        block_shapes::collision_boxes(eight),
        &[Aabb::new(0.0, 0.0, 0.0, 1.0, 0.875, 1.0)],
    );
    assert_boxes(block_shapes::outline_boxes(eight), &[Aabb::FULL]);
    assert!(block_shapes::has_collision(eight));
}

#[test]
fn carpets_and_ground_layers_have_partial_heights() {
    assert_boxes(
        &collision("minecraft:white_carpet", &[]),
        &[Aabb::new(0.0, 0.0, 0.0, 1.0, 0.0625, 1.0)],
    );
    assert_boxes(
        &collision("minecraft:farmland", &[("moisture", "0")]),
        &[Aabb::new(0.0, 0.0, 0.0, 1.0, 0.9375, 1.0)],
    );
    assert_boxes(
        &collision("minecraft:dirt_path", &[]),
        &[Aabb::new(0.0, 0.0, 0.0, 1.0, 0.9375, 1.0)],
    );
    assert_boxes(
        &collision("minecraft:soul_sand", &[]),
        &[Aabb::new(0.0, 0.0, 0.0, 1.0, 0.875, 1.0)],
    );
    assert_boxes(&outline("minecraft:soul_sand", &[]), &[Aabb::FULL]);
    assert_boxes(
        &collision("minecraft:honey_block", &[]),
        &[Aabb::new(0.0625, 0.0, 0.0625, 0.9375, 0.9375, 0.9375)],
    );
    assert_boxes(&outline("minecraft:honey_block", &[]), &[Aabb::FULL]);
}

#[test]
fn doors_and_trapdoors_rotate_with_open_and_hinge() {
    let closed = id(
        "minecraft:oak_door",
        &[
            ("facing", "north"),
            ("half", "lower"),
            ("hinge", "left"),
            ("open", "false"),
            ("powered", "false"),
        ],
    );
    assert_boxes(
        block_shapes::collision_boxes(closed),
        &[Aabb::new(0.0, 0.0, 0.8125, 1.0, 1.0, 1.0)],
    );

    let open_left = id(
        "minecraft:oak_door",
        &[
            ("facing", "north"),
            ("half", "lower"),
            ("hinge", "left"),
            ("open", "true"),
            ("powered", "false"),
        ],
    );
    assert_boxes(
        block_shapes::collision_boxes(open_left),
        &[Aabb::new(0.0, 0.0, 0.0, 0.1875, 1.0, 1.0)],
    );

    let open_right = id(
        "minecraft:oak_door",
        &[
            ("facing", "north"),
            ("half", "lower"),
            ("hinge", "right"),
            ("open", "true"),
            ("powered", "false"),
        ],
    );
    assert_boxes(
        block_shapes::collision_boxes(open_right),
        &[Aabb::new(0.8125, 0.0, 0.0, 1.0, 1.0, 1.0)],
    );

    let trapdoor_bottom = id(
        "minecraft:oak_trapdoor",
        &[
            ("facing", "north"),
            ("half", "bottom"),
            ("open", "false"),
            ("powered", "false"),
            ("waterlogged", "false"),
        ],
    );
    assert_boxes(
        block_shapes::collision_boxes(trapdoor_bottom),
        &[Aabb::new(0.0, 0.0, 0.0, 1.0, 0.1875, 1.0)],
    );

    let trapdoor_top = id(
        "minecraft:oak_trapdoor",
        &[
            ("facing", "north"),
            ("half", "top"),
            ("open", "false"),
            ("powered", "false"),
            ("waterlogged", "false"),
        ],
    );
    assert_boxes(
        block_shapes::collision_boxes(trapdoor_top),
        &[Aabb::new(0.0, 0.8125, 0.0, 1.0, 1.0, 1.0)],
    );

    let trapdoor_open = id(
        "minecraft:oak_trapdoor",
        &[
            ("facing", "north"),
            ("half", "bottom"),
            ("open", "true"),
            ("powered", "false"),
            ("waterlogged", "false"),
        ],
    );
    assert_boxes(
        block_shapes::collision_boxes(trapdoor_open),
        &[Aabb::new(0.0, 0.0, 0.8125, 1.0, 1.0, 1.0)],
    );
}

#[test]
fn passable_states_and_fluids_have_no_boxes() {
    for (name, properties) in [
        ("minecraft:water", &[("level", "0")][..]),
        ("minecraft:short_grass", &[][..]),
        ("minecraft:poppy", &[][..]),
        ("minecraft:torch", &[][..]),
    ] {
        let state_id = id(name, properties);
        assert!(
            block_shapes::collision_boxes(state_id).is_empty(),
            "{name} collision"
        );
        assert!(
            block_shapes::outline_boxes(state_id).is_empty(),
            "{name} outline"
        );
        assert!(
            !block_shapes::has_collision(state_id),
            "{name} collision flag"
        );
    }
}

#[test]
fn unknown_state_ids_fall_back_to_a_full_cube() {
    for state_id in [-1, 29_671, 1_000_000] {
        assert_boxes(block_shapes::collision_boxes(state_id), &[Aabb::FULL]);
        assert_boxes(block_shapes::outline_boxes(state_id), &[Aabb::FULL]);
        assert!(block_shapes::has_collision(state_id));
    }
}

/// A world made of a ground plane plus one bottom slab at `(2, 0, 0)`.
struct SlabWorld {
    slab_id: i32,
}

impl PhysicsWorld for SlabWorld {
    fn collision_box(&mut self, x: i32, y: i32, z: i32) -> Option<Aabb> {
        self.collision_boxes(x, y, z).into_iter().next()
    }

    fn collision_boxes(&mut self, x: i32, y: i32, z: i32) -> Vec<Aabb> {
        if y < 0 {
            return vec![Aabb::block(x, y, z)];
        }
        if (x, y, z) == (2, 0, 0) {
            return block_shapes::collision_boxes(self.slab_id)
                .iter()
                .map(|bounds| bounds.offset(f64::from(x), f64::from(y), f64::from(z)))
                .collect();
        }
        Vec::new()
    }
}

#[test]
fn player_steps_onto_and_off_a_bottom_slab() {
    assert_eq!(STEP_HEIGHT, 0.6);
    let mut world = SlabWorld {
        slab_id: id(
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        ),
    };
    assert_boxes(
        block_shapes::collision_boxes(world.slab_id),
        &[Aabb::new(0.0, 0.0, 0.0, 1.0, 0.5, 1.0)],
    );

    let mut physics = PlayerPhysics::new(0.5, 0.0, 0.5);
    physics.on_ground = true;
    physics.yaw = -90.0; // face east (+X)

    let mut stepped_on = false;
    for _ in 0..80 {
        physics.tick(&mut world, MovementInput::walking_forward());
        if physics.y > 0.4 {
            stepped_on = true;
            break;
        }
    }
    assert!(stepped_on, "player did not step onto the slab");
    assert!(
        (physics.y - 0.5).abs() < 1.0e-9,
        "feet on the slab top, y={}",
        physics.y
    );
    assert!(physics.on_ground);
    assert!(
        physics.x > 1.69 && physics.x < 3.3,
        "player is over the slab, x={}",
        physics.x
    );

    let mut stepped_off = false;
    for _ in 0..120 {
        physics.tick(&mut world, MovementInput::walking_forward());
        if physics.x > 3.3 && physics.y.abs() < 1.0e-9 {
            stepped_off = true;
            break;
        }
    }
    assert!(stepped_off, "player did not step off the slab");
    assert!(physics.on_ground);
    assert_eq!(physics.y, 0.0);
}

struct GroundAndSlabCollider;

impl BlockCollider for GroundAndSlabCollider {
    fn is_solid(&mut self, _x: i32, y: i32, _z: i32) -> bool {
        y <= 0
    }

    fn collision_boxes(&mut self, x: i32, y: i32, z: i32) -> Vec<Aabb> {
        if (x, y, z) == (0, 0, 0) {
            return vec![Aabb::new(0.0, 0.0, 0.0, 1.0, 0.5, 1.0)];
        }
        if y < 0 {
            return vec![Aabb::block(x, y, z)];
        }
        Vec::new()
    }
}

#[test]
fn entity_physics_lands_on_a_slab_shape() {
    let mut entity = Entity::new(42, [0; 16], 5, 0.5, 2.0, 0.5);
    let mut collider = GroundAndSlabCollider;
    for _ in 0..200 {
        entity.tick_physics(&mut collider);
    }
    assert!(
        (entity.y - 0.5).abs() < 1.0e-9,
        "armor stand rests on the slab, y={}",
        entity.y
    );
    assert!(entity.on_ground);
    assert_eq!(entity.velocity[1], 0.0);
}
