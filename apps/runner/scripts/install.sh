#!/usr/bin/env bash
# install.sh — Cross-platform installer for Sylvode Webhook
#
# Supports:
#   Linux  — installs a systemd user service (sylvode-webhook.service)
#   macOS  — installs a launchd user agent (dev.sylvode.webhook)
#
# An existing installation under the legacy names (openpr-webhook.service,
# dev.openpr.webhook) is updated in place under its old name, so units that
# are already enabled keep working; the installer prints a deprecation
# notice. Having both the legacy and the canonical service installed is an
# error, because both would bind the same port.
#
# Usage:
#   ./scripts/install.sh [--binary-path <path>] [--config-dir <path>]
#
# Options:
#   --binary-path PATH   Path to the sylvode-webhook binary.
#                        Default: ./sylvode-webhook, then
#                                 ./target/release/sylvode-webhook, then the
#                                 Sylvode workspace target/release; the
#                                 legacy openpr-webhook name is tried last
#   --config-dir  PATH   Directory that contains config.toml (and will be
#                        used as the service working directory).
#                        Default: current working directory
#
# Examples:
#   # Use defaults (binary in cwd, config in cwd)
#   ./scripts/install.sh
#
#   # Explicit paths
#   ./scripts/install.sh \
#       --binary-path /usr/local/bin/sylvode-webhook \
#       --config-dir  /etc/sylvode-webhook

set -euo pipefail

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

log()  { printf '\033[1;32m[install]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[warn]\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31m[error]\033[0m %s\n' "$*" >&2; exit 1; }
# deprecated KIND OLD NEW
deprecated() {
    warn "the $1 $2 is deprecated, use $3 instead; $2 is not removed before Sylvode v2.0"
}

CANONICAL_NAME="sylvode-webhook"
LEGACY_NAME="openpr-webhook"
CANONICAL_LABEL="dev.sylvode.webhook"
LEGACY_LABEL="dev.openpr.webhook"

