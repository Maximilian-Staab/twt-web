# syntax=docker/dockerfile:1
# Multi-stage build: cargo build in rust:1-slim, copy binary into slim runtime.
# All deps are pure-rust (rustls, no openssl), so the runtime needs no extra libs.
# `sqlx::migrate!` embeds migrations at compile time — no runtime copy needed.
#
# Base images are pinned by digest for reproducible builds. To refresh a pin:
#   docker buildx imagetools inspect <image>:<tag>

FROM rust:1-slim@sha256:b8e062b1c3a80183df83d9ba187cadd2b5ab84bc78d453dbfeb2fccd5f982247 AS build
WORKDIR /build
# cache deps first: build a dummy bin, then swap in the real sources
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src
COPY src ./src
COPY migrations ./migrations
RUN touch src/main.rs && cargo build --release --locked

FROM debian:bookworm-slim@sha256:99c0b25c46d0214d5a2091092e7908907dcaecd6a962cde0ac700acee222b6f3

# curl is only the healthcheck client; the server needs CA roots and nothing
# else at runtime. Run unprivileged: the API binds 8080 and writes no files.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 app \
    && useradd --uid 10001 --gid app --no-create-home \
         --home-dir /nonexistent --shell /usr/sbin/nologin app

COPY --from=build --chown=app:app /build/target/release/twt-cloud /usr/local/bin/twt-cloud

ENV BIND_ADDR=0.0.0.0:8080
USER app
EXPOSE 8080

# Unauthenticated liveness endpoint; poll the port the server actually binds.
HEALTHCHECK --interval=30s --timeout=3s --start-period=20s --retries=3 \
    CMD curl --fail --silent --show-error \
        "http://127.0.0.1:${BIND_ADDR##*:}/api/v1/healthz" || exit 1

CMD ["/usr/local/bin/twt-cloud"]
