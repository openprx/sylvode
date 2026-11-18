import { test } from 'node:test';
import assert from 'node:assert/strict';
import { flowApi } from '../src/lib/api/flow';
import { apiClient } from '../src/lib/api/client';
test('actual body upload adapter sends multipart file/kind, cancellation, progress and unchanged idempotency identity', async () => {
	const original = apiClient.postFormDataWithProgress;
	const file = new File(['fixture'], 'sample.png', { type: 'image/png' });
	const controller = new AbortController();
	let saw = false;
	apiClient.postFormDataWithProgress = async (endpoint, data, headers, signal, progress) => {
		assert.equal(endpoint, '/api/v2/flow/objects/page/body-assets');
		assert.equal(data.get('kind'), 'image');
		assert.equal((data.get('file') as File).name, 'sample.png');
		assert.equal(headers?.['Idempotency-Key'], 'same-intent');
		assert.equal(signal, controller.signal);
		progress?.({ loaded: 3, total: 7 });
		saw = true;
		return { code: 500, message: 'fixture', data: null };
	};
	try {
		let loaded = 0;
		await flowApi.uploadBodyAsset(
			'page',
			'image',
			file,
			'same-intent',
			controller.signal,
			(value) => (loaded = value.loaded)
		);
		assert(saw);
		assert.equal(loaded, 3);
	} finally {
		apiClient.postFormDataWithProgress = original;
	}
});

test('private/public real binary adapters parse HTTP200 SV rejection envelopes but preserve legitimate JSON attachment bytes', async () => {
	const previous = globalThis.fetch;
	const reads = [
		() =>
			flowApi.bodyAsset(
				'11111111-1111-4111-8111-111111111111',
				'22222222-2222-4222-8222-222222222222'
			),
		() => flowApi.publicBodyAsset('synthetic-public-token', '22222222-2222-4222-8222-222222222222')
	];
	try {
		for (const read of reads) {
			globalThis.fetch = async () =>
				new Response(JSON.stringify({ code: 404, message: 'not available', data: null }), {
					status: 200,
					headers: { 'Content-Type': 'application/json' }
				});
			const denied = await read();
			assert.equal(denied.code, 404);
			assert.equal(denied.data, null);
			const contents = '{"code":404,"message":"this is a user file"}';
			globalThis.fetch = async () =>
				new Response(contents, {
					status: 200,
					headers: {
						'Content-Type': 'application/octet-stream',
						'Content-Disposition': 'attachment; filename="user.json"',
						'X-Content-Type-Options': 'nosniff',
						'X-Flow-Asset-SHA256': 'a'.repeat(64),
						'Cache-Control': 'private, no-store'
					}
				});
			const valid = await read();
			assert.equal(valid.code, 0);
			assert.equal(await valid.data?.blob.text(), contents);
			globalThis.fetch = async () =>
				new Response('not an authoritative asset', {
					status: 200,
					headers: { 'Content-Type': 'application/octet-stream' }
				});
			const invalid = await read();
			assert.equal(invalid.data, null);
			assert.notEqual(invalid.code, 0);
		}
	} finally {
		globalThis.fetch = previous;
	}
});
