mod jvm;

use anyhow::{Context, Result};
use folia_core::config::ServerProperties;
use folia_core::region::{ChunkPos, RegionScheduler};
use folia_core::status::{StatusConfig, serve};
use std::io::BufRead;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()?;
    // Windows canonicalize() returns a \\?\ verbatim path, which the JVM's
    // class path parser does not understand.
    let root = match root.to_string_lossy().strip_prefix(r"\\?\") {
        Some(stripped) => PathBuf::from(stripped),
        None => root,
    };

    let properties = ServerProperties::read(root.join("server.properties"))
        .unwrap_or_else(|_| ServerProperties::default());
    let level_name =
        std::env::var("FOLIA_LEVEL_NAME").unwrap_or_else(|_| properties.level_name.clone());
    folia_core::join::configure_playerdata(root.join("run").join("playerdata"));
    folia_core::join::configure_operators(root.join("run").join("ops.txt"));
    folia_core::join::configure_world_dir(root.join(&level_name));
    folia_core::join::configure_view_distance(properties.view_distance);
    folia_core::join::configure_online_mode(properties.online_mode);
    folia_core::join::configure_game_mode(
        match properties.gamemode.to_ascii_lowercase().as_str() {
            "creative" => 1,
            "adventure" => 2,
            "spectator" => 3,
            _ => 0,
        },
    );
    folia_core::entity::configure_world(root.join(&level_name));
    let seed = log_level_data(&root, &level_name);
    match folia_core::worldgen::generator::configure(seed.unwrap_or(0)) {
        Ok(()) => println!(
            "[rust] noise world generator ready (seed={})",
            seed.unwrap_or(0)
        ),
        Err(error) => eprintln!("[rust] could not initialize world generator: {error}"),
    }
    // The shared overworld storage owns the scheduled/random tick queues, so
    // seed its sampler before any session can edit terrain.
    {
        let storage = folia_core::world::shared_overworld(root.join(&level_name));
        folia_core::world::lock(storage).set_tick_seed(seed.unwrap_or(0));
    }
    start_status_server(&properties)?;
    run_server(&root, &root.join(&level_name))
}

fn run_server(root: &std::path::Path, level_root: &std::path::Path) -> Result<()> {
    let level_path = level_root.join("level.dat");
    // Java is only a compatibility layer for plugin jars. With no jars (or
    // FOLIA_DISABLE_PLUGINS=1) the process runs entirely native.
    let mut plugins = jvm::PluginRuntime::start(root)?;

    let running = Arc::new(AtomicBool::new(true));
    {
        let running = Arc::clone(&running);
        ctrlc::set_handler(move || running.store(false, Ordering::SeqCst))
            .context("failed to install the Ctrl+C handler")?;
    }

    let max_ticks = std::env::var("FOLIA_MAX_TICKS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok());

    match max_ticks {
        Some(limit) => println!("[rust] native server loop running for at most {limit} ticks"),
        None => println!("[rust] native server loop running (Ctrl+C to stop)"),
    }

    start_console(Arc::clone(&running));
    let mut regions = RegionScheduler::new(8);
    regions.schedule_at(ChunkPos::new(0, 0), 5, 0, || {
        println!("[rust] spawn region task fired");
    });
    regions.schedule_global(0, 100, || {
        println!("[rust] global region heartbeat");
    });

    let mut ticks = 0u64;
    while running.load(Ordering::SeqCst) {
        let region_tasks = regions.tick_parallel(4);
        if region_tasks > 0 {
            println!("[rust] region scheduler ran {region_tasks} task(s)");
        }
        for joined in folia_core::join::take_pending_joins() {
            if let Some(plugins) = plugins.as_mut() {
                plugins.fire_player_join(&joined.name, &joined.uuid)?;
            }
        }
        if let Some(plugins) = plugins.as_mut() {
            plugins.tick()?;
        }
        ticks += 1;
        // Block-level world tick: pending neighbour updates, scheduled ticks,
        // and random ticks for every cached chunk. This precedes entity ticks,
        // matching ServerLevel.tick's block-then-entity order. The storage lock
        // is released before sleeping; sessions take it for edits and loads.
        let storage = folia_core::world::shared_overworld(level_root.to_path_buf());
        if let Err(error) =
            folia_core::tick::tick_world(&mut folia_core::world::lock(storage), ticks)
        {
            eprintln!("[rust] world tick {ticks} failed: {error}");
        }
        folia_core::entity::tick_all();
        if max_ticks.is_some_and(|limit| ticks >= limit) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    println!("[rust] server loop stopped after {ticks} ticks");
    folia_core::join::shutdown_sessions(Duration::from_secs(5));
    if let Err(error) = folia_core::world::flush_shared() {
        eprintln!("[rust] could not flush terrain: {error}");
    }
    // The level file records game time and the last played stamp; the world
    // spawn is unchanged because nothing moves it yet.
    match folia_core::level::record_session(&level_path, ticks, None) {
        Ok(level) => println!(
            "[rust] level.dat updated: day_time={} last_played={}",
            level.day_time, level.last_played
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => eprintln!("[rust] could not update {}: {error}", level_path.display()),
    }

    if let Some(mut plugins) = plugins {
        plugins.shutdown()?;
    }
    println!("[rust] shutdown complete");
    Ok(())
}

fn start_console(running: Arc<AtomicBool>) {
    println!("[rust] console ready: type 'help' for commands");
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else {
                break;
            };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line.eq_ignore_ascii_case("stop") || line.eq_ignore_ascii_case("shutdown") {
                println!("[console] stopping server");
                running.store(false, Ordering::SeqCst);
                break;
            }
            for output in folia_core::join::execute_console_command(line) {
                println!("[console] {output}");
            }
        }
        eprintln!("[console] input closed; use Ctrl+C to stop");
    });
}

