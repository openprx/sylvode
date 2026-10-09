/**
 * Flow -> Forms conversion job page (`/workspace/{id}/flow/conversions/{job_id}`;
 * `contracts/rest-api-v1.md` v0.7 rows `GET /flow/conversions/{job_id}` and
 * `POST /flow/conversions/{job_id}/retry`; task FP-N5).
 *
 * Checks the real request shapes through `FlowCommandService.convertStatus/convertRetry`,
 * `FlowObjectRepository.get` and `formsApi.getRecord` against a mocked `globalThis.fetch`, then
 * drives `ConversionJobController` with injected fakes: the status table matches the backend
 * (`started` polls, `completed`/`failed` stop), polling backs off 1s -> 5s, `canRetry` holds only
 * for a retryable `failed` job, retry needs an explicit confirmation and reuses one key per job,
 * `server_rejected` never offers retry, and `not_found`/`forbidden` (by `error_code` or legacy
 * numeric code, never by `message`) land on one safe state.
 *
 * Run standalone: `bun tests/flow-conversion-job.test.ts`
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import {
	CONVERSION_JOB_STATUSES,
	ConversionJobController,
	JOB_FAILURE_KEYS,
	JOB_STATUS_KEYS,
	canRetry,
	classifyJobFailure,
	isTerminalConversionStatus,
	jobErrorKey,
	type ConversionJobDeps,
	type ConversionJobSnapshot
} from '../src/lib/flow/conversion-job';
import { FlowCommandService } from '../src/lib/flow/command-service';
import { FlowObjectRepository } from '../src/lib/flow/object-repository';
import type { ApiResult } from '../src/lib/api/client';
import type { FlowConversionJob, FlowObjectView } from '../src/lib/api/flow';
import type { FormRecord } from '../src/lib/api/forms';

const suite = new Suite('flow-conversion-job');

const WS = '11111111-1111-4111-8111-111111111111';
const OTHER_WS = '99999999-9999-4999-8999-999999999999';
const OBJ = '22222222-2222-4222-8222-222222222222';
const PROJECT = '33333333-3333-4333-8333-333333333333';
const JOB = '55555555-5555-4555-8555-555555555555';
const RECORD = '66666666-6666-4666-8666-666666666666';
const RECORD_2 = '77777777-7777-4777-8777-777777777777';

function ok<T>(data: T): ApiResult<T> {
	return { code: 0, message: 'ok', data };
}

function err(code: number, extra: Partial<ApiResult<never>> = {}): ApiResult<never> {
	return { code, message: 'everything is fine', data: null, ...extra };
}

function job(overrides: Partial<FlowConversionJob> = {}): FlowConversionJob {
	return {
		job_id: JOB,
		status: 'completed',
		source_object_id: OBJ,
		source_frontier: 'F-1',
		target_schema_version: 3,
		lineage_id: 'lineage-1',
		created_target_ids: [RECORD],
		warnings: [],
		error: null,
		...overrides
	};
}

const started = (): FlowConversionJob =>
	job({ status: 'started', lineage_id: null, created_target_ids: [] });
const failed = (error: string | null = 'injected_before_target_create'): FlowConversionJob =>
	job({ status: 'failed', lineage_id: null, created_target_ids: [], error });

function object(overrides: Partial<FlowObjectView> = {}): FlowObjectView {
	return {
		id: OBJ,
		workspace_id: WS,
		project_id: null,
		object_type: 'page',
		lifecycle_status: 'active',
		governance_metadata: {},
		title: 'Launch plan',
		semantic_content: null,
		document_id: 'doc-1',
		document_seq: 2,
		frontier: 'F-1',
		projection_seq: 2,
		projection_lag: 0,
		created_at: '2026-10-09T11:00:00Z',
		updated_at: '2026-10-09T11:30:00Z',
		archived_at: null,
		...overrides
	};
}

function record(id: string, overrides: Partial<FormRecord> = {}): FormRecord {
	return {
		id,
		workspace_id: WS,
		project_id: PROJECT,
		form_id: 'form-1',
		title: `Record ${id.slice(0, 4)}`,
		values: {},
		source: {},
		created_at: '2026-10-09T12:00:00Z',
		updated_at: '2026-10-09T12:00:00Z',
		...overrides
	};
}

interface Harness {
	controller: ConversionJobController;
	reads: string[];
	retries: Array<{ jobId: string; key: string }>;
	sleeps: number[];
	sourceGets: string[];
	recordGets: string[];
	snapshots: ConversionJobSnapshot[];
	keysMinted: () => number;
}

/** A runaway poll loop (one that ignores a terminal status) is cut off here: the controller is
 * disposed (a thrown error would be swallowed as a transient read failure and loop forever) and
 * the read count then exceeds what every polling check expects. */
