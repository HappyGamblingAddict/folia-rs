use crate::block_shapes::Aabb;
use crate::chunk::{BlockState, Chunk};
use crate::entity_ai::{ActorKind, AiMob, MemoryWorld};
use crate::entity_metadata::{Attribute, HurtResult, LivingEntity};
use crate::math::Vec3;
use crate::protocol::PacketWriter;
use crate::world::WorldStorage;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};

pub const CLIENTBOUND_ADD_ENTITY: i32 = 1;
pub const CLIENTBOUND_ENTITY_POSITION_SYNC: i32 = 35;
pub const CLIENTBOUND_REMOVE_ENTITIES: i32 = 75;

/// Protocol entity-type id for `minecraft:item` (`EntityType.ITEM`).
pub fn item_entity_type_id() -> i32 {
    crate::registry::entity_type_id("minecraft:item").unwrap_or(71)
}

/// Protocol entity-type id for `minecraft:experience_orb`
/// (`EntityType.EXPERIENCE_ORB`).
pub fn experience_orb_type_id() -> i32 {
    crate::registry::entity_type_id("minecraft:experience_orb").unwrap_or(49)
}

pub const GRAVITY: f64 = 0.08;
pub const DRAG: f64 = 0.98;
/// The fixed point of `(v - 0.08) * 0.98`; armor stands reach this terminal
/// fall speed after a long fall. Vanilla does not clamp explicitly.
pub const MAX_FALL_SPEED: f64 = -3.92;
const EPSILON: f64 = 1.0e-7;

#[derive(Debug, Clone, PartialEq)]
pub struct Entity {
    pub id: i32,
    pub uuid: [u8; 16],
    pub entity_type: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub velocity: [f64; 3],
    pub width: f64,
    pub height: f64,
}

impl Entity {
    pub fn new(id: i32, uuid: [u8; 16], entity_type: i32, x: f64, y: f64, z: f64) -> Self {
        Self {
            id,
            uuid,
            entity_type,
            x,
            y,
            z,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
            velocity: [0.0; 3],
            width: 0.6,
            height: 1.8,
        }
    }

    pub fn armor_stand(id: i32, uuid: [u8; 16], x: f64, y: f64, z: f64) -> Self {
        let entity_type = crate::registry::entity_type_id("minecraft:armor_stand").unwrap_or(5);
        let mut entity = Self::new(id, uuid, entity_type, x, y, z);
        entity.width = 0.5;
        entity.height = 1.975;
        entity
    }

    /// Base entity for a dropped `minecraft:item` (0.25 x 0.25).
    pub fn item(id: i32, uuid: [u8; 16], x: f64, y: f64, z: f64) -> Self {
        let mut entity = Self::new(id, uuid, item_entity_type_id(), x, y, z);
        entity.width = 0.25;
        entity.height = 0.25;
        entity
    }

    /// Base entity for a `minecraft:experience_orb` (0.5 x 0.5).
    pub fn experience_orb(id: i32, uuid: [u8; 16], x: f64, y: f64, z: f64) -> Self {
        let mut entity = Self::new(id, uuid, experience_orb_type_id(), x, y, z);
        entity.width = 0.5;
        entity.height = 0.5;
        entity
    }

    pub fn set_velocity(&mut self, x: f64, y: f64, z: f64) {
        self.velocity = [x, y, z];
    }

    /// Create the living-entity state (health, attributes, equipment,
    /// metadata) for this entity's id. The state is stored separately by id so
    /// physics and gameplay data stay independent.
    pub fn living(&self) -> crate::entity_metadata::LivingEntity {
        crate::entity_metadata::LivingEntity::new(self.id)
    }

    pub fn aabb(&self) -> (f64, f64, f64, f64, f64, f64) {
        let half = self.width * 0.5;
        (
            self.x - half,
            self.y,
            self.z - half,
            self.x + half,
            self.y + self.height,
            self.z + half,
        )
    }
}

pub trait BlockCollider {
    fn is_solid(&mut self, x: i32, y: i32, z: i32) -> bool;
    /// State at `(x, y, z)` when the collider is world-backed. The default
    /// returns `None`, and callers fall back to their own approximated view.
    fn block_at(&mut self, _x: i32, _y: i32, _z: i32) -> Option<BlockState> {
        None
    }
    /// Every collision box of the block at `(x, y, z)`.
    ///
    /// Simple test colliders only implement [`Self::is_solid`]; the default
    /// treats each solid block as a full cube. World-backed colliders override
    /// this with the generated per-state shape table.
    fn collision_boxes(&mut self, x: i32, y: i32, z: i32) -> Vec<Aabb> {
        if self.is_solid(x, y, z) {
            vec![Aabb::block(x, y, z)]
        } else {
            Vec::new()
        }
    }
}

pub struct FlatCollider {
    pub ground_y: i32,
}

impl BlockCollider for FlatCollider {
    fn is_solid(&mut self, _x: i32, y: i32, _z: i32) -> bool {
        y < self.ground_y
    }
}

#[derive(Debug, Clone, Copy)]
enum Axis {
    X,
    Y,
    Z,
}

