/**
 * Object page right panel: history semantic diff (`GET .../diff`) and single-object export
 * (`POST /flow/objects/{id}/exports` -> `GET /flow/exports/{job_id}` -> artifact).
 *
 * - diff lines carry node kind, text and property changes only (no node ids, frontiers or
 *   bytes), and a rejected diff is mapped by `error_code` through the real `CommandService`;
 * - export: csv only for a Collection, `include_history` only with `package`, the contract
 *   request shape, one key per intent (reused on a retryable failure), polling through
 *   `PackageExportController` to the terminal status, and the local SHA-256 check on download.
 *
 * Run standalone: `bun tests/flow-object-panel.test.ts`
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assert, assertDeepEqual, assertEqual, finish, waitFor } from './support/harness';
import {
	DIFF_KIND_KEYS,
	DIFF_OPERATION_KEYS,
	HistoryDiffLoader,
	diffRangeFor,
	semanticDiffLines
} from '../src/lib/flow/history-diff';
import {
	OBJECT_EXPORT_FIELD_SECRECY_KEY,
	OBJECT_EXPORT_FORMAT_KEYS,
	ObjectExportController,
	buildObjectExportRequest,
	exportJobErrorKey,
	objectExportFileName,
	objectExportFormats
} from '../src/lib/flow/object-export';
import { FlowCommandService } from '../src/lib/flow/command-service';
import type { CreateObjectExportInput, FlowExportJob } from '../src/lib/api/flow';
import type { ApiResult } from '../src/lib/api/client';

const OBJECT = 'obj-1';
const WS = 'ws-1';
/** Artifact body; its SHA-256 is computed in the test. */
const MD = '# Title';

function ok<T>(data: T): ApiResult<T> {
	return { code: 0, message: 'ok', data };
}

async function sha256Hex(text: string): Promise<string> {
	const digest = new Uint8Array(
		await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text))
	);
	return Array.from(digest, (byte) => byte.toString(16).padStart(2, '0')).join('');
}

const suite = new Suite('flow-object-panel');

suite.check('semantic diff lines: title, added, removed, changed, moved; no ids', () => {
	const diff = {
		title: { before: 'Old', after: 'New' },
		nodes: {
			added: {
				'node-secret-a': {
					parent: null,
					order_key: 'a',
					kind: 'block',
					text: 'hello',
					properties: {},
					deleted: false
				}
			},
			removed: {
				'node-secret-b': {
					parent: null,
					order_key: 'b',
					kind: 'block',
					text: 'bye',
					properties: {},
					deleted: false
				}
			},
			changed: {
				'node-secret-c': {
					before: {
						parent: null,
						order_key: 'c',
						kind: 'block',
						text: 'one',
						properties: { level: '1' },
						deleted: false
					},
					after: {
						parent: null,
						order_key: 'c',
						kind: 'block',
						text: 'two',
						properties: { level: '2' },
						deleted: false
					}
				},
				'node-secret-d': {
					before: {
						parent: null,
						order_key: 'd',
						kind: 'block',
						text: 'same',
						properties: {},
						deleted: false
					},
					after: {
						parent: 'node-secret-x',
						order_key: 'e',
						kind: 'block',
						text: 'same',
						properties: {},
						deleted: false
					}
				}
			}
		}
	};
	const lines = semanticDiffLines(diff);
	assert(lines, 'documented shape is recognised');
	assertDeepEqual(
		lines.map((line) => [line.op, line.kind, line.before, line.after]),
		[
			['title', null, 'Old', 'New'],
			['added', 'block', null, 'hello'],
			['removed', 'block', 'bye', null],
			['changed', 'block', 'one', 'two'],
			['moved', 'block', 'same', 'same']
		],
		'lines'
	);
	assertDeepEqual(
		lines[3].properties,
		[{ key: 'level', before: '1', after: '2' }],
		'property change'
	);
	const text = JSON.stringify(lines);
	assert(!text.includes('node-secret') && !text.includes('order_key'), 'no node ids or order keys');
	assertEqual(semanticDiffLines('rendered string'), null, 'unknown shape is not guessed');
	assertDeepEqual(
		semanticDiffLines({ title: null, nodes: { added: {}, removed: {}, changed: {} } }),
		[],
		'empty diff'
	);
	assertDeepEqual(diffRangeFor(5), { from: 4, to: 5 }, 'entry seq n diffs n-1..n');
	assertDeepEqual(diffRangeFor(0), { from: 0, to: 0 }, 'never negative');
});