const RUNAWAY_LIMIT = 50;

function harness(
	options: {
		jobId?: string;
		reads?: Array<ApiResult<FlowConversionJob>>;
		retries?: Array<ApiResult<FlowConversionJob>>;
		source?: ApiResult<FlowObjectView>;
		records?: Record<string, ApiResult<FormRecord>>;
	} = {}
): Harness {
	const readReplies = options.reads ?? [ok(job())];
	const retryReplies = options.retries ?? [];
	const reads: string[] = [];
	const retries: Array<{ jobId: string; key: string }> = [];
	const sleeps: number[] = [];
	const sourceGets: string[] = [];
	const recordGets: string[] = [];
	const snapshots: ConversionJobSnapshot[] = [];
	let minted = 0;
	let controllerRef: ConversionJobController | null = null;
	const deps: Partial<ConversionJobDeps> & Pick<ConversionJobDeps, 'commands' | 'repository'> = {
		commands: {
			convertStatus: (id: string) => {
				reads.push(id);
				if (reads.length > RUNAWAY_LIMIT) controllerRef?.dispose();
				const next = readReplies.length > 1 ? readReplies.shift() : readReplies[0];
				if (!next) throw new Error('unexpected status read');
				return Promise.resolve(next);
			},
			convertRetry: (id: string, key: string) => {
				retries.push({ jobId: id, key });
				const next = retryReplies.shift();
				if (!next) throw new Error('unexpected retry call');
				return Promise.resolve(next);
			}
		},
		repository: {
			get: (id: string) => {
				sourceGets.push(id);
				return Promise.resolve(options.source ?? ok(object()));
			}
		},
		getRecord: (id: string) => {
			recordGets.push(id);
			return Promise.resolve(options.records?.[id] ?? ok(record(id)));
		},
		sleep: (ms: number) => {
			sleeps.push(ms);
			return Promise.resolve();
		},
		newKey: () => `key-${++minted}`
	};
	const controller: ConversionJobController = new ConversionJobController(
		WS,
		options.jobId ?? JOB,
		(snap) => snapshots.push(snap),
		deps
	);
	controllerRef = controller;
	return {
		controller,
		reads,
		retries,
		sleeps,
		sourceGets,
		recordGets,
		snapshots,
		keysMinted: () => minted
	};
}

// ---- 1. wire shapes through the real adapters -------------------------------------------------

