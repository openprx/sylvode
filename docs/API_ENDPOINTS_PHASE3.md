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
