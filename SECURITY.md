# Security Policy

## Supported versions

Security fixes are made on the latest release of Sylvode Webhook (formerly OpenPR Webhook). The
legacy `openpr-webhook` executable, image and archive names receive the same fixes, because they
are the same build.

## Reporting a vulnerability

Please do not open a public issue for a vulnerability. Report it privately through
[GitHub security advisories](https://github.com/openprx/openpr-webhook/security/advisories/new).

Include the affected version, a description of the problem, steps to reproduce and the impact you
expect. We will acknowledge the report, keep you informed while we investigate, and credit you in
the advisory unless you prefer otherwise.

## Scope notes

- Inbound webhooks are authenticated by an HMAC-SHA256 signature in `X-Sylvode-Signature`,
  `X-Webhook-Signature` or the legacy `X-OpenPR-Signature`. Requests whose signature headers
  disagree are refused. `allow_unsigned = true` disables verification and is meant for
  development only.
- The `cli` and `custom` agent types run local commands. Keep `features.cli_enabled` off unless you
  need it, and use `SYLVODE_WEBHOOK_SAFE_MODE=1` to force the tunnel, CLI and callback paths off.
- Never commit a `config.toml` containing secrets; `.gitignore` excludes it.
