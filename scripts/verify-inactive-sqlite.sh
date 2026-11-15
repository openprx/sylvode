#!/usr/bin/env bash
# RUSTSEC-2026-0459 exception is valid only while SQLite is absent from every
# resolved normal/build/dev graph, including all workspace features and targets.
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if ! graph="$(cd "$ROOT_DIR" && CARGO_TERM_COLOR=never cargo tree --locked --workspace --all-features --target all --edges normal,build,dev --prefix none --format '{p}' 2>&1)"; then
  echo "FAIL: unable to resolve the complete dependency graph; SQLite exception is not validated" >&2
  exit 1
fi
if grep -Eq '^(sqlx-sqlite|libsqlite3-sys) v[0-9]' <<<"$graph"; then
  echo "FAIL: SQLite entered the resolved dependency graph; remove/reassess RUSTSEC-2026-0459 exception" >&2
  exit 1
fi
echo "PASS: SQLite is absent from all workspace normal/build/dev dependencies, features and targets"
