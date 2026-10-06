#!/usr/bin/env bash
# Fails when a tracked text file carries a private IPv4 address or a bot token literal.
#
# Deployment details stay out of an open-source repository: a private address (10.0.0.0/8,
# 172.16.0.0/12, 192.168.0.0/16) is some maintainer's LAN, VPN or container network and is
# meaningless, or misleading, anywhere else; a default must be `localhost`/`127.0.0.1` with an
# override. A bot token (`opr_` followed by 20 or more letters and digits) is a credential, and one
# committed to a public history must be revoked, so none may be added again.
#
# Every tracked text file is scanned (binary files are skipped). A range in CIDR notation
# (`10.0.0.0/8`) names a range, not an address, and is not reported. The only exemptions are listed
# below with the reason; each must still match, so a stale entry fails as well. A built-in
# mutation control proves each pattern can go red.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OCTET='[0-9]{1,3}'
PRIVATE_IPV4="(^|[^0-9.])(10\\.${OCTET}\\.${OCTET}\\.${OCTET}|172\\.(1[6-9]|2[0-9]|3[01])\\.${OCTET}\\.${OCTET}|192\\.168\\.${OCTET}\\.${OCTET})([^0-9/]|$)"
TOKEN_LITERAL='opr_[A-Za-z0-9]{20,}'
PATTERN="${PRIVATE_IPV4}|${TOKEN_LITERAL}"

# path<TAB>exact line text<TAB>reason
EXEMPTIONS=$'apps/api/src/outbound.rs\t            "10.0.0.1",\ttest input: the outbound URL guard must refuse a private address
frontend/docker-entrypoint.d/10-resolver.sh\t# and per host (podman hands out 10.89.0.1 on one machine and 10.89.3.1 on another), so\texplains why the resolver address is discovered at runtime instead of hard-coded'

exempt() {
  local rel=$1 text=$2 e_path e_text e_reason
  while IFS=$'\t' read -r e_path e_text e_reason; do
    [[ -n $e_reason ]] || continue
    if [[ $rel == "$e_path" && $text == "$e_text" ]]; then
      return 0
    fi
  done <<<"$EXEMPTIONS"
  return 1
}

# scan ROOT LISTFILE -> prints "path:line:text" for every non-exempt hit
scan() {
  local root=$1 list=$2 rel line_no text
  while IFS= read -r rel; do
    [[ -f "$root/$rel" && ! -L "$root/$rel" ]] || continue
    while IFS=: read -r line_no text; do
      if ! exempt "$rel" "$text"; then
        printf '%s:%s:%s\n' "$rel" "$line_no" "$text"
      fi
    done < <(grep -n -I -E "$PATTERN" "$root/$rel" || true)
  done <"$list"
}

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# This file is excluded: its patterns and exemption table necessarily spell out what they ban.
(cd "$ROOT_DIR" && git ls-files | grep -vxF scripts/test-no-instance-literals.sh) >"$WORK/files"
file_count=$(wc -l <"$WORK/files")
if [[ $file_count -eq 0 ]]; then
  echo "FAIL: no tracked files were examined" >&2
  exit 1
fi

scan "$ROOT_DIR" "$WORK/files" >"$WORK/hits"

stale=0
while IFS=$'\t' read -r e_path e_text _; do
  [[ -n $e_path ]] || continue
  if ! grep -qxF -- "$e_text" "$ROOT_DIR/$e_path" 2>/dev/null; then
    echo "FAIL: stale exemption, line no longer present in $e_path: $e_text" >&2
    stale=$((stale + 1))
  fi
done <<<"$EXEMPTIONS"

# Mutation control: one injected literal of each class must be reported; the public, loopback
# and placeholder lines must not be.
mkdir -p "$WORK/mutant/docs"
{
  printf 'BASE_URL=http://%s.%s.0.3:3000\n' 10 72
  printf 'DB_HOST=%s.%s.4.2\n' 172 20
  printf 'NAS=%s.%s.1.10\n' 192 168
  printf 'token = "opr_%s"\n' 0123456789abcdefABCDEF0123
  printf 'BASE_URL=http://127.0.0.1:3000 or http://localhost:3000 or 8.8.8.8 or 172.32.0.1\n'
  printf 'version 110.72.0.30 and token = "opr_short" and bot_token = "opr_<token>"\n'
  printf 'the private ranges are 10.0.0.0/8, 172.16.0.0/12 and 192.168.0.0/16\n'
} >"$WORK/mutant/docs/deploy.md"
printf 'docs/deploy.md\n' >"$WORK/mutant/files"
mutant_hits=$(scan "$WORK/mutant" "$WORK/mutant/files" | wc -l)
if [[ $mutant_hits -ne 4 ]]; then
  echo "FAIL: mutation control: expected 4 injected literals to be reported, got $mutant_hits" >&2
  scan "$WORK/mutant" "$WORK/mutant/files" >&2
  exit 1
fi

if [[ -s "$WORK/hits" ]]; then
  echo "FAIL: private IPv4 addresses or bot token literals in tracked files:" >&2
  sed 's/^/  /' "$WORK/hits" >&2
  exit 1
fi
if [[ $stale -ne 0 ]]; then
  exit 1
fi
printf 'No private addresses or token literals: %s files scanned, %s exemptions, mutation control red as expected.\n' \
  "$file_count" "$(grep -c . <<<"$EXEMPTIONS")"
