# KiwiManga — multi-stage build with cargo-chef (critical for Railway build timeouts:
# dependency layers are cached and only re-compiled when Cargo.toml/Cargo.lock change).

FROM rust:1-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
# Glob so the build works with or without a committed Cargo.lock; when the lock
# exists it is included here and honoured by `cargo chef cook`.
COPY Cargo.* ./
COPY src ./src
COPY migrations ./migrations
COPY locales ./locales
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
# libssl-dev: sqlx default features may pull openssl-sys; installing headers keeps the
# build green either way. pkg-config + ca-certificates for TLS fetches at build time.
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY Cargo.* ./
COPY src ./src
COPY migrations ./migrations
COPY locales ./locales
RUN cargo build --release --bin kiwimanga

FROM debian:bookworm-slim AS runtime
# libssl3: runtime counterpart of libssl-dev (harmless if the binary is rustls-only).
# ca-certificates: required for HTTPS (Telegram API, MangaDex, RanobeLib).
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/kiwimanga /app/kiwimanga

# Railway: attach a persistent Volume mounted at /data, otherwise bot.db + cache
# are wiped on every redeploy!
ENV DATA_DIR=/data
ENV DATABASE_URL=sqlite:/data/bot.db?mode=rwc

# Long-polling bot: no ports, no webhook.
CMD ["/app/kiwimanga"]
