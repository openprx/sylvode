import { test } from 'node:test';
import assert from 'node:assert/strict';
import { PageBodyJobs, type BodyJobHost } from '../src/lib/flow/body/jobs';
import type { BodyAssetReceipt, BodyAssetTransport } from '../src/lib/flow/body/assets';
const page = '11111111-1111-4111-8111-111111111111',
	asset = '22222222-2222-4222-8222-222222222222',
	child = '33333333-3333-4333-8333-333333333333';
const file = new File(['own-fixture'], 'fixture.png', { type: 'image/png' });
const ok = <T>(data: T) => ({ code: 0, message: 'ok', data });
const receipt: BodyAssetReceipt = {
	asset_id: asset,
	object_id: page,
	kind: 'image',
	filename: file.name,
	content_type: 'image/png',
	byte_size: file.size,
	sha256: 'a'.repeat(64),
	state: 'staged',
	read_path: `/api/v2/flow/objects/${page}/body-assets/${asset}`
};
const reservation = {
	state: 'pending' as const,
	reservation_id: asset,
	child_page_id: child,
	source_object_id: page,
	title: 'Child draft',
	expires_at: '2099-01-01T00:00:00Z'
};
function deferred<T>() {
	let resolve!: (value: T) => void;
	return { promise: new Promise<T>((done) => (resolve = done)), resolve };
}
async function until(check: () => boolean) {
	for (let i = 0; i < 30 && !check(); i++) await new Promise((resolve) => setTimeout(resolve, 0));
	assert(check());
}
function host(patch: Partial<BodyJobHost> = {}): BodyJobHost {
	return {
		objectId: page,
		allowed: () => true,
		ensure: async () => true,
		contains: async () => true,
		failureReason: () => null,
		insert: async () => ({ blockId: 'cid:new' }),
		saved: async () => true,
		prepare: async () => ok(reservation),
		cancelReservation: async () => ok({}),
		reservationStatus: async () => ok(reservation),
		childReady: () => {},
		...patch
	};
}
function transport(patch: Partial<BodyAssetTransport> = {}): BodyAssetTransport {
	return {
		limits: async () =>
			ok({
				max_bytes: 4096,
				image_content_types: ['image/png'],
				upload_transport: 'multipart',
				editor_model: 5
			}),
		upload: async () => ok(receipt),
		discard: async () => ok({}),
		...patch
	};
}
test('a background upload keeps its original Page and bookmark after UI unsubscription; repeated retry does not duplicate insertion', async () => {
	const upload = deferred<ReturnType<typeof ok<BodyAssetReceipt>>>();
	const targets: string[] = [];
	let pending = 0;
	const jobs = new PageBodyJobs(
		host({
			insert: async (bookmark) => {
				targets.push(`${page}:${bookmark.blockId}`);
				return { blockId: 'cid:inserted' };
			}
		}),
		transport({ upload: async () => upload.promise })
	);
	const release = jobs.subscribe((value) => (pending = value.pending));
	const job = jobs.upload('image', file, { blockId: 'cid:original-page', side: 'after' });
	assert.equal(pending, 1);
	release();
	void jobs.retryUpload(job);
	upload.resolve(ok(receipt));
	await until(() => targets.length === 1 && job.state.phase === 'complete');
	assert.deepEqual(targets, [`${page}:cid:original-page`]);
	jobs.dispose();
});
test('lost position retains staged bytes until an explicit new bookmark; retries do not upload again', async () => {
	let uploads = 0;
	const targets: (string | null)[] = [];
	const jobs = new PageBodyJobs(
		host({
			insert: async (bookmark) => {
				targets.push(bookmark.blockId);
				return bookmark.blockId === 'cid:gone' ? null : { blockId: 'cid:new' };
			}
		}),
		transport({
			upload: async () => {
				uploads++;
				return ok(receipt);
			}
		})
	);
	const job = jobs.upload('image', file, { blockId: 'cid:gone', side: 'after' });
	await until(() => job.state.error === 'position');
	assert.equal(job.state.receipt?.asset_id, asset);
	assert.deepEqual(targets, ['cid:gone']);
	jobs.repositionUpload(job, { blockId: 'cid:chosen-by-user', side: 'after' });
	await until(() => job.state.phase === 'complete');
	assert.equal(uploads, 1);
	assert.deepEqual(targets, ['cid:gone', 'cid:chosen-by-user']);
	jobs.dispose();
});
test('new child stays pending through raw acknowledgement and never stores title/reservation in its canonical atom', async () => {
	const saved = deferred<boolean>();
	let attrs: Record<string, string> | null = null,
		ready = '';
	const jobs = new PageBodyJobs(
		host({
			insert: async (_bookmark, intent) => {
				attrs = intent.attrs;
				return { blockId: 'cid:child-ref' };
			},
			saved: async () => saved.promise,
			childReady: (id) => (ready = id)
		}),
		transport()
	);
	jobs.createChild('Child draft', { blockId: 'cid:source', side: 'after' });
	await until(() => attrs !== null);
	assert.deepEqual(attrs, { child_page_id: child });
	assert.equal(jobs.pendingTitle(child), 'Child draft');
	assert.equal(ready, '');
	saved.resolve(true);
	await until(() => ready === child);
	assert.equal(jobs.pendingTitle(child), null);
	jobs.dispose();
});
test('closing an unused child intent before prepare replies cancels the reservation without inserting a Page ref', async () => {
	const prepared = deferred<ReturnType<typeof ok<typeof reservation>>>();
	let inserted = 0,
		canceled = 0;
	const jobs = new PageBodyJobs(
		host({
			prepare: async () => prepared.promise,
			insert: async () => {
				inserted++;
				return { blockId: 'bad' };
			},
			cancelReservation: async () => {
				canceled++;
				return ok({});
			}
		}),
		transport()
	);
	const job = jobs.createChild('Child draft', { blockId: 'cid:source', side: 'after' });
	await Promise.resolve();
	await jobs.cancelChild(job);
	prepared.resolve(ok(reservation));
	await until(() => canceled === 1);
	assert.equal(inserted, 0);
	jobs.dispose();
});

test('job notifications expose fresh render snapshots while actions retain canonical task identity', async () => {
	const snapshots: import('../src/lib/flow/body/jobs').BodyJobsSnapshot[] = [];
	const jobs = new PageBodyJobs(
		host(),
		transport({ upload: async () => ({ code: 400, message: 'invalid image', data: null }) })
	);
	jobs.subscribe((snapshot) => snapshots.push(snapshot));
	const job = jobs.upload('image', file, { blockId: 'cid:target', side: 'after' });
	await until(() => job.state.phase === 'failed');
	const rows = snapshots.flatMap((snapshot) => snapshot.uploads);
	const uploading = rows.find((row) => row.state.phase === 'uploading')!;
	const failed = rows.find((row) => row.state.phase === 'failed')!;
	assert(uploading && failed);
	assert.equal(uploading.job, job);
	assert.equal(failed.job, job);
	assert.notEqual(uploading, failed);
	assert.notEqual(uploading.state, failed.state);
	assert.equal(uploading.state.phase, 'uploading');
	assert.equal(failed.state.phase, 'failed');
	jobs.dispose();
});
