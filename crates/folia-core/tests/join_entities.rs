//! Live-session entity wiring: deterministic join mobs, initial metadata and
//! equipment, per-tick physics, damage/death lifecycle, and per-session
//! tracking cleanup.

use folia_core::entity::{FlatCollider, SessionEntityTracker};
use folia_core::entity_metadata::{
    CLIENTBOUND_ENTITY_EVENT, CLIENTBOUND_SET_ENTITY_DATA, CLIENTBOUND_SET_EQUIPMENT,
    CLIENTBOUND_SET_HEALTH, DATA_HEALTH, ENTITY_EVENT_DEATH, ENTITY_EVENT_DEATH_POOF,
    MetadataValue, read_entity_event, read_set_entity_data, read_set_equipment, read_set_health,
};
use folia_core::join::{self, handle_login};
use folia_core::player::PlayerSession;
use folia_core::protocol::{
    Handshake, NextState, PacketReader, PacketWriter, build_login_start, read_packet, write_packet,
};
use folia_core::world::{Dimension, WorldStorage};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const VIEW_DISTANCE: i32 = 2;
const CHUNK_COUNT: usize = ((VIEW_DISTANCE * 2 + 1) * (VIEW_DISTANCE * 2 + 1)) as usize;

fn packet_id(packet: &[u8]) -> i32 {
    let mut reader = PacketReader::new(packet);
    reader.read_varint().unwrap()
}

fn simple_packet(id: i32) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(id).unwrap();
    writer.into_inner()
}

fn empty_known_packs() -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(7).unwrap();
    writer.write_varint(0).unwrap();
    writer.into_inner()
}

fn client_information() -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(0).unwrap();
    writer.write_string("en_us").unwrap();
    writer.write_u8(6).unwrap();
    writer.write_varint(0).unwrap();
    writer.write_u8(1).unwrap();
    writer.write_u8(0).unwrap();
    writer.write_varint(0).unwrap();
    writer.write_u8(0).unwrap();
    writer.write_u8(0).unwrap();
    writer.write_varint(0).unwrap();
    writer.into_inner()
}

fn temp_world(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "folia-rs-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(path.join("region")).unwrap();
    path
}

