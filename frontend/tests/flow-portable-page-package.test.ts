import { test } from 'node:test';
import assert from 'node:assert/strict';
import { apiClient } from '../src/lib/api/client';
import { flowApi } from '../src/lib/api/flow';

test('portable preview and import send the exact selected ZIP bytes with the preview receipt', async () => {
	const originalFetch = globalThis.fetch;
	const archiveBytes = Uint8Array.from([0x50, 0x4b, 0x03, 0x04, 0xff]);
	const archive = new Blob([archiveBytes], { type: 'application/zip' });
	const checksum = 'a'.repeat(64);
	const seenPaths: string[] = [];
	apiClient.setToken('synthetic-page-session');
	globalThis.fetch = async (input, init) => {
		const path = String(input);
		const headers = new Headers(init?.headers);
		assert.equal(headers.get('Authorization'), 'Bearer synthetic-page-session');
		seenPaths.push(path);
		if (init?.method === 'GET') {
			return new Response(archive, { headers: { 'Content-Type': 'application/zip' } });
		}

		assert.equal(headers.get('Content-Type'), 'application/zip');
		assert(init?.body instanceof Blob);
		assert.deepEqual(new Uint8Array(await init.body.arrayBuffer()), archiveBytes);
		if (path.includes('/preview')) {
			assert.equal(headers.get('Idempotency-Key'), null);
			assert.equal(headers.get('X-Package-SHA256'), null);
			return Response.json({ code: 0, data: { package_sha256: checksum } });
		}
		assert.equal(headers.get('Idempotency-Key'), 'import-intent');
		assert.equal(headers.get('X-Package-SHA256'), checksum);
		return Response.json({ code: 0, data: { object_id: 'new-page' } });
	};

	try {
		const download = await flowApi.exportPortablePage('page');
		assert(download.data);
		const exportReview = await flowApi.previewPortablePage('page', download.data.blob, 'export');
		assert.equal(exportReview.data?.package_sha256, checksum);
		const preview = await flowApi.previewPortablePage('page', download.data.blob);
		assert(preview.data);
		const imported = await flowApi.importPortablePage(
			'page',
			download.data.blob,
			preview.data.package_sha256,
			'import-intent'
		);
		assert.equal(imported.data?.object_id, 'new-page');
		assert.deepEqual(seenPaths, [
			'/api/v2/flow/objects/page/portable-page-package',
			'/api/v2/flow/objects/page/portable-page-package/preview?mode=export',
			'/api/v2/flow/objects/page/portable-page-package/preview',
			'/api/v2/flow/objects/page/portable-page-package'
		]);
	} finally {
		globalThis.fetch = originalFetch;
		apiClient.clearAuth();
	}
});
