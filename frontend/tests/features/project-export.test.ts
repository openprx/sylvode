import { test } from 'node:test';
import assert from 'node:assert/strict';
import { apiClient } from '../../src/lib/api/client';
import { filenameFromDisposition, importExportApi } from '../../src/lib/api/import-export';

const realFetch = globalThis.fetch;

function answer(body: string, headers: Record<string, string>): void {
	globalThis.fetch = (async () => new Response(body, { status: 200, headers })) as typeof fetch;
}

test('the file name comes from Content-Disposition', () => {
	assert.equal(
		filenameFromDisposition('attachment; filename="project_SV_export.json"', 'x'),
		'project_SV_export.json'
	);
	assert.equal(filenameFromDisposition("attachment; filename*=UTF-8''p%C3%A9.csv", 'x'), 'pé.csv');
	assert.equal(filenameFromDisposition('attachment; filename=a.csv', 'x'), 'a.csv');
	assert.equal(filenameFromDisposition('attachment', 'fallback.json'), 'fallback.json');
	assert.equal(filenameFromDisposition(null, 'fallback.json'), 'fallback.json');
});

test('a JSON attachment from the export route is the file, not an envelope', async () => {
	assert.ok(apiClient);
	try {
		answer('{"format":"sylvode.project_export"}', {
			'Content-Type': 'application/json; charset=utf-8',
			'Content-Disposition': 'attachment; filename="project_SV_export.json"'
		});
		const result = await importExportApi.exportProject('p1', 'json');
		assert.equal(result.code, 0);
		assert.equal(result.data?.filename, 'project_SV_export.json');
		assert.equal(await result.data?.blob.text(), '{"format":"sylvode.project_export"}');
	} finally {
		globalThis.fetch = realFetch;
	}
});

test('a CSV attachment is the file', async () => {
	try {
		answer('a,b\n1,2\n', {
			'Content-Type': 'text/csv; charset=utf-8',
			'Content-Disposition': 'attachment; filename="project_SV_export.csv"'
		});
		const result = await importExportApi.exportProject('p1', 'csv');
		assert.equal(result.code, 0);
		assert.equal(result.data?.filename, 'project_SV_export.csv');
	} finally {
		globalThis.fetch = realFetch;
	}
});

test('a rejection stays an envelope and carries its code', async () => {
	let requested = '';
	try {
		globalThis.fetch = (async (url: unknown) => {
			requested = String(url);
			return new Response(JSON.stringify({ code: 403, message: 'No access', data: null }), {
				status: 200,
				headers: { 'Content-Type': 'application/json; charset=utf-8' }
			});
		}) as typeof fetch;
		const result = await importExportApi.exportProject('p1', 'json');
		assert.notEqual(result.code, 0);
		assert.equal(result.data, null);
		assert.match(requested, /\/api\/v1\/projects\/p1\/export\?format=json$/);
	} finally {
		globalThis.fetch = realFetch;
	}
});