await suite.checkAsync(
	'status, retry, source and record reads hit the contract paths with exact bodies',
	async () => {
		const original = globalThis.fetch;
		const calls: Array<{ method: string; path: string; body: unknown }> = [];
		let controllerRef: ConversionJobController | null = null;
		globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
			const raw = String(input);
			const path = raw.startsWith('http') ? new URL(raw).pathname : raw;
			const method = init?.method ?? 'GET';
			calls.push({ method, path, body: init?.body ? JSON.parse(String(init.body)) : null });
			if (calls.length > RUNAWAY_LIMIT) controllerRef?.dispose();
			let data: unknown = {};
			if (path === `/api/v1/flow/conversions/${JOB}` && method === 'GET') data = failed();
			if (path === `/api/v1/flow/conversions/${JOB}/retry`) data = job();
			if (path === `/api/v1/flow/objects/${OBJ}`) data = object();
			if (path === `/api/v1/form-records/${RECORD}`) data = record(RECORD);
			return new Response(JSON.stringify({ code: 0, message: 'ok', data }), {
				headers: { 'Content-Type': 'application/json' }
			});
		}) as typeof fetch;
		let last: ConversionJobSnapshot | null = null;
		try {
			const controller = new ConversionJobController(
				WS,
				JOB,
				(snap) => {
					last = snap;
				},
				{
					commands: new FlowCommandService(),
					repository: new FlowObjectRepository(),
					newKey: () => 'retry-key',
					sleep: () => Promise.resolve()
				}
			);
			controllerRef = controller;
			await controller.poll();
			assert(controller.requestRetry(), 'retry offered for a failed job');
			assertEqual((await controller.confirmRetry()).status, 'retried', 'retried');
		} finally {
			globalThis.fetch = original;
		}
		assertDeepEqual(
			calls.map(({ method, path }) => [method, path]),
			[
				['GET', `/api/v1/flow/conversions/${JOB}`],
				['GET', `/api/v1/flow/objects/${OBJ}`],
				['POST', `/api/v1/flow/conversions/${JOB}/retry`],
				['GET', `/api/v1/form-records/${RECORD}`]
			],
			'paths'
		);
		assertDeepEqual(calls[2].body, { idempotency_key: 'retry-key', confirm: true }, 'retry body');
		const snap = last as ConversionJobSnapshot | null;
		assertEqual(snap?.job?.status, 'completed', 'job after retry');
		assertDeepEqual(
			snap?.targets,
			[{ id: RECORD, state: 'record', projectId: PROJECT, title: record(RECORD).title }],
			'target resolved through formsApi.getRecord'
		);
	}
);

// ---- 2. status table --------------------------------------------------------------------------

suite.check('status table matches the backend: started polls, completed/failed stop', () => {
	assertDeepEqual([...CONVERSION_JOB_STATUSES], ['started', 'completed', 'failed'], 'statuses');
	assertEqual(isTerminalConversionStatus('started'), false, 'started');
	assertEqual(isTerminalConversionStatus('completed'), true, 'completed');
	assertEqual(isTerminalConversionStatus('failed'), true, 'failed');
	assertEqual(isTerminalConversionStatus('queued'), true, 'unknown status stops polling');
});

suite.check('canRetry holds only for a failed job that is not server_rejected', () => {
	assertEqual(canRetry(failed()), true, 'failed');
	assertEqual(canRetry(failed(null)), true, 'failed without code');
	assertEqual(canRetry(job()), false, 'completed');
	assertEqual(canRetry(started()), false, 'started');
	assertEqual(canRetry(job({ status: 'queued' as never })), false, 'unknown');
	assertEqual(canRetry(failed('server_rejected')), false, 'server_rejected job');
	assertEqual(canRetry(null), false, 'no job');
});

// ---- 3. polling -------------------------------------------------------------------------------

await suite.checkAsync(
	'a non-terminal job keeps polling with 1s..5s backoff and stops at the terminal status',
	async () => {
		const h = harness({
			reads: [
				ok(started()),
				ok(started()),
				ok(started()),
				ok(started()),
				ok(started()),
				ok(started()),
				ok(job())
			]
		});
		const state = await h.controller.poll();
		assertEqual(state, 'ready', 'ready');
		assertEqual(h.reads.length, 7, 'reads until completed');
		assertDeepEqual(h.sleeps, [1000, 2000, 4000, 5000, 5000, 5000], 'backoff capped at 5s');
		const snap = h.controller.snapshot();
		assertEqual(snap.polling, false, 'polling stopped');
		assertEqual(snap.terminal, true, 'terminal');
		assertEqual(snap.job?.status, 'completed', 'completed');
		assert(
			h.snapshots.some((s) => s.polling && s.job?.status === 'started'),
			'a started job was shown while polling'
		);
		// A second poll on a finished job reads once and stops again.
		await h.controller.poll();
		assertEqual(h.reads.length, 8, 'one more read only');
	}
);

await suite.checkAsync('a terminal status at the first read stops polling at once', async () => {
	for (const terminal of [job(), failed()]) {
		const h = harness({ reads: [ok(terminal), ok(started())] });
		await h.controller.poll();
		assertEqual(h.reads.length, 1, `${terminal.status}: one read`);
		assertDeepEqual(h.sleeps, [], `${terminal.status}: no sleep`);
	}
});

