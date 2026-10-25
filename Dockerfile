# Builds the server binary and the hydrated site (wasm, JS, CSS) with
# cargo-leptos, then copies both into a slim runtime image.
#
#   docker build -t account-manager .
#   docker run -p 3000:3000 -v ./config.yaml:/app/config.yaml:ro -v ./data:/app/data account-manager

FROM rust:1-trixie AS builder

ARG CARGO_LEPTOS_VERSION=0.3.11

# Built natively for each platform (x86_64 or aarch64), so tools are fetched
# for the build machine's architecture.
RUN rustup target add wasm32-unknown-unknown \
    && curl -fsSL "https://github.com/cargo-bins/cargo-binstall/releases/latest/download/cargo-binstall-$(uname -m)-unknown-linux-musl.tgz" \
        | tar -xz -C /usr/local/cargo/bin \
    && cargo binstall --no-confirm --locked cargo-leptos@${CARGO_LEPTOS_VERSION}

WORKDIR /build
COPY . .

RUN cargo leptos build --release

FROM debian:trixie-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --user-group --no-create-home --shell /usr/sbin/nologin app \
    && mkdir -p /app/data \
    && chown app:app /app/data

WORKDIR /app
COPY --from=builder /build/target/release/server /usr/local/bin/account-manager
COPY --from=builder /build/target/site /app/site
COPY config.example.yaml /app/config.example.yaml

ENV LEPTOS_ENV=PROD \
    LEPTOS_OUTPUT_NAME=account-manager \
    LEPTOS_SITE_ROOT=/app/site \
    LEPTOS_SITE_PKG_DIR=pkg \
    LEPTOS_SITE_ADDR=0.0.0.0:3000 \
    LEPTOS_RELOAD_PORT=3001 \
    APP_CONFIG=/app/config.yaml

USER app
EXPOSE 3000
# SQLite database (database.url defaults to ./data/account-manager.db).
VOLUME ["/app/data"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD ["account-manager", "healthcheck"]

ENTRYPOINT ["account-manager"]
