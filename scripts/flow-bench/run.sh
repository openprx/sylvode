#!/usr/bin/env bash
# Drives the Flow performance benchmark. GitHub Actions starts PostgreSQL and builds
# the release binaries and the frontend; this script creates a database, starts two
# API processes, and runs the measurement scripts.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="${FLOW_BENCH_OUT:-$ROOT/flow-bench-results}"
STATE="${FLOW_BENCH_STATE:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/flow-bench}"
BIN_DIR="${FLOW_BENCH_BIN_DIR:-$ROOT/target/release}"
BUILD_DIR="${FLOW_BENCH_BUILD_DIR:-$ROOT/frontend/build}"
API_A_PORT="${FLOW_BENCH_API_PORT:-18081}"
API_B_PORT="${FLOW_BENCH_OBSERVER_PORT:-18082}"
WEB_PORT="${FLOW_BENCH_WEB_PORT:-18080}"
PY="$STATE/py/bin/python"

die() {
  echo "flow-bench: $*" >&2
  exit 1
}

with_database() {
  local url=$1 name=$2 base query=""
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
  local file="$STATE/$1.pid" pid
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
  local i
  for i in $(seq 1 180); do
    if [[ $(curl -s -o /dev/null -w '%{http_code}' "$url" || true) == 200 ]]; then
      return 0
    fi
    if ! pid_alive "$pid_file"; then
      echo "flow-bench: $what exited before $url answered 200; last log lines:" >&2
      tail -n 60 "$log" >&2 || true
      return 1
    fi
    sleep 1
  done
  echo "flow-bench: $url did not answer 200 within 180s; last log lines:" >&2
  tail -n 60 "$log" >&2 || true
  return 1
}

copy_logs() {
  mkdir -p "$OUT/logs" "$OUT/parts"
  local name
  for name in api-a api-b worker proxy; do
    if [[ -f $STATE/$name.log ]]; then
      tail -n 400 "$STATE/$name.log" >"$OUT/logs/$name.tail.log" || true
    fi
  done
}

finished=0
finish() {
  if [[ $finished == 1 ]]; then
    return 0
  fi
  finished=1
  copy_logs
  if [[ -x $PY ]]; then
    "$PY" "$ROOT/scripts/flow-bench/bench.py" finalize --out "$OUT" || return 1
  fi
}

stop_all() {
  stop_pid proxy || true
  stop_pid worker || true
  stop_pid api-b || true
  stop_pid api-a || true
}

trap 'code=$?; set +e; finish; finish_code=$?; stop_all; if [[ $code -eq 0 ]]; then exit "$finish_code"; else exit "$code"; fi' EXIT

[[ -n ${OPENPR_TEST_DATABASE_URL:-} ]] || die "OPENPR_TEST_DATABASE_URL is not set"
[[ -x $BIN_DIR/api ]] || die "missing $BIN_DIR/api"
[[ -x $BIN_DIR/worker ]] || die "missing $BIN_DIR/worker"
[[ -x $BIN_DIR/collab-isolated-apply-worker ]] || die "missing $BIN_DIR/collab-isolated-apply-worker"
[[ -x $BIN_DIR/flow-loro-bench ]] || die "missing $BIN_DIR/flow-loro-bench"
command -v psql >/dev/null || die "psql is required"
command -v curl >/dev/null || die "curl is required"
command -v python3 >/dev/null || die "python3 is required"

rm -rf "$OUT" "$STATE"
mkdir -p "$OUT/parts" "$OUT/screenshots" "$STATE/uploads"
chmod 700 "$STATE"

db_name="sylvode_flow_bench"
database_url="$(with_database "$OPENPR_TEST_DATABASE_URL" "$db_name")"
[[ $database_url != *'"'* ]] || die "database url cannot be embedded in toml"
psql "$OPENPR_TEST_DATABASE_URL" -X -q -v ON_ERROR_STOP=1 \
  -c "DROP DATABASE IF EXISTS \"$db_name\" WITH (FORCE)" >/dev/null
psql "$OPENPR_TEST_DATABASE_URL" -X -q -v ON_ERROR_STOP=1 \
  -c "CREATE DATABASE \"$db_name\"" >/dev/null

jwt="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
api_origin="http://127.0.0.1:$API_A_PORT"
observer_origin="http://127.0.0.1:$API_B_PORT"
web_origin="http://127.0.0.1:$WEB_PORT"

write_config() {
  local path=$1 port=$2
  cat >"$path" <<TOML
[server]
app_name = "api"
bind_addr = "127.0.0.1:$port"

[database]
url = "$database_url"
max_connections = 20
min_connections = 2

[auth]
jwt_secret = "$jwt"

[logging]

[storage]
backend = "local"
dir = "$STATE/uploads"

[flow]
collab_allowed_origins = ["$web_origin"]
TOML
  chmod 600 "$path"
}

write_config "$STATE/api-a.toml" "$API_A_PORT"
write_config "$STATE/api-b.toml" "$API_B_PORT"

