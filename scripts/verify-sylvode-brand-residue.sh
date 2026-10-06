#!/usr/bin/env bash
# Brand residue gate (ADR-0020 判据 4). See usage below.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ALLOWLIST="$REPO_ROOT/scripts/contracts/sylvode-brand-allowlist.json"
JSON_MODE=0
STRICT=0
RELEASE=0
SELF_TEST=0
REPOS=()

usage() {
  cat <<'EOF'
Usage: scripts/verify-sylvode-brand-residue.sh --json [--repo NAME=PATH]... [--strict] [--release]
       scripts/verify-sylvode-brand-residue.sh --json --self-test

Scans the tracked files (`git ls-files`) of the Sylvode checkouts for `openpr` in any letter
case (`OpenPR`, `OPENPR`, `OpenPr`, ...; not part of `openprx`, not a camel-case word such as
`openProject`) in file content (UTF-8 or UTF-16), tracked paths and symbolic link targets. Every
hit must be covered by an entry of the allow-list, which gives a reason code, a repository, path
globs, a pattern, the places it applies to and an explanation. Anything not covered is a
failure, listed with file:line.

Repositories:
  This checkout is always scanned as `sylvode`. Add the sibling checkouts with --repo, or with
  SYLVODE_BRAND_RESIDUE_REPOS="name=path,name=path". The expected set for a release is:
    sylvode         this repository (openprx/sylvode)
    openpr-webhook  the Sylvode Webhook repository
    docs            the documentation site
    site            the website
    .github         the organisation profile repository
  A requested repository that is missing, empty or not a git checkout is a failure, not zero
  hits, and so is a scan that examined zero files.

Where it runs:
  CI runs the single-repository form (this checkout only, with --strict), because the sibling
  repositories are not checked out there. The multi-repository form is a release gate run
  locally with --release, which also fails when any of the five expected repositories is absent:
    scripts/verify-sylvode-brand-residue.sh --json --strict --release \
      --repo openpr-webhook=../openpr-webhook --repo docs=../docs \
      --repo site=../openprx-site --repo .github=../openprx-github

Options:
  --json              Required; the report is JSON on stdout.
  --repo NAME=PATH    Scan another checkout under NAME (repeatable).
  --allowlist FILE    Default: scripts/contracts/sylvode-brand-allowlist.json
  --strict            An allow-list entry that covers nothing in a scanned repository fails
                      (otherwise it is counted as a warning).
  --release           Require all five expected repositories, each with a clean working tree.
  --self-test         Run the built-in mutation controls against a temporary copy of this
                      checkout: injected uncovered names (mixed case, all caps, camel case, a
                      file name, a link target, UTF-16 text), an internal identifier outside
                      source code, a dirty checkout under --release, a deleted entry that is in
                      use, a blanket entry, an empty repository directory and a stale entry.

Exit codes: 0 clean; 1 uncovered hits, an unreachable or empty repository, a missing expected
repository, a dirty checkout under --release, a stale entry under --strict, or a failed mutation
control; 2 usage error or a
refused allow-list (malformed, unknown reason code, or a blanket waiver).
EOF
}

while (($#)); do
  case "$1" in
    --json) JSON_MODE=1; shift ;;
    --repo) REPOS+=("${2:?--repo requires NAME=PATH}"); shift 2 ;;
    --allowlist) ALLOWLIST="${2:?--allowlist requires a path}"; shift 2 ;;
    --strict) STRICT=1; shift ;;
    --release) RELEASE=1; shift ;;
    --self-test) SELF_TEST=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "FAIL: unsupported argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done
[[ $JSON_MODE -eq 1 ]] || { echo "FAIL: --json is required" >&2; exit 2; }
command -v python3 >/dev/null || { echo "FAIL: python3 is required" >&2; exit 2; }
command -v git >/dev/null || { echo "FAIL: git is required" >&2; exit 2; }

if [[ -n ${SYLVODE_BRAND_RESIDUE_REPOS:-} ]]; then
  IFS=',' read -r -a env_repos <<<"$SYLVODE_BRAND_RESIDUE_REPOS"
  for item in "${env_repos[@]}"; do
    [[ -n $item ]] && REPOS+=("$item")
  done
fi

args=(--allowlist "$ALLOWLIST" --repo "sylvode=$REPO_ROOT")
for item in "${REPOS[@]}"; do
  args+=(--repo "$item")
done
[[ $STRICT -eq 1 ]] && args+=(--strict)
[[ $RELEASE -eq 1 ]] && args+=(--release)
if [[ $SELF_TEST -eq 1 ]]; then
  args=(--allowlist "$ALLOWLIST" --repo "sylvode=$REPO_ROOT" --self-test)
fi

exec python3 "$REPO_ROOT/scripts/lib/sylvode_brand_residue.py" "${args[@]}"
