#!/usr/bin/env bash
# Linux attribution run for the Flow write path: per-step table for flat and structured pages at
# 1,000 / 2,700 / 5,000 / 10,000 blocks. Two measurements, one table:
#
#   1. crates/collab-core/tests/window_attribution.rs (release build): the isolated decode/apply
#      window split into import / semantic_snapshot / check_snapshot, in process without a
#      ceiling, and through the real worker process with the worker's own `window_cpu_us`.
#   2. The release API against PostgreSQL 18: pages seeded in 50-block batches over REST; the
#      debug events of api::flow::collab::write give the time of every write step and the
#      worker's `window_cpu_us`. Writes the current code refuses (CPU ceiling) stop the seeding;
#      sizes beyond that are reported as not reached.
#
# On the Mac (starts its own PostgreSQL 18 container and one builder container, removes both):
#   window-attribution.sh [--src DIR] [--out DIR] [--cpus N] [--sizes LIST] [--shapes LIST]
#                         [--runs N] [--max-seconds N] [--no-api] [--no-window]
# Offline:
#   window-attribution.sh --table-only OUT_DIR   re-render attribution.txt from a finished run
#   window-attribution.sh --selftest             argument parsing and table formatting, no containers
#
#   --src DIR          source tree, mounted read-only (default: the repository this script is in)
#   --out DIR          results directory (default: <harness>/results/window-attribution-<UTC time>)
#   --cpus N           CPU limit of the builder container (use 2 for runner-like numbers)
#   --sizes LIST       block counts, multiples of 50 (default 1000,2700,5000,10000)
#   --shapes LIST      flat,structured (default both)
#   --runs N           repetitions per cell of the collab-core measurement (default 7)
#   --max-seconds N    seeding budget per shape in the API run (default 1500)
#
# The image, target directory and cargo registry are the ones of the local CI harness in
# /Users/nn/worker/llama/ops/sylvode-linux-test/ (read, never modified; its containers are not
# touched). The script starts nothing but its own sv-attr-* containers and network.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# Where window_attribution.py lives: next to this script, or in the mounted source tree when this
# script itself runs from /ops inside the container.
PY_DIR="${SCRIPT_DIR_IN:-$SCRIPT_DIR}"

HARNESS=/Users/nn/worker/llama/ops/sylvode-linux-test
RUNTIME_DIR=/Users/nn/worker/llama/reports/sv-flow-decoupling-2026-11-14/runtime
TARGET_DIR="$RUNTIME_DIR/linux-target"
REGISTRY_DIR=/Users/nn/worker/llama/reports/sv-plugin-analysis-2026-11-14/release-next-local-deploy-20261119T153906Z/cargo-registry
IMAGE=localhost/sylvode-builder-20261109:latest
PG_IMAGE=postgres:18
NET=sv-attr-net
PG_NAME=sv-attr-pg18
RUN_NAME=sv-attr-run
# Public CI-only credentials, copied from .github/workflows/ci.yml.
PG_USER=sylvode_ci
PG_PASSWORD=sylvode_ci_local_only
PG_DB=postgres

die() {
  echo "window-attribution: $*" >&2
  exit 1
}

usage() {
  sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'
}

SRC="$REPO_DIR"
OUT=""
CPUS=""
SIZES="1000,2700,5000,10000"
SHAPES="flat,structured"
RUNS=7
MAX_SECONDS=1500
DO_API=1
DO_WINDOW=1
IN_CONTAINER=0
TABLE_ONLY=""
SELFTEST=0

while [ $# -gt 0 ]; do
  case "$1" in
    --src) [ $# -ge 2 ] || die "--src needs a directory"; SRC="$2"; shift 2 ;;
    --out) [ $# -ge 2 ] || die "--out needs a directory"; OUT="$2"; shift 2 ;;
    --cpus) [ $# -ge 2 ] || die "--cpus needs a number"; CPUS="$2"; shift 2 ;;
    --sizes) [ $# -ge 2 ] || die "--sizes needs a list"; SIZES="$2"; shift 2 ;;
    --shapes) [ $# -ge 2 ] || die "--shapes needs a list"; SHAPES="$2"; shift 2 ;;
    --runs) [ $# -ge 2 ] || die "--runs needs a number"; RUNS="$2"; shift 2 ;;
    --max-seconds) [ $# -ge 2 ] || die "--max-seconds needs a number"; MAX_SECONDS="$2"; shift 2 ;;
    --no-api) DO_API=0; shift ;;
    --no-window) DO_WINDOW=0; shift ;;
    --in-container) IN_CONTAINER=1; shift ;;
    --table-only) [ $# -ge 2 ] || die "--table-only needs a results directory"; TABLE_ONLY="$2"; shift 2 ;;
    --selftest) SELFTEST=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "window-attribution: unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

