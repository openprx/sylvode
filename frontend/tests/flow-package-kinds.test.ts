import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
	buildWorkspaceExportRequest,
	selectedPackageObjectTypes,
	PackageExportController
} from '../src/lib/flow/package-export';
import { classifyPackageFailure } from '../src/lib/flow/package-import';
test('explicit supported kinds are sent while omitted legacy selection stays omitted; unknown kinds never silently drop', () => {
	assert.deepEqual(
		buildWorkspaceExportRequest(
			{ includeHistory: false, projectId: null, objectTypes: ['navigator', 'page', 'page'] },
			'intent'
		),
		{
			format: 'package',
			include_history: false,
			object_types: ['page', 'navigator'],
			idempotency_key: 'intent'
		}
	);
	assert(
		!(
			'object_types' in
			buildWorkspaceExportRequest({ includeHistory: false, projectId: null }, 'legacy')
		)
	);
	assert.throws(() => selectedPackageObjectTypes([]), /invalid_package_object_types/);
	assert.throws(
		() => selectedPackageObjectTypes(['page', 'collection']),
		/invalid_package_object_types/
	);
});
test('kind changes get a new retry intent while reordered equivalent filters retain it', () => {
	let next = 0;
	const controller = new PackageExportController('scope', () => {}, {
		newKey: () => `intent-${++next}`,
		store: null
	});
	const base = { includeHistory: false, projectId: null };
	const both = controller.keyFor({ ...base, objectTypes: ['page', 'navigator'] });
	assert.equal(controller.keyFor({ ...base, objectTypes: ['navigator', 'page'] }), both);
	assert.notEqual(controller.keyFor({ ...base, objectTypes: ['page'] }), both);
	controller.dispose();
});
test('typed raw package refusal selects a precise blocking UI state without prose-message branching', () => {
	const refused = classifyPackageFailure(
		{
			code: 400,
			error_code: 'unsupported_format',
			details: {
				reason: 'typed_package_roundtrip_unsupported',
				unsupported_object_types: ['collection', 'record']
			}
		},
		'preview'
	);
	assert.equal(refused.messageKey, 'flow.package.error.typedRoundtripUnsupported');
	assert.equal(refused.blocking, true);
	assert.equal(refused.retryable, false);
	const invalid = classifyPackageFailure(
		{
			code: 400,
			error_code: 'invalid_update',
			details: { reason: 'invalid_package_object_types' }
		},
		'export'
	);
	assert.equal(invalid.messageKey, 'flow.package.error.invalidObjectTypes');
	const generic = classifyPackageFailure(
		{ code: 400, error_code: 'unsupported_format', details: { reason: 'different_format_issue' } },
		'preview'
	);
	assert.notEqual(generic.messageKey, refused.messageKey);
});

test('real HTTP adapter serializes the explicit supported-kind filter on the canonical scope request', async () => {
	const { flowApi } = await import('../src/lib/api/flow');
	const saved = globalThis.fetch;
	let captured: { url: string; body: unknown } | null = null;
	globalThis.fetch = (async (url, init) => {
		captured = { url: String(url), body: JSON.parse(String(init?.body)) };
		return new Response(
			JSON.stringify({
				code: 0,
				message: 'ok',
				data: { job_id: 'synthetic-job', status: 'started' }
			}),
			{ headers: { 'Content-Type': 'application/json' } }
		);
	}) as typeof fetch;
	try {
		const request = buildWorkspaceExportRequest(
			{ includeHistory: false, projectId: null, objectTypes: ['page'] },
			'explicit-page-intent'
		);
		await flowApi.exportWorkspace('canonical-scope', request);
		assert.deepEqual(captured, {
			url: '/api/v2/flow/scopes/canonical-scope/exports',
			body: {
				format: 'package',
				include_history: false,
				object_types: ['page'],
				idempotency_key: 'explicit-page-intent'
			}
		});
	} finally {
		globalThis.fetch = saved;
	}
});

test('only actual server warnings describe filtered/omitted kinds; empty old receipts never imply complete scope', async () => {
	const { packageExportWarnings } = await import('../src/lib/flow/package-warnings');
	assert.deepEqual(
		packageExportWarnings(
			['object_type_filter_applied:page,navigator', 'object_types_omitted:collection,record'],
			true
		),
		[
			{ messageKey: 'flow.package.filterApplied', objectTypes: ['page', 'navigator'] },
			{ messageKey: 'flow.package.typesOmitted', objectTypes: ['collection', 'record'] }
		]
	);
	assert.deepEqual(packageExportWarnings([], true), [
		{ messageKey: 'flow.package.selectionUnrecorded', objectTypes: [] }
	]);
	assert.deepEqual(packageExportWarnings(undefined, false), []);
	for (const warning of [
		'object_types_omitted:constructor',
		'object_types_omitted:record:invented',
		'object_types_omitted:unknown',
		'object_type_filter_applied:collection'
	])
		assert.deepEqual(packageExportWarnings([warning]), [
			{ messageKey: 'flow.package.additionalWarning', objectTypes: [] }
		]);
});
