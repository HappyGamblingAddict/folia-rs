//! Standalone bind/connect matrix for the Folia-RS status listener.
//!
//! Run with:
//!     cargo run -p folia-server --example bind_variants
//!
//! Every variant binds a real `TcpListener` on an ephemeral port, starts
//! `folia_core::status::serve` on a background thread, and then performs a
//! real Minecraft status exchange against it. Listener threads are detached;
//! the process exits explicitly at the end, so a blocked `accept()` can never
//! keep the harness alive.

use folia_core::protocol::{
    NextState, build_handshake, build_ping, build_status_request, parse_ping,
    parse_status_response, read_packet, write_packet,
};
use folia_core::status::{StatusConfig, serve};
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Connect and read timeout used for every attempt.
const TIMEOUT: Duration = Duration::from_secs(2);

const PING_PAYLOAD: i64 = 0x0123_4567_89AB_CDEF;

fn main() {
    println!("Folia-RS status listener bind/connect matrix");
    println!("os: {}", std::env::consts::OS);
    println!("connect/read timeout: {} ms", TIMEOUT.as_millis());
    println!();

    variant_1();
    variant_2();
    variant_3();
    variant_4();
    variant_5();
    variant_6();

    println!();
    println!("all variants finished; exiting process (listener threads stop with the process)");
    let _ = io::stdout().flush();
    std::process::exit(0);
}

fn variant_1() {
    variant_single(
        1,
        "bind 127.0.0.1:0, connect to 127.0.0.1",
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        "127.0.0.1:0",
        "127.0.0.1",
        "v1",
    );
}

fn variant_2() {
    variant_single(
        2,
        "bind 0.0.0.0:0, connect to 127.0.0.1",
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        "0.0.0.0:0",
        "127.0.0.1",
        "v2",
    );
}

fn variant_3() {
    variant_single(
        3,
        "bind [::1]:0, connect to [::1]",
        SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 0),
        "[::1]:0",
        "::1",
        "v3",
    );
}

fn variant_single(
    number: u8,
    description: &str,
    bind: SocketAddr,
    bind_label: &str,
    host: &str,
    thread_label: &'static str,
) {
    section(number, description);
    let started = Instant::now();
    println!("  bind address: {bind_label}");

    match start_listener(thread_label, bind) {
        Ok(local) => {
            println!("  local_addr: {local}");
            let report = query_target(host, local.port());
            println!("  variant result: {}", verdict(&report));
        }
        Err(error) => {
            println!("  bind FAILURE: {}", describe_io(&error));
            println!("  variant result: FAILURE");
        }
    }

    println!("  elapsed_ms: {}", started.elapsed().as_millis());
}

fn variant_4() {
    section(
        4,
        "bind [::]:0, connect to [::1] and 127.0.0.1 (dual-stack check)",
    );
    let started = Instant::now();
    println!("  bind address: [::]:0");

    match start_listener("v4", SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)) {
        Ok(local) => {
            println!("  local_addr: {local}");
            let ipv6 = query_target("::1", local.port());
            let ipv4 = query_target("127.0.0.1", local.port());

            println!("  ::1 result: {}", verdict(&ipv6));
            println!("  127.0.0.1 result: {}", verdict(&ipv4));
            if ipv4.any_success() {
                println!(
                    "  [::] dual-stack on this machine: YES (IPv4 loopback reached the [::] listener)"
                );
            } else {
                println!(
                    "  [::] dual-stack on this machine: NO (IPv4 loopback did not reach the [::] listener)"
                );
            }
        }
        Err(error) => {
            println!("  bind FAILURE: {}", describe_io(&error));
            println!("  [::] dual-stack on this machine: UNKNOWN (bind failed)");
        }
    }

    println!("  elapsed_ms: {}", started.elapsed().as_millis());
}

