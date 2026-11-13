//! Item entities and XP orbs spawned from block drops.
//!
//! Ports the behavior of:
//!
//! * `net.minecraft.world.entity.item.ItemEntity` — gravity `0.04`, drag
//!   `0.98`, ground bounce `-0.5`, pickup delay `10`, lifetime `6000`,
//!   stack merge for same item + components.
//! * `net.minecraft.world.entity.ExperienceOrb` — gravity `0.03`, drag `0.98`,
//!   `8`-block attraction, `getExperienceValue` splitting, merge within
//!   `0.5` blocks.
//!
//! The model is deliberately session-free: entities are values that tick
//! against a [`BlockCollider`] and an optional player position, and collect
//! through the [`PickupTarget`] trait. Live `join.rs` wiring can implement
//! that trait for a player session without this module touching sessions.
//!
//! ## Deliberate simplifications (documented in `DECISIONS.md`)
//!
//! * Item magnet: vanilla `ItemEntity` has no in-tick magnet; pickup uses
//!   the player box inflated by `1.0, 0.5, 1.0`. This slice adds a `4`-block
//!   item magnet (pickup radius `1.5`) so drops drift to the player as the
//!   task requires. XP attraction follows Java's `8`-block formula.
//! * Item merge radius is `3.0` blocks per the task (vanilla uses the
//!   spigot `itemMerge` radius, default `1.0`). Merge requires same item +
//!   components and a combined count within the destination max stack.
//! * XP merge requires equal values and `<= 0.5` distance. Paper's
//!   `xpOrbGroupsPerArea` modulo (`(orb.id - amount) % 40 == 0`) is not
//!   modeled; the scan still runs every 20 ticks.
//! * Fluids, fire/lava damage, `noCollision` ejection, and the Paper despawn
//!   hooks are not modeled. Despawn is a plain `age >= 6000` check.

use crate::block_breaking::BlockDrops;
use crate::block_shapes::Aabb;
use crate::entity::{
    BlockCollider, Entity, add_entity_packet, entity_position_sync_packet, remove_entities_packet,
};
use crate::entity_metadata::{EntityMetadata, MetadataValue};
use crate::inventory::{ItemStack, add_stack};
use crate::math::{BlockPos, Vec3};
use crate::random::RandomSource;
use std::sync::atomic::{AtomicI32, Ordering};

/// Vanilla `ItemEntity.DATA_ITEM` index (first accessor after the base `Entity`
/// indices `0..8`).
pub const DATA_ITEM: u8 = 8;
/// Vanilla `ExperienceOrb.DATA_VALUE` index (same slot, different serializer).
pub const DATA_VALUE: u8 = 8;

/// Vanilla `ItemEntity` pickup delay for block drops.
pub const ITEM_PICKUP_DELAY: i32 = 10;
/// Vanilla `ItemEntity` lifetime in ticks.
pub const ITEM_LIFETIME: i32 = 6000;
/// Vanilla `ItemEntity.getDefaultGravity`.
pub const ITEM_GRAVITY: f64 = 0.04;
/// Item drag applied after the move (vanilla `0.98`).
pub const ITEM_DRAG: f64 = 0.98;
/// Item merge radius in blocks (task value; vanilla default is `1.0`).
pub const ITEM_MERGE_DISTANCE: f64 = 3.0;
/// Item magnet range in blocks. Pickup only happens inside
/// [`ITEM_PICKUP_DISTANCE`], but the magnet drifts the entity inward from
/// here. This has no vanilla counterpart; see the module docs.
pub const ITEM_MAGNET_DISTANCE: f64 = 4.0;
/// Distance at which an item entity is collected by a [`PickupTarget`].
pub const ITEM_PICKUP_DISTANCE: f64 = 1.5;
/// Item entity width/height (`EntityType.ITEM` sized `0.25 x 0.25`).
pub const ITEM_WIDTH: f64 = 0.25;
pub const ITEM_HEIGHT: f64 = 0.25;

/// Vanilla `ExperienceOrb` lifetime in ticks.
pub const XP_LIFETIME: i32 = 6000;
/// Vanilla `ExperienceOrb.getDefaultGravity`.
pub const XP_GRAVITY: f64 = 0.03;
/// Vanilla `ExperienceOrb.MAX_FOLLOW_DIST`.
pub const XP_FOLLOW_DISTANCE: f64 = 8.0;
/// Vanilla `ExperienceOrb.ORB_MERGE_DISTANCE`.
pub const XP_MERGE_DISTANCE: f64 = 0.5;
/// Vanilla `ExperienceOrb.ENTITY_SCAN_PERIOD`.
pub const XP_SCAN_PERIOD: i64 = 20;
/// Distance at which an XP orb is collected by a [`PickupTarget`].
pub const XP_PICKUP_DISTANCE: f64 = 1.5;
/// XP orb width/height (`EntityType.EXPERIENCE_ORB` sized `0.5 x 0.5`).
pub const XP_WIDTH: f64 = 0.5;
pub const XP_HEIGHT: f64 = 0.5;

/// Vanilla `ItemEntity` health.
pub const ITEM_HEALTH: i32 = 5;
/// Vanilla `ExperienceOrb` health.
pub const XP_HEALTH: i32 = 5;

const EPSILON: f64 = 1.0e-7;