await suite.checkAsync(
	'a transient read failure keeps polling and keeps the last job',
	async () => {
		const h = harness({
			reads: [ok(started()), err(503), { code: 500, message: 'not_found', data: null }, ok(job())]
		});
		await h.controller.poll();
		assertEqual(h.reads.length, 4, 'kept polling through 5xx');
		assert(
			h.snapshots.some((s) => s.pollFailure?.kind === 'unavailable' && s.job?.status === 'started'),
			'transient failure shown with the last job'
		);
		const snap = h.controller.snapshot();
		assertEqual(snap.pollFailure, null, 'cleared after a good read');
		assertEqual(snap.loadState, 'ready', 'ready');
	}
);

await suite.checkAsync('dispose stops polling', async () => {
	const h = harness({ reads: [ok(started())] });
	const run = h.controller.poll();
	h.controller.dispose();
	await run;
	assert(h.reads.length <= 1, `reads after dispose: ${h.reads.length}`);
});

// ---- 4. not_found / forbidden -----------------------------------------------------------------

await suite.checkAsync(
	'not_found and forbidden land on one safe state by error_code or numeric code',
	async () => {
		const cases: Array<[string, ApiResult<FlowConversionJob>]> = [
			['legacy 404', err(404)],
			['legacy 403', err(403)],
			['typed not_found', err(404, { error_code: 'not_found' })],
			['typed forbidden on 400', err(400, { error_code: 'forbidden' })]
		];
		for (const [label, reply] of cases) {
			const h = harness({ reads: [reply] });
			const state = await h.controller.poll();
			assertEqual(state, 'not_found', label);
			assertEqual(h.reads.length, 1, `${label}: stops`);
			const snap = h.controller.snapshot();
			assertEqual(snap.job, null, `${label}: no job`);
			assertEqual(snap.canRetry, false, `${label}: no retry`);
			assertDeepEqual(h.sourceGets, [], `${label}: source not read`);
		}
	}
);

await suite.checkAsync('message is never a branch', async () => {
	assertEqual(classifyJobFailure({ code: 500, error_code: undefined }).kind, 'unavailable', '500');
	const notFoundMessage = harness({
		reads: [{ code: 400, message: 'not_found', data: null }]
	});
	assertEqual(
		await notFoundMessage.controller.poll(),
		'failed',
		'legacy 400 with message not_found'
	);
	assertEqual(notFoundMessage.controller.snapshot().loadFailure?.kind, 'rejected', 'rejected');
	assertEqual(
		classifyJobFailure({ code: 409, error_code: undefined }).kind,
		'conflict',
		'legacy 409'
	);
	assertEqual(
		classifyJobFailure({ code: 400, error_code: 'brand_new_code' }).retryable,
		false,
		'unknown typed'
	);
	assertEqual(
		classifyJobFailure({
			code: 500,
			error_code: 'server_draining',
			details: { reason: 'contention' }
		}).messageKey,
		'flow.error.server_draining.contention',
		'drain reason key'
	);
});

await suite.checkAsync('a malformed job id never reaches the server', async () => {
	const h = harness({ jobId: 'not-a-uuid' });
	assertEqual(await h.controller.poll(), 'not_found', 'not_found');
	assertEqual(h.reads.length, 0, 'no request');
});

// ---- 5. retry gating --------------------------------------------------------------------------

