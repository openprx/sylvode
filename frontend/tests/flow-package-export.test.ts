/**
 * Workspace package export (`POST /workspaces/{id}/flow/exports` -> `GET /flow/exports/{job_id}`
 * -> `download_url`, `contracts/rest-api-v1.md` v0.8 table).
 *
 * Checks the real `flowApi` request shapes against a mocked `globalThis.fetch`, then drives
 * `PackageExportController` with an injected API and sleep so the polling schedule is exact:
 * immediate first poll, 1s/2s/4s/5s backoff, stop at the terminal status, `download_url` only on
 * success, one idempotency key per intent, `error_code`-only branching, and the guarded
 * per-workspace recent-jobs list.
 *
 * Run standalone: `bun tests/flow-package-export.test.ts`
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
	EXPORT_STATUS_KEYS,
	PackageExportController,
	RECENT_EXPORTS_MAX,
	downloadExport,
	exportDownloadUrl,
	exportFileName,
	readRecentExports,
	recentExportsKey,
	rememberExport,
	sha256Fallback,
	sha256HexOfBlob,
	type FlowExportJob,
	type KeyValueStore
} from '../src/lib/flow/package-export';
import { FLOW_PACKAGE_ERROR_KEYS } from '../src/lib/flow/package-import';
import { flowApi } from '../src/lib/api/flow';
import type { ApiResult } from '../src/lib/api/client';

const WS = 'ws-1';
const CHECKSUM = 'd'.repeat(64);
/** SHA-256 of the three ASCII bytes `zip`. */
const ZIP_SHA256 = '4a70fe9aa6436e02c2dea340fbd1e352e4ef2d8ce6ca52ad25d4b95471fc8bf2';

function job(status: string, extra: Partial<FlowExportJob> = {}): FlowExportJob {
	return {
		job_id: 'job-1',
		status,
		format: 'package',
		workspace_id: WS,
		package_schema: 'v1',
		checksum: CHECKSUM,
		size: 1234,
		expires_at: '2099-01-01T00:00:00Z',
		...extra
	};
}

function ok<T>(data: T): ApiResult<T> {
	return { code: 0, message: 'ok', data };
}

class MemoryStore implements KeyValueStore {
	readonly map = new Map<string, string>();
	getItem(key: string): string | null {
		return this.map.get(key) ?? null;
	}
	setItem(key: string, value: string): void {
		this.map.set(key, value);
	}
}

interface Harness {
	controller: PackageExportController;
	posts: Array<{ workspaceId: string; body: unknown }>;
	gets: string[];
	sleeps: number[];
	store: MemoryStore;
}

function harness(
	postReplies: Array<ApiResult<FlowExportJob>>,
	getReplies: Array<ApiResult<FlowExportJob>>,
	store = new MemoryStore()
): Harness {
	const posts: Harness['posts'] = [];
	const gets: string[] = [];
	const sleeps: number[] = [];
	let n = 0;
	const next = <T>(queue: T[]): T => (queue.length > 1 ? (queue.shift() as T) : queue[0]);
	const controller = new PackageExportController(WS, () => {}, {
		api: {
			exportWorkspace: async (workspaceId, body) => {
				posts.push({ workspaceId, body });
				return next(postReplies);
			},
			getExportJob: async (jobId) => {
				gets.push(jobId);
				// Runaway guard: a poller that ignores terminal statuses ends here as a visible failure
				// instead of spinning forever on mocked sleeps.
				if (gets.length > 50) return { code: 404, message: 'runaway', data: null };
				return next(getReplies);
			}
		},
		sleep: async (ms) => {
			sleeps.push(ms);
		},
		newKey: () => {
			n += 1;
			return `key-${n}`;
		},
		store
	});
	return { controller, posts, gets, sleeps, store };
}

async function settle(): Promise<void> {
	for (let i = 0; i < 50; i += 1) await Promise.resolve();
	await new Promise((resolve) => setTimeout(resolve, 0));
}

const suite = new Suite('flow-package-export');

