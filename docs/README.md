# Sylvode documentation

Start with the [project README](../README.md) for an overview and a quick start. The documents
below are grouped by reader.

## Operators

- [install.md](install.md): requirements, the compose stack, release archives, building from
  source, the first administrator account, demo data and upgrades.
- [configuration.md](configuration.md): every configuration file key and every environment
  variable the programs read.
- [operations.md](operations.md): the processes and what each does, health checks, logging,
  reverse proxies, storage, backup and restore, Flow runtime capabilities and retention.
- [../apps/runner/README.md](../apps/runner/README.md): installing, configuring and operating
  `sylvode-runner`.
- [../apps/runner/SECURITY.md](../apps/runner/SECURITY.md): what the runner verifies and how it
  runs agents.
- [../frontend/README.md](../frontend/README.md): building and serving the web interface.
- Annotated configuration examples: [../config/sylvode.example.toml](../config/sylvode.example.toml),
  [../config/sylvode-runner.example.toml](../config/sylvode-runner.example.toml) and
  [../apps/runner/config.example.toml](../apps/runner/config.example.toml).

## Users

- [guide/workspaces-and-projects.md](guide/workspaces-and-projects.md): navigation, workspaces
  and members, projects, work items, boards, cycles, labels, workflows, notifications, search and
  project export/import.
- [guide/governance.md](guide/governance.md): proposals, voting, decisions, veto and escalation,
  appeals, trust scores, impact reviews and audit logs.
- [guide/forms.md](guide/forms.md): universal forms, fields, views, records, formulas,
  permissions, import/export, attachments and signatures.
- [guide/scenario-templates.md](guide/scenario-templates.md): the built-in project templates and
  how to start a project from one.
- [guide/flow.md](guide/flow.md): Flow pages, collections and records, live editing, sharing,
  conversion to forms and packages.
- [guide/agents.md](guide/agents.md): bots and tokens, how agent tasks arise and end, and
  connecting a bot to a project.

## Integrators

- [integrations/api.md](integrations/api.md): REST conventions, authentication, errors and
  endpoints by area.
- [integrations/webhooks.md](integrations/webhooks.md): outbound events, webhook signatures and
  deliveries.
- [integrations/alerts.md](integrations/alerts.md): sending alerts to Sylvode and the rules that
  turn them into work.
- [integrations/oauth.md](integrations/oauth.md): the OAuth authorization server, scopes, tokens
  and connected apps.
- [integrations/cli.md](integrations/cli.md): the `sylvode` command-line client.
- [../apps/mcp-server/README.md](../apps/mcp-server/README.md): the MCP server, its transports,
  authentication and tools.

### MCP skill package

<!-- BEGIN GENERATED: tool-count (sylvode-skillgen; do not edit) -->
The MCP server registers 158 tools in 20 categories. By default it lists nine of them (search mode) and reaches the rest through `sylvode_search`, `sylvode_read` and `sylvode_write`. Every tool, with its category and access, is in [reference/tools.md](../skills/sylvode-mcp/reference/tools.md).
<!-- END GENERATED: tool-count -->

- [../skills/sylvode-mcp/SKILL.md](../skills/sylvode-mcp/SKILL.md): the skill agents load to work
  with Sylvode over MCP.
- [../skills/sylvode-mcp/reference/tools.md](../skills/sylvode-mcp/reference/tools.md): every
  tool, one line each.

## Plugin authors

- [plugins/README.md](plugins/README.md): WASM plugins and form plugin packages.
- [plugins/openpr-plugin-v1.wit](plugins/openpr-plugin-v1.wit): the plugin interface.
- [plugins/extension-example/](plugins/extension-example/): example form plugin packages, one `ui` and
  one `wasm` package.

## Project

- [../SECURITY.md](../SECURITY.md): reporting a vulnerability.
- [../CHANGELOG.md](../CHANGELOG.md): release notes.
- Licence: [MIT](../LICENSE-MIT) or [Apache-2.0](../LICENSE-APACHE).
