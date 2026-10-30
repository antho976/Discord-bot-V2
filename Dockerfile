# One image: the bot (Rust, Discord + dashboard), Caddy in front, and Relay's rendezvous and cloud
# engine beside it (deploy/start.sh). Same layout as antho-s-bot's image, so it can replace it on
# the same Render service and disk.

ARG RELAY_IMAGE=ghcr.io/antho976/relay-server:latest
ARG CADDY_IMAGE=caddy:2.8.4

FROM ${RELAY_IMAGE} AS relay
FROM ${CADDY_IMAGE} AS caddy
# Render refuses to exec binaries with file capabilities; Caddy binds $PORT, so a plain copy works.
RUN cp /usr/bin/caddy /caddy

FROM rust:1-slim-trixie AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo build --release -p bot --locked && strip target/release/bot

FROM debian:trixie-slim AS runner
ENV PORT=3000 \
    DATABASE_PATH=/data/bot-v2.db \
    XDG_DATA_HOME=/data/relay \
    XDG_RUNTIME_DIR=/tmp/relay-run \
    RUST_LOG=info
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*
COPY --from=relay /usr/local/bin/relay /usr/local/bin/relay
COPY --from=caddy /caddy /usr/local/bin/caddy
COPY --from=builder /src/target/release/bot /usr/local/bin/bot
COPY deploy /app/deploy
EXPOSE 3000
CMD ["/app/deploy/start.sh"]
