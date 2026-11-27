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
| 0137 | sites |
| 0138 | installations |
| 0139 | drafts |
| 0140 | navigation (added: `0140_nav.sql`) |