# resolve_path: canonicalise a path without requiring it to exist yet.
# Falls back to a simple absolute-path construction when realpath is absent.
resolve_path() {
    local p="$1"
    if command -v realpath &>/dev/null; then
        realpath -m -- "$p" 2>/dev/null || echo "$p"
    else
        case "$p" in
            /*) echo "$p" ;;
            *)  echo "$PWD/$p" ;;
        esac
    fi
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

BINARY_PATH=""
CONFIG_DIR=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --binary-path)
            [[ -n "${2-}" ]] || die "--binary-path requires an argument"
            BINARY_PATH="$(resolve_path "$2")"
            shift 2
            ;;
        --config-dir)
            [[ -n "${2-}" ]] || die "--config-dir requires an argument"
            CONFIG_DIR="$(resolve_path "$2")"
            shift 2
            ;;
        -h|--help)
            sed -n '2,/^set -/p' "$0" | grep '^#' | sed 's/^# \?//'
            exit 0
            ;;
        *)
            die "Unknown option: $1  (try --help)"
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Detect OS
# ---------------------------------------------------------------------------

OS="$(uname -s)"
case "$OS" in
    Linux)  PLATFORM="linux"  ;;
    Darwin) PLATFORM="macos"  ;;
    *)      die "Unsupported OS: $OS. Only Linux and macOS are supported." ;;
esac

log "Detected platform: $PLATFORM"

# ---------------------------------------------------------------------------
# Locate the script's own directory so we can find templates
# ---------------------------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ---------------------------------------------------------------------------
# Resolve binary path
# ---------------------------------------------------------------------------

if [[ -z "$BINARY_PATH" ]]; then
    # Try common locations in order of preference.
    for candidate in \
        "$PWD/$CANONICAL_NAME" \
        "$PWD/target/release/$CANONICAL_NAME" \
        "$SCRIPT_DIR/../$CANONICAL_NAME" \
        "$SCRIPT_DIR/../target/release/$CANONICAL_NAME" \
        "$SCRIPT_DIR/../../../target/release/$CANONICAL_NAME" \
        "$PWD/$LEGACY_NAME" \
        "$PWD/target/release/$LEGACY_NAME" \
        "$SCRIPT_DIR/../$LEGACY_NAME" \
        "$SCRIPT_DIR/../target/release/$LEGACY_NAME" \
        "$SCRIPT_DIR/../../../target/release/$LEGACY_NAME"
    do
        if [[ -f "$candidate" ]]; then
            BINARY_PATH="$(resolve_path "$candidate")"
            log "Auto-detected binary: $BINARY_PATH"
            break
        fi
    done
fi

[[ -n "$BINARY_PATH" ]] \
    || die "Could not locate the $CANONICAL_NAME binary. Build it first with
       'cargo build --release', then re-run with --binary-path <path>."

[[ -f "$BINARY_PATH" ]] \
    || die "Binary not found: $BINARY_PATH"

[[ -x "$BINARY_PATH" ]] \
    || die "Binary is not executable: $BINARY_PATH
       Run: chmod +x $BINARY_PATH"

log "Binary: $BINARY_PATH"
if [[ "$(basename "$BINARY_PATH")" == "$LEGACY_NAME" ]]; then
    deprecated "executable name" "$LEGACY_NAME" "$CANONICAL_NAME"
fi

# ---------------------------------------------------------------------------
# Resolve config directory
# ---------------------------------------------------------------------------

if [[ -z "$CONFIG_DIR" ]]; then
    CONFIG_DIR="$PWD"
    log "Auto-detected config directory: $CONFIG_DIR (current directory)"
fi

[[ -d "$CONFIG_DIR" ]] \
    || die "Config directory does not exist: $CONFIG_DIR"

[[ -f "$CONFIG_DIR/config.toml" ]] \
    || die "config.toml not found in $CONFIG_DIR
       Copy config.example.toml to $CONFIG_DIR/config.toml and edit it first."

log "Config directory: $CONFIG_DIR"

# ---------------------------------------------------------------------------
# Build PATH string to embed in the service
#
# We capture the user's current PATH and augment it with the most common
# locations where tools like node, cargo, and homebrew binaries live.
# ---------------------------------------------------------------------------

EXTRA_PATHS=(
    "$HOME/.cargo/bin"
    "${NVM_BIN:-}"             # the node of the active nvm version, if any
    "$HOME/.local/bin"
    "/usr/local/bin"
    "/opt/homebrew/bin"        # macOS Apple Silicon Homebrew
    "/usr/local/opt/node/bin"  # macOS Homebrew node (Intel)
    "/usr/bin"
    "/bin"
    "/usr/sbin"
    "/sbin"
)

# Start from the extras, then the current PATH, dropping empty and repeated entries. Plain
# string matching keeps this working in the bash 3.2 that macOS ships (no associative arrays).
SERVICE_PATH=""
IFS=: read -ra _existing_paths <<< "$PATH"
for p in "${EXTRA_PATHS[@]}" "${_existing_paths[@]}"; do
    [[ -z "$p" ]] && continue
    case ":$SERVICE_PATH:" in
        *":$p:"*) ;;
        *) SERVICE_PATH="${SERVICE_PATH:+$SERVICE_PATH:}$p" ;;
    esac
done

log "Embedded PATH: $SERVICE_PATH"

# ---------------------------------------------------------------------------
# Locate template files
# ---------------------------------------------------------------------------

SYSTEMD_TEMPLATE="$SCRIPT_DIR/sylvode-webhook.service.template"
LAUNCHD_TEMPLATE="$SCRIPT_DIR/dev.sylvode.webhook.plist.template"

if [[ "$PLATFORM" == "linux" && ! -f "$SYSTEMD_TEMPLATE" ]]; then
    die "systemd template not found: $SYSTEMD_TEMPLATE"
fi
if [[ "$PLATFORM" == "macos" && ! -f "$LAUNCHD_TEMPLATE" ]]; then
    die "launchd template not found: $LAUNCHD_TEMPLATE"
fi

# ---------------------------------------------------------------------------
# render_template: replace {{KEY}} tokens in a template file.
# Usage: render_template <input-file> <output-file> <escape-function> KEY=VALUE...
# Each value is passed through the escape function for the target format and inserted
# literally (no sed, so `&`, `\` and `/` in paths are safe).
# ---------------------------------------------------------------------------

# bash 5.2 expands `&` in the replacement of ${var//pattern/replacement}; turn that off.
shopt -u patsub_replacement 2>/dev/null || true

render_template() {
    local src="$1" dst="$2" escape="$3"
    shift 3
    local content pair key val
    content="$(cat "$src")"
    for pair in "$@"; do
        key="${pair%%=*}"
        val="$("$escape" "${pair#*=}")"
        # The replacement is left unquoted: bash before 4.3 (macOS ships 3.2) keeps the quotes
        # of a quoted replacement inside a double-quoted word, so "$val" would insert literal
        # quote characters. Unquoted it is not split or globbed here, and its expansion is not
        # subject to quote removal in any version, so the value is inserted byte for byte.
        content="${content//"{{$key}}"/$val}"
    done
    printf '%s\n' "$content" > "$dst"
}

# A value inside a double-quoted systemd setting (Environment=, the program of ExecStart=):
# escape \ and ", and double % (specifier).
systemd_quoted_escape() {
    local v="$1"
    v="${v//\\/\\\\}"
    v="${v//\"/\\\"}"
    v="${v//%/%%}"
    printf '%s' "$v"
}

# An argument of ExecStart= inside double quotes: as above, and $ doubled, because systemd
# expands $VARIABLE in arguments.
systemd_arg_escape() {
    local v
    v="$(systemd_quoted_escape "$1")"
    printf '%s' "${v//\$/\$\$}"
}

# A value of an unquoted systemd path setting (WorkingDirectory=): only % (specifier) is special.
systemd_path_escape() {
    printf '%s' "${1//%/%%}"
}

# A value inside a plist <string>: escape the XML special characters.
xml_escape() {
    local v="$1"
    v="${v//&/&amp;}"
    v="${v//</&lt;}"
    v="${v//>/&gt;}"
    printf '%s' "$v"
}

# ---------------------------------------------------------------------------
# Platform: Linux — systemd user service
# ---------------------------------------------------------------------------

install_linux() {
    local service_dir="$HOME/.config/systemd/user"
    local canonical_file="$service_dir/$CANONICAL_NAME.service"
    local legacy_file="$service_dir/$LEGACY_NAME.service"
    local unit_name="$CANONICAL_NAME"

    if [[ -f "$canonical_file" && -f "$legacy_file" ]]; then
        die "Both $canonical_file and $legacy_file exist; they would bind the same port.
       Stop and remove the legacy unit first:
         systemctl --user disable --now $LEGACY_NAME && rm $legacy_file"
    fi
    if [[ -f "$legacy_file" ]]; then
        unit_name="$LEGACY_NAME"
        deprecated "systemd unit" "$LEGACY_NAME.service" "$CANONICAL_NAME.service"
        warn "Updating the existing legacy unit in place. To migrate, run ./scripts/uninstall.sh and install again."
    fi
    local service_file="$service_dir/$unit_name.service"

    mkdir -p "$service_dir"

    render_template "$SYSTEMD_TEMPLATE" "$service_file" systemd_quoted_escape \
        "BINARY_PATH=$BINARY_PATH" \
        "PATH=$SERVICE_PATH" \
        "UNIT_NAME=$unit_name"
    render_template "$service_file" "$service_file" systemd_arg_escape \
        "CONFIG_DIR=$CONFIG_DIR"
    render_template "$service_file" "$service_file" systemd_path_escape \
        "WORKING_DIRECTORY=$CONFIG_DIR"
    log "Installed service file: $service_file"

    # Reload the systemd daemon so it picks up the new unit file.
    if command -v systemctl &>/dev/null; then
        systemctl --user daemon-reload
        log "systemd user daemon reloaded"
    else
        warn "systemctl not found; skipping daemon-reload. Run manually later."
    fi

    cat <<EOF

  Installation complete (Linux / systemd user service).

  Start the service:
    systemctl --user start $unit_name

  Enable auto-start on login:
    systemctl --user enable $unit_name

  Stop the service:
    systemctl --user stop $unit_name

  View logs:
    journalctl --user -u $unit_name -f

  Service file:
    $service_file

EOF
}

# ---------------------------------------------------------------------------
# Platform: macOS — launchd user agent
# ---------------------------------------------------------------------------

install_macos() {
    local agents_dir="$HOME/Library/LaunchAgents"
    local log_dir="$HOME/Library/Logs"
    local label="$CANONICAL_LABEL"
    local log_name="$CANONICAL_NAME"

    if [[ -f "$agents_dir/$CANONICAL_LABEL.plist" && -f "$agents_dir/$LEGACY_LABEL.plist" ]]; then
        die "Both $CANONICAL_LABEL and $LEGACY_LABEL are installed; they would bind the same port.
       Unload and remove the legacy agent first:
         launchctl unload $agents_dir/$LEGACY_LABEL.plist && rm $agents_dir/$LEGACY_LABEL.plist"
    fi
    if [[ -f "$agents_dir/$LEGACY_LABEL.plist" ]]; then
        label="$LEGACY_LABEL"
        log_name="$LEGACY_NAME"
        deprecated "launchd label" "$LEGACY_LABEL" "$CANONICAL_LABEL"
        warn "Updating the existing legacy agent in place. To migrate, run ./scripts/uninstall.sh and install again."
    fi
    local plist_file="$agents_dir/$label.plist"

    mkdir -p "$agents_dir" "$log_dir"

    render_template "$LAUNCHD_TEMPLATE" "$plist_file" xml_escape \
        "BINARY_PATH=$BINARY_PATH" \
        "CONFIG_DIR=$CONFIG_DIR" \
        "PATH=$SERVICE_PATH" \
        "LOG_DIR=$log_dir" \
        "LABEL=$label" \
        "LOG_NAME=$log_name"
    log "Installed plist file: $plist_file"

    # Validate the generated plist before loading it.
    if command -v plutil &>/dev/null; then
        if plutil -lint "$plist_file" &>/dev/null; then
            log "Plist syntax OK"
        else
            warn "plutil reported issues with $plist_file — check and reload manually."
        fi
    fi

    # If the agent is already loaded, unload it first to pick up changes.
    if launchctl list 2>/dev/null | grep -q "$label"; then
        warn "Agent already loaded; unloading first to apply changes."
        launchctl unload "$plist_file" 2>/dev/null || true
    fi

    launchctl load "$plist_file"
    log "Agent loaded via launchctl"

    cat <<EOF

  Installation complete (macOS / launchd user agent).

  Start the agent (loads and starts automatically on login):
    launchctl load ~/Library/LaunchAgents/$label.plist

  Start immediately without waiting for login:
    launchctl start $label

  Stop the agent:
    launchctl stop $label

  Unload (disable auto-start):
    launchctl unload ~/Library/LaunchAgents/$label.plist

  View logs:
    tail -f ~/Library/Logs/$log_name.log
    tail -f ~/Library/Logs/$log_name.error.log

  Plist file:
    $plist_file

EOF
}

# ---------------------------------------------------------------------------
# Dispatch
# ---------------------------------------------------------------------------

case "$PLATFORM" in
    linux) install_linux ;;
    macos) install_macos ;;
esac
