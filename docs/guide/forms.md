# Forms

Forms (universal forms) let a project hold its own business data: orders, contracts, inspections,
tickets or anything else. A form defines fields; records hold the values; views show the records
as tables, boards, calendars and more. People work with forms in the web interface, and scripts
and agents use the same data through the REST API, MCP and the `sylvode` command.

Every project has forms. In a Custom Form project they are the whole project; in a code project
they sit next to issues. Open **Forms** in the project tree (`/workspace/{workspaceId}/projects/{projectId}/forms`);
each form is a child row.

## Creating a form

On the forms page, start a new form and fill in:

| Field | Meaning |
|---|---|
| Key | Identifier of the form within the project, for example `order`. Used by the API and MCP. |
| Name | Display name. |
| **Title template** | How a record's title is built from its values, for example `{order_no}`. The default `{id}` uses the record id. |
| Description | Optional. |

Then add fields one by one, or edit the schema as JSON.

The **Template library** in the same panel lists the form-template packages (see
[Template packages](template-packages.md)). **Install** adds all forms of a package to the project at
once (workspace owners and admins). **Refresh templates** reloads the list.

The new form opens in **Field design**.

## Form modes

A form has these modes:

| Mode | Purpose |
|---|---|
| **Enter data** | The record list and data entry. |
| **Record detail** | One record with its details, attachments, comments, links and events. |
| **Field design** | Fields, required rules, controls and validation. |
| **Detail layout** | The record page: header, highlights, sections and side widgets. |
| **Permissions** | What the Member role may do, which records it sees, and field read and write rules. |
| **Import/export** | Import records, download import templates, export the current view, package attachments. |

**Field design**, **Detail layout**, **Permissions** and **Import/export** appear as tabs above the
form.

## Fields

| Group | Field types |
|---|---|
| Basic | Text, Textarea, Rich text, Phone, Email, Address, Location, Scan code |
| Numbers | Decimal, Integer, Amount, Rating, Progress |
| Dates and choices | Date, Date time, Single select, Multi select, Yes/No |
| People and media | Member, Attachment, Image, Signature |
| Relations | Relation, Child table |
| Automation | Autonumber, Formula, AI summary |

Notes on some types:

- **Amount** has a currency, a scale (decimal places), a rounding mode and whether negative values
  are allowed. Amounts and other decimals are exchanged as decimal strings, such as `"19.90"`; the
  API rejects a JSON number for an amount so that money is never rounded by floating point.
- **Autonumber** is filled in when the record is saved.
- **Formula** computes a value from other fields with `add`, `sum`, `subtract`, `multiply`,
  `divide`, `min`, `max` or `count`, or from the rows of a child table with `child_sum`,
  `child_min`, `child_max` or `child_count`. Formula values are shown as calculated and cannot be
  typed in.
- **Relation** links a record to records of another form. **Child table** holds child records that
  belong to the record (for example the lines of an order); the record must be saved before child
  rows can be added.
- **Signature** is drawn on a signature canvas. When the record is saved, the drawing is stored as
  a PNG file (up to 2 MiB) and an audit entry with its SHA-256 hash is written. A signature field
  can require a reason field to be filled in.
- A field that other fields or views depend on cannot be deleted until those dependencies are
  removed; **Field design** shows the blocking dependencies.

## Records

In **Enter data**, add records, edit them in a side panel and open them as a full page
(`.../forms/records/{recordId}`). The record page shows, depending on the form's **Detail layout**:

- the values in sections, with highlights in the header;
- **Child records** with **Add child record**;
- **Record links** to other records, with **Add link**;
- **Comments**, an internal discussion thread;
- **Attachments**: upload a local file or link a URL; images get previews; downloads use
  short-lived signed links;
- events: the record's change history;
- **Flow**: the Flow pages that reference this record or created it, described below.

Deleting a record moves it aside: a banner above the record list offers **Restore record**.

## Views

The view switcher above the record list holds saved views. The view types are **Table**,
**Board**, **Calendar**, **Timeline**, **Gallery**, **Pivot**, **Gantt** and **Detail**.

Filter, sort and show or hide fields; changes apply at once and are kept for you on that device.
**Save view…** stores them as a named view. Under **Who can see it**, choose **Only me** or
**Everyone in this project**. A shared view can be made the default with **Set as default for
everyone**. A view can be renamed, duplicated, made private or deleted from its menu.

Before a view is saved, a filter applies to the records already loaded; saving the view filters
all records on the server.

## Permissions

Workspace owners and admins can always do everything in a form. For the Member role, the
**Permissions** mode sets:

| Setting | Options |
|---|---|
| Actions | **View form**, **Design form**, **Create records**, **Edit records**, **Delete records**, **Export records** |
| Record scope | **All records** or **Owned records only** (records the member created) |
| **Field policies** | Per field: readable and writable. Unreadable fields are left out of responses and exports; values for unwritable fields are rejected. |

**Save permissions** stores the policy. A bot uses the role its token gives it.

Flow uses form permissions more strictly than the Forms pages do: through Flow, a form that has no
saved policy for your role is view-only. The **Permissions** mode saves the Member policy only.
For owners and admins to create records in a form from Flow, an owner or admin saves a policy for
their role through `PATCH /api/v1/forms/{form_id}/permissions`, for example:

```json
{"policies": [{"subject_type": "role", "subject_id": "admin",
  "policy": {"actions": ["form.view", "record.create", "record.update"]}}]}
```

## Import and export

**Import/export** has a four-step import:

1. **Choose source**: upload a CSV or JSON file (**Upload CSV/JSON file**), paste CSV or a JSON
   array, or **Download import template** for a ready header row.
2. **Map fields**: map each column to the record title or a field, skip it, or apply a transform
   (trim, change case, digits only and others). **Save template** keeps the mapping for the next
   import of the same form.
3. **Preview validation**: **Preview import** shows valid and invalid rows. Invalid rows must be
   fixed before committing.
4. **Commit result**: **Commit import** writes the records.

Export writes the current view, or matching records, as CSV or JSON. **Export attachment package**
packages the form's attachments for download. Large imports and exports run as background jobs.

## Flow lineage

A Flow page can reference or embed a form or a record, and a page can be converted into a record or
a new form (see [Flow](flow.md#converting-a-page-to-forms)). The **Flow** section of the record page
shows both:

- **References**: Flow pages that reference this record. Removing the reference in Flow removes it
  here and never changes the record.
- **Conversions**: conversion jobs that created this record, with their status and a link to the
  job.

If you cannot read a source page, the entry shows only that a source exists, without its title.
`GET /api/v1/form-records/{record_id}` returns the same information in its `flow_lineage` field,
and the MCP tool `form_records.get` returns the same response.

## Automation and integrations

- Every record write produces business events. External systems read them through the form and
  record event endpoints or the MCP `events.tail` tool; see [Webhooks](../integrations/webhooks.md)
  for the event ledger. Workspace webhooks carry issue, comment, label, sprint and agent task
  events, not form events.
- Scripts and agents act on forms through the REST API ([API reference](../integrations/api.md)),
  MCP ([MCP server](../../apps/mcp-server/README.md)) or the `sylvode` command
  ([CLI](../integrations/cli.md)), with a workspace bot token for that surface.
- WASM plugins can validate fields, calculate values and react to events, and form plugin packages
  add controls, data views and project apps. See [Plugins](../plugins/README.md).
- The API also duplicates and restores forms and keeps a version for every schema change.
