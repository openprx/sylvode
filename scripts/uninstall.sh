#!/usr/bin/env bash
# uninstall.sh — Remove the openpr-webhook system service
#
# Stops the running service (if any) and removes the service/agent file for
# both Linux (systemd user service) and macOS (launchd user agent).
#
# Usage:
#   ./scripts/uninstall.sh
#
# No options are needed; the script detects the OS and removes the
# appropriate files automatically.

set -euo pipefail

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

log()  { printf '\033[1;32m[uninstall]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[warn]\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31m[error]\033[0m %s\n' "$*" >&2; exit 1; }

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

# ---------------------------------------------------------------------------
# Platform: Linux — systemd user service
# ---------------------------------------------------------------------------

uninstall_linux() {
    local service_name="openpr-webhook"
    local service_file="$HOME/.config/systemd/user/${service_name}.service"

    if ! command -v systemctl &>/dev/null; then
        warn "systemctl not found; cannot manage the service automatically."
        warn "Remove $service_file manually if it exists."
        return
    fi

    # Stop the service if it is currently running.
    if systemctl --user is-active --quiet "$service_name" 2>/dev/null; then
        log "Stopping service: $service_name"
        systemctl --user stop "$service_name"
    else
        log "Service is not running (nothing to stop)"
    fi

    # Disable the service so it no longer starts on login.
    if systemctl --user is-enabled --quiet "$service_name" 2>/dev/null; then
        log "Disabling service: $service_name"
        systemctl --user disable "$service_name"
    else
        log "Service was not enabled (nothing to disable)"
    fi

    # Remove the unit file.
    if [[ -f "$service_file" ]]; then
        rm -f "$service_file"
        log "Removed: $service_file"
    else
        warn "Service file not found: $service_file (already removed?)"
    fi

    # Tell systemd to forget about the now-absent unit.
    systemctl --user daemon-reload
    log "systemd user daemon reloaded"

    cat <<EOF

  Uninstall complete (Linux / systemd).

  The binary and config.toml are NOT removed.
  Delete them manually if no longer needed.

EOF
}

# ---------------------------------------------------------------------------
# Platform: macOS — launchd user agent
# ---------------------------------------------------------------------------

uninstall_macos() {
    local label="dev.openpr.webhook"
    local plist_file="$HOME/Library/LaunchAgents/${label}.plist"

    # Unload the agent if it is currently loaded (stops it and removes it
    # from launchd's memory).
    if launchctl list 2>/dev/null | grep -q "$label"; then
        log "Unloading agent: $label"
        # Use the plist file path if it still exists; fall back to label.
        if [[ -f "$plist_file" ]]; then
            launchctl unload "$plist_file" 2>/dev/null \
                || { warn "launchctl unload failed; trying 'launchctl remove'"; \
                     launchctl remove "$label" 2>/dev/null || true; }
        else
            launchctl remove "$label" 2>/dev/null || true
        fi
        log "Agent unloaded"
    else
        log "Agent is not loaded (nothing to unload)"
    fi

    # Remove the plist file.
    if [[ -f "$plist_file" ]]; then
        rm -f "$plist_file"
        log "Removed: $plist_file"
    else
        warn "Plist file not found: $plist_file (already removed?)"
    fi

    # Note: log files are left in place so the user can review them.
    local log_out="$HOME/Library/Logs/openpr-webhook.log"
    local log_err="$HOME/Library/Logs/openpr-webhook.error.log"
    if [[ -f "$log_out" ]] || [[ -f "$log_err" ]]; then
        warn "Log files were NOT removed. Delete manually if desired:"
        [[ -f "$log_out" ]] && warn "  $log_out"
        [[ -f "$log_err" ]] && warn "  $log_err"
    fi

    cat <<EOF

  Uninstall complete (macOS / launchd).

  The binary and config.toml are NOT removed.
  Delete them manually if no longer needed.

EOF
}

# ---------------------------------------------------------------------------
# Dispatch
# ---------------------------------------------------------------------------

case "$PLATFORM" in
    linux) uninstall_linux ;;
    macos) uninstall_macos ;;
esac
