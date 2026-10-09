/**
 * Flow Page -> Forms record conversion wizard (`/workspace/{id}/flow/{objectId}/convert`;
 * `contracts/rest-api-v1.md` v0.7 Forms Bridge table, task FP-N4).
 *
 * Checks the real request shapes through `FlowObjectRepository.get` and
 * `FlowCommandService.convertPreview/convertCommit` against a mocked `globalThis.fetch`, then
 * drives `ConvertWizard` with injected fakes: steps cannot be skipped, the mapping is the
 * backend's `form_record` shape, commit copies its three preview-derived fields from the preview
 * response, the commit key is fixed per `preview_id`, an expired preview cannot be committed,
 * `stale_frontier` (by `error_code` only) re-reads the source and previews under a new key, and
 * non-Page objects cannot enter.
 *
 * Run standalone: `bun tests/flow-convert-wizard.test.ts`
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import {
	Suite,
	assert,
	assertDeepEqual,
	assertEqual,
	assertNotEqual,
	finish
} from './support/harness';
import {
	ConvertWizard,
	FAILURE_KEYS,
	FIELD_PROBLEM_KEYS,
	PERMISSION_ACCESS_KEYS,
	PERMISSION_ACTION_KEYS,
	PERMISSION_CONFIGURATION_KEYS,
	PERMISSION_LIMIT_KEYS,
	STEP_KEYS,
	buildRecordMapping,
	classifyConvertFailure,
	describeFormFields,
	permissionExplanation,
	type ConvertWizardDeps
} from '../src/lib/flow/convert-wizard';
import { FlowCommandService } from '../src/lib/flow/command-service';
import { FlowObjectRepository } from '../src/lib/flow/object-repository';
import type { ApiResult, PaginatedData } from '../src/lib/api/client';
import type {
	BridgePermissionState,
	FlowConversionCommitInput,
	FlowConversionJob,
	FlowConversionPreview,
	FlowConversionPreviewInput,
	FlowObjectView
} from '../src/lib/api/flow';
import type { UniversalForm } from '../src/lib/api/forms';

const suite = new Suite('flow-convert-wizard');

const WS = '11111111-1111-4111-8111-111111111111';
const OBJ = '22222222-2222-4222-8222-222222222222';
const PROJECT = '33333333-3333-4333-8333-333333333333';
const FORM = '44444444-4444-4444-8444-444444444444';
const NOW = Date.parse('2026-10-09T12:00:00Z');
const MINUTE = 60_000;

function ok<T>(data: T): ApiResult<T> {
	return { code: 0, message: 'ok', data };
}

function err(code: number, extra: Partial<ApiResult<never>> = {}): ApiResult<never> {
	return { code, message: 'everything is fine', data: null, ...extra };
}

function object(overrides: Partial<FlowObjectView> = {}): FlowObjectView {
	return {
		id: OBJ,
		workspace_id: WS,
		project_id: null,
		object_type: 'page',
		lifecycle_status: 'active',
		governance_metadata: {},
		title: 'Quarterly plan',
		semantic_content: null,
		document_id: 'doc-1',
		document_seq: 4,
		frontier: 'F-local',
		projection_seq: 4,
		projection_lag: 0,
		created_at: '2026-10-09T11:00:00Z',
		updated_at: '2026-10-09T11:30:00Z',
		archived_at: null,
		...overrides
	};
}

const FORM_ROW: UniversalForm = {
	id: FORM,
	workspace_id: WS,
	project_id: PROJECT,
	key: 'plans',
	name: 'Plans',
	description: '',
	title_template: '',
	schema: {
		version: 'openpr.form.schema.v1',
		fields: [
			{ key: 'ticket', type: 'autonumber', required: true },
			{ key: 'owner_note', label: 'Owner note', type: 'text', required: true },
			{ key: 'budget', type: 'number' },
			{ key: 'headcount', type: 'integer' },
			{ key: 'approved', type: 'boolean' },
			{ key: 'stage', type: 'single_select', options: ['draft', 'final'] },
			{ key: 'tags', type: 'multi_select', options: ['a', 'b', 'c'] },
			{ key: 'files', type: 'attachment' }
		]
	},
	detail_layout: {},
	schema_version: 7,
	created_at: '2026-10-01T00:00:00Z',
	updated_at: '2026-10-01T00:00:00Z'
};

const CONTROLLED: BridgePermissionState = {
	access: 'controlled',
	configuration: 'explicit',
	actions: ['form.view', 'record.create'],
	field_read_limited: false,
	field_write_limited: false,
	record_limited: false
};

function previewResponse(overrides: Partial<FlowConversionPreview> = {}): FlowConversionPreview {
	return {
		preview_id: 'preview-1',
		expires_at: new Date(NOW + 15 * MINUTE).toISOString(),
		// Deliberately different from the locally read frontier, so a commit that sends the local
		// value is caught.
		source_frontier: 'F-server',
		target_schema_version: 7,
		mapping: { target_form_id: FORM, title: 'Quarterly plan', values: { owner_note: 'x' } },
		warnings: [],
		permission_decision: CONTROLLED,
		estimated_objects: 1,
		...overrides
	};
}

function job(id = 'job-1'): FlowConversionJob {
	return {
		job_id: id,
		status: 'completed',
		source_object_id: OBJ,
		source_frontier: 'F-server',
		target_schema_version: 7,
		lineage_id: 'lineage-1',
		created_target_ids: ['record-1'],
		warnings: [],
		error: null
	};
}

function keys(): () => string {
	let n = 0;
	return () => `key-${++n}`;
}

interface Harness {
	wizard: ConvertWizard;
	gets: string[];
	previews: FlowConversionPreviewInput[];
	commits: FlowConversionCommitInput[];
	clock: { now: number };
}

function harness(
	options: {
		objects?: Array<ApiResult<FlowObjectView>>;
		previews?: Array<ApiResult<FlowConversionPreview>>;
		commits?: Array<ApiResult<FlowConversionJob>>;
	} = {}
): Harness {
	const objects = options.objects ?? [ok(object())];
	const previewReplies = options.previews ?? [];
	const commitReplies = options.commits ?? [];
	const gets: string[] = [];
	const previews: FlowConversionPreviewInput[] = [];
	const commits: FlowConversionCommitInput[] = [];
	const clock = { now: NOW };
	const deps: Partial<ConvertWizardDeps> & Pick<ConvertWizardDeps, 'repository' | 'commands'> = {
		repository: {
			get: (id: string) => {
				gets.push(id);
				const next = objects.length > 1 ? objects.shift() : objects[0];
				if (!next) throw new Error('unexpected get');
				return Promise.resolve(next);
			}
		},
		commands: {
			convertPreview: (input: FlowConversionPreviewInput) => {
				previews.push(JSON.parse(JSON.stringify(input)) as FlowConversionPreviewInput);
				const next = previewReplies.shift();
				if (!next) throw new Error('unexpected preview call');
				return Promise.resolve(next);
			},
			convertCommit: (input: FlowConversionCommitInput) => {
				commits.push({ ...input });
				const next = commitReplies.shift();
				if (!next) throw new Error('unexpected commit call');
				return Promise.resolve(next);
			}
		},
		listProjects: () => Promise.resolve([{ id: PROJECT, label: 'PLN - Planning' }]),
		listForms: () =>
			Promise.resolve(
				ok<PaginatedData<UniversalForm>>({
					items: [FORM_ROW],
					total: 1,
					page: 1,
					per_page: 100,
					total_pages: 1
				})
			),
		newKey: keys(),
		now: () => clock.now
	};
	return { wizard: new ConvertWizard(WS, OBJ, deps), gets, previews, commits, clock };
}

/** Loads the source, picks the project and form and fills the one required field. */
async function toMapping(h: Harness): Promise<void> {
	await h.wizard.load();
	assert(await h.wizard.advance(), 'source -> mapping');
	await h.wizard.selectProject(PROJECT);
	h.wizard.selectForm(FORM);
	h.wizard.setValue('owner_note', 'x');
}

