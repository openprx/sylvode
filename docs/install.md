# Installing Sylvode

A Sylvode deployment consists of these parts:

| Part | What it is |
|---|---|
| `api` | The HTTP API. Applies database migrations at startup. Spawns `collab-isolated-apply-worker` for every Flow collaborative write. |
| `collab-isolated-apply-worker` | Helper executable of the API. Must sit in the same directory as `api`. |
| `worker` | Background jobs: bot task delivery, form jobs, governance settlement, Flow event delivery and maintenance, retention. |
| `mcp-server` | The MCP server for AI assistants. Optional. |
| `sylvode` | The command line for workspace and Flow commands. Optional; see [integrations/cli.md](integrations/cli.md). |
| Web UI | Static files built from `frontend/`, served by nginx, which also forwards `/api` to the API. |
| PostgreSQL 18 | The only database. |
| `sylvode-runner` | Optional. Runs AI agents for bots; see [apps/runner/README.md](../apps/runner/README.md). |

What each process does at runtime is described in [operations.md](operations.md); every
configuration key in [configuration.md](configuration.md).

## Requirements

- PostgreSQL 18. The Compose stack runs the `postgres:18` image.
- A Linux server for Flow. On every other platform the API serves projects, forms, MCP and Flow
  reads, but refuses every Flow collaborative write and object diff (error `server_rejected`,
  reason `isolated_apply_unsupported_platform`) and logs a warning at startup. The macOS and
  Windows release archives are therefore suitable for evaluation without Flow editing only.
