#!/usr/bin/env bash
# install.sh — Installer for the Sylvode MCP server
#
# Checks the binary and its configuration, prints the exact commands that register the server
# in your agent CLI, and ends by running `mcp-server doctor`.
#
# It never registers anything: the registration commands are printed for you to run. It never
# installs tmux, a CLI or the runner either, and it never rewrites an existing configuration.
# The only thing it writes is the configuration directory, when it does not exist yet.
#
# Usage:
#   ./scripts/install.sh [--binary-path <path>] [--config-dir <path>] [--dry-run]
#
# Options:
#   --binary-path PATH   Path to the mcp-server binary.
#                        Default: ./mcp-server, then ./target/release/mcp-server, then the
#                                 Sylvode workspace target/release
#   --config-dir  PATH   Directory that holds config.toml (the MCP server configuration; it
#                        holds the bot token, so keep it readable by you only: chmod 600).
#                        Default: $HOME/.config/sylvode-mcp
#   --dry-run            Write nothing. Report what would be created, then run doctor.
#
# Examples:
#   ./scripts/install.sh --binary-path ./mcp-server --config-dir ~/.config/sylvode-mcp
#   ./scripts/install.sh --dry-run

set -euo pipefail

log()  { printf '\033[1;32m[install]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[warn]\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31m[error]\033[0m %s\n' "$*" >&2; exit 1; }
NAME="mcp-server"
REGISTERED_NAME="sylvode"

# resolve_path: canonicalise a path without requiring it to exist yet.
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

# shell_word: a value as one shell word, quoted only when it needs it.
shell_word() {
    local v="$1" q="'"
    if [[ -n "$v" && "$v" =~ ^[A-Za-z0-9_./:@%+=,-]+$ ]]; then
        printf '%s' "$v"
    else
        printf '%s%s%s' "$q" "${v//$q/$q\\$q$q}" "$q"
    fi
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

BINARY_PATH=""
CONFIG_DIR=""
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
    Linux)  PLATFORM="linux" ;;
    Darwin) PLATFORM="macos" ;;
    *)      die "Unsupported OS: $OS. Only Linux and macOS are supported." ;;
esac
log "Detected platform: $PLATFORM"
[[ "$DRY_RUN" -eq 0 ]] || log "Dry run: nothing will be written"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ---------------------------------------------------------------------------
# Resolve and check the binary
# ---------------------------------------------------------------------------

if [[ -z "$BINARY_PATH" ]]; then
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
       'cargo build --release -p mcp-server', then re-run with --binary-path <path>."
[[ -f "$BINARY_PATH" ]] || die "Binary not found: $BINARY_PATH"
[[ -x "$BINARY_PATH" ]] \
    || die "Binary is not executable: $BINARY_PATH
       Run: chmod +x $BINARY_PATH"

# --check reads no secrets and probes nothing: it only proves the file is this server.
"$BINARY_PATH" --check >/dev/null 2>&1 \
    || die "$BINARY_PATH does not answer --check; it is not a usable mcp-server binary."
log "Binary: $BINARY_PATH"

# ---------------------------------------------------------------------------
# Configuration directory and file
# ---------------------------------------------------------------------------

if [[ -z "$CONFIG_DIR" ]]; then
    CONFIG_DIR="$(resolve_path "$HOME/.config/sylvode-mcp")"
    log "Default config directory: $CONFIG_DIR"
fi
CONFIG_FILE="$CONFIG_DIR/config.toml"

if [[ ! -d "$CONFIG_DIR" ]]; then
    if [[ "$DRY_RUN" -eq 1 ]]; then
        log "Would create the config directory: $CONFIG_DIR"
    else
        mkdir -p "$CONFIG_DIR"
        chmod 700 "$CONFIG_DIR"
        log "Created the config directory: $CONFIG_DIR"
    fi
fi

if [[ -f "$CONFIG_FILE" ]]; then
    # The first field of `ls -l` is the mode string: positions 5-10 are group and others.
    mode_string="$(ls -ld -- "$CONFIG_FILE" | cut -c1-10)"
    if [[ "${mode_string:4:6}" != "------" ]]; then
        warn "$CONFIG_FILE holds the bot token and others can read it ($mode_string). Run: chmod 600 $(shell_word "$CONFIG_FILE")"
    else
        log "Config file: $CONFIG_FILE (mode ok)"
    fi
else
    warn "No configuration at $CONFIG_FILE. Copy config/sylvode.example.toml there, fill in [mcp] (api_url, bot_token, workspace_id) and run: chmod 600 $(shell_word "$CONFIG_FILE")"
fi

# ---------------------------------------------------------------------------
# The registration commands: printed, never run
# ---------------------------------------------------------------------------

SERVER_ARGS="$REGISTERED_NAME -- $(shell_word "$BINARY_PATH") serve --transport stdio --config $(shell_word "$CONFIG_FILE")"
cat <<MSG

  Register the server in your agent CLI (run the one you use; nothing was registered):

    claude mcp add --scope user $SERVER_ARGS
    codex mcp add $SERVER_ARGS

  opencode: add an "mcp" entry named $REGISTERED_NAME to opencode.json that runs
    $(shell_word "$BINARY_PATH") serve --transport stdio --config $(shell_word "$CONFIG_FILE")

  Task delivery to a CLI needs sylvode-runner and tmux as well; their installers are
  apps/runner/scripts/install.sh and your package manager (brew install tmux, sudo apt install
  tmux, sudo dnf install tmux or sudo pacman -S tmux).

MSG

# ---------------------------------------------------------------------------
# Doctor
# ---------------------------------------------------------------------------

doctor_args=(doctor --config "$CONFIG_FILE")
for cli in claude codex opencode; do
    if command -v "$cli" &>/dev/null; then
        doctor_args+=(--cli "$cli")
    fi
done

log "Running doctor"
set +e
"$BINARY_PATH" "${doctor_args[@]}"
rc=$?
set -e
case "$rc" in
    0) log "doctor: no failures" ;;
    2) die "doctor reported at least one failure (see the table above)." ;;
    *) die "doctor could not run (exit $rc)." ;;
esac
