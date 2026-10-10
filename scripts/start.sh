#!/usr/bin/env bash
set -euo pipefail

echo "🚀 Sylvode Quick Start"
echo "===================="
echo ""

usage() {
  cat <<'USAGE'
Usage: scripts/start.sh [--check-config | --pull | --no-build]

Generates .env, config/sylvode.compose.toml and config/sylvode.compose.mcp.toml when they are
missing (never rewriting existing ones), then builds and starts the compose stack.

  (no argument)    build the release binaries and start the stack
  --check-config   generate any missing file, validate all three, and exit without touching
                   docker
  --pull           pull the latest base images before building
  --no-build       use the binaries already in target/release instead of building them
  -h, --help       print this help
USAGE
}

case "${1:-}" in
  '' | --check-config | --pull | --no-build) ;;
  -h | --help) usage; exit 0 ;;
  *) echo "Unknown argument: $1" >&2; usage >&2; exit 2 ;;
esac
if (($# > 1)); then
  echo "Only one argument is accepted, got: $*" >&2
  usage >&2
  exit 2
fi

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_ROOT"
MODE="${1:-}"

# The api, worker and mcp-server binaries read no environment variables; these two files are the
# only source of their configuration. `.env` survives for docker-compose's own interpolation and
# for the postgres image, which initialises itself from POSTGRES_PASSWORD.
ENV_FILE=".env"
# shellcheck source=scripts/lib/sylvode_compat.sh
source "$PROJECT_ROOT/scripts/lib/sylvode_compat.sh"
CANONICAL_APP_CONFIG="./config/sylvode.compose.toml"
LEGACY_APP_CONFIG="./config/openpr.compose.toml"
CANONICAL_MCP_CONFIG="./config/sylvode.compose.mcp.toml"
LEGACY_MCP_CONFIG="./config/openpr.compose.mcp.toml"

# A legacy file still works; picking one prints its deprecation notice on stderr.
sylvode_select_config_into APP_CONFIG "$CANONICAL_APP_CONFIG" "$LEGACY_APP_CONFIG"
sylvode_select_config_into MCP_CONFIG "$CANONICAL_MCP_CONFIG" "$LEGACY_MCP_CONFIG"
export SYLVODE_APP_CONFIG_PATH="$APP_CONFIG"
export SYLVODE_MCP_CONFIG_PATH="$MCP_CONFIG"

# Values generated below are never printed. A secret that reaches the terminal reaches the scroll
# buffer, the CI log and the screenshot. .env is written with mode 600; the two compose
# configuration files are 644 so the container user can read the bind mount, which is why the
# README recommends restricting the config/ directory instead.
random_hex() {
  if command -v openssl >/dev/null 2>&1; then
    openssl rand -hex "$1"
  else
    od -An -N "$1" -tx1 /dev/urandom | tr -d ' \n'
  fi
}

random_uuid() {
  if command -v uuidgen >/dev/null 2>&1; then
    uuidgen | tr '[:upper:]' '[:lower:]'
  else
    printf '%s-%s-%s-%s-%s\n' \
      "$(random_hex 4)" \
      "$(random_hex 2)" \
      "$(random_hex 2)" \
      "$(random_hex 2)" \
      "$(random_hex 6)"
  fi
}

upsert_env() {
  local key="$1"
  local value="$2"
  if grep -qE "^#?${key}=" "$ENV_FILE"; then
    sed -i.bak -E "s|^#?${key}=.*|${key}=${value}|" "$ENV_FILE"
    rm -f "$ENV_FILE.bak"
  else
    printf '%s=%s\n' "$key" "$value" >> "$ENV_FILE"
  fi
}

env_value() {
  local key="$1"
  sylvode_env_value "$ENV_FILE" "$key"
}

# docker-compose creates a directory when a bind mount source does not exist, and the container
# then dies on a configuration file it cannot read. Catch that here, where the message can say so.
guard_not_directory() {
  local path="$1"
  if [ -d "$path" ]; then
    echo "❌ $path is a directory."
    echo "   docker-compose creates one when it mounts a configuration file that does not exist yet."
    echo "   Remove it and rerun this script: rmdir '$path'"
    exit 1
  fi
}

guard_not_directory "$APP_CONFIG"
guard_not_directory "$MCP_CONFIG"

# ---------------------------------------------------------------------------------------------
# .env — docker-compose interpolation only.
# ---------------------------------------------------------------------------------------------
if [ ! -f "$ENV_FILE" ]; then
  echo "📝 Creating $ENV_FILE from .env.example (docker-compose values only)"
  cp .env.example "$ENV_FILE"
  chmod 600 "$ENV_FILE"
fi

# Resolved in this shell, not in command substitutions, so each legacy variable that is set
# prints its deprecation notice once per run.
sylvode_resolve_env_into SYLVODE_BIND_HOST "$ENV_FILE" SYLVODE_BIND_HOST OPENPR_BIND_HOST 127.0.0.1
sylvode_resolve_env_into SYLVODE_API_PORT "$ENV_FILE" SYLVODE_API_PORT OPENPR_API_PORT 8081
sylvode_resolve_env_into SYLVODE_FRONTEND_PORT "$ENV_FILE" SYLVODE_FRONTEND_PORT OPENPR_FRONTEND_PORT 3000
sylvode_resolve_env_into SYLVODE_MCP_PORT "$ENV_FILE" SYLVODE_MCP_PORT MCP_SERVER_PORT 8090
sylvode_resolve_env_into SYLVODE_RUNTIME_BASE "$ENV_FILE" SYLVODE_RUNTIME_BASE OPENPR_RUNTIME_BASE ''
sylvode_resolve_env_into SYLVODE_FRONTEND_DOCKERFILE "$ENV_FILE" SYLVODE_FRONTEND_DOCKERFILE OPENPR_FRONTEND_DOCKERFILE Dockerfile
sylvode_resolve_env_into SYLVODE_WEBHOOK_PORT "$ENV_FILE" SYLVODE_WEBHOOK_PORT OPENPR_WEBHOOK_PORT 9090
sylvode_resolve_env_into SYLVODE_WEBHOOK_IMAGE "$ENV_FILE" SYLVODE_WEBHOOK_IMAGE OPENPR_WEBHOOK_IMAGE ghcr.io/openprx/sylvode-webhook:latest
sylvode_resolve_env_into SYLVODE_WEBHOOK_CONFIG "$ENV_FILE" SYLVODE_WEBHOOK_CONFIG OPENPR_WEBHOOK_CONFIG ./config/sylvode-webhook.example.toml
# Ports reach generated TOML and compose port mappings, so anything but a port number is refused
# here, before any file is written.
for port_var in SYLVODE_API_PORT SYLVODE_FRONTEND_PORT SYLVODE_MCP_PORT SYLVODE_WEBHOOK_PORT; do
  port_value="${!port_var}"
  if ! [[ "$port_value" =~ ^[0-9]{1,5}$ ]] || ((10#$port_value < 1 || 10#$port_value > 65535)); then
    echo "❌ $port_var must be a port number between 1 and 65535, got '$port_value'." >&2
    exit 1
  fi
done
export SYLVODE_BIND_HOST SYLVODE_API_PORT SYLVODE_FRONTEND_PORT SYLVODE_MCP_PORT SYLVODE_RUNTIME_BASE
export SYLVODE_FRONTEND_DOCKERFILE SYLVODE_WEBHOOK_PORT SYLVODE_WEBHOOK_IMAGE SYLVODE_WEBHOOK_CONFIG

postgres_password="$(env_value POSTGRES_PASSWORD)"
if [ -z "$postgres_password" ] || [[ "$postgres_password" == *replace_with* ]]; then
  # An existing configuration file wins: postgres only runs initdb once, so regenerating the
  # password against an existing pgdata volume would lock the stack out of its own database.
  if [ -f "$APP_CONFIG" ]; then
    postgres_password="$(OPENPR_APP_CONFIG="$APP_CONFIG" python3 -c '
import os
import sys
import tomllib
from urllib.parse import urlsplit, unquote

try:
    with open(os.environ["OPENPR_APP_CONFIG"], "rb") as handle:
        url = tomllib.load(handle).get("database", {}).get("url", "")
except (OSError, tomllib.TOMLDecodeError):
    raise SystemExit(0)
password = urlsplit(url).password
if password:
    sys.stdout.write(unquote(password))
')"
  fi
  if [ -z "$postgres_password" ]; then
    postgres_password="local_$(random_hex 16)"
  fi
  upsert_env "POSTGRES_PASSWORD" "$postgres_password"
  echo "🔐 Set POSTGRES_PASSWORD in $ENV_FILE (value not printed)."
fi

# The browser origins the generated stack is reachable at, as a TOML array, for
# [flow] collab_allowed_origins. An empty list refuses every live-editing session, so a fresh
# deployment needs the origins its own frontend is served from: the published frontend port on the
# bind host. A browser sends the port in Origin unless it is the scheme default, and it sends the
# host exactly as typed, so a loopback bind lists both localhost and 127.0.0.1. A wildcard bind
# (0.0.0.0 or ::) is reachable at every address of this host, which only the operator can name for
# certain; it lists loopback plus the host's own IPv4 addresses. Never a wildcard origin.
collab_origins_toml() {
  local host="${SYLVODE_BIND_HOST#[}" port="$SYLVODE_FRONTEND_PORT" suffix="" address
  local -a hosts=()
  host="${host%]}"
  [[ "$port" == 80 ]] || suffix=":$port"
  case "$host" in
    '' | 0.0.0.0 | '::' | '*')
      hosts=(localhost 127.0.0.1)
      if command -v hostname >/dev/null 2>&1; then
        for address in $(hostname -I 2>/dev/null || true); do
          [[ "$address" =~ ^[0-9]+(\.[0-9]+){3}$ && "$address" != 127.* ]] && hosts+=("$address")
        done
      fi
      ;;
    localhost | 127.* | ::1)
      hosts=(localhost 127.0.0.1)
      [[ "$host" == ::1 ]] && hosts+=("[::1]")
      ;;
    *:*) hosts=("[$host]") ;;
    *) hosts=("$host") ;;
  esac
  local out="" entry
  for entry in "${hosts[@]}"; do
    out+="${out:+, }\"http://${entry}${suffix}\""
  done
  printf '[%s]' "$out"
}