impl Entity {
    /// One Minecraft-style physics tick: gravity and drag, axis-separated AABB
    /// collision, and ground detection. Returns true when the position changed.
    ///
    /// The Java armor-stand fall trace captured from Folia (see
    /// `tests/entity_physics.rs`) shows that vanilla applies gravity and drag
    /// before the move for living entities:
    /// `velocity = (velocity - gravity) * 0.98`, then `position += velocity`.
    pub fn tick_physics(&mut self, collider: &mut impl BlockCollider) -> bool {
        self.velocity[0] *= DRAG;
        self.velocity[1] = (self.velocity[1] - GRAVITY) * DRAG;
        self.velocity[2] *= DRAG;

        let start = (self.x, self.y, self.z);
        let delta = self.velocity;
        self.on_ground = false;
        self.move_axis(delta[0], Axis::X, collider);
        let falling = delta[1] < 0.0;
        self.move_axis(delta[1], Axis::Y, collider);
        if falling && self.velocity[1] == 0.0 && self.y <= start.1 + f64::EPSILON {
            self.on_ground = true;
        }
        self.move_axis(delta[2], Axis::Z, collider);

        (self.x, self.y, self.z) != start
    }

    fn move_axis(&mut self, amount: f64, axis: Axis, collider: &mut impl BlockCollider) {
        if amount == 0.0 {
            return;
        }
        let start = Aabb::player(self.x, self.y, self.z, self.width, self.height);
        let sweep = match axis {
            Axis::X => start.expand_towards(amount, 0.0, 0.0),
            Axis::Y => start.expand_towards(0.0, amount, 0.0),
            Axis::Z => start.expand_towards(0.0, 0.0, amount),
        };

        // Vanilla widens the block cursor by one block on each side so shapes
        // that extend past their block (fences and walls are 24/16 tall for
        // collision) are still found.
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
                            Axis::X => {
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
                            Axis::Y => {
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
                            Axis::Z => {
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
            match axis {
                Axis::X => self.velocity[0] = 0.0,
                Axis::Y => self.velocity[1] = 0.0,
                Axis::Z => self.velocity[2] = 0.0,
            }
        }
        match axis {
            Axis::X => self.x += allowed,
            Axis::Y => self.y += allowed,
            Axis::Z => self.z += allowed,
        }
    }
}

pub fn add_entity_packet(entity: &Entity) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(CLIENTBOUND_ADD_ENTITY).unwrap();
    writer.write_varint(entity.id).unwrap();
    writer.write_bytes(&entity.uuid);
    writer.write_varint(entity.entity_type).unwrap();
    writer.write_f64(entity.x).unwrap();
    writer.write_f64(entity.y).unwrap();
    writer.write_f64(entity.z).unwrap();
    writer.write_u8(0).unwrap(); // LpVec3 zero movement
    writer.write_u8(pack_degrees(entity.pitch)).unwrap();
    writer.write_u8(pack_degrees(entity.yaw)).unwrap();
    writer.write_u8(pack_degrees(entity.yaw)).unwrap();
    writer.write_varint(0).unwrap(); // entity data
    writer.into_inner()
}

pub fn remove_entities_packet(ids: &[i32]) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(CLIENTBOUND_REMOVE_ENTITIES).unwrap();
    writer.write_varint(ids.len() as i32).unwrap();
    for id in ids {
        writer.write_varint(*id).unwrap();
    }
    writer.into_inner()
}

pub fn entity_position_sync_packet(entity: &Entity) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer
        .write_varint(CLIENTBOUND_ENTITY_POSITION_SYNC)
        .unwrap();
    writer.write_varint(entity.id).unwrap();
    writer.write_f64(entity.x).unwrap();
    writer.write_f64(entity.y).unwrap();
    writer.write_f64(entity.z).unwrap();
    writer.write_f64(entity.velocity[0]).unwrap();
    writer.write_f64(entity.velocity[1]).unwrap();
    writer.write_f64(entity.velocity[2]).unwrap();
    writer.write_f32(entity.yaw).unwrap();
    writer.write_f32(entity.pitch).unwrap();
    writer.write_u8(entity.on_ground as u8).unwrap();
    writer.into_inner()
}

fn pack_degrees(degrees: f32) -> u8 {
    ((degrees * 256.0 / 360.0) as i32 & 0xFF) as u8
}

/// Session-local mobs are removed once this far from their viewer.
pub const SESSION_TRACKING_RANGE: f64 = 80.0;

const SESSION_MOB_SPAWNS: [SessionMobSpawn; 3] = [
    SessionMobSpawn {
        name: "minecraft:cow",
        dx: 3.0,
        dz: 2.0,
        max_health: 10.0,
    },
    SessionMobSpawn {
        name: "minecraft:pig",
        dx: -3.0,
        dz: 3.0,
        max_health: 10.0,
    },
    SessionMobSpawn {
        name: "minecraft:zombie",
        dx: 2.0,
        dz: -4.0,
        max_health: 20.0,
    },
];

struct SessionMobSpawn {
    name: &'static str,
    dx: f64,
    dz: f64,
    max_health: f32,
}

