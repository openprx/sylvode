/**
 * Package import wizard (`ui-surface-v1.md` "Package round-trip UI (v0.8)") and import report.
 *
 * Drives the real `PackageImportWizard` -> `FlowPackageImportSession` -> `flowApi` -> `apiClient`
 * chain with `globalThis.fetch` mocked, so the assertions are about the bytes that would leave
 * the browser: steps cannot be skipped, blocking package errors keep the wizard out of confirm,
 * commit needs the hash acknowledgement and an unexpired preview and sends the preview's own
 * hash, idempotency keys are reused per intent, a cancelled upload leaves no artifact, errors
 * branch on `error_code` (never `message`). The XHR upload path (progress) is exercised against
 * a fake `XMLHttpRequest`.
 *
 * Run standalone: `bun tests/flow-package-wizard.test.ts`
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
	CONFLICT_POLICY_KEYS,
	ESTIMATE_KIND_KEYS,
	EXTERNAL_POLICY_KEYS,
	FILE_REJECTION_KEYS,
	IMPORT_ARCHIVE_BYTES_MAX,
	PREVIEW_HOLD_KEYS,
	PackageImportWizard,
	STEP_KEYS,
	UPLOAD_STATE_KEYS,
	validatePackageFile
} from '../src/lib/flow/package-wizard';
import { FLOW_PACKAGE_ERROR_KEYS, FlowPackageImportSession } from '../src/lib/flow/package-import';
import {
	IMPORT_COUNT_LABEL_KEYS,
	IMPORT_STATUS_KEYS,
	ImportReportPoller,
	type ImportReportState
} from '../src/lib/flow/import-report';
import { DEFAULT_FLOW_LIMITS } from '../src/lib/flow/limits';
import { flowApi } from '../src/lib/api/flow';

interface Call {
	method: string;
	path: string;
	body: unknown;
	idempotencyKey: string | null;
}

type Reply = {
	code: number;
	message?: string;
	data?: unknown;
	error_code?: string;
	details?: unknown;
};

const WS = 'ws-1';
const UPLOAD_HASH = 'a'.repeat(64);
const PREVIEW_HASH = 'c'.repeat(64);
const MAPPING_HASH = 'b'.repeat(64);
const SOURCE_PROJECT = '11111111-1111-4111-8111-111111111111';
const TARGET_PROJECT = '22222222-2222-4222-8222-222222222222';

let calls: Call[] = [];
let replies: Record<string, Reply[]> = {};
let hangUploads = false;

function route(method: string, path: string): string {
	if (path.endsWith('/import-artifacts')) return 'upload';
	if (path.endsWith('/imports/preview')) return 'preview';
	if (path.endsWith('/commit')) return 'commit';
	if (method === 'GET' && path.includes('/flow/imports/')) return 'report';
	return 'other';
}

function reply(kind: string, ...queue: Reply[]): void {
	replies[kind] = queue;
}

function reset(): void {
	calls = [];
	replies = {};
	hangUploads = false;
}

const okUpload: Reply = {
	code: 0,
	data: {
		artifact_id: 'artifact-1',
		package_sha256: UPLOAD_HASH,
		size: 9,
		expires_at: '2099-01-01T00:00:00Z'
	}
};

const MANIFEST = {
	projects: [],
	object_count: 2,
	document_count: 2,
	package_schema: 'sylvode.flow.export-package.v1',
	engine: { name: 'yrs', crate_version: '0.21.0', wire_format_version: 1 }
};

function okPreview(
	expiresAt = '2099-01-01T00:00:00Z',
	previewId = 'preview-1',
	extra: { conflict_details?: unknown[]; conflict_policy?: string } = {}
): Reply {
	return {
		code: 0,
		data: {
			preview_id: previewId,
			package_id: 'package-1',
			package_sha256: PREVIEW_HASH,
			mapping_hash: MAPPING_HASH,
			manifest_summary: MANIFEST,
			mapping: {
				object_map: { s1: 't1', s2: 't2' },
				project_map: {},
				conflict_policy: extra.conflict_policy ?? 'reject_existing'
			},
			conflicts: [],
			conflict_details: extra.conflict_details ?? [],
			warnings: [],
			estimated_changes: { objects: 2, documents: 2, relations: 0 },
			expires_at: expiresAt
		}
	};
}

const okCommit: Reply = {
	code: 0,
	data: { job_id: 'import-1', import_id: 'import-1', status: 'completed' }
};

globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
	const raw = String(input);
	const path = raw.startsWith('http') ? new URL(raw).pathname : raw;
	const method = init?.method ?? 'GET';
	const body =
		init?.body instanceof FormData
			? { filename: (init.body.get('package') as File).name }
			: init?.body
				? JSON.parse(String(init.body))
				: null;
	calls.push({
		method,
		path,
		body,
		idempotencyKey: new Headers(init?.headers).get('Idempotency-Key')
	});
	const kind = route(method, path);
	if (kind === 'upload' && hangUploads) {
		await new Promise<never>((_, reject) => {
			init?.signal?.addEventListener('abort', () =>
				reject(new DOMException('The operation was aborted.', 'AbortError'))
			);
		});
	}
	const queue = replies[kind] ?? [];
	const next = queue.length > 1 ? queue.shift() : queue[0];
	const payload = next ?? { code: 500, message: 'no reply configured' };
	return new Response(JSON.stringify({ message: 'ok', data: null, ...payload }), {
		headers: { 'Content-Type': 'application/json' }
	});
}) as typeof fetch;

function packageFile(name = 'workspace.sylvode-flow.zip'): { blob: Blob; name: string } {
	return { blob: new Blob(['zip-bytes']), name };
}

function newWizard(now = () => Date.parse('2026-10-09T00:00:00Z')): {
	wizard: PackageImportWizard;
	session: FlowPackageImportSession;
	keys: string[];
} {
	const session = new FlowPackageImportSession(WS);
	const keys: string[] = [];
	let n = 0;
	const wizard = new PackageImportWizard(WS, () => {}, {
		session,
		now,
		newKey: () => {
			n += 1;
			const key = `key-${n}`;
			keys.push(key);
			return key;
		}
	});
	return { wizard, session, keys };
}

/** Walks a wizard to the preview step with the configured replies. */
async function toPreview(wizard: PackageImportWizard): Promise<void> {
	const file = packageFile();
	wizard.selectFile(file.blob, file.name);
	assert(await wizard.advance(), 'file -> upload');
	assert(await wizard.startUpload(), 'upload succeeds');
	assert(await wizard.advance(), 'upload -> options');
	assert(await wizard.advance(), 'options -> preview (runs preview)');
}