# ---------------------------------------------------------------------------------------------
# config/sylvode.compose.toml — read by the api and the worker (legacy filename is auto-discovered).
# ---------------------------------------------------------------------------------------------
if [ ! -f "$APP_CONFIG" ]; then
  echo "📝 Generating $APP_CONFIG (api + worker)"
  umask 077
  cat > "$APP_CONFIG" <<EOF
# Sylvode configuration for the api and the worker containers, generated by scripts/start.sh.
#
# Mounted read-only at /app/config/sylvode.toml in both services; see docker-compose.yml. The
# annotated reference for every key is config/sylvode.example.toml.
#
# THIS FILE HOLDS DEPLOYMENT SECRETS. It is not committed, and the bootstrap values below are for
# local use only -- replace them before running this anywhere real.

[server]
# The api's own default is 0.0.0.0:8081, but docker-compose.yml publishes the container's 8080 and
# frontend/nginx.conf proxies to api:8080, so the api is pinned to 8080 here.
# app_name is deliberately unset: this file also serves the worker, and a shared name would make
# the worker log itself as the api. Each binary keeps its own.
# The worker never opens a listener, so bind_addr is simply unread there.
bind_addr = "0.0.0.0:8080"

[database]
# The host is the compose service name, so this URL only resolves inside the compose network. A
# host-side \`cargo run\` needs its own file pointing at localhost.
# The password must stay in step with POSTGRES_PASSWORD in .env: the postgres image initialises
# itself from that variable and only ever runs initdb once.
url = "postgres://openpr:${postgres_password}@postgres:5432/openpr"
max_connections = 20
min_connections = 2

