use crate::anvil::Compression;
use crate::chunk::BlockState;
use crate::chunk_packet::{block_state_id, heightmap_bytes};
use crate::light::{DirtySections, LightScene, light_update_for_sections};
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
pub struct ChunkLightUpdate {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub light: Vec<u8>,
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
    /// Section-scoped `ClientboundLightUpdatePacketData` body for
    /// `chunk_x`/`chunk_z`. Empty when no section in that chunk changed.
    pub light: Vec<u8>,
    /// Sections whose light nibbles changed, including loaded neighbours.
    pub dirty_sections: DirtySections,
    /// Light payloads for neighbouring chunks that received border light.
    pub neighbour_light: Vec<ChunkLightUpdate>,
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
    if old_state.as_ref() == Some(&state) {
        return Ok(None);
    }

    // The edit may change opacity or emission, and light can reach across the
    // chunk border. Load whatever neighbours already exist (never generate)
    // and let the incremental scene update the edited chunk plus the affected
    // border of each neighbour.
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

    let dirty = {
        let mut scene = LightScene::new(&mut chunk, &mut neighbours);
        if !scene.set_block_state(x, y, z, state.clone()) {
            return Ok(None);
        }
        scene.dirty().clone()
    };

    // Persist neighbour light before the centre chunk, and build one
    // section-scoped light payload per neighbour chunk that changed.
    let mut neighbour_coords: Vec<(i32, i32)> = dirty
        .iter()
        .filter(|pos| pos.chunk_x != chunk_x || pos.chunk_z != chunk_z)
        .map(|pos| (pos.chunk_x, pos.chunk_z))
        .collect();
    neighbour_coords.sort_unstable();
    neighbour_coords.dedup();
    let mut neighbour_light = Vec::new();
    for (neighbour_x, neighbour_z) in neighbour_coords {
        let Some(index) = neighbours
            .iter()
            .position(|chunk| chunk.x == neighbour_x && chunk.z == neighbour_z)
        else {
            continue;
        };
        let payload = light_update_for_sections(&neighbours[index], &dirty);
        storage.update_chunk(&neighbours[index], Compression::Zlib, &[])?;
        if let Some(payload) = payload {
            neighbour_light.push(ChunkLightUpdate {
                chunk_x: neighbour_x,
                chunk_z: neighbour_z,
                light: payload,
            });
        }
    }

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
        light: light_update_for_sections(&chunk, &dirty).unwrap_or_default(),
        dirty_sections: dirty,
        neighbour_light,
    };

    storage.update_chunk(&chunk, Compression::Zlib, &[(x, y, z)])?;
    storage.flush()?;

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
        // refresh the dirty-section payload from the final chunk state.
        if let Some(chunk) = storage.load_chunk(chunk_x, chunk_z)? {
            change.heightmaps = heightmap_bytes(&chunk);
            change.light =
                light_update_for_sections(&chunk, &change.dirty_sections).unwrap_or_default();
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

/// Whether a state is a slab whose `type` is not `double`.
fn is_mergeable_slab(state: &BlockState) -> bool {
    state.name.ends_with("_slab") && crate::redstone::property(state, "type") != Some("double")
}

/// Whether placing `held_block` against `clicked` replaces `clicked` in place,
/// mirroring vanilla `Block.canBeReplaced`: ordinary replaceable blocks and a
/// non-double slab of the same type are overwritten rather than offset from.
pub fn is_replaceable_for_item(clicked: &BlockState, held_block: &BlockState) -> bool {
    crate::block_behaviour::replaceable(clicked)
        || (is_mergeable_slab(clicked) && held_block.name == clicked.name)
}

/// The block position a placement writes to: the clicked block when it can be
/// replaced in place, otherwise the neighbour in the clicked face direction.
/// An out-of-range face falls back to the block above.
pub fn placement_target(
    clicked: BlockPos,
    direction: u8,
    clicked_state: &BlockState,
    held_block: &BlockState,
) -> BlockPos {
    if is_replaceable_for_item(clicked_state, held_block) {
        return clicked;
    }
    let (dx, dy, dz) = if direction > 5 {
        (0, 1, 0)
    } else {
        direction_offset(direction)
    };
    BlockPos::new(clicked.x() + dx, clicked.y() + dy, clicked.z() + dz)
}

/// The state written at a resolved placement target.
///
/// Clicking a non-double slab with the same slab merges it into a double slab;
/// a fresh slab's `type` follows the clicked face and cursor height; and a
/// waterloggable state placed into a water *source* becomes waterlogged
/// (flowing water never waterlogs, matching `Fluids.WATER` only).
pub fn state_for_placement(
    clicked_state: Option<&BlockState>,
    target_current: Option<&BlockState>,
    held_block: &BlockState,
    direction: u8,
    cursor_y: f32,
    target_is_clicked: bool,
) -> BlockState {
    let mut state = held_block.clone();

    let merges = target_is_clicked
        && clicked_state
            .is_some_and(|clicked| is_mergeable_slab(clicked) && clicked.name == held_block.name)
        && target_current
            .is_some_and(|current| is_mergeable_slab(current) && current.name == held_block.name);
    if merges {
        state = crate::redstone::with_property(&state, "type", "double");
        state = crate::redstone::with_property(&state, "waterlogged", "false");
        return state;
    }

    if state.name.ends_with("_slab") {
        let top = direction == 0 || (direction != 1 && cursor_y > 0.5);
        state = crate::redstone::with_property(&state, "type", if top { "top" } else { "bottom" });
    }

    if crate::redstone::property(&state, "waterlogged").is_some()
        && target_current.is_some_and(|current| {
            crate::block_behaviour::fluid(current) == Some("minecraft:water")
        })
    {
        state = crate::redstone::with_property(&state, "waterlogged", "true");
    }

    state
}

/// Map a player yaw in degrees to a face direction in the same numeric domain
/// as [`direction_offset`]: 0=DOWN, 1=UP, 2=NORTH, 3=SOUTH, 4=WEST, 5=EAST.
///
/// Minecraft's horizontal facing order is SOUTH(0), WEST(1), NORTH(2),
/// EAST(3), which differs from that 3D ordering, so the yaw-facing value is
/// translated explicitly (SOUTH->3, WEST->4, NORTH->2, EAST->5). The yaw is
/// normalized with `rem_euclid(360.0)` and rounded to the nearest quarter
/// turn; an exact half-turn boundary (for example 45 degrees) rounds up to the
/// next quarter, matching Java's `Direction.fromYRot`.
pub fn direction_from_yaw(yaw: f32) -> u8 {
    let normalized = yaw.rem_euclid(360.0);
    let quarter = ((normalized / 90.0).round() as i32).rem_euclid(4);
    crate::math::Direction::from_2d_data(quarter).data_3d() as u8
}

/// Yaw-aware placement state.
///
/// Starts from [`state_for_placement`] so slab merging and waterlogging are
/// unchanged. For stairs the `facing` follows the player yaw and `half`
/// follows the clicked face and cursor height, while `shape` is forced to
/// `straight` when that property is present. The existing waterlogged result
/// from `state_for_placement` is preserved.
///
/// Known simplification: stair corner shapes need neighbouring stair context,
/// so `shape` stays `straight` here instead of being derived from neighbours.
pub fn state_for_placement_with_yaw(
    clicked_state: Option<&BlockState>,
    target_current: Option<&BlockState>,
    held_block: &BlockState,
    clicked_face: u8,
    cursor_y: f32,
    target_is_clicked: bool,
    player_yaw: f32,
) -> BlockState {
    let mut state = state_for_placement(
        clicked_state,
        target_current,
        held_block,
        clicked_face,
        cursor_y,
        target_is_clicked,
    );

    if state.name.ends_with("_stairs") {
        let direction = direction_from_yaw(player_yaw);
        state = crate::redstone::with_facing(
            &state,
            crate::math::Direction::from_3d_data(direction as i32),
        );
        let top = clicked_face == 0 || (clicked_face != 1 && cursor_y > 0.5);
        state = crate::redstone::with_property(&state, "half", if top { "top" } else { "bottom" });
        if crate::redstone::property(&state, "shape").is_some() {
            state = crate::redstone::with_property(&state, "shape", "straight");
        }
    }

    state
}

/// Placement states for a two-block door.
///
/// Returns the lower and upper halves for a held door state, or `None` when the
/// held state is not a door. `facing` follows the player yaw; `half`, `hinge`,
/// `open`, and `powered` are only touched when the state actually declares them
/// (vanilla doors always do).
///
/// This compatibility helper always uses the `left` hinge; the live placement
/// path uses [`door_states_with_hinge`] with [`door_hinge_from_click`].
pub fn door_states(held: &BlockState, player_yaw: f32) -> Option<(BlockState, BlockState)> {
    door_states_with_hinge(held, player_yaw, "left")
}

/// Vanilla door hinge side from the absolute click location.
///
/// `Direction.getClockWise` is the positive 2D-data neighbour, so the dot
/// product of the click offset with that axis is positive on the right half of
/// the target block.
pub fn door_hinge_from_click(
    player_yaw: f32,
    hit_x: f64,
    hit_z: f64,
    target_x: f64,
    target_z: f64,
) -> &'static str {
    let facing = crate::math::Direction::from_3d_data(direction_from_yaw(player_yaw) as i32);
    let right = crate::math::Direction::from_2d_data(facing.data_2d() + 1);
    let offset_x = hit_x - target_x;
    let offset_z = hit_z - target_z;
    let side = offset_x * f64::from(right.step_x()) + offset_z * f64::from(right.step_z());
    if side > 0.5 { "right" } else { "left" }
}