await suite.checkAsync(
	'retry is refused for completed and started jobs and without a confirmation',
	async () => {
		for (const reply of [job(), started()]) {
			const h = harness({ reads: [ok(reply), ok(job())] });
			await h.controller.poll();
			const before = h.controller.snapshot();
			if (reply.status === 'started') assertEqual(before.job?.status, 'completed', 'polled on');
			assertEqual(h.controller.requestRetry(), false, `${reply.status}: requestRetry`);
			assertEqual(
				(await h.controller.confirmRetry()).status,
				'refused',
				`${reply.status}: confirm`
			);
			assertEqual(h.retries.length, 0, `${reply.status}: no retry sent`);
		}
		const h = harness({ reads: [ok(failed())] });
		await h.controller.poll();
		assertEqual(h.controller.snapshot().canRetry, true, 'failed job offers retry');
		assertEqual((await h.controller.confirmRetry()).status, 'refused', 'confirm before request');
		assert(h.controller.requestRetry(), 'request');
		assertEqual(h.controller.snapshot().confirmingRetry, true, 'confirming');
		h.controller.cancelRetry();
		assertEqual((await h.controller.confirmRetry()).status, 'refused', 'confirm after cancel');
		assertEqual(h.retries.length, 0, 'nothing sent');
	}
);

await suite.checkAsync('the retry key is fixed per job and reused by every retry', async () => {
	const h = harness({
		reads: [ok(failed())],
		retries: [err(503), ok(failed()), ok(job({ created_target_ids: [RECORD_2] }))]
	});
	await h.controller.poll();
	for (let i = 0; i < 3; i += 1) {
		assert(h.controller.requestRetry(), `request ${i + 1}`);
		await h.controller.confirmRetry();
	}
	assertDeepEqual(
		h.retries,
		[
			{ jobId: JOB, key: 'key-1' },
			{ jobId: JOB, key: 'key-1' },
			{ jobId: JOB, key: 'key-1' }
		],
		'same key for every retry of the job'
	);
	assertEqual(h.keysMinted(), 1, 'one key minted');
	const snap = h.controller.snapshot();
	assertEqual(snap.job?.status, 'completed', 'completed');
	assertEqual(snap.canRetry, false, 'no retry after completion');
	assertDeepEqual(
		snap.targets.map((t) => [t.id, t.state]),
		[[RECORD_2, 'record']],
		'targets re-resolved after the retry'
	);
});

await suite.checkAsync('a retry that returns a running job resumes polling', async () => {
	const h = harness({ reads: [ok(failed()), ok(started()), ok(job())], retries: [ok(started())] });
	await h.controller.poll();
	h.controller.requestRetry();
	await h.controller.confirmRetry();
	for (let i = 0; i < 100 && h.controller.snapshot().polling; i += 1) {
		await new Promise((resolve) => setTimeout(resolve, 0));
	}
	assertEqual(h.controller.snapshot().polling, false, 'polling finished');
	assertEqual(h.reads.length, 3, 'two more reads after the retry');
	assertEqual(h.controller.snapshot().job?.status, 'completed', 'polled to completion');
});

await suite.checkAsync('server_rejected is never retryable', async () => {
	const stored = harness({ reads: [ok(failed('server_rejected'))] });
	await stored.controller.poll();
	assertEqual(stored.controller.snapshot().canRetry, false, 'stored server_rejected');
	assertEqual(stored.controller.requestRetry(), false, 'request refused');
	assertEqual(jobErrorKey('server_rejected'), 'flow.error.server_rejected', 'error key');

	const h = harness({
		reads: [ok(failed())],
		retries: [err(500, { error_code: 'server_rejected', message: 'please retry' })]
	});
	await h.controller.poll();
	h.controller.requestRetry();
	const outcome = await h.controller.confirmRetry();
	assert(outcome.status === 'failed', 'failed');
	assertEqual(outcome.failure.kind, 'server_rejected', 'kind');
	assertEqual(outcome.failure.retryable, false, 'not retryable');
	assertEqual(outcome.failure.messageKey, 'flow.error.server_rejected', 'key');
	assertEqual(h.controller.snapshot().canRetry, false, 'retry withdrawn');
	assertEqual(h.controller.requestRetry(), false, 'second request refused');
	assertEqual(h.retries.length, 1, 'one request');
});

