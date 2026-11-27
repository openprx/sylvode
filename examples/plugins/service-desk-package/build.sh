#!/usr/bin/env bash
# Builds the signed sample package `qa/service-desk` from this directory and the committed
# components in ../dist (build them with ../build.sh first when they change).
#
#   ./build.sh <key-file> <out.svpkg>
#
# <key-file> is the secret PKCS#8 key of publisher `qa`: it is created (mode 0600) when missing, next
# to the output unless it exists, and its public key is printed so it can be registered with
# `POST /api/v1/workspaces/{id}/publisher-keys {"publisher": "qa", "public_key": "<printed>"}`. The key
# is never committed. Needs `cargo` (builds the `sv-pack` tool of this repository).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
[ "$#" -eq 2 ] || { echo "usage: build.sh <key-file> <out.svpkg>" >&2; exit 2; }
key="$1"
out="$2"

work="$(mktemp -d "${TMPDIR:-/tmp}/service-desk-package.XXXXXX")"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/contents/components"
cp "$here/sv-package.json" "$work/"
cp -R "$here/contents/forms" "$here/contents/layouts" "$work/contents/"
cp "$here/../dist/service-desk-action.wasm" "$here/../dist/service-desk-rule.wasm" "$work/contents/components/"

cd "$root"
if [ ! -f "$key" ]; then
  cargo run -q -p sv-package --bin sv-pack -- keygen "$key" "$key.pub"
fi
cargo run -q -p sv-package --bin sv-pack -- pack "$work" "$key" "$out"
if [ -f "$key.pub" ]; then
  echo "public key (register for publisher qa): $(cat "$key.pub")"
fi
