# Workspaces and projects

This guide covers daily work in the Sylvode web interface: finding your way around, workspaces and
their members, projects and work items, notifications and search. Related guides:

- [Forms](forms.md) for the business data inside a project.
- [Flow](flow.md) for pages your team writes together.
- [Governance](governance.md) for proposals, voting and decisions.
- [Agents](agents.md) for bots and the work they take on.

## Navigation

### The rail

The narrow bar on the left is the rail. It always shows three destinations:

| Destination | Opens |
|---|---|
| **Home** | Your workspaces, the work assigned to you and recent activity (`/workspace`). |
| **Projects** | The project list of the current workspace (`/workspace/{workspaceId}/projects`). |
| **Flow** | Flow pages of the current scope: your personal Flow (`/flow`) or the workspace's Flow (`/workspace/{workspaceId}/flow`). |

**Governance** is not shown by default. Add it, or the page you are on, as a shortcut with
**Edit navigation** at the bottom of the rail. The rail holds up to eight shortcuts. **Restore
default navigation** removes all shortcuts and keeps the three main destinations.

Bots and their connections are not a rail destination: manage bots in the workspace's **Members and
bots** page and connect them to a project in its **Execution** settings (see [Agents](agents.md)).

Each destination remembers the last page you opened in it, per scope, and takes you back there.

### Scope: personal or a workspace

The secondary column next to the rail has a scope switcher at the top. The scope is either
**Personal** or one workspace. The scope decides what **Projects** and **Flow** show:

- In the **Personal** scope, **Flow** opens your personal Flow, where you keep your own drafts.
  Projects always belong to a workspace, so **Projects** lists your workspaces instead.
- In a workspace scope, every destination shows that workspace.

If Flow is turned off for a workspace, the **Flow** destination shows "Flow is off in this
workspace." instead of opening it. See [Flow](flow.md).

### Search and keyboard shortcuts

| Keys | Action |
|---|---|
| Ctrl+K (Cmd+K on macOS) | Opens **Search and commands**: search projects, forms, Flow pages, issues and comments, switch scope or workspace, change theme or language, open settings, sign out. |
| Ctrl+, (Cmd+,) | Opens settings. |
| G then H, P or F | Goes to **Home**, **Projects** or **Flow**. |
| G then S | Opens settings. |
| P on a focused row of the secondary column | Adds that row to the rail as a shortcut. |

The sequence keys do not act while you are typing in a field or while a dialog is open.

The full search page is at `/search` (**Global Search**). It searches projects, issues and comments
in the workspaces you belong to.

### Settings

Settings open as their own mode with a list of groups. Which groups you see depends on your role:

| Group | Sections | Who sees it |
|---|---|---|
| **Account** | **Profile and appearance** (`/settings`), **My plugins** (`/plugins`), **Flow packages** (`/flow/package`, when personal Flow is available to you) | Everyone |
| **Workspace** | **General**, **Members and bots**, **Webhooks**, **Operation Records**, **Workflows**, **Flow** | Owners and admins of the current workspace. In the **Personal** scope this group is one row, **Choose a workspace…** |
| **Instance** | **Dashboard** (`/admin`), **Users** (`/admin/users`), **System** (`/admin/settings`) | Instance administrators |
| **Project settings** | **Plugins** and the project's execution settings | Owners and admins, on a project's settings pages |

## Your account

**Profile and appearance** (`/settings`, page title **Personal Settings**) holds your name, email
and avatar URL, password change, theme and language. It also lists the applications you have
connected through OAuth; see [OAuth](../integrations/oauth.md).

The **Notification Preferences** on this page are saved with your profile. Sylvode does not send
email, so they do not change what you receive; notifications appear in the web interface only.

## Workspaces

A workspace holds projects, members, bots, labels, workflows, webhooks and the workspace's Flow.

### Creating a workspace

On **Home**, choose **Create Workspace** and enter a name, a slug (lowercase letters, numbers and
hyphens; used in the URL) and an optional description. You become the workspace's owner. The slug
cannot be changed later.