[auth]
# Signs every access and refresh token in this deployment. Rotating it invalidates all of them.
jwt_secret = "$(random_hex 32)"
access_ttl_seconds = 1296000
refresh_ttl_seconds = 1728000
# default_author_id is deliberately unset: it must name a user that actually exists, and a
# generated UUID would only point at a row that never gets created.

[logging]
# filter is left out so each binary keeps its own default scope ("<service>=info,tower_http=info").
format = "json"
output = "stderr"

[storage]
backend = "local"
# Must match the ./uploads bind mount of the api and worker services in docker-compose.yml.
dir = "/app/uploads"

[migrations]
replay = false
continue_on_error = false

[outbound]
# Webhook and other outbound endpoints resolving to a private address are refused unless the host is
# listed here. These are the in-compose services, including the optional webhook receiver.
# Entries are matched literally: "host" or "host:port", no wildcards and no URLs.
allowed_hosts = ["webhook:9090", "api:8080", "mcp-server:8090", "frontend:80"]
allow_private = false

[flow]
# Browser origins allowed to open live-editing (collaboration) sessions. An empty list refuses every
# session, so these were derived from SYLVODE_BIND_HOST and SYLVODE_FRONTEND_PORT: the addresses
# this stack's frontend is published at. REPLACE THEM with the origin your users actually type
# (for example "https://sylvode.example.com") as soon as the frontend sits behind a domain or a
# reverse proxy. Exact scheme://host[:port] literals only; there are no wildcards.
collab_allowed_origins = $(collab_origins_toml)

