#!/usr/bin/env bash
# Builds the signed probe package `qa/api-probe` from this directory and the committed component in
# ../dist (build it with ../build.sh first when it changes).
#
#   ./build.sh <key-file> <out.svpkg> [capability-id,capability-id,...]
#
# The third argument is the list of capability ids the package requests (default: the ids of the
# end-of-cycle QA round). Rebuild with another list to change what the installation can be granted.
# <key-file> is the secret PKCS#8 key of publisher `qa`: it is created (mode 0600) when missing, and
# its public key is printed so it can be registered with
# `POST /api/v1/workspaces/{id}/publisher-keys {"publisher": "qa", "public_key": "<printed>"}`. The key
# is never committed. Needs `cargo` (builds the `sv-pack` tool of this repository).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
[ "$#" -ge 2 ] && [ "$#" -le 3 ] || { echo "usage: build.sh <key-file> <out.svpkg> [id,id,...]" >&2; exit 2; }
key="$1"
out="$2"
ids="${3:-work_items.items.read,flow.objects.read,governance.proposals.read,forms.views.read}"

requests=""
IFS=',' read -r -a list <<< "$ids"
for id in "${list[@]}"; do
  [[ "$id" =~ ^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*){1,5}$ ]] || { echo "bad capability id: $id" >&2; exit 2; }
  requests="${requests:+$requests,
}    {\"id\": \"$id\"}"
done

work="$(mktemp -d "${TMPDIR:-/tmp}/api-probe-package.XXXXXX")"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/contents/components"
while IFS= read -r line; do
  if [ "$line" = "__REQUESTS__" ]; then printf '%s\n' "$requests"; else printf '%s\n' "$line"; fi
done < "$here/sv-package.json" > "$work/sv-package.json"
cp "$here/../dist/api-probe-action.wasm" "$work/contents/components/"

cd "$root"
if [ ! -f "$key" ]; then
  cargo run -q -p sv-package --bin sv-pack -- keygen "$key" "$key.pub"
fi
cargo run -q -p sv-package --bin sv-pack -- pack "$work" "$key" "$out"
if [ -f "$key.pub" ]; then
  echo "public key (register for publisher qa): $(cat "$key.pub")"
fi
