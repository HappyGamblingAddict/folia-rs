use folia_core::player::PlayerSession;

fn main() -> std::io::Result<()> {
    let directory = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "run/playerdata".to_owned());
    let uuid = [0x24u8; 16];
    match PlayerSession::load(&directory, uuid)? {
        Some(session) => {
            println!(
                "{} {} pos=({:.2}, {:.2}, {:.2}) rot=({:.2}, {:.2}) on_ground={}",
                session.name,
                folia_core::player::uuid_string(&session.uuid),
                session.x,
                session.y,
                session.z,
                session.yaw,
                session.pitch,
                session.on_ground
            );
        }
        None => println!("no playerdata"),
    }
    Ok(())
}