/// Protocol entity-type id for `minecraft:item`.
pub fn item_entity_type_id() -> i32 {
    crate::registry::entity_type_id("minecraft:item").unwrap_or(71)
}

/// Protocol entity-type id for `minecraft:experience_orb`.
pub fn experience_orb_type_id() -> i32 {
    crate::registry::entity_type_id("minecraft:experience_orb").unwrap_or(49)
}

static NEXT_DROP_ENTITY_ID: AtomicI32 = AtomicI32::new(20_000);

fn next_drop_id() -> i32 {
    NEXT_DROP_ENTITY_ID.fetch_add(1, Ordering::Relaxed)
}

fn drop_uuid(id: i32, salt: u8) -> [u8; 16] {
    let mut uuid = [0u8; 16];
    uuid[0..4].copy_from_slice(&id.to_be_bytes());
    uuid[4] = 0x1e;
    uuid[5] = salt;
    uuid[15] = 0x7f;
    uuid
}

/// Collects items and XP by proximity. Implemented by player sessions (and
/// by tests) so this module never needs to know about sessions.
///
/// `add_item` returns the part of the stack that did not fit, exactly like
/// [`crate::inventory::add_stack`].
pub trait PickupTarget {
    /// Feet position used for distance checks and magnet targeting.
    fn position(&self) -> Vec3;
    fn add_item(&mut self, stack: ItemStack) -> ItemStack;
    fn add_experience(&mut self, amount: i32);
    fn can_pickup_items(&self) -> bool {
        true
    }
    fn can_pickup_experience(&self) -> bool {
        true
    }
}

/// A dropped item entity.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemEntity {
    pub id: i32,
    pub uuid: [u8; 16],
    pub stack: ItemStack,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub velocity: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub age: i32,
    pub pickup_delay: i32,
    pub health: i32,
    pub bob_offset: f32,
}

impl ItemEntity {
    /// Deterministic constructor without RNG draws (used by tests).
    pub fn new_with(id: i32, stack: ItemStack, x: f64, y: f64, z: f64, velocity: [f64; 3]) -> Self {
        Self {
            id,
            uuid: drop_uuid(id, 0x11),
            stack,
            x,
            y,
            z,
            velocity,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            age: 0,
            pickup_delay: ITEM_PICKUP_DELAY,
            health: ITEM_HEALTH,
            bob_offset: 0.0,
        }
    }

    /// Spawn one item at the block centre with Java's initial velocity:
    /// `x/z = nextDouble * 0.2 - 0.1`, `y = 0.2`, `pickup_delay = 10`.
    ///
    /// RNG order matches Java: `bobOffset` (`nextFloat`), yaw (`nextFloat`),
    /// then the two `nextDouble` velocity draws.
    pub fn spawn_one(pos: BlockPos, stack: ItemStack, rng: &mut dyn RandomSource) -> Self {
        let id = next_drop_id();
        let bob_offset = rng.next_float() * std::f32::consts::TAU;
        let yaw = rng.next_float() * 360.0;
        let vx = rng.next_double() * 0.2 - 0.1;
        let vz = rng.next_double() * 0.2 - 0.1;
        Self {
            id,
            uuid: drop_uuid(id, 0x11),
            stack,
            x: f64::from(pos.x()) + 0.5,
            y: f64::from(pos.y()) + 0.5,
            z: f64::from(pos.z()) + 0.5,
            velocity: [vx, 0.2, vz],
            yaw,
            pitch: 0.0,
            on_ground: false,
            age: 0,
            pickup_delay: ITEM_PICKUP_DELAY,
            health: ITEM_HEALTH,
            bob_offset,
        }
    }

    pub fn position(&self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }

    pub fn set_velocity(&mut self, x: f64, y: f64, z: f64) {
        self.velocity = [x, y, z];
    }

    pub fn entity_type_id(&self) -> i32 {
        item_entity_type_id()
    }

    /// Whether this entity can absorb or be absorbed by a merge.
    pub fn is_mergable(&self) -> bool {
        !self.stack.is_empty()
            && self.pickup_delay != 32_767
            && self.age >= 0
            && self.age < ITEM_LIFETIME
            && self.stack.count < self.stack.max_stack_size()
    }

    /// Whether the entity has reached the `6000`-tick lifetime.
    pub fn should_despawn(&self) -> bool {
        self.age >= ITEM_LIFETIME
    }

