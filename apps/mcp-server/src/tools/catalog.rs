//! Tool catalogue: one entry per registry tool with its category, access class, one line
//! summary and search keywords.
//!
//! The catalogue is the only input for read/write classification, categories and summaries.
//! `ToolDefinition.description` stays the detailed text of the schema. The classification rule:
//! a tool is `Read` only when it causes no state change in Sylvode (no row written, no job
//! started, nothing staged). In doubt it is `Write`: a read misclassified as a write only costs
//! a permission prompt, the opposite defeats client side permission systems. Exports, previews
//! and "extract/review" scenario tools are `Write` for that reason and are listed in
//! [`WRITE_BY_JOB_SEMANTICS`], so a review can move them with a baseline change.
//!
//! The tests of this module make a misclassification fail CI: completeness, an HTTP method
//! witness for every read and write tool, a name heuristic guard, and a hash pinned in
//! `tool-registry-baseline.json`.

use crate::protocol::ToolDefinition;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

impl Access {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

#[derive(Debug)]
pub struct ToolMeta {
    /// Exact registry name.
    pub name: &'static str,
    /// One of [`CATEGORIES`].
    pub category: &'static str,
    pub access: Access,
    /// One line, at most 120 characters, imperative, no trailing period.
    pub summary: &'static str,
    /// Search synonyms ("close", "resolve" for `work_items.update`).
    pub keywords: &'static [&'static str],
}

#[derive(Debug)]
pub struct Category {
    pub name: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
}

/// The frozen category names.
pub static CATEGORIES: &[Category] = &[
    Category {
        name: "projects",
        title: "Projects",
        summary: "Create, read, update and delete projects",
    },
    Category {
        name: "project_types",
        title: "Project types",
        summary: "Read the project scenario types and their capability schemas",
    },
    Category {
        name: "scenario_templates",
        title: "Scenario templates",
        summary: "Browse and install ready-made multi-form setups",
    },
    Category {
        name: "context",
        title: "Context",
        summary: "Read project context, governance and the agent policy",
    },
    Category {
        name: "forms",
        title: "Universal forms",
        summary: "Forms, records, attachments, permissions, imports and business events",
    },
    Category {
        name: "layouts",
        title: "Layouts",
        summary: "Check layouts and propose them as drafts for a person to approve",
    },
    Category {
        name: "plugins",
        title: "Plugins",
        summary: "Install, inspect and invoke WASM plugins",
    },
    Category {
        name: "work_items",
        title: "Work items",
        summary: "Issues of a project: list, read, create, update, label",
    },
    Category {
        name: "comments",
        title: "Comments",
        summary: "Discuss work items",
    },
    Category {
        name: "files",
        title: "Files",
        summary: "Upload files and get their URLs",
    },
    Category {
        name: "labels",
        title: "Labels",
        summary: "Workspace and project labels",
    },
    Category {
        name: "sprints",
        title: "Sprints",
        summary: "Plan iterations of a project",
    },
    Category {
        name: "milestones",
        title: "Milestones",
        summary: "Workspace milestones that span projects",
    },
    Category {
        name: "proposals",
        title: "Proposals",
        summary: "Governance proposals and check results",
    },
    Category {
        name: "release",
        title: "Release",
        summary: "Release readiness gates and blockers",
    },
    Category {
        name: "code",
        title: "Code",
        summary: "Context and governed proposals for code tasks",
    },
    Category {
        name: "documents",
        title: "Documents",
        summary: "Governed document summaries, risk reviews and approvals",
    },
    Category {
        name: "operations",
        title: "Operations",
        summary: "Governed inspection reports and corrective actions",
    },
    Category {
        name: "workspace",
        title: "Workspace",
        summary: "Members, global search and bot operation records",
    },
    Category {
        name: "flow",
        title: "Flow",
        summary: "Pages, collections, shares, comments, imports, exports and maintenance of Flow",
    },
    Category {
        name: "tasks",
        title: "Agent tasks",
        summary: "The protocol of tasks assigned to a bot: get, acknowledge, report progress, complete, fail",
    },
];

/// Tools whose name suggests the opposite access class, each with the reason.
///
/// Names ending in a write verb must be `Write`, names ending in a read verb must be `Read`,
/// unless they are listed here.
pub static ACCESS_EXCEPTIONS: &[(&str, &str)] = &[];

/// `Read` tools that issue a non-GET request, each with the endpoint and why it reads only.
pub static POST_READ_ENDPOINTS: &[(&str, &str)] = &[
    // POST /api/v1/forms/schema/validate: the save-time check of a schema. The handler stores nothing and
    // emits no event; it answers the verdict and the normalised schema. The POST carries the schema.
    (
        "forms.validate_schema",
        "POST /api/v1/forms/schema/validate (schema in the body, answers a report, stores nothing)",
    ),
    // POST /api/v1/layouts/validate: the validator of sv.layout.v1. The handler stores nothing; the POST
    // carries the spec.
    (
        "layouts.validate",
        "POST /api/v1/layouts/validate (spec in the body, answers a report, stores nothing)",
    ),
    // POST /api/v2/flow/objects/{object_id}/collab/verify (the object_id shape): the shallow integrity
    // check of one object. The handler only reads (`verify_collab` writes no row, answers a dry-run
    // receipt with a random operation id and no audit event); the POST carries `deep: false` and an
    // idempotency key. Needs bot `write` permission only because the API gates by HTTP method.
    (
        "objects.integrity",
        "POST /api/v2/flow/objects/{object_id}/collab/verify (shallow check, read-only handler)",
    ),
    // POST /api/v2/flow/collections/{collection_id}/query: the filter, sort and group of the query
    // travel in the body; the endpoint only reads typed record projections.
    (
        "collections.query",
        "POST /api/v2/flow/collections/{collection_id}/query (query body, projections only)",
    ),
];

/// `Write` tools that issue no non-GET request when called with minimal arguments, each with
/// the reason: the write is a job or a staged artifact, or the tool refuses locally before any
/// request.
pub static WRITE_BY_JOB_SEMANTICS: &[(&str, &str)] = &[
    // GET /api/v1/forms/{form_id}/records/export: the export may create an artifact or a job.
    (
        "form_records.export",
        "export may create artifacts or jobs; conservative write",
    ),
];

