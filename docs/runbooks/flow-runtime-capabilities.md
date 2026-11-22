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

**Experimental, off by default (`[flow] incremental_window = false`).** The default is the behaviour
before the feature existed: every write runs the full computation inside the worker's 50 ms window.
It is off because, with it on, one crafted update can push a text past the setup's per-text budget
and leave a page whose later writes are all refused by the content limit, which the full
computation never does. The rest of this section describes the incremental window for an operator
who turns it on.

Every Page write is decided inside the isolated worker's 50 ms CPU window. The ceiling is a timer on
the worker's own CPU clock (`CLOCK_PROCESS_CPUTIME_ID`, user plus system time, the clock the host
measures it with): the worker is killed once it has consumed the ceiling, never before, and at most
one scheduler tick (1 to 10 ms, depending on the kernel's `CONFIG_HZ`) after it, however loaded the
host is. The budget counts from the moment the window opens, and work after the window closes
(exporting the snapshot, writing the response) is not counted. The same holds for the import
window's 2 s ceiling, where the head export and the fingerprints come after the window. The worker
needs the `timer_create`, `timer_settime` and `timer_delete` syscalls; if a sandbox blocks them,
every write fails closed as a host failure (worker exit status 71). For flat (model 1) and
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

To let writes use the incremental window (experimental), set in the API's configuration file:

```toml
[flow]
incremental_window = true
```

The default is `false`. With `false` the worker computes no facts and every window is the full
computation, which is what the worker did before the incremental window existed: its decision and
the document it accepts are the same for every input except an update whose tree ops name a parent
that does not exist when the op runs, which is refused with either setting (see "Refused tree
updates" below). The API is stricter than before with either
setting: it refuses, with an internal error, a document whose projection is over a limit, which was
committed before. With `false`, a Page above roughly 3,000 flat blocks (or the equivalent in legacy
nodes) can again exceed the 50 ms window on small runners, exactly as before the incremental window
existed.
The API reads it at startup: restart it to apply the change.

### The worker's setup, its budgets and its backstop

Before its window opens the worker loads the Page, prewarms it and computes the facts (the setup),
outside the CPU and memory ceilings of the window. Three guards bound it. Each yields no facts when
it fires, which sends the write to the full window (an ordinary outcome, not a failure), and the
worker reports which one fired for every attempt it opened its window in, accepted, refused or
killed (`setup_guard` on the `flow::apply_refusal` line, and the `window_setup_guard` warning):

- `allocation`: the setup requested more than 2 GiB from the allocator.
- `cpu`: the setup consumed more CPU time than `[flow] setup_cpu_budget_ms` (default 2500, range
  500 to 4000), or its CPU clock could not be read. The budget is machine-dependent: it was
  calibrated on a runner about 3.6 times slower than a development machine, with the largest honest
  pages needing about half of it. A host slower than that, or one whose CPU time is inflated, may
  raise it; the setup's wall timeout of 5 seconds does not move.
- `text`: one text cost the setup more than the window's own CPU ceiling (50 ms of CPU time) or 48 MiB
  of allocation to decode and convert. Before the incremental window existed every write decoded the
  Page inside that ceiling, so a text above it could never have been accepted; the largest text the
  limits allow (100,000 characters, heavily formatted, with a long edit history) costs a quarter of
  it. It is not configurable.

With the incremental window on, the API also measures the texts of the blocks an accepted update
touched, in the candidate document the worker returned, against the same per-text budget before it
commits anything. An update that would take a text over it (a few hundred alternating marks on one
character, say) is refused with `limit_exceeded{text_decode_cost}` (the limit is the budget in
milliseconds), nothing is applied, and the Page stays writable: no text over the budget is ever
committed by an incremental window. The full window needs no such measure, because it decodes every
text inside its own 50 ms ceiling.

A fired guard also puts the document on the full window for the next ten minutes (logged once at
warning level as `window_setup_cooldown`), so the writes that follow skip the facts computation and
pay one metered window each, not a setup that can cost seconds. The cooldown is not a divergence and
ends by itself.

After a fired guard the full window decodes every text again inside its 50 ms window, so a Page with
a text that is too expensive is refused with the content limit `decode_apply_cpu_ms`, on every
surface, and the session stays open. Removing the text's formatting does not repair it: that update
touches the text, so the full window decodes it and the CPU limit kills the window again. Deleting
the whole block may be accepted (the document's later state no longer holds the text), but that has
not been verified; there is no operator restore that replaces the document state yet.

Backstop for what the guards cannot interrupt (one library call: a Page that was grown before the
per-text guard existed can hold a text whose decode alone takes seconds): a setup that times out at
5 seconds without having been starved is decided again by the same write with the worker told to
compute no facts (the full window), and the document is decided by the full window directly for the
next ten minutes. It is logged once at warning level as `window_setup_backstop`. Only if that
attempt fails too is the write refused, with the answer the full window produces (the content limit,
or `server_rejected{isolated_setup_exhausted}` when even the load timed out). A setup was starved,
not heavy, when it consumed less than 80 % of the CPU budget or when its CPU was under 70 % of the
wall time it was given (a host that did not schedule it, whatever it had consumed): the client is
told `apply_busy` (`setup_starved`, resend), as before, and nothing is cooled.

A setup that ends without facts for another reason than a guard (a Page over the item budget) has
also decoded its texts on the engine it holds, so the worker starts the full window from a freshly
loaded engine in that case too, and the full window meters every decode, as it did before the
incremental window existed.

Residual bands of the busy classification, documented and not changed: a write that needs 40 to
50 ms of CPU (the worker killed at the 100 ms wall limit with 40 to 50 ms of CPU) on a worker given
only 40 to 50 % of a core is answered with the wall content limit although it would be accepted on an
idle host; and the CPU of a starved worker is read when the host sees the marker, so a late read lowers
the figure and biases toward `apply_busy`. Both are bounded by the client's retry caps.

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
