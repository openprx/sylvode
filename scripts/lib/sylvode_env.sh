#!/usr/bin/env bash

# Shared, side-effect-free readers for the compose environment file.
# Callers remain responsible for exporting the selected values. Every function prints its
# result on stdout and nothing else there: callers read it through command substitution.

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

# The value of KEY from the environment when it is set there, else from the env file.
sylvode_configured_env_value() {
  local env_file="$1" key="$2"
  if [[ -v $key ]]; then
    printf '%s\n' "${!key}"
  else
    sylvode_env_value "$env_file" "$key"
  fi
}

# Assigns the value of a compose variable to the variable named by $1: the environment or the
# env file, else the fallback.
sylvode_resolve_env_into() {
  local -n sylvode_resolved_value="$1"
  local env_file="$2" key="$3" fallback="$4" value
  value=$(sylvode_configured_env_value "$env_file" "$key")
  # shellcheck disable=SC2034 # assigned through the nameref
  sylvode_resolved_value=${value:-$fallback}
}
