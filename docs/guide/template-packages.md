# Template packages

A template is a signed built-in package (publisher `sv`) that sets up a project for one kind of business
work. Templates are installed through the same installation flow as any package, so every installation is
recorded and its forms, layouts and site pages are tied to it.

A template package creates:

- its forms (ordinary project forms; they stay when the installation is disabled or revoked);
- its layouts and placements, and its site pages (in a new site with status `draft`: nothing is public until a
  person publishes the site and binds a form to it);
- when a project is created from it, the optional project settings of the package: the project type, a workflow
  with the template's states and the template's labels (labels that already exist are kept).

Templates never write governance configuration, project agent policy, form permissions or roles.

## Built-in packages

| Package | Project type | Best fit | Forms |
| --- | --- | --- | --- |
| `sv/code-delivery` | `code_project` | AI-assisted software delivery, code review, CI and release evidence | `code_task`, `change_record`, `release_check` |
| `sv/contract-review` | `custom_form` | Contract intake, risk review, approval chain | `contract`, `risk_clause`, `approval_record` |
| `sv/equipment-maintenance` | `custom_form` | Fault reports, maintenance ownership, inspection acceptance | `equipment`, `repair_order`, `inspection_record` |
| `sv/quality-corrective-action` | `custom_form` | Defect classification, root cause, corrective action, recheck | `defect_record`, `root_cause_analysis`, `corrective_action` |
| `sv/customer-delivery` | `custom_form` | Customer milestones, acceptance, change governance | `customer`, `delivery_milestone`, `change_request` |
| `sv/restaurant-ordering` | `custom_form` | Menu, tables, orders, print jobs, daily report | `menu_category`, `sku`, `table`, `order`, `order_line`, `print_job`, `business_report` |
| `sv/service-request` | `custom_form` | A request form with normalisation, format and cross-field rules | `service_request` |
| `sv/service-desk` | `custom_form` | Request form, close action, public request page, reference-code plugin (has code: installed through the review) | `desk_requests` |

Owner and approver fields are member fields, phone fields are phone fields and signatures are signature fields.
The sources are in `packages/builtin/`; `packages/builtin/build.sh` packs and signs them (see its README).

## Using a template

Only built-in packages can be used this way, only by a workspace owner or admin who is a person (not a bot, an
OAuth client or an MCP agent), and only packages without code or capability requests. Anything else goes through
**Installations** and its review.

- **Create a project from a template.** In **Create Project** choose a template. The project type, workflow and
  labels come from the package; a `type_key` you pass overrides the package's type. Over the API:

  ```bash
  curl -sS -X POST "https://sylvode.example.com/api/v1/workspaces/<workspace-id>/projects" \
    -H "Authorization: Bearer <token>" -H "Content-Type: application/json" \
    -d '{"name": "Restaurant Demo", "key": "RESTDEMO", "template_package": "sv/restaurant-ordering"}'
  ```

  The project, installation, forms, workflow and labels are created in one transaction; a failure leaves
  nothing behind.
- **Preview.** `GET /api/v1/workspaces/{workspace_id}/project-templates/preview?package_key=sv/restaurant-ordering`
  shows the type, workflow states, labels and forms that creating a project would apply. It writes nothing.
- **Install into an existing project.** In **Forms**, start a new form and use the **Template library**, or call
  `POST /api/v1/projects/{project_id}/template-install` with `{"package_key": "sv/contract-review"}`. The forms
  are created; the package's project settings are not applied to an existing project.

MCP agents create projects from a template with `projects.create` and `template_package`; the project's
admin check is the same.