validate_args() {
  local item
  [[ "$RUNS" =~ ^[0-9]+$ ]] && [ "$RUNS" -ge 1 ] || die "--runs must be a positive number: $RUNS"
  [[ "$MAX_SECONDS" =~ ^[0-9]+$ ]] || die "--max-seconds must be a number: $MAX_SECONDS"
  [ -z "$CPUS" ] || [[ "$CPUS" =~ ^[0-9]+(\.[0-9]+)?$ ]] || die "--cpus must be a number: $CPUS"
  IFS=',' read -r -a size_items <<<"$SIZES"
  [ "${#size_items[@]}" -ge 1 ] || die "--sizes is empty"
  for item in "${size_items[@]}"; do
    [[ "$item" =~ ^[0-9]+$ ]] && [ $((item % 50)) -eq 0 ] && [ "$item" -ge 50 ] \
      || die "size $item is not a positive multiple of 50"
  done
  IFS=',' read -r -a shape_items <<<"$SHAPES"
  for item in "${shape_items[@]}"; do
    case "$item" in flat|structured) ;; *) die "unknown shape: $item" ;; esac
  done
}

render_table() {
  local out_dir=$1 drives=() shape
  IFS=',' read -r -a shape_items <<<"$SHAPES"
  for shape in "${shape_items[@]}"; do
    drives+=(--drive "$out_dir/drive-$shape.json")
  done
  python3 "$PY_DIR/window_attribution.py" table \
    --wa "$out_dir/window.tsv" --api-log "$out_dir/api.log" "${drives[@]}" --sizes "$SIZES" \
    | tee "$out_dir/attribution.txt"
}

if [ "$SELFTEST" = 1 ]; then
  validate_args
  python3 "$PY_DIR/window_attribution.py" selftest >/dev/null
  echo "selftest: table formatting ok; sizes=$SIZES shapes=$SHAPES runs=$RUNS cpus=${CPUS:-unlimited}"
  exit 0
fi

if [ -n "$TABLE_ONLY" ]; then
  [ -d "$TABLE_ONLY" ] || die "not a directory: $TABLE_ONLY"
  validate_args
  render_table "$TABLE_ONLY"
  exit 0
fi

validate_args