const suite = new Suite('flow-package-wizard');

await suite.checkAsync('steps cannot be skipped; back keeps entered values', async () => {
	reset();
	reply('upload', okUpload);
	reply('preview', okPreview());
	const { wizard } = newWizard();
	assertEqual(wizard.step, 'file', 'starts at file');
	assertEqual(await wizard.advance(), false, 'no file: cannot advance');
	assertEqual(wizard.goTo('confirm'), false, 'cannot jump forward to confirm');
	assertEqual(wizard.goTo('preview'), false, 'cannot jump forward to preview');
	assertEqual(wizard.step, 'file', 'still on file');
	const file = packageFile();
	wizard.selectFile(file.blob, file.name);
	assert(await wizard.advance(), 'file -> upload');
	assertEqual(await wizard.advance(), false, 'not uploaded: cannot leave upload');
	assertEqual(wizard.goTo('options'), false, 'cannot jump forward to options');
	assertEqual(calls.length, 0, 'nothing was sent before the user started the upload');
	assert(await wizard.startUpload(), 'upload');
	assert(await wizard.advance(), 'upload -> options');
	wizard.setOptions({
		externalReferencePolicy: 'detach',
		projectMapping: [{ sourceProjectId: SOURCE_PROJECT, targetProjectId: TARGET_PROJECT }]
	});
	assert(wizard.back(), 'back to upload');
	assert(wizard.goTo('file'), 'back to file');
	assertEqual(wizard.step, 'file', 'on file');
	assertEqual(
		wizard.snapshot().options.externalReferencePolicy,
		'detach',
		'policy kept after going back'
	);
	assertEqual(
		wizard.snapshot().artifact?.artifact_id,
		'artifact-1',
		'artifact kept after going back'
	);
	assert(await wizard.advance(), 'file -> upload again without re-choosing');
	assert(await wizard.advance(), 'upload -> options again');
	assertEqual(
		wizard.snapshot().options.projectMapping[0]?.targetProjectId,
		TARGET_PROJECT,
		'mapping kept after going back'
	);
	assert(await wizard.advance(), 'options -> preview');
	assertEqual(wizard.step, 'preview', 'on preview');
	assertDeepEqual(
		calls.map((call) => [call.method, call.path]),
		[
			['POST', `/api/v1/workspaces/${WS}/flow/import-artifacts`],
			['POST', `/api/v1/workspaces/${WS}/flow/imports/preview`]
		],
		'exactly one upload and one preview'
	);
	assertDeepEqual(
		calls[1].body,
		{
			artifact_id: 'artifact-1',
			project_mapping: { [SOURCE_PROJECT]: TARGET_PROJECT },
			external_reference_policy: 'detach',
			conflict_policy: 'reject_existing',
			include_history: false,
			idempotency_key: 'key-2'
		},
		'preview request shape'
	);
});

await suite.checkAsync(
	'commit sends the preview hash (not the upload or a local one) after acknowledgement',
	async () => {
		reset();
		reply('upload', okUpload);
		reply('preview', okPreview());
		reply('commit', okCommit);
		const { wizard } = newWizard();
		await toPreview(wizard);
		assert(wizard.canAdvance('preview'), 'preview is complete');
		assert(await wizard.advance(), 'preview -> confirm');
		const before = calls.length;
		assertEqual(wizard.canCommit(), false, 'not acknowledged: cannot commit');
		assertDeepEqual(await wizard.commit(), { status: 'refused' }, 'unacknowledged commit refused');
		assertEqual(calls.length, before, 'an unacknowledged commit sends nothing');
		assertEqual(
			wizard.snapshot().canCommit,
			false,
			'snapshot: not committable before acknowledgement'
		);
		wizard.acknowledgeHash(true);
		assert(wizard.canCommit(), 'acknowledged: can commit');
		assertEqual(wizard.snapshot().canCommit, true, 'snapshot reflects the acknowledgement');
		const outcome = await wizard.commit();
		assertDeepEqual(outcome, { status: 'committed', importId: 'import-1' }, 'committed');
		const commit = calls.at(-1);
		assertEqual(
			commit?.path,
			`/api/v1/workspaces/${WS}/flow/imports/preview-1/commit`,
			'commit path uses preview id'
		);
		assertDeepEqual(
			commit?.body,
			{
				package_sha256: PREVIEW_HASH,
				mapping_hash: MAPPING_HASH,
				conflict_policy: 'reject_existing',
				confirm: true,
				idempotency_key: 'key-3'
			},
			'commit carries the preview hash, mapping hash and policy'
		);
		assertNotEqual(PREVIEW_HASH, UPLOAD_HASH, 'fixture: preview and upload hashes differ');
	}
);