async function toCommit(h: Harness): Promise<void> {
	await toMapping(h);
	assert(await h.wizard.advance(), 'mapping -> preview');
	assertEqual(h.wizard.snapshot().previewState, 'ready', 'preview ready');
	assert(await h.wizard.advance(), 'preview -> commit');
	assertEqual(h.wizard.step, 'commit', 'on commit');
}

// ---- 1. wire shapes through the real adapters -------------------------------------------------

await suite.checkAsync(
	'repository.get and CommandService convert calls hit the contract paths with exact bodies',
	async () => {
		const original = globalThis.fetch;
		const calls: Array<{ method: string; path: string; body: unknown }> = [];
		globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
			const raw = String(input);
			calls.push({
				method: init?.method ?? 'GET',
				path: raw.startsWith('http') ? new URL(raw).pathname : raw,
				body: init?.body ? JSON.parse(String(init.body)) : null
			});
			return new Response(JSON.stringify({ code: 0, message: 'ok', data: {} }), {
				headers: { 'Content-Type': 'application/json' }
			});
		}) as typeof fetch;
		try {
			await new FlowObjectRepository().get(OBJ);
			const commands = new FlowCommandService();
			await commands.convertPreview({
				source_object_id: OBJ,
				source_frontier: 'F',
				target_type: 'form_record',
				mapping: { target_form_id: FORM, title: 'T', values: { a: 1 } },
				idempotency_key: 'pk'
			});
			await commands.convertCommit({
				preview_id: 'p',
				source_frontier: 'F',
				target_schema_version: 3,
				idempotency_key: 'ck',
				confirm: true
			});
		} finally {
			globalThis.fetch = original;
		}
		assertDeepEqual(
			calls.map(({ method, path }) => [method, path]),
			[
				['GET', `/api/v1/flow/objects/${OBJ}`],
				['POST', '/api/v1/flow/conversions/preview'],
				['POST', '/api/v1/flow/conversions']
			],
			'paths'
		);
		assertDeepEqual(
			calls[1].body,
			{
				source_object_id: OBJ,
				source_frontier: 'F',
				target_type: 'form_record',
				mapping: { target_form_id: FORM, title: 'T', values: { a: 1 } },
				idempotency_key: 'pk'
			},
			'preview body'
		);
		assertDeepEqual(
			calls[2].body,
			{
				preview_id: 'p',
				source_frontier: 'F',
				target_schema_version: 3,
				idempotency_key: 'ck',
				confirm: true
			},
			'commit body'
		);
	}
);