/// A session-owned living entity. The process-wide [`ENTITIES`] map is for
/// legacy broadcast entities; tracked mobs live here so each player sees only
/// their own deterministic population.
#[derive(Debug, Clone)]
pub struct TrackedEntity {
    pub entity: Entity,
    pub living: LivingEntity,
    /// AI environment state for mobs whose goals are usable with the current
    /// world model. Goal-driven movement remains a later P5.6 slice.
    pub ai: Option<AiMob>,
}

/// Packets produced by one session entity tick.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SessionEntityTick {
    pub packets: Vec<Vec<u8>>,
    pub removed: Vec<i32>,
}

/// Last position packet payload sent to one viewer for one entity.
#[derive(Debug, Clone, Copy, PartialEq)]
struct EntitySnapshot {
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    pitch: f32,
    on_ground: bool,
}

impl EntitySnapshot {
    fn of(entity: &Entity) -> Self {
        Self {
            x: entity.x,
            y: entity.y,
            z: entity.z,
            yaw: entity.yaw,
            pitch: entity.pitch,
            on_ground: entity.on_ground,
        }
    }

    fn matches(self, entity: &Entity) -> bool {
        self == Self::of(entity)
    }
}

/// One viewer's range set: ids currently visible and the last position synced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ViewerView {
    position: (f64, f64, f64),
    range: f64,
    tracked: BTreeSet<i32>,
    last_sent: BTreeMap<i32, EntitySnapshot>,
    pending: Vec<Vec<u8>>,
}

impl ViewerView {
    pub fn position(&self) -> (f64, f64, f64) {
        self.position
    }

    pub fn range(&self) -> f64 {
        self.range
    }

    /// Ids currently inside this viewer's range set, ascending.
    pub fn tracked_ids(&self) -> impl Iterator<Item = i32> + '_ {
        self.tracked.iter().copied()
    }

    pub fn tracks(&self, entity_id: i32) -> bool {
        self.tracked.contains(&entity_id)
    }
}

/// Shared entity tracking state keyed by network entity id.
///
/// Entity state lives here once; viewers keep only their range set and last
/// synced snapshot. `SessionEntityTracker` embeds this tracker as its entity
/// storage, so live sessions and multi-viewer tracking share the same records.
#[derive(Debug, Default)]
pub struct EntityTracker {
    entities: BTreeMap<i32, TrackedEntity>,
    viewers: BTreeMap<i32, ViewerView>,
}

