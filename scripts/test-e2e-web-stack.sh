#!/usr/bin/env bash
# Self-test of scripts/e2e-web-stack.sh preflight checks. Needs no database, no build and no
# network: the binaries, the frontend build and the bun/curl/psql tools are stubs in a scratch
# directory.
#
# The property that matters: `up` must exit non-zero, naming the missing file, when
# collab-isolated-apply-worker is not next to the API binary. An image or a release that ships the
# API without it accepts no Flow collaborative write, and the stack the browser E2E specs run
# against must not come up in that state. The control case (worker present) must get past every
# preflight check and reach database creation, which proves the check is what fails the others.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STACK="$ROOT_DIR/scripts/e2e-web-stack.sh"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

failures=0
pass() { echo "PASS: $*"; }
fail() {
  echo "FAIL: $*" >&2
  failures=$((failures + 1))
}

mkdir -p "$SCRATCH/bin" "$SCRATCH/build" "$SCRATCH/tools"
printf '#!/bin/sh\nexit 0\n' >"$SCRATCH/bin/api"
chmod +x "$SCRATCH/bin/api"
printf '<!doctype html>\n' >"$SCRATCH/build/index.html"
for tool in bun curl; do
  printf '#!/bin/sh\necho "stub %s" >&2\nexit 1\n' "$tool" >"$SCRATCH/tools/$tool"
  chmod +x "$SCRATCH/tools/$tool"
done
printf '#!/bin/sh\necho "stub psql reached" >&2\nexit 1\n' >"$SCRATCH/tools/psql"
chmod +x "$SCRATCH/tools/psql"

# run_up NAME [VAR=VALUE...] -> sets $rc and $out
run_up() {
  local name=$1
  shift
  set +e
  out=$(env PATH="$SCRATCH/tools:$PATH" \
    E2E_BIN_DIR="$SCRATCH/bin" E2E_BUILD_DIR="$SCRATCH/build" E2E_STACK_DIR="$SCRATCH/state-$name" \
    E2E_API_PORT=18981 E2E_WEB_PORT=18980 GITHUB_ENV= \
    OPENPR_TEST_DATABASE_URL=postgres://e2e:e2e@127.0.0.1:1/postgres \
    "$@" bash "$STACK" up 2>&1)
  rc=$?
  set -e
}

worker="$SCRATCH/bin/collab-isolated-apply-worker"

# 1. Worker binary absent.
run_up absent
if [[ $rc -ne 0 && $out == *"missing collab-isolated-apply-worker: $worker"* && $out != *"stub psql reached"* ]]; then
  pass "up exits $rc and names $worker when the worker binary is absent"
else
  fail "worker absent: expected non-zero exit naming $worker before any database work, got rc=$rc: $out"
fi
if [[ ! -e $SCRATCH/state-absent ]]; then
  pass "nothing was started or written when the worker is absent"
else
  fail "worker absent: state directory was created"
fi

# 2. Worker present but not executable.
printf '#!/bin/sh\nexit 0\n' >"$worker"
chmod -x "$worker"
run_up not-executable
if [[ $rc -ne 0 && $out == *"missing collab-isolated-apply-worker: $worker"* ]]; then
  pass "up exits $rc when the worker binary is not executable"
else
  fail "worker not executable: expected non-zero exit naming $worker, got rc=$rc: $out"
fi

# 3. Control: worker present and executable gets past the preflight checks.
chmod +x "$worker"
run_up control
if [[ $out == *"stub psql reached"* && $out != *"missing collab-isolated-apply-worker"* ]]; then
  pass "control: with the worker present, up passes preflight and reaches database creation"
else
  fail "control: expected up to reach database creation, got rc=$rc: $out"
fi
if [[ ! -e $SCRATCH/state-control ]]; then
  pass "control: a failed up removed its state directory"
else
  fail "control: state directory left behind after a failed up"
fi

# 4. Missing database URL.
run_up no-db OPENPR_TEST_DATABASE_URL=
if [[ $rc -ne 0 && $out == *"OPENPR_TEST_DATABASE_URL is not set"* ]]; then
  pass "up exits $rc without OPENPR_TEST_DATABASE_URL"
else
  fail "no database URL: expected non-zero exit, got rc=$rc: $out"
fi

if ((failures > 0)); then
  echo "test-e2e-web-stack: $failures failure(s)" >&2
  exit 1
fi
echo "test-e2e-web-stack: all checks passed"