// ---- 2. mapping shape ---------------------------------------------------------------------------

suite.check('mapping is {target_form_id, title?, values} with per-type coercion', () => {
	const fields = describeFormFields(FORM_ROW);
	assertDeepEqual(
		fields.map((field) => [field.key, field.disposition.kind]),
		[
			['ticket', 'server'],
			['owner_note', 'input'],
			['budget', 'input'],
			['headcount', 'input'],
			['approved', 'input'],
			['stage', 'input'],
			['tags', 'input'],
			['files', 'unsupported']
		],
		'dispositions'
	);
	assertEqual(fields[1].label, 'Owner note', 'label');
	const built = buildRecordMapping(FORM, '  Plan  ', fields, {
		owner_note: ' hello ',
		budget: '12.50',
		headcount: '7',
		approved: 'false',
		stage: 'final',
		tags: ['a', 'c']
	});
	assertDeepEqual(built.problems, {}, 'no problems');
	assertDeepEqual(
		built.mapping,
		{
			target_form_id: FORM,
			title: 'Plan',
			values: {
				owner_note: 'hello',
				// `normalize_decimal_like` rejects a JSON number: decimals travel as strings.
				budget: '12.50',
				headcount: 7,
				approved: false,
				stage: 'final',
				tags: ['a', 'c']
			}
		},
		'mapping'
	);
	const empty = buildRecordMapping(FORM, '   ', fields, { owner_note: 'x', budget: '', tags: [] });
	assertDeepEqual(empty.mapping, { target_form_id: FORM, values: { owner_note: 'x' } }, 'unset');
	const bad = buildRecordMapping(FORM, 't', fields, {
		budget: '1e3',
		headcount: '7.5',
		stage: 'other',
		tags: ['z']
	});
	assertDeepEqual(
		bad.problems,
		{
			owner_note: 'required',
			budget: 'invalid_decimal',
			headcount: 'invalid_integer',
			stage: 'invalid_option',
			tags: 'invalid_option'
		},
		'problems'
	);
	const unmappable = buildRecordMapping(
		FORM,
		't',
		describeFormFields({
			schema: {
				fields: [
					{ key: 'sig', type: 'signature', required: true },
					{ key: 'rows', type: 'child_table', required: true }
				]
			}
		}),
		{}
	);
	assertDeepEqual(unmappable.problems, { sig: 'unmappable_required' }, 'child_table exempt');
});