    /// One physics tick: magnet (when `player` is within
    /// [`ITEM_MAGNET_DISTANCE`]), gravity, swept move, drag, ground bounce,
    /// delay/age bookkeeping. Returns `true` when the position changed.
    pub fn tick_physics(
        &mut self,
        collider: &mut impl BlockCollider,
        player: Option<Vec3>,
    ) -> bool {
        let start = (self.x, self.y, self.z);
        let position = self.position();
        if let Some(player) = player {
            apply_item_magnet(&mut self.velocity, position, player);
        }
        // Vanilla `applyGravity`: vy -= 0.04, then move, then drag, then
        // ground bounce `vy *= -0.5` when on the ground and moving down.
        self.velocity[1] -= ITEM_GRAVITY;
        let mut pos = [self.x, self.y, self.z];
        move_axis(
            &mut pos,
            &mut self.velocity,
            ITEM_WIDTH,
            ITEM_HEIGHT,
            0,
            collider,
        );
        move_axis(
            &mut pos,
            &mut self.velocity,
            ITEM_WIDTH,
            ITEM_HEIGHT,
            1,
            collider,
        );
        let hit_ground = self.velocity[1] == 0.0 && pos[1] <= start.1 + f64::EPSILON;
        move_axis(
            &mut pos,
            &mut self.velocity,
            ITEM_WIDTH,
            ITEM_HEIGHT,
            2,
            collider,
        );
        self.x = pos[0];
        self.y = pos[1];
        self.z = pos[2];
        self.velocity[0] *= ITEM_DRAG;
        self.velocity[1] *= ITEM_DRAG;
        self.velocity[2] *= ITEM_DRAG;
        self.on_ground = hit_ground;
        if self.on_ground && self.velocity[1] < 0.0 {
            self.velocity[1] *= -0.5;
        }
        if self.pickup_delay > 0 && self.pickup_delay != 32_767 {
            self.pickup_delay -= 1;
        }
        if self.age >= 0 {
            self.age += 1;
        }
        (self.x, self.y, self.z) != start
    }

    /// Collect this entity into `target` when the pickup delay has elapsed
    /// and the player is within [`ITEM_PICKUP_DISTANCE`]. Returns `true`
    /// when the entity is fully collected and should be removed.
    pub fn try_pickup(&mut self, target: &mut dyn PickupTarget) -> bool {
        if self.pickup_delay > 0 || !target.can_pickup_items() || self.stack.is_empty() {
            return false;
        }
        if self.position().distance_to(target.position()) > ITEM_PICKUP_DISTANCE {
            return false;
        }
        let leftover = target.add_item(self.stack.clone());
        if leftover.is_empty() {
            self.stack = ItemStack::empty();
            true
        } else if leftover.count < self.stack.count {
            // Partial pickup: keep the remainder and report the new stack.
            self.stack = leftover;
            false
        } else {
            false
        }
    }

    fn base_entity(&self) -> Entity {
        let mut entity = Entity::new(
            self.id,
            self.uuid,
            self.entity_type_id(),
            self.x,
            self.y,
            self.z,
        );
        entity.yaw = self.yaw;
        entity.pitch = self.pitch;
        entity.velocity = self.velocity;
        entity.on_ground = self.on_ground;
        entity.width = ITEM_WIDTH;
        entity.height = ITEM_HEIGHT;
        entity
    }

    /// `ClientboundAddEntity` for `minecraft:item`.
    pub fn add_packet(&self) -> Vec<u8> {
        add_entity_packet(&self.base_entity())
    }

    /// Full `ClientboundSetEntityData` with the base indices and the item
    /// stack at [`DATA_ITEM`].
    pub fn metadata_packet(&self) -> Vec<u8> {
        let mut metadata = EntityMetadata::new(self.id);
        let _ = metadata.set(DATA_ITEM, MetadataValue::ItemStack(self.stack.clone()));
        metadata.all_packet()
    }

    /// Position sync sent when the entity moves.
    pub fn position_sync_packet(&self) -> Vec<u8> {
        entity_position_sync_packet(&self.base_entity())
    }
}

/// A dropped experience orb.
#[derive(Debug, Clone, PartialEq)]
pub struct ExperienceOrb {
    pub id: i32,
    pub uuid: [u8; 16],
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub velocity: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub value: i32,
    pub age: i32,
    pub count: i32,
    pub health: i32,
    pub tick_count: i64,
}

impl ExperienceOrb {
    /// Deterministic constructor without RNG draws (used by tests).
    pub fn new_with(id: i32, x: f64, y: f64, z: f64, velocity: [f64; 3], value: i32) -> Self {
        Self {
            id,
            uuid: drop_uuid(id, 0x22),
            x,
            y,
            z,
            velocity,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            value: value.max(0),
            age: 0,
            count: 1,
            health: XP_HEALTH,
            tick_count: 0,
        }
    }

    /// Spawn one orb with Java's initial velocity: yaw (`nextFloat`), then
    /// `(nextDouble * 0.2 - 0.1) * 2` on x/z and `nextDouble * 0.2 * 2` on y.
    pub fn spawn_one(pos: BlockPos, value: i32, rng: &mut dyn RandomSource) -> Self {
        let id = next_drop_id();
        let yaw = rng.next_float() * 360.0;
        let vx = (rng.next_double() * 0.2 - 0.1) * 2.0;
        let vy = rng.next_double() * 0.2 * 2.0;
        let vz = (rng.next_double() * 0.2 - 0.1) * 2.0;
        Self {
            id,
            uuid: drop_uuid(id, 0x22),
            x: f64::from(pos.x()) + 0.5,
            y: f64::from(pos.y()) + 0.5,
            z: f64::from(pos.z()) + 0.5,
            velocity: [vx, vy, vz],
            yaw,
            pitch: 0.0,
            on_ground: false,
            value: value.max(0),
            age: 0,
            count: 1,
            health: XP_HEALTH,
            tick_count: 0,
        }
    }

    pub fn position(&self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }

    pub fn entity_type_id(&self) -> i32 {
        experience_orb_type_id()
    }

    pub fn should_despawn(&self) -> bool {
        self.age >= XP_LIFETIME
    }

