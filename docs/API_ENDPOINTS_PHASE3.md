# Sylvode Phase 3 - New API Endpoints

## Webhook System

### Create a Webhook
```http
POST /api/v1/workspaces/:workspace_id/webhooks
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "My Webhook",
  "url": "https://example.com/webhook",
  "events": ["issue.created", "comment.created"],
  "active": true
}
```

`secret` (optional) sets the HMAC-SHA256 signing key; without it the server generates one. The
response carries the webhook and its `secret`. This is the only time the secret is returned: list,
get and update responses never include it, so store it in the receiver's configuration now.

### List Webhooks
```http
GET /api/v1/workspaces/:workspace_id/webhooks
Authorization: Bearer <token>
```

### Get Webhook Details
```http
GET /api/v1/workspaces/:workspace_id/webhooks/:webhook_id
Authorization: Bearer <token>
```

### Update a Webhook
```http
PATCH /api/v1/workspaces/:workspace_id/webhooks/:webhook_id
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "Updated Name",
  "active": false
}
```

### Rotate the Signing Secret
```http
POST /api/v1/workspaces/:workspace_id/webhooks/:webhook_id/rotate-secret
Authorization: Bearer <token>
```

Workspace admin or owner only. Replaces the secret with a newly generated one and returns the
webhook with the new `secret`, once. Deliveries are signed with the new secret from then on; give
it to the receiver (`sylvode-runner` accepts several `webhook_secrets`, so add the new one before
rotating and remove the old one afterwards).

### Delete a Webhook
```http
DELETE /api/v1/workspaces/:workspace_id/webhooks/:webhook_id
Authorization: Bearer <token>
```

**Supported event types:**
- `issue.created`, `issue.updated`, `issue.deleted`, `issue.status_changed`
- `comment.created`, `comment.updated`, `comment.deleted`
- `project.created`, `project.updated`, `project.deleted`
- `sprint.created`, `sprint.updated`, `sprint.deleted`

---

## Bot Connections

A project is connected to **one or several** bots, each running its agent tasks on a runner. A
connection is identified by (project, bot) and owns the rows it makes: the bot user, its workspace
seat, an `opr_` token (surface `mcp_stdio`, `read` and `write`), a webhook (`events = []`, URL =
runner address + `/webhook`), the bot's AI participant registration for the project and the
execution settings of the connection (enabled flag, channel, CLI, working directory). They are shown as
`managed_by` on the Bots and Webhooks endpoints and cannot be edited, revoked or deleted there.
Every write is one transaction and carries the connection's `version`.

Only workspace owners and admins manage a connection. Bots and connected (OAuth) applications are
refused. Project members read a reduced view. The token and the webhook's signing secret are
returned once, inside `config_block.text` (a `sylvode-runner.toml`), with `Cache-Control: no-store`;
no read returns them again.

### List and Read Connections
```http
GET /api/v1/projects/:project_id/bot-connections
GET /api/v1/projects/:project_id/bot-connections/:bot_id
Authorization: Bearer <token>
```
A project may have several connections, one per bot, ordered by bot name. A connection carries its
own `enabled` flag, channel, CLI and working directory. Within one project at most one **tmux**
connection exists per CLI type; headless connections are not limited.

The list answers `{"connections": [{"connection": {...}, "status": {...}}]}` for owners and admins
and `{"connections": [{"bot": {"id", "name"}, "enabled", "cli", "channel"}]}` for other members. The
single read answers `{"connection": {...}, "status": {...}}` (members: `{"connection": {...}}` with
the reduced view) and 404 `project has no connection for this bot`.

`connection.execution` holds `channel`, `cli`, `working_directory`, `on_complete_state`,
`on_fail_label_id`, `approval_gate` and, for the tmux channel, `session_name` (`<KEY>-<cli>`).
`status` is computed on read: the runner (`waiting`, `online`, `not_reporting`, `unreachable`), the
CLI and tmux (`ready`, `warning`, `missing`, `disabled`, `unknown`), the last delivery, and from a
fresh runner report `tmux_session` (the session row of this connection: `session_name`, `status`,
`state`, `created_by`, `hooks`, `permissions`, `mcp`) and `bypass` (`true` when tasks of the
connection run with permission checks off on its channel, `false`, or `null` when unknown).