// ---- 3. step gating -----------------------------------------------------------------------------

await suite.checkAsync('steps cannot be skipped and nothing is sent early', async () => {
	const h = harness();
	assertEqual(h.wizard.canAdvance('source'), false, 'before load');
	await h.wizard.load();
	assertEqual(h.gets.length, 1, 'one GET');
	assertEqual(h.wizard.snapshot().title, 'Quarterly plan', 'title defaults to the Page title');
	assertEqual(h.wizard.goTo('preview'), false, 'no jump to preview');
	assertEqual(h.wizard.goTo('commit'), false, 'no jump to commit');
	assert(await h.wizard.advance(), 'source -> mapping');
	assertEqual(h.wizard.canAdvance('mapping'), false, 'no form chosen');
	assertEqual(await h.wizard.advance(), false, 'cannot leave mapping without a form');
	await h.wizard.selectProject(PROJECT);
	h.wizard.selectForm(FORM);
	assertEqual(h.wizard.snapshot().problems.owner_note, 'required', 'required problem');
	assertEqual(await h.wizard.advance(), false, 'required field blocks');
	assertEqual(await h.wizard.runPreview(), false, 'runPreview refused outside preview step');
	assertEqual(h.previews.length, 0, 'no preview sent');
	assertEqual(h.wizard.goTo('commit'), false, 'still no jump');
	h.wizard.setValue('owner_note', 'x');
	assert(h.wizard.canAdvance('mapping'), 'mapping complete');
	assertEqual(h.wizard.canAdvance('preview'), false, 'preview not done');
	assertEqual((await h.wizard.commit()).status, 'refused', 'commit refused before preview');
	assertEqual(h.commits.length, 0, 'no commit sent');
});

await suite.checkAsync('a non-Page object cannot enter the wizard', async () => {
	const h = harness({ objects: [ok(object({ object_type: 'navigator' }))] });
	assertEqual(await h.wizard.load(), 'unsupported', 'unsupported');
	assertEqual(h.wizard.canAdvance('source'), false, 'cannot leave source');
	assertEqual(await h.wizard.advance(), false, 'advance refused');
	assertEqual(h.wizard.snapshot().source, null, 'no source exposed');
	assertEqual(h.previews.length, 0, 'nothing sent');
});

await suite.checkAsync('a hidden or missing source is the same not-found state', async () => {
	for (const reply of [
		err(404),
		err(403),
		err(400, { error_code: 'not_found' }),
		err(400, { error_code: 'forbidden' })
	]) {
		const h = harness({ objects: [reply] });
		assertEqual(await h.wizard.load(), 'not_found', `code ${reply.code} ${reply.error_code}`);
		assertEqual(h.wizard.canAdvance('source'), false, 'blocked');
	}
	const h = harness({ objects: [err(500, { message: 'not_found' })] });
	assertEqual(await h.wizard.load(), 'failed', '500 is a failure, message ignored');
});

// ---- 4. preview request and commit fields from the preview --------------------------------------

await suite.checkAsync(
	'preview sends the source frontier and mapping; commit copies preview_id, source_frontier and target_schema_version from the preview response',
	async () => {
		const h = harness({ previews: [ok(previewResponse())], commits: [ok(job())] });
		await toCommit(h);
		assertDeepEqual(
			h.previews,
			[
				{
					source_object_id: OBJ,
					source_frontier: 'F-local',
					target_type: 'form_record',
					mapping: { target_form_id: FORM, title: 'Quarterly plan', values: { owner_note: 'x' } },
					idempotency_key: 'key-1'
				}
			],
			'preview body'
		);
		const outcome = await h.wizard.commit();
		assertEqual(outcome.status, 'refused', 'refused before acknowledgement');
		assertEqual(h.commits.length, 0, 'nothing sent before acknowledgement');
		assertEqual(h.wizard.snapshot().canCommit, false, 'snapshot canCommit false');
		h.wizard.acknowledge(true);
		assertEqual(h.wizard.snapshot().canCommit, true, 'snapshot canCommit follows ack');
		const committed = await h.wizard.commit();
		assertEqual(committed.status, 'committed', 'committed');
		assertDeepEqual(
			h.commits,
			[
				{
					preview_id: 'preview-1',
					source_frontier: 'F-server',
					target_schema_version: 7,
					idempotency_key: 'key-2',
					confirm: true
				}
			],
			'commit body'
		);
		assertEqual(h.wizard.snapshot().jobId, 'job-1', 'job id');
		assertEqual(h.wizard.canCommit(), false, 'no second commit');
		assertEqual((await h.wizard.commit()).status, 'refused', 'second commit refused');
		assertEqual(h.commits.length, 1, 'one commit');
	}
);