The workspace home page (`/workspace/{workspaceId}`) shows your role, recent projects, the
workspace's Flow entry and blocks that other pages have shared into the workspace (see
[Flow](flow.md#shared-blocks)).

### General settings

**General** (`/workspace/{workspaceId}/settings`) edits the name and description. Under **Danger
Zone**, **Delete Workspace** removes the workspace and everything in it. Only the owner can delete
a workspace.

### Members and roles

**Members and bots** (`/workspace/{workspaceId}/members`) lists members and their roles.

| Role | Can do |
|---|---|
| **Owner** | Everything, including deleting the workspace. The creator of the workspace is its owner. |
| **Admin** | Manage members, roles, settings, webhooks, workflows and Flow settings. |
| **Member** | Regular collaborator: create and work on projects, issues, forms and Flow pages within the permissions set on them; create and edit sprints (starting, completing, cancelling and deleting a sprint needs Owner or Admin). |

Owners and admins add existing users with **Add Internal User** (search by username or email) and
change a member's role between **Admin** and **Member**. User accounts themselves are created by an
instance administrator under **Users**.

The same page has **API Tokens**: bot tokens for scripts and AI agents. **Create Token** asks
**Where will this token be used?**; a token works on exactly one surface (REST API, MCP over HTTP,
SSE or stdio, the command line, or CLI `tools call`) and is refused everywhere else. The full token
is shown once. Bots and their tokens are described in [Agents](agents.md).

### Other workspace settings

| Section | Path | Purpose |
|---|---|---|
| **Webhooks** | `/workspace/{workspaceId}/webhooks` | Send workspace events to an external URL. See [Webhooks](../integrations/webhooks.md). |
| **Operation Records** | `/workspace/{workspaceId}/connections` | A metadata-only history of every authenticated MCP, CLI and bot REST operation in the workspace. |
| **Workflows** | `/workspace/{workspaceId}/workflows` | Issue states for projects; see [Workflows](#workflows). |
| **Flow** | `/workspace/{workspaceId}/settings/flow` | Turn Flow on or off and set the default member access. See [Flow](flow.md#workspace-flow-settings). |

## Projects

A project lives in a workspace and has a **Project Type** that decides which parts it offers:

| Project type | Offers |
|---|---|
| Project Management (Code) (`code_project`) | **Overview**, **Issues**, **Board**, **Cycles** and **Forms**. |
| Custom Form (`custom_form`) | **Forms** only; the project opens on its forms page. |

Industry scenarios such as restaurant ordering or contract review are built with forms and form
templates inside a project, not with project types. See [Scenario templates](scenario-templates.md).

### Creating and editing a project

Open **Projects** and choose **Create Project**. Enter a **Project Name**, a **Project Key** (2 to 10
uppercase letters or digits; issues are numbered with it, for example `MOBILE-12`) and an optional
description, and pick the project type. Any workspace member can create a project. The key cannot
be changed after creation.

Editing a project from the project list changes its name, description and **Workflow** (a workspace
workflow, or **Use Workspace Default**).

### The project tree

In a workspace scope the secondary column shows each project as a tree:

| Row | Path | Contents |
|---|---|---|
| **Overview** | `/workspace/{workspaceId}/projects/{projectId}` | Recent work items, entry points, project export and import. |
| **Issues** | `.../issues` | The issue list. |
| **Board** | `.../board` | Issues in columns by state. |
| **Cycles** | `.../cycles` | Sprints. |
| **Forms** | `.../forms` | The project's forms, one child row per form. With many forms an **All forms** row opens the full list. See [Forms](forms.md). |
| **Apps** | `.../apps` | Apps that form plugin packages add to the project. Shown when the project has apps, and always to owners and admins. See [Plugins](../plugins/README.md). |
| **Settings** | `.../settings` | Owners and admins only: **Plugins** and the project's execution settings for connected bots (see [Agents](agents.md)). |

Custom Form projects show only **Forms**, **Apps** and **Settings**.

### Issues

**Issues** lists the project's work items with search, filters (state, priority, assignee, label,
sprint), sorting and paging. **Create Issue** opens the full form; on the board, **Quick create
issue...** adds one with just a title and **Detailed create** opens the full form.

Every issue has an identifier made of the project key and a running number, such as `WEB-12`; the
lists, the board and the issue page show it. An issue has a title, a Markdown description, a state,
a priority (**Urgent**, **High**, **Medium**, **Low**), an assignee, labels, a sprint, attachments
and comments. The issue page also shows its
activity. Assigning someone else, or mentioning them in a comment, sends them a notification; your
own actions never notify you.

On the issue page, **Sprint** moves the issue into a planned or active sprint, or back to the
backlog with **No Sprint**; a sprint that has ended is shown only while the issue is still in it and
cannot be chosen. **Due date** is a calendar date with a picker and a clear action. It is saved as
the start of that day in UTC and shown as the same date to everyone, so a date set in one time zone
does not move to another day in another.

Assigning an issue to a bot, or mentioning a bot, can start agent work; see [Agents](agents.md).

### Labels

Labels belong to the workspace and are shared by all its projects. Add an existing label to an
issue on the issue page, or create one there with **Create label**. A label colour is a hex colour
such as `#2563eb`, and label names are unique in the workspace ignoring case (`Bug` and `bug` are
the same name). Adding a label that is already on the issue, or removing one that is not, changes
nothing.

### Board

**Board** (**Issue Board**) shows the issues in one column per state. Drag a card to another column
to change its state. Opening the board from a sprint filters it to that sprint.

### Cycles

**Cycles** (**Sprint Management**) plans sprints. **Create Sprint** takes a name, an optional
description and start and end dates; the end cannot be before the start, and names are unique in a
project (ignoring case). The page shows the **Current Sprint** with its progress and linked issues,
**Planned Sprints**, **Completed Sprints** and **Cancelled Sprints**; the last two are collapsed
groups with their own heading and count.

A project has one active sprint at a time. **Start sprint** on a planned sprint is refused while
another is active, and the message names that sprint. Refusals of the sprint rules (a second active
sprint, a status change that is not allowed, an end date before the start, a name that is already
taken) are shown in the page's language. **Complete sprint** and **Cancel sprint** open
a dialog that asks where the issues that are not finished go: **Backlog** (the default) or one of the
planned sprints. The message that follows names the sprint the issues went to, or says that nothing
needed moving. Finished issues stay in the sprint. Every moved issue records the change in its
activity, and deleting a sprint moves all its issues to the backlog the same way. A completed or
cancelled sprint cannot be started again.

Members can create sprints and edit planned ones. Starting, completing, cancelling and deleting a
sprint are for owners and admins; members do not see those buttons.

### Workflows

A workflow is the list of states an issue moves through. **Workflows** in the workspace settings
lists the workspace's workflows. The **System Default** workflow (Backlog, To Do, In Progress,
Done) is read-only.

**New Workflow** creates a custom workflow; **Manage States** adds, edits, reorders and deletes its
states. A state has a key, a display name, a category (**Active** or **Done**), a color and two
flags: **Is Initial** for the state new issues start in and **Is Terminal** for an end state.

A project uses the workflow chosen when it is edited, or the workspace default.

### Exporting and importing a project

The project **Overview** has **Export JSON** and **Import JSON**.

- **Export JSON** downloads the project's issues and their comments as one JSON file (format
  `sylvode.project_export`, version 1). Each issue has its identifier, title, description, state,
  priority and due date; each comment has the identifier of its issue, its author's name, its text
  and its time. Assignees, labels and sprints are not part of the file.
