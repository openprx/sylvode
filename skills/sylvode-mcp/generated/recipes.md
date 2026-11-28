# Common workflows

## Fix an assigned issue

1. `sylvode_task_get`, then `sylvode_task_ack` with your plan.
2. `sylvode_read` `comments.list` with `work_item_id` for the discussion. It returns the newest 20 comments first; read further with `page` (and `per_page`, up to 100) while `total_pages` says there are more, so the first comment you read is the latest one.
3. Change the code. Run the project's tests.
4. `sylvode_task_progress` after the tests pass.
5. `sylvode_task_complete` with `outcome: "done"`, the summary and a `commit` or `branch` artifact.

## Answer a mention

1. `sylvode_task_get`. The comment that mentions you is in `source.comment.body`.
2. `sylvode_task_ack` with one line, for example "Answer the question about X".
3. Read what you need, for example `sylvode_read` `work_items.get` or `comments.list`.
4. `sylvode_task_complete` with `outcome: "done"` and the answer as `summary`. Sylvode posts it as a reply.

## Vote on a proposal

1. `sylvode_task_get`, then `sylvode_read` `proposals.get` with the proposal id from `source.proposal.id`.
2. `sylvode_task_ack` with the criteria you will apply.
3. `sylvode_task_complete` with `outcome: "done"`, `vote` (`yes`, `no` or `abstain`), `vote_reason` and a short `summary`.

## Find and change data outside a task

1. `sylvode_search {"query": "<what you want>"}`.
2. Read with `sylvode_read`. Change with `sylvode_write`, using the schema from the search result.
3. Do not use the task tools when you have no task id.

## Propose a form schema or a layout

A form schema or a layout you write is a draft. A person who owns or administers the workspace reviews and approves it in the console (project settings, Drafts); you cannot approve, and nothing is live before that.

1. For a form: read the target with `forms.get` (an existing form) and write the full schema (`sv.form.schema.v2`). For a layout: write the spec (`sv.layout.v1`) for a slot, for example "form.record.detail", at a scope (`workspace`, `project` or `form`).
2. Check it first: `forms.validate_schema` or `layouts.validate`. The answer lists every error with a JSON Pointer `path` and a `code`. Fix them and check again until `valid` is true.
3. Send it: `forms.create_schema_draft` (`form_id` for an existing form, `key` and `name` for a new one) or `layouts.create_draft`. The answer is the stored draft. Read `valid` and `errors` in the draft's `validation`: an invalid draft is kept with its report but cannot be approved, so send a corrected one. A newer draft of yours for the same target replaces your older pending one.
4. A draft for an existing form records the schema version it was written against. If the form changed before approval the draft is refused as stale: read the form again and send a new draft.
5. Say in your summary which draft you created and that it waits for review. Do not retry an approval you cannot make.

## Build a plugin package and submit it

A plugin package (`sv.package.v1`) bundles a manifest, WASM components, layouts, forms and site pages. You build and sign it yourself, on your own machine, with your own key. Sylvode has no builder service; it verifies what you upload and a person decides. You need a Sylvode repository checkout (for `sv-pack`, the WIT file and the sample), a Rust toolchain with the `wasm32-unknown-unknown` target and `wasm-tools`.

1. Ask a person who owns the workspace to register your public key (console, publisher keys; publisher id and key). Create it once: `cargo run -q -p sv-package --bin sv-pack -- keygen <key-file> <public-key-file>`. The key file is secret: keep it mode 0600 and never paste or upload it. Only the public key goes to the person. Packages signed by any other key are refused.
2. Lay out a directory:
   - `sv-package.json`: the manifest.
   - `contents/components/<name>.wasm`: WASM components built against `wit/sv-plugin.wit` (package `sv:plugin@4.0.0`). World `action` exports `run(action, input)` and may call `records`, `catalogue`, `log` and `clock`; world `field-rule` exports `apply(input)`.
   - `contents/layouts/<key>.json`: layouts (`sv.layout.v1`); check each with `layouts.validate`.
   - `contents/forms/<name>.json`: form schemas (`sv.form.schema.v2`) that a bind creates a form from; check with `forms.validate_schema`.
3. Write the manifest. Required: `schema` ("sv.package.v1"), `kind` (`form-template`, `page-template`, `app` or `plugin`), `id` ("<publisher>/<name>"), `version` (semver), `publisher` (`id` and a placeholder `key_id`, which the packer sets), `sv` (`min`, `layout`, `form_schema`), `summary` (i18n map with `en`), `license` (SPDX) and `files` ({}; the packer fills it). With components also `wit` ("sv:plugin@4.0.0"). Optional: `binds` (`{name: {kind: "form", create_from, requires_fields}}`), `requests` (the capabilities the package asks for: `{id, form: "$bind.<name>"}`, explicit ids only), `actions` (`{name, world: "action", component, label}`), `rules` (`{name, world: "field-rule", component, stages, field_types}`), `messages`, `layouts` (`{key, path, slots}`), `pages` (`{route, layout, audience, nav}`), `depends`. The manifest has no `custom_code` field; the server computes it. Copy the working example `examples/plugins/service-desk-package/` (its build script shows the whole build) and change it.
4. Build the components (the script in `examples/plugins/` is the reference build), then pack and sign: `cargo run -q -p sv-package --bin sv-pack -- pack <dir> <key-file> <out.svpkg>`. Check it: `sv-pack inspect <out.svpkg>`. The limit is 32 MiB.
5. Submit with `packages.upload` and `package_base64` (standard base64 of the `.svpkg`). It needs a bot of the whole workspace with write permission. The server verifies the signature against the registered keys and the layouts; it refuses with a `package_invalid` reason (`signature_invalid` with cause `key_unknown` or `key_revoked`, `manifest_invalid`, `layout_invalid`, `too_large`, and so on): fix and submit again. On success the answer is a `package_upload` draft that waits; nothing is stored as a package yet. A newer upload of yours with the same id and version replaces your older pending one, at most 3 of your uploads and 10 of the workspace wait, a pending upload expires after 7 days, and one upload of yours runs at a time. Over HTTP the server takes at most two requests above 2 MiB at once: on HTTP 429 wait and send again.
6. A person who owns or administers the workspace approves the draft in the console (Drafts). Approval stores the package. Installing it is a separate review by a person: tell them the package id and version, and what it requests. Do not try to approve or install it yourself, and do not retry a refusal you cannot resolve.
