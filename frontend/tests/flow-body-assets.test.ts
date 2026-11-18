import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
	BodyAssetUpload,
	type BodyAssetTransport,
	type BodyAssetReceipt
} from '../src/lib/flow/body/assets';
const page = '11111111-1111-4111-8111-111111111111',
	asset = '22222222-2222-4222-8222-222222222222';
const file = () => new File(['fixture'], 'fixture.png', { type: 'image/png' });
const receipt = (f: File): BodyAssetReceipt => ({
	asset_id: asset,
	object_id: page,
	kind: 'image',
	filename: f.name,
	content_type: 'image/png',
	byte_size: f.size,
	sha256: 'a'.repeat(64),
	state: 'staged',
	read_path: `/api/v2/flow/objects/${page}/body-assets/${asset}`
});
const ok = <T>(data: T) => ({ code: 0, message: 'ok', data });
const limits = {
	max_bytes: 1024,
	image_content_types: ['image/png', 'image/jpeg', 'image/webp', 'image/gif'],
	upload_transport: 'multipart' as const,
	editor_model: 5 as const
};
test('upload reports progress, retries the same intent and retains the captured page/bookmark/file', async () => {
	const source = file(),
		keys: string[] = [];
	let attempts = 0;
	const transport: BodyAssetTransport = {
		limits: async () => ok(limits),
		upload: async (id, kind, f, key, _signal, progress) => {
			assert.equal(id, page);
			assert.equal(kind, 'image');
			keys.push(key);
			progress({ loaded: 3, total: f.size });
			if (!attempts++) return { code: 500, message: 'offline', data: null };
			return ok(receipt(f));
		},
		discard: async () => ok({})
	};
	const job = new BodyAssetUpload(
		page,
		'image',
		source,
		{ blockId: 'cid:source', side: 'after' },
		transport
	);
	const states: string[] = [];
	job.subscribe((state) => states.push(state.phase));
	assert.equal(await job.start(), null);
	assert.equal(job.state.phase, 'failed');
	assert.equal(job.file, source);
	assert.equal(job.bookmark.blockId, 'cid:source');
	assert.equal((await job.start())?.asset_id, asset);
	assert.equal(keys[0], keys[1]);
	assert.equal(job.state.phase, 'staged');
	assert.equal(job.state.loaded, source.size);
	assert(states.includes('uploading'));
});
test('cancel aborts upload; late stage stays available for the same job without inserting or silently deleting its retry asset', async () => {
	const source = file();
	let resolve!: (value: ReturnType<typeof ok<BodyAssetReceipt>>) => void;
	let aborted = false,
		discarded = 0;
	const transport: BodyAssetTransport = {
		limits: async () => ok(limits),
		upload: async (_id, _kind, _file, _key, signal) => {
			signal.addEventListener('abort', () => (aborted = true));
			return new Promise((done) => (resolve = done));
		},
		discard: async () => {
			discarded++;
			return ok({});
		}
	};
	const job = new BodyAssetUpload(
		page,
		'image',
		source,
		{ blockId: 'cid:old-page-target', side: 'after' },
		transport
	);
	const pending = job.start();
	await Promise.resolve();
	await Promise.resolve();
	job.cancel();
	resolve(ok(receipt(source)));
	await pending;
	assert(aborted);
	assert.equal(job.state.phase, 'canceled');
	assert.equal(job.state.receipt?.asset_id, asset);
	assert.equal(discarded, 0);
	assert.equal(job.bookmark.blockId, 'cid:old-page-target');
	assert.equal((await job.start())?.asset_id, asset);
	assert.equal(await job.discard(), true);
	assert.equal(discarded, 1);
});
test('limits and foreign-page receipts fail without discarding the user file or producing a durable reference', async () => {
	const source = file();
	let calls = 0;
	const transport: BodyAssetTransport = {
		limits: async () => ok({ ...limits, max_bytes: 1 }),
		upload: async () => {
			calls++;
			return ok(receipt(source));
		},
		discard: async () => ok({})
	};
	const oversized = new BodyAssetUpload(
		page,
		'image',
		source,
		{ blockId: 'cid:target', side: 'before' },
		transport
	);
	assert.equal(await oversized.start(), null);
	assert.equal(calls, 0);
	assert.equal(oversized.state.error, 'size');
	const foreign = new BodyAssetUpload(
		page,
		'image',
		source,
		{ blockId: 'cid:target', side: 'after' },
		{
			...transport,
			limits: async () => ok(limits),
			upload: async () => ok({ ...receipt(source), object_id: 'another-page' })
		}
	);
	assert.equal(await foreign.start(), null);
	assert.equal(foreign.state.error, 'upload');
	assert.equal(foreign.file, source);
	assert.equal(foreign.state.receipt, null);
});
