# Sylvode Webhook container image.
#
# Published as ghcr.io/openprx/sylvode-webhook and, for the whole Sylvode 1.x line, under the legacy name
# ghcr.io/openprx/openpr-webhook with the same digest. Both executables are installed: the
# canonical /app/sylvode-webhook and the legacy /app/openpr-webhook, which runs the same code and
# writes one deprecation line to stderr. There is no ENTRYPOINT, so a compose `command` that names
# either executable replaces CMD as a whole.

FROM rust:1-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked --bins

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /app --shell /usr/sbin/nologin sylvode
COPY --from=builder /src/target/release/sylvode-webhook /src/target/release/openpr-webhook /app/
LABEL org.opencontainers.image.title="Sylvode Webhook" \
      org.opencontainers.image.description="Webhook receiver and agent dispatcher for Sylvode (formerly OpenPR Webhook)" \
      org.opencontainers.image.source="https://github.com/openprx/openpr-webhook" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"
WORKDIR /app
USER sylvode
EXPOSE 9090
CMD ["/app/sylvode-webhook", "/etc/sylvode-webhook/config.toml"]