/// One entry per registry tool, in registry order.
pub static CATALOG: &[ToolMeta] = &[
    ToolMeta {
        name: "projects.list",
        category: "projects",
        access: Access::Read,
        summary: "List the projects of the bound workspace",
        keywords: &["projects", "workspace", "all projects"],
    },
    ToolMeta {
        name: "projects.get",
        category: "projects",
        access: Access::Read,
        summary: "Get one project by id",
        keywords: &["project", "details"],
    },
    ToolMeta {
        name: "projects.create",
        category: "projects",
        access: Access::Write,
        summary: "Create a project in the bound workspace",
        keywords: &["new project", "add project"],
    },
    ToolMeta {
        name: "projects.update",
        category: "projects",
        access: Access::Write,
        summary: "Update the fields of a project",
        keywords: &["edit project", "rename project", "change project"],
    },
    ToolMeta {
        name: "projects.delete",
        category: "projects",
        access: Access::Write,
        summary: "Delete a project",
        keywords: &["remove project"],
    },
    ToolMeta {
        name: "project_types.list",
        category: "project_types",
        access: Access::Read,
        summary: "List the project scenario types",
        keywords: &["project type", "scenario", "kinds of project"],
    },
    ToolMeta {
        name: "project_types.get",
        category: "project_types",
        access: Access::Read,
        summary: "Get one project type with its capability schema",
        keywords: &["project type", "capabilities"],
    },
    ToolMeta {
        name: "scenario_templates.list",
        category: "scenario_templates",
        access: Access::Read,
        summary: "List scenario templates for project setup",
        keywords: &["template", "scenario", "catalog"],
    },
    ToolMeta {
        name: "scenario_templates.get",
        category: "scenario_templates",
        access: Access::Read,
        summary: "Get one scenario template by key",
        keywords: &["template", "scenario", "usage guide"],
    },
    ToolMeta {
        name: "scenario_templates.install",
        category: "scenario_templates",
        access: Access::Write,
        summary: "Install a scenario template into an existing project",
        keywords: &["apply template", "set up forms", "bootstrap"],
    },
    ToolMeta {
        name: "forms.list",
        category: "forms",
        access: Access::Read,
        summary: "List the universal forms of a project",
        keywords: &["forms", "tables", "schemas"],
    },
    ToolMeta {
        name: "forms.get",
        category: "forms",
        access: Access::Read,
        summary: "Get one universal form schema",
        keywords: &["form schema", "fields"],
    },
    ToolMeta {
        name: "forms.create",
        category: "forms",
        access: Access::Write,
        summary: "Create a universal form in a project",
        keywords: &["new form", "add form", "define form"],
    },
    ToolMeta {
        name: "forms.create_from_template",
        category: "forms",
        access: Access::Write,
        summary: "Create a form from a scenario template",
        keywords: &["form from template", "new form"],
    },
    ToolMeta {
        name: "forms.update_schema",
        category: "forms",
        access: Access::Write,
        summary: "Update the schema and layout of a form",
        keywords: &["edit form", "add field", "change fields", "alter schema"],
    },
    ToolMeta {
        name: "forms.duplicate",
        category: "forms",
        access: Access::Write,
        summary: "Duplicate a form without its records",
        keywords: &["copy form", "clone form"],
    },
    ToolMeta {
        name: "forms.schema_summary",
        category: "forms",
        access: Access::Read,
        summary: "Get schema, view, record and archive counts of a form",
        keywords: &["form overview", "statistics"],
    },
    ToolMeta {
        name: "forms.field_usage",
        category: "forms",
        access: Access::Read,
        summary: "Get value usage and dependency counts per form field",
        keywords: &["field statistics", "unused fields"],
    },
    ToolMeta {
        name: "forms.field_dependencies",
        category: "forms",
        access: Access::Read,
        summary: "List the dependencies of the fields of a form",
        keywords: &["field references", "formula", "title template"],
    },
    ToolMeta {
        name: "form_schema_versions.list",
        category: "forms",
        access: Access::Read,
        summary: "List the archived schema versions of a form",
        keywords: &["schema history", "versions"],
    },
    ToolMeta {
        name: "form_schema_versions.get",
        category: "forms",
        access: Access::Read,
        summary: "Get one archived schema version of a form",
        keywords: &["schema history", "old schema"],
    },
    ToolMeta {
        name: "form_permissions.get",
        category: "forms",
        access: Access::Read,
        summary: "Get the role permissions of a form and the caller's actions",
        keywords: &["access", "roles", "who can"],
    },
    ToolMeta {
        name: "form_permissions.update",
        category: "forms",
        access: Access::Write,
        summary: "Replace the role permission policies of a form",
        keywords: &["grant", "revoke", "access control"],
    },
    ToolMeta {
        name: "form_views.list",
        category: "forms",
        access: Access::Read,
        summary: "List the saved views of a form visible to the caller",
        keywords: &["views", "filters", "saved view"],
    },
    ToolMeta {
        name: "form_attachments.list",
        category: "forms",
        access: Access::Read,
        summary: "List attachment metadata of a form",
        keywords: &["files", "images", "attachments"],
    },
    ToolMeta {
        name: "form_attachments.create",
        category: "forms",
        access: Access::Write,
        summary: "Register attachment metadata for a form field",
        keywords: &["attach file", "add image"],
    },
    ToolMeta {
        name: "form_attachments.archive",
        category: "forms",
        access: Access::Write,
        summary: "Archive one form attachment",
        keywords: &["remove attachment", "detach"],
    },
    ToolMeta {
        name: "form_attachments.restore",
        category: "forms",
        access: Access::Write,
        summary: "Restore one archived form attachment",
        keywords: &["undo archive", "reattach"],
    },
    ToolMeta {
        name: "form_records.list",
        category: "forms",
        access: Access::Read,
        summary: "List the records of a form with paging, view, sort and filter",
        keywords: &["rows", "entries", "table data"],
    },
    ToolMeta {
        name: "form_records.export",
        category: "forms",
        access: Access::Write,
        summary: "Export form records as CSV or row data",
        keywords: &["download", "csv", "dump"],
    },
    ToolMeta {
        name: "form_records.import_preview",
        category: "forms",
        access: Access::Write,
        summary: "Preview a record import without writing records",
        keywords: &["dry run", "validate import", "csv import"],
    },
    ToolMeta {
        name: "form_records.import_commit",
        category: "forms",
        access: Access::Write,
        summary: "Commit validated import rows as new records",
        keywords: &["bulk create", "load rows", "csv import"],
    },
    ToolMeta {
        name: "form_records.get",
        category: "forms",
        access: Access::Read,
        summary: "Get one form record",
        keywords: &["row", "entry"],
    },
    ToolMeta {
        name: "form_records.create",
        category: "forms",
        access: Access::Write,
        summary: "Create a form record",
        keywords: &["add row", "new entry", "insert record"],
    },
    ToolMeta {
        name: "form_records.update",
        category: "forms",
        access: Access::Write,
        summary: "Update fields of a form record",
        keywords: &["edit row", "change entry", "modify record"],
    },
    ToolMeta {
        name: "form_records.link",
        category: "forms",
        access: Access::Write,
        summary: "Link a form record to another record or object",
        keywords: &["relate", "connect", "relation"],
    },
    ToolMeta {
        name: "form_records.relation_targets",
        category: "forms",
        access: Access::Read,
        summary: "List selectable targets of a relation field",
        keywords: &["relation options", "lookup"],
    },
    ToolMeta {
        name: "form_records.children",
        category: "forms",
        access: Access::Read,
        summary: "List the child records of a parent record",
        keywords: &["sub records", "detail rows"],
    },
    ToolMeta {
        name: "form_records.child_create",
        category: "forms",
        access: Access::Write,
        summary: "Create a child record under a parent record",
        keywords: &["add detail row", "sub record"],
    },
    ToolMeta {
        name: "form_records.child_update",
        category: "forms",
        access: Access::Write,
        summary: "Update a child record of a parent record",
        keywords: &["edit detail row", "sub record"],
    },
    ToolMeta {
        name: "form_records.child_archive",
        category: "forms",
        access: Access::Write,
        summary: "Archive a child record of a parent record",
        keywords: &["remove detail row"],
    },
    ToolMeta {
        name: "form_records.child_restore",
        category: "forms",
        access: Access::Write,
        summary: "Restore an archived child record",
        keywords: &["undo archive", "detail row"],
    },
    ToolMeta {
        name: "form_records.aggregate",
        category: "forms",
        access: Access::Read,
        summary: "Aggregate a numeric field of a form",
        keywords: &["sum", "total", "average", "count", "statistics"],
    },
    ToolMeta {
        name: "events.tail",
        category: "forms",
        access: Access::Read,
        summary: "Read recent business events of a form or record",
        keywords: &["audit", "history", "activity log", "changes"],
    },
    ToolMeta {
        name: "forms.validate_schema",
        category: "forms",
        access: Access::Read,
        summary: "Check a form schema and get the typed error report",
        keywords: &["validate schema", "check fields", "schema errors", "lint form"],
    },
    ToolMeta {
        name: "forms.create_schema_draft",
        category: "forms",
        access: Access::Write,
        summary: "Propose a form schema as a draft for a person to approve",
        keywords: &[
            "draft form",
            "propose schema",
            "new form for review",
            "change fields for review",
        ],
    },
    ToolMeta {
        name: "layouts.validate",
        category: "layouts",
        access: Access::Read,
        summary: "Check a layout spec and get the typed error report",
        keywords: &[
            "validate layout",
            "check layout",
            "layout errors",
            "capabilities of a layout",
        ],
    },
    ToolMeta {
        name: "layouts.create_draft",
        category: "layouts",
        access: Access::Write,
        summary: "Propose a layout for a slot as a draft for a person to approve",
        keywords: &["draft layout", "propose layout", "place layout", "ui for review"],
    },
    ToolMeta {
        name: "plugins.list",
        category: "plugins",
        access: Access::Read,
        summary: "List the WASM plugins installed in a project",
        keywords: &["extensions", "wasm"],
    },
    ToolMeta {
        name: "plugins.get",
        category: "plugins",
        access: Access::Read,
        summary: "Get one installed plugin and its manifest",
        keywords: &["extension", "wasm", "manifest"],
    },
    ToolMeta {
        name: "plugins.install",
        category: "plugins",
        access: Access::Write,
        summary: "Install a WASM plugin into a project",
        keywords: &["add plugin", "register plugin"],
    },
    ToolMeta {
        name: "plugins.invoke",
        category: "plugins",
        access: Access::Write,
        summary: "Invoke a plugin hook or plugin tool",
        keywords: &["run plugin", "call plugin", "execute"],
    },
    ToolMeta {
        name: "plugin_invocations.list",
        category: "plugins",
        access: Access::Read,
        summary: "List the invocation logs of a plugin",
        keywords: &["plugin runs", "plugin history"],
    },
    ToolMeta {
        name: "context.get_project",
        category: "context",
        access: Access::Read,
        summary: "Get the project context with type, connectors, workflow and decisions",
        keywords: &["project context", "workflow", "overview"],
    },
    ToolMeta {
        name: "context.get_governance",
        category: "context",
        access: Access::Read,
        summary: "Get the governance and workflow context of a project",
        keywords: &["review policy", "approvals", "decisions"],
    },
    ToolMeta {
        name: "context.get_agent_policy",
        category: "context",
        access: Access::Read,
        summary: "Get the effective agent policy of a project",
        keywords: &["allowed tools", "permissions", "policy"],
    },
    ToolMeta {
        name: "work_items.list",
        category: "work_items",
        access: Access::Read,
        summary: "List the work items of a project with filters",
        keywords: &["issues", "tickets", "tasks", "backlog", "board"],
    },
    ToolMeta {
        name: "work_items.get",
        category: "work_items",
        access: Access::Read,
        summary: "Get one work item by id",
        keywords: &["issue", "ticket", "details"],
    },
    ToolMeta {
        name: "work_items.get_by_identifier",
        category: "work_items",
        access: Access::Read,
        summary: "Get one work item by identifier such as PRX-42",
        keywords: &["issue key", "ticket number", "lookup"],
    },
    ToolMeta {
        name: "work_items.create",
        category: "work_items",
        access: Access::Write,
        summary: "Create a work item in a project",
        keywords: &["new issue", "new ticket", "add task", "file bug"],
    },
    ToolMeta {
        name: "work_items.update",
        category: "work_items",
        access: Access::Write,
        summary: "Update a work item",
        keywords: &[
            "edit issue",
            "close",
            "resolve",
            "assign",
            "change state",
            "set priority",
        ],
    },
    ToolMeta {
        name: "work_items.add_label",
        category: "work_items",
        access: Access::Write,
        summary: "Add one label to a work item",
        keywords: &["tag", "categorize"],
    },
    ToolMeta {
        name: "work_items.remove_label",
        category: "work_items",
        access: Access::Write,
        summary: "Remove one label from a work item",
        keywords: &["untag"],
    },
    ToolMeta {
        name: "work_items.list_labels",
        category: "work_items",
        access: Access::Read,
        summary: "List the labels of a work item",
        keywords: &["tags"],
    },
    ToolMeta {
        name: "work_items.delete",
        category: "work_items",
        access: Access::Write,
        summary: "Delete a work item",
        keywords: &["remove issue", "remove ticket"],
    },
    ToolMeta {
        name: "work_items.search",
        category: "work_items",
        access: Access::Read,
        summary: "Search work items across the accessible projects",
        keywords: &["find issue", "find ticket", "query"],
    },
    ToolMeta {
        name: "comments.list",
        category: "comments",
        access: Access::Read,
        summary: "List the comments of a work item",
        keywords: &["discussion", "replies", "thread"],
    },
    ToolMeta {
        name: "comments.create",
        category: "comments",
        access: Access::Write,
        summary: "Post a comment on a work item",
        keywords: &["reply", "discuss", "mention", "add note"],
    },
    ToolMeta {
        name: "comments.delete",
        category: "comments",
        access: Access::Write,
        summary: "Delete a comment",
        keywords: &["remove comment"],
    },
    ToolMeta {
        name: "files.upload",
        category: "files",
        access: Access::Write,
        summary: "Upload a file and return its URL",
        keywords: &["attachment", "screenshot", "image", "log file"],
    },
    ToolMeta {
        name: "proposals.list",
        category: "proposals",
        access: Access::Read,
        summary: "List governance proposals",
        keywords: &["votes", "decisions", "reviews"],
    },
    ToolMeta {
        name: "proposals.get",
        category: "proposals",
        access: Access::Read,
        summary: "Get one governance proposal",
        keywords: &["vote", "decision", "review"],
    },
    ToolMeta {
        name: "proposals.create",
        category: "proposals",
        access: Access::Write,
        summary: "Create a governance proposal in draft state",
        keywords: &["new proposal", "propose"],
    },
    ToolMeta {
        name: "proposals.create_from_result",
        category: "proposals",
        access: Access::Write,
        summary: "Create a proposal from a check result",
        keywords: &["escalate", "high risk", "propose"],
    },
    ToolMeta {
        name: "check_results.create",
        category: "proposals",
        access: Access::Write,
        summary: "Record a governed check result for a high-risk action",
        keywords: &["audit", "evidence", "result"],
    },
    ToolMeta {
        name: "release.readiness.get",
        category: "release",
        access: Access::Read,
        summary: "Get the release readiness gates and blockers of a project",
        keywords: &["go live", "acceptance", "blockers", "next actions"],
    },
    ToolMeta {
        name: "members.list",
        category: "workspace",
        access: Access::Read,
        summary: "List the members and roles of the workspace",
        keywords: &["people", "users", "team", "bots"],
    },
    ToolMeta {
        name: "bot_operation_logs.list",
        category: "workspace",
        access: Access::Read,
        summary: "List the metadata-only operation records of bots",
        keywords: &["audit log", "bot activity"],
    },
    ToolMeta {
        name: "sprints.create",
        category: "sprints",
        access: Access::Write,
        summary: "Create a sprint in a project",
        keywords: &["new sprint", "iteration", "milestone"],
    },
    ToolMeta {
        name: "sprints.list",
        category: "sprints",
        access: Access::Read,
        summary: "List the sprints of a project",
        keywords: &["iterations", "milestones"],
    },
    ToolMeta {
        name: "sprints.update",
        category: "sprints",
        access: Access::Write,
        summary: "Update the fields of a sprint",
        keywords: &["edit sprint", "rename sprint", "change dates"],
    },
    ToolMeta {
        name: "sprints.delete",
        category: "sprints",
        access: Access::Write,
        summary: "Delete a sprint",
        keywords: &["remove sprint"],
    },
    ToolMeta {
        name: "milestones.list",
        category: "milestones",
        access: Access::Read,
        summary: "List the milestones of the workspace with their progress",
        keywords: &["release", "target date", "roadmap"],
    },
    ToolMeta {
        name: "milestones.get",
        category: "milestones",
        access: Access::Read,
        summary: "Get a milestone with its work items grouped by project",
        keywords: &["release", "progress", "roadmap"],
    },
    ToolMeta {
        name: "milestones.create",
        category: "milestones",
        access: Access::Write,
        summary: "Create a milestone in the workspace",
        keywords: &["new milestone", "release", "target date"],
    },
    ToolMeta {
        name: "milestones.update",
        category: "milestones",
        access: Access::Write,
        summary: "Update, close or reopen a milestone",
        keywords: &["edit milestone", "rename milestone", "close milestone", "reopen"],
    },
    ToolMeta {
        name: "milestones.delete",
        category: "milestones",
        access: Access::Write,
        summary: "Delete a milestone and clear it from its work items",
        keywords: &["remove milestone"],
    },
    ToolMeta {
        name: "labels.create",
        category: "labels",
        access: Access::Write,
        summary: "Create a label in the workspace",
        keywords: &["new label", "new tag"],
    },
    ToolMeta {
        name: "labels.list",
        category: "labels",
        access: Access::Read,
        summary: "List the labels of the workspace",
        keywords: &["tags"],
    },
    ToolMeta {
        name: "labels.list_by_project",
        category: "labels",
        access: Access::Read,
        summary: "List the labels available to a project",
        keywords: &["project tags"],
    },
    ToolMeta {
        name: "labels.update",
        category: "labels",
        access: Access::Write,
        summary: "Update the fields of a label",
        keywords: &["edit label", "rename label", "recolor"],
    },
    ToolMeta {
        name: "labels.delete",
        category: "labels",
        access: Access::Write,
        summary: "Delete a label",
        keywords: &["remove label", "remove tag"],
    },
    ToolMeta {
        name: "work_items.add_labels",
        category: "work_items",
        access: Access::Write,
        summary: "Add several labels to a work item in one request",
        keywords: &["tags", "categorize", "bulk label"],
    },
    ToolMeta {
        name: "work_items.link_page",
        category: "work_items",
        access: Access::Write,
        summary: "Link a Flow page to a work item",
        keywords: &["attach page", "page link", "document link", "spec"],
    },
    ToolMeta {
        name: "work_items.unlink_page",
        category: "work_items",
        access: Access::Write,
        summary: "Remove the link between a work item and a Flow page",
        keywords: &["detach page", "unlink page"],
    },
    ToolMeta {
        name: "work_items.list_pages",
        category: "work_items",
        access: Access::Read,
        summary: "List the Flow pages linked to a work item",
        keywords: &["linked pages", "attached pages", "documents of an issue"],
    },
    ToolMeta {
        name: "objects.list_work_items",
        category: "flow",
        access: Access::Read,
        summary: "List the work items linked to a Flow page",
        keywords: &["linked issues", "linked tickets", "page backlinks"],
    },
    ToolMeta {
        name: "search.all",
        category: "workspace",
        access: Access::Read,
        summary: "Search projects, work items and comments across the workspace",
        keywords: &["global search", "find", "lookup"],
    },
    ToolMeta {
        name: "code.task_context.get",
        category: "code",
        access: Access::Read,
        summary: "Get the project context and optional work item context for a code task",
        keywords: &["coding task", "repository context"],
    },
    ToolMeta {
        name: "code.change_proposal.create",
        category: "code",
        access: Access::Write,
        summary: "Record a governed code change proposal as a check result",
        keywords: &["patch proposal", "pull request", "change request"],
    },
    ToolMeta {
        name: "documents.extract_summary",
        category: "documents",
        access: Access::Write,
        summary: "Record a document summary as a governed check result",
        keywords: &["summarize document", "extract"],
    },
    ToolMeta {
        name: "documents.review_risk",
        category: "documents",
        access: Access::Write,
        summary: "Record document risk findings as a governed check result",
        keywords: &["contract review", "risk", "clauses"],
    },
    ToolMeta {
        name: "approval.request",
        category: "documents",
        access: Access::Write,
        summary: "Create an approval request for a human decision",
        keywords: &["sign off", "ask approval", "authorize"],
    },
    ToolMeta {
        name: "inspection.report",
        category: "operations",
        access: Access::Write,
        summary: "Record an inspection report as a governed check result",
        keywords: &["inspect", "audit", "quality"],
    },
    ToolMeta {
        name: "corrective_action.propose",
        category: "operations",
        access: Access::Write,
        summary: "Record a corrective action proposal as a governed check result",
        keywords: &["fix plan", "remediation", "quality"],
    },
    ToolMeta {
        name: "flow.feature_get",
        category: "flow",
        access: Access::Read,
        summary: "Get the Flow rollout flag of a workspace",
        keywords: &["flow enabled", "feature flag", "rollout"],
    },
    ToolMeta {
        name: "flow.feature_set",
        category: "flow",
        access: Access::Write,
        summary: "Change the Flow rollout flag of a workspace",
        keywords: &["enable flow", "disable flow", "feature flag"],
    },
    ToolMeta {
        name: "objects.get",
        category: "flow",
        access: Access::Read,
        summary: "Get one Flow object with its content",
        keywords: &["page", "document", "navigator", "collection"],
    },
    ToolMeta {
        name: "objects.query",
        category: "flow",
        access: Access::Read,
        summary: "List the Flow objects of a workspace or project",
        keywords: &["pages", "documents", "tree", "browse"],
    },
    ToolMeta {
        name: "objects.history",
        category: "flow",
        access: Access::Read,
        summary: "Get the accepted update history of a Flow object",
        keywords: &["versions", "changes", "revisions"],
    },
    ToolMeta {
        name: "objects.create",
        category: "flow",
        access: Access::Write,
        summary: "Create a page, navigator or Collection object",
        keywords: &["new page", "new document", "new collection"],
    },
    ToolMeta {
        name: "objects.patch",
        category: "flow",
        access: Access::Write,
        summary: "Apply semantic operations to a Flow object",
        keywords: &["edit page", "write content", "modify document"],
    },
    ToolMeta {
        name: "objects.move",
        category: "flow",
        access: Access::Write,
        summary: "Move a Flow object under a new parent",
        keywords: &["reparent", "reorganize", "relocate"],
    },
    ToolMeta {
        name: "objects.link",
        category: "flow",
        access: Access::Write,
        summary: "Create a typed relation between two Flow objects",
        keywords: &["relate", "connect", "backlink"],
    },
    ToolMeta {
        name: "objects.unlink",
        category: "flow",
        access: Access::Write,
        summary: "Remove a relation from a Flow object",
        keywords: &["disconnect", "remove relation"],
    },
    ToolMeta {
        name: "objects.diff",
        category: "flow",
        access: Access::Read,
        summary: "Read the semantic diff between two history points",
        keywords: &["compare versions", "changes"],
    },
    ToolMeta {
        name: "objects.grants_get",
        category: "flow",
        access: Access::Read,
        summary: "Read the effective access and grants of a Flow object",
        keywords: &["permissions", "sharing", "who can"],
    },
    ToolMeta {
        name: "objects.grants_set",
        category: "flow",
        access: Access::Write,
        summary: "Replace the explicit grants of a Flow object",
        keywords: &["share", "permissions", "access"],
    },
    ToolMeta {
        name: "objects.inheritance_set",
        category: "flow",
        access: Access::Write,
        summary: "Set permission inheritance of a Flow object",
        keywords: &["permissions", "access", "parent"],
    },
    ToolMeta {
        name: "objects.relations",
        category: "flow",
        access: Access::Read,
        summary: "List the relations of a Flow object",
        keywords: &["links", "backlinks", "references"],
    },
    ToolMeta {
        name: "objects.search",
        category: "flow",
        access: Access::Read,
        summary: "Search Flow objects in a project or the projectless scope",
        keywords: &["find page", "find document"],
    },
    ToolMeta {
        name: "collab.projection_lag",
        category: "flow",
        access: Access::Read,
        summary: "Read the projection lag aggregates of Flow",
        keywords: &["sync delay", "health"],
    },
    ToolMeta {
        name: "collections.describe",
        category: "flow",
        access: Access::Read,
        summary: "Describe a Collection with its fields, views and counts",
        keywords: &["table schema", "fields"],
    },
    ToolMeta {
        name: "collections.query",
        category: "flow",
        access: Access::Read,
        summary: "Query the records of a Collection with filter, sort and group",
        keywords: &["rows", "table data"],
    },
    ToolMeta {
        name: "records.create",
        category: "flow",
        access: Access::Write,
        summary: "Create one record in a Collection",
        keywords: &["add row", "new entry"],
    },
    ToolMeta {
        name: "objects.reference",
        category: "flow",
        access: Access::Write,
        summary: "Reference a form or form record from a Flow object",
        keywords: &["embed form", "link record"],
    },
    ToolMeta {
        name: "objects.unreference",
        category: "flow",
        access: Access::Write,
        summary: "Remove a Flow reference to a form or record",
        keywords: &["unlink form", "detach"],
    },
    ToolMeta {
        name: "objects.convert_preview",
        category: "flow",
        access: Access::Write,
        summary: "Preview a conversion of a Flow object into forms",
        keywords: &["dry run", "convert to form"],
    },
    ToolMeta {
        name: "objects.convert_commit",
        category: "flow",
        access: Access::Write,
        summary: "Commit a previewed Flow to forms conversion",
        keywords: &["convert to form", "apply conversion"],
    },
    ToolMeta {
        name: "objects.convert_status",
        category: "flow",
        access: Access::Read,
        summary: "Read the status of a conversion job",
        keywords: &["conversion progress"],
    },
    ToolMeta {
        name: "objects.convert_retry",
        category: "flow",
        access: Access::Write,
        summary: "Retry a failed conversion job",
        keywords: &["rerun conversion"],
    },
    ToolMeta {
        name: "objects.export",
        category: "flow",
        access: Access::Write,
        summary: "Create an export job for a Flow object",
        keywords: &["download", "backup", "package"],
    },
    ToolMeta {
        name: "objects.export_workspace",
        category: "flow",
        access: Access::Write,
        summary: "Create a package export of a whole workspace",
        keywords: &["download", "backup", "package"],
    },
    ToolMeta {
        name: "objects.import_artifact",
        category: "flow",
        access: Access::Write,
        summary: "Stage one package artifact for import",
        keywords: &["upload package", "restore"],
    },
    ToolMeta {
        name: "objects.import_preview",
        category: "flow",
        access: Access::Write,
        summary: "Preview a package import without writing",
        keywords: &["dry run", "validate import"],
    },
    ToolMeta {
        name: "objects.import_commit",
        category: "flow",
        access: Access::Write,
        summary: "Commit a previewed package import",
        keywords: &["restore", "load package"],
    },
    ToolMeta {
        name: "objects.import_status",
        category: "flow",
        access: Access::Read,
        summary: "Read the report of a package import",
        keywords: &["import progress"],
    },
    ToolMeta {
        name: "collab.status",
        category: "flow",
        access: Access::Read,
        summary: "Read the Flow health and lag of a workspace",
        keywords: &["sync status", "health"],
    },
    ToolMeta {
        name: "objects.integrity",
        category: "flow",
        access: Access::Read,
        summary: "Read workspace integrity or verify one object",
        keywords: &["verify", "consistency", "corruption"],
    },
    ToolMeta {
        name: "collab.compact",
        category: "flow",
        access: Access::Write,
        summary: "Dry-run or run document compaction",
        keywords: &["shrink", "optimize", "maintenance"],
    },
    ToolMeta {
        name: "deliveries.replay",
        category: "flow",
        access: Access::Write,
        summary: "Replay or requeue deliveries of a retained window",
        keywords: &["resend", "redeliver", "webhooks"],
    },
    ToolMeta {
        name: "collab.rebuild_projection",
        category: "flow",
        access: Access::Write,
        summary: "Dry-run or run a projection rebuild of one object",
        keywords: &["reindex", "repair", "maintenance"],
    },
    ToolMeta {
        name: "collab.repair_quarantine",
        category: "flow",
        access: Access::Write,
        summary: "Dry-run or run quarantine of open integrity findings",
        keywords: &["repair", "maintenance"],
    },
    ToolMeta {
        name: "flow.comments.list",
        category: "flow",
        access: Access::Read,
        summary: "List the comment threads of a Flow object",
        keywords: &["discussion", "replies"],
    },
    ToolMeta {
        name: "flow.comments.create",
        category: "flow",
        access: Access::Write,
        summary: "Create a comment or reply on a Flow object",
        keywords: &["discuss", "mention", "reply"],
    },
    ToolMeta {
        name: "flow.comments.update",
        category: "flow",
        access: Access::Write,
        summary: "Edit the caller's own Flow comment",
        keywords: &["edit comment"],
    },
    ToolMeta {
        name: "flow.comments.delete",
        category: "flow",
        access: Access::Write,
        summary: "Delete a Flow comment",
        keywords: &["remove comment"],
    },
    ToolMeta {
        name: "flow.comments.resolve",
        category: "flow",
        access: Access::Write,
        summary: "Resolve or reopen a Flow comment thread",
        keywords: &["close thread", "reopen"],
    },
    ToolMeta {
        name: "flow.shares.list",
        category: "flow",
        access: Access::Read,
        summary: "List the public links of a Flow page",
        keywords: &["share links", "public"],
    },
    ToolMeta {
        name: "flow.shares.create",
        category: "flow",
        access: Access::Write,
        summary: "Create a public read-only link for a Flow page",
        keywords: &["share", "publish", "public link"],
    },
    ToolMeta {
        name: "flow.shares.revoke",
        category: "flow",
        access: Access::Write,
        summary: "Revoke a public link of a Flow page",
        keywords: &["unshare", "unpublish"],
    },
    ToolMeta {
        name: "flow.appearance.set",
        category: "flow",
        access: Access::Write,
        summary: "Set the icon and cover of a Flow page",
        keywords: &["emoji", "banner", "decorate"],
    },
    ToolMeta {
        name: "flow.appearance.upload",
        category: "flow",
        access: Access::Write,
        summary: "Upload an image for a Flow page icon or cover",
        keywords: &["icon", "cover", "picture"],
    },
    ToolMeta {
        name: "objects.restore",
        category: "flow",
        access: Access::Write,
        summary: "Restore a Flow page to an earlier history point",
        keywords: &["revert", "undo", "rollback"],
    },
    ToolMeta {
        name: "flow.trash.list",
        category: "flow",
        access: Access::Read,
        summary: "List the archived Flow objects of a workspace or project",
        keywords: &["deleted", "recycle bin"],
    },
    ToolMeta {
        name: "objects.purge",
        category: "flow",
        access: Access::Write,
        summary: "Permanently delete an archived Flow object",
        keywords: &["destroy", "erase", "empty trash"],
    },
    ToolMeta {
        name: "flow.mentions.list",
        category: "flow",
        access: Access::Read,
        summary: "List the people who can be mentioned on a Flow object",
        keywords: &["at mention", "people"],
    },
    ToolMeta {
        name: "sylvode_task_get",
        category: "tasks",
        access: Access::Read,
        summary: "Read a task assigned to the bot with its source, project and status",
        keywords: &["task", "assignment", "work order", "job"],
    },
    ToolMeta {
        name: "sylvode_task_ack",
        category: "tasks",
        access: Access::Write,
        summary: "Take a task and state a short plan",
        keywords: &["acknowledge", "accept task", "plan", "start task"],
    },
    ToolMeta {
        name: "sylvode_task_progress",
        category: "tasks",
        access: Access::Write,
        summary: "Report progress of a task and keep it alive",
        keywords: &["heartbeat", "status update", "milestone", "keepalive"],
    },
    ToolMeta {
        name: "sylvode_task_complete",
        category: "tasks",
        access: Access::Write,
        summary: "Finish a task and let Sylvode post the reply",
        keywords: &["done", "finish task", "submit result", "vote"],
    },
    ToolMeta {
        name: "sylvode_task_fail",
        category: "tasks",
        access: Access::Write,
        summary: "End a task as failed with the reason",
        keywords: &["give up", "cannot do", "error", "abort task"],
    },
];

