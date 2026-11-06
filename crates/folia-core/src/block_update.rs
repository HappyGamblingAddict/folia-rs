use crate::anvil::Compression;
use crate::chunk::BlockState;
use crate::chunk_packet::{block_state_id, heightmap_bytes, light_bytes};
use crate::math::BlockPos;
use crate::protocol::{PacketReader, PacketWriter};
use crate::tick::UpdateFlags;
use crate::world::WorldStorage;
use std::io;
use std::sync::{LazyLock, Mutex, mpsc};

pub const CLIENTBOUND_BLOCK_UPDATE: i32 = 8;
pub const CLIENTBOUND_SECTION_BLOCKS_UPDATE: i32 = 82;
pub const CLIENTBOUND_LIGHT_UPDATE: i32 = 47;
pub const SERVERBOUND_PLAYER_ACTION: i32 = 40;
pub const SERVERBOUND_USE_ITEM_ON: i32 = 63;
pub const ACTION_START_DESTROY_BLOCK: i32 = 0;
pub const ACTION_STOP_DESTROY_BLOCK: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerAction {
    pub action: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub direction: u8,
    pub sequence: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UseItemOn {
    pub hand: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub direction: u8,
    pub cursor_x: f32,
    pub cursor_y: f32,
    pub cursor_z: f32,
    pub inside: bool,
    pub sequence: i32,
}

#[derive(Debug, Clone)]
pub struct BlockChange {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub old_state: Option<BlockState>,
    pub new_state: BlockState,
    pub state_id: i32,
    pub packet: Vec<u8>,
    pub heightmaps: Vec<u8>,
    pub light: Vec<u8>,
}

/// Mutate a block in the world, persist the chunk, queue the six neighbour
/// updates, and process them before returning.
///
/// Vanilla `Level.setBlock(..., 3)` sends the client update, calls
/// `updateNeighborsAt` for the six neighbours, and then runs
/// `updateNeighbourShapes`. This entry point mirrors that order: the caller
/// broadcasts the returned primary change, while neighbour-driven changes are
/// applied and broadcast here.
pub fn set_block(
    storage: &mut WorldStorage,
    x: i32,
    y: i32,
    z: i32,
    state: BlockState,
) -> io::Result<Option<BlockChange>> {
    storage.tick_state_mut().last_neighbor_updates.clear();
    perform_block_change(
        storage,
        BlockPos::new(x, y, z),
        state,
        UpdateFlags::ALL | UpdateFlags::SHAPE,
        true,
        false,
    )
}

/// Shared mutation path for player edits, scheduled ticks, random ticks, and
/// neighbour shape updates.
///
/// `process` drains the neighbour queue before returning; behaviour-driven
/// changes use `false` so the enclosing tick owns the drain and the 512-update
/// chain limit. `broadcast` sends the change packet from this path (used by
/// ticks, whose caller has no return value to forward).
pub(crate) fn perform_block_change(
    storage: &mut WorldStorage,
    pos: BlockPos,
    state: BlockState,
    flags: u32,
    process: bool,
    broadcast: bool,
) -> io::Result<Option<BlockChange>> {
    let (x, y, z) = (pos.x(), pos.y(), pos.z());
    let chunk_x = x.div_euclid(16);
    let chunk_z = z.div_euclid(16);
    let local_x = x.rem_euclid(16) as usize;
    let local_z = z.rem_euclid(16) as usize;

    let Some(mut chunk) = storage.load_chunk(chunk_x, chunk_z)? else {
        return Ok(None);
    };
    let old_state = chunk.block_state(local_x, y, local_z).cloned();
    if !chunk.set_block_state(local_x, y, local_z, state.clone()) {
        return Ok(None);
    }

    // The edit may change opacity or emission, and light can reach across the
    // chunk border. Load whatever neighbours already exist (never generate)
    // and relight the edited chunk plus the one-block border deterministically.
    let mut neighbours = Vec::new();
    for dx in -1..=1 {
        for dz in -1..=1 {
            if dx == 0 && dz == 0 {
                continue;
            }
            if let Some(neighbour) = storage.load_chunk(chunk_x + dx, chunk_z + dz)? {
                neighbours.push(neighbour);
            }
        }
    }
    chunk.recompute_light_with_neighbours(&neighbours);

    storage.update_chunk(&chunk, Compression::Zlib, &[(x, y, z)])?;
    storage.flush()?;
    let state_id = block_state_id(&state);
    let mut change = BlockChange {
        chunk_x,
        chunk_z,
        x,
        y,
        z,
        old_state,
        new_state: state,
        state_id,
        packet: block_update_packet(x, y, z, state_id),
        heightmaps: heightmap_bytes(&chunk),
        light: light_bytes(&chunk),
    };

    if broadcast {
        broadcast_change(&change);
    }
    crate::tick::on_block_change(
        storage,
        pos,
        change.old_state.as_ref(),
        &change.new_state,
        flags,
        process,
    )?;
    if !broadcast {
        // Neighbour shape updates may have changed the same chunk again (for
        // example, removing a plant above the edited block). The caller
        // broadcasts this primary change after the neighbour packets, so
        // refresh its whole-chunk payloads to the final chunk state.
        if let Some(chunk) = storage.load_chunk(chunk_x, chunk_z)? {
            change.heightmaps = heightmap_bytes(&chunk);
            change.light = light_bytes(&chunk);
        }
    }
    Ok(Some(change))
}

/// Dispatches `useWithoutItem` for the ported interactive blocks.
///
/// The live session's use-item-on path still handles block placement; this is
/// the hook a future interaction handler should call for levers and buttons.
pub fn use_block(storage: &mut WorldStorage, x: i32, y: i32, z: i32) -> io::Result<bool> {
    crate::redstone::use_block(storage, BlockPos::new(x, y, z))
}

pub fn block_update_packet(x: i32, y: i32, z: i32, state_id: i32) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(CLIENTBOUND_BLOCK_UPDATE).unwrap();
    writer.write_i64(block_pos_long(x, y, z)).unwrap();
    writer.write_varint(state_id).unwrap();
    writer.into_inner()
}

