# Scenario templates

A scenario template sets up a project for one kind of business work in a single step. Installing a
template into a project creates:

- a project workflow with the template's states (when the project has no workflow yet);
- the template's labels in the workspace (labels that already exist are kept);
- the template's forms, each with a table view and a detail view;
- a governance configuration for the project (see [Governance](governance.md));
- for the restaurant template, the `restaurant_calc` WASM plugin, installed and active.

Templates are built in. Each template also names a project type, which a project created from the
template receives.

## Ways to use a template

### In the web interface

The web interface installs single forms from templates. Open **Forms** in a project and start a new
form: the **Template library** lists the templates whose project type is `custom_form` (every
built-in template except `code_delivery_default`).

- **Apply to draft** fills the new-form draft with the template's headline fields, so you can change
  them before creating the form.
- **Install** creates one form with those fields immediately.

These buttons create one form from the template's headline fields. They do not create the
template's full form set, workflow, labels, governance configuration or plugins. The project list
has no template picker: **Create Project** offers the project types only. To install a whole
template, use the API or MCP.

### With the REST API

Create a project from a template by passing `scenario_template_key` (the project type is taken
from the template when `type_key` is omitted; if both are given they must match):

```bash
curl -sS -X POST "https://sylvode.example.com/api/v1/workspaces/<workspace-id>/projects" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Restaurant Demo",
    "key": "RESTDEMO",
    "scenario_template_key": "restaurant_ordering_default"
  }'
```

