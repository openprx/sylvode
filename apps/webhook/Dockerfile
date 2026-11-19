# Sylvode Webhook container image.
#
# Published as ghcr.io/openprx/sylvode-webhook and, for the whole Sylvode 1.x line, under the legacy name
# ghcr.io/openprx/openpr-webhook with the same digest. Both executables are installed: the
# canonical /app/sylvode-webhook and the legacy /app/openpr-webhook, which runs the same code and
# writes one deprecation line to stderr. There is no ENTRYPOINT, so a compose `command` that names
# either executable replaces CMD as a whole.
#
# Build it from the root of the Sylvode repository, which is the build context: the receiver is a
# member of the Sylvode cargo workspace (apps/webhook) and shares its lockfile and toolchain.
#
#   docker build -f apps/webhook/Dockerfile -t sylvode-webhook .
#
# The root .dockerignore decides what the context contains.

# Base images are pinned by digest so a rebuild of the same commit uses the same bases; update the
# tag and the digest together.
FROM rust:1-bookworm@sha256:59037199c44290f2befcdd58dcc540164763fc296950255aaefeef096a1866b0 AS builder
WORKDIR /src
# The lockfile pins every dependency; rust-toolchain.toml pins the compiler the workspace uses.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
# cargo loads the whole workspace even to build one package, so every member's manifest and
# sources are needed (the same set the root Dockerfile copies).
COPY apps apps
COPY crates crates
COPY benches benches
COPY migrations migrations
COPY spikes/collab-shared spikes/collab-shared
COPY spikes/collab-loro spikes/collab-loro
COPY spikes/collab-yrs-yjs spikes/collab-yrs-yjs
RUN cargo build --release --locked -p sylvode-webhook --bins

FROM debian:bookworm-slim@sha256:3783cc01769c7b2b1b83a5c5ad96c815348e28ed7da68e2e3687004faa906251
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /app --shell /usr/sbin/nologin sylvode
COPY --from=builder /src/target/release/sylvode-webhook /src/target/release/openpr-webhook /app/
LABEL org.opencontainers.image.title="Sylvode Webhook" \
      org.opencontainers.image.description="Webhook receiver and agent dispatcher for Sylvode (formerly OpenPR Webhook)" \
      org.opencontainers.image.source="https://github.com/openprx/sylvode" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"
WORKDIR /app
USER sylvode
EXPOSE 9090
CMD ["/app/sylvode-webhook", "/etc/sylvode-webhook/config.toml"]
