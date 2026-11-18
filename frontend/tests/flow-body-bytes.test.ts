import { test } from 'node:test';
import assert from 'node:assert/strict';
import { bodyBytesValid, bodyDownloadName } from '../src/lib/flow/body/bytes';
const base = () =>
	new Headers({ 'X-Flow-Asset-SHA256': 'a'.repeat(64), 'X-Content-Type-Options': 'nosniff' });
test('only authoritative four image MIME types render inline; attachments require octet-stream and forced download', () => {
	const headers = base();
	headers.set('Content-Type', 'image/png');
	assert(bodyBytesValid('image', new Blob(['owned']), headers));
	headers.set('Content-Type', 'image/svg+xml');
	assert.equal(bodyBytesValid('image', new Blob(['<script>']), headers), false);
	headers.set('Content-Type', 'application/json');
	headers.set('Content-Disposition', 'attachment; filename="safe.json"');
	assert.equal(bodyBytesValid('attachment', new Blob(['{}']), headers), false);
	headers.set('Content-Type', 'application/octet-stream');
	assert(bodyBytesValid('attachment', new Blob(['{}']), headers));
	headers.delete('Content-Disposition');
	assert.equal(bodyBytesValid('attachment', new Blob(['{}']), headers), false);
});
test('download filename comes only from the response header and cannot carry a filesystem path', () => {
	const headers = base();
	headers.set('Content-Disposition', "attachment; filename*=UTF-8''note%20%E4%B8%AD.json");
	assert.equal(bodyDownloadName(headers), 'note 中.json');
	headers.set('Content-Disposition', 'attachment; filename="../../unsafe.html"');
	assert.equal(bodyDownloadName(headers), '.._.._unsafe.html');
});

test('real new backend JSON-attachment headers preserve download bytes and authoritative UTF8 filename', async () => {
	const { readFile } = await import('node:fs/promises');
	const fixture = JSON.parse(
		await readFile(new URL('./fixtures/page-v5/asset-headers.json', import.meta.url), 'utf8')
	);
	const headers = new Headers(fixture.headers);
	const blob = new Blob(['{"hello":"world"}'], { type: headers.get('content-type')! });
	assert.equal(blob.size, 17);
	assert(bodyBytesValid('attachment', blob, headers));
	assert.equal(bodyDownloadName(headers), '说明.json');
	const digest = Array.from(
		new Uint8Array(await crypto.subtle.digest('SHA-256', await blob.arrayBuffer())),
		(value) => value.toString(16).padStart(2, '0')
	).join('');
	assert.equal(digest, headers.get('x-flow-asset-sha256'));
	assert.equal(await blob.text(), '{"hello":"world"}');
});