const UNMAPPED_PROJECT = '33333333-3333-4333-8333-333333333333';
const LINEAGE = [
	{ kind: 'lineage_exists', source_object_id: 's1', existing_target_id: 't-old-1' },
	{ kind: 'lineage_exists', source_object_id: 's2', existing_target_id: 't-old-2' }
];

await suite.checkAsync(
	'project_unmapped: back to mapping with one titled, unresolved row per project; re-preview after choosing',
	async () => {
		reset();
		reply('upload', okUpload);
		reply(
			'preview',
			okPreview('2099-01-01T00:00:00Z', 'preview-1', {
				conflict_details: [
					{ kind: 'project_unmapped', source_project_id: UNMAPPED_PROJECT, title: 'Roadmap' }
				]
			}),
			okPreview('2099-01-01T00:00:00Z', 'preview-2')
		);
		reply('commit', okCommit);
		const { wizard } = newWizard();
		const file = packageFile();
		wizard.selectFile(file.blob, file.name);
		assert(await wizard.advance(), 'file -> upload');
		assert(await wizard.startUpload(), 'upload');
		assert(await wizard.advance(), 'upload -> options');
		assertEqual(await wizard.advance(), false, 'the preview found an unmapped project: no move');
		assertEqual(wizard.step, 'options', 'back on the mapping step');
		const snap = wizard.snapshot();
		assertDeepEqual(
			snap.unmappedProjects,
			[{ kind: 'project_unmapped', source_project_id: UNMAPPED_PROJECT, title: 'Roadmap' }],
			'the unmapped projects are listed'
		);
		assertDeepEqual(
			snap.options.projectMapping,
			[
				{
					sourceProjectId: UNMAPPED_PROJECT,
					targetProjectId: null,
					title: 'Roadmap',
					unresolved: true
				}
			],
			'one row per unmapped project, title kept, target not chosen'
		);
		assertEqual(snap.preview, null, 'the preview no longer matches the options and is dropped');
		assertEqual(snap.optionsValid, false, 'an unresolved row is not a valid mapping');
		assertEqual(wizard.canAdvance('options'), false, 'cannot preview before choosing');
		assertEqual(wizard.reachable('confirm'), false, 'confirm unreachable');
		const before = calls.length;
		assertEqual(await wizard.advance(), false, 'refused while unresolved');
		assertEqual(calls.length, before, 'nothing sent while a target is missing');

		// The user picks "unprojected" (null) explicitly.
		wizard.setOptions({
			projectMapping: [
				{
					sourceProjectId: UNMAPPED_PROJECT,
					targetProjectId: null,
					title: 'Roadmap',
					unresolved: false
				}
			]
		});
		assert(wizard.canAdvance('options'), 'chosen: can preview again');
		assert(await wizard.advance(), 'options -> preview');
		assertEqual(wizard.step, 'preview', 'on preview');
		const previews = calls.filter((call) => call.path.endsWith('/imports/preview'));
		assertEqual(previews.length, 2, 'two previews');
		assertDeepEqual(
			(previews[1].body as { project_mapping: unknown }).project_mapping,
			{ [UNMAPPED_PROJECT]: null },
			'the re-preview maps the project explicitly'
		);
		assertNotEqual(
			(previews[0].body as { idempotency_key: string }).idempotency_key,
			(previews[1].body as { idempotency_key: string }).idempotency_key,
			'a new preview key'
		);
		assertEqual(wizard.snapshot().unmappedProjects.length, 0, 'nothing unmapped any more');
		assertEqual(wizard.snapshot().previewHold, null, 'no hold');
		assert(wizard.canAdvance('preview'), 'the second preview can be confirmed');
	}
);