impl EntityTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    pub fn insert(&mut self, tracked: TrackedEntity) -> Option<TrackedEntity> {
        self.entities.insert(tracked.entity.id, tracked)
    }

    /// Remove an entity. Every viewer that currently tracks it receives a
    /// remove-entities packet on its next [`Self::update_viewer`].
    pub fn remove_entity(&mut self, entity_id: i32) -> Option<TrackedEntity> {
        let removed = self.entities.remove(&entity_id)?;
        for viewer in self.viewers.values_mut() {
            if viewer.tracked.remove(&entity_id) {
                viewer.pending.push(remove_entities_packet(&[entity_id]));
            }
            viewer.last_sent.remove(&entity_id);
        }
        Some(removed)
    }

    pub fn get(&self, entity_id: i32) -> Option<&TrackedEntity> {
        self.entities.get(&entity_id)
    }

    pub fn get_mut(&mut self, entity_id: i32) -> Option<&mut TrackedEntity> {
        self.entities.get_mut(&entity_id)
    }

    pub fn contains(&self, entity_id: i32) -> bool {
        self.entities.contains_key(&entity_id)
    }

    /// Deterministic entity iteration (ascending network id).
    pub fn iter(&self) -> impl Iterator<Item = &TrackedEntity> + '_ {
        self.entities.values()
    }

    /// Deterministic entity ids (ascending network id).
    pub fn ids(&self) -> impl Iterator<Item = i32> + '_ {
        self.entities.keys().copied()
    }

    pub fn clear(&mut self) {
        self.entities.clear();
        self.viewers.clear();
    }

    pub fn viewer_count(&self) -> usize {
        self.viewers.len()
    }

    /// Deterministic viewer ids (ascending viewer id).
    pub fn viewer_ids(&self) -> impl Iterator<Item = i32> + '_ {
        self.viewers.keys().copied()
    }

    pub fn viewer(&self, viewer_id: i32) -> Option<&ViewerView> {
        self.viewers.get(&viewer_id)
    }

    pub fn add_viewer(&mut self, viewer_id: i32, position: (f64, f64, f64), range: f64) -> bool {
        self.viewers
            .insert(
                viewer_id,
                ViewerView {
                    position,
                    range: range.max(0.0),
                    ..ViewerView::default()
                },
            )
            .is_none()
    }

    pub fn remove_viewer(&mut self, viewer_id: i32) -> bool {
        self.viewers.remove(&viewer_id).is_some()
    }

    pub fn set_viewer_position(&mut self, viewer_id: i32, position: (f64, f64, f64)) -> bool {
        let Some(viewer) = self.viewers.get_mut(&viewer_id) else {
            return false;
        };
        if viewer.position == position {
            return false;
        }
        viewer.position = position;
        true
    }

    pub fn set_viewer_range(&mut self, viewer_id: i32, range: f64) -> bool {
        let Some(viewer) = self.viewers.get_mut(&viewer_id) else {
            return false;
        };
        let range = range.max(0.0);
        if viewer.range == range {
            return false;
        }
        viewer.range = range;
        true
    }

    pub fn is_tracked_by(&self, viewer_id: i32, entity_id: i32) -> bool {
        self.viewers
            .get(&viewer_id)
            .is_some_and(|viewer| viewer.tracks(entity_id))
    }

    /// Pairing packets for one entity: Add Entity, all metadata, equipment,
    /// then one `ClientboundUpdateMobEffect` per active effect.
    pub fn enter_packets(&self, entity_id: i32) -> Vec<Vec<u8>> {
        let Some(tracked) = self.entities.get(&entity_id) else {
            return Vec::new();
        };
        let mut packets = vec![
            add_entity_packet(&tracked.entity),
            tracked.living.all_metadata_packet(),
            tracked.living.equipment_packet(),
        ];
        packets.extend(tracked.living.active_effect_packets());
        packets
    }

    /// Recompute one viewer's range set. Returns, in order: pending removals,
    /// one batched removal for ids that left the range, pairing packets for
    /// entering ids (ascending), and position syncs for tracked ids whose
    /// position/yaw/pitch changed (ascending).
    pub fn update_viewer(&mut self, viewer_id: i32) -> Vec<Vec<u8>> {
        let Some(viewer) = self.viewers.get(&viewer_id) else {
            return Vec::new();
        };
        let position = viewer.position;
        let range = viewer.range;
        let range_sq = range * range;
        let visible: BTreeSet<i32> = self
            .entities
            .iter()
            .filter(|(_, tracked)| {
                let dx = tracked.entity.x - position.0;
                let dy = tracked.entity.y - position.1;
                let dz = tracked.entity.z - position.2;
                dx * dx + dy * dy + dz * dz <= range_sq
            })
            .map(|(id, _)| *id)
            .collect();
        let left: Vec<i32> = viewer.tracked.difference(&visible).copied().collect();
        let entered: Vec<i32> = visible.difference(&viewer.tracked).copied().collect();
        let updated: Vec<i32> = visible.intersection(&viewer.tracked).copied().collect();

        let viewer = self
            .viewers
            .get_mut(&viewer_id)
            .expect("viewer was read above");
        let mut packets = std::mem::take(&mut viewer.pending);
        if !left.is_empty() {
            packets.push(remove_entities_packet(&left));
            for id in &left {
                viewer.last_sent.remove(id);
            }
        }
        for id in &entered {
            if let Some(tracked) = self.entities.get(id) {
                packets.push(add_entity_packet(&tracked.entity));
                packets.push(tracked.living.all_metadata_packet());
                packets.push(tracked.living.equipment_packet());
                packets.extend(tracked.living.active_effect_packets());
                viewer
                    .last_sent
                    .insert(*id, EntitySnapshot::of(&tracked.entity));
            }
        }
        for id in &updated {
            if let Some(tracked) = self.entities.get(id)
                && viewer
                    .last_sent
                    .get(id)
                    .is_none_or(|snapshot| !snapshot.matches(&tracked.entity))
            {
                packets.push(entity_position_sync_packet(&tracked.entity));
                viewer
                    .last_sent
                    .insert(*id, EntitySnapshot::of(&tracked.entity));
            }
        }
        viewer.tracked = visible;
        packets
    }

    /// Update every viewer in ascending viewer-id order. Only viewers with
    /// packets are returned, preserving per-viewer packet order.
    pub fn update_all_viewers(&mut self) -> Vec<(i32, Vec<Vec<u8>>)> {
        let viewers: Vec<i32> = self.viewers.keys().copied().collect();
        let mut updates = Vec::new();
        for viewer in viewers {
            let packets = self.update_viewer(viewer);
            if !packets.is_empty() {
                updates.push((viewer, packets));
            }
        }
        updates
    }
}

/// Per-session entity view: deterministic mobs near the player plus their
/// metadata, health, equipment, and removal lifecycle.
///
/// The tracker owns its living-entity records instead of using the
/// process-wide [`crate::entity_metadata::living_entities`] store. That keeps
/// two players from sharing one cow and makes disconnect cleanup a plain drop.
pub struct SessionEntityTracker {
    entities: EntityTracker,
    ai_world: MemoryWorld,
    game_time: i64,
}

static NEXT_SESSION_ENTITY_ID: AtomicI32 = AtomicI32::new(10_000);

impl SessionEntityTracker {
    /// Spawn the pinned mob set on a flat surface at `ground_y` (the block the
    /// mobs stand on). Used by tests; the live join path uses
    /// [`Self::spawn_near_player`].
    pub fn deterministic(player: (f64, f64, f64), ground_y: i32, seed: i64) -> Self {
        Self::build(player, seed, |_, _| f64::from(ground_y))
    }

