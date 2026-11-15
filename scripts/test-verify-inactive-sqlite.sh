#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fixture_dir="$(mktemp -d)"
trap 'rm -rf "$fixture_dir"' EXIT
cat > "$fixture_dir/cargo" <<'CARGO'
#!/usr/bin/env bash
set -euo pipefail
[[ "$*" == "tree --locked --workspace --all-features --target all --edges normal,build,dev --prefix none --format {p}" ]] || exit 12
case "$FIXTURE_GRAPH" in
  inactive) echo 'sqlx-postgres v0.8.6' ;;
  sqlite) echo 'sqlx-sqlite v0.8.6' ;;
  library) echo 'libsqlite3-sys v0.39.0' ;;
  error) exit 9 ;;
esac
CARGO
chmod +x "$fixture_dir/cargo"
for scenario in inactive sqlite library error; do
  if PATH="$fixture_dir:$PATH" FIXTURE_GRAPH="$scenario" bash "$ROOT_DIR/scripts/verify-inactive-sqlite.sh" >"$fixture_dir/result" 2>&1; then
    [[ "$scenario" == inactive ]] || { echo "FAIL: accepted $scenario"; exit 1; }
  else
    [[ "$scenario" != inactive ]] || { cat "$fixture_dir/result"; exit 1; }
  fi
  echo "PASS: $scenario graph control"
done
