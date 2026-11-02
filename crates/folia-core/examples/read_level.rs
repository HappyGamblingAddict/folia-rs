use folia_core::level;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: read_level <level.dat>");
    let level = level::read(&path)?;

    println!("level_name   = {}", level.level_name);
    println!("data_version = {}", level.data_version);
    println!("game_type    = {}", level.game_type);
    println!("spawn        = {:?}", level.spawn);
    println!("day_time     = {}", level.day_time);
    println!("last_played  = {}", level.last_played);
    println!(
        "version      = {:?} ({:?})",
        level.version_name, level.version_id
    );
    println!("seed         = {:?}", level.seed);
    println!("hardcore     = {}", level.hardcore);
    println!("allow_cmds   = {}", level.allow_commands);
    Ok(())
}
