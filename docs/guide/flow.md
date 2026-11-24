# Flow

Flow is where people and agents write together: pages of rich text that several people edit at
the same time, with comments, sharing, history and links to Forms. Flow is on for every workspace
and for every user's personal space unless an administrator turns it off.

## Personal space and workspace

Flow has two kinds of space:

| Space | Path | Use |
|---|---|---|
| Personal Flow | `/flow` | Your own space. Use it for drafts and notes that are not ready for the team. |
| Workspace Flow | `/workspace/{workspaceId}/flow` | Shared by the workspace's members. Use it for the content the team relies on. |

Open **Flow** from the rail. The scope switcher decides which space it opens: **Personal** opens
your personal Flow, a workspace opens that workspace's Flow. See
[Workspaces and projects](workspaces-and-projects.md#scope-personal-or-a-workspace).

Moving work from a draft to the team:

- **Team placements** (personal pages only) make a personal page available inside a workspace
  without moving it; see [Team placements](#team-placements).
- **Move to Page…** and **Copy to Page…** move or copy blocks into a page of the workspace; see
  [Moving and copying blocks](#moving-and-copying-blocks).
- Packages carry pages from one space to another; see [Packages](#export-import-and-packages).

If Flow is turned off for a workspace, the rail shows "Flow is off in this workspace." and links to
the Flow settings. If personal Flow is turned off on the server, the rail shows "Flow is not
available for your account."

## Pages

The **Navigator** on the left lists the pages of the space as a tree. **New Page** creates a page;
drag pages to reorder or nest them, or use the move actions on a page (keyboard: pick up a page,
move it with the arrow keys, drop it with Space). **Show navigator** and **Hide navigator** toggle
it.

A page has a title, an optional icon and cover image, and a body of blocks. Type `/` in the body to
insert a block:

| Block | |
|---|---|
| **Text**, **Heading 1**, **Heading 2**, **Heading 3** | Paragraphs and headings. |
| **Bulleted list**, **Numbered list**, **To-do list**, **Toggle list** | Lists; to-do items have a checkbox, toggles fold their children. |
| **Quote**, **Code block**, **Divider** | |
| **Reference a Form**, **Embed a Form**, **Convert to Forms…** | Links to Forms; see [Forms on a page](#forms-on-a-page). |

Images and files can be added to the body. Drag a block by its handle to move it with its children;
hold Alt (Option) to copy it, or Shift to nest it. **Find in page** searches and replaces text, and
**Outline** lists the headings.

The web interface works with pages. Flow also stores collections (table-like objects with records),
which can be created through the API or the `sylvode` command ([CLI](../integrations/cli.md)); the
web interface keeps their data but does not show them.

### Editing together

Changes are saved continuously and reach everyone on the page at once. The page shows who else is
on it. The sync indicator shows the state of your connection: **Saved**, **Saving…**,
**Offline**, **Reconnecting…**, **Read-only** and so on.

When you are offline, your edits stay in the browser and are sent when the connection returns. If
the server refuses an edit (for example because your access changed), the page shows a **Recovery
draft** with the refused changes, which you can download, copy as text or discard.

A page created in an older editor format opens as a read-only preview with **Enable editing**,
which converts it to the current editor when you have edit access.

### Trash

**Move to Trash** removes a page from the navigator. **Trash** at the bottom of the navigator lists
removed pages with **Restore** and **Delete forever**. A page can be deleted forever only after all
its subpages are in the trash.

### Searching

The Flow search (**Search pages…**) finds pages in the current space. Results can lag slightly
behind the latest edits; a result marked as stale shows how many changes the index is behind, and
**Only show results from an up-to-date index** hides such results. Flow pages also appear in
**Search and commands** (Ctrl+K).

## The page panel

The panel beside a page has five tabs:

| Tab | Contents |
|---|---|
| **Info** | Object ID, dates, **Convert to Forms…**, archive and package tools, and **Export this object**. |
| **Comments** | Page and block comments. |
| **Share** | Access, public links and team placements. |
| **Relations** | Links to and from other pages and objects, filtered by direction and type. |
| **History** | Earlier versions of the page. |

### Comments

Comment on the whole page, or select a block and choose **Comment on this block**. Mention people
in a comment; reply in a thread; **Resolve** and **Reopen** a thread.

### History

**History** lists the saved versions with who made them and when. **Show changes** displays what
changed in a version. **Restore this version** makes an earlier version current again; the version
you replace stays in the history.

### Sharing and access

Every Flow page has an access level per person or bot:

| Level | Allows |
|---|---|
| **Full access** | Everything, including sharing and access changes. |
| **Can edit** | Editing the page. |
| **Can comment** | Reading and commenting. |
| **Can view** | Reading. |

In a workspace, every member starts with the workspace's default member level (see
[Workspace Flow settings](#workspace-flow-settings)). On the **Share** tab:

- **Add a person or bot** grants a level on this page.
- A page inherits access from its parent page. **Stop inheriting** gives the page its own access
  list; **Inherit from parent again** returns to the parent's.
- A change that lowers access, including your own, is shown for review before it is applied.

Only people with full access can see who else has access.

### Public links

Under **Public link**, **Create link** makes a read-only link that anyone can open without signing
in, with an optional expiry. The link opens the page only; child pages stay private and embedded
content is left out. Copy the link when it is created: it is shown only once. **Revoke** ends it.
Public links open at `/s/{token}`.

### Team placements

The **Share** tab of a page in your personal Flow has **Team placements** when you have full access.
**Add placement** places the page in a destination workspace, optionally in a project, with an
access level for that team. The page stays in your personal Flow and you keep ownership; the
placement gives access to this page only, not to its parents or siblings. Only an owner or admin of
the destination workspace can add a placement there. **Remove placement** removes it and its access.

## Shared blocks

A block and its subtree can be shared without sharing the page. Open a block's menu and choose
**Share this block**, then choose where it is shown (**Display in the workspace**, or **Display in
a project; choose who can access**) and who can access it. Review the fixed set of blocks and
**Publish reviewed blocks**.

- A share contains at most 256 blocks. Child blocks added later are not included.
- Publishing into a workspace requires owner or admin rights there.
- Recipients see only the shared blocks, not the source page, its navigation, comments or history.
  Depending on the granted access, they can edit supported content of the shown blocks, and the
  edits update the source.
- Shared blocks appear on the workspace home page and on project overview pages, under **Shared in
  this location**, and open at `/shared-blocks/{shareId}`.

**Manage shared blocks** on the source page lists its shares: **Pause share**, review and republish
a share after the source changed, or **Revoke access** for good.

## Moving and copying blocks

**Move to Page…** and **Copy to Page…** in a block's menu transfer a block, or a block with its
children, into another page you may edit, at a chosen position. You review the transfer before
confirming it, and a receipt offers an undo. A copy gets new block identities; moving blocks pauses
the shares that contain them until they are reviewed again.

## Forms on a page

A page can work with [Forms](forms.md) of a workspace:

- **Reference a Form** links a form or a record. Nothing is copied into the page and ownership does
  not change.
- **Embed a Form** shows the form or record inside the page, subject to the form's own permissions.
- **Remove reference** removes the link only; the form or record is not changed. **Open in Forms**
  opens the target.

What you can do with a referenced form depends on both your Flow access and the form's permissions.
A form with no saved permission policy for your role is view-only from Flow; see
[Forms permissions](forms.md#permissions).

### Converting a page to Forms

**Convert to Forms…** (on the **Info** tab or in the `/` menu) opens **Convert Page to Forms**
(`/flow/{objectId}/convert`). It is offered for an active page you can edit; otherwise the panel
says "Conversion to Forms is not available for this page."

Converting creates something new in Forms and leaves the page in Flow unchanged. Later edits on
either side do not affect the other; there is no ongoing sync.

1. Choose the **Destination workspace**. You can choose among the workspaces you belong to; for a
   workspace page, its own workspace is preselected. When the server allows conversion only within
   the page's workspace, the list holds that workspace alone.
2. Choose what to create:
   - a record in an existing form (the default), or
   - a new form, by ticking **Create a new Form**.

Converting into a record (**Convert to a Forms record**) runs four steps:

| Step | What you do |
|---|---|
| **Source** | Check the page that will be converted. |
| **Target and mapping** | Choose the **Project** and **Form**, set the **Record title** (empty uses the page title) and enter the record's starting values. Page content is not copied into fields. |
| **Preview** | **Run preview** asks the server what will be created, with warnings and your effective permissions on the form. Nothing is created yet. |
| **Confirm** | Tick the confirmation and choose **Convert**. |

In the mapping step you can fill fields of these types: text, phone, address, scan code, textarea,
rich text, email, date, date time, integer, rating, progress, decimal, yes/no, single select and
multi select. Autonumber and formula fields are filled in by Forms. Other field types cannot be
filled here and can be edited in Forms afterwards; if such a field is required, the record cannot
be created from Flow.

The preview expires. If it expires, or the form's fields change before you confirm, run the preview
again. The target form must allow you to create records through Flow; if it has no permission
policy for your role, the wizard says so and the conversion is refused until an owner or admin sets
one. Workspace owners and admins are the exception: when no policy names their role, they have full
permission on the form through Flow, and a policy row for their role still applies (see
[Forms permissions](forms.md#permissions)).

Creating a new form (**Define the Form**) asks for a **Destination project**, a **Form name**, a
**Form key** (lowercase letters, digits and underscores, starting with a letter) and fields, each
with a label, key, type (short text, long text, whole number, number, date, yes or no) and whether
it is required. **Preview Form** shows the server's review; tick the confirmation and **Create
Form**. Only workspace owners and admins can create a form this way.

### Conversion jobs

Confirming a conversion opens its **Conversion job** page (`/flow/conversions/{jobId}`, or
`/workspace/{workspaceId}/flow/conversions/{jobId}`). It shows the status (In progress, Completed or
Failed), the source page, the created records or form, warnings and the lineage ID, and refreshes
by itself while the job runs.

A failed job created nothing. **Retry conversion** runs it again against the page version and form
schema the job recorded, with all permissions checked again; it is refused if the page or the form
has changed since, in which case **Start a new conversion from the Page**. The person who started a
job and workspace owners and admins can open it.

A record created by conversion shows its source page in the **Flow** section of the record page;
see [Forms](forms.md#flow-lineage).

## Export, import and packages

| Tool | Where | What it does |
|---|---|---|
| **Export this object** | **Info** tab | Downloads the page as JSON, Markdown, CSV or a **Sylvode package**. A package can include separate update and event files. |
| **Portable Page package** | **Info** tab | **Prepare Page package** downloads the page body with its images and attachments (up to 32 MiB); linked pages stay links. **Import as child Page** creates a new subpage from such a package. |
| **Page and Form archive** | **Info** tab, **Open archive tools** | A ZIP of selected pages and form definitions, optionally with the retained page versions. Importing creates the reviewed objects under the current page. |
| **Flow packages** | Account settings, `/flow/package` | Import into and export from your personal Flow. |
| **Flow export and import** | Workspace **Flow** settings, **Export and import** (`/workspace/{workspaceId}/settings/flow/package`) | Export the workspace's Flow pages as a package, or import a package after reviewing the server preview and confirming the package hash. Workspace admins only. |

Imported objects get new identities. An import shows a report with counts, the mapping from source
to new objects and any references that could not be resolved.

## Workspace Flow settings

Workspace owners and admins open **Flow** in the workspace settings
(`/workspace/{workspaceId}/settings/flow`, **Sylvode Flow settings**):

- **Enable Flow** and **Disable Flow** turn Flow on or off for the workspace. Flow is on by default.
  Disabling takes effect for every member at once: the Flow entry disappears from navigation,
  open collaboration connections close and direct links to Flow pages stop working. The pages are
  kept.
- **Default member level** sets the access every member gets on the workspace's Flow pages unless a
  page grants otherwise: **Full access**, **Edit** (the default), **Comment** or **View**. **Apply
  level** asks for confirmation because it changes access for the whole workspace.
- **Allow converting Flow pages into forms** (the **Turn on** and **Turn off** button) is on by
  default. When it is off, nobody can turn Flow pages into forms or form records in this
  workspace; pages and forms stay as they are, and turning it on again allows conversion again.
- **Export and import** opens the workspace package page described above.
- **Operations** (`/workspace/{workspaceId}/settings/flow/operations`, **Flow operations**) shows
  health, indexing lag and integrity checks of the workspace's Flow data and offers maintenance
  actions, each previewed as a dry run first. It is available while Flow is disabled. See
  [Operations](../operations.md) for the operator side.

## Agents and integrations

Bots can be given access to pages like people (**Add a person or bot**). Agents work with Flow
through MCP and the `sylvode` command; see the [MCP server](../../apps/mcp-server/README.md) and the
[CLI](../integrations/cli.md). Flow endpoints are listed in the
[API reference](../integrations/api.md).