/// Door placement states with an explicit hinge side.
pub fn door_states_with_hinge(
    held: &BlockState,
    player_yaw: f32,
    hinge: &str,
) -> Option<(BlockState, BlockState)> {
    if !held.name.ends_with("_door") {
        return None;
    }

    let direction = crate::math::Direction::from_3d_data(direction_from_yaw(player_yaw) as i32);

    let mut lower = held.clone();
    let mut upper = held.clone();

    if crate::redstone::property(&lower, "half").is_some() {
        lower = crate::redstone::with_property(&lower, "half", "lower");
        upper = crate::redstone::with_property(&upper, "half", "upper");
    }

    lower = crate::redstone::with_facing(&lower, direction);
    upper = crate::redstone::with_facing(&upper, direction);

    if crate::redstone::property(&lower, "hinge").is_some() {
        lower = crate::redstone::with_property(&lower, "hinge", hinge);
        upper = crate::redstone::with_property(&upper, "hinge", hinge);
    }
    if crate::redstone::property(&lower, "open").is_some() {
        lower = crate::redstone::with_property(&lower, "open", "false");
        upper = crate::redstone::with_property(&upper, "open", "false");
    }
    if crate::redstone::property(&lower, "powered").is_some() {
        lower = crate::redstone::with_property(&lower, "powered", "false");
        upper = crate::redstone::with_property(&upper, "powered", "false");
    }

    debug_assert_eq!(lower.name, upper.name);

    Some((lower, upper))
}