    /// Spawn the pinned set around `player`, resolving each column's surface
    /// through `storage`. Missing chunks fall back to the player's own y.
    pub fn spawn_near_player(
        storage: &mut WorldStorage,
        player: (f64, f64, f64),
        seed: i64,
    ) -> Self {
        Self::build(player, seed, |x, z| {
            let block_x = x.floor() as i32;
            let block_z = z.floor() as i32;
            let chunk_x = block_x.div_euclid(16);
            let chunk_z = block_z.div_euclid(16);
            match storage.load_chunk(chunk_x, chunk_z) {
                Ok(Some(chunk)) => f64::from(crate::chunk::surface_y(
                    &chunk,
                    block_x.rem_euclid(16) as usize,
                    block_z.rem_euclid(16) as usize,
                )),
                _ => player.1,
            }
        })
    }

    fn build(player: (f64, f64, f64), seed: i64, mut ground: impl FnMut(f64, f64) -> f64) -> Self {
        let base_id = NEXT_SESSION_ENTITY_ID.fetch_add(8, Ordering::Relaxed);
        let mut entities = EntityTracker::new();
        for (index, spawn) in SESSION_MOB_SPAWNS.iter().enumerate() {
            let x = player.0.floor() + spawn.dx + 0.5;
            let z = player.2.floor() + spawn.dz + 0.5;
            let y = ground(x, z);
            let entity_type = crate::spawn::entity_type(spawn.name)
                .expect("every pinned session mob is a shipped entity type");
            let id = base_id + index as i32;
            let mut entity = Entity::new(
                id,
                session_entity_uuid(id, index),
                entity_type.id(),
                x,
                y,
                z,
            );
            entity.width = f64::from(entity_type.width);
            entity.height = f64::from(entity_type.height);
            entity.yaw = match index {
                0 => 90.0,
                1 => 180.0,
                _ => 0.0,
            };

            let mut living = LivingEntity::new(id);
            living.set_attribute(Attribute::MaxHealth, f64::from(spawn.max_health));
            living.set_health(spawn.max_health);
            // The initial all-metadata packet carries these values; do not
            // re-send them on the first tick.
            living.metadata.pack_dirty();

            let ai = (spawn.name == "minecraft:zombie").then(|| {
                AiMob::new(
                    id,
                    ActorKind::Zombie,
                    Vec3::new(x, y, z),
                    seed ^ i64::from(id),
                )
            });

            entities.insert(TrackedEntity { entity, living, ai });
        }

        Self {
            entities,
            ai_world: MemoryWorld::flat(-64, 320, player.1.floor() as i32 - 1, 24),
            game_time: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Shared entity records without cloning. Callers that need per-viewer
    /// tracking can drive [`EntityTracker`] directly instead of maintaining a
    /// second entity list.
    pub fn shared(&self) -> &EntityTracker {
        &self.entities
    }

    pub fn shared_mut(&mut self) -> &mut EntityTracker {
        &mut self.entities
    }

    pub fn tracked_ids(&self) -> Vec<i32> {
        self.entities.ids().collect()
    }

    pub fn tracks(&self, id: i32) -> bool {
        self.entities.contains(id)
    }

    pub fn entity(&self, id: i32) -> Option<&TrackedEntity> {
        self.entities.get(id)
    }

    /// Add Entity, initial metadata (including health), equipment, and active
    /// effects for every tracked mob, in ascending network-id order.
    pub fn join_packets(&self) -> Vec<Vec<u8>> {
        let mut packets = Vec::with_capacity(self.entities.len() * 3);
        for id in self.entities.ids() {
            packets.extend(self.entities.enter_packets(id));
        }
        packets
    }

    /// One session tick: physics/gravity, AI environment, dirty metadata,
    /// death animation, and out-of-range removal.
    pub fn tick(
        &mut self,
        player: (f64, f64, f64),
        collider: &mut impl BlockCollider,
    ) -> SessionEntityTick {
        self.game_time += 1;
        let mut out = SessionEntityTick::default();
        let range_sq = SESSION_TRACKING_RANGE * SESSION_TRACKING_RANGE;
        let ai_world = &self.ai_world;
        let ids: Vec<i32> = self.entities.ids().collect();
        for id in ids {
            let Some(tracked) = self.entities.get_mut(id) else {
                continue;
            };
            let dx = tracked.entity.x - player.0;
            let dy = tracked.entity.y - player.1;
            let dz = tracked.entity.z - player.2;
            if dx * dx + dy * dy + dz * dz > range_sq {
                out.packets.push(remove_entities_packet(&[id]));
                out.removed.push(id);
                self.entities.remove_entity(id);
                continue;
            }

            let moved = tracked.entity.tick_physics(collider);
            if let Some(ai) = &mut tracked.ai {
                ai.health = tracked.living.health;
                ai.pos = Vec3::new(tracked.entity.x, tracked.entity.y, tracked.entity.z);
                let feet = ai.block_pos();
                let feet_block = collider.block_at(feet.x(), feet.y(), feet.z());
                let below = feet.below();
                let below_block = collider.block_at(below.x(), below.y(), below.z());
                if feet_block.is_some() || below_block.is_some() {
                    ai.update_environment_from_blocks(feet_block.as_ref(), below_block.as_ref());
                } else {
                    ai.update_environment(ai_world);
                }
            }
            if moved {
                out.packets
                    .push(entity_position_sync_packet(&tracked.entity));
            }
            out.packets.extend(tracked.living.tick());
            if let Some(packet) = tracked.living.dirty_metadata_packet() {
                out.packets.push(packet);
            }
            if tracked.living.removed {
                out.packets.push(remove_entities_packet(&[id]));
                out.removed.push(id);
                self.entities.remove_entity(id);
            }
        }
        out
    }

    /// Apply damage to one tracked mob. The returned packets are the vanilla
    /// death event, metadata health/pose update, and (for players) SetHealth.
    pub fn damage(&mut self, id: i32, amount: f32) -> Option<HurtResult> {
        let tracked = self.entities.get_mut(id)?;
        let result = tracked.living.hurt(amount);
        let health = tracked.living.health;
        if let Some(ai) = &mut tracked.ai {
            ai.health = health;
            if tracked.living.dead {
                ai.alive = false;
            }
        }
        Some(result)
    }

    /// Remove one tracked id and return its remove-entities packet.
    pub fn remove(&mut self, id: i32) -> Option<Vec<u8>> {
        self.entities.remove_entity(id)?;
        Some(remove_entities_packet(&[id]))
    }

    /// Drop tracked mobs whose chunk is no longer in the session's sent set.
    pub fn retain_in_chunks(&mut self, chunks: &HashSet<(i32, i32)>) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        let ids: Vec<i32> = self.entities.ids().collect();
        for id in ids {
            let Some(tracked) = self.entities.get(id) else {
                continue;
            };
            let chunk = (
                (tracked.entity.x.floor() as i32).div_euclid(16),
                (tracked.entity.z.floor() as i32).div_euclid(16),
            );
            if !chunks.contains(&chunk) {
                packets.push(remove_entities_packet(&[id]));
                self.entities.remove_entity(id);
            }
        }
        packets
    }

    /// Disconnect cleanup. The per-session store makes this equivalent to
    /// dropping the tracker, but the live loop calls it explicitly so tracking
    /// state is empty before the socket closes.
    pub fn clear(&mut self) {
        self.entities.clear();
    }
}

fn session_entity_uuid(id: i32, index: usize) -> [u8; 16] {
    let mut uuid = [0u8; 16];
    uuid[0..4].copy_from_slice(&id.to_be_bytes());
    uuid[4] = 0x5a;
    uuid[5] = index as u8;
    uuid[15] = 0x7f;
    uuid
}

impl Drop for SessionEntityTracker {
    fn drop(&mut self) {
        self.clear();
    }
}

static ENTITIES: LazyLock<Mutex<HashMap<i32, Entity>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static WORLD: OnceLock<&'static Mutex<WorldStorage>> = OnceLock::new();

pub fn configure_world(world_dir: impl Into<PathBuf>) {
    let world_dir = world_dir.into();
    if world_dir.join("region").is_dir() {
        // Physics must read the same storage the session writes, otherwise a
        // region save from one side drops the other side's chunks.
        let _ = WORLD.set(crate::world::shared_overworld(world_dir));
    }
}

pub struct WorldCollider<'a> {
    storage: &'a mut WorldStorage,
    chunks: HashMap<(i32, i32), Chunk>,
}

