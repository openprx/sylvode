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