EOF
  # 0644, not 0600: the containers run as their own uid and a rootless runtime maps the
  # host owner to a different one inside, so an owner-only file is unreadable there.
  # Protect the deployment directory instead (e.g. chmod 750 on the parent).
  chmod 644 "$APP_CONFIG"
fi

# ---------------------------------------------------------------------------------------------
# config/sylvode.compose.mcp.toml — read by the mcp-server (legacy filename is auto-discovered).
#
# Separate from the file above on purpose: the MCP server is the published, agent facing surface
# and it needs neither the database URL nor the signing key. It reaches the API over HTTP with a
# workspace bot token, so this file carries [logging] and [mcp] and nothing else.
# ---------------------------------------------------------------------------------------------
if [ ! -f "$MCP_CONFIG" ]; then
  echo "📝 Generating $MCP_CONFIG (mcp-server)"
  umask 077
  cat > "$MCP_CONFIG" <<EOF
# Sylvode configuration for the mcp-server container, generated by scripts/start.sh.
#
# Mounted read-only at /app/config/sylvode.toml; see docker-compose.yml. No [database] and no
# [auth] section: this service opens no database connection and signs no token, and the lazy
# validation of those sections is what lets the file leave them out entirely.
#
# THIS FILE HOLDS DEPLOYMENT SECRETS. It is not committed.

[logging]
format = "json"
output = "stderr"

[mcp]
# The api service inside the compose network, not the published host port.
api_url = "http://api:8080"

# Bootstrap placeholders that satisfy validation so the container starts. They are NOT credentials
# of a real workspace yet -- scripts/bootstrap-restaurant-demo.sh replaces both with a bot token
# it creates through the API, then recreates this service.
# bot_token is what the stdio transport and the CLI subcommands act as. docker-compose.yml serves
# http, where it is unused: every inbound request carries its own caller's bot token in
# \`Authorization: Bearer opr_...\` and the server forwards that one to the API.
bot_token = "opr_local_$(random_hex 24)"
workspace_id = "$(random_uuid)"

# There is no inbound shared secret. An http/sse request without a caller bot token is refused
# with 401; /health stays open so the healthcheck keeps working.

# transport and bind_addr are supplied as flags by docker-compose.yml, which wins over this file.
transport = "stdio"
EOF
  # 0644, not 0600: the containers run as their own uid and a rootless runtime maps the
  # host owner to a different one inside, so an owner-only file is unreadable there.
  # Protect the deployment directory instead (e.g. chmod 750 on the parent).
  chmod 644 "$MCP_CONFIG"
fi

# ---------------------------------------------------------------------------------------------
# Validation. Mirrors crates/platform/src/config/raw.rs so a bad file is reported here rather than
# as a container that exits during \`compose up\`. Nothing below prints a value.
# ---------------------------------------------------------------------------------------------
if ! OPENPR_APP_CONFIG="$APP_CONFIG" OPENPR_MCP_CONFIG="$MCP_CONFIG" OPENPR_ENV_FILE="$ENV_FILE" \
  SYLVODE_CONFIG_SCHEMA="$PROJECT_ROOT/scripts/lib/sylvode_config_schema.json" python3 -c '
