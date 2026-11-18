import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { get } from 'svelte/store';
import { FlowObjectRepository } from '../src/lib/flow/object-repository';
import { flowApi, type FlowObjectView } from '../src/lib/api/flow';
import { sourceVersion, captureBlockShareReview } from '../src/lib/flow/shared-blocks/review';
import { structureEntries } from '../src/lib/flow/vendor/loro-prosemirror/page-structure';
import {
	FakeCollabServer,
	FakeConnectivity,
	freshIndexedDb,
	installFakes,
	objectView,
	OBJECT_ID,
	WORKSPACE_ID
} from './support/collab-fakes';
test('clean reopened Local source is reviewed only against a fresh matching server frontier; mismatched head is refused', async () => {
	await freshIndexedDb();
	const server = new FakeCollabServer();
	server.doc.import(
		await readFile(new URL('./fixtures/page-v4/rust-rich.snapshot', import.meta.url))
	);
	const restore = await installFakes(server),
		original = flowApi.getObject;
	const repo = new FlowObjectRepository({
		currentUserId: () => 'source-review-fixture',
		session: { connectivity: new FakeConnectivity(), answerTimeoutMs: 1000 }
	});
	try {
		const entry = await repo.open({
			scopeId: WORKSPACE_ID,
			objectId: OBJECT_ID,
			signal: new AbortController().signal,
			object: objectView() as unknown as FlowObjectView
		});
		assert.equal(get(entry.session.state), 'local');
		assert.equal(get(entry.pending), 0);
		let fresh = 0;
		flowApi.getObject = async () => {
			fresh++;
			return {
				code: 0,
				message: 'ok',
				data: { ...objectView(), frontier: sourceVersion(entry.doc) } as unknown as FlowObjectView
			};
		};
		const abort = new AbortController();
		const timeout = setTimeout(() => abort.abort(), 150);
		const reviewed = await repo.reviewBlockShare(
			OBJECT_ID,
			structureEntries(entry.doc)[0].block.id,
			'block',
			abort.signal
		);
		clearTimeout(timeout);
		assert(reviewed.ok);
		assert.equal(fresh, 1);
		flowApi.getObject = async () => ({
			code: 0,
			message: 'ok',
			data: { ...objectView(), frontier: 'different-accepted-head' } as unknown as FlowObjectView
		});
		const rejected = await repo.reviewBlockShare(
			OBJECT_ID,
			structureEntries(entry.doc)[0].block.id,
			'block',
			new AbortController().signal
		);
		assert.equal(rejected.ok, false);
		if (!rejected.ok) assert.equal(rejected.reason, 'source_changed');
	} finally {
		flowApi.getObject = original;
		await repo.disposeAll();
		restore();
	}
});

test('pending unconfirmed local operations cannot be reviewed even if a getter could claim a matching head', async () => {
	await freshIndexedDb();
	const server = new FakeCollabServer();
	server.doc.import(
		await readFile(new URL('./fixtures/page-v4/rust-rich.snapshot', import.meta.url))
	);
	const restore = await installFakes(server),
		original = flowApi.getObject,
		connectivity = new FakeConnectivity();
	const repo = new FlowObjectRepository({
		currentUserId: () => 'pending-review-fixture',
		session: { connectivity, answerTimeoutMs: 1000 }
	});
	try {
		const entry = await repo.open({
			scopeId: WORKSPACE_ID,
			objectId: OBJECT_ID,
			signal: new AbortController().signal,
			object: objectView() as unknown as FlowObjectView
		});
		connectivity.goOffline();
		entry.doc.getMap('data').set('unconfirmed_review_fixture', 'Must remain pending');
		entry.doc.commit();
		for (let i = 0; i < 30 && get(entry.pending) === 0; i++)
			await new Promise((resolve) => setTimeout(resolve, 0));
		assert(get(entry.pending) > 0);
		let reads = 0;
		flowApi.getObject = async () => {
			reads++;
			return {
				code: 0,
				message: 'ok',
				data: { ...objectView(), frontier: sourceVersion(entry.doc) } as unknown as FlowObjectView
			};
		};
		const abort = new AbortController(),
			pending = repo.reviewBlockShare(
				OBJECT_ID,
				structureEntries(entry.doc)[0].block.id,
				'block',
				abort.signal
			);
		await new Promise((resolve) => setTimeout(resolve, 30));
		assert.equal(reads, 0);
		abort.abort();
		const result = await pending;
		assert.equal(result.ok, false);
		if (!result.ok) assert.equal(result.reason, 'not_saved');
		assert(get(entry.pending) > 0);
	} finally {
		flowApi.getObject = original;
		await repo.disposeAll();
		restore();
	}
});

test('Rust FxHashMap VV encodings may differ while native peer/counter comparison remains exactly equal', async () => {
	const { VersionVector } = await import('loro-crdt');
	const { nativeVersionMatches } = await import('../src/lib/flow/shared-blocks/review');
	const first = 'AwECAgQDBg==',
		same = 'AwMGAgQBAg==';
	assert.notEqual(first, same);
	const local = VersionVector.decode(Uint8Array.from(atob(first), (char) => char.charCodeAt(0)));
	try {
		assert(nativeVersionMatches(local, same));
		assert.equal(nativeVersionMatches(local, 'AA=='), false);
		assert.equal(nativeVersionMatches(local, 'invalid opaque token'), false);
	} finally {
		local.free();
	}
});

test('actual source review accepts reordered equivalent native VV and preserves its opaque CAS token, but rejects a changed counter', async () => {
	const { VersionVector } = await import('loro-crdt');
	const { sourceFrontierMatches } = await import('../src/lib/flow/shared-blocks/review');
	const doc = new (await import('loro-crdt')).LoroDoc() as import('loro-prosemirror').LoroDocType;
	doc.import(await readFile(new URL('./fixtures/page-v4/rust-rich.snapshot', import.meta.url)));
	doc.setPeerId('999777');
	doc.getMap('data').set('vv_order_fixture', 'synthetic');
	doc.commit();
	const vv = doc.version(),
		changed = VersionVector.decode(vv.encode());
	const entries = [...vv.toJSON()].reverse();
	const variable = (value: bigint) => {
		const bytes: number[] = [];
		do {
			let byte = Number(value & 127n);
			value >>= 7n;
			if (value) byte |= 128;
			bytes.push(byte);
		} while (value);
		return bytes;
	};
	const bytes = Uint8Array.from([
		...variable(BigInt(entries.length)),
		...entries.flatMap(([peer, counter]) => [
			...variable(BigInt(peer)),
			...variable(BigInt(counter) * 2n)
		])
	]);
	const reordered = btoa(Array.from(bytes, (byte) => String.fromCharCode(byte)).join(''));
	try {
		assert(sourceFrontierMatches(doc, reordered));
		assert.notEqual(reordered, sourceVersion(doc));
		const result = captureBlockShareReview(
			doc,
			structureEntries(doc)[0].block.id,
			'block',
			reordered
		);
		assert.equal(result.expected_frontier, reordered);
		const [firstPeer, firstCounter] = entries[0];
		changed.setEnd({ peer: firstPeer, counter: firstCounter + 1 });
		assert.equal(changed.compare(vv), 1);
		const advanced = btoa(
			Array.from(changed.encode(), (byte) => String.fromCharCode(byte)).join('')
		);
		assert.equal(sourceFrontierMatches(doc, advanced), false);
		assert.throws(() => captureBlockShareReview(doc, result.root_block_cid, 'block', advanced));
	} finally {
		changed.free();
		vv.free();
		doc.free();
	}
});
