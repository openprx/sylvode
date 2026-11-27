#!/usr/bin/env bash
# Packs and signs the built-in packages of publisher `sv`.
#
#   packages/builtin/build.sh <key-file> <out-dir>
#
# <key-file>  secret PKCS#8 key of publisher `sv` (mode 0600). Created when missing (QA builds), and its
#             public key is written next to it as <key-file>.pub. The key is NEVER committed; the release
#             key is held by the owner and is not available to this script.
# <out-dir>   receives one `<name>.svpkg` per source directory; point `[packages] builtin_dir` at it.
#
# The server accepts the packages only when `[packages] builtin_public_key` (debug and QA builds) holds the
# public key printed at the end, base64url without padding. Release builds pin the release key instead.
#
# A source directory holds `sv-package.json` and `contents/`; an optional `components.list` names the
# components of `examples/plugins/dist` (without `.wasm`) to copy into `contents/components/`.
# Needs `cargo`: it builds the `sv-pack` tool of this repository.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
[ "$#" -eq 2 ] || { echo "usage: build.sh <key-file> <out-dir>" >&2; exit 2; }
key="$1"
out="$2"

# The key and the packages never go into the repository: a QA key committed by accident is a leak.
inside() { case "$1/" in "$root"/*) return 0 ;; esac; return 1; }
mkdir -p "$(dirname "$key")" "$out"
key="$(cd "$(dirname "$key")" && pwd -P)/$(basename "$key")"
out="$(cd "$out" && pwd -P)"
root_real="$(cd "$root" && pwd -P)"
root="$root_real"
if inside "$key" || inside "$out"; then
  echo "refusing: <key-file> and <out-dir> must be outside the repository ($root)" >&2
  exit 2
fi

cd "$root"
cargo build -q -p sv-package --bin sv-pack
pack="${CARGO_TARGET_DIR:-$root/target}/debug/sv-pack"
[ -x "$pack" ] || { echo "sv-pack was not built at $pack" >&2; exit 1; }

if [ ! -f "$key" ]; then
  "$pack" keygen "$key" "$key.pub"
fi
mkdir -p "$out"
work="$(mktemp -d "${TMPDIR:-/tmp}/sv-builtin-packages.XXXXXX")"
trap 'rm -rf "$work"' EXIT

count=0
for dir in "$here"/*/; do
  [ -f "$dir/sv-package.json" ] || continue
  name="$(basename "$dir")"
  stage="$work/$name"
  mkdir -p "$stage"
  cp "$dir/sv-package.json" "$stage/"
  cp -R "$dir/contents" "$stage/contents"
  if [ -f "$dir/components.list" ]; then
    mkdir -p "$stage/contents/components"
    while IFS= read -r component; do
      [ -n "$component" ] || continue
      cp "$root/examples/plugins/dist/$component.wasm" "$stage/contents/components/"
    done < "$dir/components.list"
  fi
  "$pack" pack "$stage" "$key" "$out/$name.svpkg" >/dev/null
  echo "packed $name"
  count=$((count + 1))
done
[ "$count" -gt 0 ] || { echo "no package source found in $here" >&2; exit 1; }
if [ -f "$key.pub" ]; then
  echo "public key (set [packages] builtin_public_key to this): $(cat "$key.pub")"
fi