#[test]
fn join_sends_pinned_mobs_with_metadata_and_equipment() {
    join::configure_view_distance(VIEW_DISTANCE);
    let world = temp_world("join-entities");
    {
        let mut fixture = WorldStorage::for_dimension(&world, Dimension::Overworld);
        for chunk_x in -VIEW_DISTANCE..=VIEW_DISTANCE {
            for chunk_z in -VIEW_DISTANCE..=VIEW_DISTANCE {
                fixture
                    .save_chunk(
                        &folia_core::chunk_packet::flat_chunk(chunk_x, chunk_z),
                        folia_core::anvil::Compression::Zlib,
                    )
                    .unwrap();
            }
        }
        fixture.flush().unwrap();
    }
    join::configure_world_dir(world.clone());
    join::configure_playerdata(world.join("playerdata"));

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let address = listener.local_addr().expect("address");
    let handshake = Handshake {
        protocol_version: 774,
        server_address: "localhost".into(),
        server_port: address.port(),
        next_state: NextState::Login,
    };
    let server = std::thread::spawn(move || -> io::Result<()> {
        let (stream, _) = listener.accept()?;
        handle_login(stream, &handshake)
    });

    let mut client = TcpStream::connect(address).expect("connect");
    client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    client
        .set_write_timeout(Some(Duration::from_secs(10)))
        .expect("write timeout");

    write_packet(
        &mut client,
        &build_login_start("EntityJoin", Some([0x31; 16])).unwrap(),
    )
    .unwrap();

    let login_success = read_packet(&mut client).expect("login success");
    let mut reader = PacketReader::new(&login_success);
    assert_eq!(reader.read_varint().unwrap(), 2);
    let _ = reader.read_bytes(16).unwrap();
    assert_eq!(reader.read_string().unwrap(), "EntityJoin");
    write_packet(&mut client, &simple_packet(3)).unwrap();

    loop {
        let packet = read_packet(&mut client).expect("config packet");
        if packet_id(&packet) == 14 {
            break;
        }
    }
    write_packet(&mut client, &client_information()).unwrap();
    write_packet(&mut client, &empty_known_packs()).unwrap();
    loop {
        let packet = read_packet(&mut client).expect("registry packet");
        if packet_id(&packet) == 3 {
            break;
        }
    }
    write_packet(&mut client, &simple_packet(3)).unwrap();

    let first = read_packet(&mut client).expect("play login");
    assert_eq!(packet_id(&first), 48);

    // Position comes before the chunk batch; the tracked mobs are announced
    // immediately after it and before the columns.
    let mut player_position = None;
    while player_position.is_none() {
        let packet = read_packet(&mut client).expect("position");
        if packet_id(&packet) == 70 {
            let mut reader = PacketReader::new(&packet);
            assert_eq!(reader.read_varint().unwrap(), 70);
            let _teleport_id = reader.read_varint().unwrap();
            player_position = Some((
                reader.read_f64().unwrap(),
                reader.read_f64().unwrap(),
                reader.read_f64().unwrap(),
            ));
        }
    }
    let player_position = player_position.expect("position");

    let cow = folia_core::registry::entity_type_id("minecraft:cow").unwrap();
    let pig = folia_core::registry::entity_type_id("minecraft:pig").unwrap();
    let zombie = folia_core::registry::entity_type_id("minecraft:zombie").unwrap();
    let expected_types = [cow, pig, zombie];

    let mut adds: HashMap<i32, i32> = HashMap::new();
    let mut metadata: HashMap<i32, f32> = HashMap::new();
    let mut equipment: HashSet<i32> = HashSet::new();
    let mut chunks = 0;
    while chunks < CHUNK_COUNT {
        let packet = read_packet(&mut client).expect("join packet");
        match packet_id(&packet) {
            1 => {
                let mut reader = PacketReader::new(&packet);
                assert_eq!(reader.read_varint().unwrap(), 1);
                let id = reader.read_varint().unwrap();
                let _uuid = reader.read_bytes(16).unwrap();
                let entity_type = reader.read_varint().unwrap();
                let x = reader.read_f64().unwrap();
                let y = reader.read_f64().unwrap();
                let z = reader.read_f64().unwrap();
                if expected_types.contains(&entity_type) {
                    let dx = x - player_position.0;
                    let dz = z - player_position.2;
                    assert!(
                        dx.abs() <= 8.0 && dz.abs() <= 8.0,
                        "mob {id} spawned at ({x}, {y}, {z}), player {player_position:?}"
                    );
                    assert!(
                        (y - player_position.1).abs() <= 1.5,
                        "mob {id} y={y} is not on the player's surface {:.1}",
                        player_position.1
                    );
                    adds.insert(id, entity_type);
                }
            }
            97 => {
                let (entity_id, values) = read_set_entity_data(&packet).expect("metadata");
                if adds.contains_key(&entity_id) {
                    let health = values
                        .iter()
                        .find(|value| value.index == DATA_HEALTH)
                        .and_then(|value| match &value.value {
                            MetadataValue::Float(health) => Some(*health),
                            _ => None,
                        })
                        .expect("initial health metadata");
                    metadata.insert(entity_id, health);
                }
            }
            100 => {
                let (entity_id, slots) = read_set_equipment(&packet).expect("equipment");
                if adds.contains_key(&entity_id) {
                    assert_eq!(slots.len(), 8, "full equipment snapshot");
                    assert!(slots.iter().all(|(_, stack)| stack.is_empty()));
                    equipment.insert(entity_id);
                }
            }
            44 => chunks += 1,
            _ => {}
        }
    }

    assert_eq!(adds.len(), 3, "three pinned mobs: {adds:?}");
    let types: HashSet<i32> = adds.values().copied().collect();
    assert_eq!(
        types,
        expected_types.into_iter().collect(),
        "cow, pig, and zombie are all present"
    );
    for (id, entity_type) in &adds {
        let health = *metadata.get(id).expect("metadata for every mob");
        let expected_health = if *entity_type == zombie { 20.0 } else { 10.0 };
        assert_eq!(health, expected_health, "health for entity {id}");
        assert!(equipment.contains(id), "equipment for entity {id}");
    }

    drop(client);
    server.join().expect("server thread").expect("join flow");
    fs::remove_dir_all(world).expect("remove temp world");
}

#[test]
fn session_entity_tick_syncs_falling_mob_and_clears_out_of_range() {
    let mut tracker = SessionEntityTracker::deterministic((0.5, 50.0, 0.5), 50, 7);
    let ids = tracker.tracked_ids();
    assert_eq!(ids.len(), 3);
    let mut collider = FlatCollider { ground_y: 0 };
    let tick = tracker.tick((0.5, 50.0, 0.5), &mut collider);
    let sync = tick
        .packets
        .iter()
        .find(|packet| packet_id(packet) == 35)
        .expect("position sync for a falling mob");
    let mut reader = PacketReader::new(sync);
    assert_eq!(reader.read_varint().unwrap(), 35);
    let entity_id = reader.read_varint().unwrap();
    assert!(ids.contains(&entity_id));
    let _x = reader.read_f64().unwrap();
    let y = reader.read_f64().unwrap();
    let _z = reader.read_f64().unwrap();
    assert!(y < 50.0, "gravity moved the mob down: {y}");

    // Move the viewer far away: the next tick removes every tracked id.
    let tick = tracker.tick((500.0, 50.0, 500.0), &mut collider);
    assert_eq!(tick.removed.len(), 3);
    assert!(tracker.is_empty());
    let removals = tick
        .packets
        .iter()
        .filter(|packet| packet_id(packet) == 75)
        .count();
    assert_eq!(removals, 3);
}

