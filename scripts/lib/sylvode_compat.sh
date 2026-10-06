#!/usr/bin/env bash

# Shared, side-effect-free compatibility decisions for the Sylvode transition.
# Callers remain responsible for exporting the selected values.
#
# Every function prints its result on stdout and nothing else there: callers read it through
# command substitution. Deprecation notices (ADR-0020 D2) go to stderr only, one line per
# distinct legacy name, and a notice that cannot be written never fails the caller.
#
# A command substitution runs in a subshell, so the record of which names were already
# reported cannot survive it. A caller that resolves several names in one run should use the
# *_into variants, which assign the result to a variable in the calling shell and therefore
# report each legacy name once per run; the stdout variants report once per call.

# Legacy names already reported in this shell.
if ! declare -p SYLVODE_COMPAT_REPORTED >/dev/null 2>&1; then
  declare -gA SYLVODE_COMPAT_REPORTED=()
fi

# The single source of the shell-side notice text. It carries the legacy name, the
# replacement and the earliest removal release; crates/platform/src/deprecation.rs holds the
# binaries' texts and a test there checks that this file uses the same removal clause.
sylvode_deprecation_notice() {
  local kind="$1" legacy="$2" canonical="$3"
  printf 'warning: legacy %s %s is deprecated; use %s instead (%s is not removed before Sylvode v2.0)\n' \
    "$kind" "$legacy" "$canonical" "$legacy"
}

# Writes the notice for one legacy name to stderr unless this shell already reported it.
sylvode_report_legacy() {
  local kind="$1" legacy="$2" canonical="$3"
  [[ -z "${SYLVODE_COMPAT_REPORTED[$legacy]+reported}" ]] || return 0
  SYLVODE_COMPAT_REPORTED[$legacy]=1
  sylvode_deprecation_notice "$kind" "$legacy" "$canonical" >&2 2>/dev/null || true
}

# Decides between a canonical and a legacy file without reporting anything.
sylvode_choose_config() {
  local canonical="$1" legacy="$2"
  if [[ -e "$canonical" && -e "$legacy" ]]; then
    echo "Both $canonical and legacy $legacy exist; refusing silent precedence." >&2
    return 1
  fi
  if [[ -e "$canonical" || ! -e "$legacy" ]]; then
    printf '%s\n' "$canonical"
  else
    printf '%s\n' "$legacy"
  fi
}

# Assigns the configuration file to use to the variable named by $1, reporting a legacy pick.
sylvode_select_config_into() {
  local -n sylvode_selected_config="$1"
  local canonical="$2" legacy="$3" chosen
  chosen=$(sylvode_choose_config "$canonical" "$legacy") || return 1
  if [[ "$chosen" == "$legacy" ]]; then
    sylvode_report_legacy "configuration file" "$legacy" "$canonical"
  fi
  # shellcheck disable=SC2034 # assigned through the nameref
  sylvode_selected_config=$chosen
}

sylvode_select_config() {
  local selected
  sylvode_select_config_into selected "$1" "$2" || return 1
  printf '%s\n' "$selected"
}

sylvode_env_value() {
  local env_file="$1" key="$2" value
  [[ -f "$env_file" ]] || return 0
  value=$(grep -E "^${key}=" "$env_file" | tail -n 1 | cut -d= -f2- || true)
  # docker-compose reads KEY="value" and KEY='value' as value; so must every reader here, or the
  # quotes end up inside generated files.
  if [[ ${#value} -ge 2 && ( ( $value == \"*\" ) || ( $value == \'*\' ) ) ]]; then
    value=${value:1:${#value}-2}
  fi
  printf '%s\n' "$value"
}

sylvode_configured_env_value() {
  local env_file="$1" key="$2"
  if [[ -v $key ]]; then
    printf '%s\n' "${!key}"
  else
    sylvode_env_value "$env_file" "$key"
  fi
}

# Assigns the resolved value of a compose variable to the variable named by $1, reporting the
# legacy name when it is set. An optional sixth argument names the kind of variable in that
# notice (default "compose variable"); a script reading its own inputs passes
# "environment variable".
sylvode_resolve_env_into() {
  local -n sylvode_resolved_value="$1"
  local env_file="$2" canonical_key="$3" legacy_key="$4" fallback="$5" kind="${6:-compose variable}"
  local canonical_value legacy_value
  canonical_value=$(sylvode_configured_env_value "$env_file" "$canonical_key")
  legacy_value=$(sylvode_configured_env_value "$env_file" "$legacy_key")
  if [[ -n "$canonical_value" && -n "$legacy_value" && "$canonical_value" != "$legacy_value" ]]; then
    echo "$canonical_key conflicts with legacy $legacy_key; refusing silent precedence." >&2
    return 1
  fi
  if [[ -n "$legacy_value" ]]; then
    sylvode_report_legacy "$kind" "$legacy_key" "$canonical_key"
  fi
  # shellcheck disable=SC2034 # assigned through the nameref
  sylvode_resolved_value=${canonical_value:-${legacy_value:-$fallback}}
}

sylvode_resolve_env() {
  local resolved
  sylvode_resolve_env_into resolved "$1" "$2" "$3" "$4" || return 1
  printf '%s\n' "$resolved"
}