pub fn section_blocks_update_packet(
    section_x: i32,
    section_y: i32,
    section_z: i32,
    changes: &[(u8, u8, u8, i32)],
) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer
        .write_varint(CLIENTBOUND_SECTION_BLOCKS_UPDATE)
        .unwrap();
    writer
        .write_i64(section_pos_long(section_x, section_y, section_z))
        .unwrap();
    writer.write_varint(changes.len() as i32).unwrap();
    for &(x, y, z, state_id) in changes {
        let packed_position = (u64::from(x) << 8) | (u64::from(z) << 4) | u64::from(y);
        let value = ((state_id as i64) << 12) | packed_position as i64;
        writer.write_varlong(value).unwrap();
    }
    writer.into_inner()
}

pub fn block_pos_long(x: i32, y: i32, z: i32) -> i64 {
    ((x as i64 & 0x3FF_FFFF) << 38) | (y as i64 & 0xFFF) | ((z as i64 & 0x3FF_FFFF) << 12)
}

pub fn section_pos_long(x: i32, y: i32, z: i32) -> i64 {
    ((x as i64 & 0x3F_FFFF) << 42) | (y as i64 & 0xF_FFFF) | ((z as i64 & 0x3F_FFFF) << 20)
}

pub fn section_of_block(x: i32, y: i32, z: i32) -> (i32, i32, i32) {
    (x.div_euclid(16), y.div_euclid(16), z.div_euclid(16))
}

