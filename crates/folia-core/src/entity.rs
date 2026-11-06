use crate::chunk::Chunk;
use crate::protocol::PacketWriter;
use crate::world::WorldStorage;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, OnceLock};

pub const CLIENTBOUND_ADD_ENTITY: i32 = 1;
pub const CLIENTBOUND_ENTITY_POSITION_SYNC: i32 = 35;
pub const CLIENTBOUND_REMOVE_ENTITIES: i32 = 75;

pub const GRAVITY: f64 = 0.08;
pub const DRAG: f64 = 0.98;
/// The fixed point of `(v - 0.08) * 0.98`; armor stands reach this terminal
/// fall speed after a long fall. Vanilla does not clamp explicitly.
pub const MAX_FALL_SPEED: f64 = -3.92;

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
        let (min_x, min_y, min_z, max_x, max_y, max_z) = self.aabb();
        let (target_min_x, target_min_y, target_min_z, target_max_x, target_max_y, target_max_z) =
            match axis {
                Axis::X => (min_x + amount, min_y, min_z, max_x + amount, max_y, max_z),
                Axis::Y => (min_x, min_y + amount, min_z, max_x, max_y + amount, max_z),
                Axis::Z => (min_x, min_y, min_z + amount, max_x, max_y, max_z + amount),
            };

        let x0 = min_x.min(target_min_x).floor() as i32;
        let x1 = (max_x.max(target_max_x) - 1.0e-7).floor() as i32;
        let y0 = min_y.min(target_min_y).floor() as i32;
        let y1 = (max_y.max(target_max_y) - 1.0e-7).floor() as i32;
        let z0 = min_z.min(target_min_z).floor() as i32;
        let z1 = (max_z.max(target_max_z) - 1.0e-7).floor() as i32;

        let mut allowed = amount;
        for bx in x0..=x1 {
            for by in y0..=y1 {
                for bz in z0..=z1 {
                    if !collider.is_solid(bx, by, bz) {
                        continue;
                    }
                    let block_min_x = f64::from(bx);
                    let block_max_x = block_min_x + 1.0;
                    let block_min_y = f64::from(by);
                    let block_max_y = block_min_y + 1.0;
                    let block_min_z = f64::from(bz);
                    let block_max_z = block_min_z + 1.0;

                    match axis {
                        Axis::X => {
                            if amount > 0.0
                                && max_x <= block_min_x + 1.0e-7
                                && target_max_x > block_min_x
                            {
                                allowed = allowed.min(block_min_x - max_x);
                            } else if amount < 0.0
                                && min_x >= block_max_x - 1.0e-7
                                && target_min_x < block_max_x
                            {
                                allowed = allowed.max(block_max_x - min_x);
                            }
                        }
                        Axis::Y => {
                            if amount > 0.0
                                && max_y <= block_min_y + 1.0e-7
                                && target_max_y > block_min_y
                            {
                                allowed = allowed.min(block_min_y - max_y);
                            } else if amount < 0.0
                                && min_y >= block_max_y - 1.0e-7
                                && target_min_y < block_max_y
                            {
                                allowed = allowed.max(block_max_y - min_y);
                            }
                        }
                        Axis::Z => {
                            if amount > 0.0
                                && max_z <= block_min_z + 1.0e-7
                                && target_max_z > block_min_z
                            {
                                allowed = allowed.min(block_min_z - max_z);
                            } else if amount < 0.0
                                && min_z >= block_max_z - 1.0e-7
                                && target_min_z < block_max_z
                            {
                                allowed = allowed.max(block_max_z - min_z);
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

struct WorldCollider<'a> {
    storage: &'a mut WorldStorage,
    chunks: HashMap<(i32, i32), Chunk>,
}

impl<'a> WorldCollider<'a> {
    fn new(storage: &'a mut WorldStorage) -> Self {
        Self {
            storage,
            chunks: HashMap::new(),
        }
    }
}

impl BlockCollider for WorldCollider<'_> {
    fn is_solid(&mut self, x: i32, y: i32, z: i32) -> bool {
        let chunk_x = x.div_euclid(16);
        let chunk_z = z.div_euclid(16);
        let key = (chunk_x, chunk_z);
        if !self.chunks.contains_key(&key) {
            match self.storage.load_chunk(chunk_x, chunk_z) {
                Ok(Some(chunk)) => {
                    self.chunks.insert(key, chunk);
                }
                _ => return false,
            }
        }
        let Some(chunk) = self.chunks.get(&key) else {
            return false;
        };
        let local_x = x.rem_euclid(16) as usize;
        let local_z = z.rem_euclid(16) as usize;
        match chunk.block_state(local_x, y, local_z) {
            Some(state) => !matches!(
                state.name.as_str(),
                "minecraft:air"
                    | "minecraft:cave_air"
                    | "minecraft:void_air"
                    | "minecraft:water"
                    | "minecraft:lava"
            ),
            None => false,
        }
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

    fn packet_id(packet: &[u8]) -> i32 {
        let mut reader = PacketReader::new(packet);
        reader.read_varint().unwrap()
    }
}