- The `worker` running next to the API. Without it, Page creation and editing in Flow are refused
  (see [operations.md](operations.md#flow-reader-leases)) and no background job runs.

## Choosing a method

| Method | Use it when |
|---|---|
| [Docker Compose](#docker-compose) | You want the whole stack on one host with generated configuration. The script builds the server binaries on the host, so it needs a Rust toolchain unless you supply prebuilt binaries. |
| [Release archives](#release-archives) | You run the binaries under your own process supervisor and your own PostgreSQL and web server. |
| [Building from source](#building-from-source) | You build the binaries and the web UI yourself. |

## Docker Compose

### What you need

- Docker with the Compose plugin, or Podman with a Compose provider.
- `python3` 3.11 or newer and `curl` (used by `scripts/start.sh`).
- A Rust toolchain (see [Building from source](#building-from-source)), unless you run the script
  with `--no-build` and place prebuilt Linux binaries in `target/release`.

### Start the stack

```bash
git clone https://github.com/openprx/sylvode.git
cd sylvode
bash scripts/start.sh
```

`scripts/start.sh` does the following:

1. Creates `.env` from `.env.example` when it is missing (mode 600) and generates
   `POSTGRES_PASSWORD` when it is unset.
2. Generates `config/sylvode.compose.toml` (read by `api` and `worker`) and
   `config/sylvode.compose.mcp.toml` (read by `mcp-server`) when they are missing. Existing files
   are never rewritten. The first holds a generated `auth.jwt_secret` and the database URL; the
   second holds only `[logging]` and `[mcp]`, with a placeholder bot token and workspace id.
3. Validates the three files and stops with a list of problems if any value is unusable.
4. Builds the release binaries with `cargo build --workspace --release`.
5. Builds the images and starts the services, then waits until the API, the MCP server and the web
   UI answer.

Both generated configuration files contain secrets. They are created with mode 644 so the
container user (uid 1000) can read the bind mount; restrict access to the `config/` directory
instead. The script warns when the files or `./uploads` are not owned by uid 1000, because the
containers then cannot read the configuration or write uploads.

| Option | Effect |
|---|---|
| (none) | Build the binaries and start the stack. |
| `--check-config` | Generate missing files, validate them, and exit without touching Docker. |
| `--pull` | Pull the latest base images before building. |
| `--no-build` | Use the binaries already in `target/release` (`api`, `collab-isolated-apply-worker`, `worker`, `mcp-server`). They must be built against a glibc no newer than the runtime image's. |

### Services and ports

| Service | Published at | Notes |
|---|---|---|
| `frontend` | `127.0.0.1:3000` | The web UI. Its nginx forwards `/api` and `/uploads` to the API, so browsers need only this port. |
| `api` | `127.0.0.1:8081` | Listens on 8080 inside the network (`server.bind_addr = "0.0.0.0:8080"` in the generated file). |
| `mcp-server` | `127.0.0.1:8090` | Serves the `http` transport. Every request needs its caller's workspace bot token; only `/health` answers without one. |
| `worker` | not published | Opens no listener. |
| `postgres` | not published | Reachable only inside the Compose network. |
| `runner` | `127.0.0.1:9090` | Only with the `connectors` profile; see below. |

Change the bind host and ports with the variables in `.env` (`SYLVODE_BIND_HOST`,
`SYLVODE_API_PORT`, `SYLVODE_FRONTEND_PORT`, `SYLVODE_MCP_PORT`, `SYLVODE_RUNNER_PORT`); see
[configuration.md](configuration.md#docker-compose-variables). Keep the bind host at `127.0.0.1`
and put a reverse proxy that terminates TLS in front of the web UI; see
[operations.md](operations.md#reverse-proxy).

### Live editing origins

The generated file lists the addresses the stack's web UI is published at in
`flow.collab_allowed_origins`. Browsers send the origin exactly as typed, and live editing sessions
from any other origin are refused. Once users reach the web UI through a domain or a reverse proxy,
replace the list with that origin, for example:

```toml
[flow]
collab_allowed_origins = ["https://sylvode.example.com"]
```

Then restart the API: `docker compose restart api`.

### Serving the web UI over plain HTTP

HTTPS is recommended for every deployment that is reached from another machine. If you serve the web
UI over plain `http://` on an address other than `localhost` (for example `http://10.0.0.5:3000`
on a trusted LAN), browsers treat the page as an insecure context and hide a few features that
exist only on HTTPS and on `localhost`. What that means:

What works:

- Sign-in and all pages, only if the authentication cookies are allowed without `Secure`: browsers
  do not store a `Secure` cookie that arrives over plain HTTP outside `localhost`, so set
  `auth.allow_insecure_cookies = true`. The API accepts that only while `server.bind_addr` is a
  loopback host, so the plain HTTP address has to be a proxy or port forward on the same machine
  in front of the API; see [configuration.md](configuration.md).
- Flow live editing. The browser builds its identifiers from `crypto.getRandomValues`, which exists
  in every context. List the origin exactly as typed, `http://` and port included, in
  `flow.collab_allowed_origins`, or live editing is refused.
- Copy buttons. The browser's Clipboard API is missing there, so the page copies through a hidden
  text field. If the browser still refuses, the page says so and asks you to select the text and
  copy it yourself.
- Integrity checks that hash content in the browser (workspace package downloads and plugin panels).
  Web Crypto is missing there, so the same SHA-256 runs in script. The result is identical.

What is limited:

- Nothing protects the traffic. Passwords, access tokens, session cookies and every document cross
  the network in clear text, and anyone who can alter the traffic can also alter the pages and the
  checksums the integrity checks compare against. Use plain HTTP only on a network you trust.
- Plugin panels run in sandboxed frames that inherit the insecure context, so a plugin that uses a
  browser feature limited to HTTPS (for example `crypto.randomUUID` or Web Crypto) fails there.
  The plugin templates of the plugin creator do not need either.
- If the live editing connection cannot be established, the page shows "Live editing could not
  start" with a Try again button. On a plain HTTP deployment the usual causes are an origin missing
  from `flow.collab_allowed_origins` and a proxy that does not forward WebSocket upgrades.

### The MCP server in Compose

The generated MCP file carries a random placeholder `workspace_id` and `bot_token` so the container
starts. Replace `workspace_id` with the id of the workspace MCP clients work in, then recreate the
service with `docker compose up -d mcp-server`. Clients authenticate with their own workspace bot
tokens; see [apps/mcp-server/README.md](../apps/mcp-server/README.md). The demo script below fills
in both values for the demo workspace.

### The optional runner

The runner service belongs to the `connectors` profile and is not started by default:

```bash
docker compose --profile connectors up -d runner
```

The service runs with `init: true` and `stop_grace_period: 45s`, as the runner's container
guidance asks. By default it mounts `config/sylvode-runner.example.toml`. Copy it to a file of your own, set real
secrets, and point `SYLVODE_RUNNER_CONFIG` at it. When no image named `SYLVODE_RUNNER_IMAGE` is
present, Compose builds it from `apps/runner`; `docker compose pull runner` fetches the published
one. Configuration and operation are described in
[apps/runner/README.md](../apps/runner/README.md).

### Useful commands

| Task | Command |
|---|---|
| Follow the logs | `docker compose logs -f` |
| Stop the stack | `docker compose down` |
| Restart services | `docker compose restart` |
| Check the configuration | `bash scripts/start.sh --check-config` |

## Release archives

Each release publishes these archives, each with a `.sha256` file:

| File | Contents |
|---|---|
| `sylvode-linux-amd64.tar.gz`, `sylvode-linux-arm64.tar.gz` | `api`, `collab-isolated-apply-worker`, `worker`, `mcp-server`, `sylvode` |
| `sylvode-macos-amd64.tar.gz`, `sylvode-macos-arm64.tar.gz` | the same executables for macOS (no Flow editing, see [Requirements](#requirements)) |
| `sylvode-windows-amd64.zip` | the same executables with `.exe` (no Flow editing) |
| `sylvode-frontend.tar.gz` | `build/` (the web UI) and `nginx.conf` |
| `sylvode-runner-<platform>.tar.gz` | `sylvode-runner`, for Linux and macOS on amd64 and arm64 |

The runner is also published as the container image `ghcr.io/openprx/sylvode-runner:<version>`
(the release version without the `v`, and `latest`) for linux/amd64 and linux/arm64.

### Install the server binaries

```bash
sha256sum -c sylvode-linux-amd64.tar.gz.sha256
sudo mkdir -p /srv/sylvode/bin /srv/sylvode/config /srv/sylvode/uploads
sudo tar -xzf sylvode-linux-amd64.tar.gz -C /srv/sylvode/bin
```

Keep `collab-isolated-apply-worker` in the same directory as `api`; the API looks for it there.
To place it elsewhere, set `COLLAB_ISOLATED_APPLY_WORKER_PATH` for the `api` process.

### Prepare the database and the configuration

Create a PostgreSQL 18 database and role for Sylvode. The API creates the schema itself on its
first start.

Copy [config/sylvode.example.toml](../config/sylvode.example.toml) to
`/srv/sylvode/config/sylvode.toml` and set at least:

```toml
[server]
bind_addr = "127.0.0.1:8081"

[database]
url = "postgres://sylvode:change-this-password@localhost:5432/sylvode"

[auth]
jwt_secret = "replace_with_output_of_openssl_rand_hex_32"

[storage]
backend = "local"
dir = "/srv/sylvode/uploads"

[flow]
collab_allowed_origins = ["https://sylvode.example.com"]
```

Every `replace_with_*` value of the example is refused on purpose, so the file does not start
until each one is filled in or removed. Make the file readable only by the account that runs
Sylvode, since it holds the database password and the signing key. The MCP server can use the same
file or a separate one that carries only `[logging]` and `[mcp]`.

### Run the processes

Run each process under your process supervisor, from a working directory of your choice:

```bash
/srv/sylvode/bin/api --config /srv/sylvode/config/sylvode.toml
/srv/sylvode/bin/worker --config /srv/sylvode/config/sylvode.toml
/srv/sylvode/bin/mcp-server serve --config /srv/sylvode/config/sylvode.toml --transport http --bind-addr 127.0.0.1:8090
```

Start the API first on a new or upgraded database: it applies the migrations, the worker does not.
The MCP server needs `mcp.workspace_id` in the file; the flags select the `http` transport, which
`mcp.transport` can also set. The worker accepts `--concurrency <n>` (default 4), the batch size multiplier of its jobs.
`api --build-info` prints the build provenance as JSON and exits.

Relative paths in the file, such as the default `storage.dir = "./uploads"`, resolve against the
working directory, so the API and the worker must share one working directory or use absolute
paths.

### Serve the web UI

```bash
sha256sum -c sylvode-frontend.tar.gz.sha256
tar -xzf sylvode-frontend.tar.gz
```

`build/` is a static single-page application. `nginx.conf` is the configuration the web UI image
uses: it serves the files from `/usr/share/nginx/html`, forwards `/api` (including WebSocket
upgrades), `/uploads`, the OAuth discovery documents and the event stream to the API, sets the
security headers and redacts credentials from the access log. It is written for the Compose
network:

- It proxies to `api:8080`. Change `set $api_upstream api:8080;` to the address of your API.
- It includes `/etc/nginx/conf.d/resolver.conf`, which the container writes at startup with a
  `resolver` line. Create that file with a `resolver` line for your host, or remove the `include`
  when the upstream is an IP address.
- It listens on port 80 without TLS. Terminate TLS in front of it or add it to the server block.

See [frontend/README.md](../frontend/README.md) and [operations.md](operations.md#reverse-proxy).

### Install the runner

Unpack `sylvode-runner-<platform>.tar.gz` or use the container image, then follow
[apps/runner/README.md](../apps/runner/README.md).

## Building from source

### Server binaries

The Rust toolchain version is pinned in `rust-toolchain.toml`; `rustup` installs it on first use.
On Debian or Ubuntu the build also needs `pkg-config` and `libssl-dev`.

```bash
cargo build --workspace --release
```

The executables are written to `target/release`: `api`, `collab-isolated-apply-worker`, `worker`,
`mcp-server`, `sylvode` and `sylvode-runner`. The database migrations are compiled into `api`, so
no other file needs to be shipped with it.

The Compose images built from `Dockerfile.prebuilt` copy these binaries into a Debian image that
installs `ca-certificates`, `libpq5` and `curl`.

### Web UI

See [frontend/README.md](../frontend/README.md). In short:

```bash
cd frontend
bun install --frozen-lockfile
bun run build
```

The result is in `frontend/build/`.

## The first administrator account

The web UI has a login page but no sign-up page. On an empty database, the first account
registered through the API becomes the instance administrator:

```bash
curl -X POST http://localhost:8081/api/v1/auth/register \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@example.com","password":"choose-a-long-password","name":"Admin"}'
```

The password must be at least 8 characters. Registrations are serialised, so two simultaneous
first registrations cannot both become administrators. Once an account exists, the same endpoint
refuses callers that are not signed in as an administrator; an administrator creates further
accounts in the web UI on the instance administration's **Users** page (`/admin/users`) with
**Create User**, or with an administrator token on the same endpoint.

## Demo data

`scripts/bootstrap-restaurant-demo.sh` creates a demo through the public API: a user, a workspace,
a restaurant ordering project and sample form records. It needs `node` and `python3`.

```bash
bash scripts/bootstrap-restaurant-demo.sh
```

On an empty database the demo user is the first account and therefore becomes the instance
administrator. If accounts already exist, set `SYLVODE_DEMO_EMAIL` and `SYLVODE_DEMO_PASSWORD` to
an existing account. When `config/sylvode.compose.mcp.toml` exists, the script writes the demo
bot's token and workspace id into it and recreates the `mcp-server` service. It refuses an API
URL other than localhost unless `SYLVODE_DEMO_ALLOW_REMOTE=1` is set. `--help` lists every
variable. It is a local onboarding helper, not a way to seed production data.

## Upgrading

1. Read the release's section in [CHANGELOG.md](../CHANGELOG.md).
2. Back up the database (see [operations.md](operations.md#backup-and-restore)).
3. Replace the binaries, keeping `collab-isolated-apply-worker` beside `api`, and replace the web UI
   files.
4. Restart the API first. It applies any new migrations at startup and refuses to start if one
   fails (unless `migrations.continue_on_error` is set). Then restart the worker and the MCP
   server.

With Compose, update the checkout and run `bash scripts/start.sh` again. It keeps the existing
`.env` and configuration files, rebuilds the binaries and images, and recreates the services.

A stopped API or worker keeps its Flow reader lease for up to 30 seconds, so an ordinary restart
does not interrupt Page editing; see [operations.md](operations.md#flow-reader-leases).
