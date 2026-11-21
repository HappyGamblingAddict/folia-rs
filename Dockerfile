# syntax=docker/dockerfile:1
#
# Multi-stage build for the native (no-Java) folia-server release binary.
#
# IMPORTANT: crates/folia-server/src/main.rs resolves its data root from the
# compile-time env!("CARGO_MANIFEST_DIR"). Building the source at /data bakes
# that root into the binary so the runtime /data volume is where server.properties,
# <level-name>/ (world + region files) and run/ (playerdata, ops.txt) are read from.
# Do not change the builder WORKDIR without changing the runtime data root.

# ---- Build stage ----------------------------------------------------------
FROM rust:1.100-bookworm AS builder

WORKDIR /data
COPY . .
RUN cargo build --release --locked -p folia-server

# ---- Runtime stage --------------------------------------------------------
FROM debian:bookworm-slim AS runtime

ENV DEBIAN_FRONTEND=noninteractive \
    FOLIA_DISABLE_PLUGINS=1

# Fixed uid/gid keep the /data volume ownership predictable.
RUN groupadd --system --gid 1000 folia \
 && useradd --system --uid 1000 --gid 1000 --home-dir /data \
      --no-create-home --shell /usr/sbin/nologin folia \
 && mkdir -p /data /etc/folia \
 && chown -R folia:folia /data

COPY --from=builder /data/target/release/folia-server /usr/local/bin/folia-server
COPY --chown=folia:folia --from=builder /data/server.properties.example /etc/folia/server.properties.example
RUN chmod 0755 /usr/local/bin/folia-server

VOLUME ["/data"]
WORKDIR /data

# Default 25565/TCP. Override the bind with FOLIA_BIND or server-port in
# server.properties; if you change the port, adjust the healthcheck too.
EXPOSE 25565

# Basic TCP liveness probe: the healthcheck only confirms the status socket is
# accepting connections. It does not perform a full Minecraft status handshake,
# which would need a protocol client; a portable shell probe is not available.
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
    CMD ["/bin/bash", "-c", "exec 3<>/dev/tcp/127.0.0.1/25565"]

USER folia

# Seed a working offline-mode server.properties into a fresh /data volume on
# first run, then exec the binary as PID 1. Note: graceful shutdown is wired to
# SIGINT / the console `stop` command, not SIGTERM, so `docker stop` relies on
# the stop timeout to flush terrain (same limitation as the systemd unit).
# Override defaults with e.g.:
#   -e FOLIA_DISABLE_PLUGINS=0   (requires a JVM-enabled image to load plugins)
#   -e FOLIA_BIND=127.0.0.1:25566
ENTRYPOINT ["/bin/sh", "-c", "if [ ! -f /data/server.properties ]; then cp /etc/folia/server.properties.example /data/server.properties; fi; exec /usr/local/bin/folia-server \"$@\"", "folia-server"]