/// Whether the collision shape of `state` placed at `block` strictly overlaps
/// an entity AABB.
///
/// Collision boxes from `block_shapes` are block-local in world units (a full
/// cube spans 0.0..1.0), so translating by the block origin is enough; they
/// must not be rescaled. Face-touching boxes do not count as an intersection.
pub fn entity_block_intersection(
    block: BlockPos,
    state: &BlockState,
    min: [f64; 3],
    max: [f64; 3],
) -> bool {
    if min[0] >= max[0] || min[1] >= max[1] || min[2] >= max[2] {
        return false;
    }
    let boxes = crate::block_shapes::collision_boxes(block_state_id(state));
    if boxes.is_empty() {
        return false;
    }
    let origin = [
        f64::from(block.x()),
        f64::from(block.y()),
        f64::from(block.z()),
    ];
    for bounds in boxes {
        let box_min = [
            origin[0] + bounds.min_x,
            origin[1] + bounds.min_y,
            origin[2] + bounds.min_z,
        ];
        let box_max = [
            origin[0] + bounds.max_x,
            origin[1] + bounds.max_y,
            origin[2] + bounds.max_z,
        ];
        if (0..3).all(|axis| box_min[axis] < max[axis] && box_max[axis] > min[axis]) {
            return true;
        }
    }
    false
}