await suite.checkAsync('flowApi export endpoints use the contract paths and body', async () => {
	const seen: Array<{ method: string; path: string; body: unknown }> = [];
	const original = globalThis.fetch;
	globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
		const raw = String(input);
		seen.push({
			method: init?.method ?? 'GET',
			path: raw.startsWith('http') ? new URL(raw).pathname : raw,
			body: init?.body ? JSON.parse(String(init.body)) : null
		});
		return new Response(JSON.stringify({ code: 0, message: 'ok', data: job('completed') }), {
			headers: { 'Content-Type': 'application/json' }
		});
	}) as typeof fetch;
	try {
		await flowApi.exportWorkspace(WS, {
			format: 'package',
			include_history: false,
			idempotency_key: 'k'
		});
		await flowApi.getExportJob('job-1');
		const refused = await flowApi.downloadExportArtifact('https://evil.example/x');
		assertNotEqual(refused.code, 0, 'a foreign download URL is refused');
	} finally {
		globalThis.fetch = original;
	}
	assertDeepEqual(
		seen,
		[
			{
				method: 'POST',
				path: `/api/v1/workspaces/${WS}/flow/exports`,
				body: { format: 'package', include_history: false, idempotency_key: 'k' }
			},
			{ method: 'GET', path: '/api/v1/flow/exports/job-1', body: null }
		],
		'exactly POST exports and GET job; the refused download sent nothing'
	);
});

await suite.checkAsync(
	'submit body: whole workspace omits project_id, project scope carries it',
	async () => {
		const h = harness([ok(job('completed'))], [ok(job('completed', { download_url: '/x' }))]);
		await h.controller.submit({ includeHistory: false, projectId: null });
		await settle();
		await h.controller.submit({ includeHistory: true, projectId: 'project-1' });
		await settle();
		assertDeepEqual(
			h.posts.map((post) => post.body),
			[
				{ format: 'package', include_history: false, idempotency_key: 'key-1' },
				{
					format: 'package',
					include_history: true,
					project_id: 'project-1',
					idempotency_key: 'key-2'
				}
			],
			'request bodies'
		);
		assertEqual(h.posts[0].workspaceId, WS, 'workspace path parameter');
	}
);

await suite.checkAsync(
	'a retryable failure keeps the key; a changed option is a new intent',
	async () => {
		const h = harness(
			[
				{ code: 503, message: 'busy', data: null },
				{ code: 503, message: 'busy', data: null },
				ok(job('completed'))
			],
			[ok(job('completed'))]
		);
		const first = await h.controller.submit({ includeHistory: false, projectId: null });
		assertEqual(first.status, 'failed', 'first fails');
		assert(first.status === 'failed' && first.failure.retryable, 'retryable');
		await h.controller.submit({ includeHistory: false, projectId: null });
		await h.controller.submit({ includeHistory: true, projectId: null });
		await settle();
		const keys = h.posts.map((post) => (post.body as { idempotency_key: string }).idempotency_key);
		assertEqual(keys[0], keys[1], 'retry reuses the key');
		assertNotEqual(keys[2], keys[1], 'changed option mints a new key');
	}
);

await suite.checkAsync(
	'polling: immediate, then 1s/2s/4s/5s backoff, stops at the terminal status',
	async () => {
		const h = harness(
			[ok(job('queued'))],
			[
				ok(job('queued')),
				ok(job('running')),
				ok(job('running')),
				ok(job('running')),
				ok(job('running')),
				ok(job('completed', { download_url: '/api/v1/flow/exports/job-1/artifact' })),
				ok(job('running'))
			]
		);
		await h.controller.submit({ includeHistory: false, projectId: null });
		await settle();
		assertEqual(h.gets.length, 6, 'six polls, the last one terminal');
		assertDeepEqual(h.sleeps, [1000, 2000, 4000, 5000, 5000], 'backoff schedule capped at 5s');
		const view = h.controller.snapshot[0];
		assertEqual(view.polling, false, 'no longer polling');
		assertEqual(view.job?.status, 'completed', 'completed');
		await settle();
		assertEqual(h.gets.length, 6, 'no poll after the terminal status');
		assertEqual(
			exportDownloadUrl(view.job),
			'/api/v1/flow/exports/job-1/artifact',
			'download available on success'
		);
	}
);

await suite.checkAsync(
	'download_url is surfaced only for a successful terminal status',
	async () => {
		const url = '/api/v1/flow/exports/job-1/artifact';
		assertEqual(exportDownloadUrl(job('running', { download_url: url })), null, 'running');
		assertEqual(exportDownloadUrl(job('queued', { download_url: url })), null, 'queued');
		assertEqual(exportDownloadUrl(job('failed', { download_url: url })), null, 'failed');
		assertEqual(exportDownloadUrl(job('expired', { download_url: url })), null, 'expired');
		assertEqual(exportDownloadUrl(job('completed')), null, 'completed without url');
		assertEqual(
			exportDownloadUrl(job('completed', { download_url: url })),
			url,
			'completed with url'
		);
		assertEqual(exportDownloadUrl(null), null, 'no job');
	}
);