/// The catalogue entry of a registry tool.
pub fn meta(name: &str) -> Option<&'static ToolMeta> {
    CATALOG.iter().find(|entry| entry.name == name)
}

/// The text hashed into `catalog_sha256`: name, category and access of every entry in registry
/// order. Summaries and keywords are wording and do not enter the hash; a reclassification does.
pub fn classification_text() -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    for entry in CATALOG {
        // Writing into a String cannot fail.
        let _ = writeln!(text, "{}\t{}\t{}", entry.name, entry.category, entry.access.as_str());
    }
    text
}

/// Adds `annotations.readOnlyHint` from the catalogue to every listed tool that has an entry.
pub fn annotate(tools: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
    tools
        .into_iter()
        .map(|mut tool| {
            if let Some(entry) = meta(&tool.name) {
                // Keep the hints a definition carries itself (`idempotentHint` of the task tools).
                let mut annotations = match tool.annotations.take() {
                    Some(Value::Object(map)) => map,
                    _ => serde_json::Map::new(),
                };
                annotations.insert("readOnlyHint".to_string(), json!(entry.access == Access::Read));
                tool.annotations = Some(Value::Object(annotations));
            }
            tool
        })
        .collect()
}

/// The rows of `list-tools --json`: name, access and category of every registry tool.
pub fn list_json() -> Value {
    Value::Array(
        CATALOG
            .iter()
            .map(|entry| json!({ "name": entry.name, "access": entry.access.as_str(), "category": entry.category }))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::{ACCESS_EXCEPTIONS, Access, CATALOG, CATEGORIES, POST_READ_ENDPOINTS, WRITE_BY_JOB_SEMANTICS};
    use crate::server::McpServer;
    use crate::tools::test_support::{Api, Policy, argument_variants};
    use serde_json::json;
    use std::collections::BTreeSet;

    const FROZEN_CATEGORIES: [&str; 21] = [
        "projects",
        "project_types",
        "scenario_templates",
        "context",
        "forms",
        "layouts",
        "plugins",
        "work_items",
        "comments",
        "files",
        "labels",
        "sprints",
        "milestones",
        "proposals",
        "release",
        "code",
        "documents",
        "operations",
        "workspace",
        "flow",
        "tasks",
    ];

    fn registry_names() -> Vec<String> {
        crate::tools::get_all_tool_definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect()
    }

    #[test]
    fn every_registry_tool_has_one_entry() {
        let registry = registry_names();
        let catalog = CATALOG.iter().map(|entry| entry.name).collect::<Vec<_>>();
        let unique = catalog.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(catalog.len(), unique.len(), "a tool has two catalogue entries");
        let registered = registry.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let missing = registered.difference(&unique).collect::<Vec<_>>();
        let stale = unique.difference(&registered).collect::<Vec<_>>();
        assert!(
            missing.is_empty(),
            "registry tools without a catalogue entry: {missing:?}"
        );
        assert!(
            stale.is_empty(),
            "catalogue entries that are not registry tools: {stale:?}"
        );
        assert_eq!(
            registry.iter().map(String::as_str).collect::<Vec<_>>(),
            catalog,
            "the catalogue must list the tools in registry order"
        );
    }

    #[test]
    fn the_category_names_are_the_frozen_twenty_one() {
        let names = CATEGORIES.iter().map(|category| category.name).collect::<Vec<_>>();
        assert_eq!(names, FROZEN_CATEGORIES);
        for category in CATEGORIES {
            assert!(
                !category.title.is_empty() && !category.summary.is_empty(),
                "{}",
                category.name
            );
        }
        for entry in CATALOG {
            assert!(
                names.contains(&entry.category),
                "{} uses {}",
                entry.name,
                entry.category
            );
        }
    }

    #[test]
    fn every_category_is_used() {
        let used = CATALOG.iter().map(|entry| entry.category).collect::<BTreeSet<_>>();
        for category in CATEGORIES {
            assert!(used.contains(category.name), "category {} is unused", category.name);
        }
    }

    #[test]
    fn summaries_are_one_short_line_without_a_trailing_period() {
        for entry in CATALOG {
            let summary = entry.summary;
            assert!(!summary.is_empty(), "{} has no summary", entry.name);
            assert!(summary.chars().count() <= 120, "{} summary is too long", entry.name);
            assert!(!summary.ends_with('.'), "{} summary ends with a period", entry.name);
            assert!(!summary.contains('\n'), "{} summary has a line break", entry.name);
            assert!(!entry.keywords.is_empty(), "{} has no keywords", entry.name);
        }
    }

    const WRITE_VERBS: [&str; 16] = [
        "create", "update", "delete", "set", "install", "invoke", "upload", "commit", "purge", "restore", "archive",
        "revoke", "replay", "compact", "add", "remove",
    ];
    const READ_VERBS: [&str; 6] = ["list", "get", "search", "query", "history", "status"];

    fn last_segment(name: &str) -> &str {
        name.rsplit('.').next().unwrap_or(name)
    }

    fn ends_with_verb(segment: &str, verb: &str) -> bool {
        segment == verb || segment.ends_with(&format!("_{verb}"))
    }

    fn suggests_write(name: &str) -> bool {
        let segment = last_segment(name);
        segment.starts_with("add_")
            || segment.starts_with("remove_")
            || WRITE_VERBS.iter().any(|verb| ends_with_verb(segment, verb))
    }

    fn suggests_read(name: &str) -> bool {
        let segment = last_segment(name);
        READ_VERBS.iter().any(|verb| ends_with_verb(segment, verb))
    }

    #[test]
    fn names_agree_with_access_or_are_listed() {
        let exceptions = ACCESS_EXCEPTIONS.iter().map(|(name, _)| *name).collect::<BTreeSet<_>>();
        for entry in CATALOG {
            let contradicts = match entry.access {
                Access::Read => suggests_write(entry.name),
                Access::Write => suggests_read(entry.name),
            };
            assert_eq!(
                contradicts,
                exceptions.contains(entry.name),
                "{}: {:?} against its name; list it in ACCESS_EXCEPTIONS with a reason, or drop a stale entry",
                entry.name,
                entry.access
            );
        }
        for (name, reason) in ACCESS_EXCEPTIONS {
            assert!(!reason.is_empty(), "{name} has no reason");
        }
    }

    #[test]
    fn the_exception_lists_name_registry_tools_of_the_right_access() {
        for (name, reason) in POST_READ_ENDPOINTS {
            assert!(!reason.is_empty(), "{name} has no endpoint comment");
            assert_eq!(
                super::meta(name).map(|entry| entry.access),
                Some(Access::Read),
                "{name}"
            );
        }
        for (name, reason) in WRITE_BY_JOB_SEMANTICS {
            assert!(!reason.is_empty(), "{name} has no reason");
            assert_eq!(
                super::meta(name).map(|entry| entry.access),
                Some(Access::Write),
                "{name}"
            );
        }
    }

    // ---- HTTP method witness ----

    /// Calls every registry tool against an in-process API mock with every argument shape that can
    /// change what it does (see [`argument_variants`]) and returns, per tool, the requests of all
    /// the calls in registry order.
    async fn requests_per_tool() -> Result<Vec<(String, Vec<(String, String)>)>, Box<dyn std::error::Error>> {
        let api = Api::spawn(Policy::Permissive).await?;
        let server = McpServer::new(api.client()?, platform::config::McpToolMode::Full);
        let mut rows = Vec::new();
        for tool in crate::tools::get_all_tool_definitions() {
            api.clear();
            for arguments in argument_variants(&tool) {
                let _ = server.execute_tool_for_catalog_witness(&tool.name, arguments).await;
            }
            rows.push((
                tool.name,
                api.seen().into_iter().map(|seen| (seen.method, seen.path)).collect(),
            ));
        }
        Ok(rows)
    }

    /// The variants really cover more than the minimal call: tools with several shapes get several.
    #[test]
    fn the_witness_exercises_every_shape_of_the_tools_that_have_several() {
        let by_name = |name: &str| {
            crate::tools::get_all_tool_definitions()
                .into_iter()
                .find(|tool| tool.name == name)
                .map(|tool| argument_variants(&tool))
                .unwrap_or_default()
        };
        let integrity = by_name("objects.integrity");
        assert!(
            integrity.iter().any(|variant| variant.get("object_id").is_some()),
            "{integrity:?}"
        );
        assert!(
            integrity.iter().any(|variant| variant.get("workspace_id").is_some()),
            "{integrity:?}"
        );
        let total = crate::tools::get_all_tool_definitions()
            .iter()
            .map(|tool| argument_variants(tool).len())
            .sum::<usize>();
        assert!(
            total > 2 * CATALOG.len(),
            "{total} variants for {} tools",
            CATALOG.len()
        );
    }

    #[tokio::test]
    async fn read_tools_only_issue_get_requests() -> Result<(), Box<dyn std::error::Error>> {
        let post_reads = POST_READ_ENDPOINTS
            .iter()
            .map(|(name, _)| *name)
            .collect::<BTreeSet<_>>();
        let mut violations = Vec::new();
        for (name, requests) in requests_per_tool().await? {
            let Some(entry) = super::meta(&name) else { continue };
            if entry.access != Access::Read {
                continue;
            }
            if requests.is_empty() {
                violations.push(format!(
                    "read tool {name} issued no request, so the witness proved nothing"
                ));
            }
            let posts = requests
                .iter()
                .filter(|(method, _)| method != "GET")
                .collect::<Vec<_>>();
            if post_reads.contains(name.as_str()) {
                if posts.is_empty() {
                    violations.push(format!(
                        "{name} is listed in POST_READ_ENDPOINTS but issued no non-GET request"
                    ));
                }
            } else if !posts.is_empty() {
                violations.push(format!("read tool {name} issued {posts:?}"));
            }
        }
        assert!(violations.is_empty(), "{violations:#?}");
        Ok(())
    }

    #[tokio::test]
    async fn write_tools_issue_a_non_get_or_are_listed() -> Result<(), Box<dyn std::error::Error>> {
        let listed = WRITE_BY_JOB_SEMANTICS
            .iter()
            .map(|(name, _)| *name)
            .collect::<BTreeSet<_>>();
        let mut violations = Vec::new();
        for (name, requests) in requests_per_tool().await? {
            let Some(entry) = super::meta(&name) else { continue };
            if entry.access != Access::Write {
                continue;
            }
            let writes = requests.iter().any(|(method, _)| method != "GET");
            match (writes, listed.contains(name.as_str())) {
                (false, false) => violations.push(format!("write tool {name} issued no non-GET request: {requests:?}")),
                (true, true) => {
                    violations.push(format!("{name} is listed in WRITE_BY_JOB_SEMANTICS but issues a write"));
                }
                _ => {}
            }
        }
        assert!(violations.is_empty(), "{violations:#?}");
        Ok(())
    }

    // ---- Annotations ----

    #[tokio::test]
    async fn the_full_listing_carries_the_read_only_hint_of_the_catalogue() -> Result<(), Box<dyn std::error::Error>> {
        let server = McpServer::new(
            crate::client::test_api::client("http://127.0.0.1:1".to_string())?,
            platform::config::McpToolMode::Full,
        );
        let request = crate::protocol::JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/list".to_string(),
            params: None,
        };
        let response = server.handle_request(request).await.ok_or("no response")?;
        let listed = response
            .result
            .and_then(|result| result.get("tools").cloned())
            .and_then(|tools| tools.as_array().cloned())
            .ok_or("tools/list returned no array")?;
        assert_eq!(listed.len(), CATALOG.len());
        for tool in &listed {
            let name = tool["name"].as_str().ok_or("tool without name")?;
            let entry = super::meta(name).ok_or("listed tool without entry")?;
            assert_eq!(
                tool.pointer("/annotations/readOnlyHint"),
                Some(&json!(entry.access == Access::Read)),
                "{name}"
            );
        }
        let hint = |wanted: &str| {
            listed
                .iter()
                .find(|tool| tool["name"] == wanted)
                .and_then(|tool| tool.pointer("/annotations/readOnlyHint").cloned())
        };
        assert_eq!(hint("work_items.get"), Some(json!(true)));
        assert_eq!(hint("work_items.update"), Some(json!(false)));
        Ok(())
    }

    #[test]
    fn a_definition_without_annotations_serializes_without_the_field() {
        let tool = crate::protocol::ToolDefinition {
            name: "x".to_string(),
            description: "y".to_string(),
            input_schema: json!({}),
            annotations: None,
        };
        assert!(serde_json::to_value(&tool).is_ok_and(|value| value.get("annotations").is_none()));
    }

    // ---- Runner bot scope ----

    /// Tools whose request is refused to the token of a managed runner bot, and why. A runner bot's
    /// token is limited to the projects where it has an enabled connection
    /// (`api::middleware::scoped_bot`); these tools act on the workspace or on nothing that
    /// belongs to a project, so an agent that works on a task does not use them. Every other tool of
    /// the catalogue must resolve to an allowed route, and every entry here must really be refused:
    /// the test fails for a tool that lost its route and for a stale entry alike.
    const REFUSED_FOR_A_RUNNER_BOT: &[(&str, &str)] = &[
        // Workspace administration.
        ("projects.create", "creates a project"),
        ("projects.delete", "deletes a project"),
        ("members.list", "lists the workspace's members"),
        ("bot_operation_logs.list", "reads the workspace's bot operation log"),
        ("labels.create", "creates a workspace label"),
        ("labels.update", "changes a workspace label"),
        ("labels.delete", "deletes a workspace label"),
        (
            "milestones.list",
            "milestones are workspace-wide; routes refused for scoped and runner bots",
        ),
        (
            "milestones.get",
            "milestones are workspace-wide; routes refused for scoped and runner bots",
        ),
        ("milestones.create", "creates a workspace milestone"),
        ("milestones.update", "changes a workspace milestone"),
        ("milestones.delete", "deletes a workspace milestone"),
        ("project_types.list", "lists the workspace's project types"),
        ("flow.feature_get", "reads the workspace's Flow switch"),
        ("flow.feature_set", "changes the workspace's Flow switch"),
        // Workspace-wide listings: the project-scoped tool is the one to use. `projects.list`,
        // `search.all` and `work_items.search` are not here: a runner bot gets the connected
        // projects only.
        (
            "labels.list",
            "lists the workspace's labels; labels.list_by_project is allowed",
        ),
        ("proposals.list", "lists every proposal of the workspace"),
        // Operational Flow reads. objects.query, objects.search and flow.trash.list are not here: they
        // take a project_id, and a runner bot's request is allowed when it names a connected project.
        ("collab.projection_lag", "reads the workspace's Flow projection lag"),
        // Flow administration, imports and workspace exports.
        ("objects.export_workspace", "exports the whole workspace's Flow"),
        ("objects.import_artifact", "imports into the workspace's Flow"),
        ("objects.import_preview", "imports into the workspace's Flow"),
        ("objects.import_commit", "imports into the workspace's Flow"),
        ("objects.import_status", "reads a workspace Flow import"),
        ("collab.rebuild_projection", "Flow administration"),
        ("collab.compact", "Flow administration"),
        ("collab.repair_quarantine", "Flow administration"),
        ("collab.status", "Flow administration"),
        ("objects.integrity", "Flow administration"),
        ("deliveries.replay", "Flow administration"),
    ];

    fn pattern_matches(pattern: &str, path: &str) -> bool {
        let path = path.split('?').next().unwrap_or(path);
        let (mut a, mut b) = (pattern.split('/'), path.split('/'));
        loop {
            match (a.next(), b.next()) {
                (None, None) => return true,
                (Some(segment), Some(_)) if segment.starts_with("{*") => {
                    let _ = segment;
                    return true;
                }
                (Some(segment), Some(actual)) if segment.starts_with('{') && !actual.is_empty() => {}
                (Some(segment), Some(actual)) if segment == actual => {}
                _ => return false,
            }
        }
    }

    /// The class of the most specific route that a request matches (a literal segment beats a
    /// parameter, and the catch-all comes last), `None` when no route matches.
    fn class_of(method: &str, path: &str) -> Option<api::middleware::scoped_bot::Class> {
        api::catalogue::bot_routes()
            .filter(|route| route.method == method && pattern_matches(route.pattern, path))
            .max_by_key(|route| {
                (
                    !route.pattern.contains("{*"),
                    route
                        .pattern
                        .split('/')
                        .filter(|segment| !segment.starts_with('{'))
                        .count(),
                )
            })
            .map(|route| route.class)
    }

    #[tokio::test]
    async fn every_tool_a_runner_agent_uses_reaches_an_allowed_route() -> Result<(), Box<dyn std::error::Error>> {
        use api::middleware::scoped_bot::Class;
        let intended: BTreeSet<&str> = REFUSED_FOR_A_RUNNER_BOT.iter().map(|(name, _)| *name).collect();
        for (name, reason) in REFUSED_FOR_A_RUNNER_BOT {
            assert!(!reason.is_empty(), "{name} has no reason");
            assert!(
                super::meta(name).is_some(),
                "{name} is not a catalogue tool: drop the stale entry"
            );
        }
        let mut surprises = Vec::new();
        let mut refused_tools = BTreeSet::new();
        for (name, requests) in requests_per_tool().await? {
            if super::meta(&name).is_none() {
                continue;
            }
            for (method, path) in &requests {
                let class = class_of(method, path);
                let Some(class) = class else {
                    surprises.push(format!("{name}: {method} {path} matches no classified route"));
                    continue;
                };
                if !matches!(class, Class::Admin | Class::Refused) {
                    continue;
                }
                refused_tools.insert(name.clone());
                if !intended.contains(name.as_str()) {
                    surprises.push(format!(
                        "{name}: {method} {path} is refused for a runner bot ({class:?})"
                    ));
                }
            }
        }
        for name in &intended {
            if !refused_tools.contains(*name) {
                surprises.push(format!(
                    "{name} is listed as refused but none of its requests is: drop the entry"
                ));
            }
        }
        assert!(surprises.is_empty(), "{surprises:#?}");
        Ok(())
    }

    /// The Flow reads of a project reach a route that a runner bot is allowed on when they name the
    /// project: the route is classified by the query's `project_id`, so the request must carry it,
    /// and it must carry the very project of the call.
    #[tokio::test]
    async fn the_project_form_of_the_flow_reads_reaches_an_allowed_route() -> Result<(), Box<dyn std::error::Error>> {
        use api::catalogue::Find;
        use api::middleware::scoped_bot::Class;
        const PROJECT: &str = "7d1c2f0e-4b6a-4c55-9a3e-0f2d8b1a6c11";
        const WORKSPACE: &str = "3a9f5c42-81de-4e07-b6a1-5c2e90d47f38";
        let api = Api::spawn(Policy::Permissive).await?;
        let server = McpServer::new(api.client()?, platform::config::McpToolMode::Full);
        for (name, arguments) in [
            (
                "objects.query",
                json!({"workspace_id": WORKSPACE, "project_id": PROJECT}),
            ),
            (
                "objects.search",
                json!({"workspace_id": WORKSPACE, "project_id": PROJECT, "q": "plan"}),
            ),
            (
                "flow.trash.list",
                json!({"workspace_id": WORKSPACE, "project_id": PROJECT}),
            ),
        ] {
            api.clear();
            let _ = server.execute_tool_for_catalog_witness(name, arguments).await;
            let seen = api.seen();
            // The workspace resolves to its Flow scope first; the read itself is the second request.
            let [resolution, request] = seen.as_slice() else {
                return Err(format!("{name} issued {seen:?}").into());
            };
            assert_eq!(resolution.method, "GET", "{name}");
            assert_eq!(
                class_of(&resolution.method, &resolution.path),
                Some(Class::NoProject),
                "{name}: {}",
                resolution.path
            );
            assert_eq!(request.method, "GET", "{name}");
            assert_eq!(
                class_of(&request.method, &request.path),
                Some(Class::Project(Find::Query("project_id"))),
                "{name}: {}",
                request.path
            );
            assert!(
                request
                    .query
                    .split('&')
                    .any(|pair| pair == format!("project_id={PROJECT}")),
                "{name}: {}",
                request.query
            );
        }
        Ok(())
    }
}
