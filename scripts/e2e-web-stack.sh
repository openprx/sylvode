#!/usr/bin/env bash
# Starts and stops the local stack the browser E2E specs in tests/e2e/web run against.
# CI (the "Web E2E" job in .github/workflows/ci.yml) and local runs use this same script.
#
#   scripts/e2e-web-stack.sh up     # start; prints the environment the specs need
#   scripts/e2e-web-stack.sh down   # stop what `up` started and remove its state
#
# The stack:
# - a scratch database created next to OPENPR_TEST_DATABASE_URL (dropped again by `down`);
# - the release API (target/release/api), which must have collab-isolated-apply-worker next to
#   it: the API spawns that binary for every Flow collaborative write, so `up` refuses to start
#   without it;
# - a generated configuration with a random jwt_secret, uploads in the state directory, and
#   [flow] collab_allowed_origins set to the UI origin;
# - a Bun same-origin proxy (scripts/lib/e2e-web-proxy.ts) serving frontend/build and forwarding
#   /api/* including WebSocket upgrades, with Origin passed through;
# - an instance admin registered as the first account with a random password.
#
# Inputs (environment):
#   OPENPR_TEST_DATABASE_URL  required. A PostgreSQL URL whose role may CREATE/DROP DATABASE; the
#                             last path segment is swapped for the scratch database name.
#   E2E_API_PORT              API port, default 18081.
#   E2E_WEB_PORT              UI port, default 18080.
#   E2E_BIN_DIR               directory with api and collab-isolated-apply-worker,
#                             default <repo>/target/release.
#   E2E_BUILD_DIR             frontend build output, default <repo>/frontend/build.
#   E2E_STACK_DIR             state directory (config, logs, pids, env file), default
#                             ${RUNNER_TEMP:-${TMPDIR:-/tmp}}/sylvode-e2e-web-<E2E_API_PORT>.
#
# Outputs: `up` writes <state>/env (shell `export` lines for BASE_URL, E2E_DATABASE_URL,
# ADMIN_EMAIL, ADMIN_PASSWORD) and prints its path; source it before running Playwright. When
# GITHUB_ENV is set the same variables are appended there for the following CI steps. The API
# log is <state>/api.log and the proxy log <state>/proxy.log.
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
API_PORT="${E2E_API_PORT:-18081}"
WEB_PORT="${E2E_WEB_PORT:-18080}"
BIN_DIR="${E2E_BIN_DIR:-$ROOT_DIR/target/release}"
BUILD_DIR="${E2E_BUILD_DIR:-$ROOT_DIR/frontend/build}"
STACK_DIR="${E2E_STACK_DIR:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/sylvode-e2e-web-$API_PORT}"
WORKER_NAME=collab-isolated-apply-worker

die() {
  echo "e2e-web-stack: $*" >&2
  exit 1
}

usage() {
  echo "usage: $(basename "$0") up|down" >&2
  exit 2
}

random_hex() {
  od -An -N"$1" -tx1 /dev/urandom | tr -d ' \n'
}