    /// One physics tick. `player` is the feet position of the nearest
    /// player; attraction follows Java's formula within
    /// [`XP_FOLLOW_DISTANCE`]. Returns `true` when the position changed.
    pub fn tick_physics(
        &mut self,
        collider: &mut impl BlockCollider,
        player: Option<Vec3>,
    ) -> bool {
        self.tick_count += 1;
        let start = (self.x, self.y, self.z);
        let position = self.position();
        if let Some(player) = player {
            apply_xp_attraction(&mut self.velocity, position, player);
        }
        let pre_y = self.velocity[1];
        self.velocity[1] -= XP_GRAVITY;
        let mut pos = [self.x, self.y, self.z];
        move_axis(
            &mut pos,
            &mut self.velocity,
            XP_WIDTH,
            XP_HEIGHT,
            0,
            collider,
        );
        move_axis(
            &mut pos,
            &mut self.velocity,
            XP_WIDTH,
            XP_HEIGHT,
            1,
            collider,
        );
        let vertical_hit = self.velocity[1] == 0.0 && pre_y < -XP_GRAVITY;
        move_axis(
            &mut pos,
            &mut self.velocity,
            XP_WIDTH,
            XP_HEIGHT,
            2,
            collider,
        );
        self.x = pos[0];
        self.y = pos[1];
        self.z = pos[2];
        self.velocity[0] *= XP_DRAG;
        self.velocity[1] *= XP_DRAG;
        self.velocity[2] *= XP_DRAG;
        self.on_ground = self.velocity[1] == 0.0 && self.y <= start.1 + f64::EPSILON;
        if vertical_hit && pre_y < -XP_GRAVITY {
            // Java: `if verticalCollisionBelow && d < -gravity { vy = -d * 0.4 }`
            // where `d` is the pre-move vertical velocity.
            self.velocity[1] = -pre_y * 0.4;
        }
        self.age += 1;
        (self.x, self.y, self.z) != start
    }

    /// Collect this orb into `target` when within [`XP_PICKUP_DISTANCE`].
    /// Returns `true` when the orb count reached zero and it should be
    /// removed. A merged orb (count > 1) needs one touch per count.
    pub fn try_pickup(&mut self, target: &mut dyn PickupTarget) -> bool {
        if !target.can_pickup_experience() || self.value <= 0 && self.count <= 0 {
            return false;
        }
        if self.position().distance_to(target.position()) > XP_PICKUP_DISTANCE {
            return false;
        }
        target.add_experience(self.value);
        self.count -= 1;
        self.count <= 0
    }

    fn base_entity(&self) -> Entity {
        let mut entity = Entity::new(
            self.id,
            self.uuid,
            self.entity_type_id(),
            self.x,
            self.y,
            self.z,
        );
        entity.yaw = self.yaw;
        entity.pitch = self.pitch;
        entity.velocity = self.velocity;
        entity.on_ground = self.on_ground;
        entity.width = XP_WIDTH;
        entity.height = XP_HEIGHT;
        entity
    }

    /// `ClientboundAddEntity` for `minecraft:experience_orb`.
    pub fn add_packet(&self) -> Vec<u8> {
        add_entity_packet(&self.base_entity())
    }

    /// Full `ClientboundSetEntityData` with the base indices and the value
    /// at [`DATA_VALUE`].
    pub fn metadata_packet(&self) -> Vec<u8> {
        let mut metadata = EntityMetadata::new(self.id);
        let _ = metadata.set(DATA_VALUE, MetadataValue::Int(self.value));
        metadata.all_packet()
    }

    pub fn position_sync_packet(&self) -> Vec<u8> {
        entity_position_sync_packet(&self.base_entity())
    }
}

const XP_DRAG: f64 = 0.98;

/// One spawned drop entity.
#[derive(Debug, Clone, PartialEq)]
pub enum DroppedEntity {
    Item(ItemEntity),
    ExperienceOrb(ExperienceOrb),
}

impl DroppedEntity {
    pub fn id(&self) -> i32 {
        match self {
            Self::Item(entity) => entity.id,
            Self::ExperienceOrb(orb) => orb.id,
        }
    }

    pub fn is_item(&self) -> bool {
        matches!(self, Self::Item(_))
    }

    pub fn is_experience_orb(&self) -> bool {
        matches!(self, Self::ExperienceOrb(_))
    }

    pub fn entity_type_id(&self) -> i32 {
        match self {
            Self::Item(_) => item_entity_type_id(),
            Self::ExperienceOrb(_) => experience_orb_type_id(),
        }
    }

    pub fn add_packet(&self) -> Vec<u8> {
        match self {
            Self::Item(entity) => entity.add_packet(),
            Self::ExperienceOrb(orb) => orb.add_packet(),
        }
    }

    pub fn metadata_packet(&self) -> Vec<u8> {
        match self {
            Self::Item(entity) => entity.metadata_packet(),
            Self::ExperienceOrb(orb) => orb.metadata_packet(),
        }
    }

    pub fn position_sync_packet(&self) -> Vec<u8> {
        match self {
            Self::Item(entity) => entity.position_sync_packet(),
            Self::ExperienceOrb(orb) => orb.position_sync_packet(),
        }
    }
}

/// `ClientboundRemoveEntities` for pickup/despawn/merge removals.
pub fn remove_packet(ids: &[i32]) -> Vec<u8> {
    remove_entities_packet(ids)
}