import json
import os
import re
import sys
import tomllib
from urllib.parse import urlsplit, unquote

# The key sets accepted by crates/platform/src/config/raw.rs. Every Raw* struct carries
# deny_unknown_fields, so a misspelled key is a hard startup failure rather than a default. The
# table lives in scripts/lib/sylvode_config_schema.json, which a platform test compares with the
# Rust types, so a key the binaries accept cannot be refused here (or the other way round).
# Keys the binaries used to accept and now reject outright are listed there as "retired": a file
# that still carries one is not a file with a stale comment in it, the process refuses to start,
# so it is reported as the removal it is, with the edit that fixes it.
with open(os.environ["SYLVODE_CONFIG_SCHEMA"], "r", encoding="utf-8") as handle:
    _schema = json.load(handle)
SCHEMA = {name: set(keys) for name, keys in _schema["sections"].items() if "." not in name}
S3_KEYS = set(_schema["sections"]["storage.s3"])
RETIRED = {tuple(dotted.split(".", 1)): why for dotted, why in _schema["retired"].items()}
PLACEHOLDERS = ("${", "replace_with", "change-me-in-production")
UUID_RE = re.compile(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
NIL_UUID = "00000000-0000-0000-0000-000000000000"
ORIGIN_RE = re.compile(r"^https?://(\[[0-9A-Fa-f:.]+\]|[A-Za-z0-9.-]+)(:[0-9]{1,5})?$")

issues = []
# Valid but almost certainly not what the operator wants; printed, never fatal.
warnings = []


def load(path, label):
    try:
        with open(path, "rb") as handle:
            return tomllib.load(handle)
    except FileNotFoundError:
        issues.append(f"{label}: {path} does not exist; rerun scripts/start.sh to generate it")
    except IsADirectoryError:
        issues.append(f"{label}: {path} is a directory, not a file")
    except OSError as err:
        issues.append(f"{label}: {path} is unreadable ({err.strerror})")
    except tomllib.TOMLDecodeError as err:
        issues.append(f"{label}: {path} is not valid TOML ({err})")
    return None


def check_keys(data, label, path):
    for section, value in data.items():
        if section not in SCHEMA:
            issues.append(f"{label}: unknown section [{section}]; the binaries reject unknown keys")
            continue
        if not isinstance(value, dict):
            continue
        for key in value:
            retired = RETIRED.get((section, key))
            if retired is not None:
                issues.append(f"{label}: {section}.{key} is retired: {retired} of {path}")
            elif key not in SCHEMA[section]:
                issues.append(f"{label}: unknown key {section}.{key}; the binaries reject unknown keys")
    for key in data.get("storage", {}).get("s3", {}):
        if key not in S3_KEYS:
            issues.append(f"{label}: unknown key storage.s3.{key}")


def concrete(value):
    return isinstance(value, str) and value.strip() and not any(m in value for m in PLACEHOLDERS)


def get(data, section, key):
    value = data.get(section, {})
    return value.get(key) if isinstance(value, dict) else None


app_path = os.environ["OPENPR_APP_CONFIG"]
mcp_path = os.environ["OPENPR_MCP_CONFIG"]
app = load(app_path, "api/worker config")
mcp = load(mcp_path, "mcp config")

if app is not None:
    label = "api/worker config"
    check_keys(app, label, app_path)
    url = get(app, "database", "url")
    if url is None:
        issues.append(f"{label}: database.url is required by the api and the worker")
    elif not concrete(url):
        issues.append(f"{label}: database.url is empty or still a placeholder")
    secret = get(app, "auth", "jwt_secret")
    if secret is None:
        issues.append(f"{label}: auth.jwt_secret is required by the api and the worker")
    elif not concrete(secret):
        issues.append(f"{label}: auth.jwt_secret is empty or still a placeholder")
    elif len(secret.strip()) < 16:
        issues.append(f"{label}: auth.jwt_secret must be at least 16 characters")
    hosts = get(app, "outbound", "allowed_hosts")
    if hosts is not None and not isinstance(hosts, list):
        issues.append(f"{label}: outbound.allowed_hosts must be an array of strings")
    elif isinstance(hosts, list):
        for entry in hosts:
            if not isinstance(entry, str) or "://" in entry or "/" in entry or "*" in entry:
                issues.append(
                    f"{label}: outbound.allowed_hosts entry {entry!r} must be a host or host:port"
                )

    origins = get(app, "flow", "collab_allowed_origins")
    if origins is not None and not isinstance(origins, list):
        issues.append(f"{label}: flow.collab_allowed_origins must be an array of origins")
    elif not origins:
        warnings.append(
            f"{label}: [flow] collab_allowed_origins is empty, so the api refuses every live-editing "
            "(collaboration) session and Flow pages open without live editing. List the origin "
            "users reach the frontend at, for example collab_allowed_origins = "
            "[\"http://localhost:3000\"] or [\"https://sylvode.example.com\"]"
        )
    else:
        for entry in origins:
            if not isinstance(entry, str) or not ORIGIN_RE.match(entry.strip()):
                issues.append(
                    f"{label}: flow.collab_allowed_origins entry {entry!r} must be "
                    "http(s)://host[:port] with no path and no wildcard"
                )

if mcp is not None:
    label = "mcp config"
    check_keys(mcp, label, mcp_path)
    if "database" in mcp or "auth" in mcp:
        issues.append(
            f"{label}: carries [database] or [auth]; the MCP server needs neither and must not "
            "hold the database URL or the signing key"
        )
    api_url = get(mcp, "mcp", "api_url")
    if api_url is not None and not (concrete(api_url) and api_url.startswith(("http://", "https://"))):
        issues.append(f"{label}: mcp.api_url must be an http:// or https:// URL")
    # bot_token is the identity of the stdio transport and of the CLI subcommands. The compose
    # stack serves http, where it is unused and may be absent: each request brings the bot token of
    # the caller that made it. Only the shape is checked, and only when the file states one.
    token = get(mcp, "mcp", "bot_token")
    if token is not None:
        if not concrete(token):
            issues.append(f"{label}: mcp.bot_token is empty or still a placeholder")
        elif not token.strip().startswith("opr_"):
            issues.append(f"{label}: mcp.bot_token must use the opr_ token prefix")
    workspace = get(mcp, "mcp", "workspace_id")
    if workspace is None:
        issues.append(f"{label}: mcp.workspace_id is required to run the MCP server")
    elif not concrete(workspace) or not UUID_RE.match(workspace.strip()):
        issues.append(f"{label}: mcp.workspace_id must be a UUID")
    elif workspace.strip() == NIL_UUID:
        issues.append(f"{label}: mcp.workspace_id must not be the nil UUID placeholder")

# The postgres image runs initdb once, from POSTGRES_PASSWORD. If database.url disagrees with it
# the stack builds, starts, and then fails every query with an authentication error.
env_password = None
try:
    with open(os.environ["OPENPR_ENV_FILE"], "r", encoding="utf-8") as handle:
        for line in handle:
            if line.startswith("POSTGRES_PASSWORD="):
                env_password = line.split("=", 1)[1].strip()
except OSError:
    issues.append(".env: cannot be read; docker-compose needs POSTGRES_PASSWORD from it")

if not env_password:
    issues.append(".env: POSTGRES_PASSWORD is not set; the postgres service will not start")
elif app is not None:
    url = get(app, "database", "url")
    if isinstance(url, str):
        url_password = urlsplit(url).password
        if url_password is not None and unquote(url_password) != env_password:
            issues.append(
                "the password in database.url does not match POSTGRES_PASSWORD in .env; "
                "postgres would reject every connection"
            )

for warning in warnings:
    print(f"⚠️  {warning}", file=sys.stderr)

if issues:
    for issue in issues:
        print(f"❌ {issue}", file=sys.stderr)
    raise SystemExit(1)
'; then
  echo ""
  echo "Fix the values above, or delete $APP_CONFIG / $MCP_CONFIG and rerun this script to"
  echo "regenerate local bootstrap values."
  exit 1
fi

# The compose containers run as uid 1000. The configuration files are mode 644 so that the
# container user can read them at all, but when the repository belongs to another user the
# mount can still be unreadable inside the container.
file_owner="$(stat -c '%u' "$APP_CONFIG" 2>/dev/null || echo 1000)"
if [ "$file_owner" != "1000" ]; then
  echo "⚠️  $APP_CONFIG is owned by uid $file_owner but the containers run as uid 1000."
  echo "   The mount will be unreadable inside the container; chown the files to uid 1000."
  echo ""
fi

# The same uid rule applies to ./uploads, which the api and worker containers write to. The
# config check above does not cover it, and a directory the container user cannot write turns
# every upload into a 500 long after the stack looks healthy. Under rootless podman the host
# directory belongs to the invoking user, which maps to uid 0 inside the container -- so the
# owner the container sees is not the owner `ls` shows on the host.
mkdir -p uploads
uploads_owner="$(stat -c '%u' uploads 2>/dev/null || echo 1000)"
if [ "$uploads_owner" != "1000" ]; then
  echo "⚠️  ./uploads is owned by uid $uploads_owner but the containers run as uid 1000."
  echo "   Uploads will fail with 500 once the stack is up. Fix it before serving traffic:"
  echo "     rootless podman:  podman unshare chown -R 1000:1000 uploads"
  echo "     docker / root:    sudo chown -R 1000:1000 uploads"
  echo ""
fi

if [ "$MODE" = "--check-config" ]; then
  echo "✅ $APP_CONFIG, $MCP_CONFIG and $ENV_FILE are valid."
  exit 0
fi

# Check if Docker is running
if ! docker info > /dev/null 2>&1; then
  echo "❌ Docker is not running. Please start Docker first."
  exit 1
fi

# Pull latest images (optional)
if [ "$MODE" = "--pull" ]; then
  echo "📥 Pulling latest base images..."
  docker compose pull
  echo ""
fi

# Build and start services. A host that received prebuilt binaries (no Rust toolchain
# installed) passes --no-build; the binaries must then already sit in target/release.
if [ "$MODE" = "--no-build" ]; then
  missing_binaries=()
  # collab-isolated-apply-worker ships next to api in the api image (Flow collaborative writes).
  for binary in api collab-isolated-apply-worker worker mcp-server; do
    [ -x "target/release/$binary" ] || missing_binaries+=("target/release/$binary")
  done
  if [ "${#missing_binaries[@]}" -ne 0 ]; then
    echo "❌ --no-build was requested but these binaries are missing: ${missing_binaries[*]}"
    echo "Build them on a host with the same or older glibc and copy them over."
    exit 1
  fi
  echo "⏭️  Using the prebuilt binaries in target/release (--no-build)."
else
  echo "🔨 Building release binaries for Dockerfile.prebuilt..."
  cargo build --workspace --release
fi

# The binaries are linked against the host glibc, so the runtime image has to be
# at least as new. Derive it from the host release unless the caller pinned one.
if [ -z "$SYLVODE_RUNTIME_BASE" ] && [ -r /etc/os-release ]; then
  host_id=$(. /etc/os-release && echo "${ID:-}")
  host_codename=$(. /etc/os-release && echo "${VERSION_CODENAME:-}")
  if [ "$host_id" = "debian" ] && [ -n "$host_codename" ]; then
    SYLVODE_RUNTIME_BASE="debian:${host_codename}-slim"
    export SYLVODE_RUNTIME_BASE
    echo "   runtime base image: $SYLVODE_RUNTIME_BASE (matched to host)"
    # Persist it so a plain `docker compose up --build` keeps the same base.
    if ! grep -q '^SYLVODE_RUNTIME_BASE=' "$ENV_FILE" 2>/dev/null; then
      printf '\n# Runtime base image for Dockerfile.prebuilt; must ship a glibc at least as new as the host.\nSYLVODE_RUNTIME_BASE=%s\n' "$SYLVODE_RUNTIME_BASE" >> "$ENV_FILE"
    fi
  fi
fi

echo "🔨 Building and starting services..."
if [[ "${SYLVODE_COMPOSE_STAGED:-0}" == 1 ]]; then
  # podman-compose 1.3 may lose an already-created dependency while building
  # one combined start graph. Build once, then start the real services in
  # dependency order without asking the provider to reconstruct that graph.
  docker compose build
  docker compose up -d postgres
  postgres_ready=0
  for _ in $(seq 1 60); do
    if docker compose exec -T postgres pg_isready -U openpr -d openpr >/dev/null 2>&1; then
      postgres_ready=1
      break
    fi
    sleep 2
  done
  [[ $postgres_ready -eq 1 ]] || { echo "❌ PostgreSQL did not become ready during staged startup"; exit 1; }
  docker compose up -d --no-deps --no-recreate api
  api_ready=0
  staged_probe_host=$SYLVODE_BIND_HOST
  [[ $staged_probe_host == 0.0.0.0 ]] && staged_probe_host=127.0.0.1
  for _ in $(seq 1 60); do
    if curl -fsS "http://${staged_probe_host}:${SYLVODE_API_PORT}/health" >/dev/null 2>&1; then
      api_ready=1
      break
    fi
    sleep 2
  done
  [[ $api_ready -eq 1 ]] || { echo "❌ API did not become ready during staged startup"; exit 1; }
  docker compose up -d --no-deps --no-recreate worker
  docker compose up -d --no-deps --no-recreate mcp-server
  docker compose up -d --no-deps --no-recreate frontend
else
  docker compose up -d --build
fi

# Wait for the services to answer. Probing the published endpoints beats parsing
# `docker compose ps`: that output differs between podman-compose and the docker
# compose plugin, and a zero match used to read back as the string "0\n0".
echo ""
echo "⏳ Waiting for services to be ready..."
probe_host="$SYLVODE_BIND_HOST"
[ "$probe_host" = "0.0.0.0" ] && probe_host=127.0.0.1
api_probe="http://${probe_host}:${SYLVODE_API_PORT}/health"
mcp_probe="http://${probe_host}:${SYLVODE_MCP_PORT}/health"
frontend_probe="http://${probe_host}:${SYLVODE_FRONTEND_PORT}/"

max_wait=120
elapsed=0
services_ready=0
while [ $elapsed -lt $max_wait ]; do
  if curl -fsS -m 5 "$api_probe" >/dev/null 2>&1 &&
    curl -fsS -m 5 "$mcp_probe" >/dev/null 2>&1 &&
    curl -fsS -m 5 -o /dev/null "$frontend_probe" 2>/dev/null; then
    echo "✅ All services are answering!"
    services_ready=1
    break
  fi

  printf "."
  sleep 2
  elapsed=$((elapsed + 2))
done

echo ""
echo ""

if [ "$services_ready" -ne 1 ]; then
  echo "⚠️  Timeout waiting for services. Checking status..."
  docker compose ps
  echo ""
  echo "Check logs with: docker compose logs"
  exit 1
fi

# Display service URLs
echo "🎉 Sylvode is ready!"
echo ""
echo "📍 Service URLs:"
echo "  - Frontend:   http://localhost:$SYLVODE_FRONTEND_PORT"
echo "  - API:        http://localhost:$SYLVODE_API_PORT"
echo "  - MCP Server: http://localhost:$SYLVODE_MCP_PORT"
echo "  - PostgreSQL: compose network only"
echo ""
echo "📄 Configuration:"
echo "  - api + worker: $APP_CONFIG"
echo "  - mcp-server:   $MCP_CONFIG"
echo "  - compose:      $ENV_FILE"
echo "  Both configuration files contain generated secrets. Do not commit them."
echo ""
echo "📊 Service Status:"
docker compose ps
echo ""
echo "📝 Useful Commands:"
echo "  - View logs:       docker compose logs -f"
echo "  - Stop services:   docker compose down"
echo "  - Restart:         docker compose restart"
echo "  - Check config:    bash scripts/start.sh --check-config"
echo "  - Run tests:       bash scripts/e2e-test.sh"
echo ""
