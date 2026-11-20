#!/usr/bin/env bash
# Stand-in for the two regression scripts of case A20, used by the dry run only
# (OAUTH_E2E_A20_OVERRIDE). It proves how the case runs and judges a script; it proves nothing
# about the real scripts. The first argument is "mcp" or "ui". OAUTH_E2E_A20_STUB_EXIT overrides
# the exit status.
echo "stub regression script: $1"
exit "${OAUTH_E2E_A20_STUB_EXIT:-0}"
