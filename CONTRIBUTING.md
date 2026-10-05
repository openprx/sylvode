# Contributing to Sylvode Webhook

Thank you for your interest in contributing. Sylvode Webhook (formerly OpenPR Webhook) is the
webhook receiver and agent dispatcher for [Sylvode](https://github.com/openprx/sylvode).

## Code of Conduct

Be respectful, inclusive, and constructive.

## Getting Started

```bash
git clone https://github.com/openprx/openpr-webhook.git
cd openpr-webhook
cp config.example.toml config.toml
cargo check
```

The crate builds one library and two executables: `sylvode-webhook` and the legacy
`openpr-webhook`. Both are thin wrappers around `sylvode_webhook::run`; put behaviour in the
library, never in a `src/bin/` file.

## Checks

Every change must pass, on a clean committed tree:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo check --all-features
cargo machete
cargo audit
cargo deny check
cargo test --no-fail-fast
```

`cargo test` is the floor, not the finish line: start the service and exercise the changed
behaviour over HTTP before calling a change done.

## Code Standards

The production code rules in [`CLAUDE.md`](CLAUDE.md) apply to every contributor:

- No `unwrap`, `expect`, `panic!`, `todo!` or `unimplemented!` outside tests.
- Zero warnings; no dead code.
- Explicit error handling with `Result` / `Option`; validate external input at the boundary.
- `parking_lot` or `tokio` mutexes, never `std::sync::Mutex`.
- Every `unsafe` block carries a `// SAFETY:` comment.

## Legacy names

Legacy `openpr-webhook` names (executable, image, `X-OpenPR-Signature`, `OPENPR_WEBHOOK_SAFE_MODE`,
the `openpr_webhook` log target, service units) keep working for the whole 1.x line and must not be
removed before v2.0. A change that touches one of them needs tests in both directions: the legacy
form works and writes its deprecation notice, and the canonical form works and writes none.

## Commits and Pull Requests

- English, conventional subjects: `feat: ...`, `fix: ...`, `refactor: ...`, `docs: ...`, `ci: ...`.
- One logical change per commit; the body describes the change only.
- Bump the patch version in `Cargo.toml` with every feature commit and add a `CHANGELOG.md` entry.

## License

By contributing you agree that your contributions are licensed under either of
[Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at the user's option.
