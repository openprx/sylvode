# Reserved migration numbers (low-code programme)

`apps/api/build.rs` embeds every `migrations/*.sql` file in name order, so an empty or placeholder `.sql`
file would be applied and recorded. These numbers are therefore reserved here and no file exists for them
until the owning batch adds the real migration. 0128 and 0129 are reserved by an earlier plan.

| Number | Purpose |
|---|---|
| 0133 | catalogue and grants |
| 0134 | packages |
| 0135 | layouts |
| 0136 | form schema v2 |
| 0137 | sites |
| 0138 | installations |
| 0139 | drafts |