await suite.checkAsync('the frozen preview is not affected by later local edits', async () => {
	const response = previewResponse();
	const h = harness({ previews: [ok(response)] });
	await toMapping(h);
	await h.wizard.advance();
	response.source_frontier = 'mutated';
	response.target_schema_version = 99;
	assertEqual(h.wizard.snapshot().preview?.source_frontier, 'F-server', 'frozen copy');
	h.wizard.goTo('mapping');
	h.wizard.setValue('owner_note', 'changed');
	assertEqual(h.wizard.snapshot().preview, null, 'mapping edit discards the preview');
	assertEqual(h.wizard.snapshot().acknowledged, false, 'and the acknowledgement');
});

// ---- 5. idempotency keys ------------------------------------------------------------------------

await suite.checkAsync(
	'commit key is fixed per preview_id and reused by retries; a new preview gets a new commit key',
	async () => {
		const h = harness({
			previews: [ok(previewResponse()), ok(previewResponse({ preview_id: 'preview-2' }))],
			commits: [err(503), err(502), ok(job())]
		});
		await toCommit(h);
		h.wizard.acknowledge(true);
		const first = await h.wizard.commit();
		assertEqual(first.status, 'failed', 'first failed');
		assert(first.status === 'failed' && first.failure.retryable, '503 retryable');
		assertEqual(h.wizard.canCommit(), true, 'retry allowed');
		await h.wizard.commit();
		assertDeepEqual(
			h.commits.map((c) => c.idempotency_key),
			['key-2', 'key-2'],
			'retry reuses the commit key'
		);
		// Back to mapping, change a value, preview again: a different preview, a different key.
		assert(h.wizard.goTo('mapping'), 'back to mapping');
		h.wizard.setValue('owner_note', 'y');
		await h.wizard.advance();
		await h.wizard.advance();
		h.wizard.acknowledge(true);
		await h.wizard.commit();
		assertDeepEqual(
			h.previews.map((p) => p.idempotency_key),
			['key-1', 'key-3'],
			'second preview new key'
		);
		assertDeepEqual(
			h.commits.map((c) => [c.preview_id, c.idempotency_key]),
			[
				['preview-1', 'key-2'],
				['preview-1', 'key-2'],
				['preview-2', 'key-4']
			],
			'commit key per preview'
		);
	}
);

await suite.checkAsync(
	'a preview retry after a 5xx reuses its key; a changed mapping does not',
	async () => {
		const h = harness({ previews: [err(500), ok(previewResponse())] });
		await toMapping(h);
		await h.wizard.advance();
		const failed = h.wizard.snapshot();
		assertEqual(failed.previewState, 'failed', 'failed');
		assertEqual(failed.previewFailure?.retryable, true, 'retryable');
		assertEqual(h.wizard.canAdvance('preview'), false, 'blocked');
		await h.wizard.runPreview();
		assertDeepEqual(
			h.previews.map((p) => p.idempotency_key),
			['key-1', 'key-1'],
			'same key on retry'
		);
		const h2 = harness({ previews: [err(500), ok(previewResponse())] });
		await toMapping(h2);
		await h2.wizard.advance();
		h2.wizard.goTo('mapping');
		h2.wizard.setValue('owner_note', 'other');
		await h2.wizard.advance();
		assertDeepEqual(
			h2.previews.map((p) => p.idempotency_key),
			['key-1', 'key-2'],
			'new key for a new body'
		);
	}
);

// ---- 6. expiry ----------------------------------------------------------------------------------

