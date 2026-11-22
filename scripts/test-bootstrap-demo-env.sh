#!/usr/bin/env bash
# Checks how scripts/bootstrap-restaurant-demo.sh reads its inputs, without an API.
#
# The non-local URL guard runs before any request, so a remote API URL shows which variable the
# script read: only SYLVODE_* names are inputs, and a legacy OPENPR_* name is ignored.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$ROOT_DIR/scripts/bootstrap-restaurant-demo.sh"
OUT="$(mktemp)"
trap 'rm -f "$OUT"' EXIT

passed=0
failed=0
ok() { printf '  ok   %s\n' "$1"; passed=$((passed + 1)); }
bad() { printf '  FAIL %s\n' "$1"; sed 's/^/       /' "$OUT"; failed=$((failed + 1)); }

# Runs the script with a clean slate of demo inputs plus the given assignments.
run() {
  local status=0
  local -a clean=()
  local name
  for name in $(compgen -e | grep -E '^(SYLVODE|OPENPR)_(API_URL|DEMO_|MCP_BOT_TOKEN)' || true); do
    clean+=(-u "$name")
  done
  env "${clean[@]}" "$@" bash "$SCRIPT" >"$OUT" 2>&1 || status=$?
  printf '%s' "$status"
}

echo "bootstrap-restaurant-demo.sh inputs"

status="$(run SYLVODE_API_URL=http://example.com)"
if [[ "$status" == 2 ]] && grep -q "Refusing to seed a non-local API URL: http://example.com" "$OUT" && ! grep -q "deprecated" "$OUT"; then
  ok "SYLVODE_API_URL is read"
else
  bad "SYLVODE_API_URL is read"
fi

# A legacy OPENPR_API_URL next to the canonical one is not read: the refusal names the canonical
# value, and there is no notice and no conflict error.
status="$(run SYLVODE_API_URL=http://example.com OPENPR_API_URL=http://example.org)"
if [[ "$status" == 2 ]] && grep -q "Refusing to seed a non-local API URL: http://example.com" "$OUT" \
  && ! grep -qE "deprecated|conflicts" "$OUT"; then
  ok "a legacy OPENPR_API_URL is ignored"
else
  bad "a legacy OPENPR_API_URL is ignored"
fi

# 0.0.0.0 is not one of the local names the guard allows, and a connection to it fails at once.
status="$(run SYLVODE_API_URL=http://0.0.0.0:1 SYLVODE_DEMO_ALLOW_REMOTE=1 SYLVODE_DEMO_VERIFY_MCP_HTTP=0)"
if [[ "$status" != 0 ]] && ! grep -q "Refusing to seed" "$OUT"; then
  ok "a canonical flag combines with canonical inputs"
else
  bad "a canonical flag combines with canonical inputs"
fi

status="$(run SYLVODE_API_URL=http://0.0.0.0:1 OPENPR_DEMO_ALLOW_REMOTE=1 SYLVODE_DEMO_VERIFY_MCP_HTTP=0)"
if [[ "$status" == 2 ]] && grep -q "Refusing to seed a non-local API URL: http://0.0.0.0:1" "$OUT" \
  && ! grep -q "deprecated" "$OUT"; then
  ok "a legacy flag is ignored"
else
  bad "a legacy flag is ignored"
fi

if bash "$SCRIPT" --help | grep -q "SYLVODE_DEMO_ALLOW_REMOTE=1" && ! bash "$SCRIPT" --help | grep -qE '^  OPENPR_'; then
  ok "--help documents the SYLVODE_* names"
else
  bash "$SCRIPT" --help >"$OUT"
  bad "--help documents the SYLVODE_* names"
fi

printf '\nbootstrap-restaurant-demo.sh inputs: %d passed, %d failed\n' "$passed" "$failed"
[[ "$failed" -eq 0 ]]