check_port() {
  local name=$1 value=$2
  if ! [[ $value =~ ^[0-9]{1,5}$ ]] || ((10#$value < 1 || 10#$value > 65535)); then
    die "$name must be a port number between 1 and 65535, got '$value'"
  fi
}

# Swaps the database name (last path segment, query string kept) of a PostgreSQL URL.
with_database() {
  local url=$1 name=$2 base query=''
  base=${url%%\?*}
  if [[ $url == *\?* ]]; then
    query="?${url#*\?}"
  fi
  [[ $base =~ ^postgres(ql)?://[^/]+/[^/]*$ ]] || die "OPENPR_TEST_DATABASE_URL must look like postgres://user:password@host:port/database"
  printf '%s/%s%s' "${base%/*}" "$name" "$query"
}

pid_alive() {
  local file=$1 pid
  [[ -f $file ]] || return 1
  pid=$(<"$file")
  [[ -n $pid ]] && kill -0 "$pid" 2>/dev/null
}

stop_pid() {
  local file=$1 pid
  pid_alive "$file" || return 0
  pid=$(<"$file")
  kill "$pid" 2>/dev/null || true
  for _ in $(seq 1 50); do
    kill -0 "$pid" 2>/dev/null || return 0
    sleep 0.1
  done
  kill -9 "$pid" 2>/dev/null || true
}

wait_http_200() {
  local url=$1 pid_file=$2 log=$3 what=$4
  for _ in $(seq 1 120); do
    if [[ $(curl -s -o /dev/null -w '%{http_code}' "$url" || true) == 200 ]]; then
      return 0
    fi
    if ! pid_alive "$pid_file"; then
      echo "e2e-web-stack: $what exited before $url answered 200; last log lines:" >&2
      tail -n 40 "$log" >&2 || true
      return 1
    fi
    sleep 0.5
  done
  echo "e2e-web-stack: $url did not answer 200 within 60s; last $what log lines:" >&2
  tail -n 40 "$log" >&2 || true
  return 1
}

cmd_down() {
  if [[ ! -d $STACK_DIR ]]; then
    echo "e2e-web-stack: nothing to stop ($STACK_DIR does not exist)"
    return 0
  fi
  stop_pid "$STACK_DIR/proxy.pid"
  stop_pid "$STACK_DIR/api.pid"
  if [[ -f $STACK_DIR/admin_database_url && -f $STACK_DIR/database_name ]]; then
    local admin_url db_name
    admin_url=$(<"$STACK_DIR/admin_database_url")
    db_name=$(<"$STACK_DIR/database_name")
    if [[ $db_name =~ ^sylvode_e2e_web_[0-9a-f]+$ ]]; then
      psql "$admin_url" -X -q -v ON_ERROR_STOP=1 \
        -c "DROP DATABASE IF EXISTS \"$db_name\" WITH (FORCE)" >/dev/null ||
        echo "e2e-web-stack: could not drop database $db_name" >&2
    fi
  fi
  rm -rf "$STACK_DIR"
  echo "e2e-web-stack: stopped, $STACK_DIR removed"
}

cmd_up() {
  [[ -n ${OPENPR_TEST_DATABASE_URL:-} ]] || die "OPENPR_TEST_DATABASE_URL is not set"
  check_port E2E_API_PORT "$API_PORT"
  check_port E2E_WEB_PORT "$WEB_PORT"
  [[ $API_PORT != "$WEB_PORT" ]] || die "E2E_API_PORT and E2E_WEB_PORT must differ"

  local api_bin="$BIN_DIR/api" worker_bin="$BIN_DIR/$WORKER_NAME"
  [[ -x $api_bin ]] || die "missing API binary: $api_bin is not an executable file (build it with: cargo build --workspace --release)"
  # The API looks for the worker next to its own executable and refuses every Flow write without it.
  [[ -x $worker_bin ]] || die "missing $WORKER_NAME: $worker_bin is not an executable file; the API spawns it for every Flow collaborative write and looks for it next to $api_bin"
  [[ -f $BUILD_DIR/index.html ]] || die "missing frontend build: $BUILD_DIR/index.html (run: cd frontend && bun run build)"
  local tool
  for tool in bun curl psql; do
    command -v "$tool" >/dev/null || die "$tool is required but not on PATH"
  done
  if pid_alive "$STACK_DIR/api.pid" || pid_alive "$STACK_DIR/proxy.pid"; then
    die "a stack is already running from $STACK_DIR; run '$0 down' first"
  fi

  local db_name database_url api_origin web_origin
  db_name="sylvode_e2e_web_$(random_hex 6)"
  database_url=$(with_database "$OPENPR_TEST_DATABASE_URL" "$db_name")
  api_origin="http://127.0.0.1:$API_PORT"
  web_origin="http://127.0.0.1:$WEB_PORT"

  rm -rf "$STACK_DIR"
  mkdir -p "$STACK_DIR/uploads"
  chmod 700 "$STACK_DIR"
  # From here on a failure tears down whatever was started.
  trap 'echo "e2e-web-stack: up failed, cleaning up" >&2; cmd_down >&2' ERR
  printf '%s\n' "$OPENPR_TEST_DATABASE_URL" >"$STACK_DIR/admin_database_url"
  printf '%s\n' "$db_name" >"$STACK_DIR/database_name"

  psql "$OPENPR_TEST_DATABASE_URL" -X -q -v ON_ERROR_STOP=1 -c "CREATE DATABASE \"$db_name\"" >/dev/null

  cat >"$STACK_DIR/sylvode.toml" <<TOML
[server]
app_name = "api"
bind_addr = "127.0.0.1:$API_PORT"

[database]
url = "$database_url"
max_connections = 20
min_connections = 2

[auth]
jwt_secret = "$(random_hex 32)"

[logging]

[storage]
backend = "local"
dir = "$STACK_DIR/uploads"

[flow]
collab_allowed_origins = ["$web_origin"]
TOML
  chmod 600 "$STACK_DIR/sylvode.toml"

  (cd "$STACK_DIR" && exec "$api_bin" --config "$STACK_DIR/sylvode.toml") >"$STACK_DIR/api.log" 2>&1 &
  echo $! >"$STACK_DIR/api.pid"
  wait_http_200 "$api_origin/health" "$STACK_DIR/api.pid" "$STACK_DIR/api.log" api

  E2E_PROXY_BUILD_DIR="$BUILD_DIR" E2E_PROXY_API_ORIGIN="$api_origin" E2E_PROXY_PORT="$WEB_PORT" \
    bun "$ROOT_DIR/scripts/lib/e2e-web-proxy.ts" >"$STACK_DIR/proxy.log" 2>&1 &
  echo $! >"$STACK_DIR/proxy.pid"
  wait_http_200 "$web_origin/" "$STACK_DIR/proxy.pid" "$STACK_DIR/proxy.log" proxy
  wait_http_200 "$web_origin/health" "$STACK_DIR/proxy.pid" "$STACK_DIR/proxy.log" proxy

  # The specs register their own users, and registration after the first account needs an
  # instance admin; the first account registered on a fresh database is that admin.
  local admin_email admin_password register
  admin_email="e2e-admin-$(random_hex 4)@e2e.sylvode.test"
  admin_password="E2e-$(random_hex 16)-1!"
  register=$(curl -s -X POST "$web_origin/api/v1/auth/register" -H 'Content-Type: application/json' \
    --data "{\"email\":\"$admin_email\",\"password\":\"$admin_password\",\"name\":\"E2E Admin\"}")
  [[ $register == *'"code":0'* ]] || {
    echo "e2e-web-stack: registering the instance admin failed: $register" >&2
    false
  }

  {
    printf 'export BASE_URL=%q\n' "$web_origin"
    printf 'export E2E_DATABASE_URL=%q\n' "$database_url"
    printf 'export ADMIN_EMAIL=%q\n' "$admin_email"
    printf 'export ADMIN_PASSWORD=%q\n' "$admin_password"
  } >"$STACK_DIR/env"
  chmod 600 "$STACK_DIR/env"
  if [[ -n ${GITHUB_ENV:-} ]]; then
    echo "::add-mask::$admin_password"
    {
      echo "BASE_URL=$web_origin"
      echo "E2E_DATABASE_URL=$database_url"
      echo "ADMIN_EMAIL=$admin_email"
      echo "ADMIN_PASSWORD=$admin_password"
      echo "E2E_STACK_DIR=$STACK_DIR"
    } >>"$GITHUB_ENV"
  fi
  trap - ERR

  echo "e2e-web-stack: up"
  echo "  UI:       $web_origin (proxy log $STACK_DIR/proxy.log)"
  echo "  API:      $api_origin (log $STACK_DIR/api.log)"
  echo "  database: $db_name"
  echo "  env:      source $STACK_DIR/env"
}

case "${1:-}" in
  up) cmd_up ;;
  down) cmd_down ;;
  *) usage ;;
esac
