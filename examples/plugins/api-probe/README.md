# api-probe

A test plugin with no business behaviour. Its actions call catalogue operations through the plugin host and
return the outcome of each call. It is used to test which operations a plugin can reach, how grants scope
them, the host quotas, and the refusals.

Layout: `action/` is the guest crate (built by `../build.sh` into `../dist/api-probe-action.wasm`),
`sv-package.json` is the manifest template, `build.sh` signs the package.

## Build

```bash
../build.sh                                              # only when the guest crate changed
./build.sh <key-file> <out.svpkg> [id,id,...]
```

The third argument is the list of capability ids the package requests; without it the package requests
`work_items.items.read`, `flow.objects.read`, `governance.proposals.read` and `forms.views.read`. Request more
ids (including write and destructive ones) by listing them; the ids must be plugin-exposed catalogue ids
(see the catalogue browser under Market, tab Developers). The package declares no binds, so every grant
applies to the whole installation scope (a project or the workspace).

`<key-file>` is the secret PKCS#8 key of publisher `qa`; it is created with mode 0600 when missing and never
committed. The script prints the public key; register it once per workspace:

```bash
POST /api/v1/workspaces/{workspace_id}/publisher-keys   {"publisher": "qa", "public_key": "<printed>"}
```

## Install and approve

1. Upload the `.svpkg` in the console (Market, Installed, Request) or `POST /api/v1/workspaces/{w}/packages`
   (multipart `file`).
2. Request an installation with the scope to test: a project (`{"kind": "project", "project_id": ...}`) or the
   workspace.
3. Review and approve. The review screen groups the requested ids by area and risk. Destructive ids are
   unticked; tick one and confirm to grant it. Untick ids to test `capability_not_granted`.

## Invoke

`POST /api/v1/installations/{installation_id}/actions/{action}` as a workspace member, body
`{"input": {...}, "context": {"project_id": "..."}}` (context optional; it must lie inside the scope).

### `call`: one operation, with the data

```json
{"input": {"op": "work_items.items.read/list", "request": {"path": {"project_id": "<uuid>"}, "query": {}}}}
```

`op` is an operation key of the catalogue (`GET /api/v1/catalogue?exposure=plugin`, the `operations[].op` field).
`request` is `{"path": {name: value}, "query": {name: value}, "body": json}`; leave it out when the operation
needs none.

### `probe`: a list of operations, outcomes only

```json
{"input": {"calls": [
  {"op": "work_items.items.read/list", "request": {"path": {"project_id": "<p1>"}}},
  {"op": "work_items.items.read/list", "request": {"path": {"project_id": "<p2>"}}},
  {"op": "workspace.members.read/list"},
  {"op": "flow.objects.purge", "request": {"path": {"id": "<uuid>"}}, "repeat": 21}
]}}
```

Every call runs in order and a refusal does not stop the list. `repeat` repeats one call (1 to 50, default 1),
for quota tests. At most 100 entries. The answer is

```json
{"ok": true, "output": "{\"results\": [...]}", "invocation": "<id>"}
```

with one result per call: `{"index", "op", "ok": true, "bytes": <size of the data>}` or
`{"index", "op", "ok": false, "kind": "denied|invalid|conflict|limit|not_found|unavailable", "code": "..."}`.
Codes to expect: `capability_not_exposed` (the op is not plugin-exposed), `capability_not_granted` (id not
ticked), `capability_scope_mismatch` (target outside the installation scope), `plugin_host_call_limit` and
a 429-style `limit` (quotas), `installation_not_active` (disabled or revoked mid-run).

## Where the calls show

The installation's log dialog (Market, Installed, Log) lists recent invocations and, below them, the recent
write calls from `GET /api/v1/installations/{id}/calls`: operation, outcome, duration and time. Read calls are not
audited per call.