await suite.checkAsync(
	'lineage_exists under reject_existing: preview succeeds but confirm stays closed until a reuse re-preview',
	async () => {
		reset();
		reply('upload', okUpload);
		reply(
			'preview',
			okPreview('2099-01-01T00:00:00Z', 'preview-1', { conflict_details: LINEAGE }),
			okPreview('2099-01-01T00:00:00Z', 'preview-2', {
				conflict_details: LINEAGE,
				conflict_policy: 'reuse_import_lineage'
			})
		);
		reply('commit', okCommit);
		const { wizard } = newWizard();
		await toPreview(wizard);
		const held = wizard.snapshot();
		assertEqual(held.previewState, 'ready', 'the preview itself succeeded');
		assertEqual(held.failure, null, 'no error is shown for a successful preview');
		assertEqual(held.previewHold, 'lineage_exists', 'held for the existing lineage');
		assertEqual(held.canAdvance, false, 'snapshot: cannot advance');
		assertEqual(wizard.canAdvance('preview'), false, 'cannot advance to confirm');
		assertEqual(await wizard.advance(), false, 'advance refused');
		assertEqual(wizard.goTo('confirm'), false, 'no jump to confirm');
		assertEqual(wizard.reachable('confirm'), false, 'confirm unreachable');
		assertDeepEqual(await wizard.commit(), { status: 'refused' }, 'commit refused');
		assertEqual(
			calls.filter((call) => call.path.endsWith('/commit')).length,
			0,
			'no commit sent while held'
		);

		assert(await wizard.previewWithReuse(), 'previewed again with reuse');
		const previews = calls.filter((call) => call.path.endsWith('/imports/preview'));
		assertEqual(previews.length, 2, 'a second preview');
		assertEqual(
			(previews[1].body as { conflict_policy: string }).conflict_policy,
			'reuse_import_lineage',
			'the re-preview asks for reuse'
		);
		assertNotEqual(
			(previews[0].body as { idempotency_key: string }).idempotency_key,
			(previews[1].body as { idempotency_key: string }).idempotency_key,
			'new key for the new intent'
		);
		assertEqual(wizard.snapshot().previewHold, null, 'reuse: the lineage no longer holds');
		assertEqual(wizard.step, 'preview', 'still on preview');
		assert(await wizard.advance(), 'preview -> confirm');
		wizard.acknowledgeHash(true);
		const outcome = await wizard.commit();
		assertEqual(outcome.status, 'committed', 'committed');
		assertEqual(
			(calls.at(-1)?.body as { conflict_policy: string }).conflict_policy,
			'reuse_import_lineage',
			'the commit carries the reuse policy'
		);
		assertEqual(await new PackageImportWizard(WS).previewWithReuse(), false, 'no preview: refused');
	}
);

await suite.checkAsync(
	'lineage_exists under reuse_import_lineage does not hold the preview',
	async () => {
		reset();
		reply('upload', okUpload);
		reply(
			'preview',
			okPreview('2099-01-01T00:00:00Z', 'preview-1', {
				conflict_details: LINEAGE,
				conflict_policy: 'reuse_import_lineage'
			})
		);
		const { wizard } = newWizard();
		const file = packageFile();
		wizard.selectFile(file.blob, file.name);
		await wizard.advance();
		await wizard.startUpload();
		await wizard.advance();
		wizard.setOptions({ conflictPolicy: 'reuse_import_lineage' });
		assert(await wizard.advance(), 'options -> preview');
		assertEqual(wizard.snapshot().previewHold, null, 'no hold under reuse');
		assert(wizard.canAdvance('preview'), 'can confirm');
		assertEqual(await wizard.previewWithReuse(), false, 'nothing to switch');
	}
);

await suite.checkAsync(
	'commit rejections: project_unmapped returns to mapping, policy_rejected is the lineage refusal; message never read',
	async () => {
		reset();
		reply('upload', okUpload);
		reply('preview', okPreview());
		reply('commit', {
			code: 400,
			error_code: 'invalid_update',
			details: { reason: 'project_unmapped' },
			message: 'policy_rejected'
		});
		const first = newWizard().wizard;
		await toPreview(first);
		assert(await first.advance(), 'preview -> confirm');
		first.acknowledgeHash(true);
		const unmapped = await first.commit();
		assertEqual(unmapped.status, 'failed', 'failed');
		assertEqual(
			first.snapshot().failure?.messageKey,
			'flow.import.error.projectUnmapped',
			'project_unmapped wording'
		);
		assertEqual(first.step, 'options', 'back on the mapping step');
		assertEqual(first.snapshot().preview, null, 'the preview is dropped');

		reset();
		reply('upload', okUpload);
		reply('preview', okPreview());
		reply('commit', { code: 403, error_code: 'policy_rejected', message: 'project_unmapped' });
		const second = newWizard().wizard;
		await toPreview(second);
		assert(await second.advance(), 'preview -> confirm');
		second.acknowledgeHash(true);
		await second.commit();
		const snap = second.snapshot();
		assertEqual(snap.failure?.messageKey, 'flow.import.error.lineageRejected', 'lineage wording');
		assertEqual(
			snap.blockingFailure?.messageKey,
			'flow.import.error.lineageRejected',
			'blocks until a new preview'
		);
		assertEqual(snap.canCommit, false, 'cannot commit again');
		assertEqual(second.step, 'confirm', 'stays on confirm');

		// An invalid_update with another reason keeps the generic stable-code wording.
		reset();
		reply('upload', okUpload);
		reply('preview', okPreview());
		reply('commit', {
			code: 400,
			error_code: 'invalid_update',
			details: { reason: 'idempotency_key_body_drift' }
		});
		const third = newWizard().wizard;
		await toPreview(third);
		assert(await third.advance(), 'preview -> confirm');
		third.acknowledgeHash(true);
		await third.commit();
		assertEqual(third.snapshot().failure?.messageKey, 'flow.error.invalid_update', 'generic');
		assertEqual(third.step, 'confirm', 'no mapping detour');
	}
);

