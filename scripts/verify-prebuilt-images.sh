#!/usr/bin/env bash
# Builds the api and worker images from Dockerfile.prebuilt and checks what each one ships.
# CI runs this in the "Image contents" job; it runs the same way locally (docker or the podman
# docker shim).
#
#   scripts/verify-prebuilt-images.sh
#
# Needs target/release/{api,worker,collab-isolated-apply-worker} (cargo build --workspace --release).
#
# Assertions:
# 1. the api image has /app/api and /app/collab-isolated-apply-worker, both executable. The API
#    spawns the worker for every Flow collaborative write and looks for it next to its own
#    executable; an api image without it refuses every edit.
# 2. the worker image does not contain collab-isolated-apply-worker (the selection is per image,
#    not "copy everything").
# 3. /app/api --build-info runs in the api image and prints the build identity. The API has no
#    --version flag; --build-info is its side-effect-free identity query, and running it proves
#    the binary loads against the image's libc.
#
# Inputs (environment):
#   RUNTIME_BASE   base image passed to Dockerfile.prebuilt. Default: debian:<codename>-slim on a
#                  Debian host, otherwise debian:trixie-slim. The binaries are built on the host,
#                  so the base must ship a glibc at least as new as the host's.
#   IMAGE_PREFIX   tag prefix, default sylvode-prebuilt-check.
#   KEEP_IMAGES=1  keep the images afterwards.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${IMAGE_PREFIX:-sylvode-prebuilt-check}"

if [[ -z ${RUNTIME_BASE:-} ]]; then
  RUNTIME_BASE=debian:trixie-slim
  if [[ -r /etc/os-release ]]; then
    # shellcheck disable=SC1091
    host_id=$(. /etc/os-release && echo "${ID:-}")
    # shellcheck disable=SC1091
    host_codename=$(. /etc/os-release && echo "${VERSION_CODENAME:-}")
    if [[ $host_id == debian && -n $host_codename ]]; then
      RUNTIME_BASE="debian:${host_codename}-slim"
    fi
  fi
fi

for bin in api worker collab-isolated-apply-worker; do
  [[ -x $ROOT_DIR/target/release/$bin ]] ||
    { echo "verify-prebuilt-images: missing target/release/$bin (run: cargo build --workspace --release)" >&2; exit 1; }
done

api_image="$PREFIX-api:local"
worker_image="$PREFIX-worker:local"
cleanup() {
  if [[ ${KEEP_IMAGES:-0} != 1 ]]; then
    docker rmi -f "$api_image" "$worker_image" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

echo "verify-prebuilt-images: runtime base $RUNTIME_BASE"
for app in api worker; do
  echo "::group::docker build APP_BIN=$app"
  docker build -f "$ROOT_DIR/Dockerfile.prebuilt" --build-arg APP_BIN="$app" \
    --build-arg RUNTIME_BASE="$RUNTIME_BASE" -t "$PREFIX-$app:local" "$ROOT_DIR"
  echo "::endgroup::"
done

failures=0
check() {
  local what=$1
  shift
  if "$@"; then
    echo "PASS: $what"
  else
    echo "FAIL: $what" >&2
    failures=$((failures + 1))
  fi
}

check "api image ships executable /app/api and /app/collab-isolated-apply-worker" \
  docker run --rm --entrypoint sh "$api_image" -c 'test -x /app/api && test -x /app/collab-isolated-apply-worker'
check "worker image does not ship collab-isolated-apply-worker" \
  docker run --rm --entrypoint sh "$worker_image" -c 'test -x /app/worker && ! test -e /app/collab-isolated-apply-worker'

build_info=$(docker run --rm "$api_image" /app/api --build-info) || true
echo "api --build-info: $build_info"
check "api --build-info prints the build identity" \
  grep -q '"schema_version":"openpr.build-info.v1"' <<<"$build_info"

if ((failures > 0)); then
  echo "verify-prebuilt-images: $failures assertion(s) failed" >&2
  exit 1
fi
echo "verify-prebuilt-images: all assertions passed"