/// Java `ExperienceOrb.getExperienceValue`: the largest orb value that does
/// not exceed the remaining amount, using the vanilla threshold table.
pub fn get_experience_value(exp_value: i32) -> i32 {
    if exp_value > 162_670_129 {
        exp_value - 100_000
    } else if exp_value > 81_335_063 {
        81_335_063
    } else if exp_value > 40_667_527 {
        40_667_527
    } else if exp_value > 20_333_759 {
        20_333_759
    } else if exp_value > 10_166_857 {
        10_166_857
    } else if exp_value > 5_083_423 {
        5_083_423
    } else if exp_value > 2_541_701 {
        2_541_701
    } else if exp_value > 1_270_849 {
        1_270_849
    } else if exp_value > 635_413 {
        635_413
    } else if exp_value > 317_701 {
        317_701
    } else if exp_value > 158_849 {
        158_849
    } else if exp_value > 79_423 {
        79_423
    } else if exp_value > 39_709 {
        39_709
    } else if exp_value > 19_853 {
        19_853
    } else if exp_value > 9_923 {
        9_923
    } else if exp_value > 4_957 {
        4_957
    } else if exp_value >= 2_477 {
        2_477
    } else if exp_value >= 1_237 {
        1_237
    } else if exp_value >= 617 {
        617
    } else if exp_value >= 307 {
        307
    } else if exp_value >= 149 {
        149
    } else if exp_value >= 73 {
        73
    } else if exp_value >= 37 {
        37
    } else if exp_value >= 17 {
        17
    } else if exp_value >= 7 {
        7
    } else if exp_value >= 3 {
        3
    } else {
        1
    }
}

/// Split an XP amount into orb values, largest first, summing to `amount`.
pub fn split_experience(amount: i32) -> Vec<i32> {
    let mut remaining = amount;
    let mut orbs = Vec::new();
    while remaining > 0 {
        let value = get_experience_value(remaining);
        orbs.push(value);
        remaining -= value;
    }
    orbs
}

/// Spawn entities for a block-drop roll at the block centre.
///
/// Item stacks become [`ItemEntity`] values in roll order with Java's
/// initial velocity and a `10`-tick pickup delay. The XP amount is split
/// with [`split_experience`] into [`ExperienceOrb`] values. The caller owns
/// the returned entities and sends their add/metadata packets.
pub fn spawn_drops_at(
    pos: BlockPos,
    drops: &BlockDrops,
    rng: &mut dyn RandomSource,
) -> Vec<DroppedEntity> {
    let mut spawned = Vec::new();
    for stack in &drops.items {
        if stack.is_empty() {
            continue;
        }
        spawned.push(DroppedEntity::Item(ItemEntity::spawn_one(
            pos,
            stack.clone(),
            rng,
        )));
    }
    for value in split_experience(drops.experience) {
        spawned.push(DroppedEntity::ExperienceOrb(ExperienceOrb::spawn_one(
            pos, value, rng,
        )));
    }
    spawned
}

/// Spawn XP orbs for an explicit amount (no items).
pub fn spawn_experience_at(
    pos: BlockPos,
    amount: i32,
    rng: &mut dyn RandomSource,
) -> Vec<ExperienceOrb> {
    split_experience(amount)
        .into_iter()
        .map(|value| ExperienceOrb::spawn_one(pos, value, rng))
        .collect()
}

/// Whether two stacks can merge: same item + components and the combined
/// count fits the destination max stack (vanilla `areMergable`).
pub fn are_mergable(destination: &ItemStack, origin: &ItemStack) -> bool {
    if destination.is_empty() || origin.is_empty() {
        return false;
    }
    origin.count + destination.count <= origin.max_stack_size()
        && destination.same_item_and_components(origin)
}

/// Merge all pairs within [`ITEM_MERGE_DISTANCE`]. The larger stack absorbs
/// the smaller; ties keep the earlier entity as the destination, matching
/// Java's `tryToMerge`. Returns removed entity ids in removal order.
///
/// The merged entity keeps `max(pickup_delay)` and `min(age)`, also Java's
/// merge rules. Deterministic for a fixed input order.
pub fn merge_item_entities(entities: &mut Vec<ItemEntity>) -> Vec<i32> {
    let mut removed = Vec::new();
    let mut absorbed = vec![false; entities.len()];
    for i in 0..entities.len() {
        if absorbed[i] || !entities[i].is_mergable() {
            continue;
        }
        for j in (i + 1)..entities.len() {
            if absorbed[j] || !entities[j].is_mergable() {
                continue;
            }
            if entities[i].position().distance_to(entities[j].position()) > ITEM_MERGE_DISTANCE {
                continue;
            }
            // Decide destination: larger count wins; ties keep `i`.
            let (dest, origin) = if entities[j].stack.count > entities[i].stack.count {
                (j, i)
            } else {
                (i, j)
            };
            if !are_mergable(&entities[dest].stack, &entities[origin].stack) {
                continue;
            }
            let origin_count = entities[origin].stack.count;
            let origin_delay = entities[origin].pickup_delay;
            let origin_age = entities[origin].age;
            entities[dest].stack.count += origin_count;
            entities[dest].pickup_delay = entities[dest].pickup_delay.max(origin_delay);
            entities[dest].age = entities[dest].age.min(origin_age);
            entities[origin].stack = ItemStack::empty();
            absorbed[origin] = true;
            removed.push(entities[origin].id);
        }
    }
    // Retain in order; indices in `removed` already collected.
    let mut index = 0;
    entities.retain(|_| {
        let keep = !absorbed[index];
        index += 1;
        keep
    });
    removed
}