await suite.checkAsync('the preview exposes manifest_summary as the server sent it', async () => {
	reset();
	reply('upload', okUpload);
	reply('preview', okPreview());
	const { wizard } = newWizard();
	await toPreview(wizard);
	assertDeepEqual(wizard.snapshot().preview?.manifest_summary, MANIFEST, 'summary passed through');
});

await suite.checkAsync(
	'checksum_mismatch at preview blocks confirm; message is never read',
	async () => {
		reset();
		reply('upload', okUpload);
		reply('preview', { code: 400, error_code: 'checksum_mismatch', message: 'all good, continue' });
		const { wizard } = newWizard();
		await toPreview(wizard);
		const snap = wizard.snapshot();
		assertEqual(snap.blockingFailure?.messageKey, 'flow.error.checksum_mismatch', 'blocking key');
		assertEqual(
			wizard.canAdvance('preview'),
			false,
			'checksum_mismatch: cannot advance to confirm'
		);
		assertEqual(await wizard.advance(), false, 'advance refused');
		assertEqual(wizard.step, 'preview', 'still on preview');
		// Re-running preview with the same failure keeps the block on screen.
		await wizard.runPreview();
		assertEqual(
			wizard.snapshot().blockingFailure?.messageKey,
			'flow.error.checksum_mismatch',
			'still blocked'
		);
	}
);

await suite.checkAsync('unsupported_format and policy_rejected at preview also block', async () => {
	for (const code of ['unsupported_format', 'policy_rejected']) {
		reset();
		reply('upload', okUpload);
		reply('preview', { code: 400, error_code: code });
		const { wizard } = newWizard();
		await toPreview(wizard);
		assertEqual(
			wizard.snapshot().blockingFailure?.messageKey,
			`flow.error.${code}`,
			`${code} blocks`
		);
		assertEqual(wizard.canAdvance('preview'), false, `${code}: no confirm`);
	}
});

await suite.checkAsync('checksum_mismatch at commit blocks a second commit', async () => {
	reset();
	reply('upload', okUpload);
	reply('preview', okPreview());
	reply('commit', { code: 400, error_code: 'checksum_mismatch' });
	const { wizard } = newWizard();
	await toPreview(wizard);
	await wizard.advance();
	wizard.acknowledgeHash(true);
	const outcome = await wizard.commit();
	assertEqual(outcome.status, 'failed', 'commit failed');
	assertEqual(
		wizard.snapshot().blockingFailure?.messageKey,
		'flow.error.checksum_mismatch',
		'blocked'
	);
	assertEqual(wizard.canCommit(), false, 'cannot commit again');
	assertEqual(wizard.canAdvance('preview'), false, 'preview no longer leads to confirm');
	const before = calls.length;
	assertDeepEqual(await wizard.commit(), { status: 'refused' }, 'second commit refused');
	assertEqual(calls.length, before, 'no request after a blocking commit failure');
});

await suite.checkAsync('an expired preview cannot be committed', async () => {
	reset();
	reply('upload', okUpload);
	reply('preview', okPreview('2026-10-09T00:30:00Z'));
	let now = Date.parse('2026-10-09T00:00:00Z');
	const { wizard } = newWizard(() => now);
	await toPreview(wizard);
	await wizard.advance();
	wizard.acknowledgeHash(true);
	assert(wizard.canCommit(), 'fresh preview: can commit');
	assertEqual(wizard.secondsUntilExpiry(), 1800, '30 minutes left');
	now = Date.parse('2026-10-09T00:30:00Z');
	assertEqual(wizard.snapshot().expired, true, 'expired');
	assertEqual(wizard.canCommit(), false, 'expired: cannot commit');
	assertEqual(wizard.canAdvance('preview'), false, 'expired: preview does not lead to confirm');
	const before = calls.length;
	assertDeepEqual(await wizard.commit(), { status: 'refused' }, 'commit refused');
	assertEqual(calls.length, before, 'expired commit sends nothing');
});