/// Whether placing `state` at `block` would put a solid block inside any of the
/// supplied entity AABBs.
///
/// This is the counterpart of vanilla `Level.isUnobstructed`: block placement
/// is rejected when the block would overlap the player or a living entity.
pub fn placement_obstructed(
    block: BlockPos,
    state: &BlockState,
    entities: &[([f64; 3], [f64; 3])],
) -> bool {
    entities
        .iter()
        .any(|&(min, max)| entity_block_intersection(block, state, min, max))
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
    if !change.light.is_empty() {
        broadcast(&light_update_packet(
            change.chunk_x,
            change.chunk_z,
            &change.light,
        ));
    }
    for update in &change.neighbour_light {
        broadcast(&light_update_packet(
            update.chunk_x,
            update.chunk_z,
            &update.light,
        ));
    }
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
    use crate::chunk_packet::light_bytes;
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

    fn named_state(name: &str, properties: &[(&str, &str)]) -> BlockState {
        BlockState {
            name: name.into(),
            properties: properties
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect(),
        }
    }

    fn slab(kind: &str, waterlogged: bool) -> BlockState {
        named_state(
            "minecraft:oak_slab",
            &[
                ("type", kind),
                ("waterlogged", if waterlogged { "true" } else { "false" }),
            ],
        )
    }

    fn stairs(waterlogged: bool) -> BlockState {
        named_state(
            "minecraft:oak_stairs",
            &[
                ("facing", "north"),
                ("half", "bottom"),
                ("shape", "straight"),
                ("waterlogged", if waterlogged { "true" } else { "false" }),
            ],
        )
    }

    #[test]
    fn replaceable_clicked_is_replaced_in_place() {
        let air = named_state("minecraft:air", &[]);
        assert!(is_replaceable_for_item(&air, &stone()));
        let clicked = BlockPos::new(3, 4, 5);
        assert_eq!(
            placement_target(clicked, 1, &air, &stone()),
            clicked,
            "a replaceable clicked block is the placement target"
        );
    }

    #[test]
    fn solid_clicked_places_on_the_clicked_face() {
        let solid = stone();
        assert!(!is_replaceable_for_item(&solid, &stone()));
        let clicked = BlockPos::new(3, 4, 5);
        assert_eq!(
            placement_target(clicked, 1, &solid, &stone()),
            BlockPos::new(3, 5, 5),
            "UP face places one block above a solid clicked block"
        );
        assert_eq!(
            placement_target(clicked, 9, &solid, &stone()),
            BlockPos::new(3, 5, 5),
            "an invalid face falls back to the block above"
        );
    }

    #[test]
    fn same_slab_merges_into_double_and_is_not_waterlogged() {
        let bottom = slab("bottom", false);
        let held = slab("bottom", false);
        assert!(is_replaceable_for_item(&bottom, &held));
        let state = state_for_placement(Some(&bottom), Some(&bottom), &held, 1, 0.1, true);
        assert_eq!(crate::redstone::property(&state, "type"), Some("double"));
        assert_eq!(
            crate::redstone::property(&state, "waterlogged"),
            Some("false")
        );

        let double = slab("double", false);
        assert!(
            !is_replaceable_for_item(&double, &held),
            "a double slab cannot merge again"
        );
        let stone_held = stone();
        assert!(
            !is_replaceable_for_item(&bottom, &stone_held),
            "a different block cannot merge a slab"
        );
    }

    #[test]
    fn slab_type_follows_face_and_cursor_height() {
        let held = slab("bottom", false);
        let state = state_for_placement(None, None, &held, 0, 0.9, false);
        assert_eq!(crate::redstone::property(&state, "type"), Some("top"));

        let state = state_for_placement(None, None, &held, 1, 0.9, false);
        assert_eq!(crate::redstone::property(&state, "type"), Some("bottom"));

        let state = state_for_placement(None, None, &held, 3, 0.9, false);
        assert_eq!(crate::redstone::property(&state, "type"), Some("top"));

        let state = state_for_placement(None, None, &held, 3, 0.1, false);
        assert_eq!(crate::redstone::property(&state, "type"), Some("bottom"));
    }

    #[test]
    fn direction_from_yaw_maps_quarters() {
        assert_eq!(direction_from_yaw(0.0), 3, "yaw 0 -> SOUTH numeric 3");
        assert_eq!(direction_from_yaw(90.0), 4, "yaw 90 -> WEST numeric 4");
        assert_eq!(direction_from_yaw(180.0), 2, "yaw 180 -> NORTH numeric 2");
        assert_eq!(direction_from_yaw(-90.0), 5, "yaw -90 -> EAST numeric 5");
        assert_eq!(direction_from_yaw(270.0), 5, "yaw 270 -> EAST numeric 5");
        assert_eq!(direction_from_yaw(360.0), 3, "yaw 360 -> SOUTH numeric 3");
        // 45 degrees is exactly between SOUTH and WEST; round() rounds the
        // half-way quarter up, so it resolves to WEST (numeric 4).
        assert_eq!(direction_from_yaw(45.0), 4, "yaw 45 -> WEST numeric 4");
    }

    #[test]
    fn door_states_sets_lower_and_upper_halves() {
        let held = named_state(
            "minecraft:oak_door",
            &[
                ("facing", "north"),
                ("half", "lower"),
                ("hinge", "right"),
                ("open", "true"),
                ("powered", "true"),
            ],
        );
        let (lower, upper) = door_states(&held, 0.0).expect("oak door states");
        assert_eq!(crate::redstone::property(&lower, "half"), Some("lower"));
        assert_eq!(crate::redstone::property(&upper, "half"), Some("upper"));
        assert_eq!(crate::redstone::property(&lower, "facing"), Some("south"));
        assert_eq!(crate::redstone::property(&upper, "facing"), Some("south"));
        assert_eq!(lower.name, upper.name);
    }

    #[test]
    fn door_facing_follows_yaw() {
        let held = named_state(
            "minecraft:oak_door",
            &[("facing", "north"), ("half", "lower")],
        );
        for (yaw, expected) in [
            (0.0, "south"),
            (90.0, "west"),
            (180.0, "north"),
            (-90.0, "east"),
        ] {
            let (lower, upper) = door_states(&held, yaw).expect("oak door states");
            assert_eq!(
                crate::redstone::property(&lower, "facing"),
                Some(expected),
                "yaw {yaw} lower facing"
            );
            assert_eq!(
                crate::redstone::property(&upper, "facing"),
                Some(expected),
                "yaw {yaw} upper facing"
            );
        }
    }

    #[test]
    fn door_defaults_are_closed_unpowered_left_hinged() {
        let held = named_state(
            "minecraft:oak_door",
            &[
                ("facing", "north"),
                ("half", "lower"),
                ("hinge", "right"),
                ("open", "true"),
                ("powered", "true"),
            ],
        );
        let (lower, upper) = door_states(&held, 0.0).expect("oak door states");
        for state in [&lower, &upper] {
            assert_eq!(crate::redstone::property(state, "hinge"), Some("left"));
            assert_eq!(crate::redstone::property(state, "open"), Some("false"));
            assert_eq!(crate::redstone::property(state, "powered"), Some("false"));
        }
    }

    #[test]
    fn non_door_state_has_no_door_states() {
        assert!(door_states(&stone(), 0.0).is_none());
    }

    #[test]
    fn door_hinge_follows_click_side() {
        // Facing north, the clockwise/right side is east (positive X).
        assert_eq!(door_hinge_from_click(180.0, 0.2, 0.5, 0.0, 0.0), "left");
        assert_eq!(door_hinge_from_click(180.0, 0.8, 0.5, 0.0, 0.0), "right");
        // Facing east, the right side is south (positive Z).
        assert_eq!(door_hinge_from_click(-90.0, 0.5, 0.2, 0.0, 0.0), "left");
        assert_eq!(door_hinge_from_click(-90.0, 0.5, 0.8, 0.0, 0.0), "right");
    }

    #[test]
    fn door_states_with_hinge_sets_both_halves() {
        let held = named_state(
            "minecraft:oak_door",
            &[("facing", "north"), ("half", "lower"), ("hinge", "left")],
        );
        let (lower, upper) = door_states_with_hinge(&held, 0.0, "right").expect("oak door states");
        assert_eq!(crate::redstone::property(&lower, "hinge"), Some("right"));
        assert_eq!(crate::redstone::property(&upper, "hinge"), Some("right"));
    }

    #[test]
    fn stairs_facing_follows_player_yaw() {
        let held = stairs(false);
        let south = state_for_placement_with_yaw(None, None, &held, 1, 0.1, false, 0.0);
        assert_eq!(crate::redstone::property(&south, "facing"), Some("south"));
        let north = state_for_placement_with_yaw(None, None, &held, 1, 0.1, false, 180.0);
        assert_eq!(crate::redstone::property(&north, "facing"), Some("north"));
        let east = state_for_placement_with_yaw(None, None, &held, 1, 0.1, false, -90.0);
        assert_eq!(crate::redstone::property(&east, "facing"), Some("east"));
        let west = state_for_placement_with_yaw(None, None, &held, 1, 0.1, false, 90.0);
        assert_eq!(crate::redstone::property(&west, "facing"), Some("west"));
    }

    #[test]
    fn stairs_half_follows_face_and_cursor() {
        let held = stairs(false);
        let down = state_for_placement_with_yaw(None, None, &held, 0, 0.9, false, 0.0);
        assert_eq!(crate::redstone::property(&down, "half"), Some("top"));
        let up = state_for_placement_with_yaw(None, None, &held, 1, 0.9, false, 0.0);
        assert_eq!(crate::redstone::property(&up, "half"), Some("bottom"));
        let high = state_for_placement_with_yaw(None, None, &held, 3, 0.75, false, 0.0);
        assert_eq!(crate::redstone::property(&high, "half"), Some("top"));
        let low = state_for_placement_with_yaw(None, None, &held, 3, 0.25, false, 0.0);
        assert_eq!(crate::redstone::property(&low, "half"), Some("bottom"));
    }

    #[test]
    fn stairs_keep_waterlogging() {
        let held = stairs(false);
        let source = named_state("minecraft:water", &[("level", "0")]);
        let state = state_for_placement_with_yaw(None, Some(&source), &held, 3, 0.75, false, 90.0);
        assert_eq!(
            crate::redstone::property(&state, "waterlogged"),
            Some("true")
        );
        assert_eq!(crate::redstone::property(&state, "half"), Some("top"));
        assert_eq!(crate::redstone::property(&state, "facing"), Some("west"));
        assert_eq!(crate::redstone::property(&state, "shape"), Some("straight"));
    }

    #[test]
    fn slabs_ignore_yaw() {
        let held = slab("bottom", false);
        let facing_zero = state_for_placement_with_yaw(None, None, &held, 3, 0.9, false, 0.0);
        let facing_137 = state_for_placement_with_yaw(None, None, &held, 3, 0.9, false, 137.0);
        let plain = state_for_placement(None, None, &held, 3, 0.9, false);
        assert_eq!(facing_zero, facing_137);
        assert_eq!(facing_zero, plain);
    }

    #[test]
    fn water_source_waterlogs_but_flowing_water_does_not() {
        let held = slab("bottom", false);
        let source = named_state("minecraft:water", &[("level", "0")]);
        assert_eq!(
            crate::block_behaviour::fluid(&source),
            Some("minecraft:water")
        );
        let state = state_for_placement(None, Some(&source), &held, 1, 0.1, false);
        assert_eq!(
            crate::redstone::property(&state, "waterlogged"),
            Some("true")
        );

        let flowing = named_state("minecraft:water", &[("level", "1")]);
        assert_eq!(
            crate::block_behaviour::fluid(&flowing),
            Some("minecraft:flowing_water")
        );
        let state = state_for_placement(None, Some(&flowing), &held, 1, 0.1, false);
        assert_eq!(
            crate::redstone::property(&state, "waterlogged"),
            Some("false")
        );
    }

    #[test]
    fn stone_placement_inside_player_is_obstructed() {
        let player = ([-0.3, 64.0, -0.3], [0.3, 65.8, 0.3]);
        assert!(placement_obstructed(
            BlockPos::new(0, 64, 0),
            &stone(),
            &[player]
        ));
    }

    #[test]
    fn stone_placement_adjacent_to_player_is_not_obstructed() {
        let player = ([-0.3, 64.0, -0.3], [0.3, 65.8, 0.3]);
        assert!(!placement_obstructed(
            BlockPos::new(1, 64, 0),
            &stone(),
            &[player]
        ));
    }

    #[test]
    fn touching_faces_do_not_intersect() {
        // A full cube at (0,64,0) spans y 64.0..65.0; the entity begins at the
        // exact top face, so the strict comparison must not count it.
        assert!(!entity_block_intersection(
            BlockPos::new(0, 64, 0),
            &stone(),
            [-0.3, 65.0, -0.3],
            [0.3, 66.0, 0.3]
        ));
    }

    #[test]
    fn collisionless_block_does_not_obstruct() {
        let player = ([-0.3, 64.0, -0.3], [0.3, 65.8, 0.3]);
        let air = named_state("minecraft:air", &[]);
        assert!(!placement_obstructed(
            BlockPos::new(0, 64, 0),
            &air,
            &[player]
        ));
    }

    #[test]
    fn any_entity_can_obstruct() {
        let far = ([10.0, 64.0, 10.0], [10.6, 65.8, 10.6]);
        let inside = ([-0.3, 64.0, -0.3], [0.3, 65.8, 0.3]);
        assert!(!placement_obstructed(
            BlockPos::new(0, 64, 0),
            &stone(),
            &[]
        ));
        assert!(!placement_obstructed(
            BlockPos::new(0, 64, 0),
            &stone(),
            &[far]
        ));
        assert!(placement_obstructed(
            BlockPos::new(0, 64, 0),
            &stone(),
            &[far, inside]
        ));
    }

    #[test]
    fn degenerate_entity_aabb_never_intersects() {
        assert!(!entity_block_intersection(
            BlockPos::new(0, 64, 0),
            &stone(),
            [0.2, 64.2, 0.2],
            [0.2, 64.2, 0.2]
        ));
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