/// Merge XP orbs with equal values within [`XP_MERGE_DISTANCE`]. Returns
/// removed entity ids. Counts are summed and age takes the minimum, as in
/// Java `merge`.
pub fn merge_experience_orbs(orbs: &mut Vec<ExperienceOrb>) -> Vec<i32> {
    let mut removed = Vec::new();
    let mut absorbed = vec![false; orbs.len()];
    for i in 0..orbs.len() {
        if absorbed[i] {
            continue;
        }
        for j in (i + 1)..orbs.len() {
            if absorbed[j] {
                continue;
            }
            if orbs[i].value != orbs[j].value {
                continue;
            }
            if orbs[i].position().distance_to(orbs[j].position()) > XP_MERGE_DISTANCE {
                continue;
            }
            orbs[i].count += orbs[j].count;
            orbs[i].age = orbs[i].age.min(orbs[j].age);
            absorbed[j] = true;
            removed.push(orbs[j].id);
        }
    }
    let mut index = 0;
    orbs.retain(|_| {
        let keep = !absorbed[index];
        index += 1;
        keep
    });
    removed
}

fn apply_item_magnet(velocity: &mut [f64; 3], pos: Vec3, player: Vec3) {
    let target = Vec3::new(player.x, player.y + 0.81, player.z);
    let delta = target.subtract(pos);
    let dist = delta.length();
    if !(1.0e-4..=ITEM_MAGNET_DISTANCE).contains(&dist) {
        return;
    }
    let factor = 1.0 - dist / ITEM_MAGNET_DISTANCE;
    let strength = factor * factor * 0.2;
    let pull = delta.normalize().scale(strength);
    velocity[0] += pull.x;
    velocity[1] += pull.y;
    velocity[2] += pull.z;
}

fn apply_xp_attraction(velocity: &mut [f64; 3], pos: Vec3, player: Vec3) {
    let target = Vec3::new(player.x, player.y + 0.81, player.z);
    let delta = target.subtract(pos);
    let dist = delta.length();
    if !(1.0e-4..=XP_FOLLOW_DISTANCE).contains(&dist) {
        return;
    }
    // Java: d1 = 1 - sqrt(d) / 8; velocity += normalize(delta) * (d1*d1*0.1)
    // where `d` is the squared distance and 8 is MAX_FOLLOW_DIST.
    let factor = 1.0 - dist / XP_FOLLOW_DISTANCE;
    let strength = factor * factor * 0.1;
    let pull = delta.normalize().scale(strength);
    velocity[0] += pull.x;
    velocity[1] += pull.y;
    velocity[2] += pull.z;
}

/// Swept AABB move for one axis using the shared block-shape colliders.
/// Zeroes the velocity component on a blocking collision.
fn move_axis(
    pos: &mut [f64; 3],
    velocity: &mut [f64; 3],
    width: f64,
    height: f64,
    axis: u8,
    collider: &mut impl BlockCollider,
) {
    let amount = velocity[axis as usize];
    if amount == 0.0 {
        return;
    }
    let start = Aabb::player(pos[0], pos[1], pos[2], width, height);
    let sweep = match axis {
        0 => start.expand_towards(amount, 0.0, 0.0),
        1 => start.expand_towards(0.0, amount, 0.0),
        _ => start.expand_towards(0.0, 0.0, amount),
    };
    let x0 = (sweep.min_x - EPSILON).floor() as i32 - 1;
    let x1 = (sweep.max_x + EPSILON).floor() as i32 + 1;
    let y0 = (sweep.min_y - EPSILON).floor() as i32 - 1;
    let y1 = (sweep.max_y + EPSILON).floor() as i32 + 1;
    let z0 = (sweep.min_z - EPSILON).floor() as i32 - 1;
    let z1 = (sweep.max_z + EPSILON).floor() as i32 + 1;
    let mut allowed = amount;
    for bx in x0..=x1 {
        for by in y0..=y1 {
            for bz in z0..=z1 {
                for block in collider.collision_boxes(bx, by, bz) {
                    if !block.intersects(&sweep) {
                        continue;
                    }
                    match axis {
                        0 => {
                            if amount > 0.0
                                && start.max_x <= block.min_x + EPSILON
                                && start.max_x + amount > block.min_x
                            {
                                allowed = allowed.min(block.min_x - start.max_x);
                            } else if amount < 0.0
                                && start.min_x >= block.max_x - EPSILON
                                && start.min_x + amount < block.max_x
                            {
                                allowed = allowed.max(block.max_x - start.min_x);
                            }
                        }
                        1 => {
                            if amount > 0.0
                                && start.max_y <= block.min_y + EPSILON
                                && start.max_y + amount > block.min_y
                            {
                                allowed = allowed.min(block.min_y - start.max_y);
                            } else if amount < 0.0
                                && start.min_y >= block.max_y - EPSILON
                                && start.min_y + amount < block.max_y
                            {
                                allowed = allowed.max(block.max_y - start.min_y);
                            }
                        }
                        _ => {
                            if amount > 0.0
                                && start.max_z <= block.min_z + EPSILON
                                && start.max_z + amount > block.min_z
                            {
                                allowed = allowed.min(block.min_z - start.max_z);
                            } else if amount < 0.0
                                && start.min_z >= block.max_z - EPSILON
                                && start.min_z + amount < block.max_z
                            {
                                allowed = allowed.max(block.max_z - start.min_z);
                            }
                        }
                    }
                }
            }
        }
    }
    if allowed != amount {
        velocity[axis as usize] = 0.0;
    }
    pos[axis as usize] += allowed;
}