await suite.checkAsync(
	'stale_frontier and policy_rejected on retry withdraw the retry action',
	async () => {
		for (const code of ['stale_frontier', 'policy_rejected'] as const) {
			const h = harness({ reads: [ok(failed())], retries: [err(409, { error_code: code })] });
			await h.controller.poll();
			h.controller.requestRetry();
			const outcome = await h.controller.confirmRetry();
			assert(outcome.status === 'failed', `${code}: failed`);
			assertEqual(outcome.failure.kind, code, `${code}: kind`);
			assertEqual(h.controller.snapshot().canRetry, false, `${code}: withdrawn`);
		}
		const transient = harness({ reads: [ok(failed())], retries: [err(502)] });
		await transient.controller.poll();
		transient.controller.requestRetry();
		await transient.controller.confirmRetry();
		assertEqual(transient.controller.snapshot().canRetry, true, '5xx keeps retry available');
	}
);

await suite.checkAsync('a not_found retry lands on the safe state', async () => {
	const h = harness({ reads: [ok(failed())], retries: [err(404)] });
	await h.controller.poll();
	h.controller.requestRetry();
	await h.controller.confirmRetry();
	const snap = h.controller.snapshot();
	assertEqual(snap.loadState, 'not_found', 'not_found');
	assertEqual(snap.job, null, 'job hidden');
});

// ---- 6. source and targets --------------------------------------------------------------------

await suite.checkAsync(
	'the source title comes from ObjectRepository.get; without access only the id',
	async () => {
		const visible = harness();
		await visible.controller.poll();
		assertDeepEqual(
			visible.controller.snapshot().source,
			{ state: 'ready', title: 'Launch plan' },
			'ready'
		);
		assertDeepEqual(visible.sourceGets, [OBJ], 'read once');
		for (const [label, reply] of [
			['404', err(404)],
			['typed forbidden', err(403, { error_code: 'forbidden' })],
			['other workspace', ok(object({ workspace_id: OTHER_WS }))]
		] as const) {
			const h = harness({ source: reply });
			await h.controller.poll();
			assertDeepEqual(h.controller.snapshot().source, { state: 'hidden' }, label);
		}
	}
);

await suite.checkAsync(
	'created ids link only when they resolve to a record in this workspace',
	async () => {
		const FORM_ID = '88888888-8888-4888-8888-888888888888';
		const h = harness({
			reads: [ok(job({ created_target_ids: [RECORD, RECORD_2, FORM_ID] }))],
			records: {
				[RECORD_2]: ok(record(RECORD_2, { workspace_id: OTHER_WS })),
				[FORM_ID]: err(404)
			}
		});
		await h.controller.poll();
		assertDeepEqual(
			h.controller.snapshot().targets,
			[
				{ id: RECORD, state: 'record', projectId: PROJECT, title: record(RECORD).title },
				{ id: RECORD_2, state: 'unresolved' },
				{ id: FORM_ID, state: 'unresolved' }
			],
			'targets'
		);
		assertDeepEqual(h.recordGets, [RECORD, RECORD_2, FORM_ID], 'each id looked up once');
	}
);

// ---- 7. error and i18n keys -------------------------------------------------------------------

suite.check('a stored error code is shown through a key, never as text', () => {
	assertEqual(jobErrorKey(null), null, 'null');
	assertEqual(jobErrorKey(''), null, 'empty');
	assertEqual(jobErrorKey('policy_rejected'), 'flow.error.policy_rejected', 'known code');
	assertEqual(
		jobErrorKey('injected_before_target_create'),
		JOB_FAILURE_KEYS.jobFailed,
		'unknown code -> generic'
	);
});

suite.check('every static key and every key the view uses exists in zh and en', () => {
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
	const view = readFileSync(join(root, 'src/lib/components/flow/ConversionJobView.svelte'), 'utf8');
	const used = [...view.matchAll(/'(flow\.bridge\.job\.[A-Za-z.]+)'/g)].map((m) => m[1]);
	assert(used.length >= 20, `view keys found: ${used.length}`);
	const all = [
		...Object.values(JOB_STATUS_KEYS),
		...Object.values(JOB_FAILURE_KEYS),
		...used,
		'pageTitle.flowConversion'
	];
	for (const locale of ['zh', 'en']) {
		const tree = load(locale);
		for (const key of all) {
			const value = lookup(tree, key);
			assert(typeof value === 'string' && value !== '', `${locale}.json is missing ${key}`);
		}
	}
});

finish(suite.result());