await suite.checkAsync(
	'a rejected diff is mapped by error_code through CommandService',
	async () => {
		const seen: string[] = [];
		const original = globalThis.fetch;
		globalThis.fetch = (async (input: string | URL | Request) => {
			const raw = String(input);
			seen.push(raw.startsWith('http') ? new URL(raw).pathname + new URL(raw).search : raw);
			return new Response(
				JSON.stringify({
					code: 409,
					message: 'not_found forbidden',
					data: null,
					error_code: 'resync_required'
				}),
				{ headers: { 'Content-Type': 'application/json' } }
			);
		}) as typeof fetch;
		try {
			const loader = new HistoryDiffLoader(OBJECT, new FlowCommandService());
			const result = await loader.load(7);
			assertDeepEqual(
				result,
				{ status: 'failed', messageKey: 'flow.error.resync_required' },
				'typed key'
			);
		} finally {
			globalThis.fetch = original;
		}
		assertDeepEqual(
			seen,
			[`/api/v1/flow/objects/${OBJECT}/diff?from_seq=6&to_seq=7`],
			'diff query'
		);
	}
);

await suite.checkAsync('a successful diff is cached per seq', async () => {
	let calls = 0;
	const loader = new HistoryDiffLoader(OBJECT, {
		diff: async (objectId, from, to) => {
			calls += 1;
			return {
				object_id: objectId,
				from_seq: from,
				to_seq: to,
				from_frontier: 'f',
				to_frontier: 't',
				semantic_diff: { title: null, nodes: { added: {}, removed: {}, changed: {} } }
			};
		}
	});
	await loader.load(3);
	await loader.load(3);
	assertEqual(calls, 1, 'second expand reuses the result');
});

suite.check('export formats and request body', () => {
	assertDeepEqual(
		objectExportFormats('page'),
		['json', 'markdown', 'package'],
		'no csv for a page'
	);
	assertDeepEqual(
		objectExportFormats('collection'),
		['json', 'markdown', 'csv', 'package'],
		'csv for a collection'
	);
	assertDeepEqual(
		buildObjectExportRequest({ format: 'markdown', includeHistory: true }, 'k'),
		{ format: 'markdown', include_history: false, idempotency_key: 'k' },
		'history is never sent with a non-package format'
	);
	assertDeepEqual(
		buildObjectExportRequest({ format: 'package', includeHistory: true }, 'k'),
		{ format: 'package', include_history: true, idempotency_key: 'k' },
		'package carries history'
	);
	assertEqual(
		objectExportFileName('Launch plan!', 'abcdef123456', 'markdown'),
		'Launch-plan-abcdef12.md',
		'file name'
	);
	assertEqual(
		objectExportFileName('', 'abcdef123456', 'package'),
		'page-abcdef12.sylvode-flow.zip',
		'package extension'
	);
	assertEqual(exportJobErrorKey(null), null, 'no job, no error');
	assertEqual(
		exportJobErrorKey({
			job_id: 'j',
			status: 'failed',
			format: 'json',
			checksum: '',
			size: 0,
			expires_at: '',
			error: 'limit_exceeded'
		}),
		'flow.error.limit_exceeded',
		'a stable error code maps to its key'
	);
});