# ---------------------------------------------------------------------------------------------
# Inside the builder container
# ---------------------------------------------------------------------------------------------
if [ "$IN_CONTAINER" = 1 ]; then
  cd /work
  OUT=/out
  STATE=/tmp/window-attribution
  rm -rf "$STATE"
  mkdir -p "$STATE"

  # Same linker arrangement as the harness (container-run.sh).
  host_triple=$(rustc -vV | sed -n 's/^host: //p')
  env_triple=$(printf '%s' "$host_triple" | tr 'a-z-' 'A-Z_')
  rust_lld="$(rustc --print sysroot)/lib/rustlib/$host_triple/bin/rust-lld"
  [ -x "$rust_lld" ] || die "rust-lld not found at $rust_lld"
  linker_dir=/tmp/sv-linux-test-linker
  mkdir -p "$linker_dir"
  printf '#!/bin/sh\nexec "%s" -flavor gnu "$@"\n' "$rust_lld" >"$linker_dir/ld.lld"
  printf '#!/bin/sh\nexec cc -B%s -fuse-ld=lld "$@"\n' "$linker_dir" >"$linker_dir/cc-lld"
  chmod +x "$linker_dir/ld.lld" "$linker_dir/cc-lld"
  export "CARGO_TARGET_${env_triple}_LINKER=$linker_dir/cc-lld"

  echo "== $(cargo --version), cgroup cpu.max: $(cat /sys/fs/cgroup/cpu.max 2>/dev/null || echo unavailable)"

  if [ "$DO_WINDOW" = 1 ]; then
    echo "== window measurement (release): cargo test -p collab-core --test window_attribution"
    WINDOW_ATTR_SIZES="$SIZES" WINDOW_ATTR_SHAPES="$SHAPES" WINDOW_ATTR_RUNS="$RUNS" \
      cargo test --release -p collab-core --test window_attribution -- --ignored --nocapture \
      >"$STATE/window-test.log" 2>&1 || {
        tail -n 40 "$STATE/window-test.log" >&2
        die "the window measurement failed"
      }
    cp "$STATE/window-test.log" "$OUT/window-test.log"
    grep -E '^WA_' "$STATE/window-test.log" >"$OUT/window.tsv" || die "no WA_ lines in the window measurement"
  fi

  if [ "$DO_API" = 1 ]; then
    echo "== release API build"
    cargo build --release -p api --bin api -p collab-core --bin collab-isolated-apply-worker
    if ! command -v psql >/dev/null; then
      echo "== apt-get install postgresql-client"
      mkdir -p /ci-cache/apt/archives/partial
      apt-get -o Dir::Cache::Archives=/ci-cache/apt/archives -qq update >/dev/null
      DEBIAN_FRONTEND=noninteractive apt-get -o Dir::Cache::Archives=/ci-cache/apt/archives \
        -qq install -y --no-install-recommends postgresql-client >/dev/null
    fi
    base_url="postgres://$PG_USER:$PG_PASSWORD@$PG_NAME:5432/$PG_DB"
    db_name=sylvode_window_attribution
    psql "$base_url" -X -q -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS \"$db_name\" WITH (FORCE)"
    psql "$base_url" -X -q -v ON_ERROR_STOP=1 -c "CREATE DATABASE \"$db_name\""
    database_url="postgres://$PG_USER:$PG_PASSWORD@$PG_NAME:5432/$db_name"
    jwt="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
    mkdir -p "$STATE/uploads"
    cat >"$STATE/api.toml" <<TOML
[server]
app_name = "api"
bind_addr = "127.0.0.1:18081"

[database]
url = "$database_url"
max_connections = 20
min_connections = 2

[auth]
jwt_secret = "$jwt"

[logging]
filter = "api=info,api::flow::collab::write=debug"

[storage]
backend = "local"
dir = "$STATE/uploads"

[flow]
collab_allowed_origins = ["http://127.0.0.1:18080"]
TOML
    chmod 600 "$STATE/api.toml"
    (cd "$STATE" && exec /work/target/release/api --config "$STATE/api.toml") >"$STATE/api.log" 2>&1 &
    api_pid=$!
    stop_api() { kill "$api_pid" 2>/dev/null || true; wait "$api_pid" 2>/dev/null || true; }
    trap stop_api EXIT
    ready=0
    for _ in $(seq 1 180); do
      if [ "$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:18081/health || true)" = 200 ]; then
        ready=1
        break
      fi
      kill -0 "$api_pid" 2>/dev/null || { tail -n 40 "$STATE/api.log" >&2; die "the API exited before it answered"; }
      sleep 1
    done
    [ "$ready" = 1 ] || { tail -n 40 "$STATE/api.log" >&2; die "the API did not answer within 180s"; }

    export FLOW_BENCH_API_ORIGIN=http://127.0.0.1:18081
    export FLOW_BENCH_DATABASE_URL="$database_url"
    IFS=',' read -r -a shape_items <<<"$SHAPES"
    for shape in "${shape_items[@]}"; do
      echo "== API seeding: $shape pages"
      python3 "$PY_DIR/window_attribution.py" drive --shape "$shape" --sizes "$SIZES" \
        --max-seconds "$MAX_SECONDS" --out "$OUT/drive-$shape.json" \
        || echo "window-attribution: the $shape drive failed; its section will be empty" >&2
    done
    stop_api
    trap - EXIT
    cp "$STATE/api.log" "$OUT/api.log"
  fi

  echo "== table"
  render_table "$OUT"
  exit 0
fi