await suite.checkAsync(
	'idempotency keys: retries reuse, changed intents mint new ones',
	async () => {
		reset();
		reply('upload', { code: 503 }, okUpload);
		reply('preview', { code: 500 }, okPreview(), okPreview('2099-01-01T00:00:00Z', 'preview-2'));
		reply('commit', { code: 502 }, okCommit);
		const { wizard } = newWizard();
		const file = packageFile();
		wizard.selectFile(file.blob, file.name);
		await wizard.advance();
		assertEqual(await wizard.startUpload(), false, 'first upload fails');
		assertEqual(wizard.snapshot().failure?.retryable, true, 'upload failure is retryable');
		assert(await wizard.startUpload(), 'retry succeeds');
		const uploads = calls.filter((call) => call.path.endsWith('/import-artifacts'));
		assertEqual(uploads.length, 2, 'two upload attempts');
		assertEqual(
			uploads[0].idempotencyKey,
			uploads[1].idempotencyKey,
			'upload retry reuses the key'
		);
		await wizard.advance();
		await wizard.advance();
		assertEqual(wizard.snapshot().previewState, 'failed', 'first preview failed');
		await wizard.runPreview();
		const previews = () => calls.filter((call) => call.path.endsWith('/imports/preview'));
		assertEqual(
			(previews()[0].body as { idempotency_key: string }).idempotency_key,
			(previews()[1].body as { idempotency_key: string }).idempotency_key,
			'preview retry reuses the key'
		);
		wizard.goTo('options');
		wizard.setOptions({ conflictPolicy: 'reuse_import_lineage' });
		assertEqual(wizard.snapshot().preview, null, 'changed options drop the old preview');
		await wizard.advance();
		assertNotEqual(
			(previews()[2].body as { idempotency_key: string }).idempotency_key,
			(previews()[1].body as { idempotency_key: string }).idempotency_key,
			'changed options use a new preview key'
		);
		await wizard.advance();
		wizard.acknowledgeHash(true);
		assertEqual((await wizard.commit()).status, 'failed', 'first commit fails');
		assertEqual((await wizard.commit()).status, 'committed', 'retry commits');
		const commits = calls.filter((call) => call.path.endsWith('/commit'));
		assertEqual(commits.length, 2, 'two commit attempts');
		assertEqual(
			(commits[0].body as { idempotency_key: string }).idempotency_key,
			(commits[1].body as { idempotency_key: string }).idempotency_key,
			'commit retry reuses the key'
		);
		assertEqual(
			(commits[1].body as { conflict_policy: string }).conflict_policy,
			'reuse_import_lineage',
			'commit carries the policy the preview was computed with'
		);
	}
);

await suite.checkAsync(
	'cancelling the upload clears the session and nothing can be previewed',
	async () => {
		reset();
		hangUploads = true;
		const { wizard, session } = newWizard();
		const file = packageFile();
		wizard.selectFile(file.blob, file.name);
		await wizard.advance();
		const pending = wizard.startUpload();
		assertEqual(wizard.snapshot().uploadState, 'uploading', 'uploading');
		wizard.cancelUpload();
		assertEqual(await pending, false, 'upload did not complete');
		const snap = wizard.snapshot();
		assertEqual(snap.uploadState, 'cancelled', 'cancelled');
		assertEqual(snap.artifact, null, 'no artifact');
		assertEqual(session.hasArtifact, false, 'session holds no artifact');
		assertEqual(session.hasPreview, false, 'session holds no preview');
		assertEqual(wizard.canAdvance('upload'), false, 'cannot leave upload');
		const before = calls.length;
		assertEqual(await wizard.runPreview(), false, 'preview refused');
		assertEqual(calls.length, before, 'no preview request after cancel');
	}
);

await suite.checkAsync(
	'package bytes are dropped after upload and never kept by the session',
	async () => {
		reset();
		reply('upload', okUpload);
		reply('preview', okPreview());
		const { wizard, session } = newWizard();
		await toPreview(wizard);
		const holdsBlob = (value: object) =>
			Object.values(value as Record<string, unknown>).some((field) => field instanceof Blob);
		assertEqual(holdsBlob(wizard), false, 'wizard keeps no Blob after upload');
		assertEqual(holdsBlob(session), false, 'session keeps no Blob');
	}
);

await suite.checkAsync(
	'file choice: extension, empty and size limit are checked locally',
	async () => {
		assertEqual(
			IMPORT_ARCHIVE_BYTES_MAX,
			DEFAULT_FLOW_LIMITS.importArchiveBytesMax,
			'limit source'
		);
		assertEqual(IMPORT_ARCHIVE_BYTES_MAX, 134_217_728, 'limits-v1 import_archive_bytes_max');
		assertEqual(validatePackageFile('a.zip', 10), 'extension', '.zip alone is not accepted');
		assertEqual(validatePackageFile('a.sylvode-flow.zip', 0), 'empty', 'empty');
		assertEqual(
			validatePackageFile('a.sylvode-flow.zip', IMPORT_ARCHIVE_BYTES_MAX),
			null,
			'at the limit'
		);
		assertEqual(
			validatePackageFile('a.sylvode-flow.zip', IMPORT_ARCHIVE_BYTES_MAX + 1),
			'too_large',
			'over'
		);
		assertEqual(validatePackageFile('A.SYLVODE-FLOW.ZIP', 1), null, 'extension check ignores case');
		reset();
		const { wizard } = newWizard();
		const big = { size: IMPORT_ARCHIVE_BYTES_MAX + 1 } as Blob;
		assertEqual(wizard.selectFile(big, 'big.sylvode-flow.zip'), 'too_large', 'too large rejected');
		assertEqual(wizard.canAdvance('file'), false, 'rejected file cannot advance');
		assertEqual(calls.length, 0, 'nothing sent for a rejected file');
	}
);

