#!/usr/bin/env bash
# Static assertions over frontend/nginx.conf for the authorization server's routes and headers
# (contract 13.4, 16, 18 WP6). It reads the file only: it does not run nginx and so does not prove
# that the file loads (`nginx -t` does that where nginx is available). It checks that
#   - the two discovery documents the API serves have their own exact locations that proxy to the
#     API with the same forwarded headers as the general API location,
#   - /.well-known/oauth-protected-resource/mcp is not proxied to the API,
#   - /oauth/ is its own location whose headers include frame-ancestors 'none', X-Frame-Options:
#     DENY, Cache-Control: no-store and a referrer policy that sends no referrer, together with the
#     inherited security headers (a location with its own add_header drops the server-level ones),
#   - the event stream location exists exactly once (it has its own check).
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONF="${1:-$ROOT_DIR/frontend/nginx.conf}"
failures=0
pass() { echo "PASS: $*"; }
fail() {
  echo "FAIL: $*" >&2
  failures=$((failures + 1))
}

# The body of the location whose header line starts with $1.
block() {
  awk -v header="$1" '
    index($0, header) && !open { open = 1; next }
    open && /^[[:space:]]*}/ { exit }
    open { print }
  ' "$CONF"
}

general="$(block 'location ^~ /api {')"

for path in /.well-known/oauth-authorization-server /.well-known/oauth-protected-resource/api; do
  body="$(block "location = $path {")"
  if [ -z "$body" ]; then
    fail "no exact location for $path"
    continue
  fi
  pass "exact location for $path"
  if grep -qF 'proxy_pass http://$api_upstream;' <<<"$body"; then
    pass "$path proxies to the API"
  else
    fail "$path does not proxy to the API upstream"
  fi
  while IFS= read -r header; do
    [ -n "$header" ] || continue
    if grep -qF -- "$header" <<<"$body"; then
      pass "$path forwards: $header"
    else
      fail "$path does not forward: $header"
    fi
  done < <(grep -E 'proxy_set_header (Host|X-Real-IP|X-Forwarded-For|X-Forwarded-Proto) ' <<<"$general" | sed 's/^[[:space:]]*//')
done

# The MCP document belongs to the MCP server: no location may send it to the API.
if grep -qE '^[[:space:]]*location[^{]*oauth-protected-resource/mcp' "$CONF"; then
  fail "oauth-protected-resource/mcp has a location (it must not be proxied to the API)"
else
  pass "oauth-protected-resource/mcp is not proxied to the API"
fi

oauth="$(block 'location ^~ /oauth/ {')"
if [ -z "$oauth" ]; then
  fail "no location for /oauth/"
else
  pass "/oauth/ has its own location"
  csp="$(grep -F 'add_header Content-Security-Policy' <<<"$oauth" || true)"
  if grep -qF "frame-ancestors 'none'" <<<"$csp"; then
    pass "/oauth/ CSP has frame-ancestors 'none'"
  else
    fail "/oauth/ CSP lacks frame-ancestors 'none'"
  fi
  if grep -qF "frame-ancestors 'self'" <<<"$csp"; then
    fail "/oauth/ CSP also names frame-ancestors 'self'"
  fi
  for directive in \
    "script-src 'self' 'wasm-unsafe-eval'" \
    "form-action 'self'" \
    "base-uri 'self'"; do
    if grep -qF -- "$directive" <<<"$csp"; then
      pass "/oauth/ CSP keeps: $directive"
    else
      fail "/oauth/ CSP lost: $directive"
    fi
  done
  for header in \
    'add_header X-Frame-Options "DENY" always;' \
    'add_header Cache-Control "no-store" always;' \
    'add_header Referrer-Policy "no-referrer" always;' \
    'add_header X-Content-Type-Options "nosniff" always;'; do
    if grep -qF -- "$header" <<<"$oauth"; then
      pass "/oauth/ sets: $header"
    else
      fail "/oauth/ lacks: $header"
    fi
  done
  if grep -qF 'try_files $uri /index.html;' <<<"$oauth"; then
    pass "/oauth/ falls back to the SPA"
  else
    fail "/oauth/ does not fall back to index.html"
  fi
fi

streams="$(grep -c 'location = /api/v1/oauth/events/stream {' "$CONF" || true)"
if [ "$streams" = "1" ]; then
  pass "the event stream location appears once"
else
  fail "the event stream location appears $streams times"
fi

if [ "$failures" -gt 0 ]; then
  echo "$failures check(s) failed" >&2
  exit 1
fi
echo "nginx oauth locations: ok"
