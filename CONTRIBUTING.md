# Contributing to Sylvode

Thank you for helping improve Sylvode (formerly OpenPR). This guide describes how the
repository is actually built, tested and committed, so that a change which passes here also
passes CI.

Security problems are not reported through issues or pull requests; see
[SECURITY.md](SECURITY.md).

## Where to talk

- Bugs and feature requests: [GitHub Issues](https://github.com/openprx/sylvode/issues).
- Questions and general discussion: [community.openprx.dev](https://community.openprx.dev).

Be respectful and constructive. Keep issues, pull requests, commit messages and code comments in
English.

## Repository layout

| Path | Contents |
| --- | --- |
| `apps/api` | HTTP API, universal forms, Sylvode Flow, plugin runtime, event emission |
| `apps/worker` | Background pipelines (form jobs, Flow event dispatch, AI tasks, retention) |
| `apps/mcp-server` | MCP server (`mcp-server`), the `sylvode` CLI and `list-tools` |
| `crates/platform` | Shared configuration, database connection, auth, errors, logging, deprecation texts |
| `crates/collab-core` | Collaboration engine on Loro and the `collab-isolated-apply-worker` binary |
| `frontend` | SvelteKit web UI, built with Bun |
| `migrations` | Ordered SQL migrations, applied by the API at startup |
| `scripts` | Deployment, development and verification scripts |
| `docs` | Product, operations and compatibility documentation |
| `spikes` | Evaluation crates kept for reference; they are workspace members and must build |

## Getting started

```bash
git clone https://github.com/openprx/sylvode.git
cd sylvode
```

The Rust toolchain is pinned in `rust-toolchain.toml` (with `rustfmt` and `clippy`); `rustup`
installs it on the first `cargo` command. The frontend needs [Bun](https://bun.sh). Docker or
Podman with Compose is needed for `scripts/start.sh` and `scripts/dev-up.sh`.

### Full stack in containers

```bash
bash scripts/start.sh                  # generate configuration, build, docker compose up -d
bash scripts/start.sh --check-config   # only generate and validate the configuration
```

`scripts/start.sh` writes `config/sylvode.compose.toml`, `config/sylvode.compose.mcp.toml` and
`.env` with random non-production secrets on first run. These files are ignored by git; never
commit them.

### Running the services on the host

```bash
bash scripts/dev-up.sh                 # start only PostgreSQL; prints the database URL to use
cp config/sylvode.example.toml config/sylvode.toml
$EDITOR config/sylvode.toml            # [database] url, [auth] jwt_secret, [mcp] settings

cargo run --bin api -- --config config/sylvode.toml
cargo run --bin worker -- --config config/sylvode.toml
cargo run --bin mcp-server -- serve --config config/sylvode.toml --transport http
bun install --cwd frontend && bun run --cwd frontend dev
```

`scripts/dev-up.sh` starts only the compose PostgreSQL service through a
temporary localhost-only port override, and prints the `url` line for the `[database]` section of
`config/sylvode.toml` plus the `PGPASSWORD` value `scripts/init-db.sh` needs. Inspect the database with
`docker compose exec postgres psql -U openpr -d openpr` (the database and role keep their
original names).

The binaries read no environment variables; everything comes from the configuration file, and
`config/sylvode.example.toml` documents every key. Set log levels with `filter` under
`[logging]`, for example `filter = "api=debug,tower_http=debug"`. The API applies pending
migrations from `migrations/` at startup and records them in the `schema_migrations` ledger.

## Checks to run before you push

CI runs these on every push and pull request. Run them locally on a clean, committed tree:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo check --workspace --all-features
cargo machete                          # cargo install cargo-machete
cargo audit                            # cargo install cargo-audit
cargo deny check                       # cargo install cargo-deny
cargo build -p collab-core --bin collab-isolated-apply-worker
cargo test --workspace --no-fail-fast

bun install --cwd frontend
bun run --cwd frontend check
bun run --cwd frontend build
```

From inside `frontend/` the same checks are `bun run check && bun run build`.

Run the whole test suite, not a filtered subset: integration tests and binary targets are only
covered by `cargo test --workspace`. The collaboration tests start
`collab-isolated-apply-worker` as a separate process, which is why it is built first.

### Database-backed tests

Many tests need PostgreSQL. They read one variable, `OPENPR_TEST_DATABASE_URL` (the name
predates the rename and is kept on purpose):

- It is a **maintenance connection**: a PostgreSQL 13 or newer role that is a superuser, or at
  least has `CREATEDB` and may create the `pgcrypto` extension.
- Its path must be a bare database name, such as `/postgres`. Tests replace it with the name of
  a scratch database that they create, migrate and drop themselves; the database you point at
  only serves as the connection used to create and drop them.

```bash
docker run -d --name sylvode-test-pg -e POSTGRES_USER=sylvode_test \
  -e POSTGRES_PASSWORD=sylvode_test -p 127.0.0.1:55432:5432 postgres:16
export OPENPR_TEST_DATABASE_URL=postgres://sylvode_test:sylvode_test@127.0.0.1:55432/postgres
cargo test --workspace --no-fail-fast
```

**When the variable is unset, the database-backed tests print `skipped: ...` and return, and
cargo still counts them as passed.** A green run without `OPENPR_TEST_DATABASE_URL` therefore
proves little about anything that touches the database. CI always sets it.

`bash scripts/ci-universal-forms-gates.sh` reproduces the CI "Universal Forms Gates" job: static
audits of the source tree followed by the forms regression tests. It requires
`OPENPR_TEST_DATABASE_URL`.

### Brand residue gate

`scripts/verify-sylvode-brand-residue.sh --json` scans the tracked files of the Sylvode checkouts
for `OpenPR` (case-sensitive, not part of `OpenPRX`) and lowercase `openpr` (not part of
`openprx`). Every hit must be covered by an entry of
`scripts/contracts/sylvode-brand-allowlist.json`: a reason code from a closed set
(`stable_identifier`, `legacy_alias_documented`, `formerly_note`, `migration_page`,
`redirect_rule`, `historical_document`, `frozen_evidence`, `legacy_behaviour_test`,
`kept_repository_url`), one repository, path globs, a pattern and an explanation. Anything not
covered fails with file:line. The script refuses an entry that waives the bare name across a
whole repository, reports entries that cover nothing (a failure under `--strict`), and treats a
requested repository that is missing, empty or not a git checkout as a failure.

CI runs the single-repository form with `--strict` plus `--self-test`, the built-in mutation
controls. Before a release, run the five-repository form locally from sibling checkouts:

```bash
bash scripts/verify-sylvode-brand-residue.sh --json --strict --release \
  --repo openpr-webhook=../openpr-webhook --repo docs=../docs \
  --repo site=../openprx-site --repo .github=../openprx-github
```

When a new hit is legitimate (a kept identifier, a legacy alias that is documented, a
migration note), add or extend the narrowest entry that covers it. When it is leftover product
naming, rename it instead.

### Where scripts read and write

Nothing under `scripts/` assumes a particular machine. Generated output defaults to the ignored
`.flow-gate/` directory of the checkout, and inputs that live outside the repository must be
named explicitly; a script that needs one and does not get it stops with an error instead of
treating the missing input as a pass.

| Variable | Used for | Default |
|---|---|---|
| `SYLVODE_CONTRACTS_ROOT` | The Sylvode Flow contracts checkout (`gates/`, `contracts/`) read by the Flow gate scripts; `--contracts-root` overrides it | none, required |
| `SYLVODE_SCRATCH_ROOT` | Scratch trees, mutation copies and separate cargo target directories | `.flow-gate/cache` |
| `SYLVODE_UF_REPORT_ROOT` | Universal Forms acceptance reports (`docs/`) and screenshots (`artifacts/`) | `.flow-gate/universal-forms` |
| `SYLVODE_FLOW_PRIOR_EVIDENCE_ROOT` | Earlier release receipts read by `verify-flow-prior-receipts-v1.0.sh` | `.flow-gate/evidence` |
| `SYLVODE_WEBHOOK_DIR` | The Sylvode Webhook checkout used by the webhook smokes | none, required |
| `SYLVODE_FLOW_V05_CONTRACT_GAPS` | The v0.5 contract-gap register read by `verify-flow-authz-v0.5.sh`, or `none` | none, required |

Flow gate scripts write their evidence to `.flow-gate/evidence/<release>` unless `--evidence-root`
names another directory. The screenshot collectors and render smokes refuse to write into a
non-empty directory unless `--overwrite` is given. `bash scripts/test-no-machine-paths.sh` runs in
CI and fails when a tracked file under `scripts/` names a machine-specific absolute path.

## Code rules

The workspace lints in `Cargo.toml` and the settings in `clippy.toml` enforce these rules;
`clippy -D warnings` turns every warning into an error.

- **No panics in production code.** `unwrap()`, `expect()`, `panic!`, `todo!`,
  `unimplemented!`, `unreachable!` and unchecked indexing or slicing are denied. Return a typed
  error and propagate it with `?`. Tests may use `unwrap`, `expect`, `panic!` and `dbg!`.
- **No `unsafe`.** `unsafe_code` is denied; any exception needs a `// SAFETY:` comment, which
  `undocumented_unsafe_blocks` requires.
- **No printing from library code.** `println!`, `eprintln!` and `dbg!` are denied; use
  `tracing`, or write to an explicit handle and handle the write error. Never log tokens,
  passwords, signed URLs or authorization headers.
- **No `std::sync::Mutex` or `RwLock`.** Use `parking_lot` for synchronous locks and `tokio::sync`
  in async code.
- **No dead code and no warnings.** Clippy's `pedantic` and `nursery` groups are on as warnings,
  and every result marked `#[must_use]` must be used.
- **Parameterized SQL only.** Never build SQL by string concatenation.
- Formatting follows `rustfmt.toml` (edition 2024, 120 columns).

## Compatibility

Sylvode keeps its OpenPR-era identifiers stable. Do not rename anything listed in
[docs/sylvode-v1.0-compatibility.md](docs/sylvode-v1.0-compatibility.md): the REST paths and
schema identifiers, MCP tool names, database objects and migrations, the plugin ABI, the `opr_`
token prefix, compose service labels and `OPENPR_TEST_DATABASE_URL`. Legacy entry points stay
working with a deprecation warning and are not removed before Sylvode v2.0.

Migrations are append-only. Add a new `migrations/NNNN_description.sql` with the next number;
never edit one that has been released.

## Commits

- One feature or fix per commit, with a [Conventional Commits](https://www.conventionalcommits.org/)
  subject in English: `feat(scope): ...`, `fix(scope): ...`, `docs: ...`, `test(scope): ...`,
  `refactor(scope): ...`. The body explains what changed and why.
- Every commit bumps the patch version, and the version must stay identical in the
  `[workspace.package]` section of `Cargo.toml`, the workspace crates in `Cargo.lock` (`api`,
  `collab-core`, `mcp-server`, `platform`, `worker`) and `frontend/package.json`.
  `scripts/bump-version.sh patch` updates `Cargo.toml` and `frontend/package.json`; run
  `cargo check --workspace` afterwards to update `Cargo.lock`.
- Add user-visible changes to the `[Unreleased]` section of [CHANGELOG.md](CHANGELOG.md), and call
  out anything that changes existing behaviour.
- Add or update tests with every behaviour change, and update the documentation in `README.md`
  or `docs/` in the same commit.

## Pull requests

Open pull requests against `main`. Describe what changed, why, how you tested it (including
whether `OPENPR_TEST_DATABASE_URL` was set), and any compatibility or migration impact.

## License

Sylvode is dual-licensed under the [MIT License](LICENSE-MIT) or the
[Apache License, Version 2.0](LICENSE-APACHE), at your option. Unless you explicitly state
otherwise, any contribution you intentionally submit for inclusion in this project, as defined in
the Apache-2.0 license, is dual-licensed as above, without any additional terms or conditions.
