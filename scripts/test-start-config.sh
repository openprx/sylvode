#!/usr/bin/env bash
# Exercises scripts/start.sh --check-config in throwaway copies of the files it needs.
#
# Covers what the script promises a first-time and a returning operator:
#   - a fresh generation validates and lists the origins the stack is published at in
#     [flow] collab_allowed_origins (an empty list kills live editing), never a wildcard;
#   - a re-run leaves the generated files byte-for-byte alone;
#   - a legacy config/openpr.compose*.toml pair is discovered, used and not rewritten, and the
#     check says what its missing collab_allowed_origins means;
#   - keys the binaries accept ([flow], [audit], auth.allow_insecure_cookies) are not refused;
#   - an origin that is not scheme://host[:port] is refused.
#
# Nothing here starts a container: --check-config exits before docker is touched.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

passed=0
failed=0
ok() { printf '  ok   %s\n' "$1"; passed=$((passed + 1)); }
bad() { printf '  FAIL %s\n' "$1"; [[ -n "${2:-}" ]] && printf '%s\n' "$2" | sed 's/^/       /'; failed=$((failed + 1)); }

# A minimal repository copy: start.sh resolves everything relative to its parent directory.
fresh_copy() {
  local dir="$WORK/$1"
  mkdir -p "$dir/scripts/lib" "$dir/config"
  cp "$ROOT_DIR/scripts/start.sh" "$dir/scripts/start.sh"
  cp "$ROOT_DIR"/scripts/lib/sylvode_compat.sh "$ROOT_DIR"/scripts/lib/sylvode_config_schema.json "$dir/scripts/lib/"
  cp "$ROOT_DIR/.env.example" "$dir/.env.example"
  printf '%s' "$dir"
}

# Runs the check in $1 with the environment overrides given as further arguments; stdout+stderr
# land in $1/out, the exit status in $1/status.
check() {
  local dir="$1"
  shift
  local status=0
  (cd "$dir" && env -u SYLVODE_BIND_HOST -u OPENPR_BIND_HOST -u SYLVODE_FRONTEND_PORT -u OPENPR_FRONTEND_PORT \
    "$@" bash scripts/start.sh --check-config) >"$dir/out" 2>&1 || status=$?
  printf '%s' "$status" >"$dir/status"
}

origins_of() {
  python3 - "$1" <<'PY'
import sys, tomllib
with open(sys.argv[1], "rb") as handle:
    print(" ".join(tomllib.load(handle).get("flow", {}).get("collab_allowed_origins", ["<absent>"])))
PY
}

echo "start.sh --check-config"

# 1. Fresh generation with the defaults (127.0.0.1, frontend port 3000).
dir="$(fresh_copy fresh)"
check "$dir"
if [[ "$(cat "$dir/status")" == 0 ]]; then ok "fresh generation validates"; else bad "fresh generation validates" "$(cat "$dir/out")"; fi
origins="$(origins_of "$dir/config/sylvode.compose.toml")"
if [[ "$origins" == "http://localhost:3000 http://127.0.0.1:3000" ]]; then
  ok "default bind lists http://localhost:3000 and http://127.0.0.1:3000"
else
  bad "default bind lists http://localhost:3000 and http://127.0.0.1:3000" "got: $origins"
fi
if grep -q "REPLACE THEM with the origin your users actually type" "$dir/config/sylvode.compose.toml"; then
  ok "the generated file tells the operator to replace the origins"
else
  bad "the generated file tells the operator to replace the origins"
fi
if grep -q "collab_allowed_origins is empty" "$dir/out"; then bad "no empty-origins warning on a fresh file" "$(cat "$dir/out")"; else ok "no empty-origins warning on a fresh file"; fi

# 2. Idempotence: a second run changes nothing it generated.
before="$(sha256sum "$dir/config/sylvode.compose.toml" "$dir/config/sylvode.compose.mcp.toml" "$dir/.env")"
check "$dir"
after="$(sha256sum "$dir/config/sylvode.compose.toml" "$dir/config/sylvode.compose.mcp.toml" "$dir/.env")"
if [[ "$(cat "$dir/status")" == 0 && "$before" == "$after" ]]; then ok "a re-run leaves the generated files unchanged"; else bad "a re-run leaves the generated files unchanged" "$(cat "$dir/out")"; fi

# 3. Wildcard bind on another port: loopback is listed with that port, the wildcard never is.
dir="$(fresh_copy wildcard)"
check "$dir" SYLVODE_BIND_HOST=0.0.0.0 SYLVODE_FRONTEND_PORT=8443
origins="$(origins_of "$dir/config/sylvode.compose.toml")"
if [[ "$(cat "$dir/status")" == 0 && " $origins " == *" http://localhost:8443 "* && " $origins " == *" http://127.0.0.1:8443 "* \
  && "$origins" != *0.0.0.0* && "$origins" != *'*'* ]]; then
  ok "a 0.0.0.0 bind lists loopback on the published port and never the wildcard"
else
  bad "a 0.0.0.0 bind lists loopback on the published port and never the wildcard" "got: $origins
$(cat "$dir/out")"
fi

# 4. Port 80: a browser omits the default port from Origin, so the entry must too.
dir="$(fresh_copy port80)"
check "$dir" SYLVODE_BIND_HOST=192.0.2.10 SYLVODE_FRONTEND_PORT=80
origins="$(origins_of "$dir/config/sylvode.compose.toml")"
if [[ "$origins" == "http://192.0.2.10" ]]; then ok "a specific host on port 80 is listed without the port"; else bad "a specific host on port 80 is listed without the port" "got: $origins"; fi

