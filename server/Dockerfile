# Multi-stage build for the Carryall sync relay — a tiny E2E-ciphertext fan-out
# service. Builds only the `carryall-relay` member of the workspace.
FROM rust:1-slim AS build
WORKDIR /app
COPY . .
RUN cargo build --release -p carryall-relay

FROM debian:bookworm-slim
RUN useradd -r -s /usr/sbin/nologin carryall
COPY --from=build /app/target/release/carryall-relay /usr/local/bin/carryall-relay
# /data holds the durable backlog (carryall-relay-backlog.db, written relative to
# the working dir by default). Mount a volume here to persist offline catch-up
# across restarts; without one it still works but resets on restart.
RUN mkdir -p /data && chown carryall /data
USER carryall
WORKDIR /data
VOLUME ["/data"]
ENV CLIP_RELAY_ADDR=0.0.0.0:8787
EXPOSE 8787
CMD ["carryall-relay"]