impl<'a> WorldCollider<'a> {
    pub fn new(storage: &'a mut WorldStorage) -> Self {
        Self {
            storage,
            chunks: HashMap::new(),
        }
    }

    fn block_state(&mut self, x: i32, y: i32, z: i32) -> Option<&BlockState> {
        let chunk_x = x.div_euclid(16);
        let chunk_z = z.div_euclid(16);
        let key = (chunk_x, chunk_z);
        if !self.chunks.contains_key(&key) {
            match self.storage.load_chunk(chunk_x, chunk_z) {
                Ok(Some(chunk)) => {
                    self.chunks.insert(key, chunk);
                }
                _ => return None,
            }
        }
        let chunk = self.chunks.get(&key)?;
        let local_x = x.rem_euclid(16) as usize;
        let local_z = z.rem_euclid(16) as usize;
        chunk.block_state(local_x, y, local_z)
    }
}

impl BlockCollider for WorldCollider<'_> {
    fn is_solid(&mut self, x: i32, y: i32, z: i32) -> bool {
        let Some(state) = self.block_state(x, y, z) else {
            return false;
        };
        crate::block_shapes::has_collision_state(state)
    }

    fn block_at(&mut self, x: i32, y: i32, z: i32) -> Option<BlockState> {
        self.block_state(x, y, z).cloned()
    }

    fn collision_boxes(&mut self, x: i32, y: i32, z: i32) -> Vec<Aabb> {
        let Some(state) = self.block_state(x, y, z) else {
            return Vec::new();
        };
        let state_id = crate::registry::block_state_id(state);
        crate::block_shapes::collision_boxes(state_id)
            .iter()
            .map(|bounds| bounds.offset(f64::from(x), f64::from(y), f64::from(z)))
            .collect()
    }
}

/// Tick every tracked entity once and broadcast position syncs for entities that
/// moved. The server calls this from its 50 ms tick loop.
pub fn tick_all() -> Vec<Vec<u8>> {
    let mut entities = ENTITIES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let mut packets = Vec::new();
    if let Some(world) = WORLD.get() {
        let mut storage = crate::world::lock(world);
        let mut collider = WorldCollider::new(&mut storage);
        tick_entities(&mut entities, &mut collider, &mut packets);
    } else {
        let mut collider = FlatCollider { ground_y: -60 };
        tick_entities(&mut entities, &mut collider, &mut packets);
    }
    drop(entities);

    for packet in &packets {
        crate::block_update::broadcast(packet);
    }
    packets
}