await suite.checkAsync(
	'export submit, retry key, poll to completed, download with local hash',
	async () => {
		const checksum = await sha256Hex(MD);
		const posts: CreateObjectExportInput[] = [];
		const gets: string[] = [];
		const sleeps: number[] = [];
		let n = 0;
		const job = (status: string, extra: Partial<FlowExportJob> = {}): FlowExportJob => ({
			job_id: 'job-12345678',
			status,
			format: 'markdown',
			object_id: OBJECT,
			checksum,
			size: MD.length,
			expires_at: '2099-01-01T00:00:00Z',
			...extra
		});
		const postReplies: Array<ApiResult<FlowExportJob>> = [
			{
				code: 503,
				message: 'busy',
				data: null,
				error_code: 'server_draining',
				details: { reason: 'contention' }
			},
			ok(job('running'))
		];
		const getReplies: Array<ApiResult<FlowExportJob>> = [
			ok(job('running')),
			ok(job('completed', { download_url: '/api/v1/flow/exports/job-12345678/artifact' }))
		];
		const downloads: string[] = [];
		let blobText = MD;
		const controller = new ObjectExportController(OBJECT, WS, () => {}, {
			commands: {
				startObjectExport: async (objectId, body) => {
					assertEqual(objectId, OBJECT, 'object path parameter');
					posts.push(body);
					return postReplies.length > 1
						? (postReplies.shift() as ApiResult<FlowExportJob>)
						: postReplies[0];
				}
			},
			reads: {
				getExportJob: async (jobId) => {
					gets.push(jobId);
					if (gets.length > 20) return { code: 404, message: 'runaway', data: null };
					return getReplies.length > 1
						? (getReplies.shift() as ApiResult<FlowExportJob>)
						: getReplies[0];
				},
				downloadExportArtifact: async (url) => {
					downloads.push(url);
					return ok({ blob: new Blob([blobText]), sha256: checksum });
				}
			},
			sleep: async (ms) => {
				sleeps.push(ms);
			},
			newKey: () => {
				n += 1;
				return `key-${n}`;
			}
		});
		const first = await controller.submit({ format: 'markdown', includeHistory: false });
		assert(first.status === 'failed', 'first submit fails');
		assertEqual(first.failure.retryable, true, 'contention is retryable');
		const second = await controller.submit({ format: 'markdown', includeHistory: false });
		assertEqual(second.status, 'started', 'retry starts');
		assertDeepEqual(
			posts,
			[
				{ format: 'markdown', include_history: false, idempotency_key: 'key-1' },
				{ format: 'markdown', include_history: false, idempotency_key: 'key-1' }
			],
			'same intent, same key'
		);
		await waitFor(() => controller.snapshot.job?.polling === false, 'the background poll finishes');
		const final = controller.snapshot.job;
		assertEqual(final?.job?.status, 'completed', 'polled to completed');
		assert(gets.length >= 2 && gets.every((id) => id === 'job-12345678'), 'polled the started job');
		assert(sleeps.length >= 1 && sleeps[0] === 1000, 'backoff starts at 1s');
		const completed = controller.snapshot.job?.job;
		assert(completed, 'snapshot exposes the job');
		const download = await controller.download(completed, 'Title');
		assert(download.status === 'ready', 'download verified');
		assertEqual(download.fileName, 'Title-job-1234.md', 'markdown file name');
		assertDeepEqual(downloads, ['/api/v1/flow/exports/job-12345678/artifact'], 'download_url used');
		blobText = '# Tampered';
		const tampered = await controller.download(completed, 'Title');
		assert(tampered.status === 'failed', 'bytes that do not hash to the checksum are refused');
		assertEqual(tampered.failure.messageKey, 'flow.error.checksum_mismatch', 'checksum_mismatch');
		const next = await controller.submit({ format: 'package', includeHistory: true });
		assertEqual(next.status, 'started', 'a new intent');
		assertEqual(posts[2].idempotency_key, 'key-2', 'a new intent gets a new key');
		controller.dispose();
	}
);

await suite.checkAsync('every diff and export i18n key exists in zh and en', async () => {
	const root = new URL('..', import.meta.url).pathname;
	for (const locale of ['zh', 'en']) {
		const data = JSON.parse(readFileSync(join(root, 'src/lib/i18n', `${locale}.json`), 'utf8'));
		const lookup = (key: string): unknown =>
			key
				.split('.')
				.reduce<unknown>((node, part) => (node as Record<string, unknown>)?.[part], data);
		const missing = [
			...Object.values(DIFF_OPERATION_KEYS),
			...Object.values(DIFF_KIND_KEYS),
			'flow.history.diff.kind.other',
			'flow.history.diff.error',
			...Object.values(OBJECT_EXPORT_FORMAT_KEYS),
			'flow.panel.export.failed'
		].filter((key) => typeof lookup(key) !== 'string');
		assertDeepEqual(missing, [], `${locale}.json missing keys`);
	}
});

await suite.checkAsync(
	'a secret record keeps the package option; its field_secrecy refusal has its own wording',
	async () => {
		assertDeepEqual(
			objectExportFormats('record'),
			['json', 'markdown', 'package'],
			'the client does not guess which records are secret'
		);
		const controller = new ObjectExportController(OBJECT, WS, () => {}, {
			commands: {
				startObjectExport: async () => ({
					code: 403,
					message: 'irrelevant prose',
					data: null,
					error_code: 'policy_rejected',
					details: { reason: 'field_secrecy' }
				})
			},
			reads: {
				getExportJob: async () => {
					throw new Error('no job is polled');
				},
				downloadExportArtifact: async () => {
					throw new Error('nothing is downloaded');
				}
			},
			sleep: async () => {},
			newKey: () => 'k-secret'
		});
		const outcome = await controller.submit({ format: 'package', includeHistory: false });
		assertEqual(outcome.status, 'failed', 'refused');
		const failure = outcome.status === 'failed' ? outcome.failure : null;
		assertEqual(failure?.messageKey, OBJECT_EXPORT_FIELD_SECRECY_KEY, 'field secrecy wording');
		assertEqual(failure?.retryable, false, 'not retryable as the same request');
		for (const locale of ['zh', 'en']) {
			const data = JSON.parse(
				readFileSync(join(new URL('..', import.meta.url).pathname, 'src/lib/i18n', `${locale}.json`), 'utf8')
			);
			const text = OBJECT_EXPORT_FIELD_SECRECY_KEY.split('.').reduce<unknown>(
				(node, part) => (node as Record<string, unknown> | undefined)?.[part],
				data
			);
			assert(typeof text === 'string' && text.length > 0, `${locale} has the field secrecy text`);
		}
	}
);

finish(suite.result());