/// Pickup-target adapter over a `36`-slot inventory slice plus an XP pool.
/// Provided so tests (and later session code) do not re-implement the
/// [`crate::inventory::add_stack`] plumbing.
pub struct SlicePickupTarget<'a> {
    pub inventory: &'a mut [ItemStack],
    pub experience: &'a mut i32,
    pub position: Vec3,
}

impl PickupTarget for SlicePickupTarget<'_> {
    fn position(&self) -> Vec3 {
        self.position
    }

    fn add_item(&mut self, stack: ItemStack) -> ItemStack {
        add_stack(self.inventory, stack)
    }

    fn add_experience(&mut self, amount: i32) {
        *self.experience = self.experience.saturating_add(amount);
    }
}

/// Result of one [`DropEntityStore::tick`].
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DropStoreTick {
    pub packets: Vec<Vec<u8>>,
    pub removed: Vec<i32>,
    pub picked_items: bool,
    pub picked_experience: bool,
}

/// Session-free container for dropped item entities and XP orbs.
///
/// Entities tick against a [`BlockCollider`] and a [`PickupTarget`] without
/// any live session dependency.
#[derive(Debug, Default)]
pub struct DropEntityStore {
    entities: Vec<DroppedEntity>,
}

impl DropEntityStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    pub fn clear(&mut self) {
        self.entities.clear();
    }

    /// Insert an existing entity (used by tests and deterministic setup).
    pub fn insert(&mut self, entity: DroppedEntity) {
        self.entities.push(entity);
    }

    /// Spawn drop entities at `pos` and emit add+metadata packets in entity
    /// order.
    pub fn spawn(
        &mut self,
        pos: BlockPos,
        drops: &BlockDrops,
        rng: &mut dyn RandomSource,
    ) -> Vec<Vec<u8>> {
        let spawned = spawn_drops_at(pos, drops, rng);
        let mut packets = Vec::with_capacity(spawned.len() * 2);
        for entity in spawned {
            packets.push(entity.add_packet());
            packets.push(entity.metadata_packet());
            self.entities.push(entity);
        }
        packets
    }

    /// Tick all stored entities.
    ///
    /// Packet order is deterministic:
    /// 1. In original store order, each entity is processed once: if it is
    ///    picked up or despawns, a remove packet is emitted; otherwise a
    ///    position-sync packet is emitted when it moved, and an item metadata
    ///    packet is emitted when the stack changed without full pickup (partial
    ///    pickup).
    /// 2. After ticking, item entities are merged with [`merge_item_entities`]
    ///    in their original store order and remove packets are emitted for
    ///    absorbed ids in the merge's removal order. XP orbs are not merged.
    pub fn tick(
        &mut self,
        collider: &mut impl BlockCollider,
        target: &mut dyn PickupTarget,
    ) -> DropStoreTick {
        let mut out = DropStoreTick::default();
        let mut kept = Vec::with_capacity(self.entities.len());

        for mut entity in self.entities.drain(..) {
            match &mut entity {
                DroppedEntity::Item(item) => {
                    let before_stack = item.stack.clone();
                    let moved = item.tick_physics(collider, Some(target.position()));
                    if item.try_pickup(target) {
                        out.picked_items = true;
                        out.removed.push(item.id);
                        out.packets.push(remove_entities_packet(&[item.id]));
                        continue;
                    }
                    if item.should_despawn() {
                        out.removed.push(item.id);
                        out.packets.push(remove_entities_packet(&[item.id]));
                        continue;
                    }
                    if moved {
                        out.packets.push(item.position_sync_packet());
                    }
                    if item.stack != before_stack && !item.stack.is_empty() {
                        out.picked_items = true;
                        out.packets.push(item.metadata_packet());
                    }
                    kept.push(entity);
                }
                DroppedEntity::ExperienceOrb(orb) => {
                    let moved = orb.tick_physics(collider, Some(target.position()));
                    if orb.try_pickup(target) {
                        out.picked_experience = true;
                        out.removed.push(orb.id);
                        out.packets.push(remove_entities_packet(&[orb.id]));
                        continue;
                    }
                    if orb.should_despawn() {
                        out.removed.push(orb.id);
                        out.packets.push(remove_entities_packet(&[orb.id]));
                        continue;
                    }
                    if moved {
                        out.packets.push(orb.position_sync_packet());
                    }
                    kept.push(entity);
                }
            }
        }

        let mut items = Vec::new();
        for entity in &kept {
            if let DroppedEntity::Item(item) = entity {
                items.push(item.clone());
            }
        }
        let merged_removed = merge_item_entities(&mut items);
        for id in &merged_removed {
            out.removed.push(*id);
            out.packets.push(remove_entities_packet(&[*id]));
        }

        let absorbed: std::collections::HashSet<i32> = merged_removed.iter().copied().collect();
        items.retain(|item| !absorbed.contains(&item.id) && !item.stack.is_empty());

        let mut item_iter = items.into_iter();
        let mut rebuilt = Vec::with_capacity(kept.len().saturating_sub(merged_removed.len()));
        for entity in kept {
            match entity {
                DroppedEntity::Item(_) => {
                    if let Some(item) = item_iter.next() {
                        rebuilt.push(DroppedEntity::Item(item));
                    }
                }
                other => rebuilt.push(other),
            }
        }

        self.entities = rebuilt;
        out
    }

    /// Remove entities whose chunk column is not in `chunks`, emitting remove
    /// packets in ascending entity id order.
    pub fn retain_in_chunks(
        &mut self,
        chunks: &std::collections::HashSet<(i32, i32)>,
    ) -> Vec<Vec<u8>> {
        let mut removed_ids = Vec::new();
        self.entities.retain(|entity| {
            let (x, z) = match entity {
                DroppedEntity::Item(item) => (item.x, item.z),
                DroppedEntity::ExperienceOrb(orb) => (orb.x, orb.z),
            };
            let chunk = (
                (x.floor() as i32).div_euclid(16),
                (z.floor() as i32).div_euclid(16),
            );
            if chunks.contains(&chunk) {
                true
            } else {
                removed_ids.push(entity.id());
                false
            }
        });
        removed_ids.sort_unstable();
        removed_ids
            .into_iter()
            .map(|id| remove_entities_packet(&[id]))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::FlatCollider;
    use crate::inventory::ItemStack;
    use folia_random::LegacyRandomSource;

    #[test]
    fn item_spawn_velocity_matches_java_randoms() {
        let pos = BlockPos::new(10, 64, -4);
        let stack = ItemStack::new("minecraft:stone", 1);
        let mut a = LegacyRandomSource::new(42);
        let mut b = LegacyRandomSource::new(42);
        let item_a = ItemEntity::spawn_one(pos, stack.clone(), &mut a);
        let item_b = ItemEntity::spawn_one(pos, stack, &mut b);
        assert_eq!(item_a.velocity, item_b.velocity);
        assert_eq!(item_a.pickup_delay, ITEM_PICKUP_DELAY);
        assert_eq!(item_a.age, 0);
        assert_eq!((item_a.x, item_a.y, item_a.z), (10.5, 64.5, -3.5));
        assert!(item_a.velocity[1] == 0.2);
        assert!((0.0..0.2).contains(&item_a.velocity[0].abs().max(item_a.velocity[2].abs())));
    }

    #[test]
    fn xp_split_thresholds_match_java() {
        assert_eq!(get_experience_value(1), 1);
        assert_eq!(get_experience_value(2), 1);
        assert_eq!(get_experience_value(3), 3);
        assert_eq!(get_experience_value(6), 3);
        assert_eq!(get_experience_value(7), 7);
        assert_eq!(get_experience_value(37), 37);
        assert_eq!(get_experience_value(1_000), 617);
        assert_eq!(split_experience(0), Vec::<i32>::new());
        let amount: i32 = 500;
        assert_eq!(split_experience(amount).iter().sum::<i32>(), amount);
        let ones = split_experience(2);
        assert_eq!(ones, vec![1, 1]);
    }

    #[test]
    fn item_merge_requires_same_stack_and_fits() {
        let stone = ItemStack::new("minecraft:stone", 30);
        let stone_more = ItemStack::new("minecraft:stone", 30);
        let dirt = ItemStack::new("minecraft:dirt", 1);
        assert!(are_mergable(&stone, &stone_more));
        assert!(!are_mergable(&stone, &dirt));
        let full = ItemStack::new("minecraft:stone", 64);
        assert!(!are_mergable(&full, &stone));
    }

    #[test]
    fn merge_two_items_within_three_blocks() {
        let mut a = ItemEntity::new_with(
            1,
            ItemStack::new("minecraft:stone", 10),
            0.0,
            0.0,
            0.0,
            [0.0; 3],
        );
        let mut b = ItemEntity::new_with(
            2,
            ItemStack::new("minecraft:stone", 5),
            2.0,
            0.0,
            0.0,
            [0.0; 3],
        );
        a.pickup_delay = 0;
        b.pickup_delay = 0;
        let mut entities = vec![a, b];
        let removed = merge_item_entities(&mut entities);
        assert_eq!(removed.len(), 1);
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].stack.count, 15);
    }

    #[test]
    fn item_falls_and_lands() {
        let mut item = ItemEntity::new_with(
            7,
            ItemStack::new("minecraft:stone", 1),
            0.5,
            5.0,
            0.5,
            [0.0, 0.0, 0.0],
        );
        item.pickup_delay = 0;
        let mut collider = FlatCollider { ground_y: 0 };
        for _ in 0..300 {
            item.tick_physics(&mut collider, None);
        }
        assert!(item.y < 0.5);
        assert!(item.on_ground || item.velocity[1].abs() < 0.2);
    }

    #[test]
    fn xp_despawns_at_6000_ticks() {
        let mut orb = ExperienceOrb::new_with(9, 0.5, 64.5, 0.5, [0.0; 3], 3);
        orb.age = XP_LIFETIME - 1;
        let mut collider = FlatCollider { ground_y: -1_000 };
        orb.tick_physics(&mut collider, None);
        assert!(orb.should_despawn());
    }
}
