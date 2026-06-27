# Multi-stage build for the Carryall sync relay — a tiny, stateless, E2E-ciphertext
# fan-out service. Builds only the `carryall-relay` member of the workspace.
FROM rust:1-slim AS build
WORKDIR /app
COPY . .
RUN cargo build --release -p carryall-relay

FROM debian:bookworm-slim
RUN useradd -r -s /usr/sbin/nologin carryall
COPY --from=build /app/target/release/carryall-relay /usr/local/bin/carryall-relay
USER carryall
ENV CLIP_RELAY_ADDR=0.0.0.0:8787
EXPOSE 8787
CMD ["carryall-relay"]
