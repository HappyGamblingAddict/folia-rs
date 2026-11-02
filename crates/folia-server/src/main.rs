mod jvm;

use anyhow::{Context, Result};
use folia_core::config::ServerProperties;
use folia_core::status::{StatusConfig, serve};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;

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
    folia_core::join::configure_world_dir(root.join(&level_name));
    folia_core::entity::configure_world(root.join(&level_name));
    let seed = log_level_data(&root, &level_name);
    match folia_core::worldgen::generator::configure(seed.unwrap_or(0)) {
        Ok(()) => println!(
            "[rust] noise world generator ready (seed={})",
            seed.unwrap_or(0)
        ),
        Err(error) => eprintln!("[rust] could not initialize world generator: {error}"),
    }
    start_status_server(&properties)?;
    jvm::run(&root)
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
        accept_logins: !properties.online_mode,
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
        "[rust] note: offline-mode logins are accepted into the captured flat world; live simulation is not implemented"
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