fn tick_entities(
    entities: &mut HashMap<i32, Entity>,
    collider: &mut impl BlockCollider,
    packets: &mut Vec<Vec<u8>>,
) {
    for entity in entities.values_mut() {
        if entity.tick_physics(collider) {
            packets.push(entity_position_sync_packet(entity));
        }
    }
}

pub fn spawn(entity: Entity) -> Vec<u8> {
    let packet = add_entity_packet(&entity);
    ENTITIES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(entity.id, entity);
    crate::block_update::broadcast(&packet);
    packet
}

/// Spawn a living entity and register its health/metadata/equipment record by
/// id. The add-entity packet is unchanged; callers send
/// [`crate::entity_metadata::LivingEntity::all_metadata_packet`] when they need
/// the initial synced values.
pub fn spawn_living(entity: Entity) -> Vec<u8> {
    let id = entity.id;
    let packet = spawn(entity);
    crate::entity_metadata::register_living(id);
    packet
}

/// Apply damage to a registered living entity and broadcast the resulting
/// metadata/death/health packets. Returns `None` when no living record exists.
pub fn hurt_entity(id: i32, amount: f32) -> Option<crate::entity_metadata::HurtResult> {
    let result = crate::entity_metadata::living_entities().hurt(id, amount)?;
    for packet in &result.packets {
        crate::block_update::broadcast(packet);
    }
    Some(result)
}

/// Tick every registered living entity once and broadcast death-animation
/// packets. The physics tick remains in [`tick_all`].
pub fn tick_living_entities() -> Vec<Vec<u8>> {
    let packets = crate::entity_metadata::living_entities().tick_all();
    for packet in &packets {
        crate::block_update::broadcast(packet);
    }
    packets
}

/// Replace one equipment slot and broadcast a full equipment update.
pub fn set_entity_equipment(
    id: i32,
    slot: crate::entity_metadata::EquipmentSlot,
    stack: crate::item_components::ItemStack,
) -> Option<Vec<u8>> {
    let mut entities = crate::entity_metadata::living_entities();
    entities.set_equipment(id, slot, stack)?;
    let packet = entities.equipment_packet(id)?;
    drop(entities);
    crate::block_update::broadcast(&packet);
    Some(packet)
}

pub fn remove(id: i32) -> Option<Vec<u8>> {
    crate::entity_metadata::remove_living(id);
    let removed = ENTITIES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&id)?;
    let packet = remove_entities_packet(&[removed.id]);
    crate::block_update::broadcast(&packet);
    Some(packet)
}

pub fn move_entity(id: i32, x: f64, y: f64, z: f64) -> Option<Vec<u8>> {
    let mut entities = ENTITIES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let entity = entities.get_mut(&id)?;
    entity.x = x;
    entity.y = y;
    entity.z = z;
    entity.velocity = [0.0; 3];
    entity.on_ground = false;
    let packet = entity_position_sync_packet(entity);
    drop(entities);
    crate::block_update::broadcast(&packet);
    Some(packet)
}

pub fn spawn_packets() -> Vec<Vec<u8>> {
    ENTITIES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .values()
        .map(add_entity_packet)
        .collect()
}