#[test]
fn session_tracker_damage_and_death_send_metadata_events_and_remove() {
    let mut tracker = SessionEntityTracker::deterministic((0.5, -60.0, 0.5), -60, 11);
    let zombie = tracker
        .tracked_ids()
        .into_iter()
        .find(|id| {
            tracker
                .entity(*id)
                .is_some_and(|tracked| tracked.entity.height > 1.9)
        })
        .expect("zombie");

    let hit = tracker.damage(zombie, 4.0).expect("damage");
    assert!(hit.damaged);
    assert!(!hit.died);
    let (entity_id, values) = read_set_entity_data(&hit.packets[0]).expect("health metadata");
    assert_eq!(entity_id, zombie);
    assert_eq!(
        values
            .iter()
            .find(|value| value.index == DATA_HEALTH)
            .map(|value| value.value.clone()),
        Some(MetadataValue::Float(16.0))
    );

    let lethal = tracker.damage(zombie, 40.0).expect("lethal damage");
    assert!(lethal.died);
    let (event_id, event) = read_entity_event(&lethal.packets[0]).expect("death event");
    assert_eq!((event_id, event), (zombie, ENTITY_EVENT_DEATH));

    let mut collider = FlatCollider { ground_y: -61 };
    let mut saw_poof = false;
    let mut saw_remove = false;
    for _ in 0..20 {
        let tick = tracker.tick((0.5, -60.0, 0.5), &mut collider);
        for packet in &tick.packets {
            match packet_id(packet) {
                id if id == CLIENTBOUND_ENTITY_EVENT => {
                    let (_, event) = read_entity_event(packet).expect("event");
                    if event == ENTITY_EVENT_DEATH_POOF {
                        saw_poof = true;
                    }
                }
                75 => saw_remove = true,
                _ => {}
            }
        }
    }
    assert!(saw_poof, "death animation poof event");
    assert!(saw_remove, "remove-entities packet after the animation");
    assert!(!tracker.tracks(zombie));
    assert_eq!(tracker.len(), 2);
}

#[test]
fn player_damage_uses_metadata_health_and_set_health_packet() {
    let mut session = PlayerSession::new("DamageTest".into(), [0x44; 16], 0.5, -60.0, 0.5);
    let result = join::damage_player(&mut session, 501, 5.0);
    assert!(result.damaged);
    assert!(!result.died);
    assert_eq!(session.health, 15.0);

    let metadata = result
        .packets
        .iter()
        .find(|packet| packet_id(packet) == CLIENTBOUND_SET_ENTITY_DATA)
        .expect("metadata health packet");
    let (entity_id, values) = read_set_entity_data(metadata).expect("metadata");
    assert_eq!(entity_id, 501);
    assert_eq!(
        values
            .iter()
            .find(|value| value.index == DATA_HEALTH)
            .map(|value| value.value.clone()),
        Some(MetadataValue::Float(15.0))
    );

    let health_packet = result
        .packets
        .iter()
        .find(|packet| packet_id(packet) == CLIENTBOUND_SET_HEALTH)
        .expect("set health packet");
    let (health, food, saturation) = read_set_health(health_packet).expect("set health");
    assert_eq!(health, 15.0);
    assert_eq!(food, 20);
    assert_eq!(saturation, 5.0);

    let store = folia_core::entity_metadata::living_entities();
    let stored = store.get(501).expect("metadata store entry");
    assert_eq!(stored.health, 15.0);
}

#[test]
fn tracker_retain_in_chunks_and_clear_drop_disconnected_entities() {
    let mut tracker = SessionEntityTracker::deterministic((0.5, -60.0, 0.5), -60, 3);
    assert_eq!(tracker.len(), 3);

    // Only the player's chunk and the zombie's chunk (0, -1) remain loaded;
    // the pig at (-2.5, 3.5) is in chunk (-1, 0) and must be removed.
    let chunks: HashSet<(i32, i32)> = HashSet::from([(0, 0), (0, -1)]);
    let packets = tracker.retain_in_chunks(&chunks);
    assert_eq!(packets.len(), 1);
    assert_eq!(packet_id(&packets[0]), 75);
    assert_eq!(tracker.len(), 2);

    let ids = tracker.tracked_ids();
    tracker.clear();
    assert!(tracker.is_empty());
    assert!(ids.iter().all(|id| !tracker.tracks(*id)));
}

#[test]
fn equipment_encoder_is_still_the_registered_packet() {
    let tracker = SessionEntityTracker::deterministic((0.5, -60.0, 0.5), -60, 5);
    let packets = tracker.join_packets();
    assert_eq!(packets.len(), 9);
    let equipment = packets
        .iter()
        .find(|packet| packet_id(packet) == CLIENTBOUND_SET_EQUIPMENT)
        .expect("equipment packet");
    assert_eq!(packet_id(equipment), 100);
}