fn variant_5() {
    section(
        5,
        "bind 0.0.0.0:P then [::]:P on the same port; query 127.0.0.1 and ::1",
    );
    let started = Instant::now();
    println!("  bind address (IPv4): 0.0.0.0:0");

    match start_listener(
        "v5-ipv4",
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
    ) {
        Ok(local4) => {
            let port = local4.port();
            println!("  local_addr (IPv4): {local4}");
            println!("  bind address (IPv6, same port): [::]:{port}");

            match TcpListener::bind(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port)) {
                Ok(listener) => match listener.local_addr() {
                    Ok(local6) => {
                        spawn_listener("v5-ipv6", listener);
                        println!("  local_addr (IPv6): {local6}");
                    }
                    Err(error) => {
                        println!("  local_addr (IPv6) FAILURE: {}", describe_io(&error));
                    }
                },
                Err(error) => {
                    println!("  bind [::]:{port} FAILURE: {}", describe_io(&error));
                }
            }

            let ipv4 = query_target("127.0.0.1", port);
            let ipv6 = query_target("::1", port);
            println!("  127.0.0.1 result: {}", verdict(&ipv4));
            println!("  ::1 result: {}", verdict(&ipv6));
        }
        Err(error) => {
            println!("  bind 0.0.0.0:0 FAILURE: {}", describe_io(&error));
        }
    }

    println!("  elapsed_ms: {}", started.elapsed().as_millis());
}

fn variant_6() {
    section(
        6,
        "bind 127.0.0.1:0, connect to localhost:port (resolution order check)",
    );
    let started = Instant::now();
    println!("  bind address: 127.0.0.1:0");

    match start_listener("v6", SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)) {
        Ok(local) => {
            println!("  local_addr: {local}");
            let report = query_target("localhost", local.port());

            println!("  localhost resolution order:");
            for (index, address) in report.resolved.iter().enumerate() {
                println!("    {}. {address}", index + 1);
            }
            if let Some(first) = report.resolved.first() {
                println!("  first resolved address: {first}");
                let first_ok = report
                    .attempts
                    .iter()
                    .find(|attempt| attempt.address == *first)
                    .is_some_and(|attempt| attempt.success);
                println!(
                    "  first resolved address usable: {}",
                    if first_ok { "YES" } else { "NO" }
                );
            }
            match report.first_success() {
                Some(address) => println!("  first successful address: {address}"),
                None => println!("  first successful address: none"),
            }
            println!("  localhost query result: {}", verdict(&report));
        }
        Err(error) => {
            println!("  bind FAILURE: {}", describe_io(&error));
            println!("  localhost query result: FAILURE");
        }
    }

    println!("  elapsed_ms: {}", started.elapsed().as_millis());
}

/// Bind `bind`, start `serve` on a detached thread, and return the actual
/// (ephemeral) local address. The thread blocks in `accept()` until the
/// process exits, which is intentional: `serve` has no shutdown API.
fn start_listener(label: &'static str, bind: SocketAddr) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(bind)?;
    let local = listener.local_addr()?;
    spawn_listener(label, listener);
    Ok(local)
}

fn spawn_listener(label: &'static str, listener: TcpListener) {
    let config = Arc::new(StatusConfig::default());
    thread::Builder::new()
        .name(format!("status-{label}"))
        .spawn(move || {
            if let Err(error) = serve(listener, config) {
                eprintln!("[{label}] status serve stopped: {error}");
            }
        })
        .expect("spawning status listener thread");
}

struct AttemptResult {
    address: SocketAddr,
    success: bool,
}

struct QueryReport {
    resolved: Vec<SocketAddr>,
    attempts: Vec<AttemptResult>,
}

impl QueryReport {
    fn any_success(&self) -> bool {
        self.attempts.iter().any(|attempt| attempt.success)
    }

    fn first_success(&self) -> Option<SocketAddr> {
        self.attempts
            .iter()
            .find(|attempt| attempt.success)
            .map(|attempt| attempt.address)
    }
}

