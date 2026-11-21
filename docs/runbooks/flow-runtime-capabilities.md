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
before the window opens, and evaluates only the units the update touched, but only for an update
that is a strict causal extension of the Page's current head (the client had received everything
accepted before it wrote). For anything else (an update made on an older version of the Page,
which is what two clients editing at once produce, structured Pages, tree moves and deletes, edits
of the `doc` map, edits of the metadata of a deleted node, a Page whose content that no documented
limit counts has grown past a fixed budget, and any shape the check cannot be certain about) it runs
the full computation, as it always did. The limits are the same either way, and for every write the
worker accepts or refuses on its own the decision is the full computation's, with two exceptions
that are deliberate. With the incremental window on, the worker refuses (reason
`logical_id_rewritten`) an update that writes or deletes the logical id of a tree node that existed
before it; no client does that, and the full computation accepts it on stale id caches. With either
setting of the window, the worker refuses an update whose tree ops name a parent that does not exist
when the op runs (see "Refused tree updates" below).
And the API, not the worker, has the last word on every accepted write (next paragraph). On a
Page too large for the full computation to fit the 50 ms window (more than about 3,000 flat
blocks on the reference runner), an update that takes the full computation is refused by the CPU
limit.

After the worker returns, the API computes the full projection of the accepted document and compares
it with what the worker established. If they disagree, the API does not accept the write on the
worker's word: it logs

```
incremental_check_divergence: the worker's window check disagrees with the full projection; deciding again with the full window
```

at error level (fields `flow_window_check_divergence_total`, `field`, `reported`, `projected`,
`document_id`, `update_id`, `base_head_seq`; never content), and asks the worker for the full window,
which cannot skip a check. The API also checks its own projection of the document the full window
accepted against every limit. The write is accepted or refused by that second result, so a client
sees an ordinary rejection (or success), not an internal error. If the second result is contradicted
as well (or the first attempt was already the full window), the write is refused with an internal
error and logged at error level with
`window_divergence_refused: the host's projection of the document the full window accepted is not within the limits; the write is refused`.
An `incremental_check_divergence` line is a defect of the incremental check: report it with the
document and update ids. A `window_divergence_refused` line is not always one. It also appears when
the full computation's own result is contradicted: with the incremental window off, or for a Page
that has no facts, an update that rewrites a node's logical id to another node's id is accepted by
the worker (it reads stale id caches) and the API's projection of the result can be over a limit,
which `next` would have committed and this refuses. Report it when the Page has facts and the
update is not such a rewrite. A Page whose incremental checks are contradicted three times within
ten minutes is decided by the full window directly for the next ten minutes (logged once at warning
level as `window_cooldown`), so it pays one window per write instead of two. Only a contradicted
incremental check counts toward that; a contradicted full window does not, since the full window
would not change it. The record is per process and holds at most 1,024 documents. The total is only
a log field; the API exports no other counters.

The worker is started with an empty environment (it inherits nothing from the API process); the
only variables it sees are the ones the API sets for it.

To make every write run the full computation, set in the API's configuration file:

```toml
[flow]
incremental_window = false
```

The default is `true`. With `false` the worker computes no facts and every window is the full
computation, which is what the worker did before the incremental window existed: its decision and
the document it accepts are the same for every input except an update whose tree ops name a parent
that does not exist when the op runs, which is refused with either setting (see "Refused tree
updates" below). The API is stricter than before with either
setting: it refuses, with an internal error, a document whose projection is over a limit, which was
committed before. With `false`, a Page above roughly 3,000 flat blocks (or the equivalent in legacy
nodes) can again exceed the 50 ms window on small runners, exactly as before the incremental window
existed.
The API reads it at startup: restart it to apply the change.

### Refused tree updates

A browser's Loro document panics inside its WASM module when it imports an update that continues the
document's head and has a tree `create` or `move` under a parent that does not exist when the op
runs; the document object is unusable afterwards. The server refuses such an update before it is
stored or relayed, as `invalid_update` (not retryable) with `details.reason` set to one of:

- `tree_parent_missing`: the parent was never created, is a node of another tree, is the node itself,
  or is created by a later op of the same update. A parent that exists and is deleted (including one a
  concurrent update deleted) is not refused.
- `tree_parent_cycle`: a move puts a node below one of its own descendants (only for a change made on
  the head; a conflict between concurrent moves is resolved by the library and is not refused).

The refusal is logged at warning level as `collab write: refused, the update names a tree parent that
does not exist when the op runs or moves a node below itself` with `document_id`, `update_id` and
`reason` (never content). It holds with `incremental_window = false`, for every tree in the document
and for uploaded packages (the package's whole history is read; the refusal is returned with the
same reasons). The API also applies the rule to the document the worker returned, so a worker that
accepted such an update is not trusted.

Documents stored before this check may hold such an update. No scanner is provided. A document whose
history holds one makes every browser that opens it recover through the editor's reload state, and can
be found by exporting the page as a package and importing it into a scratch instance, which reads the
whole history and refuses it with one of the reasons above. The frontend no longer lets such a
document stay dead: a failed import of server bytes discards the document and rebuilds it from the
REST bootstrap, at most twice within a minute, after which the editor shows its load error.
