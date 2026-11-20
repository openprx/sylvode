# Flow runtime capabilities

Flow features that are rolled out installation-wide are switched in the single row of
`flow_runtime_capabilities`. Since migration `0106_flow_capabilities_default_on.sql` the features
the shipped components implement are on by default: on a fresh database and, once, on an existing
database when it is upgraded. No manual SQL is needed for an ordinary user to open a personal Flow
space and write Pages in it.

| Column | Default | What it gates |
|---|---|---|
| `minimum_reader_version` | `5` | Process format floor. Rich Pages (editor model 5) need at least 4; block sharing, block anchors, block transfers and portable packages need 5. Never set it above what every running component registers (currently 5). |
| `personal_provisioning` | on | Lazily creating the signed-in user's own personal scope on first visit. |
| `personal_writes` | on | Any write in a personal scope. Requires `personal_provisioning`. |
| `external_mounts` | on | Creating mounts of personal content into a workspace and changing their level. Requires `personal_writes`. |
| `personal_forms_conversion` | on | Converting personal Flow content into a Forms workspace. |
| `block_transfers_enabled` | on | Moving or copying blocks between Pages (needs `minimum_reader_version` exactly 5). |
| `portable_page_packages_enabled` | on | Portable single-Page package export and import. |
| `portable_graph_packages_enabled` | on | Portable multi-object package export and import. |
| `scope_native_legacy` | off | Scope-native editor initialization of legacy workspace Pages. |
| `personal_package_imports` | off | Importing archive packages into a personal scope. |
| `personal_scope_package_exports` | off | Exporting a whole personal scope as a package. |
| `cross_workspace_forms_conversion` | off | Converting Flow content into a different workspace's Forms. |

A capability only allows a feature; every request is still authorized per user (personal scopes
are reachable only by their owner and explicit grantees).

## Reader leases

The API registers an `api` lease, and the worker registers both a `worker` and a `maintenance`
lease, in `flow_runtime_readers` when it starts; each process renews its leases every 10 seconds
for 30 seconds. While `minimum_reader_version` is 4 or above (the default is 5), the following
are refused with `403 feature_disabled`, `details.reason = "scope_runtime_not_ready"`, and
nothing is written, unless the API, worker and maintenance components all hold a live lease at
or above `minimum_reader_version` and no live lease is below it:

- every Page creation and every Page edit, in workspaces and in personal spaces, including
  plain Pages created through the REST v1 API, MCP or the CLI;
- every write in a personal space;
- block transfers and portable package export and import, which the capabilities response
  then also reports as unsupported.

Reading, workspace comments and objects other than Pages are not affected. The worker is
therefore required for editing Pages: an installation that runs the API without the worker can
read Flow content but cannot write Pages. A stopped process keeps its lease for up to 30
seconds, so an ordinary restart does not interrupt writes; if the worker stays down longer,
Page writes are refused until it is back. To see the live leases:

```sql
SELECT component, reader_version, lease_expires_at FROM flow_runtime_readers
 WHERE lease_expires_at > now() ORDER BY component;
```

## Turning a capability off

Capabilities are switched with SQL on the singleton row. The CHECK constraints require
`external_mounts` to be off before `personal_writes`, and `personal_writes` before
`personal_provisioning`, so switch them off in one statement:

```sql
-- Turn personal Flow off entirely.
UPDATE flow_runtime_capabilities
   SET external_mounts = false, personal_writes = false, personal_provisioning = false,
       updated_at = now()
 WHERE singleton;

-- Turn off one independent feature, for example block transfers.
UPDATE flow_runtime_capabilities SET block_transfers_enabled = false, updated_at = now() WHERE singleton;
```

The change takes effect on the next request; no restart is needed. It survives restarts and
re-running migrations: `0106` switches the row on only in the one execution that records the
`flow_capabilities_default_on` marker in `flow_runtime_capability_rollouts`, so a later replay
(`migrations.replay`) or re-execution leaves the operator's values alone. Do not delete that marker
row; doing so lets the next execution of `0106` switch the defaults on again.

Turning `personal_writes` and `personal_provisioning` off stops new personal scopes and every
personal write; existing personal content stays readable to its owner and grantees, and owners can
still revoke grants and share links. Turning `external_mounts` off stops new mounts and level
changes but does not remove access granted by existing mounts; remove those mounts to withdraw it.

Do not lower `minimum_reader_version` on a database that has run at 5: content written with the
block registry, block shares and transfers relies on reader-5 processes.

## The incremental write window

Every Page write is decided inside the isolated worker's 50 ms CPU window. For flat (model 1) and
legacy tree Pages the window works from facts about the accepted Page that the worker computes
before the window opens, and evaluates only the units the update touched; for anything else
(structured Pages, tree moves and deletes, edits of the `doc` map, lists longer than 15,000
elements, and any shape the check cannot be certain about) it runs the full computation, as it
always did. The limits and the refusals are the same either way.

After the worker returns, the API computes the full projection of the accepted document and compares
it with what the worker established. If they disagree, the API does not accept the write on the
worker's word: it logs

```
incremental_check_divergence: the worker's window check disagrees with the full projection; deciding again with the full window
```

at error level (fields `flow_window_check_divergence_total`, `field`, `reported`, `projected`,
`document_id`, `update_id`, `base_head_seq`; never content), and asks the worker for the full window,
which cannot skip a check. The write is accepted or refused by that second result, so a client sees
an ordinary rejection (or success), not an internal error. Any such line is a defect of the
incremental check: report it with the document and update ids. The total is only a log field; the
API exports no other counters.

To make every write run the full computation, set in the API's configuration file:

```toml
[flow]
incremental_window = false
```

The default is `true`. With `false` the worker computes no facts and every window is the full
computation, so a Page above roughly 3,000 flat blocks (or the equivalent in legacy nodes) can again
exceed the 50 ms window on small runners, exactly as before the incremental window existed.
The API reads it at startup: restart it to apply the change.
