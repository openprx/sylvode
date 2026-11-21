#!/usr/bin/env bash
# Checks that frontend/nginx.conf serves the delegated event stream from its own exact location
# with the settings a never-ending server-sent-events response needs, and that the location sends
# the same forwarded headers as the general API location. Needs no nginx binary.
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

stream="$(block 'location = /api/v1/oauth/events/stream {')"
general="$(block 'location ^~ /api {')"

if [ -z "$stream" ]; then
  fail "no exact location for /api/v1/oauth/events/stream"
else
  pass "the event stream has its own exact location"
  for directive in \
    'proxy_buffering off;' \
    'proxy_cache off;' \
    'gzip off;' \
    'proxy_http_version 1.1;' \
    'proxy_set_header Connection "";'; do
    if grep -qF -- "$directive" <<<"$stream"; then
      pass "stream location: $directive"
    else
      fail "stream location lacks: $directive"
    fi
  done

  # The read timeout must be above the 20 s heartbeat and far below the general location's hour.
  seconds="$(sed -n 's/^[[:space:]]*proxy_read_timeout[[:space:]]*\([0-9]*\)s;.*/\1/p' <<<"$stream")"
  if [ -n "$seconds" ] && [ "$seconds" -gt 20 ] && [ "$seconds" -le 600 ]; then
    pass "stream location: proxy_read_timeout ${seconds}s is above the 20 s heartbeat"
  else
    fail "stream location: proxy_read_timeout must be a number of seconds above 20 (found: '${seconds}')"
  fi

  # The same forwarded headers as the general API location.
  while IFS= read -r header; do
    [ -n "$header" ] || continue
    if grep -qF -- "$header" <<<"$stream"; then
      pass "stream location forwards: $header"
    else
      fail "stream location does not forward: $header"
    fi
  done < <(grep -E 'proxy_set_header (Host|X-Real-IP|X-Forwarded-For|X-Forwarded-Proto) ' <<<"$general" | sed 's/^[[:space:]]*//')
fi

if [ "$failures" -gt 0 ]; then
  echo "$failures check(s) failed" >&2
  exit 1
fi
echo "nginx event stream location: ok"
