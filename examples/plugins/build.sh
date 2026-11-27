#!/usr/bin/env bash
# Builds the guest plugins into components under dist/ and records their SHA-256 in dist/SHA256SUMS.
#
#   ./build.sh           build, then write dist/ and dist/SHA256SUMS
#   ./build.sh --check   build, then compare with the committed dist/SHA256SUMS (exit 1 on a difference)
#
# Needs the Rust target wasm32-unknown-unknown and `wasm-tools` (cargo install wasm-tools --locked,
# used with 1.263.0). The toolchain is pinned by the repository's rust-toolchain.toml and the
# dependencies by Cargo.lock; paths are remapped so the output does not depend on the checkout.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
target="$root/target/example-plugins"
mode="${1:-write}"

command -v wasm-tools >/dev/null || { echo "wasm-tools is required: cargo install wasm-tools --locked" >&2; exit 2; }

cd "$here"
RUSTFLAGS="--remap-path-prefix=$root=/src --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=$(rustc --print sysroot)=/rustc" \
  cargo build --release --locked --target wasm32-unknown-unknown --target-dir "$target"

# crate library name -> published component name
components=(
  "sv_sample_service_desk_action:service-desk-action"
  "sv_sample_service_desk_rule:service-desk-rule"
  "sv_bad_endless_loop:bad-endless-loop"
  "sv_bad_memory_growth:bad-memory-growth"
  "sv_bad_host_calls:bad-host-calls"
)

out="$here/dist"
[ "$mode" = "--check" ] && out="$(mktemp -d "${TMPDIR:-$here}/plugins-check.XXXXXX")"
mkdir -p "$out"
for entry in "${components[@]}"; do
  lib="${entry%%:*}"
  name="${entry##*:}"
  wasm-tools component new "$target/wasm32-unknown-unknown/release/$lib.wasm" -o "$out/$name.wasm"
  wasm-tools validate "$out/$name.wasm"
done
(cd "$out" && shasum -a 256 -- *.wasm | LC_ALL=C sort -k2 > SHA256SUMS)

if [ "$mode" = "--check" ]; then
  if diff -u "$here/dist/SHA256SUMS" "$out/SHA256SUMS"; then
    echo "dist/ matches a fresh build"
    rm -rf "$out"
  else
    echo "dist/ differs from a fresh build (kept in $out)" >&2
    exit 1
  fi
else
  cat "$out/SHA256SUMS"
fi