fn log_level_data(root: &std::path::Path, level_name: &str) -> Option<i64> {
    let path = root.join(level_name).join("level.dat");
    match folia_core::level::read(&path) {
        Ok(level) => {
            println!(
                "[rust] level '{}' data_version={} seed={:?} game_type={}",
                level.level_name, level.data_version, level.seed, level.game_type
            );
            level.seed
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!(
                "[rust] no level.dat at {} (world loading not started yet)",
                path.display()
            );
            None
        }
        Err(error) => {
            eprintln!("[rust] could not read {}: {error}", path.display());
            None
        }
    }
}

fn start_status_server(properties: &ServerProperties) -> Result<()> {
    let port = properties.server_port;
    let mut listeners: Vec<(TcpListener, &'static str)> = Vec::new();

    if let Ok(bind) = std::env::var("FOLIA_BIND") {
        let listener = TcpListener::bind(&bind)
            .with_context(|| format!("FOLIA_BIND={bind} could not be bound"))?;
        listeners.push((listener, "FOLIA_BIND"));
    } else {
        let ip = properties.server_ip.trim();
        if ip.is_empty() || ip == "0.0.0.0" {
            // IPv4-only by default. Binding both 0.0.0.0 and [::] on this
            // Windows setup caused loopback status probes to time out, so
            // IPv6 is opt-in (server-ip=:: or FOLIA_BIND).
            let listener = TcpListener::bind(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                port,
            ))
            .with_context(|| {
                format!(
                    "could not bind 0.0.0.0:{port}. Is another server already using this port? \
                     Stop it or change server-port in server.properties."
                )
            })?;
            listeners.push((listener, "IPv4"));
        } else if ip == "::" || ip == "[::]" {
            let listener =
                TcpListener::bind(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port))
                    .with_context(|| format!("could not bind [::]:{port}"))?;
            listeners.push((listener, "IPv6"));
        } else {
            let listener = TcpListener::bind(bind_address(ip, port))
                .with_context(|| format!("could not bind server-ip={ip}:{port}"))?;
            listeners.push((listener, "configured"));
        }
    }

    let config = Arc::new(StatusConfig {
        motd: properties.motd.clone(),
        max_players: properties.max_players,
        online_players: 0,
        accept_logins: true,
        ..StatusConfig::default()
    });

    for (listener, label) in listeners {
        let address = listener.local_addr()?;
        println!("[rust] status server listening on {address} ({label})");

        let config = Arc::clone(&config);
        std::thread::spawn(move || {
            if let Err(error) = serve(listener, config) {
                eprintln!("[status] server stopped: {error}");
            }
        });
    }

    if let Some(ip) = local_ipv4() {
        println!(
            "[rust] LAN address: {ip}:{port} (allow TCP {port} through Windows Firewall for other devices)"
        );
    }
    println!(
        "[rust] note: offline-mode logins get generated terrain streamed around them; block edits and live simulation are not implemented"
    );

    Ok(())
}

fn local_ipv4() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    Some(socket.local_addr().ok()?.ip())
}

fn bind_address(ip: &str, port: u16) -> String {
    if ip.contains(':') && !ip.starts_with('[') {
        format!("[{ip}]:{port}")
    } else {
        format!("{ip}:{port}")
    }
}
