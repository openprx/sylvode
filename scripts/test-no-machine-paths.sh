#!/usr/bin/env bash
# Fails when a tracked file under scripts/ names a machine-specific absolute path.
#
# An open-source checkout can live anywhere, so a script must not default to, read from or write
# to a path that only exists on one maintainer's machine (a workspace under /opt, a home
# directory). Defaults belong inside the checkout's ignored working area (.flow-gate/) or behind
# an explicit flag or environment variable that fails clearly when unset.
#
# The only exemptions are listed below with the reason; each must still match, so a stale entry
# fails as well. A built-in mutation control proves the scan can go red.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PATTERN='/opt/(worker|working|opsx)([/"'"'"' ]|$)|/home/[A-Za-z0-9_.-]+/|/Users/[A-Za-z0-9_.-]+/|(^|[^A-Za-z0-9_$}.-])/root/'

# path<TAB>exact line text that may contain a machine path<TAB>reason
EXEMPTIONS=$'scripts/audit-universal-forms-production-readiness.sh\tnot_contains "compose does not hardcode local openpr-webhook binary path" "$COMPOSE_FILE" "/opt/opsx/openpr-webhook"\tnegative check: asserts the compose file does not contain this path
scripts/report-flow-v0.3-json.sh\t      repository: "/opt/worker/code/openpr",\tfrozen v0.3 receipt: docs/schemas/sylvode-flow-gate-v1.schema.json pins this value as a const
scripts/report-flow-v0.3-json.sh\t    source: {repository: "/opt/worker/code/openpr", head: $head, dirty: $dirty},\tfrozen v0.3 receipt: docs/schemas/sylvode-flow-gate-v1.schema.json pins this value as a const'

# scan ROOT LISTFILE -> prints "path:line:text" for every non-exempt hit
scan() {
  local root=$1 list=$2 rel line_no text
  while IFS= read -r rel; do
    [[ -f "$root/$rel" ]] || continue
    while IFS=: read -r line_no text; do
      if ! exempt "$rel" "$text"; then
        printf '%s:%s:%s\n' "$rel" "$line_no" "$text"
      fi
    done < <(grep -n -I -E "$PATTERN" "$root/$rel" || true)
  done <"$list"
}

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

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# This file is excluded: its pattern and exemption table necessarily spell out the paths it bans.
(cd "$ROOT_DIR" && git ls-files scripts | grep -vxF scripts/test-no-machine-paths.sh) >"$WORK/files"
file_count=$(wc -l <"$WORK/files")
if [[ $file_count -eq 0 ]]; then
  echo "FAIL: no tracked files under scripts/ were examined" >&2
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

# Mutation control: a fixture script with two injected machine defaults must be reported, and its
# clean lines must not be.
mkdir -p "$WORK/mutant/scripts"
printf '#!/usr/bin/env bash\nROOT_DIR="$(pwd)"\nOUT_DIR="${OUT_DIR:-$ROOT_DIR/.flow-gate/out}"\n' >"$WORK/mutant/scripts/ci-universal-forms-gates.sh"
printf 'OUT_DIR="${OUT_DIR:-/opt/%s/report/example}"\n' worker >>"$WORK/mutant/scripts/ci-universal-forms-gates.sh"
printf 'BUN=/%s/someone/.bun/bin/bun\n' home >>"$WORK/mutant/scripts/ci-universal-forms-gates.sh"
printf 'scripts/ci-universal-forms-gates.sh\n' >"$WORK/mutant/files"
mutant_hits=$(scan "$WORK/mutant" "$WORK/mutant/files" | wc -l)
if [[ $mutant_hits -ne 2 ]]; then
  echo "FAIL: mutation control: expected 2 injected machine paths to be reported, got $mutant_hits" >&2
  exit 1
fi

if [[ -s "$WORK/hits" ]]; then
  echo "FAIL: machine-specific absolute paths under scripts/:" >&2
  sed 's/^/  /' "$WORK/hits" >&2
  exit 1
fi
if [[ $stale -ne 0 ]]; then
  exit 1
fi
printf 'No machine-specific paths: %s files scanned, %s exemptions, mutation control red as expected.\n' \
  "$file_count" "$(grep -c . <<<"$EXEMPTIONS")"