Replace the address, the workspace id and `<token>` (a REST bot token or a signed-in user's token)
with your own values.

Install a template into an existing project with
`POST /api/v1/projects/{project_id}/scenario-templates/{template_key}/install`. Workspace owners,
admins and bots may install templates; members may not.

List and read templates with `GET /api/v1/scenario-templates` (filter with `project_type_key` and
`industry`) and `GET /api/v1/scenario-templates/{key}`. See the
[API reference](../integrations/api.md).

### With MCP

| Tool or resource | Use |
|---|---|
| `scenario_templates.list` | List templates. |
| `scenario_templates.get` | Read one template, including its usage guide. |
| `scenario_templates.install` | Install a template into an existing project. |
| `projects.create` | Create a project; pass `scenario_template_key`. |
| `forms.list` | List the forms the template created. |
| `sylvode://scenario-templates`, `sylvode://scenario-templates/{key}` | The same templates as MCP resources. |

Which tools a client can call depends on the MCP server's tool mode and the token; see the
[MCP server](../../apps/mcp-server/README.md).

### The usage guide

Every template returned by the API and MCP carries a `usage_guide` object with `operator_steps`,
`primary_mcp_tools`, `plugin_keys` and `acceptance_focus` for that template, plus
`operator_entrypoints`. One of the entry points, "Frontend project wizard", describes picking a
scenario template card in the project list; the web interface does not have that card (see
[In the web interface](#in-the-web-interface)).

## Built-In Templates

| Template key | Project type | Best fit | Generated universal forms | Integrations |
| --- | --- | --- | --- | --- |
| `code_delivery_default` | `code_project` | AI-assisted software delivery, code review, CI and release evidence | `code_task`, `change_record`, `release_check` | MCP coding and review assistants, business events |
| `contract_review_default` | `custom_form` | Contract intake, legal and commercial risk review, approval chain | `contract`, `risk_clause`, `approval_record` | MCP document assistant, business events |
| `equipment_maintenance_default` | `custom_form` | Fault reports, maintenance ownership, inspection acceptance | `equipment`, `repair_order`, `inspection_record` | MCP inspection assistant, business events |
| `quality_corrective_action_default` | `custom_form` | Defect classification, root-cause analysis, corrective action, recheck | `defect_record`, `root_cause_analysis`, `corrective_action` | MCP quality audit assistant, business events |
| `customer_delivery_default` | `custom_form` | Customer milestones, acceptance materials, risk and change governance | `customer`, `delivery_milestone`, `change_request` | MCP delivery assistant, REST API, business events |
| `restaurant_ordering_default` | `custom_form` | Menu, SKU, tables, orders, print jobs, receipts and daily revenue reporting | `menu_category`, `sku`, `table`, `order`, `order_line`, `print_job`, `business_report` | MCP kitchen assistant, `print_job.created` events, `restaurant_calc` WASM plugin |

## Template details

### Code Delivery

`code_delivery_default` is for software work that needs structured records around AI-assisted
changes. Its forms record code tasks, change records and release checks with CI status and
verification evidence.

- Workflow: Backlog, Ready, In Progress, Review, Release Approval, Done.
- Labels: `bug`, `feature`, `review`, `release-risk`.
- Register repositories and working directories as `directory` project resources (see
  [Project resource kinds](#project-resource-kinds)).

### Contract Review

`contract_review_default` is for document-heavy work: contract master data, risky clauses linked to
the contract, and approval records. Enter amounts as decimal strings. Assistants can summarise
contracts, flag clauses and request approvals while a person records the final decision.

- Workflow: Intake, Extract Terms, Risk Review, Approval, Signed.
- Labels: `legal-risk`, `commercial-risk`, `approval-needed`, `signed`.

### Equipment Maintenance

`equipment_maintenance_default` is for factories, sites and service teams. Its forms track
equipment, repair orders with downtime impact and an attachment, and inspection records with a
result and images. Repair orders and inspection records relate to an equipment record.

- Workflow: Reported, Triage, Repairing, Inspection, Closed.
- Labels: `safety`, `downtime`, `parts-needed`, `inspection-needed`.

### Quality Corrective Action

`quality_corrective_action_default` is for defect handling and corrective and preventive action. It
links a defect record to its root-cause analysis and corrective action. Every change produces
business events, so audit consumers can follow each step.

- Workflow: Detected, Root Cause, Action Plan, Recheck, Closed.
- Labels: `defect`, `root-cause`, `corrective-action`, `recheck`.

### Customer Delivery

`customer_delivery_default` is for service and implementation teams tracking customer milestones,
acceptance status, delivery risk and change requests. Agents send updates through MCP or the REST
API.

- Workflow: Planned, In Delivery, Acceptance, Change Review, Delivered.
- Labels: `milestone`, `acceptance`, `change-request`, `payment-risk`.

### Restaurant Ordering

`restaurant_ordering_default` runs operational data rather than software issues:

1. Create menu categories, SKUs and tables.
2. Create an order and its `order_line` child records.
3. Create `print_job` records for the kitchen and for receipts; each produces a `print_job.created`
   business event that a printer integration reads from the form events API or the MCP
   `events.tail` tool.
4. Store `business_report` records and query revenue with the MCP aggregate tool
   `form_records.aggregate`.

- Workflow: Draft, Sent to Kitchen, Served, Paid, Cancelled.
- Labels: `menu`, `kitchen`, `receipt`, `revenue`.

The template installs the `restaurant_calc` plugin with a formula hook on `order_line.line_total`
and an event handler for `print_job.created`. It is a demonstration module: it returns a fixed
result (a `line_total` of `"19.98"`) instead of calculating from the order line. Replace it with
your own plugin for real totals; see [Plugins](../plugins/README.md).

For a local demonstration with sample data, `scripts/bootstrap-restaurant-demo.sh` creates a demo
workspace and a restaurant project on a local stack; see the [installation guide](../install.md).

## Project resource kinds

Project resources describe the context a project works with. They have exactly two kinds:

| Kind | Use |
|---|---|
| `directory` | A working directory or repository checkout, addressed by a path or URL. |
| `custom` | Any other context resource. |

Creating or updating a resource with any other kind is a validation error. A template's
`resource_schema` suggests resources using only these two kinds. Resources are managed through
`/api/v1/projects/{project_id}/resources`; the web interface has no page for them.