python3 -m venv "$STATE/py"
"$PY" -m pip install --disable-pip-version-check --quiet 'websockets==13.1'

start_api() {
  local name=$1 config=$2
  (cd "$STATE" && exec "$BIN_DIR/api" --config "$config") >"$STATE/$name.log" 2>&1 &
  echo $! >"$STATE/$name.pid"
}

echo "flow-bench: starting API A"
start_api api-a "$STATE/api-a.toml"
wait_http_200 "$api_origin/health" "$STATE/api-a.pid" "$STATE/api-a.log" "api A"

echo "flow-bench: starting worker"
(cd "$STATE" && exec "$BIN_DIR/worker" --config "$STATE/api-a.toml") >"$STATE/worker.log" 2>&1 &
echo $! >"$STATE/worker.pid"
sleep 2
pid_alive "$STATE/worker.pid" || {
  echo "flow-bench: worker exited; last log lines:" >&2
  tail -n 60 "$STATE/worker.log" >&2 || true
  exit 1
}

export FLOW_BENCH_API_ORIGIN="$api_origin"
export FLOW_BENCH_OBSERVER_ORIGIN="$observer_origin"
export FLOW_BENCH_WEB_ORIGIN="$web_origin"
export FLOW_BENCH_COLLAB_ORIGIN="$web_origin"
export FLOW_BENCH_DATABASE_URL="$database_url"
export FLOW_BENCH_IDS="$STATE/ids.json"
export FLOW_BENCH_OUT="$OUT"
export FLOW_BENCH_STATE="$STATE"

echo "flow-bench: API measurements"
"$PY" "$ROOT/scripts/flow-bench/bench.py" api --ids "$STATE/ids.json" --out "$OUT"

echo "flow-bench: starting API B"
start_api api-b "$STATE/api-b.toml"
wait_http_200 "$observer_origin/health" "$STATE/api-b.pid" "$STATE/api-b.log" "api B"
sleep 2

echo "flow-bench: cross-process fanout"
"$PY" "$ROOT/scripts/flow-bench/bench.py" fanout --ids "$STATE/ids.json" --out "$OUT"

echo "flow-bench: Loro timings"
if ! "$BIN_DIR/flow-loro-bench" "$OUT/parts/loro.json"; then
  printf '%s\n' '{"ok":false,"error":"flow-loro-bench exited non-zero"}' >"$OUT/parts/loro.json"
fi

echo "flow-bench: EXPLAIN"
if ! "$PY" "$ROOT/scripts/flow-bench/bench.py" explain --ids "$STATE/ids.json" --out "$OUT"; then
  echo "flow-bench: EXPLAIN failed; continuing so the frontend can still run" >&2
  if [[ ! -f $OUT/parts/explain.json ]]; then
    printf '%s\n' '{"ok":false,"error":"explain exited before writing a result"}' >"$OUT/parts/explain.json"
  fi
fi

if [[ -f $BUILD_DIR/index.html ]] && command -v bun >/dev/null && command -v node >/dev/null; then
  echo "flow-bench: starting frontend proxy"
  E2E_PROXY_BUILD_DIR="$BUILD_DIR" E2E_PROXY_API_ORIGIN="$api_origin" E2E_PROXY_PORT="$WEB_PORT" \
    bun "$ROOT/scripts/lib/e2e-web-proxy.ts" >"$STATE/proxy.log" 2>&1 &
  echo $! >"$STATE/proxy.pid"
  wait_http_200 "$web_origin/" "$STATE/proxy.pid" "$STATE/proxy.log" proxy
  if [[ -z ${CHROMIUM_BIN:-} ]]; then
    CHROMIUM_BIN="$(node -e "process.stdout.write(require('@playwright/test').chromium.executablePath())")"
    export CHROMIUM_BIN
  fi
  if [[ -n ${FLOW_BENCH_LIGHTHOUSE_ROOT:-} && -d ${FLOW_BENCH_LIGHTHOUSE_ROOT}/node_modules/lighthouse ]]; then
    export LIGHTHOUSE_ROOT="$FLOW_BENCH_LIGHTHOUSE_ROOT"
  elif [[ ! -d $STATE/lh/node_modules/lighthouse ]]; then
    npm install --prefix "$STATE/lh" --no-save --silent lighthouse@12.1.0 || true
    export LIGHTHOUSE_ROOT="$STATE/lh"
  fi
  echo "flow-bench: frontend"
  if ! node "$ROOT/scripts/flow-bench/frontend.mjs"; then
    if [[ ! -f $OUT/parts/frontend.json ]]; then
      printf '%s\n' '{"ok":false,"error":"frontend script exited before writing a result"}' >"$OUT/parts/frontend.json"
    fi
  fi
else
  printf '%s\n' '{"ok":false,"error":"frontend build, bun, or node is missing"}' >"$OUT/parts/frontend.json"
fi

echo "flow-bench: measurements finished"