await suite.checkAsync(
	'an expired preview cannot be committed and a re-preview uses a new key',
	async () => {
		const h = harness({
			previews: [
				ok(previewResponse()),
				ok(
					previewResponse({
						preview_id: 'preview-2',
						expires_at: new Date(NOW + 30 * MINUTE).toISOString()
					})
				)
			],
			commits: [ok(job())]
		});
		await toCommit(h);
		h.wizard.acknowledge(true);
		h.clock.now = NOW + 15 * MINUTE;
		assertEqual(h.wizard.isExpired(), true, 'expired at expires_at');
		assertEqual(h.wizard.canAdvance('preview'), false, 'canAdvance(preview) false');
		assertEqual(h.wizard.canCommit(), false, 'canCommit false');
		assertEqual(h.wizard.snapshot().canCommit, false, 'snapshot canCommit false');
		assertEqual((await h.wizard.commit()).status, 'refused', 'refused');
		assertEqual(h.commits.length, 0, 'no request');
		assert(h.wizard.goTo('preview'), 'back to preview');
		await h.wizard.runPreview();
		assertDeepEqual(
			h.previews.map((p) => p.idempotency_key),
			['key-1', 'key-2'],
			're-preview uses a new key (no commit key was minted for the refused commit)'
		);
		h.clock.now = NOW + 16 * MINUTE;
		assertEqual(h.wizard.isExpired(), false, 'new preview not expired');
	}
);

// ---- 7. error_code branching --------------------------------------------------------------------

await suite.checkAsync(
	'stale_frontier at preview (error_code only) re-reads the source and previews with a new key',
	async () => {
		const h = harness({
			objects: [ok(object()), ok(object({ frontier: 'F-new', document_seq: 5 }))],
			previews: [
				err(409, {
					error_code: 'stale_frontier',
					message: 'all good, continue',
					details: { current_seq: 5 }
				}),
				ok(previewResponse({ source_frontier: 'F-new' }))
			]
		});
		await toMapping(h);
		await h.wizard.advance();
		const snap = h.wizard.snapshot();
		assertEqual(snap.previewFailure?.kind, 'stale_frontier', 'stale kind');
		assertEqual(snap.previewFailure?.messageKey, 'flow.error.stale_frontier', 'stale key');
		assertEqual(h.wizard.canAdvance('preview'), false, 'next disabled');
		assert(await h.wizard.refreshSourceAndPreview(), 'refresh + preview');
		assertEqual(h.gets.length, 2, 'source read again');
		assertDeepEqual(
			h.previews.map((p) => [p.idempotency_key, p.source_frontier]),
			[
				['key-1', 'F-local'],
				['key-2', 'F-new']
			],
			'new key, new frontier'
		);
		assertEqual(h.wizard.snapshot().source?.document_seq, 5, 'source updated');
		assert(h.wizard.canAdvance('preview'), 'can advance now');
	}
);

await suite.checkAsync(
	'stale_frontier at commit returns to a fresh preview under new keys',
	async () => {
		const h = harness({
			objects: [ok(object()), ok(object({ frontier: 'F-new' }))],
			previews: [
				ok(previewResponse()),
				ok(previewResponse({ preview_id: 'preview-2', source_frontier: 'F-new' }))
			],
			commits: [err(409, { error_code: 'stale_frontier' }), ok(job())]
		});
		await toCommit(h);
		h.wizard.acknowledge(true);
		const failed = await h.wizard.commit();
		assert(failed.status === 'failed' && failed.failure.kind === 'stale_frontier', 'stale');
		assertEqual(h.wizard.canCommit(), false, 'stale blocks commit');
		assert(await h.wizard.refreshSourceAndPreview(), 'refresh');
		assertEqual(h.wizard.step, 'preview', 'back on preview');
		await h.wizard.advance();
		assertEqual(h.wizard.snapshot().acknowledged, false, 'acknowledgement reset');
		h.wizard.acknowledge(true);
		await h.wizard.commit();
		assertDeepEqual(
			h.commits.map((c) => [c.preview_id, c.source_frontier, c.idempotency_key]),
			[
				['preview-1', 'F-server', 'key-2'],
				['preview-2', 'F-new', 'key-4']
			],
			'commits'
		);
	}
);

