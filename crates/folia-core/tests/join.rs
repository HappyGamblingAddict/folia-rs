use folia_core::block_update::block_pos_long;
use folia_core::entity::{self, Entity};
use folia_core::inventory::{self, ItemStack};
use folia_core::join::handle_login;
use folia_core::protocol::{
    Handshake, NextState, PacketReader, PacketWriter, build_login_start, read_packet, write_packet,
};
use folia_core::world::{Dimension, WorldStorage};
use std::fs;
use std::io;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn login_reaches_play_state_with_captured_world() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../world/region");
    if !fixture.is_dir() {
        eprintln!("skipping: no world region fixture");
        return;
    }
    let temp_world = std::env::temp_dir().join(format!(
        "folia-rs-join-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(temp_world.join("region")).unwrap();
    for entry in fs::read_dir(&fixture).unwrap() {
        let entry = entry.unwrap();
        if entry.path().is_file() {
            fs::copy(
                entry.path(),
                temp_world.join("region").join(entry.file_name()),
            )
            .unwrap();
        }
    }
    folia_core::join::configure_world_dir(temp_world.clone());
    folia_core::join::configure_playerdata(temp_world.join("playerdata"));
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
        &build_login_start("JoinTest", Some([0x11; 16])).unwrap(),
    )
    .unwrap();

    let login_success = read_packet(&mut client).expect("login success");
    let mut reader = PacketReader::new(&login_success);
    assert_eq!(reader.read_varint().unwrap(), 2);
    let _ = reader.read_bytes(16).unwrap();
    assert_eq!(reader.read_string().unwrap(), "JoinTest");

    write_packet(&mut client, &simple_packet(3)).unwrap();

    // Configuration: read brand/features/known-packs, then select no packs.
    loop {
        let packet = read_packet(&mut client).expect("config packet");
        let id = packet_id(&packet);
        if id == 14 {
            break;
        }
        assert!(id == 1 || id == 12, "unexpected config id {id}");
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
    assert_eq!(packet_id(&first), 48, "first play packet is Join Game");

    let mut saw_chunk = false;
    let mut chunk_count = 0;
    loop {
        let packet = read_packet(&mut client).expect("play packet");
        match packet_id(&packet) {
            44 => {
                saw_chunk = true;
                chunk_count += 1;
            }
            70 => break,
            _ => {}
        }
    }
    assert!(saw_chunk, "live play stream contains chunk packets");
    assert_eq!(chunk_count, 49, "7x7 live chunk radius");

    let mut carried = PacketWriter::new();
    carried
        .write_varint(folia_core::inventory::SERVERBOUND_SET_CARRIED_ITEM)
        .unwrap();
    carried.write_bytes(&1i16.to_be_bytes());
    write_packet(&mut client, &carried.into_inner()).expect("select hotbar slot 1");
    loop {
        let packet = read_packet(&mut client).expect("held slot packet");
        if packet_id(&packet) == 103 {
            let mut reader = PacketReader::new(&packet);
            assert_eq!(reader.read_varint().unwrap(), 103);
            assert_eq!(reader.read_varint().unwrap(), 1);
            break;
        }
    }

    let mut action = PacketWriter::new();
    action
        .write_varint(folia_core::block_update::SERVERBOUND_PLAYER_ACTION)
        .unwrap();
    action
        .write_varint(folia_core::block_update::ACTION_STOP_DESTROY_BLOCK)
        .unwrap();
    action.write_i64(block_pos_long(0, -61, 0)).unwrap();
    action.write_u8(1).unwrap();
    action.write_varint(0).unwrap();
    write_packet(&mut client, &action.into_inner()).expect("send block break");

    let mut block_update = None;
    let mut light_update = None;
    while block_update.is_none() || light_update.is_none() {
        let packet = read_packet(&mut client).expect("block/light update");
        match packet_id(&packet) {
            8 => block_update = Some(packet),
            47 => light_update = Some(packet),
            _ => {}
        }
    }

    let block_update = block_update.expect("block update");
    let mut reader = PacketReader::new(&block_update);
    assert_eq!(reader.read_varint().unwrap(), 8);
    assert_eq!(reader.read_i64().unwrap(), block_pos_long(0, -61, 0));
    assert_eq!(reader.read_varint().unwrap(), 0, "broken block becomes air");

    let light_update = light_update.expect("light update");
    let mut reader = PacketReader::new(&light_update);
    assert_eq!(reader.read_varint().unwrap(), 47);
    assert_eq!(reader.read_varint().unwrap(), 0);
    assert_eq!(reader.read_varint().unwrap(), 0);

    // Place stone on top of the broken block using ServerboundUseItemOn.
    let mut use_item = PacketWriter::new();
    use_item
        .write_varint(folia_core::block_update::SERVERBOUND_USE_ITEM_ON)
        .unwrap();
    use_item.write_varint(0).unwrap(); // main hand
    use_item.write_i64(block_pos_long(0, -61, 0)).unwrap();
    use_item.write_varint(1).unwrap(); // UP
    use_item.write_f32(0.5).unwrap();
    use_item.write_f32(1.0).unwrap();
    use_item.write_f32(0.5).unwrap();
    use_item.write_u8(0).unwrap(); // inside
    use_item.write_u8(0).unwrap(); // world border
    use_item.write_varint(0).unwrap();
    write_packet(&mut client, &use_item.into_inner()).expect("send block place");

    let mut placed_update = None;
    let mut placed_light = None;
    let mut placement_content = None;
    while placed_update.is_none() || placed_light.is_none() || placement_content.is_none() {
        let packet = read_packet(&mut client).expect("placement updates");
        match packet_id(&packet) {
            8 => placed_update = Some(packet),
            47 => placed_light = Some(packet),
            18 => placement_content = Some(packet),
            _ => {}
        }
    }
    let mut reader = PacketReader::new(placed_update.as_ref().unwrap());
    assert_eq!(reader.read_varint().unwrap(), 8);
    assert_eq!(reader.read_i64().unwrap(), block_pos_long(0, -60, 0));
    assert_eq!(reader.read_varint().unwrap(), 10, "placed block is dirt");
    let mut reader = PacketReader::new(placed_light.as_ref().unwrap());
    assert_eq!(reader.read_varint().unwrap(), 47);
    assert_eq!(reader.read_varint().unwrap(), 0);
    assert_eq!(reader.read_varint().unwrap(), 0);

    let (menu, cursor) = parse_container_content(placement_content.as_ref().unwrap());
    assert_eq!(menu[37].item, "minecraft:dirt");
    assert_eq!(menu[37].count, 63, "placement consumed one dirt");
    assert!(cursor.is_empty());

    // Creative slot update: replace hotbar 1 with five dirt.
    let creative = creative_slot_packet(37, ItemStack::new("minecraft:dirt", 5));
    write_packet(&mut client, &creative).expect("creative slot");
    let creative_content = read_until_content(&mut client);
    let (menu, cursor) = parse_container_content(&creative_content);
    assert_eq!(menu[37].item, "minecraft:dirt");
    assert_eq!(menu[37].count, 5);
    assert!(cursor.is_empty());

    // Pick up the stone stack from hotbar 0.
    let click = container_click_packet(36, 0, 0, ItemStack::new("minecraft:stone", 64));
    write_packet(&mut client, &click).expect("pickup click");
    let pickup_content = read_until_content(&mut client);
    let (menu, cursor) = parse_container_content(&pickup_content);
    assert!(menu[36].is_empty());
    assert_eq!(cursor.item, "minecraft:stone");
    assert_eq!(cursor.count, 64);

    // Swap the cursor with the dirt stack in hotbar 1.
    let click = container_click_packet(37, 0, 0, ItemStack::new("minecraft:dirt", 5));
    write_packet(&mut client, &click).expect("swap click");
    let swap_content = read_until_content(&mut client);
    let (menu, cursor) = parse_container_content(&swap_content);
    assert_eq!(menu[37].item, "minecraft:stone");
    assert_eq!(menu[37].count, 64);
    assert_eq!(cursor.item, "minecraft:dirt");
    assert_eq!(cursor.count, 5);

    // Entity visibility: add, move, remove.
    let mut entity_uuid = [0u8; 16];
    entity_uuid[15] = 0x99;
    entity::spawn(Entity::new(9001, entity_uuid, 5, 2.0, -60.0, 0.0));
    let add = read_until_packet(&mut client, 1);
    let mut reader = PacketReader::new(&add);
    assert_eq!(reader.read_varint().unwrap(), 1);
    assert_eq!(reader.read_varint().unwrap(), 9001);
    let _uuid = reader.read_bytes(16).unwrap();
    assert_eq!(reader.read_varint().unwrap(), 5, "armor stand type");
    assert_eq!(reader.read_f64().unwrap(), 2.0);
    assert_eq!(reader.read_f64().unwrap(), -60.0);
    assert_eq!(reader.read_f64().unwrap(), 0.0);

    let sync = entity::move_entity(9001, 4.0, -60.0, 0.0).expect("move entity");
    let mut reader = PacketReader::new(&sync);
    assert_eq!(reader.read_varint().unwrap(), 35);
    assert_eq!(reader.read_varint().unwrap(), 9001);
    assert_eq!(reader.read_f64().unwrap(), 4.0);
    assert_eq!(reader.read_f64().unwrap(), -60.0);
    assert_eq!(reader.read_f64().unwrap(), 0.0);
    let _ = read_until_packet(&mut client, 35);

    let remove = entity::remove(9001).expect("remove entity");
    let mut reader = PacketReader::new(&remove);
    assert_eq!(reader.read_varint().unwrap(), 75);
    assert_eq!(reader.read_varint().unwrap(), 1);
    assert_eq!(reader.read_varint().unwrap(), 9001);
    let _ = read_until_packet(&mut client, 75);

    // Per-tick entity physics through WorldStorage: spawn a stand high above
    // the flat world, run 90 server ticks, and verify the position syncs land
    // it on the grass surface at y=-60.
    entity::configure_world(temp_world.clone());
    let mut falling_uuid = [0u8; 16];
    falling_uuid[15] = 0xAA;
    entity::spawn(Entity::armor_stand(9003, falling_uuid, 8.0, 50.0, 8.0));
    let _ = read_until_packet(&mut client, 1);

    let mut landed_y = None;
    for _ in 0..90 {
        for packet in entity::tick_all() {
            let mut reader = PacketReader::new(&packet);
            if reader.read_varint().unwrap() != 35 {
                continue;
            }
            if reader.read_varint().unwrap() != 9003 {
                continue;
            }
            let _x = reader.read_f64().unwrap();
            let y = reader.read_f64().unwrap();
            let _z = reader.read_f64().unwrap();
            let _vx = reader.read_f64().unwrap();
            let _vy = reader.read_f64().unwrap();
            let _vz = reader.read_f64().unwrap();
            let _yaw = reader.read_f32().unwrap();
            let _pitch = reader.read_f32().unwrap();
            if reader.read_u8().unwrap() != 0 {
                landed_y = Some(y);
            }
        }
    }
    let landed_y = landed_y.expect("on-ground position sync");
    assert!(
        (landed_y - (-60.0)).abs() < 1.0e-9,
        "armor stand landed at {landed_y}, expected -60"
    );
    entity::remove(9003);

    drop(client);
    server.join().expect("server thread").expect("join flow");

    let mut persisted = WorldStorage::for_dimension(&temp_world, Dimension::Overworld);
    let chunk = persisted
        .load_chunk(0, 0)
        .expect("load persisted chunk")
        .expect("chunk exists");
    assert_eq!(
        chunk.block_state(0, -61, 0).unwrap().name,
        "minecraft:air",
        "broken block persisted through WorldStorage"
    );
    assert_eq!(
        chunk.block_state(0, -60, 0).unwrap().name,
        "minecraft:dirt",
        "placed block persisted through WorldStorage"
    );
    let playerdata =
        folia_core::player::PlayerSession::load(temp_world.join("playerdata"), [0x11; 16])
            .expect("load playerdata")
            .expect("playerdata exists");
    assert_eq!(playerdata.selected_slot, 1);
    assert!(playerdata.inventory[0].is_empty());
    assert_eq!(playerdata.inventory[1].item, "minecraft:stone");
    assert_eq!(playerdata.inventory[1].count, 64);
    assert_eq!(playerdata.cursor.item, "minecraft:dirt");
    assert_eq!(playerdata.cursor.count, 5);
    fs::remove_dir_all(temp_world).expect("remove temp world");
}

fn read_until_content(client: &mut TcpStream) -> Vec<u8> {
    loop {
        let packet = read_packet(client).expect("container content");
        if packet_id(&packet) == 18 {
            return packet;
        }
    }
}

fn read_until_packet(client: &mut TcpStream, id: i32) -> Vec<u8> {
    loop {
        let packet = read_packet(client).expect("expected packet");
        if packet_id(&packet) == id {
            return packet;
        }
    }
}

fn parse_container_content(packet: &[u8]) -> (Vec<ItemStack>, ItemStack) {
    let mut reader = PacketReader::new(packet);
    assert_eq!(reader.read_varint().unwrap(), 18);
    assert_eq!(reader.read_varint().unwrap(), 0);
    assert_eq!(reader.read_varint().unwrap(), 0);
    let count = reader.read_varint().unwrap();
    let mut items = Vec::new();
    for _ in 0..count {
        items.push(inventory::read_item_stack(&mut reader).expect("item stack"));
    }
    let cursor = inventory::read_item_stack(&mut reader).expect("cursor");
    (items, cursor)
}

fn creative_slot_packet(slot: i16, stack: ItemStack) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer
        .write_varint(inventory::SERVERBOUND_SET_CREATIVE_MODE_SLOT)
        .unwrap();
    writer.write_i16(slot).unwrap();
    inventory::write_item_stack(&mut writer, &stack);
    writer.into_inner()
}

fn container_click_packet(slot: i16, button: u8, click_type: i32, carried: ItemStack) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer
        .write_varint(inventory::SERVERBOUND_CONTAINER_CLICK)
        .unwrap();
    writer.write_varint(0).unwrap(); // container id
    writer.write_varint(0).unwrap(); // state id
    writer.write_i16(slot).unwrap();
    writer.write_u8(button).unwrap();
    writer.write_varint(click_type).unwrap();
    writer.write_varint(0).unwrap(); // changed slots
    if carried.is_empty() {
        writer.write_u8(0).unwrap();
    } else {
        writer.write_u8(1).unwrap();
        writer
            .write_varint(inventory::item_id(&carried.item).expect("known item"))
            .unwrap();
        writer.write_varint(carried.count).unwrap();
        writer.write_varint(0).unwrap();
        writer.write_varint(0).unwrap();
    }
    writer.into_inner()
}

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
    writer.write_u8(2).unwrap();
    writer.write_varint(0).unwrap();
    writer.write_u8(1).unwrap();
    writer.write_u8(0).unwrap();
    writer.write_varint(0).unwrap();
    writer.write_u8(0).unwrap();
    writer.write_u8(0).unwrap();
    writer.write_varint(0).unwrap();
    writer.into_inner()
}