### Connect a Bot
```http
POST /api/v1/projects/:project_id/bot-connections
Content-Type: application/json

{"bot": {"name": "Build bot"}, "runner_url": "https://runner.example.net:9090",
 "cli": "claude", "working_directory": "/srv/projects/example",
 "channel": "tmux", "on_complete_state": "auto", "on_fail_label_id": null,
 "governance": {"max_domain_level": "voter", "can_veto_human_consensus": false,
                "reason_min_length": 50, "domain_overrides": {}}}
```
`bot` is exactly one of `{"name"}` (a new bot) or `{"id"}` (a bot already connected to another
project of the workspace, with the same `runner_url`). `channel`, `on_complete_state`,
`on_fail_label_id` and `governance` are optional. `working_directory` is required for both
channels (for tmux it is where the runner starts the session). The tmux channel needs
`cli: "claude"` (400 `cli: only claude can use the tmux channel`). The runner address must pass the
outbound policy (`[outbound] allowed_hosts`). The answer is `{connection, status, config_block}`;
`config_block` is `null` when an existing bot was added.

Refusals (409): `project already has a connection for this bot`, and
`channel: project already has a tmux connection for <cli> (bot <name>)`.

### Update, Disable, Enable, Rotate
```http
PATCH /api/v1/projects/:project_id/bot-connections/:bot_id          {"expected_version": 3, "cli": "claude"}
POST  /api/v1/projects/:project_id/bot-connections/:bot_id/disable  {"expected_version": 3}
POST  /api/v1/projects/:project_id/bot-connections/:bot_id/enable   {"expected_version": 3}
POST  /api/v1/projects/:project_id/bot-connections/:bot_id/rotate   {"expected_version": 3}
```
A stale version answers business code 409 `connection changed; reload` with the current
connection. Changing `channel` or `cli` is checked against the tmux rule (the 409 above). Changing
the address or rotating bumps the version of every connection of the bot. Disabling one connection
leaves the project's other connections running; the bot's token and webhook go off only when its
last enabled connection is disabled. Rotate replaces the token and the signing secret of the bot
(shared by all its projects) and returns a new `config_block`; the bot's tasks wait (at most one
hour) until the runner has used the new token.

### Remove a Connection
```http
DELETE /api/v1/projects/:project_id/bot-connections/:bot_id?expected_version=3
```
Removes that connection and the bot's participant registration in the project. When it was the
bot's last connection the token is revoked, the webhook deleted and the workspace seat removed; the
bot's user row is kept. Deleting a project does the same for every connection of it.

### Runner Report
```http
PUT /api/v1/ai/runner/report
Authorization: Bearer <bot token>
```
The runner's self-report (a subset of `sylvode.doctor.v1`), sent after its handshake and every
600 seconds. Only component names, states and versions are kept, plus the `tmux_session` rows (project key, CLI and five fixed-vocabulary fields) and the `posture` row (`bypass` per channel); larger than 16 KiB answers 413.
`GET /api/v1/ai/protocol` lists the `runner_report` feature and `runner_report_interval_secs`.

### Removed
The AI participants API (`/projects/:id/ai-participants` and its `{id}` routes) and
the project-level execution settings (read and write) are gone: they are part of each connection. Learning
records are read per project: `GET /projects/:id/ai-participants/:bot_id/learning` and
`.../alignment-stats`.

---

## Notification Center

### List Notifications
```http
GET /api/v1/notifications?page=1&per_page=20&unread_only=true
Authorization: Bearer <token>
```

**Query parameters:**
- `page` (optional): page number, default 1
- `per_page` (optional): items per page, default 20, maximum 100
- `unread_only` (optional): return only unread notifications, default false

**Response:**
```json
{
  "notifications": [...],
  "total": 100,
  "page": 1,
  "per_page": 20,
  "total_pages": 5,
  "unread_count": 15
}
```

### Mark a Notification as Read
```http
PATCH /api/v1/notifications/:id/read
Authorization: Bearer <token>
```

### Mark All Notifications as Read
```http
PATCH /api/v1/notifications/read-all
Authorization: Bearer <token>
```

### Delete a Notification
```http
DELETE /api/v1/notifications/:id
Authorization: Bearer <token>
```

**Notification types:**
- `mention` - @mention
- `assignment` - work item assignment
- `comment_reply` - comment reply
- `issue_update` - work item update
- `project_update` - project update

---

## Full-Text Search

### Search
```http
GET /api/v1/search?q=bug&type=issue&workspace_id=xxx&limit=50
Authorization: Bearer <token>
```

**Query parameters:**
- `q` (required): search keyword
- `type` (optional): search type - `issue`, `project`, `comment` (all types are searched if omitted)
- `workspace_id` (optional): filter by workspace
- `project_id` (optional): filter by project
- `limit` (optional): number of results, default 50, maximum 100

