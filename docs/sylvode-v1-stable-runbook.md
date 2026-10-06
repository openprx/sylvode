# Sylvode Flow v1.0 stable operations and rollback runbook

This runbook is the executable checklist for a staged Flow rollout. The named people are recorded only by the external `release_owner`, `rollback_owner`, `on_call_runbook`, and `stable_contract_approval` sign-off rows; this document does not self-assign or self-sign them.

## Release preparation and staged rollout

1. Freeze the source commit, migration version, release binary SHA-256 values, dashboard snapshot, and database restore point.
2. Rehearse migration, backup restore, and rollback against a production copy. Compare document fingerprints, head sequence/frontier, projections, relations, lineage, and audit events.
3. Deploy API, worker, and MCP before Web. Confirm protocol capability negotiation and the old-client window.
4. Enable the Flow feature flag for one internal workspace, then expand by workspace. Never enable all workspaces in one step.
5. Stop rollout on any data-integrity alarm, error-budget exhaustion, unexplained rejected-update increase, or projection/search lag budget breach.

## Sync degraded or hot document

Check accepted/rejected updates, coordinator timeout rate, connections, round-trip p50/p95/p99, row-lock wait/hold, WAL commit time, queue depth, and slow-consumer disconnects. Quiesce expansion of the affected workspace. If lock hold is within budget but latency rises with client count, treat it as coordinator throughput/queueing; do not raise the frozen latency budget. Capture all PostgreSQL collector files before rotation. Drain or isolate the hot workspace and preserve rejected update IDs for replay.

## Projection or search lag

Pause rollout and record accepted head sequence/frontier against projection and search frontiers. Stop the projection worker only after its lease is visible, restart it, then run the authorized projection rebuild. Verify frontier equality and sampled semantic hashes before resuming. Never repair canonical document state from a projection.

## Corruption quarantine and forced resync

On checksum, invalid-update, or non-contiguous-tail evidence, stop writes for the document and quarantine its snapshot and update tail without rewriting either. Preserve the audit correlation ID. Restore from the last verified snapshot plus contiguous updates. Force clients to resync only after checksum, head sequence/frontier, semantic hash, and projection equality all pass; never acknowledge the corrupt update.

## Backup restore

Record the source database identifier and backup SHA-256, restore into a fresh database name, run migrations, and compare document fingerprints and object/package exports. Verify retained update tails, relations, lineage, event dispatch, and authorization epochs. Keep the source untouched until the external rollback owner accepts the comparison.

## Token or permission incident

Revoke the credential, bump the authorization epoch, close affected sessions, and remove presence entries. Confirm REST, WebSocket, MCP, CLI, search, export, relation, and ticket reads reauthorize after the final epoch check. Preserve redacted audit evidence; never place token material in logs.

## Rollback procedure

The externally signed rollback owner decides rollback. Stop feature-flag expansion, quiesce new Flow writes, drain API/worker/MCP, preserve the failed release receipt, and restore the recorded rollback point. Start the previous binaries with their supported schema, verify old-client REST/MCP access and exact data fingerprints, then reopen traffic by workspace. Forward-only migrations are not deleted; if the previous binary cannot read the migrated schema, keep traffic stopped and restore the pre-migration database backup.

## Exit and evidence

Resume only when integrity checks pass, the triggering metric is within its approved budget, queues drain, and an external on-call reviewer records the incident evidence. Attach command exits, exact test counts, ignored tests, artifact hashes, remaining failures, and rollback decision to the release receipt.

## Re-running the v1.0 gate

The commands the v1.0 gate contract names (`scripts/gate-flow-v1.0.sh --json`,
`scripts/report-flow-v1.0-json.sh`, `scripts/verify-flow-v1.0-json.sh` and the producers they
run) read these inputs from outside the repository. Each one that is missing stops the run with an
error rather than passing:

| Input | How to pass it | Default |
| --- | --- | --- |
| Sylvode Flow contracts checkout | `SYLVODE_CONTRACTS_ROOT` or `--contracts-root DIR` | none, required |
| Earlier release receipts (v0.3-v0.9) | `SYLVODE_FLOW_PRIOR_EVIDENCE_ROOT` or `--prior-evidence-root DIR` of `scripts/verify-flow-prior-receipts-v1.0.sh` | `.flow-gate/evidence` |
| Accepted v0.9 gate receipt | `--predecessor-gate-result PATH` of `scripts/report-flow-v1.0-json.sh` | `<evidence root>/../v0.9/gate-result.json` |
| Capacity measurement result | `OPENPR_V10_CAPACITY_RESULT` or `--capacity-result PATH` of `scripts/verify-flow-slo-v1.0.sh` | none, required |
| Orchestration config (`[flow_gate] test_database_url`, `backup_source_database_url`, `backup_restore_admin_url`) | `OPENPR_FLOW_V1_ORCHESTRATION_CONFIG` or `--orchestration-config PATH` of `scripts/report-flow-v1.0-json.sh` | `<SYLVODE_SCRATCH_ROOT>/v10-flow-gate.toml` (`.flow-gate/cache/v10-flow-gate.toml`) |
| Universal Forms sign-off reports | `SYLVODE_UF_REPORT_ROOT` | `.flow-gate/universal-forms` |
| Scratch trees and separate cargo targets | `SYLVODE_SCRATCH_ROOT` | `.flow-gate/cache` |
| Sibling checkouts for the brand residue release gate | `SYLVODE_SIBLINGS_ROOT` or `--siblings-root DIR` of `scripts/report-flow-v1.0-json.sh`: the directory holding `openpr-webhook`, `docs`, `openprx-site` and `openprx-github` | none, required unless all four are given one by one |
| One sibling checkout at another path | `SYLVODE_BRAND_RESIDUE_REPOS="NAME=PATH,..."` or `--brand-repo NAME=PATH` (repeatable) of `scripts/report-flow-v1.0-json.sh`, with `NAME` one of `openpr-webhook`, `docs`, `site`, `.github`; it wins over `--siblings-root` | none |

`brand_residue_release_gate` runs `scripts/verify-sylvode-brand-residue.sh --release --strict`
over this checkout and the four siblings. A sibling that is missing, empty, not a git checkout or
has uncommitted changes turns the gate red; it is never skipped. `openpr_compat_deprecation_verified`
runs `scripts/verify-sylvode-brand-compat.sh --release 1.0`, which needs no input beyond the
toolchain (cargo, bun) and builds its attribution-header mutation in a detached worktree under
`SYLVODE_SCRATCH_ROOT`.

Evidence is written to `.flow-gate/evidence/v1.0` unless `--evidence-root` names another
directory. The scripts need bash 4.3 or newer and GNU coreutils (`sha256sum`, `stat -c`); on macOS
install them (for example with Homebrew) and run the scripts with that bash.
