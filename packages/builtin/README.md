# Built-in packages

Source directories of the built-in packages (publisher `sv`). They are packed and signed by `build.sh`;
the signed `.svpkg` files are build output and are not committed.

| Directory | Package id | Kind | Content |
|---|---|---|---|
| `code-delivery` | `sv/code-delivery` | form-template | code task, change record, release check |
| `contract-review` | `sv/contract-review` | form-template | contract, risk clause, approval record |
| `equipment-maintenance` | `sv/equipment-maintenance` | form-template | equipment, repair order, inspection record |
| `quality-corrective-action` | `sv/quality-corrective-action` | form-template | defect, root cause analysis, corrective action |
| `customer-delivery` | `sv/customer-delivery` | form-template | customer, delivery milestone, change request |
| `restaurant-ordering` | `sv/restaurant-ordering` | form-template | menu, SKU, table, order, order line, print job, business report |
| `service-request` | `sv/service-request` | form-template | one request form with normalise, format and cross-field rules |
| `service-desk` | `sv/service-desk` | app | request form, close action layout, public site page, reference-code plugin |

Build and sign (QA key, created on first use outside the repository):

```bash
packages/builtin/build.sh "$HOME/.sv-qa/sv-builtin.key" "$HOME/.sv-qa/builtin-packages"
```

Then set `[packages] builtin_dir` to the output directory and `[packages] builtin_public_key` to the printed
public key; the API verifies and stores the packages at boot and refuses to start when one does not verify.
Release builds pin the release public key at compile time (`SV_RELEASE_PACKAGE_KEY`) and ignore the setting;
the release key belongs to the owner.

## Project settings

A form-template or app package may carry a `project` block (`type_key`, `workflow_states`, `labels`). It is
applied only when a project is created from the package, in the same transaction as the project; installing the
package into an existing project ignores it. The block never carries governance, agent policy or permissions.