await suite.checkAsync('legacy rejections are classified by code, never by message', async () => {
	reset();
	reply('upload', okUpload);
	reply('preview', { code: 409, message: 'checksum_mismatch' });
	const conflictWizard = newWizard().wizard;
	await toPreview(conflictWizard);
	const conflict = conflictWizard.snapshot();
	assertEqual(
		conflict.failure?.messageKey,
		'flow.import.error.conflict',
		'409 preview = lineage conflict'
	);
	assertEqual(
		conflict.blockingFailure,
		null,
		'a message saying checksum_mismatch is not a checksum error'
	);
	assertEqual(conflictWizard.canAdvance('preview'), false, 'conflict: no confirm');
	reset();
	reply('upload', okUpload);
	reply('preview', { code: 400, message: 'unsupported_format' });
	const rejectedWizard = newWizard().wizard;
	await toPreview(rejectedWizard);
	assertEqual(
		rejectedWizard.snapshot().failure?.messageKey,
		'flow.import.error.previewRejected',
		'legacy 400 preview'
	);
	assertEqual(
		rejectedWizard.snapshot().blockingFailure,
		null,
		'legacy 400 is not unsupported_format'
	);
});

await suite.checkAsync('XHR upload path: shape, progress, envelope, abort', async () => {
	const instances: FakeXhr[] = [];
	class FakeXhr {
		method = '';
		url = '';
		headers: Record<string, string> = {};
		withCredentials = false;
		body: FormData | null = null;
		aborted = false;
		responseText = '';
		upload: { onprogress: ((event: ProgressEvent) => void) | null } = { onprogress: null };
		onload: (() => void) | null = null;
		onerror: (() => void) | null = null;
		onabort: (() => void) | null = null;
		constructor() {
			instances.push(this);
		}
		open(method: string, url: string) {
			this.method = method;
			this.url = url;
		}
		setRequestHeader(name: string, value: string) {
			this.headers[name] = value;
		}
		send(body: FormData) {
			this.body = body;
		}
		abort() {
			this.aborted = true;
			this.onabort?.();
		}
	}
	const globals = globalThis as unknown as { XMLHttpRequest?: unknown };
	globals.XMLHttpRequest = FakeXhr;
	try {
		const progress: Array<[number, number | null]> = [];
		const pending = flowApi.uploadPackageArtifact(
			WS,
			new Blob(['zip-bytes']),
			'w.sylvode-flow.zip',
			'upload-key',
			undefined,
			(event) => progress.push([event.loaded, event.total])
		);
		const xhr = instances[0];
		assert(xhr, 'an XHR was opened');
		assertEqual(xhr.method, 'POST', 'method');
		assertEqual(xhr.url, `/api/v1/workspaces/${WS}/flow/import-artifacts`, 'url');
		assertEqual(xhr.headers['Idempotency-Key'], 'upload-key', 'idempotency header');
		assertEqual(xhr.withCredentials, true, 'credentials');
		assertEqual(
			(xhr.body?.get('package') as File).name,
			'w.sylvode-flow.zip',
			'single package field'
		);
		assertDeepEqual([...(xhr.body?.keys() ?? [])], ['package'], 'exactly one multipart field');
		xhr.upload.onprogress?.({ loaded: 4, total: 9, lengthComputable: true } as ProgressEvent);
		xhr.upload.onprogress?.({ loaded: 9, total: 0, lengthComputable: false } as ProgressEvent);
		xhr.responseText = JSON.stringify({
			code: 400,
			message: 'fine',
			data: null,
			error_code: 'unsupported_format'
		});
		xhr.onload?.();
		const result = await pending;
		assertDeepEqual(
			progress,
			[
				[4, 9],
				[9, null]
			],
			'progress forwarded'
		);
		assertEqual(result.error_code, 'unsupported_format', 'typed envelope normalised');
		assertEqual(result.data, null, 'no data on rejection');

		const controller = new AbortController();
		const aborted = flowApi.uploadPackageArtifact(
			WS,
			new Blob(['x']),
			'x.sylvode-flow.zip',
			'k2',
			controller.signal,
			() => {}
		);
		controller.abort();
		const abortedResult = await aborted;
		assertEqual(instances[1].aborted, true, 'abort signal aborts the XHR');
		assertNotEqual(abortedResult.code, 0, 'aborted upload is not a success');
	} finally {
		delete globals.XMLHttpRequest;
	}
});

await suite.checkAsync(
	'import report: polls through "not completed", stops at terminal, safe not-found',
	async () => {
		const sleeps: number[] = [];
		const report = {
			import_id: 'import-1',
			status: 'completed',
			counts: { planned: 2, created: 2, reused: 0, detached: 0, failed: 0 }
		};
		const queue = [
			{ code: 409, message: 'done', data: null },
			{ code: 409, message: 'done', data: null },
			{ code: 0, message: 'ok', data: report }
		];
		let gets = 0;
		const states: ImportReportState[] = [];
		const poller = new ImportReportPoller(WS, 'import-1', (state) => states.push(state), {
			reads: {
				getImportReport: async () => {
					gets += 1;
					return (queue.shift() ?? { code: 0, message: 'ok', data: report }) as never;
				}
			},
			sleep: async (ms) => {
				sleeps.push(ms);
			}
		});
		const final = await poller.run();
		assertEqual(final.status, 'ready', 'ready');
		assertEqual(gets, 3, 'stopped at terminal');
		assertDeepEqual(sleeps, [1000, 2000], 'backoff between polls');
		assert(
			states.some((state) => state.status === 'pending'),
			'pending shown while not completed'
		);

		for (const envelope of [{ code: 404 }, { code: 403 }, { code: 404, error_code: 'not_found' }]) {
			const notFound = new ImportReportPoller(WS, 'x', () => {}, {
				reads: {
					getImportReport: async () => ({ message: 'ok', data: null, ...envelope }) as never
				},
				sleep: async () => {}
			});
			assertEqual(
				(await notFound.run()).status,
				'not_found',
				`${JSON.stringify(envelope)} -> not_found`
			);
		}
	}
);