# ---------------------------------------------------------------------------------------------
# On the Mac
# ---------------------------------------------------------------------------------------------
[ -d "$SRC" ] || die "source directory not found: $SRC"
SRC="$(cd "$SRC" && pwd)"
git -C "$SRC" rev-parse --verify HEAD >/dev/null 2>&1 || die "not a git worktree: $SRC"
[ -f "$SRC/scripts/flow-bench/window_attribution.py" ] || die "the source tree has no scripts/flow-bench/window_attribution.py"
[ -d "$TARGET_DIR" ] || die "target directory missing: $TARGET_DIR"
[ -d "$REGISTRY_DIR" ] || die "cargo registry cache missing: $REGISTRY_DIR"
[ -d "$HARNESS/cache" ] || die "harness cache missing: $HARNESS/cache"
podman image exists "$IMAGE" || die "builder image missing: $IMAGE"
for c in "$RUN_NAME" "$PG_NAME"; do
  if podman container exists "$c"; then
    die "container $c already exists; remove it with: podman rm -f -v $c"
  fi
done

COMMIT="$(git -C "$SRC" rev-parse HEAD)"
COMMIT_DATE="$(git -C "$SRC" show -s --format=%cI HEAD)"
if [ -n "$(git -C "$SRC" status --porcelain)" ]; then DIRTY=true; else DIRTY=false; fi

[ -n "$OUT" ] || OUT="$HARNESS/results/window-attribution-$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"

created_net=0
cleanup() {
  podman rm -f -v "$RUN_NAME" >/dev/null 2>&1 || true
  podman rm -f -v "$PG_NAME" >/dev/null 2>&1 || true
  if [ "$created_net" = 1 ]; then podman network rm "$NET" >/dev/null 2>&1 || true; fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

if ! podman network exists "$NET"; then
  podman network create "$NET" >/dev/null
  created_net=1
fi

if [ "$DO_API" = 1 ]; then
  podman image exists "$PG_IMAGE" || podman pull "$PG_IMAGE" >/dev/null
  echo "== starting $PG_NAME"
  podman run -d --name "$PG_NAME" --network "$NET" \
    -e POSTGRES_USER="$PG_USER" -e POSTGRES_PASSWORD="$PG_PASSWORD" -e POSTGRES_DB="$PG_DB" \
    "$PG_IMAGE" >/dev/null
  ready=0
  for _ in $(seq 1 90); do
    if podman exec "$PG_NAME" pg_isready -h 127.0.0.1 -p 5432 -U "$PG_USER" -d "$PG_DB" >/dev/null 2>&1; then
      ready=1
      break
    fi
    sleep 1
  done
  [ "$ready" = 1 ] || die "PostgreSQL not ready after 90s"
fi

cpu_args=()
if [ -n "$CPUS" ]; then cpu_args=(--cpus "$CPUS"); fi
flags=(--sizes "$SIZES" --shapes "$SHAPES" --runs "$RUNS" --max-seconds "$MAX_SECONDS")
if [ "$DO_API" = 0 ]; then flags+=(--no-api); fi
if [ "$DO_WINDOW" = 0 ]; then flags+=(--no-window); fi

echo "== builder container $RUN_NAME (source $SRC, commit $COMMIT, results $OUT)"
rc=0
podman run --rm --name "$RUN_NAME" --network "$NET" ${cpu_args[@]+"${cpu_args[@]}"} \
  -v "$SRC:/work:ro" --tmpfs /work/apps/api/uploads:rw,size=512m,mode=1777 \
  -v "$TARGET_DIR:/work/target" \
  -v "$REGISTRY_DIR:/cargo/registry" \
  -v "$HARNESS/cache:/ci-cache" \
  -v "$SCRIPT_DIR/window-attribution.sh:/ops/window-attribution.sh:ro" \
  -v "$OUT:/out" \
  -w /work \
  -e SCRIPT_DIR_IN=/work/scripts/flow-bench \
  -e CARGO_HOME=/cargo \
  -e CARGO_TARGET_DIR=/work/target \
  -e CARGO_BUILD_JOBS=2 \
  -e CARGO_TERM_COLOR=never \
  -e RUSTFLAGS="-D warnings" \
  -e OPENPR_BUILD_GIT_COMMIT="$COMMIT" \
  -e OPENPR_BUILD_GIT_DIRTY="$DIRTY" \
  -e OPENPR_BUILD_GIT_COMMITTER_DATE="$COMMIT_DATE" \
  -e GIT_CONFIG_COUNT=1 -e GIT_CONFIG_KEY_0=safe.directory -e GIT_CONFIG_VALUE_0=/work \
  "$IMAGE" bash /ops/window-attribution.sh --in-container "${flags[@]}" 2>&1 | tee "$OUT/run.log" || rc=${PIPESTATUS[0]}
exit "$rc"
