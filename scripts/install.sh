#!/usr/bin/env bash
# install.sh — Cross-platform installer for openpr-webhook
#
# Supports:
#   Linux  — installs a systemd user service
#   macOS  — installs a launchd user agent (LaunchAgents plist)
#
# Usage:
#   ./scripts/install.sh [--binary-path <path>] [--config-dir <path>]
#
# Options:
#   --binary-path PATH   Path to the openpr-webhook binary.
#                        Default: ./openpr-webhook, then
#                                 ./target/release/openpr-webhook
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
#       --binary-path /usr/local/bin/openpr-webhook \
#       --config-dir  /etc/openpr-webhook

set -euo pipefail

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

log()  { printf '\033[1;32m[install]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[warn]\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31m[error]\033[0m %s\n' "$*" >&2; exit 1; }

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
        "$PWD/openpr-webhook" \
        "$PWD/target/release/openpr-webhook" \
        "$SCRIPT_DIR/../openpr-webhook" \
        "$SCRIPT_DIR/../target/release/openpr-webhook"
    do
        if [[ -f "$candidate" ]]; then
            BINARY_PATH="$(resolve_path "$candidate")"
            log "Auto-detected binary: $BINARY_PATH"
            break
        fi
    done
fi

[[ -n "$BINARY_PATH" ]] \
    || die "Could not locate the openpr-webhook binary. Build it first with
       'cargo build --release', then re-run with --binary-path <path>."

[[ -f "$BINARY_PATH" ]] \
    || die "Binary not found: $BINARY_PATH"

[[ -x "$BINARY_PATH" ]] \
    || die "Binary is not executable: $BINARY_PATH
       Run: chmod +x $BINARY_PATH"

log "Binary: $BINARY_PATH"

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
    "$HOME/.nvm/versions/node/v22.22.1/bin"  # nvm default on dev machines
    "$HOME/.nvm/versions/node/v20/bin"
    "$HOME/.local/bin"
    "/usr/local/bin"
    "/opt/homebrew/bin"   # macOS Apple Silicon Homebrew
    "/usr/local/opt/node/bin"  # macOS Homebrew node (Intel)
    "/usr/bin"
    "/bin"
    "/usr/sbin"
    "/sbin"
)

# Start from the current PATH and prepend the extras, deduplicating.
declare -A _seen
_seen_order=()
IFS=: read -ra _existing_paths <<< "$PATH"
for p in "${EXTRA_PATHS[@]}" "${_existing_paths[@]}"; do
    [[ -z "$p" ]] && continue
    if [[ -z "${_seen[$p]+set}" ]]; then
        _seen[$p]=1
        _seen_order+=("$p")
    fi
done

SERVICE_PATH="$(IFS=:; echo "${_seen_order[*]}")"
log "Embedded PATH: $SERVICE_PATH"

# ---------------------------------------------------------------------------
# Locate template files
# ---------------------------------------------------------------------------

SYSTEMD_TEMPLATE="$SCRIPT_DIR/openpr-webhook.service.template"
LAUNCHD_TEMPLATE="$SCRIPT_DIR/dev.openpr.webhook.plist.template"

if [[ "$PLATFORM" == "linux" && ! -f "$SYSTEMD_TEMPLATE" ]]; then
    die "systemd template not found: $SYSTEMD_TEMPLATE"
fi
if [[ "$PLATFORM" == "macos" && ! -f "$LAUNCHD_TEMPLATE" ]]; then
    die "launchd template not found: $LAUNCHD_TEMPLATE"
fi

# ---------------------------------------------------------------------------
# render_template: replace {{PLACEHOLDER}} tokens in a template file.
# Usage: render_template <input-file> <output-file>
# Substitutions are driven by the associative array TEMPLATE_VARS.
# ---------------------------------------------------------------------------

render_template() {
    local src="$1"
    local dst="$2"
    local content
    content="$(cat "$src")"

    # Apply each substitution in-place using bash parameter expansion.
    for key in "${!TEMPLATE_VARS[@]}"; do
        local val="${TEMPLATE_VARS[$key]}"
        # Escape forward slashes in the value so sed doesn't choke.
        local escaped_val="${val//\//\\/}"
        content="$(printf '%s' "$content" | sed "s/{{${key}}}/${escaped_val}/g")"
    done

    printf '%s\n' "$content" > "$dst"
}

# ---------------------------------------------------------------------------
# Platform: Linux — systemd user service
# ---------------------------------------------------------------------------

install_linux() {
    local service_dir="$HOME/.config/systemd/user"
    local service_file="$service_dir/openpr-webhook.service"

    mkdir -p "$service_dir"

    declare -A TEMPLATE_VARS=(
        [BINARY_PATH]="$BINARY_PATH"
        [CONFIG_DIR]="$CONFIG_DIR"
        [PATH]="$SERVICE_PATH"
    )

    render_template "$SYSTEMD_TEMPLATE" "$service_file"
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
    systemctl --user start openpr-webhook

  Enable auto-start on login:
    systemctl --user enable openpr-webhook

  Stop the service:
    systemctl --user stop openpr-webhook

  View logs:
    journalctl --user -u openpr-webhook -f

  Service file:
    $service_file

EOF
}

# ---------------------------------------------------------------------------
# Platform: macOS — launchd user agent
# ---------------------------------------------------------------------------

install_macos() {
    local agents_dir="$HOME/Library/LaunchAgents"
    local plist_file="$agents_dir/dev.openpr.webhook.plist"
    local log_dir="$HOME/Library/Logs"

    mkdir -p "$agents_dir" "$log_dir"

    declare -A TEMPLATE_VARS=(
        [BINARY_PATH]="$BINARY_PATH"
        [CONFIG_DIR]="$CONFIG_DIR"
        [PATH]="$SERVICE_PATH"
        [LOG_DIR]="$log_dir"
    )

    render_template "$LAUNCHD_TEMPLATE" "$plist_file"
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
    if launchctl list 2>/dev/null | grep -q "dev.openpr.webhook"; then
        warn "Agent already loaded; unloading first to apply changes."
        launchctl unload "$plist_file" 2>/dev/null || true
    fi

    launchctl load "$plist_file"
    log "Agent loaded via launchctl"

    cat <<EOF

  Installation complete (macOS / launchd user agent).

  Start the agent (loads and starts automatically on login):
    launchctl load ~/Library/LaunchAgents/dev.openpr.webhook.plist

  Start immediately without waiting for login:
    launchctl start dev.openpr.webhook

  Stop the agent:
    launchctl stop dev.openpr.webhook

  Unload (disable auto-start):
    launchctl unload ~/Library/LaunchAgents/dev.openpr.webhook.plist

  View logs:
    tail -f ~/Library/Logs/openpr-webhook.log
    tail -f ~/Library/Logs/openpr-webhook.error.log

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
