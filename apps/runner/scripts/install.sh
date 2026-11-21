#!/usr/bin/env bash
# install.sh — Cross-platform installer for the Sylvode runner
#
# Supports:
#   Linux  — installs a systemd user service (sylvode-runner.service)
#   macOS  — installs a launchd user agent (dev.sylvode.runner)
#
# Running it again rewrites the unit or agent in place.
#
# Usage:
#   ./scripts/install.sh [--binary-path <path>] [--config-dir <path>] [--headless-only] [--dry-run]
#
# Options:
#   --binary-path PATH   Path to the sylvode-runner binary.
#                        Default: ./sylvode-runner, then
#                                 ./target/release/sylvode-runner, then the
#                                 Sylvode workspace target/release
#   --config-dir  PATH   Directory that contains config.toml (and will be
#                        used as the service working directory).
#                        Default: current working directory
#   --headless-only      Check for headless runs only: a missing tmux is a
#                        warning, not a failure.
#   --dry-run            Run the prerequisite check and stop: nothing is
#                        installed.
#
# Before anything is installed the installer runs `sylvode-runner doctor` with
# the PATH the service will have; a failed row stops the installation and its
# hint names what to install. After the unit is installed, doctor runs once more
# in the service's context.
#
# Examples:
#   # Use defaults (binary in cwd, config in cwd)
#   ./scripts/install.sh
#
#   # Explicit paths
#   ./scripts/install.sh \
#       --binary-path /usr/local/bin/sylvode-runner \
#       --config-dir  /etc/sylvode-runner

set -euo pipefail

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

log()  { printf '\033[1;32m[install]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[warn]\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31m[error]\033[0m %s\n' "$*" >&2; exit 1; }
NAME="sylvode-runner"
LABEL="dev.sylvode.runner"

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
HEADLESS_ONLY=0
DRY_RUN=0

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
        --headless-only)
            HEADLESS_ONLY=1
            shift
            ;;
        --dry-run)
            DRY_RUN=1
            shift
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
        "$PWD/$NAME" \
        "$PWD/target/release/$NAME" \
        "$SCRIPT_DIR/../$NAME" \
        "$SCRIPT_DIR/../target/release/$NAME" \
        "$SCRIPT_DIR/../../../target/release/$NAME"
    do
        if [[ -f "$candidate" ]]; then
            BINARY_PATH="$(resolve_path "$candidate")"
            log "Auto-detected binary: $BINARY_PATH"
            break
        fi
    done
fi

[[ -n "$BINARY_PATH" ]] \
    || die "Could not locate the $NAME binary. Build it first with
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
# Prerequisites: doctor, with the PATH the service will have (not the shell's)
#
# Mode: tmux, unless --headless-only is given or the config has no runner agent (auto).
# Exit 2 means a row failed: its hint is printed and nothing is installed. Nothing is ever
# installed silently.
# ---------------------------------------------------------------------------

DOCTOR_MODE="tmux"
if [[ "$HEADLESS_ONLY" -eq 1 ]]; then
    DOCTOR_MODE="headless"
elif ! grep -Eq '^[[:space:]]*agent_type[[:space:]]*=[[:space:]]*"runner"' "$CONFIG_DIR/config.toml"; then
    DOCTOR_MODE="auto"
fi

log "Checking prerequisites (mode: $DOCTOR_MODE, PATH of the service)"
set +e
DOCTOR_OUT="$(PATH="$SERVICE_PATH" "$BINARY_PATH" doctor --mode "$DOCTOR_MODE" --config "$CONFIG_DIR/config.toml" --as-service 2>&1)"
DOCTOR_RC=$?
set -e
printf '%s\n' "$DOCTOR_OUT"
if [[ "$HEADLESS_ONLY" -eq 1 ]]; then
    warn "headless-only mode: tmux sessions will not be available"
fi
case "$DOCTOR_RC" in
    0) ;;
    2) die "a prerequisite is missing (see the rows above and their hints); nothing was installed" ;;
    *) die "doctor could not run (exit $DOCTOR_RC); nothing was installed" ;;
esac

# On Linux the service and the tmux server stop at logout unless the user lingers.
check_linger() {
    [[ "$PLATFORM" == "linux" ]] || return 0
    command -v loginctl &>/dev/null || return 0
    local user="${USER:-$(id -un)}" state
    state="$(loginctl show-user "$user" -p Linger 2>/dev/null || true)"
    if [[ "$state" == *"Linger=no"* ]]; then
        warn "linger is off: the service and its tmux sessions stop when you log out. Run: loginctl enable-linger $user"
    fi
}

