import { test } from 'node:test';
import assert from 'node:assert/strict';
import { flowApi } from '../src/lib/api/flow';
import { portablePageFailure } from '../src/lib/flow/portable-page-package';

test('portable graph submits typed selections and binds import bytes to the review digest', async () => {
	const original = globalThis.fetch;
	const calls: Array<{ path: string; init?: RequestInit }> = [];
	const archive = new Blob([new Uint8Array([80, 75, 3, 4])], { type: 'application/zip' });
	const digest = 'a'.repeat(64);
	const pages = [
		{
			source_object_id: 'source',
			source_parent_id: null,
			title: 'Page',
			asset_count: 0,
			version_count: 2,
			source_sequences: [3, 7]
		}
	];
	try {
		globalThis.fetch = async (input, init) => {
			const path = String(input);
			calls.push({ path, init });
			if (path.endsWith('/export')) return new Response(archive);
			if (path.endsWith('/preview'))
				return Response.json({
					code: 0,
					message: 'ok',
					data: { package_sha256: digest, pages, forms: [], include_history: true, references: [] }
				});
			return Response.json({
				code: 0,
				message: 'ok',
				data: { pages: [], forms: [], include_history: true }
			});
		};
		const selection = { page_ids: ['source', 'child'], form_ids: ['form'], include_history: true };
		const exported = await flowApi.exportPortableGraph('source', selection);
		assert.equal(exported.code, 0);
		assert.equal(calls[0]?.path, '/api/v2/flow/objects/source/portable-graph-package/export');
		assert.equal(calls[0]?.init?.body, JSON.stringify(selection));
		assert.equal(new Headers(calls[0]?.init?.headers).get('content-type'), 'application/json');
		const preview = await flowApi.previewPortableGraph('destination', archive);
		assert.deepEqual(preview.data?.pages[0]?.source_sequences, [3, 7]);
		assert.equal(calls[1]?.init?.body, archive);
		await flowApi.importPortableGraph('destination', archive, digest, 'same-key');
		assert.equal(calls[2]?.path, '/api/v2/flow/objects/destination/portable-graph-package');
		assert.equal(calls[2]?.init?.body, archive);
		assert.equal(new Headers(calls[2]?.init?.headers).get('X-Package-SHA256'), digest);
		assert.equal(new Headers(calls[2]?.init?.headers).get('Idempotency-Key'), 'same-key');
	} finally {
		globalThis.fetch = original;
	}
});

test('a package reference outside the selection produces an actionable typed error', () => {
	assert.equal(
		portablePageFailure({
			code: 403,
			message: '',
			data: null,
			details: { reason: 'portable_graph_target_not_selected' }
		}),
		'flow.graphPackage.targetMissing'
	);
});
