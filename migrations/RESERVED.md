# Reserved migration numbers (low-code programme)

`apps/api/build.rs` embeds every `migrations/*.sql` file in name order, so an empty or placeholder `.sql`
file would be applied and recorded. These numbers are therefore reserved here and no file exists for them
until the owning batch adds the real migration. 0128 and 0129 are reserved by an earlier plan.

| Number | Purpose |
|---|---|
| 0133 | catalogue ids and old-plugin containment (landed with L1.1; the grant tables are 0138) |
| 0134 | packages (added: `0134_packages.sql`) |
| 0135 | layouts (added: `0135_layouts.sql`) |
| 0136 | form schema v2 (added: `0136_form_schema_v2.sql`) |
| 0137 | sites (added: `0137_sites.sql`) |
| 0138 | installations (added: `0138_installations.sql`) |
| 0139 | drafts (added: `0139_artefact_drafts.sql`) |
| 0140 | navigation (added: `0140_nav.sql`) |
| 0141 | drop `scenario_templates` (added: `0141_drop_scenario_templates.sql`) |
- 0142 cap_record_titles (landed)
- 0143 change_hints_narrow (landed: work_items hint trigger WHEN, lock_timeout 2s)
- 0144 drop_project_type_default_connectors (landed)
- 0145 site_published_at (landed: `sites.published_at`)
- 0146 drop_flow_capability_rollouts (landed; 0126 is an existing file)
- 0147 installation_site (`installations.site_id`, the site holding an installation's pages)
- 0148 change_hints_work_item_insert (work_items AFTER INSERT hint trigger)