await suite.checkAsync(
	'a failed terminal status stops polling and offers no download',
	async () => {
		const h = harness(
			[ok(job('running'))],
			[ok(job('running')), ok(job('failed', { download_url: '/x' }))]
		);
		await h.controller.submit({ includeHistory: false, projectId: null });
		await settle();
		assertEqual(h.gets.length, 2, 'stopped at failed');
		assertEqual(exportDownloadUrl(h.controller.snapshot[0].job), null, 'no download for failed');
	}
);

await suite.checkAsync(
	'errors: code-only classification; permanent GET errors stop polling',
	async () => {
		const empty = harness(
			[{ code: 404, message: 'forbidden', data: null }],
			[ok(job('completed'))]
		);
		const outcome = await empty.controller.submit({ includeHistory: false, projectId: null });
		assert(outcome.status === 'failed', 'failed');
		assertEqual(
			outcome.failure.messageKey,
			'flow.export.error.empty',
			'legacy 404 export = empty scope'
		);
		assertEqual(outcome.failure.forbidden, false, 'message "forbidden" is not read');

		const typed = harness(
			[{ code: 403, message: 'everything is fine', data: null, error_code: 'forbidden' }],
			[ok(job('completed'))]
		);
		const typedOutcome = await typed.controller.submit({ includeHistory: false, projectId: null });
		assert(typedOutcome.status === 'failed' && typedOutcome.failure.forbidden, 'typed forbidden');
		assertEqual(typedOutcome.failure.messageKey, 'flow.error.forbidden', 'typed key');

		const gone = harness([ok(job('running'))], [{ code: 404, message: 'x', data: null }]);
		await gone.controller.submit({ includeHistory: false, projectId: null });
		await settle();
		assertEqual(gone.gets.length, 1, 'a 404 job stops polling');
		assertEqual(gone.controller.snapshot[0].polling, false, 'not polling');
		assert(gone.controller.snapshot[0].failure !== null, 'failure shown');
	}
);

await suite.checkAsync('dispose stops polling', async () => {
	const h = harness([ok(job('running'))], [ok(job('running'))]);
	await h.controller.submit({ includeHistory: false, projectId: null });
	await Promise.resolve();
	h.controller.dispose();
	await settle();
	const after = h.gets.length;
	await settle();
	assertEqual(h.gets.length, after, 'no polls after dispose');
	assert(after <= 2, 'stopped promptly');
});

await suite.checkAsync(
	'recent jobs: per workspace, newest first, max 10, guarded storage',
	async () => {
		const store = new MemoryStore();
		for (let i = 0; i < 12; i += 1) rememberExport(WS, `job-${i}`, store);
		rememberExport(WS, 'job-5', store);
		const list = readRecentExports(WS, store);
		assertEqual(list.length, RECENT_EXPORTS_MAX, 'capped at 10');
		assertEqual(list[0], 'job-5', 'most recent first, deduplicated');
		assertEqual(list.filter((id) => id === 'job-5').length, 1, 'no duplicates');
		assertDeepEqual(readRecentExports('other-ws', store), [], 'keyed by workspace');
		assertEqual(recentExportsKey(WS), `sylvode.flow.exports.${WS}`, 'storage key');
		store.setItem(recentExportsKey(WS), '{not json');
		assertDeepEqual(readRecentExports(WS, store), [], 'garbage reads as empty');
		const throwing: KeyValueStore = {
			getItem: () => {
				throw new Error('blocked');
			},
			setItem: () => {
				throw new Error('blocked');
			}
		};
		assertDeepEqual(readRecentExports(WS, throwing), [], 'throwing read is empty');
		assertDeepEqual(
			rememberExport(WS, 'job-x', throwing),
			['job-x'],
			'throwing write does not throw'
		);
		assertDeepEqual(readRecentExports(WS, null), [], 'no storage at all');
	}
);

await suite.checkAsync(
	'resume re-attaches to remembered jobs and polls them to terminal',
	async () => {
		const store = new MemoryStore();
		rememberExport(WS, 'job-a', store);
		const h = harness([ok(job('completed'))], [ok(job('completed', { job_id: 'job-a' }))], store);
		h.controller.resume();
		await settle();
		assertDeepEqual(h.gets, ['job-a'], 'polled the remembered job once');
		assertEqual(h.controller.snapshot[0].job?.status, 'completed', 'terminal');
		assertEqual(h.posts.length, 0, 'resume creates no export');
	}
);