/// Whether the process-wide broadcast entity map owns `id`. Live sessions use
/// this to decide whether an incoming entity packet belongs to their view.
pub fn is_tracked(id: i32) -> bool {
    ENTITIES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains_key(&id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PacketReader;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn armor_stand() -> Entity {
        let mut uuid = [0u8; 16];
        uuid[15] = 0x2a;
        Entity::new(42, uuid, 5, 2.0, -60.0, 0.0)
    }

    #[test]
    fn add_entity_body_matches_java() {
        let packet = add_entity_packet(&armor_stand());
        assert_eq!(
            hex(&packet[1..]),
            "2a0000000000000000000000000000002a054000000000000000c04e00000000000000000000000000000000000000"
        );
    }

    #[test]
    fn remove_entities_body_matches_java() {
        let packet = remove_entities_packet(&[42]);
        assert_eq!(hex(&packet[1..]), "012a");
    }

    #[test]
    fn position_sync_body_matches_java() {
        let mut entity = armor_stand();
        entity.x = 4.0;
        let packet = entity_position_sync_packet(&entity);
        assert_eq!(
            hex(&packet[1..]),
            "2a4010000000000000c04e0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001"
        );
    }

    #[test]
    fn store_spawn_move_remove_round_trip() {
        let entity = armor_stand();
        let add = spawn(entity.clone());
        let mut reader = PacketReader::new(&add);
        assert_eq!(reader.read_varint().unwrap(), CLIENTBOUND_ADD_ENTITY);
        assert!(!spawn_packets().is_empty());

        let sync = move_entity(42, 4.0, -60.0, 0.0).expect("sync");
        let mut reader = PacketReader::new(&sync);
        assert_eq!(
            reader.read_varint().unwrap(),
            CLIENTBOUND_ENTITY_POSITION_SYNC
        );

        let remove_packet = remove(42).expect("remove");
        let mut reader = PacketReader::new(&remove_packet);
        assert_eq!(reader.read_varint().unwrap(), CLIENTBOUND_REMOVE_ENTITIES);
        assert!(remove(42).is_none());
    }

    struct WallCollider {
        ground_y: i32,
        wall_x: i32,
    }

    impl BlockCollider for WallCollider {
        fn is_solid(&mut self, x: i32, y: i32, _z: i32) -> bool {
            y < self.ground_y || x >= self.wall_x
        }
    }

    #[test]
    fn gravity_falls_and_lands_on_ground() {
        let mut entity = Entity::armor_stand(42, [0; 16], 0.0, 5.0, 0.0);
        let mut collider = FlatCollider { ground_y: 0 };
        for _ in 0..200 {
            entity.tick_physics(&mut collider);
        }
        assert!(entity.on_ground);
        assert!((entity.y - 0.0).abs() < 1.0e-6, "y={}", entity.y);
        assert_eq!(entity.velocity[1], 0.0);
    }

    #[test]
    fn gravity_velocity_uses_minecraft_constants() {
        let mut entity = Entity::armor_stand(42, [0; 16], 0.0, 5.0, 0.0);
        let mut collider = FlatCollider { ground_y: -100 };
        entity.tick_physics(&mut collider);
        assert!((entity.y - 4.9216).abs() < 1.0e-9, "y={}", entity.y);
        assert!((entity.velocity[1] - (-0.08 * 0.98)).abs() < 1.0e-9);
    }

    #[test]
    fn terminal_velocity_converges_to_minecraft_value() {
        let mut entity = Entity::armor_stand(42, [0; 16], 0.0, 100_000.0, 0.0);
        let mut collider = FlatCollider { ground_y: -100_000 };
        for _ in 0..1_000 {
            entity.tick_physics(&mut collider);
        }
        assert!(
            (entity.velocity[1] - MAX_FALL_SPEED).abs() < 1.0e-6,
            "terminal velocity {}",
            entity.velocity[1]
        );
    }

    #[test]
    fn wall_blocks_horizontal_motion() {
        let mut entity = Entity::new(42, [0; 16], 5, 1.0, 0.0, 0.0);
        entity.set_velocity(1.0, 0.0, 0.0);
        let mut collider = WallCollider {
            ground_y: 0,
            wall_x: 2,
        };
        for _ in 0..20 {
            entity.tick_physics(&mut collider);
        }
        assert!(entity.x <= 1.7 + 1.0e-6, "x={}", entity.x);
        assert_eq!(entity.velocity[0], 0.0);
        assert!(entity.on_ground);
    }

    #[test]
    fn tick_all_emits_position_sync_for_moving_entity() {
        let mut uuid = [0u8; 16];
        uuid[15] = 0x42;
        spawn(Entity::armor_stand(9002, uuid, 0.0, 5.0, 0.0));
        let packets = tick_all();
        assert!(!packets.is_empty());
        assert_eq!(packet_id(&packets[0]), CLIENTBOUND_ENTITY_POSITION_SYNC);
        remove(9002);
    }

    struct LiveEnvironmentCollider {
        feet: crate::math::BlockPos,
    }

    impl BlockCollider for LiveEnvironmentCollider {
        fn is_solid(&mut self, _x: i32, y: i32, _z: i32) -> bool {
            y < self.feet.y()
        }

        fn block_at(&mut self, x: i32, y: i32, z: i32) -> Option<BlockState> {
            let state = |name: &str| BlockState {
                name: name.to_owned(),
                properties: Vec::new(),
            };
            if x == self.feet.x() && y == self.feet.y() && z == self.feet.z() {
                Some(state("minecraft:water"))
            } else if y < self.feet.y() {
                Some(state("minecraft:stone"))
            } else {
                None
            }
        }
    }

    #[test]
    fn session_ai_reads_the_real_collider_environment() {
        let mut tracker = SessionEntityTracker::deterministic((0.0, 63.0, 0.0), 62, 7);
        let mut zombie = None;
        for id in tracker.entities.ids() {
            if tracker
                .entity(id)
                .is_some_and(|tracked| tracked.ai.is_some())
            {
                zombie = Some(id);
                break;
            }
        }
        let zombie = zombie.expect("session tracker spawns a zombie");
        let feet = tracker
            .entity(zombie)
            .and_then(|tracked| tracked.ai.as_ref())
            .expect("zombie AI")
            .block_pos();
        let mut collider = LiveEnvironmentCollider { feet };
        tracker.tick((0.0, 63.0, 0.0), &mut collider);
        let ai = tracker
            .entity(zombie)
            .and_then(|tracked| tracked.ai.as_ref());
        let ai = ai.expect("zombie AI survives one tick");
        assert!(ai.in_water, "water at the zombie's feet must be visible");
        assert!(ai.on_ground, "solid ground below must be visible");
    }

    fn packet_id(packet: &[u8]) -> i32 {
        let mut reader = PacketReader::new(packet);
        reader.read_varint().unwrap()
    }
}