pub fn parse_player_action(packet: &[u8]) -> io::Result<PlayerAction> {
    let mut reader = PacketReader::new(packet);
    let id = reader.read_varint()?;
    if id != SERVERBOUND_PLAYER_ACTION {
        return Err(invalid(format!("expected player action id 40, got {id}")));
    }
    let action = reader.read_varint()?;
    let pos = reader.read_i64()?;
    let direction = reader.read_u8()?;
    let sequence = reader.read_varint()?;
    let (x, y, z) = unpack_block_pos(pos);
    Ok(PlayerAction {
        action,
        x,
        y,
        z,
        direction,
        sequence,
    })
}

pub fn parse_use_item_on(packet: &[u8]) -> io::Result<UseItemOn> {
    let mut reader = PacketReader::new(packet);
    let id = reader.read_varint()?;
    if id != SERVERBOUND_USE_ITEM_ON {
        return Err(invalid(format!("expected use item on id 63, got {id}")));
    }
    let hand = reader.read_varint()?;
    let pos = reader.read_i64()?;
    let direction = reader.read_varint()? as u8;
    let cursor_x = reader.read_f32()?;
    let cursor_y = reader.read_f32()?;
    let cursor_z = reader.read_f32()?;
    let inside = reader.read_bool()?;
    let _world_border_hit = reader.read_bool()?;
    let sequence = reader.read_varint()?;
    let (x, y, z) = unpack_block_pos(pos);
    Ok(UseItemOn {
        hand,
        x,
        y,
        z,
        direction,
        cursor_x,
        cursor_y,
        cursor_z,
        inside,
        sequence,
    })
}

pub fn unpack_block_pos(packed: i64) -> (i32, i32, i32) {
    let x = (packed >> 38) as i32;
    let y = ((packed << 52) >> 52) as i32;
    let z = ((packed << 26) >> 38) as i32;
    (x, y, z)
}

pub fn direction_offset(direction: u8) -> (i32, i32, i32) {
    match direction {
        0 => (0, -1, 0), // DOWN
        1 => (0, 1, 0),  // UP
        2 => (0, 0, -1), // NORTH
        3 => (0, 0, 1),  // SOUTH
        4 => (-1, 0, 0), // WEST
        5 => (1, 0, 0),  // EAST
        _ => (0, 0, 0),
    }
}

pub fn light_update_packet(chunk_x: i32, chunk_z: i32, light: &[u8]) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(CLIENTBOUND_LIGHT_UPDATE).unwrap();
    writer.write_varint(chunk_x).unwrap();
    writer.write_varint(chunk_z).unwrap();
    writer.write_bytes(light);
    writer.into_inner()
}

pub fn broadcast_change(change: &BlockChange) {
    broadcast(&change.packet);
    broadcast(&light_update_packet(
        change.chunk_x,
        change.chunk_z,
        &change.light,
    ));
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

static SESSIONS: LazyLock<Mutex<Vec<mpsc::Sender<Vec<u8>>>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// Register a session's outbound update queue. The session drains this in its
/// play loop; dead senders are removed on the next broadcast.
pub fn register_session() -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    SESSIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(sender);
    receiver
}

pub fn broadcast(packet: &[u8]) {
    let mut sessions = SESSIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    sessions.retain(|sender| sender.send(packet.to_vec()).is_ok());
}