- **Import JSON** uploads such a file and adds its issues and comments to the **current** project;
  the export file is the import file, no editing needed. Owners and admins can import; members
  cannot. The issues get new identifiers in the project (the answer lists which old identifier became
  which new one), no assignee, sprint or label, and you as their creator. Comments keep their
  original times and text and are authored by you. Every issue's state must be a state of the
  project's workflow and every priority one of `low`, `medium`, `high`, `urgent`; if anything in the
  file is wrong, nothing is imported and the message lists every problem. Importing sends no
  notifications and starts no agent work.

Importing never creates a project: create the project first, then import into it.

The same operations are available as `GET /api/v1/export/project/{id}` and
`POST /api/v1/workspaces/{workspace_id}/import/project`; see the [API reference](../integrations/api.md).

## Home, notifications and your work

The secondary column of **Home** has **Overview**, **Notifications**, **My work** and **Recent**.

- **Notifications** (`/inbox`) lists your notifications in **Unread** and **Read** tabs, with
  **Mark all as read**. You are notified when you are mentioned in a comment or assigned to an
  issue, and about alerts that concern you (an alert opened or escalated, a bot finishing or
  failing its work on an alert, alert handling suppressed by a safety limit, an inbound source
  throttled). Alerts are described in [Alerts](../integrations/alerts.md).
- **My work** (`/my-work`) lists every work item assigned to you across workspaces.

## Administration

Instance administrators see the **Instance** settings group:

| Section | Path | Purpose |
|---|---|---|
| **Dashboard** | `/admin` | Overview of the instance. |
| **Users** | `/admin/users` | Create users, edit them, reset passwords, enable and disable accounts, create bot users. |
| **System** | `/admin/settings` | Read-only view of the running deployment's configuration. |

Installing Sylvode and creating the first administrator are covered in the
[installation guide](../install.md).