await suite.checkAsync(
	'download hashes the downloaded bytes locally and compares them with the job checksum',
	async () => {
		const url = '/api/v1/flow/exports/job-1/artifact';
		const blob = new Blob(['zip']);
		const realHash = await sha256HexOfBlob(blob);
		assertEqual(realHash, ZIP_SHA256, 'local hash of "zip" is the known SHA-256');
		const done = job('completed', { download_url: url, checksum: realHash });
		const good = await downloadExport(done, 'f.sylvode-flow.zip', {
			downloadExportArtifact: async () => ok({ blob, sha256: realHash })
		});
		assert(
			good.status === 'ready' && good.blob === blob,
			'bytes, header and checksum agree: ready'
		);
		const uppercase = await downloadExport(
			job('completed', { download_url: url, checksum: realHash.toUpperCase() }),
			'f',
			{ downloadExportArtifact: async () => ok({ blob, sha256: null }) }
		);
		assertEqual(uppercase.status, 'ready', 'hex case does not matter');

		// No header: the bytes alone decide, both ways.
		const noHeaderGood = await downloadExport(done, 'f', {
			downloadExportArtifact: async () => ok({ blob, sha256: null })
		});
		assertEqual(noHeaderGood.status, 'ready', 'no header, matching bytes: ready');
		const tampered = new Blob(['zip!']);
		const noHeaderBad = await downloadExport(done, 'f', {
			downloadExportArtifact: async () => ok({ blob: tampered, sha256: null })
		});
		assert(noHeaderBad.status === 'failed', 'no header, tampered bytes: refused');
		assertEqual(noHeaderBad.failure.messageKey, 'flow.error.checksum_mismatch', 'mismatch key');
		assert(!('blob' in noHeaderBad), 'a refused download hands out no blob');

		// The header is not proof: a header equal to the checksum does not rescue tampered bytes.
		const headerLies = await downloadExport(done, 'f', {
			downloadExportArtifact: async () => ok({ blob: tampered, sha256: realHash })
		});
		assertEqual(headerLies.status, 'failed', 'header == checksum but bytes differ: refused');

		// A header that disagrees with the checksum is still a mismatch.
		const headerDiffers = await downloadExport(done, 'f', {
			downloadExportArtifact: async () => ok({ blob, sha256: 'e'.repeat(64) })
		});
		assertEqual(headerDiffers.status, 'failed', 'header disagrees with checksum: refused');

		// A job without a checksum cannot be verified, so nothing is handed out.
		const noChecksum = await downloadExport(
			job('completed', { download_url: url, checksum: '' }),
			'f',
			{ downloadExportArtifact: async () => ok({ blob, sha256: null }) }
		);
		assertEqual(noChecksum.status, 'failed', 'missing checksum: refused');
		const nullChecksum = await downloadExport(
			{ ...done, checksum: null as unknown as string },
			'f',
			{ downloadExportArtifact: async () => ok({ blob, sha256: null }) }
		);
		assertEqual(nullChecksum.status, 'failed', 'null checksum: refused');

		let called = false;
		const notDone = await downloadExport(job('running', { download_url: url }), 'f', {
			downloadExportArtifact: async () => {
				called = true;
				return ok({ blob, sha256: CHECKSUM });
			}
		});
		assertEqual(notDone.status, 'failed', 'running job cannot be downloaded');
		assertEqual(called, false, 'no request for a job that is not finished');
		assertEqual(
			exportFileName('My WS!', 'abcdef1234'),
			'My-WS-abcdef12.sylvode-flow.zip',
			'file name'
		);
	}
);

await suite.checkAsync('the non-secure-origin SHA-256 fallback matches Web Crypto', async () => {
	const hex = (bytes: Uint8Array) =>
		Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
	for (const length of [0, 1, 3, 55, 56, 63, 64, 65, 119, 1000, 70_000]) {
		const input = new Uint8Array(length);
		for (let i = 0; i < length; i += 1) input[i] = (i * 31 + 7) & 0xff;
		const expected = hex(new Uint8Array(await crypto.subtle.digest('SHA-256', input)));
		assertEqual(hex(sha256Fallback(input)), expected, `fallback == subtle for ${length} bytes`);
	}
});

await suite.checkAsync('every export i18n key exists in zh and en', async () => {
	const root = new URL('..', import.meta.url).pathname;
	for (const locale of ['zh', 'en']) {
		const data = JSON.parse(readFileSync(join(root, 'src/lib/i18n', `${locale}.json`), 'utf8'));
		const lookup = (key: string): unknown =>
			key
				.split('.')
				.reduce<unknown>((node, part) => (node as Record<string, unknown>)?.[part], data);
		const missing = [
			...Object.values(EXPORT_STATUS_KEYS),
			'flow.export.status.unknown',
			...FLOW_PACKAGE_ERROR_KEYS
		].filter((key) => typeof lookup(key) !== 'string');
		assertDeepEqual(missing, [], `${locale}.json missing keys`);
	}
});

finish(suite.result());