if [[ "$DRY_RUN" -eq 1 ]]; then
    check_linger
    log "Dry run: nothing was installed."
    exit 0
fi

# After the unit is installed: doctor once more, in the service's context. A failure here is a
# warning (the service may not be running yet).
doctor_in_service_context() {
    local runner=()
    case "$PLATFORM" in
        linux)
            command -v systemd-run &>/dev/null || return 0
            runner=(systemd-run --user --pipe --wait "--setenv=PATH=$SERVICE_PATH")
            ;;
        macos)
            runner=(launchctl asuser "$(id -u)")
            ;;
    esac
    log "Checking prerequisites in the service context"
    if ! "${runner[@]}" "$BINARY_PATH" doctor --mode "$DOCTOR_MODE" --config "$CONFIG_DIR/config.toml" --as-service; then
        warn "doctor reported a problem in the service context (see above); the service may not work yet"
    fi
}

# ---------------------------------------------------------------------------
# Locate template files
# ---------------------------------------------------------------------------

SYSTEMD_TEMPLATE="$SCRIPT_DIR/sylvode-runner.service.template"
LAUNCHD_TEMPLATE="$SCRIPT_DIR/dev.sylvode.runner.plist.template"

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
    local unit_name="$NAME"
    local service_file="$service_dir/$unit_name.service"

    mkdir -p "$service_dir"

    # Rendered beside the unit and put in place only when it differs: an unchanged unit is not
    # rewritten and the daemon is not reloaded.
    local staged
    staged="$(mktemp "$service_dir/.$unit_name.XXXXXX")"
    render_template "$SYSTEMD_TEMPLATE" "$staged" systemd_quoted_escape \
        "BINARY_PATH=$BINARY_PATH" \
        "PATH=$SERVICE_PATH" \
        "UNIT_NAME=$unit_name"
    render_template "$staged" "$staged" systemd_arg_escape \
        "CONFIG_DIR=$CONFIG_DIR"
    render_template "$staged" "$staged" systemd_path_escape \
        "WORKING_DIRECTORY=$CONFIG_DIR"
    if [[ -f "$service_file" ]] && cmp -s "$staged" "$service_file"; then
        rm -f "$staged"
        log "Service file unchanged: $service_file"
    else
        chmod 644 "$staged"
        mv -f "$staged" "$service_file"
        log "Installed service file: $service_file"
        # Reload the systemd daemon so it picks up the new unit file.
        if command -v systemctl &>/dev/null; then
            systemctl --user daemon-reload
            log "systemd user daemon reloaded"
        else
            warn "systemctl not found; skipping daemon-reload. Run manually later."
        fi
    fi
    check_linger
    doctor_in_service_context

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
    local label="$LABEL"
    local log_name="$NAME"
    local plist_file="$agents_dir/$label.plist"

    mkdir -p "$agents_dir" "$log_dir"

    local staged
    staged="$(mktemp "$agents_dir/.$label.XXXXXX")"
    render_template "$LAUNCHD_TEMPLATE" "$staged" xml_escape \
        "BINARY_PATH=$BINARY_PATH" \
        "CONFIG_DIR=$CONFIG_DIR" \
        "PATH=$SERVICE_PATH" \
        "LOG_DIR=$log_dir" \
        "LABEL=$label" \
        "LOG_NAME=$log_name"

    # Validate the generated plist before putting it in place.
    if command -v plutil &>/dev/null; then
        if plutil -lint "$staged" &>/dev/null; then
            log "Plist syntax OK"
        else
            warn "plutil reported issues with the generated plist — check and reload manually."
        fi
    fi

    # An unchanged agent that is loaded is left alone; a changed one is unloaded and loaded again.
    local loaded=0 changed=1
    if launchctl list 2>/dev/null | grep -q "$label"; then
        loaded=1
    fi
    if [[ -f "$plist_file" ]] && cmp -s "$staged" "$plist_file"; then
        changed=0
        rm -f "$staged"
        log "Plist file unchanged: $plist_file"
    else
        mv -f "$staged" "$plist_file"
        log "Installed plist file: $plist_file"
    fi
    if [[ "$loaded" -eq 1 && "$changed" -eq 1 ]]; then
        warn "Agent already loaded; unloading first to apply changes."
        launchctl unload "$plist_file" 2>/dev/null || true
        loaded=0
    fi
    if [[ "$loaded" -eq 0 ]]; then
        launchctl load "$plist_file"
        log "Agent loaded via launchctl"
    fi
    doctor_in_service_context

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