pub fn broadcast_block_update(x: i32, y: i32, z: i32, state_id: i32) {
    broadcast(&block_update_packet(x, y, z, state_id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk_packet::flat_chunk;
    use crate::protocol::PacketReader;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "folia-rs-block-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn stone() -> BlockState {
        BlockState {
            name: "minecraft:stone".into(),
            properties: vec![],
        }
    }

    #[test]
    fn block_update_packet_has_position_and_state() {
        let packet = block_update_packet(1, 2, 3, 1);
        let mut reader = PacketReader::new(&packet);
        assert_eq!(reader.read_varint().unwrap(), CLIENTBOUND_BLOCK_UPDATE);
        assert_eq!(reader.read_i64().unwrap(), block_pos_long(1, 2, 3));
        assert_eq!(reader.read_varint().unwrap(), 1);
        assert!(reader.is_empty());
    }

    #[test]
    fn section_packet_packs_local_positions() {
        let packet = section_blocks_update_packet(0, -1, 0, &[(1, 2, 3, 1)]);
        let mut reader = PacketReader::new(&packet);
        assert_eq!(
            reader.read_varint().unwrap(),
            CLIENTBOUND_SECTION_BLOCKS_UPDATE
        );
        assert_eq!(reader.read_i64().unwrap(), section_pos_long(0, -1, 0));
        assert_eq!(reader.read_varint().unwrap(), 1);
        assert_eq!(reader.read_varlong().unwrap(), (1i64 << 12) | 0x132);
    }

    #[test]
    fn mutation_persists_and_recomputes_heightmap_and_light() {
        let root = temp_dir("mutate");
        let region_dir = root.join("region");
        let mut storage = WorldStorage::new(&region_dir);
        let flat = flat_chunk(0, 0);
        let before_heightmap = heightmap_bytes(&flat);
        let before_light = light_bytes(&flat);
        storage
            .save_chunk(&flat, Compression::Zlib)
            .expect("save flat chunk");

        let change = set_block(&mut storage, 0, -50, 0, stone())
            .expect("set block")
            .expect("block changed");
        assert_eq!(change.state_id, 1);
        assert_eq!(
            change.old_state.as_ref().map(|state| state.name.as_str()),
            Some("minecraft:air")
        );
        assert_eq!(change.new_state.name, "minecraft:stone");
        assert_ne!(change.heightmaps, before_heightmap);
        assert_ne!(change.light, before_light);
        assert_eq!(change.packet, block_update_packet(0, -50, 0, 1));

        storage.flush().expect("flush");
        let mut reopened = WorldStorage::new(&region_dir);
        let chunk = reopened.load_chunk(0, 0).expect("load").expect("chunk");
        assert_eq!(
            chunk.block_state(0, -50, 0).unwrap().name,
            "minecraft:stone"
        );
        assert_eq!(chunk.block_state(1, -50, 0).unwrap().name, "minecraft:air");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn broadcast_reaches_registered_sessions() {
        let receiver = register_session();
        let packet = block_update_packet(4, 5, 6, 9);
        broadcast(&packet);
        assert_eq!(receiver.try_recv().unwrap(), packet);
    }

    #[test]
    fn section_coordinates_are_floor_divided() {
        assert_eq!(section_of_block(-1, -1, -1), (-1, -1, -1));
        assert_eq!(section_of_block(0, 0, 0), (0, 0, 0));
        assert_eq!(section_of_block(16, 32, 48), (1, 2, 3));
    }

    #[test]
    fn use_block_toggles_a_lever_through_the_shared_path() {
        let root = temp_dir("use-lever");
        let mut storage = WorldStorage::new(root.join("region"));
        storage
            .save_chunk(&flat_chunk(0, 0), Compression::Zlib)
            .expect("save flat chunk");
        let lever = crate::registry::default_block_state("minecraft:lever").expect("lever state");
        set_block(&mut storage, 2, -60, 2, lever)
            .expect("place lever")
            .expect("lever changed");

        assert!(use_block(&mut storage, 2, -60, 2).expect("toggle lever"));
        let state = storage.block_state_at(2, -60, 2).expect("lever is cached");
        assert_eq!(crate::redstone::property(&state, "powered"), Some("true"));
        assert!(crate::redstone::is_powered(&state));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn packet_bodies_match_java_12111_encoders() {
        let block = block_update_packet(1, 2, 3, 1);
        assert_eq!(hex(&block[1..]), "000000400000300201");

        let section = section_blocks_update_packet(0, -1, 0, &[(1, 2, 3, 1)]);
        assert_eq!(hex(&section[1..]), "00000000000fffff01b222");
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
