# Security Policy

Sylvode (formerly OpenPR) holds project data, business records, bot credentials and
plugin code for every workspace on an instance. We take reports about it seriously and
handle them privately until a fix is available.

## Supported versions

Sylvode is pre-1.0. Security fixes land on `main` and ship in the next release of the
latest release line. Older releases do not receive backports; upgrade to the latest
release to get a fix.

| Version | Supported |
| --- | --- |
| Latest release (currently the 0.2.x line) | Yes |
| Any earlier release | No, upgrade to the latest release |

The legacy names that the 1.x series keeps for compatibility (the `mcp-server` CLI
subcommands, `config/openpr.toml`, `OPENPR_*` compose variables, `openpr://` resource URIs
and `openpr-<target>` release archives) are part of the same build and receive the same
fixes. See [the compatibility matrix](docs/sylvode-v1.0-compatibility.md).

## Reporting a vulnerability

Please do not open a public issue, pull request or discussion for a security problem.

Report it privately through GitHub security advisories:

<https://github.com/openprx/sylvode/security/advisories/new>

If you cannot use GitHub, the OpenPRX organization's security contact is
`security@openprx.dev`.

A useful report contains:

- the affected version (a release tag, or the `version` in `Cargo.toml` and the commit
  SHA of a source build) and how it is deployed (compose, prebuilt binaries, behind which proxy);
- the relevant configuration sections with every secret removed;
- a minimal proof of concept: the HTTP requests, MCP JSON-RPC messages, CLI invocation or
  plugin module that triggers the problem;
- what an attacker gains, and which role or credential they need to start with.

Reports in English or Chinese are both fine.

## What to expect

We will acknowledge the report, confirm or dispute the finding, and keep you informed
while we work on a fix. When a fix is released we publish a GitHub security advisory and
list the fix under `### Security` in [CHANGELOG.md](CHANGELOG.md). Reporters are credited
in the advisory unless they ask not to be.

Please give us a reasonable chance to release a fix before disclosing publicly, and
coordinate the disclosure date with us in the advisory thread. Test only against
deployments you own or are explicitly allowed to test, and do not access or modify data
that belongs to anyone else.

## Scope notes

These areas carry the highest impact, and reports about them are especially welcome.

- **Tenant isolation.** Workspaces are the isolation boundary. Any way for a user, bot
  token or plugin in one workspace to read or change another workspace's projects, records,
  Flow objects, attachments, events or audit entries is in scope, including through search,
  MCP resources, export packages and signed download URLs.
- **WASM plugin sandbox.** Any workspace member, and a bot token with write permission, can
  install a project plugin, so the sandbox is a security boundary. Plugins run in wasmtime
  with no imports at all (a module that imports anything fails to instantiate), a module size
  limit of 4 MiB, a fuel budget, a memory limit, one instance, memory and table (of at most
  65,536 elements), and a wall-clock deadline that covers compilation and interrupts the guest.
  The manifest can raise these limits only up to fixed maxima (30 s, 1,000,000,000 fuel,
  128 MiB). Escaping the sandbox, reaching host I/O, exceeding these limits, or crashing or
  stalling the API from a plugin is in scope.
- **MCP server and bot tokens.** Bot tokens (`opr_` prefix) are scoped to one workspace and
  carry `read`, `write` or `admin` permissions that the API enforces. Over the `http` and
  `sse` transports each request is made with the caller's own bearer token, which the MCP
  server forwards to the API; only `/health` is reachable without one. Acting without a
  valid token, acting beyond a token's permissions or workspace, the MCP project policy
  gate admitting a call it should refuse, or forging the `X-Sylvode-MCP-*` /
  `X-OpenPR-MCP-*` attribution recorded in the audit trail is in scope.
- **Webhooks and outbound requests.** Outbound webhook deliveries are signed with
  HMAC-SHA256 over the raw body in `X-Webhook-Signature: sha256=<hex>`. Delivery targets
  that resolve to loopback, private, link-local or other internal addresses are refused
  unless listed in `[outbound] allowed_hosts`, and redirects are not followed. A forged or
  unverifiable signature, a way past the outbound address filter (SSRF), or a secret
  leaking into logs or stored delivery records is in scope. The inbound side lives in the
  separate [Sylvode Webhook](https://github.com/openprx/openpr-webhook) repository and has
  its own policy.
- **Collaboration sessions.** Live editing uses short-lived tickets bound to an origin in
  `[flow] collab_allowed_origins`. Joining, reading or writing a document without the
  corresponding Flow permission, or keeping access after it is revoked, is in scope.

Generally out of scope: findings that require an already compromised host or database,
volumetric denial of service, documented insecure settings intended for local
development (such as `outbound.allow_private = true` or `auth.allow_insecure_cookies`
on a loopback listener) and vulnerabilities in dependencies with no reachable path in
Sylvode. For a dependency advisory, a normal public issue is fine; `cargo audit` and
`cargo deny` run in CI.