await suite.checkAsync(
	'package calls go through the adapters: writes via CommandService, reads via ObjectRepository',
	async () => {
		const calls: string[] = [];
		const session = new FlowPackageImportSession(WS, {
			uploadImportPackage: async (workspaceId, _file, filename, key) => {
				calls.push(`upload ${workspaceId} ${filename} ${key}`);
				return {
					code: 0,
					message: 'ok',
					data: { artifact_id: 'artifact-1' } as never
				};
			},
			previewImport: async (workspaceId, input) => {
				calls.push(`preview ${workspaceId} ${input.artifact_id} ${input.idempotency_key}`);
				return { message: 'ok', ...okPreview() } as never;
			},
			commitImport: async (workspaceId, previewId, input) => {
				calls.push(
					`commit ${workspaceId} ${previewId} ${input.package_sha256} ${input.idempotency_key}`
				);
				return { message: 'ok', ...okCommit } as never;
			}
		});
		await session.upload(new Blob(['pkg']), 'a.sylvode-flow.zip', 'k-up');
		await session.preview({
			externalReferencePolicy: 'detach',
			conflictPolicy: 'reject_existing',
			includeHistory: false,
			idempotencyKey: 'k-pre'
		});
		await session.commit({ exactPackageSha256: PREVIEW_HASH, idempotencyKey: 'k-com' });
		assertDeepEqual(
			calls,
			[
				`upload ${WS} a.sylvode-flow.zip k-up`,
				`preview ${WS} artifact-1 k-pre`,
				`commit ${WS} preview-1 ${PREVIEW_HASH} k-com`
			],
			'the injected CommandService carried all three writes'
		);

		// No package module talks to `flowApi` at run time; each goes through an adapter.
		const root = new URL('..', import.meta.url).pathname;
		for (const file of [
			'package-import.ts',
			'package-export.ts',
			'import-report.ts',
			'package-wizard.ts'
		]) {
			const source = readFileSync(join(root, 'src/lib/flow', file), 'utf8');
			assert(!/\bflowApi\s*\./.test(source), `${file} calls flowApi directly`);
			assert(
				!/import\s*\{[^}]*\bflowApi\b[^}]*\}\s*from\s*'\$lib\/api\/flow'/.test(source),
				`${file} imports the flowApi value`
			);
		}
		const commands = readFileSync(join(root, 'src/lib/flow/command-service.ts'), 'utf8');
		for (const method of [
			'startWorkspaceExport',
			'uploadImportPackage',
			'previewImport',
			'commitImport'
		]) {
			assert(new RegExp(`async ${method}\\(`).test(commands), `CommandService.${method}`);
		}
		const repository = readFileSync(join(root, 'src/lib/flow/object-repository.ts'), 'utf8');
		for (const method of ['getExportJob', 'downloadExportArtifact', 'getImportReport']) {
			assert(new RegExp(`\\n\\t${method}\\(`).test(repository), `ObjectRepository.${method}`);
		}
	}
);

await suite.checkAsync('every wizard/report i18n key exists in zh and en', async () => {
	const root = new URL('..', import.meta.url).pathname;
	const flat = (locale: string): Set<string> => {
		const out = new Set<string>();
		const walk = (node: unknown, prefix: string) => {
			if (typeof node !== 'object' || node === null) {
				out.add(prefix);
				return;
			}
			for (const [k, v] of Object.entries(node)) walk(v, prefix ? `${prefix}.${k}` : k);
		};
		walk(JSON.parse(readFileSync(join(root, 'src/lib/i18n', `${locale}.json`), 'utf8')), '');
		return out;
	};
	const keys = [
		...Object.values(STEP_KEYS),
		...Object.values(FILE_REJECTION_KEYS),
		...Object.values(UPLOAD_STATE_KEYS),
		...Object.values(EXTERNAL_POLICY_KEYS),
		...Object.values(CONFLICT_POLICY_KEYS),
		...Object.values(ESTIMATE_KIND_KEYS),
		...Object.values(IMPORT_COUNT_LABEL_KEYS),
		...Object.values(IMPORT_STATUS_KEYS),
		...FLOW_PACKAGE_ERROR_KEYS,
		...Object.values(PREVIEW_HOLD_KEYS),
		'flow.import.preview.reuseAction',
		'flow.import.preview.manifest.heading',
		'flow.import.preview.manifest.engineValue',
		'flow.import.preview.conflict.lineage',
		'flow.import.preview.conflict.unmapped',
		'flow.import.options.unmappedNotice',
		'flow.import.options.chooseTarget',
		'flow.import.options.targetFor',
		'flow.import.live.unmapped',
		'flow.error.checksum_mismatch',
		'flow.error.unsupported_format'
	];
	for (const locale of ['zh', 'en']) {
		const have = flat(locale);
		const missing = keys.filter((key) => !have.has(key));
		assertDeepEqual(missing, [], `${locale}.json missing keys`);
	}
});

finish(suite.result());