/// Resolve `host:port` exactly as `TcpStream::connect(("host", port))` would,
/// then try every resolved address in order with a fresh timeout. This is what
/// makes the `localhost` case observable: std would stop after the first
/// address that connects, while this harness reports every attempt.
fn query_target(host: &str, port: u16) -> QueryReport {
    println!("  query target: {host}:{port}");

    let mut report = QueryReport {
        resolved: Vec::new(),
        attempts: Vec::new(),
    };

    let addresses = match (host, port).to_socket_addrs() {
        Ok(iter) => iter.collect::<Vec<_>>(),
        Err(error) => {
            println!("    resolve FAILURE: {}", describe_io(&error));
            println!("    query result: FAILURE");
            return report;
        }
    };

    report.resolved = addresses.clone();
    if addresses.is_empty() {
        println!("    resolve FAILURE: no addresses returned");
        println!("    query result: FAILURE");
        return report;
    }

    println!(
        "    resolved: {}",
        addresses
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );

    for (index, address) in addresses.iter().enumerate() {
        let position = if index == 0 { "first" } else { "next" };
        let connect_started = Instant::now();
        match TcpStream::connect_timeout(address, TIMEOUT) {
            Ok(mut stream) => {
                let connect_ms = connect_started.elapsed().as_millis();
                if let Err(error) = stream.set_read_timeout(Some(TIMEOUT)) {
                    let detail = format!("set_read_timeout: {}", describe_io(&error));
                    println!(
                        "    attempt {address} ({position}): FAILURE after {connect_ms} ms connect: {detail}"
                    );
                    report.attempts.push(AttemptResult {
                        address: *address,
                        success: false,
                    });
                    continue;
                }
                if let Err(error) = stream.set_write_timeout(Some(TIMEOUT)) {
                    let detail = format!("set_write_timeout: {}", describe_io(&error));
                    println!(
                        "    attempt {address} ({position}): FAILURE after {connect_ms} ms connect: {detail}"
                    );
                    report.attempts.push(AttemptResult {
                        address: *address,
                        success: false,
                    });
                    continue;
                }

                let exchange_started = Instant::now();
                match run_status_query(&mut stream, host, port) {
                    Ok(json) => {
                        let exchange_ms = exchange_started.elapsed().as_millis();
                        println!(
                            "    attempt {address} ({position}): SUCCESS (connect {connect_ms} ms, exchange {exchange_ms} ms)"
                        );
                        println!("      status_json: {json}");
                        report.attempts.push(AttemptResult {
                            address: *address,
                            success: true,
                        });
                    }
                    Err(detail) => {
                        let exchange_ms = exchange_started.elapsed().as_millis();
                        println!(
                            "    attempt {address} ({position}): FAILURE (connect {connect_ms} ms, exchange {exchange_ms} ms): {detail}"
                        );
                        report.attempts.push(AttemptResult {
                            address: *address,
                            success: false,
                        });
                    }
                }
            }
            Err(error) => {
                let connect_ms = connect_started.elapsed().as_millis();
                let detail = describe_io(&error);
                println!(
                    "    attempt {address} ({position}): FAILURE (connect {connect_ms} ms): {detail}"
                );
                report.attempts.push(AttemptResult {
                    address: *address,
                    success: false,
                });
            }
        }
    }

    println!("    query result: {}", verdict(&report));
    report
}

fn run_status_query(stream: &mut TcpStream, host: &str, port: u16) -> Result<String, String> {
    let handshake = build_handshake(774, host, port, NextState::Status)
        .map_err(|error| format!("build_handshake: {}", describe_io(&error)))?;
    write_packet(stream, &handshake)
        .map_err(|error| format!("write handshake: {}", describe_io(&error)))?;
    write_packet(stream, &build_status_request())
        .map_err(|error| format!("write status request: {}", describe_io(&error)))?;

    let response = read_packet(stream)
        .map_err(|error| format!("read status response: {}", describe_io(&error)))?;
    let json = parse_status_response(&response)
        .map_err(|error| format!("parse status response: {}", describe_io(&error)))?;

    // Complete the exchange with a ping so the server does not end the
    // connection on an unexpected EOF after the status response.
    write_packet(stream, &build_ping(PING_PAYLOAD))
        .map_err(|error| format!("write ping: {}", describe_io(&error)))?;
    let pong =
        read_packet(stream).map_err(|error| format!("read pong: {}", describe_io(&error)))?;
    let echoed =
        parse_ping(&pong).map_err(|error| format!("parse pong: {}", describe_io(&error)))?;
    if echoed != PING_PAYLOAD {
        return Err(format!(
            "pong mismatch: sent {PING_PAYLOAD:#x}, got {echoed:#x}"
        ));
    }

    Ok(json)
}

fn verdict(report: &QueryReport) -> &'static str {
    if report.any_success() {
        "SUCCESS"
    } else {
        "FAILURE"
    }
}

fn describe_io(error: &io::Error) -> String {
    format!(
        "{error} [kind={:?}, raw_os_error={:?}]",
        error.kind(),
        error.raw_os_error()
    )
}

fn section(number: u8, description: &str) {
    println!();
    println!("=== Variant {number}: {description} ===");
}
