# Multi-stage build for the Clip sync relay — a tiny, stateless, E2E-ciphertext
# fan-out service. Builds only the `clip-relay` member of the workspace.
FROM rust:1-slim AS build
WORKDIR /app
COPY . .
RUN cargo build --release -p clip-relay

FROM debian:bookworm-slim
RUN useradd -r -s /usr/sbin/nologin clip
COPY --from=build /app/target/release/clip-relay /usr/local/bin/clip-relay
USER clip
ENV CLIP_RELAY_ADDR=0.0.0.0:8787
EXPOSE 8787
CMD ["clip-relay"]