**Example response:**
```json
{
  "query": "bug",
  "total": 3,
  "results": [
    {
      "type": "issue",
      "id": "...",
      "key": "PROJ-123",
      "title": "Fix login bug",
      "description": "...",
      "status": "open",
      "project_id": "...",
      "rank": 0.45
    },
    {
      "type": "comment",
      "id": "...",
      "content": "This bug is critical",
      "work_item_id": "...",
      "created_by": "...",
      "created_at": "...",
      "rank": 0.32
    }
  ]
}
```

**Search characteristics:**
- Based on PostgreSQL full-text indexes (tsvector + GIN)
- Weighting: title > description > key
- Relevance ranking (ts_rank)
- Permission filtering (only resources the user may access are returned)
- Prefix matching supported

---

## Data Import and Export

### Export a Project
```http
GET /api/v1/export/project/:project_id?format=json
Authorization: Bearer <token>
```

**Query parameters:**
- `format` (optional): `json` or `csv`, default `json`

**JSON export contents:**
- Project metadata
- All work items
- All comments
- Export timestamp

**CSV export contents:**
- Work items only (Key, Title, Status, Priority, Type, Description, Created At, Updated At)

**Response headers:**
```
Content-Type: application/json (or text/csv)
Content-Disposition: attachment; filename="project_XXX_export.json"
```

### Import a Project
```http
POST /api/v1/workspaces/:workspace_id/import/project
Authorization: Bearer <token>
Content-Type: application/json

{
  "project_key": "EXISTING",  // Optional: import into an existing project
  "project_name": "New Project",  // Optional: required when creating a new project
  "project_description": "Description",
  "issues": [
    {
      "key": "PROJ-1",  // Regenerated on import
      "title": "Issue title",
      "description": "Issue description",
      "status": "open",
      "priority": "high",
      "type": "bug"
    }
  ]
}
```

**Response:**
```json
{
  "project_id": "...",
  "project_key": "PROJ",
  "issues_created": 10,
  "issues_failed": 0,
  "errors": []
}
```

**Import characteristics:**
- Can create a new project or import into an existing one
- Work item numbers are incremented automatically
- Transaction handling keeps data consistent
- Errors are collected and reported
- Only workspace administrators can import

---

## Permission Requirements

| API | Required permission |
|-----|---------|
| Webhook management | Workspace administrator |
| Bot connections | Workspace owner or administrator (members read a reduced view; bots and connected applications are refused) |
| Notification management | Current user (can only manage their own notifications) |
| Search | Workspace member (searches only resources the user may access) |
| Project export | Workspace member |
| Project import | Workspace administrator |

---

## Error Responses

All APIs follow a unified error response format:

```json
{
  "error": "Error message"
}
```

**HTTP status codes:**
- `400 Bad Request` - invalid request parameters
- `401 Unauthorized` - not authenticated
- `403 Forbidden` - insufficient permission
- `404 Not Found` - resource does not exist
- `409 Conflict` - resource conflict
- `500 Internal Server Error` - server error

---

## Usage Examples

### Create a webhook that listens for issue creation
```bash
curl -X POST https://api.example.com/api/v1/workspaces/xxx/webhooks \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Slack Notifications",
    "url": "https://hooks.slack.com/services/XXX",
    "events": ["issue.created", "issue.updated"],
    "active": true
  }'
```

### Search all work items
```bash
curl -X GET "https://api.example.com/api/v1/search?q=authentication&type=issue" \
  -H "Authorization: Bearer $TOKEN"
```

### Export a project as JSON
```bash
curl -X GET "https://api.example.com/api/v1/export/project/xxx?format=json" \
  -H "Authorization: Bearer $TOKEN" \
  -o project_export.json
```

### Get unread notifications
```bash
curl -X GET "https://api.example.com/api/v1/notifications?unread_only=true" \
  -H "Authorization: Bearer $TOKEN"
```

---

## Database Migrations

**Run the migrations:**
```bash
# Execute in order
psql $DATABASE_URL < migrations/0005_webhooks.sql
psql $DATABASE_URL < migrations/0006_notifications.sql
psql $DATABASE_URL < migrations/0007_fulltext_search.sql
```

**Check the full-text index:**
```sql
-- View the search vector
SELECT key, title, search_vector FROM work_items LIMIT 1;

-- Test a search
SELECT key, title, ts_rank(search_vector, to_tsquery('english', 'bug:*')) as rank
FROM work_items
WHERE search_vector @@ to_tsquery('english', 'bug:*')
ORDER BY rank DESC
LIMIT 10;
```

---

## Next Steps

1. Run the database migrations
2. Compile and start the API service
3. Test each endpoint
4. Integrate the webhook trigger logic into the existing APIs
5. Integrate the notification generation logic
6. Implement asynchronous Worker tasks
