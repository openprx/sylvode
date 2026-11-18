import { test } from 'node:test';
import assert from 'node:assert/strict';
import { get } from 'svelte/store';
import { LoroDoc } from 'loro-crdt';
import { atomicEditorWrite } from '../src/lib/flow/editor-batch';
import { FlowObjectRepository } from '../src/lib/flow/object-repository';
import { pageUndoManager } from '../src/lib/flow/editor-session';
import { IndexedDbOutboxStore } from '../src/lib/flow/outbox';
import type { FlowObjectView } from '../src/lib/api/flow';
import {
	FakeCollabServer,
	FakeConnectivity,
	freshIndexedDb,
	installFakes,
	objectView,
	OBJECT_ID,
	WORKSPACE_ID
} from './support/collab-fakes';
import { waitFor } from './support/harness';
test('real repository recovery preserves prior persisted outbox and local text, detaches the failed engine and exposes a valid recovery snapshot', async () => {
	const factory = await freshIndexedDb();
	const server = new FakeCollabServer();
	const restore = await installFakes(server);
	const store = await IndexedDbOutboxStore.open(factory, 'sv-editor-batch-recovery');
	assert(store);
	const connectivity = new FakeConnectivity();
	const repo = new FlowObjectRepository({
		outboxStore: store,
		currentUserId: () => 'editor-batch-fixture',
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
		await waitFor(() => get(entry.session.state) === 'offline', 'offline');
		const original = entry.doc;
		const manager = await pageUndoManager(original);
		let callbacks = 0;
		manager.setOnPush(() => {
			callbacks++;
			return { value: null, cursors: [] };
		});
		original.getMap('data').set('prior_unsent', 'Preserve earlier 中文');
		original.commit();
		await waitFor(() => get(entry.pending) === 1, 'prior update persisted');
		await entry.offline.idle();
		const queueBefore = await store.list(OBJECT_ID);
		const jsonBefore = original.toJSON();
		assert.throws(() =>
			atomicEditorWrite(
				original,
				() => {
					original.getMap('data').set('failed', 'never sent');
					original.commit({ origin: 'sys:storage' });
					throw new Error('Injected failure');
				},
				{ protectStorage: true, undoManager: manager }
			)
		);
		assert.notEqual(entry.doc, original);
		assert.deepEqual(entry.doc.toJSON(), jsonBefore);
		assert.equal(get(entry.readOnly), true);
		assert.equal(get(entry.docGeneration), 1);
		assert.equal(
			(get(entry.flowError)?.details as { reason?: string }).reason,
			'editor_transaction_aborted'
		);
		assert.equal(entry.session.isConnected, false);
		assert.throws(() =>
			atomicEditorWrite(entry.doc, () => entry.doc.getMap('data').set('new', 'blocked'))
		);
		const afterCallbacks = callbacks;
		original.getMap('data').set('late', 'retired');
		original.commit();
		assert.equal(callbacks, afterCallbacks);
		await entry.offline.idle();
		const queueAfter = await store.list(OBJECT_ID);
		assert.deepEqual(
			queueAfter.map((item) => item.updateId),
			queueBefore.map((item) => item.updateId)
		);
		assert.deepEqual(
			queueAfter.map((item) => Array.from(item.bytes)),
			queueBefore.map((item) => Array.from(item.bytes))
		);
		const exported = JSON.parse(await (await repo.exportRecoveryDraft(OBJECT_ID)).text());
		assert.equal(exported.undo_history_reset, true);
		const snapshot = new LoroDoc();
		snapshot.import(
			Uint8Array.from(atob(exported.preserved_snapshot_base64), (char) => char.charCodeAt(0))
		);
		assert.deepEqual(snapshot.toJSON(), jsonBefore);
		snapshot.free();
	} finally {
		await repo.disposeAll();
		restore();
	}
});