await suite.checkAsync('message text never selects a branch', async () => {
	const h = harness({ previews: [err(409, { message: 'stale_frontier' })] });
	await toMapping(h);
	await h.wizard.advance();
	assertEqual(h.wizard.snapshot().previewFailure?.kind, 'conflict', 'legacy 409 is a conflict');
	assertEqual(await h.wizard.refreshSourceAndPreview(), false, 'no stale recovery offered');
	assertEqual(h.gets.length, 1, 'no re-read');
	assertEqual(
		classifyConvertFailure({ code: 500, error_code: 'forbidden' }).kind,
		'forbidden',
		'error_code beats code'
	);
	assertEqual(
		classifyConvertFailure({ code: 403, error_code: undefined }).kind,
		'forbidden',
		'legacy 403'
	);
	assertEqual(classifyConvertFailure({ code: 404 }).kind, 'not_found', 'legacy 404');
	assertEqual(classifyConvertFailure({ code: 400 }).kind, 'rejected', 'legacy 400');
	assertEqual(classifyConvertFailure({ code: 503 }).retryable, true, '5xx retryable');
	const unknown = classifyConvertFailure({ code: 409, error_code: 'brand_new_code' });
	assertEqual(unknown.kind, 'rejected', 'unknown typed code');
	assertEqual(unknown.retryable, false, 'unknown typed code not retryable');
	const drain = classifyConvertFailure({
		code: 409,
		error_code: 'server_draining',
		details: { reason: 'contention' }
	});
	assertEqual(drain.messageKey, 'flow.error.server_draining.contention', 'drain reason key');
});

await suite.checkAsync(
	'policy_rejected blocks the step and explains the returned permission state',
	async () => {
		const state: BridgePermissionState = {
			access: 'read_only',
			configuration: 'unconfigured',
			actions: ['form.view'],
			field_read_limited: false,
			field_write_limited: false,
			record_limited: false
		};
		const h = harness({
			previews: [
				err(403, {
					error_code: 'policy_rejected',
					details: { permission_state: state }
				})
			]
		});
		await toMapping(h);
		await h.wizard.advance();
		const failure = h.wizard.snapshot().previewFailure;
		assertEqual(failure?.kind, 'policy_rejected', 'policy');
		assertEqual(failure?.retryable, false, 'no retry');
		assertDeepEqual(failure?.permissionState, state, 'state parsed');
		assertEqual(h.wizard.canAdvance('preview'), false, 'blocked');
		assertDeepEqual(
			permissionExplanation(state),
			[
				'flow.bridge.convert.permission.access.readOnly',
				'flow.bridge.convert.permission.configuration.unconfigured',
				'flow.bridge.convert.permission.action.formView'
			],
			'explanation'
		);
		const malformed = classifyConvertFailure({
			code: 403,
			error_code: 'policy_rejected',
			details: { permission_state: { access: 'root' } }
		});
		assertEqual(malformed.permissionState, null, 'malformed state dropped');
	}
);

await suite.checkAsync('a non-retryable commit failure blocks further commits', async () => {
	const h = harness({
		previews: [ok(previewResponse())],
		commits: [err(403, { error_code: 'policy_rejected' })]
	});
	await toCommit(h);
	h.wizard.acknowledge(true);
	await h.wizard.commit();
	assertEqual(h.wizard.canCommit(), false, 'blocked');
	assertEqual((await h.wizard.commit()).status, 'refused', 'refused');
	assertEqual(h.commits.length, 1, 'one request');
});

// ---- 8. i18n -----------------------------------------------------------------------------------

suite.check('every static key exists in zh and en', () => {
	const root = new URL('..', import.meta.url).pathname;
	const load = (locale: string) =>
		JSON.parse(readFileSync(join(root, 'src/lib/i18n', `${locale}.json`), 'utf8')) as Record<
			string,
			unknown
		>;
	const lookup = (tree: Record<string, unknown>, key: string): unknown =>
		key
			.split('.')
			.reduce<unknown>(
				(node, part) =>
					node && typeof node === 'object' ? (node as Record<string, unknown>)[part] : undefined,
				tree
			);
	const all = [
		...Object.values(STEP_KEYS),
		...Object.values(FIELD_PROBLEM_KEYS),
		...Object.values(FAILURE_KEYS),
		...Object.values(PERMISSION_ACTION_KEYS),
		...Object.values(PERMISSION_ACCESS_KEYS),
		...Object.values(PERMISSION_CONFIGURATION_KEYS),
		...Object.values(PERMISSION_LIMIT_KEYS),
		'pageTitle.flowConvert'
	];
	for (const locale of ['zh', 'en']) {
		const tree = load(locale);
		for (const key of all) {
			const value = lookup(tree, key);
			assert(typeof value === 'string' && value !== '', `${locale}.json is missing ${key}`);
		}
	}
	assertNotEqual(STEP_KEYS.preview, STEP_KEYS.commit, 'distinct step keys');
});

finish(suite.result());