# 5. Legacy files are discovered, used as they are, and the check explains the empty origins.
dir="$(fresh_copy legacy)"
check "$dir"
mv "$dir/config/sylvode.compose.toml" "$dir/config/openpr.compose.toml"
mv "$dir/config/sylvode.compose.mcp.toml" "$dir/config/openpr.compose.mcp.toml"
python3 - "$dir/config/openpr.compose.toml" <<'PY'
import re, sys
path = sys.argv[1]
text = open(path, encoding="utf-8").read()
open(path, "w", encoding="utf-8").write(re.sub(r"\n\[flow\][\s\S]*?collab_allowed_origins = [^\n]*\n", "\n", text))
PY
legacy_before="$(sha256sum "$dir/config/openpr.compose.toml")"
check "$dir"
if [[ "$(cat "$dir/status")" == 0 && ! -e "$dir/config/sylvode.compose.toml" && "$legacy_before" == "$(sha256sum "$dir/config/openpr.compose.toml")" ]]; then
  ok "legacy openpr.compose*.toml files are used and not rewritten"
else
  bad "legacy openpr.compose*.toml files are used and not rewritten" "$(cat "$dir/out")"
fi
if grep -q "openpr.compose.toml" "$dir/out" && grep -qi "deprecat" "$dir/out"; then ok "the legacy file prints its deprecation notice"; else bad "the legacy file prints its deprecation notice" "$(cat "$dir/out")"; fi
if grep -q "collab_allowed_origins is empty, so the api refuses every live-editing" "$dir/out"; then
  ok "the check explains an empty collab_allowed_origins"
else
  bad "the check explains an empty collab_allowed_origins" "$(cat "$dir/out")"
fi

# 6. Keys the binaries accept are accepted here too.
dir="$(fresh_copy accepted)"
check "$dir"
cat >>"$dir/config/sylvode.compose.toml" <<'TOML'

[audit]
operation_log_retention_days = 30
TOML
python3 - "$dir/config/sylvode.compose.toml" <<'PY'
import sys
path = sys.argv[1]
text = open(path, encoding="utf-8").read()
text = text.replace("[auth]\n", "[auth]\nallow_insecure_cookies = false\n", 1)
text = text.replace("[flow]\n", "[flow]\ndispatch_max_attempts = 5\n", 1)
open(path, "w", encoding="utf-8").write(text)
PY
check "$dir"
if [[ "$(cat "$dir/status")" == 0 ]]; then
  ok "[flow], [audit] and auth.allow_insecure_cookies are accepted"
else
  bad "[flow], [audit] and auth.allow_insecure_cookies are accepted" "$(cat "$dir/out")"
fi

# 7. A wildcard or a path in an origin is refused, as the api would refuse to match it.
sed -i 's|^collab_allowed_origins = .*|collab_allowed_origins = ["http://*:3000", "https://ok.example/path"]|' "$dir/config/sylvode.compose.toml"
check "$dir"
if [[ "$(cat "$dir/status")" == 1 ]] && grep -q "'http://\*:3000' must be" "$dir/out" && grep -q "'https://ok.example/path' must be" "$dir/out"; then
  ok "wildcard and path origins are refused"
else
  bad "wildcard and path origins are refused" "$(cat "$dir/out")"
fi

# 8. A quoted value in .env is unquoted, as docker-compose reads it, and never reaches the TOML.
dir="$(fresh_copy quoted)"
cp "$dir/.env.example" "$dir/.env"
printf 'SYLVODE_FRONTEND_PORT="3999"\n' >>"$dir/.env"
check "$dir"
origins="$(origins_of "$dir/config/sylvode.compose.toml" 2>&1 || true)"
if [[ "$(cat "$dir/status")" == 0 && "$origins" == "http://localhost:3999 http://127.0.0.1:3999" ]]; then
  ok "a quoted .env value is unquoted before it is used"
else
  bad "a quoted .env value is unquoted before it is used" "got: $origins
$(cat "$dir/out")"
fi

# 9. A port that is not a number is refused before any file is generated.
dir="$(fresh_copy badport)"
check "$dir" SYLVODE_FRONTEND_PORT=30x0
if [[ "$(cat "$dir/status")" == 1 ]] && grep -q "SYLVODE_FRONTEND_PORT must be a port number" "$dir/out" \
  && [[ ! -e "$dir/config/sylvode.compose.toml" ]]; then
  ok "a non-numeric port is refused before generation"
else
  bad "a non-numeric port is refused before generation" "$(cat "$dir/out")"
fi

# 10. A legacy variable set only in the process environment works with a fresh .env: the example
# file does not preset the canonical names it would conflict with.
dir="$(fresh_copy legacyenv)"
check "$dir" OPENPR_FRONTEND_PORT=3111
origins="$(origins_of "$dir/config/sylvode.compose.toml" 2>&1 || true)"
if [[ "$(cat "$dir/status")" == 0 && "$origins" == "http://localhost:3111 http://127.0.0.1:3111" ]]; then
  ok "a legacy variable in the process environment works with a fresh .env"
else
  bad "a legacy variable in the process environment works with a fresh .env" "got: $origins
$(cat "$dir/out")"
fi

# 11. An unknown argument is a usage error, not a full start.
dir="$(fresh_copy badarg)"
status=0
(cd "$dir" && bash scripts/start.sh --help-me) >"$dir/out" 2>&1 || status=$?
if [[ "$status" == 2 ]] && grep -q "Usage: scripts/start.sh" "$dir/out" && [[ ! -e "$dir/.env" ]]; then
  ok "an unknown argument prints the usage and exits 2 before touching anything"
else
  bad "an unknown argument prints the usage and exits 2 before touching anything" "status $status
$(cat "$dir/out")"
fi

printf '\nstart.sh --check-config: %d passed, %d failed\n' "$passed" "$failed"
[[ "$failed" -eq 0 ]]
